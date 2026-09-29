//! Column types for materialized queries, including compound SELECT/VALUES.
use super::*;

#[derive(Clone)]
pub(in super::super) struct ColumnType {
    pub affinity: Affinity,
    pub collation: Collation,
    pub declared_type: String,
}
impl ColumnType {
    pub(super) fn expression(expr: &Expr, fields: &[Field]) -> Self {
        Self {
            affinity: eval::expr_affinity(expr),
            collation: eval::collation(expr),
            declared_type: eval::declared_type(expr, fields),
        }
    }
}
#[derive(Clone)]
struct Column {
    first: ColumnType,
    affinity: Affinity,
    other_types: u8,
    first_cast: bool,
    selected_types: u8,
    comparison_collation: Option<Collation>,
}
#[derive(Clone, Default)]
pub(in super::super) struct CompoundTypes {
    columns: Vec<Column>,
    terms: usize,
}
impl CompoundTypes {
    pub(super) fn add(
        &mut self,
        expressions: &[Expr],
        fields: &[Field],
        context: &mut Eval<'_>,
    ) -> Result<()> {
        if self.terms == 0 {
            self.columns = expressions
                .iter()
                .map(|e| {
                    let first = ColumnType::expression(e, fields);
                    Column {
                        affinity: first.affinity,
                        first,
                        other_types: if eval::expr_affinity(e) == Affinity::None {
                            possible_types(e)
                        } else {
                            0
                        },
                        first_cast: matches!(e.kind, ExprKind::Cast(..)),
                        selected_types: if eval::expr_affinity(e) == Affinity::None {
                            0
                        } else {
                            possible_types(e)
                        },
                        comparison_collation: eval::collation_hint(e),
                    }
                })
                .collect();
        } else {
            if expressions.len() != self.columns.len() {
                return Err(error("compound SELECTs have different column counts"));
            }
            for (column, expr) in self.columns.iter_mut().zip(expressions) {
                context.fuel.spend()?;
                let affinity = eval::expr_affinity(expr);
                if column.affinity == Affinity::None && affinity != Affinity::None {
                    column.affinity = affinity;
                    column.selected_types = possible_types(expr);
                } else {
                    column.other_types |= possible_types(expr);
                }
                column.comparison_collation = column
                    .comparison_collation
                    .or_else(|| eval::collation_hint(expr));
            }
        }
        self.terms += 1;
        Ok(())
    }
    pub(super) fn add_data(&mut self, data: &Data, context: &mut Eval<'_>) -> Result<()> {
        let Some(types) = &data.column_types else {
            return self.add(&data.projection, &data.fields, context);
        };
        if self.terms == 0 {
            *self = types.clone();
            return Ok(());
        }
        if self.columns.len() != types.columns.len() {
            return Err(error("compound SELECTs have different column counts"));
        }
        for (left, right) in self.columns.iter_mut().zip(&types.columns) {
            context.fuel.spend()?;
            left.other_types |= right.other_types;
            if left.affinity == Affinity::None && right.affinity != Affinity::None {
                left.affinity = right.affinity;
                left.selected_types = right.selected_types;
            } else {
                left.other_types |= right.selected_types;
            }
            left.comparison_collation = left.comparison_collation.or(right.comparison_collation);
        }
        self.terms += types.terms;
        Ok(())
    }
    pub(super) fn comparison_collations(&self) -> Vec<Option<Collation>> {
        self.columns
            .iter()
            .map(|c| c.comparison_collation)
            .collect()
    }
    pub(super) fn columns(&self) -> Vec<ColumnType> {
        self.columns
            .iter()
            .map(|column| {
                let mut affinity = column.affinity;
                if (affinity == Affinity::Text && column.other_types & 1 != 0)
                    || (affinity.numeric() && column.other_types & 2 != 0)
                {
                    affinity = Affinity::Blob;
                }
                if self.terms > 1 && affinity.numeric() && column.first_cast {
                    affinity = Affinity::FlexNumeric;
                }
                ColumnType {
                    affinity,
                    declared_type: eval::type_for_affinity(
                        column.first.declared_type.clone(),
                        affinity,
                    ),
                    collation: column.first.collation,
                }
            })
            .collect()
    }
}

// Bit 0: numeric, bit 1: text, bit 2: blob. NULL adds no possible type.
// This intentionally reasons about expressions, not the values in current rows.
fn possible_types(expr: &Expr) -> u8 {
    use parser::{Binary, Unary};
    match &expr.kind {
        ExprKind::Literal(Value::Null) => 0,
        ExprKind::Literal(Value::Text(_)) => 2,
        ExprKind::Literal(Value::Blob(_)) => 4,
        ExprKind::Collate(e, _) | ExprKind::Unary(Unary::Plus, e) => possible_types(e),
        ExprKind::Binary(Binary::Concat, ..) => 6,
        ExprKind::Parameter(_) | ExprKind::Call { .. } => 7,
        ExprKind::Slot(..) | ExprKind::Outer(..) | ExprKind::Cast(..) => {
            affinity_types(eval::expr_affinity(expr))
        }
        ExprKind::BoundSubquery(q) if q.source.mode == parser::QueryMode::Scalar => {
            affinity_types(q.affinity)
        }
        ExprKind::Case(_, branches, otherwise) => branches.iter().fold(
            otherwise.as_ref().map_or(0, |e| possible_types(e)),
            |types, (_, value)| types | possible_types(value),
        ),
        _ => 1,
    }
}
fn affinity_types(affinity: Affinity) -> u8 {
    if affinity.numeric() {
        5
    } else if affinity == Affinity::Text {
        6
    } else {
        7
    }
}
