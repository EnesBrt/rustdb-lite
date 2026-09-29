use super::{
    error,
    lexer::{lex, Kind, Token},
    scalar::{Affinity, Collation},
    SqlLimits,
};
use crate::{Error, Result, Text, Value};
use alloc::{
    boxed::Box,
    format,
    rc::Rc,
    string::{String, ToString},
    vec,
    vec::Vec,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub depth: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Literal(Value),
    /// Decimal integer token 2^63. A syntactic unary minus can still form MIN.
    MinMagnitude,
    Boolean(bool),
    Column {
        qualifier: Option<String>,
        name: String,
        quoted: bool,
    },
    Slot(usize, Affinity, Collation),
    Outer(usize, usize, Affinity, Collation, String),
    Subquery(Subquery),
    BoundSubquery(Box<super::eval::BoundSubquery>),
    InQuery(Box<Expr>, Box<Expr>, bool),
    Parameter(usize),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Between(Box<Expr>, Box<Expr>, Box<Expr>, bool),
    In(Box<Expr>, Vec<Expr>, bool),
    Case(Option<Box<Expr>>, Vec<(Expr, Expr)>, Option<Box<Expr>>),
    Cast(Box<Expr>, Affinity),
    Collate(Box<Expr>, Collation),
    Call {
        name: String,
        args: Vec<Expr>,
        star: bool,
        distinct: bool,
    },
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QueryMode {
    Scalar,
    Exists,
    Set,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Subquery {
    pub id: usize,
    pub query: Rc<Query>,
    pub mode: QueryMode,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unary {
    Plus,
    Minus,
    Not,
    BitNot,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Concat,
    BitAnd,
    BitOr,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Is,
    IsNot,
    And,
    Or,
    Like,
    NotLike,
}
impl Expr {
    pub fn literal(value: Value) -> Self {
        Self {
            kind: ExprKind::Literal(value),
            depth: 1,
        }
    }
    pub fn children(&self) -> Vec<&Expr> {
        match &self.kind {
            ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Collate(x, _) => vec![x],
            ExprKind::Binary(_, a, b) | ExprKind::InQuery(a, b, _) => vec![a, b],
            ExprKind::Between(a, b, c, _) => vec![a, b, c],
            ExprKind::In(x, values, _) => {
                core::iter::once(x.as_ref()).chain(values.iter()).collect()
            }
            ExprKind::Case(base, arms, other) => base
                .iter()
                .map(Box::as_ref)
                .chain(arms.iter().flat_map(|(a, b)| [a, b]))
                .chain(other.iter().map(Box::as_ref))
                .collect(),
            ExprKind::Call { args, .. } => args.iter().collect(),
            _ => Vec::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub declared_type: String,
    pub affinity: Affinity,
    pub collation: Collation,
    pub collation_name: String,
    pub not_null: bool,
    pub not_null_conflict: Conflict,
    pub primary: bool,
    pub primary_desc: bool,
    pub unique: bool,
    pub primary_conflict: Conflict,
    pub unique_conflict: Conflict,
    pub index_desc: bool,
    pub default: Option<Expr>,
    pub default_sql: Option<String>,
    pub checks: Vec<Expr>,
}
impl Column {
    pub fn rowid_alias(&self) -> bool {
        self.primary && !self.primary_desc && self.declared_type.eq_ignore_ascii_case("INTEGER")
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct SelectItem {
    pub expr: Option<Expr>,
    pub star: Option<String>,
    pub alias: Option<String>,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub name: String,
    pub qualified: bool,
    pub query: Option<Box<Query>>,
    pub alias: String,
    pub left: bool,
    pub on: Option<Expr>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Ordering {
    pub expr: Expr,
    pub descending: bool,
    pub nulls_first: Option<bool>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Select {
    pub items: Vec<SelectItem>,
    pub sources: Vec<Source>,
    pub filter: Option<Expr>,
    pub group: Vec<Expr>,
    pub having: Option<Expr>,
    pub order: Vec<Ordering>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
    pub distinct: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Compound {
    UnionAll,
    Union,
    Intersect,
    Except,
}
#[derive(Clone, Debug, PartialEq)]
pub enum QueryCore {
    Select(Box<Select>),
    Values(Vec<Vec<Expr>>),
}
#[derive(Clone, Debug, PartialEq)]
pub struct CommonTable {
    pub id: usize,
    pub name: String,
    pub columns: Option<Vec<String>>,
    pub query: Box<Query>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    pub with: Vec<CommonTable>,
    pub cores: Vec<QueryCore>,
    pub operators: Vec<Compound>,
    pub order: Vec<Ordering>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
}
#[derive(Clone, Debug)]
pub enum InsertSource {
    Values(Vec<Vec<Expr>>),
    Select(Box<Query>),
    Default,
}
#[derive(Clone, Debug)]
pub struct Upsert {
    pub target: Option<Vec<Expr>>,
    pub target_where: Option<Expr>,
    pub action: UpsertAction,
}
#[derive(Clone, Debug)]
pub enum UpsertAction {
    Nothing,
    Update {
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
    },
}
#[derive(Clone, Debug)]
pub struct IndexColumn {
    pub name: String,
    pub collation: Option<Collation>,
    pub collation_name: Option<String>,
    pub descending: bool,
}
#[derive(Clone, Debug)]
pub enum TableConstraint {
    Key {
        columns: Vec<IndexColumn>,
        primary: bool,
        conflict: Conflict,
    },
    Check(Expr),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conflict {
    #[default]
    Default,
    Rollback,
    Abort,
    Fail,
    Ignore,
    Replace,
}
impl Conflict {
    pub fn resolve(self, schema: Self) -> Self {
        match (self, schema) {
            (Self::Default, Self::Default) => Self::Abort,
            (Self::Default, policy) => policy,
            (policy, _) => policy,
        }
    }
    pub fn merge(self, other: Self) -> Result<Self> {
        match (self, other) {
            (Self::Default, policy) | (policy, Self::Default) => Ok(policy),
            (a, b) if a == b => Ok(a),
            _ => Err(error("conflicting ON CONFLICT clauses")),
        }
    }
}
#[derive(Clone, Debug)]
pub enum Statement {
    CreateView {
        name: String,
        columns: Option<Vec<String>>,
        query: Rc<Query>,
        sql: String,
        if_not_exists: bool,
    },
    DropView {
        name: String,
        if_exists: bool,
    },
    CreateAs {
        name: String,
        query: Box<Query>,
        if_not_exists: bool,
    },
    With {
        tables: Vec<CommonTable>,
        statement: Box<Statement>,
    },
    CreateIndex {
        name: String,
        table: String,
        columns: Vec<IndexColumn>,
        unique: bool,
        if_not_exists: bool,
        sql: String,
    },
    DropIndex {
        name: String,
        if_exists: bool,
    },
    Create {
        name: String,
        columns: Vec<Column>,
        constraints: Vec<TableConstraint>,
        if_not_exists: bool,
        sql: String,
    },
    Drop {
        name: String,
        if_exists: bool,
    },
    Insert {
        name: String,
        alias: Option<String>,
        columns: Option<Vec<String>>,
        source: InsertSource,
        conflict: Conflict,
        upserts: Vec<Upsert>,
    },
    Select(Box<Query>),
    Update {
        name: String,
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
        conflict: Conflict,
    },
    Delete {
        name: String,
        filter: Option<Expr>,
    },
    Begin,
    Commit,
    Rollback(Option<String>),
    Savepoint(String),
    Release(String),
    Pragma {
        name: String,
        value: Option<i64>,
        argument: Option<String>,
    },
}
impl Statement {
    pub fn mutating(&self) -> bool {
        match self {
            Self::With { statement, .. } => statement.mutating(),
            Self::Create { .. }
            | Self::CreateView { .. }
            | Self::DropView { .. }
            | Self::CreateAs { .. }
            | Self::Drop { .. }
            | Self::CreateIndex { .. }
            | Self::DropIndex { .. }
            | Self::Insert { .. }
            | Self::Update { .. }
            | Self::Delete { .. }
            | Self::Pragma { value: Some(_), .. } => true,
            _ => false,
        }
    }
    pub fn changes_rows(&self) -> bool {
        match self {
            Self::With { statement, .. } => statement.changes_rows(),
            Self::Insert { .. } | Self::Update { .. } | Self::Delete { .. } => true,
            _ => false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct PreparedStatement {
    pub(crate) statement: Statement,
    pub(crate) parameter_names: Vec<Option<String>>,
}
impl PreparedStatement {
    pub fn parameter_count(&self) -> usize {
        self.parameter_names.len()
    }
    pub fn parameter_index(&self, name: &str) -> Option<usize> {
        self.parameter_names
            .iter()
            .position(|n| n.as_deref() == Some(name))
            .map(|i| i + 1)
    }
}
struct Parser<'a> {
    sql: &'a str,
    tokens: Vec<Token>,
    at: usize,
    limits: SqlLimits,
    recursion: usize,
    parameters: Vec<Option<String>>,
}
pub fn parse(sql: &str, limits: SqlLimits) -> Result<Vec<PreparedStatement>> {
    let mut p = Parser {
        sql,
        tokens: lex(sql, limits)?,
        at: 0,
        limits,
        recursion: 0,
        parameters: Vec::new(),
    };
    let mut statements = Vec::new();
    loop {
        while p.eat(";") {}
        if matches!(p.peek(), Kind::End) {
            break;
        }
        p.parameters.clear();
        let statement = p.statement()?;
        if !matches!(p.peek(), Kind::End) && !p.is(";") {
            return Err(p.expected("end of statement (syntax may be unsupported)"));
        }
        statements.push(PreparedStatement {
            statement,
            parameter_names: core::mem::take(&mut p.parameters),
        });
    }
    Ok(statements)
}
impl Parser<'_> {
    fn peek(&self) -> &Kind {
        &self.tokens[self.at].kind
    }
    fn is(&self, s: &str) -> bool {
        match self.peek() {
            Kind::Word(w, false) => w.eq_ignore_ascii_case(s),
            Kind::Symbol(w) => *w == s,
            _ => false,
        }
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.is(s) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, s: &str) -> Result<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.expected(s))
        }
    }
    fn expected(&self, s: &str) -> Error {
        error(format!(
            "expected {s} at byte {}",
            self.tokens[self.at].start
        ))
    }
    fn name(&mut self) -> Result<String> {
        match self.peek().clone() {
            Kind::Word(s, _) | Kind::String(s) => {
                if s.len() > 1024 {
                    return Err(Error::Limit("identifier bytes"));
                }
                self.at += 1;
                Ok(s)
            }
            _ => Err(self.expected("identifier")),
        }
    }
    fn table_name(&mut self) -> Result<String> {
        let first = self.name()?;
        if self.eat(".") {
            if !first.eq_ignore_ascii_case("main") {
                return Err(Error::Unsupported("attached or temporary schemas"));
            }
            self.name()
        } else {
            Ok(first)
        }
    }
    fn schema_sql(&self, start: usize, end: usize, name_start: usize, name_end: usize) -> String {
        if name_end == name_start + 3 {
            // sqlite_schema stores an unqualified object declaration, even when
            // the statement used main.name. Keeping it makes native reopen fail.
            format!(
                "{}{}",
                &self.sql[start..self.tokens[name_start].start],
                &self.sql[self.tokens[name_start + 2].start..end]
            )
        } else {
            self.sql[start..end].to_string()
        }
    }
    fn expr_list(&mut self) -> Result<Vec<Expr>> {
        let mut values = vec![self.expr(0)?];
        while self.eat(",") {
            values.push(self.expr(0)?);
        }
        Ok(values)
    }
    fn names(&mut self) -> Result<Vec<String>> {
        self.expect("(")?;
        let mut values = vec![self.name()?];
        while self.eat(",") {
            values.push(self.name()?);
        }
        self.expect(")")?;
        Ok(values)
    }
    fn optional_where(&mut self) -> Result<Option<Expr>> {
        if self.eat("WHERE") {
            Ok(Some(self.expr(0)?))
        } else {
            Ok(None)
        }
    }
    fn conflict(&mut self) -> Result<Conflict> {
        for (name, policy) in [
            ("ROLLBACK", Conflict::Rollback),
            ("ABORT", Conflict::Abort),
            ("FAIL", Conflict::Fail),
            ("IGNORE", Conflict::Ignore),
            ("REPLACE", Conflict::Replace),
        ] {
            if self.eat(name) {
                return Ok(policy);
            }
        }
        Err(self.expected("ROLLBACK, ABORT, FAIL, IGNORE or REPLACE"))
    }
    fn on_conflict(&mut self) -> Result<Conflict> {
        if self.eat("ON") {
            self.expect("CONFLICT")?;
            self.conflict()
        } else {
            Ok(Conflict::Default)
        }
    }
    fn or_conflict(&mut self) -> Result<Conflict> {
        if self.eat("OR") {
            self.conflict()
        } else {
            Ok(Conflict::Default)
        }
    }
    fn assignments(&mut self) -> Result<Vec<(String, Expr)>> {
        let mut assignments = Vec::new();
        loop {
            let name = self.name()?;
            self.expect("=")?;
            assignments.push((name, self.expr(0)?));
            if !self.eat(",") {
                break;
            }
        }
        Ok(assignments)
    }
    fn upserts(&mut self) -> Result<Vec<Upsert>> {
        let mut upserts = Vec::new();
        while self.eat("ON") {
            self.expect("CONFLICT")?;
            let target = if self.eat("(") {
                let mut terms = Vec::new();
                loop {
                    terms.push(self.expr(0)?);
                    if !self.eat("ASC") {
                        self.eat("DESC");
                    }
                    if !self.eat(",") {
                        break;
                    }
                }
                self.expect(")")?;
                Some(terms)
            } else {
                None
            };
            let target_where = if target.is_some() {
                self.optional_where()?
            } else {
                None
            };
            self.expect("DO")?;
            let action = if self.eat("NOTHING") {
                UpsertAction::Nothing
            } else {
                self.expect("UPDATE")?;
                self.expect("SET")?;
                UpsertAction::Update {
                    assignments: self.assignments()?,
                    filter: self.optional_where()?,
                }
            };
            let last = target.is_none();
            upserts.push(Upsert {
                target,
                target_where,
                action,
            });
            if upserts.len() > 1000 {
                return Err(Error::Limit("UPSERT clauses"));
            }
            if last {
                break;
            }
        }
        Ok(upserts)
    }
    fn index_columns(&mut self) -> Result<Vec<IndexColumn>> {
        self.expect("(")?;
        let mut columns = Vec::new();
        loop {
            let name = self.name()?;
            let collation_name = if self.eat("COLLATE") {
                Some(self.name()?)
            } else {
                None
            };
            let collation = collation_name
                .as_deref()
                .map(Collation::parse)
                .transpose()?;
            let descending = self.eat("DESC");
            if !descending {
                self.eat("ASC");
            }
            columns.push(IndexColumn {
                name,
                collation,
                collation_name,
                descending,
            });
            if columns.len() > 2000 {
                return Err(Error::Limit("index columns"));
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(columns)
    }
    fn statement(&mut self) -> Result<Statement> {
        let start = self.tokens[self.at].start;
        if self.is("WITH") {
            let tables = self.with_clause()?;
            if self.is("SELECT") || self.is("VALUES") {
                return Ok(Statement::Select(Box::new(self.query_body(tables)?)));
            }
            if !["INSERT", "REPLACE", "UPDATE", "DELETE"]
                .iter()
                .any(|s| self.is(s))
            {
                return Err(self.expected("SELECT, INSERT, UPDATE or DELETE after WITH"));
            }
            return Ok(Statement::With {
                tables,
                statement: Box::new(self.statement()?),
            });
        }
        if self.is("SELECT") || self.is("VALUES") {
            return Ok(Statement::Select(Box::new(self.query()?)));
        }
        if self.eat("CREATE") {
            if self.eat("VIEW") {
                let if_not_exists = if self.eat("IF") {
                    self.expect("NOT")?;
                    self.expect("EXISTS")?;
                    true
                } else {
                    false
                };
                let first = self.at;
                let name = self.table_name()?;
                let name_start = self.tokens[if self.at == first + 3 {
                    first + 2
                } else {
                    first
                }]
                .start;
                let columns = if self.is("(") {
                    Some(self.names()?)
                } else {
                    None
                };
                if columns.as_ref().is_some_and(|c| c.len() > 2000) {
                    return Err(Error::Limit("view columns"));
                }
                self.expect("AS")?;
                let query = Rc::new(self.query()?);
                if !self.parameters.is_empty() {
                    return Err(error("parameters are not allowed in views"));
                }
                let end = self.tokens[self.at - 1].end;
                let sql = format!("CREATE VIEW {}", &self.sql[name_start..end]);
                return Ok(Statement::CreateView {
                    name,
                    columns,
                    query,
                    sql,
                    if_not_exists,
                });
            }
            let unique = self.eat("UNIQUE");
            if unique || self.is("INDEX") {
                self.expect("INDEX")?;
                let if_not_exists = if self.eat("IF") {
                    self.expect("NOT")?;
                    self.expect("EXISTS")?;
                    true
                } else {
                    false
                };
                let name_start = self.at;
                let name = self.table_name()?;
                let name_end = self.at;
                self.expect("ON")?;
                let table = self.name()?;
                let columns = self.index_columns()?;
                let end = self.tokens[self.at - 1].end;
                return Ok(Statement::CreateIndex {
                    name,
                    table,
                    columns,
                    unique,
                    if_not_exists,
                    sql: self.schema_sql(start, end, name_start, name_end),
                });
            }
            self.expect("TABLE")?;
            let if_not_exists = if self.eat("IF") {
                self.expect("NOT")?;
                self.expect("EXISTS")?;
                true
            } else {
                false
            };
            let name_start = self.at;
            let name = self.table_name()?;
            let name_end = self.at;
            if self.eat("AS") {
                return Ok(Statement::CreateAs {
                    name,
                    query: Box::new(self.query()?),
                    if_not_exists,
                });
            }
            self.expect("(")?;
            let mut columns = Vec::new();
            let mut constraints = Vec::new();
            let mut table_constraints = false;
            loop {
                if ["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"]
                    .iter()
                    .any(|s| self.is(s))
                {
                    if columns.is_empty() {
                        return Err(self.expected("column definition"));
                    }
                    table_constraints = true;
                    if self.eat("CONSTRAINT") {
                        // Names are retained in CREATE SQL; diagnostics currently
                        // report constraint type rather than the supplied name.
                        self.name()?;
                    } else if self.eat("PRIMARY") {
                        self.expect("KEY")?;
                        constraints.push(TableConstraint::Key {
                            columns: self.index_columns()?,
                            primary: true,
                            conflict: self.on_conflict()?,
                        });
                    } else if self.eat("UNIQUE") {
                        constraints.push(TableConstraint::Key {
                            columns: self.index_columns()?,
                            primary: false,
                            conflict: self.on_conflict()?,
                        });
                    } else if self.eat("CHECK") {
                        self.expect("(")?;
                        constraints.push(TableConstraint::Check(self.expr(0)?));
                        self.expect(")")?;
                        // Accepted by SQLite's historical table CHECK grammar,
                        // but the stored policy has no effect on enforcement.
                        self.on_conflict()?;
                    } else {
                        return Err(Error::Unsupported("FOREIGN KEY"));
                    }
                    if constraints.len() > 2000 {
                        return Err(Error::Limit("table constraints"));
                    }
                    if self.eat(",") || !self.is(")") {
                        continue;
                    }
                    break;
                }
                if table_constraints {
                    return Err(self.expected("table constraint"));
                }
                let name = self.name()?;
                let type_start = self.at;
                while matches!(self.peek(), Kind::Word(_, _) | Kind::String(_))
                    && ![
                        "PRIMARY",
                        "NOT",
                        "NULL",
                        "UNIQUE",
                        "CHECK",
                        "DEFAULT",
                        "COLLATE",
                        "REFERENCES",
                        "CONSTRAINT",
                        "GENERATED",
                        "AS",
                    ]
                    .iter()
                    .any(|s| self.is(s))
                {
                    self.name()?;
                }
                let mut declared_type = String::new();
                if self.is("(") && self.at == type_start {
                    return Err(self.expected("declared type before type size"));
                }
                if self.eat("(") {
                    loop {
                        if !self.eat("+") {
                            self.eat("-");
                        }
                        if matches!(self.peek(), Kind::Number(_)) {
                            self.at += 1;
                        } else {
                            return Err(self.expected("type size"));
                        }
                        if !self.eat(",") {
                            break;
                        }
                    }
                    self.expect(")")?;
                }
                if self.at > type_start {
                    declared_type = match &self.tokens[type_start].kind {
                        Kind::Word(name, true) | Kind::String(name) => name.clone(),
                        _ => self.sql[self.tokens[type_start].start..self.tokens[self.at - 1].end]
                            .into(),
                    };
                    if let Some(name) = ["ANY", "BLOB", "INT", "INTEGER", "REAL", "TEXT"]
                        .iter()
                        .find(|name| name.eq_ignore_ascii_case(&declared_type))
                    {
                        declared_type = (*name).into();
                    }
                }
                let mut column = Column {
                    name,
                    affinity: Affinity::from_type(&declared_type),
                    declared_type,
                    collation: Collation::Binary,
                    collation_name: "BINARY".into(),
                    not_null: false,
                    not_null_conflict: Conflict::Default,
                    primary: false,
                    primary_desc: false,
                    unique: false,
                    primary_conflict: Conflict::Default,
                    unique_conflict: Conflict::Default,
                    index_desc: false,
                    default: None,
                    default_sql: None,
                    checks: Vec::new(),
                };
                loop {
                    if self.eat("CONSTRAINT") {
                        self.name()?;
                    } else if self.eat("PRIMARY") {
                        if column.primary {
                            return Err(error("multiple primary keys"));
                        }
                        self.expect("KEY")?;
                        column.primary = true;
                        column.primary_desc = self.eat("DESC");
                        if !column.primary_desc {
                            self.eat("ASC");
                        }
                        if !column.unique {
                            column.index_desc = column.primary_desc;
                        }
                        column.primary_conflict = self.on_conflict()?;
                    } else if self.eat("NOT") {
                        self.expect("NULL")?;
                        column.not_null = true;
                        column.not_null_conflict = self.on_conflict()?;
                    } else if self.eat("NULL") {
                        self.on_conflict()?;
                    } else if self.eat("UNIQUE") {
                        column.unique = true;
                        column.unique_conflict =
                            column.unique_conflict.merge(self.on_conflict()?)?;
                    } else if self.eat("DEFAULT") {
                        let begin = self.tokens[self.at].start;
                        let parenthesized = self.is("(");
                        column.default = Some(self.expr(90)?);
                        let end = self.tokens[self.at - 1].end;
                        let sql = if parenthesized {
                            &self.sql[begin + 1..end - 1]
                        } else {
                            &self.sql[begin..end]
                        };
                        column.default_sql =
                            Some(sql.trim_matches(|c: char| c.is_ascii_whitespace()).into());
                    } else if self.eat("COLLATE") {
                        column.collation_name = self.name()?;
                        column.collation = Collation::parse(&column.collation_name)?;
                    } else if self.eat("CHECK") {
                        self.expect("(")?;
                        column.checks.push(self.expr(0)?);
                        self.expect(")")?;
                    } else {
                        break;
                    }
                }
                columns.push(column);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(")")?;
            let end = self.tokens[self.at - 1].end;
            return Ok(Statement::Create {
                name,
                columns,
                constraints,
                if_not_exists,
                sql: self.schema_sql(start, end, name_start, name_end),
            });
        }
        if self.eat("DROP") {
            if self.eat("VIEW") {
                let if_exists = if self.eat("IF") {
                    self.expect("EXISTS")?;
                    true
                } else {
                    false
                };
                return Ok(Statement::DropView {
                    name: self.table_name()?,
                    if_exists,
                });
            }
            let index = self.eat("INDEX");
            if !index {
                self.expect("TABLE")?;
            }
            let if_exists = if self.eat("IF") {
                self.expect("EXISTS")?;
                true
            } else {
                false
            };
            let name = self.table_name()?;
            return Ok(if index {
                Statement::DropIndex { name, if_exists }
            } else {
                Statement::Drop { name, if_exists }
            });
        }
        let replace = self.eat("REPLACE");
        if replace || self.eat("INSERT") {
            let conflict = if replace {
                Conflict::Replace
            } else {
                self.or_conflict()?
            };
            self.expect("INTO")?;
            let name = self.table_name()?;
            let alias = if self.eat("AS") {
                Some(self.name()?)
            } else {
                None
            };
            let columns = if self.is("(") {
                Some(self.names()?)
            } else {
                None
            };
            let source = if self.eat("DEFAULT") {
                self.expect("VALUES")?;
                InsertSource::Default
            } else if self.is("SELECT") || self.is("WITH") || self.is("VALUES") {
                let mut query = self.query()?;
                match query.cores.as_mut_slice() {
                    [QueryCore::Values(rows)] if query.with.is_empty() => {
                        // Plain VALUES rows observe preceding insertions,
                        // notably through last_insert_rowid().
                        InsertSource::Values(core::mem::take(rows))
                    }
                    _ => InsertSource::Select(Box::new(query)),
                }
            } else {
                return Err(self.expected("VALUES or SELECT"));
            };
            let upserts = if matches!(source, InsertSource::Default) {
                Vec::new()
            } else {
                self.upserts()?
            };
            return Ok(Statement::Insert {
                name,
                alias,
                columns,
                source,
                conflict,
                upserts,
            });
        }
        if self.eat("UPDATE") {
            let conflict = self.or_conflict()?;
            let name = self.table_name()?;
            self.expect("SET")?;
            let assignments = self.assignments()?;
            return Ok(Statement::Update {
                name,
                assignments,
                filter: self.optional_where()?,
                conflict,
            });
        }
        if self.eat("DELETE") {
            self.expect("FROM")?;
            let name = self.table_name()?;
            return Ok(Statement::Delete {
                name,
                filter: self.optional_where()?,
            });
        }
        if self.eat("BEGIN") {
            if !self.eat("DEFERRED") && !self.eat("IMMEDIATE") {
                self.eat("EXCLUSIVE");
            }
            self.eat("TRANSACTION");
            return Ok(Statement::Begin);
        }
        if self.eat("COMMIT") || self.eat("END") {
            self.eat("TRANSACTION");
            return Ok(Statement::Commit);
        }
        if self.eat("ROLLBACK") {
            self.eat("TRANSACTION");
            let to = if self.eat("TO") {
                self.eat("SAVEPOINT");
                Some(self.name()?)
            } else {
                None
            };
            return Ok(Statement::Rollback(to));
        }
        if self.eat("SAVEPOINT") {
            return Ok(Statement::Savepoint(self.name()?));
        }
        if self.eat("RELEASE") {
            self.eat("SAVEPOINT");
            return Ok(Statement::Release(self.name()?));
        }
        if self.eat("PRAGMA") {
            let name = self.table_name()?;
            if [
                "table_info",
                "table_xinfo",
                "index_list",
                "index_info",
                "index_xinfo",
            ]
            .iter()
            .any(|n| name.eq_ignore_ascii_case(n))
            {
                let parenthesized = self.eat("(");
                let argument = if parenthesized || self.eat("=") {
                    let sign = if self.eat("-") {
                        Some("-")
                    } else if self.eat("+") {
                        Some("")
                    } else {
                        None
                    };
                    let value = if matches!(self.peek(), Kind::Number(_)) {
                        let token = &self.tokens[self.at];
                        let n = &self.sql[token.start..token.end];
                        // SQLite's pragma-value grammar excludes its extended
                        // numeric token (underscores), although expressions allow it.
                        if n.contains('_') {
                            return Err(self.expected("pragma numeric literal without separators"));
                        }
                        let value = format!("{}{n}", sign.unwrap_or(""));
                        self.at += 1;
                        value
                    } else {
                        if sign.is_some() {
                            return Err(self.expected("numeric pragma argument"));
                        }
                        self.name()?
                    };
                    if parenthesized {
                        self.expect(")")?;
                    }
                    Some(value)
                } else {
                    None
                };
                return Ok(Statement::Pragma {
                    name,
                    value: None,
                    argument,
                });
            }
            let value = if self.eat("=") {
                let negative = self.eat("-");
                self.eat("+");
                if let Kind::Number(n) = self.peek().clone() {
                    self.at += 1;
                    let n = n
                        .parse::<i64>()
                        .map_err(|_| error("invalid PRAGMA integer"))?;
                    Some(if negative { -n } else { n })
                } else {
                    return Err(self.expected("integer"));
                }
            } else {
                None
            };
            let argument = if self.eat("(") {
                let n = self.name()?;
                self.expect(")")?;
                Some(n)
            } else {
                None
            };
            return Ok(Statement::Pragma {
                name,
                value,
                argument,
            });
        }
        Err(self.expected("supported SQL statement"))
    }
    fn alias(&mut self) -> Result<Option<String>> {
        if self.eat("AS") {
            return Ok(Some(self.name()?));
        }
        if matches!(self.peek(), Kind::Word(_, true) | Kind::String(_)) {
            return Ok(Some(self.name()?));
        }
        if matches!(self.peek(), Kind::Word(_, false))
            && ![
                "FROM",
                "WHERE",
                "GROUP",
                "HAVING",
                "ORDER",
                "LIMIT",
                "OFFSET",
                "UNION",
                "INTERSECT",
                "EXCEPT",
                "JOIN",
                "LEFT",
                "RIGHT",
                "FULL",
                "INNER",
                "OUTER",
                "CROSS",
                "NATURAL",
                "ON",
                "USING",
                "ASC",
                "DESC",
                "NULLS",
                "RETURNING",
                "UNION",
                "INTERSECT",
                "EXCEPT",
            ]
            .iter()
            .any(|s| self.is(s))
        {
            return Ok(Some(self.name()?));
        }
        Ok(None)
    }
    fn with_clause(&mut self) -> Result<Vec<CommonTable>> {
        self.expect("WITH")?;
        self.eat("RECURSIVE");
        let mut tables: Vec<CommonTable> = Vec::new();
        loop {
            let id = self.tokens[self.at].start;
            let name = self.name()?;
            if tables.iter().any(|t| t.name.eq_ignore_ascii_case(&name)) {
                return Err(error(format!("duplicate WITH table name: {name}")));
            }
            let columns = if self.is("(") {
                Some(self.names()?)
            } else {
                None
            };
            self.expect("AS")?;
            if self.eat("NOT") {
                self.expect("MATERIALIZED")?;
            } else {
                self.eat("MATERIALIZED");
            }
            self.expect("(")?;
            let query = Box::new(self.query()?);
            self.expect(")")?;
            tables.push(CommonTable {
                id,
                name,
                columns,
                query,
            });
            if tables.len() > 2000 {
                return Err(Error::Limit("common table count"));
            }
            if !self.eat(",") {
                break;
            }
        }
        Ok(tables)
    }
    fn subquery_expr(&mut self, mode: QueryMode) -> Result<Expr> {
        let id = self.tokens[self.at].start;
        let query = Rc::new(self.query()?);
        self.make(ExprKind::Subquery(Subquery { id, query, mode }))
    }
    fn query(&mut self) -> Result<Query> {
        if self.recursion >= self.limits.max_expr_depth.min(32) {
            return Err(Error::Limit("query nesting"));
        }
        self.recursion += 1;
        let result = (|| {
            let tables = if self.is("WITH") {
                self.with_clause()?
            } else {
                Vec::new()
            };
            self.query_body(tables)
        })();
        self.recursion -= 1;
        result
    }
    fn query_body(&mut self, with: Vec<CommonTable>) -> Result<Query> {
        let mut cores = Vec::new();
        let mut operators = Vec::new();
        loop {
            cores.push(if self.eat("SELECT") {
                QueryCore::Select(Box::new(self.select_core()?))
            } else if self.eat("VALUES") {
                let mut rows: Vec<Vec<Expr>> = Vec::new();
                loop {
                    self.expect("(")?;
                    let row = self.expr_list()?;
                    self.expect(")")?;
                    if rows.first().is_some_and(|first| first.len() != row.len()) {
                        return Err(error("all VALUES rows must have the same number of terms"));
                    }
                    if row.len() > 2000 {
                        return Err(Error::Limit("result columns"));
                    }
                    rows.push(row);
                    if !self.eat(",") {
                        break;
                    }
                }
                QueryCore::Values(rows)
            } else {
                return Err(self.expected("SELECT or VALUES"));
            });
            if cores.len() > 500 {
                return Err(Error::Limit("compound SELECT terms"));
            }
            let operator = if self.eat("UNION") {
                if self.eat("ALL") {
                    Compound::UnionAll
                } else {
                    Compound::Union
                }
            } else if self.eat("INTERSECT") {
                Compound::Intersect
            } else if self.eat("EXCEPT") {
                Compound::Except
            } else {
                break;
            };
            operators.push(operator);
        }
        let (order, limit, offset) = if matches!(cores.last(), Some(QueryCore::Values(_))) {
            (Vec::new(), None, None)
        } else {
            self.query_tail()?
        };
        Ok(Query {
            with,
            cores,
            operators,
            order,
            limit,
            offset,
        })
    }
    fn select_core(&mut self) -> Result<Select> {
        let distinct = self.eat("DISTINCT");
        if !distinct {
            self.eat("ALL");
        }
        let mut items = Vec::new();
        loop {
            let start = self.tokens[self.at].start;
            let item = if self.eat("*") {
                SelectItem {
                    expr: None,
                    star: None,
                    alias: None,
                    label: "*".into(),
                }
            } else if matches!(self.peek(), Kind::Word(_, _))
                && matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(Kind::Symbol("."))
                )
                && matches!(
                    self.tokens.get(self.at + 2).map(|t| &t.kind),
                    Some(Kind::Symbol("*"))
                )
            {
                let name = self.name()?;
                self.at += 2;
                SelectItem {
                    expr: None,
                    star: Some(name),
                    alias: None,
                    label: "*".into(),
                }
            } else {
                let expr = self.expr(0)?;
                let end = self.tokens[self.at - 1].end;
                let alias = self.alias()?;
                SelectItem {
                    expr: Some(expr),
                    star: None,
                    label: self.sql[start..end].to_string(),
                    alias,
                }
            };
            items.push(item);
            if !self.eat(",") {
                break;
            }
        }
        let mut sources = Vec::new();
        if self.eat("FROM") {
            let mut left = false;
            loop {
                let start = self.at;
                let query = if self.eat("(") {
                    let query = self.query()?;
                    self.expect(")")?;
                    Some(Box::new(query))
                } else {
                    None
                };
                let name = if query.is_some() {
                    String::new()
                } else {
                    self.table_name()?
                };
                let qualified = query.is_none() && self.at == start + 3;
                let alias = self.alias()?.unwrap_or_else(|| name.clone());
                let on = if self.eat("ON") {
                    Some(self.expr(0)?)
                } else {
                    None
                };
                sources.push(Source {
                    name,
                    qualified,
                    query,
                    alias,
                    left,
                    on,
                });
                if self.eat(",") {
                    left = false;
                    continue;
                }
                left = self.eat("LEFT");
                if left {
                    self.eat("OUTER");
                }
                let inner = self.eat("INNER") || self.eat("CROSS");
                if self.eat("JOIN") {
                    continue;
                }
                if left || inner {
                    return Err(self.expected("JOIN"));
                }
                break;
            }
        }
        let filter = self.optional_where()?;
        let group = if self.eat("GROUP") {
            self.expect("BY")?;
            self.expr_list()?
        } else {
            Vec::new()
        };
        let having = if self.eat("HAVING") {
            Some(self.expr(0)?)
        } else {
            None
        };
        Ok(Select {
            items,
            sources,
            filter,
            group,
            having,
            distinct,
            order: Vec::new(),
            limit: None,
            offset: None,
        })
    }
    fn query_tail(&mut self) -> Result<(Vec<Ordering>, Option<Expr>, Option<Expr>)> {
        let mut order = Vec::new();
        if self.eat("ORDER") {
            self.expect("BY")?;
            loop {
                let expr = self.expr(0)?;
                let descending = self.eat("DESC");
                if !descending {
                    self.eat("ASC");
                }
                let nulls_first = if self.eat("NULLS") {
                    if self.eat("FIRST") {
                        Some(true)
                    } else {
                        self.expect("LAST")?;
                        Some(false)
                    }
                } else {
                    None
                };
                order.push(Ordering {
                    expr,
                    descending,
                    nulls_first,
                });
                if !self.eat(",") {
                    break;
                }
            }
        }
        let mut limit = None;
        let mut offset = None;
        if self.eat("LIMIT") {
            limit = Some(self.expr(0)?);
            if self.eat("OFFSET") {
                offset = Some(self.expr(0)?);
            } else if self.eat(",") {
                offset = limit.take();
                limit = Some(self.expr(0)?);
            }
        }
        Ok((order, limit, offset))
    }
    fn make(&self, kind: ExprKind) -> Result<Expr> {
        let mut expr = Expr { kind, depth: 1 };
        expr.depth = 1 + expr.children().iter().map(|e| e.depth).max().unwrap_or(0);
        if expr.depth > self.limits.max_expr_depth.min(64) {
            return Err(Error::Limit("SQL expression depth"));
        }
        Ok(expr)
    }
    fn expr(&mut self, min: u8) -> Result<Expr> {
        if self.recursion >= self.limits.max_expr_depth.min(64) {
            return Err(Error::Limit("SQL parse depth"));
        }
        self.recursion += 1;
        let result = self.expr_inner(min);
        self.recursion -= 1;
        result
    }
    fn expr_inner(&mut self, min: u8) -> Result<Expr> {
        let mut lhs = if self.eat("+") {
            let x = self.expr(90)?;
            self.make(ExprKind::Unary(Unary::Plus, Box::new(x)))?
        } else if self.eat("-") {
            if let Kind::Number(n) = self.peek() {
                if n == "9223372036854775808" {
                    self.at += 1;
                    Expr::literal(Value::Integer(i64::MIN))
                } else {
                    let x = self.expr(90)?;
                    self.make(ExprKind::Unary(Unary::Minus, Box::new(x)))?
                }
            } else {
                let x = self.expr(90)?;
                self.make(ExprKind::Unary(Unary::Minus, Box::new(x)))?
            }
        } else if self.eat("~") {
            let x = self.expr(90)?;
            self.make(ExprKind::Unary(Unary::BitNot, Box::new(x)))?
        } else if self.eat("NOT") {
            let x = self.expr(25)?;
            self.make(ExprKind::Unary(Unary::Not, Box::new(x)))?
        } else if self.eat("EXISTS") {
            self.expect("(")?;
            let x = self.subquery_expr(QueryMode::Exists)?;
            self.expect(")")?;
            x
        } else if self.eat("(") {
            let x = if self.is("SELECT") || self.is("WITH") || self.is("VALUES") {
                self.subquery_expr(QueryMode::Scalar)?
            } else {
                self.expr(0)?
            };
            self.expect(")")?;
            x
        } else if self.eat("NULL") {
            Expr::literal(Value::Null)
        } else if self.eat("CASE") {
            let base = if self.is("WHEN") {
                None
            } else {
                Some(Box::new(self.expr(0)?))
            };
            let mut arms = Vec::new();
            while self.eat("WHEN") {
                let when = self.expr(0)?;
                self.expect("THEN")?;
                arms.push((when, self.expr(0)?));
            }
            if arms.is_empty() {
                return Err(self.expected("WHEN"));
            }
            let other = if self.eat("ELSE") {
                Some(Box::new(self.expr(0)?))
            } else {
                None
            };
            self.expect("END")?;
            self.make(ExprKind::Case(base, arms, other))?
        } else if self.eat("CAST") {
            self.expect("(")?;
            let x = self.expr(0)?;
            self.expect("AS")?;
            let mut parts = vec![self.name()?];
            while matches!(self.peek(), Kind::Word(_, _)) {
                parts.push(self.name()?);
            }
            self.expect(")")?;
            self.make(ExprKind::Cast(
                Box::new(x),
                Affinity::from_type(&parts.join(" ")),
            ))?
        } else {
            let token = self.peek().clone();
            self.at += 1;
            match token {
                Kind::String(s) => Expr::literal(Value::Text(Text::utf8(&s))),
                Kind::Blob(b) => Expr::literal(Value::Blob(b)),
                Kind::Number(s) => {
                    if s.trim_start_matches('0') == "9223372036854775808" {
                        self.make(ExprKind::MinMagnitude)?
                    } else {
                        let v = if s.starts_with("0x") || s.starts_with("0X") {
                            Value::Integer(
                                u64::from_str_radix(&s[2..], 16)
                                    .map_err(|_| error("hexadecimal integer overflow"))?
                                    as i64,
                            )
                        } else if let Ok(n) = s.parse::<i64>() {
                            Value::Integer(n)
                        } else {
                            Value::Real(s.parse::<f64>().map_err(|_| error("invalid number"))?)
                        };
                        Expr::literal(v)
                    }
                }
                Kind::Parameter(name) => {
                    let number = if name == "?" {
                        self.parameters.len() + 1
                    } else if let Some(n) = name.strip_prefix('?') {
                        n.parse::<usize>()
                            .map_err(|_| error("invalid parameter index"))?
                    } else {
                        self.parameters
                            .iter()
                            .position(|n| n.as_deref() == Some(&name))
                            .map(|n| n + 1)
                            .unwrap_or(self.parameters.len() + 1)
                    };
                    if number == 0 || number > self.limits.max_parameters {
                        return Err(Error::Limit("parameter index"));
                    }
                    self.parameters
                        .resize(self.parameters.len().max(number), None);
                    if name != "?" && self.parameters[number - 1].is_none() {
                        self.parameters[number - 1] = Some(name);
                    }
                    self.make(ExprKind::Parameter(number - 1))?
                }
                Kind::Word(name, quoted) => {
                    if self.eat("(") {
                        let distinct = self.eat("DISTINCT");
                        let star = self.eat("*");
                        let args = if star || self.is(")") {
                            Vec::new()
                        } else {
                            self.expr_list()?
                        };
                        self.expect(")")?;
                        if args.len() > 1000 {
                            return Err(Error::Limit("function arguments"));
                        }
                        self.make(ExprKind::Call {
                            name,
                            args,
                            star,
                            distinct,
                        })?
                    } else {
                        let (qualifier, name) = if self.eat(".") {
                            (Some(name), self.name()?)
                        } else {
                            (None, name)
                        };
                        self.make(ExprKind::Column {
                            qualifier,
                            name,
                            quoted,
                        })?
                    }
                }
                _ => {
                    self.at -= 1;
                    return Err(self.expected("expression"));
                }
            }
        };
        loop {
            if self.is("COLLATE") && min <= 85 {
                self.at += 1;
                let c = Collation::parse(&self.name()?)?;
                lhs = self.make(ExprKind::Collate(Box::new(lhs), c))?;
                continue;
            }
            if (self.is("ISNULL") || self.is("NOTNULL")) && min <= 35 {
                let not = self.eat("NOTNULL");
                if !not {
                    self.expect("ISNULL")?;
                }
                lhs = self.make(ExprKind::Binary(
                    if not { Binary::IsNot } else { Binary::Is },
                    Box::new(lhs),
                    Box::new(Expr::literal(Value::Null)),
                ))?;
                continue;
            }
            let mut negated = false;
            if self.is("NOT") && min <= 35 {
                self.at += 1;
                if self.eat("NULL") {
                    lhs = self.make(ExprKind::Binary(
                        Binary::IsNot,
                        Box::new(lhs),
                        Box::new(Expr::literal(Value::Null)),
                    ))?;
                    continue;
                }
                if self.is("BETWEEN") || self.is("IN") || self.is("LIKE") {
                    negated = true;
                } else {
                    self.at -= 1;
                }
            }
            if self.is("BETWEEN") && min <= 35 {
                self.at += 1;
                let low = self.expr(36)?;
                self.expect("AND")?;
                let high = self.expr(36)?;
                lhs = self.make(ExprKind::Between(
                    Box::new(lhs),
                    Box::new(low),
                    Box::new(high),
                    negated,
                ))?;
                continue;
            }
            if self.is("IN") && min <= 35 {
                self.at += 1;
                self.expect("(")?;
                if self.is("SELECT") || self.is("WITH") || self.is("VALUES") {
                    let query = self.subquery_expr(QueryMode::Set)?;
                    self.expect(")")?;
                    lhs = self.make(ExprKind::InQuery(Box::new(lhs), Box::new(query), negated))?;
                    continue;
                }
                let values = if self.is(")") {
                    Vec::new()
                } else {
                    self.expr_list()?
                };
                self.expect(")")?;
                lhs = self.make(ExprKind::In(Box::new(lhs), values, negated))?;
                continue;
            }
            let op = if self.is("OR") {
                Some((10, Binary::Or))
            } else if self.is("AND") {
                Some((20, Binary::And))
            } else if self.is("IS") {
                Some((35, Binary::Is))
            } else if self.is("LIKE") {
                Some((
                    35,
                    if negated {
                        Binary::NotLike
                    } else {
                        Binary::Like
                    },
                ))
            } else {
                match self.peek() {
                    Kind::Symbol(s) => match *s {
                        "=" | "==" => Some((35, Binary::Equal)),
                        "!=" | "<>" => Some((35, Binary::NotEqual)),
                        "<" => Some((40, Binary::Less)),
                        "<=" => Some((40, Binary::LessEqual)),
                        ">" => Some((40, Binary::Greater)),
                        ">=" => Some((40, Binary::GreaterEqual)),
                        "&" => Some((50, Binary::BitAnd)),
                        "|" => Some((50, Binary::BitOr)),
                        "<<" => Some((50, Binary::ShiftLeft)),
                        ">>" => Some((50, Binary::ShiftRight)),
                        "+" => Some((60, Binary::Add)),
                        "-" => Some((60, Binary::Subtract)),
                        "*" => Some((70, Binary::Multiply)),
                        "/" => Some((70, Binary::Divide)),
                        "%" => Some((70, Binary::Remainder)),
                        "||" => Some((80, Binary::Concat)),
                        _ => None,
                    },
                    _ => None,
                }
            };
            let Some((precedence, mut op)) = op else {
                break;
            };
            if precedence < min {
                break;
            }
            self.at += 1;
            if op == Binary::Is {
                let not = self.eat("NOT");
                let distinct = self.eat("DISTINCT");
                if distinct {
                    self.expect("FROM")?;
                }
                if not ^ distinct {
                    op = Binary::IsNot;
                }
            }
            let rhs = self.expr(precedence + 1)?;
            lhs = self.make(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)))?;
        }
        Ok(lhs)
    }
}
