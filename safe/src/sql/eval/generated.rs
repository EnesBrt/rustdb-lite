//! Shared, table-local generated expressions. References remain column leaves
//! for affinity/collation/aggregate inference; reading a virtual value uses a
//! bounded dependency evaluator instead of expanding a potentially exponential
//! expression tree into each projection and correlated scope.
use super::*;
use alloc::{rc::Rc, vec};

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub expression: Option<Expr>,
    pub virtual_column: bool,
    pub affinity: Affinity,
    pub collation: Collation,
    pub declared_type: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub columns: Vec<Column>,
    pub read_depth: Vec<Option<usize>>,
    pub write_order: Option<Vec<usize>>,
    pub strict: bool,
    /// A NOT NULL primary key or the hidden rowid distinguishes an outer-join
    /// NULL row from an actual row whose ordinary columns are all NULL.
    pub presence: usize,
    pub width: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    pub schema: Rc<Schema>,
    pub column: usize,
    pub offset: usize,
    pub outer: Option<usize>,
}
impl Reference {
    pub fn metadata(&self) -> &Column {
        &self.schema.columns[self.column]
    }
    pub fn expression(&self, slot: usize, outer: Option<usize>) -> Result<Expr> {
        let depth =
            self.schema.read_depth[self.column].ok_or_else(|| error("generated column loop"))?;
        if depth > 64 {
            return Err(Error::Limit("generated expression depth"));
        }
        let mut reference = self.clone();
        reference.offset = slot
            .checked_sub(self.column)
            .ok_or(Error::Corrupt("generated column offset"))?;
        reference.outer = outer;
        Ok(Expr {
            location: Default::default(),
            token: None,
            kind: ExprKind::Generated(reference),
            depth,
        })
    }
    pub fn read(
        &self,
        row: &[Value],
        queries: &mut dyn Subqueries,
        context: &mut Eval<'_>,
    ) -> Result<Value> {
        let captured;
        let row = if let Some(frame) = self.outer {
            let mut values = Vec::new();
            let mut bytes = 0usize;
            for i in 0..self.schema.width {
                context.fuel.spend()?;
                let value = queries.outer(frame, self.offset + i)?;
                bytes = bytes
                    .checked_add(scalar::size(&value) + core::mem::size_of::<Value>())
                    .ok_or(Error::Limit("generated outer row bytes"))?;
                if bytes > context.limits.max_database_bytes {
                    return Err(Error::Limit("generated outer row bytes"));
                }
                values.push(value);
            }
            captured = values;
            captured.as_slice()
        } else {
            row.get(self.offset..).unwrap_or(&[])
        };
        if row.get(self.schema.presence).is_none_or(scalar::null) {
            return Ok(Value::Null);
        }
        Values {
            schema: &self.schema,
            active: vec![false; self.schema.columns.len()],
            depth: 0,
        }
        .slot(self.column, row, context)
    }
}
struct Values<'a> {
    schema: &'a Schema,
    active: Vec<bool>,
    depth: usize,
}
impl Subqueries for Values<'_> {
    fn slot(&mut self, slot: usize, row: &[Value], context: &mut Eval<'_>) -> Result<Value> {
        let Some(column) = self.schema.columns.get(slot).filter(|c| c.virtual_column) else {
            return Ok(row.get(slot).cloned().unwrap_or(Value::Null));
        };
        if self.active[slot] {
            return Err(error("generated column loop"));
        }
        if self.depth >= context.limits.max_expr_depth.min(64) {
            return Err(Error::Limit("generated expression depth"));
        }
        self.active[slot] = true;
        self.depth += 1;
        let expr = column
            .expression
            .as_ref()
            .ok_or(Error::Corrupt("generated expression missing"))?;
        let value = scalar::affinity(context.eval_with(expr, row, None, self)?, column.affinity)?;
        if self.schema.strict {
            typecheck(&value, &column.declared_type)?;
        }
        self.depth -= 1;
        self.active[slot] = false;
        Ok(value)
    }
    fn outer(&self, _: usize, _: usize) -> Result<Value> {
        Err(error("outer reference in generated expression"))
    }
    fn run(
        &mut self,
        _: &BoundSubquery,
        _: &[Value],
        _: &mut Eval<'_>,
    ) -> Result<Rc<Vec<Vec<Value>>>> {
        Err(error("subquery in generated expression"))
    }
}
pub fn typecheck(value: &Value, typ: &str) -> Result<()> {
    if scalar::null(value)
        || match typ {
            "ANY" => true,
            "INT" | "INTEGER" => matches!(value, Value::Integer(_)),
            "REAL" => matches!(value, Value::Real(_)),
            "TEXT" => matches!(value, Value::Text(_)),
            "BLOB" => matches!(value, Value::Blob(_)),
            _ => false,
        }
    {
        Ok(())
    } else {
        Err(Error::Datatype(format!("generated column requires {typ}")))
    }
}

pub fn field(field: &Field, slot: usize, outer: Option<usize>) -> Result<Expr> {
    if let Some(reference) = &field.generated {
        reference.expression(slot, outer)
    } else {
        Ok(Expr {
            location: Default::default(),
            token: None,
            depth: 1,
            kind: match outer {
                Some(frame) => ExprKind::Outer(
                    frame,
                    slot,
                    field.affinity,
                    field.collation,
                    field.declared_type.clone(),
                ),
                None => ExprKind::Slot(slot, field.affinity, field.collation),
            },
        })
    }
}
