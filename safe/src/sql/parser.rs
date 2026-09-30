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
    /// Spelling retained where SQLite compares schema expressions lexically
    /// (large/real numeric literals, blobs, and CAST type names).
    pub token: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Literal(Value),
    /// Decimal integer token 2^63. A syntactic unary minus can still form MIN.
    MinMagnitude,
    /// A join-generated coalesce inherits affinity/collation from its first
    /// argument, unlike a user-written coalesce function.
    Merged(Vec<Expr>),
    Vector(Vec<Expr>),
    /// One column of a shared row-valued assignment subquery.
    RowField(Box<Expr>, usize, usize),
    Boolean(bool),
    Column {
        qualifier: Option<String>,
        name: String,
        quoted: bool,
    },
    Slot(usize, Affinity, Collation),
    Generated(super::eval::generated::Reference),
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
        order: Vec<Ordering>,
        filter: Option<Box<Expr>>,
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
}
impl Expr {
    pub fn literal(value: Value) -> Self {
        Self {
            kind: ExprKind::Literal(value),
            depth: 1,
            token: None,
        }
    }
    pub fn children(&self) -> Vec<&Expr> {
        match &self.kind {
            ExprKind::Unary(_, x)
            | ExprKind::Cast(x, _)
            | ExprKind::Collate(x, _)
            | ExprKind::RowField(x, ..) => vec![x],
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
            ExprKind::Call {
                args,
                order,
                filter,
                ..
            } => args
                .iter()
                .chain(order.iter().map(|o| &o.expr))
                .chain(filter.iter().map(Box::as_ref))
                .collect(),
            ExprKind::Merged(args) | ExprKind::Vector(args) => args.iter().collect(),
            _ => Vec::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct Column {
    pub generated: Option<(Expr, bool)>,
    pub single_type_token: bool,
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
    pub fn virtual_column(&self) -> bool {
        self.generated.as_ref().is_some_and(|(_, stored)| !stored)
    }
    pub fn rowid_alias(&self) -> bool {
        self.primary
            && !self.primary_desc
            && self.single_type_token
            && self.declared_type.eq_ignore_ascii_case("INTEGER")
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
    pub right: bool,
    pub natural: bool,
    pub using: Option<Vec<String>>,
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
    pub nested_from: bool,
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
    pub materialized: Option<bool>,
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
pub struct IndexExpression {
    pub expr: Expr,
    pub descending: bool,
    pub collation_name: Option<String>,
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
pub enum Alter {
    Add {
        column: Box<Column>,
        sql: String,
    },
    Drop(String),
    SetNotNull {
        column: String,
        sql: String,
    },
    DropNotNull(String),
    AddCheck {
        name: Option<String>,
        expression: Box<Expr>,
        sql: String,
    },
    DropConstraint(String),
}
#[derive(Clone, Debug)]
pub enum Statement {
    Alter {
        name: String,
        action: Alter,
    },
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
        columns: Vec<IndexExpression>,
        predicate: Option<Expr>,
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
        autoincrement: bool,
        strict: bool,
        without_rowid: bool,
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
        returning: Vec<SelectItem>,
    },
    Select(Box<Query>),
    Update {
        name: String,
        alias: Option<String>,
        assignments: Vec<(String, Expr)>,
        filter: Option<Expr>,
        conflict: Conflict,
        returning: Vec<SelectItem>,
    },
    Delete {
        name: String,
        alias: Option<String>,
        filter: Option<Expr>,
        returning: Vec<SelectItem>,
    },
    Begin,
    Commit,
    Rollback(Option<String>),
    Savepoint(String),
    Release(String),
    Pragma {
        schema: Option<String>,
        name: String,
        value: Option<i64>,
        argument: Option<String>,
    },
}
impl Statement {
    pub fn mutating(&self) -> bool {
        match self {
            Self::With { statement, .. } => statement.mutating(),
            Self::Alter { .. }
            | Self::Create { .. }
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
            let names = if self.is("(") {
                self.names()?
            } else {
                vec![self.name()?]
            };
            self.expect("=")?;
            let expr = self.expr(0)?;
            if matches!(expr.kind, ExprKind::Subquery(_)) {
                let width = names.len();
                for (i, name) in names.into_iter().enumerate() {
                    assignments.push((
                        name,
                        self.make(ExprKind::RowField(Box::new(expr.clone()), i, width))?,
                    ));
                }
            } else {
                let values = match expr.kind {
                    ExprKind::Vector(values) => values,
                    _ => vec![expr],
                };
                if names.len() != values.len() {
                    return Err(error("assignment column count mismatch"));
                }
                assignments.extend(names.into_iter().zip(values));
            }
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
        self.key_columns(false).map(|(columns, _)| columns)
    }
    fn index_expressions(&mut self) -> Result<Vec<IndexExpression>> {
        self.expect("(")?;
        let mut terms = Vec::new();
        loop {
            let start = self.at;
            let expr = self.expr(0)?;
            let collation_name = if matches!(expr.kind, ExprKind::Collate(..)) {
                self.tokens[start..self.at].windows(2).rev().find_map(|pair| {
                    if matches!(&pair[0].kind, Kind::Word(s, false) if s.eq_ignore_ascii_case("COLLATE")) {
                        match &pair[1].kind { Kind::Word(s, _) | Kind::String(s) => Some(s.clone()), _ => None }
                    } else { None }
                })
            } else {
                None
            };
            let descending = self.eat("DESC");
            if !descending {
                self.eat("ASC");
            }
            terms.push(IndexExpression {
                expr,
                descending,
                collation_name,
            });
            if terms.len() > 2000 {
                return Err(Error::Limit("index terms"));
            }
            if !self.eat(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(terms)
    }
    fn key_columns(&mut self, primary: bool) -> Result<(Vec<IndexColumn>, bool)> {
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
        let autoincrement = primary && self.eat("AUTOINCREMENT");
        self.expect(")")?;
        Ok((columns, autoincrement))
    }
    // Preserve trailing block comments in a constraint, but discard trailing
    // line comments before inserting its text into a CREATE TABLE statement.
    fn constraint_sql(&self, start: usize) -> String {
        let mut end = self.tokens[self.at - 1].end;
        let mut at = end;
        let stop = self.tokens[self.at].start;
        let bytes = self.sql.as_bytes();
        while at < stop {
            if bytes[at].is_ascii_whitespace() {
                at += 1;
            } else if bytes[at..].starts_with(b"--") {
                while at < stop && bytes[at] != b'\n' {
                    at += 1;
                }
            } else if bytes[at..].starts_with(b"/*") {
                at += 2;
                while at < stop && !bytes[at..].starts_with(b"*/") {
                    at += 1;
                }
                at = (at + 2).min(stop);
                end = at;
            } else {
                break;
            }
        }
        self.sql[start..end].into()
    }
    fn column(&mut self) -> Result<(Column, bool)> {
        let mut autoincrement = false;
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
                "AS",
                "AUTOINCREMENT",
            ]
            .iter()
            .any(|s| self.is(s))
            && !(self.is("GENERATED")
                && matches!(self.tokens.get(self.at + 1).map(|t| &t.kind), Some(Kind::Word(s, false)) if s.eq_ignore_ascii_case("ALWAYS")))
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
                _ => self.sql[self.tokens[type_start].start..self.tokens[self.at - 1].end].into(),
            };
            if let Some(name) = ["ANY", "BLOB", "INT", "INTEGER", "REAL", "TEXT"]
                .iter()
                .find(|name| name.eq_ignore_ascii_case(&declared_type))
            {
                declared_type = (*name).into();
            }
        }
        let mut column = Column {
            generated: None,
            single_type_token: self.at == type_start + 1,
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
                autoincrement |= self.eat("AUTOINCREMENT");
            } else if self.eat("NOT") {
                self.expect("NULL")?;
                column.not_null = true;
                column.not_null_conflict = self.on_conflict()?;
            } else if self.eat("NULL") {
                self.on_conflict()?;
            } else if self.eat("UNIQUE") {
                column.unique = true;
                column.unique_conflict = column.unique_conflict.merge(self.on_conflict()?)?;
            } else if self.eat("DEFAULT") {
                let begin = self.tokens[self.at].start;
                let parenthesized = self.is("(");
                if (self.is("+") || self.is("-"))
                    && matches!(
                        self.tokens.get(self.at + 1).map(|t| &t.kind),
                        Some(Kind::Symbol("("))
                    )
                {
                    return Err(self.expected("literal after default sign"));
                }
                let mut default = self.expr(90)?;
                if !parenthesized {
                    let term =
                        |e: &Expr| matches!(e.kind, ExprKind::Literal(_) | ExprKind::MinMagnitude);
                    if !(term(&default)
                        || matches!(
                            default.kind,
                            ExprKind::Column {
                                qualifier: None,
                                ..
                            }
                        )
                        || matches!(&default.kind, ExprKind::Unary(Unary::Plus | Unary::Minus, child) if term(child)))
                    {
                        return Err(self.expected("literal or parenthesized default"));
                    }
                    if let ExprKind::Column {
                        qualifier: None,
                        name,
                        quoted,
                    } = &default.kind
                    {
                        if !quoted
                            && ["CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP"]
                                .iter()
                                .any(|v| name.eq_ignore_ascii_case(v))
                        {
                            return Err(Error::Unsupported("date/time default functions"));
                        }
                        default = if !quoted
                            && (name.eq_ignore_ascii_case("true")
                                || name.eq_ignore_ascii_case("false"))
                        {
                            Expr {
                                kind: ExprKind::Boolean(name.eq_ignore_ascii_case("true")),
                                depth: 1,
                                token: None,
                            }
                        } else {
                            Expr::literal(Value::Text(Text::utf8(name)))
                        };
                    }
                }
                column.default = Some(default);
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
            } else if self.is("GENERATED") || self.is("AS") {
                if column.generated.is_some() {
                    return Err(error("multiple generated column expressions"));
                }
                if self.eat("GENERATED") {
                    self.expect("ALWAYS")?;
                }
                self.expect("AS")?;
                self.expect("(")?;
                let expr = self.expr(0)?;
                self.expect(")")?;
                let stored = self.eat("STORED");
                if !stored {
                    self.eat("VIRTUAL");
                }
                column.generated = Some((expr, stored));
            } else {
                break;
            }
        }
        Ok((column, autoincrement))
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
                let columns = self.index_expressions()?;
                let predicate = self.optional_where()?;
                let end = self.tokens[self.at - 1].end;
                return Ok(Statement::CreateIndex {
                    name,
                    table,
                    columns,
                    predicate,
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
            let mut autoincrement = false;
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
                        let (keys, automatic) = self.key_columns(true)?;
                        autoincrement |= automatic;
                        constraints.push(TableConstraint::Key {
                            columns: keys,
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
                let (column, automatic) = self.column()?;
                autoincrement |= automatic;
                columns.push(column);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(")")?;
            let mut strict = false;
            let mut without_rowid = false;
            while self.is("STRICT") || self.is("WITHOUT") {
                if self.eat("STRICT") {
                    strict = true;
                } else {
                    self.expect("WITHOUT")?;
                    self.expect("ROWID")?;
                    without_rowid = true;
                }
                if !self.eat(",") {
                    break;
                }
                if !self.is("STRICT") && !self.is("WITHOUT") {
                    return Err(self.expected("table option"));
                }
            }
            let end = self.tokens[self.at - 1].end;
            return Ok(Statement::Create {
                name,
                columns,
                constraints,
                autoincrement,
                strict,
                without_rowid,
                if_not_exists,
                sql: self.schema_sql(start, end, name_start, name_end),
            });
        }
        if self.eat("ALTER") {
            self.expect("TABLE")?;
            let name = self.table_name()?;
            let action = if self.eat("ADD") {
                if self.is("CONSTRAINT") || self.is("CHECK") {
                    let start = self.tokens[self.at].start;
                    let name = if self.eat("CONSTRAINT") {
                        Some(self.name()?)
                    } else {
                        None
                    };
                    self.expect("CHECK")?;
                    self.expect("(")?;
                    let expression = Box::new(self.expr(0)?);
                    self.expect(")")?;
                    self.on_conflict()?;
                    Alter::AddCheck {
                        name,
                        expression,
                        sql: self.constraint_sql(start),
                    }
                } else {
                    self.eat("COLUMN");
                    if ["CONSTRAINT", "CHECK", "PRIMARY", "UNIQUE", "FOREIGN"]
                        .iter()
                        .any(|s| self.is(s))
                    {
                        return Err(self.expected("column definition"));
                    }
                    let start = self.tokens[self.at].start;
                    let (column, _) = self.column()?;
                    let end = self.tokens[self.at - 1].end;
                    Alter::Add {
                        column: Box::new(column),
                        sql: self.sql[start..end].into(),
                    }
                }
            } else if self.eat("ALTER") {
                self.eat("COLUMN");
                let column = self.name()?;
                if self.eat("SET") {
                    let start = self.tokens[self.at].start;
                    self.expect("NOT")?;
                    self.expect("NULL")?;
                    self.on_conflict()?;
                    Alter::SetNotNull {
                        column,
                        sql: self.constraint_sql(start),
                    }
                } else {
                    self.expect("DROP")?;
                    self.expect("NOT")?;
                    self.expect("NULL")?;
                    Alter::DropNotNull(column)
                }
            } else {
                self.expect("DROP")?;
                if self.eat("CONSTRAINT") {
                    Alter::DropConstraint(self.name()?)
                } else {
                    self.eat("COLUMN");
                    if self.is("CONSTRAINT") {
                        return Err(self.expected("column name"));
                    }
                    Alter::Drop(self.name()?)
                }
            };
            return Ok(Statement::Alter { name, action });
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
                returning: self.returning()?,
            });
        }
        if self.eat("UPDATE") {
            let conflict = self.or_conflict()?;
            let name = self.table_name()?;
            let alias = if self.eat("AS") {
                Some(self.name()?)
            } else {
                None
            };
            self.expect("SET")?;
            let assignments = self.assignments()?;
            return Ok(Statement::Update {
                name,
                alias,
                assignments,
                filter: self.optional_where()?,
                conflict,
                returning: self.returning()?,
            });
        }
        if self.eat("DELETE") {
            self.expect("FROM")?;
            let name = self.table_name()?;
            let alias = if self.eat("AS") {
                Some(self.name()?)
            } else {
                None
            };
            return Ok(Statement::Delete {
                name,
                alias,
                filter: self.optional_where()?,
                returning: self.returning()?,
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
            let first = self.name()?;
            let (schema, name) = if self.eat(".") {
                (Some(first), self.name()?)
            } else {
                (None, first)
            };
            if schema.as_ref().is_some_and(|s| {
                !(s.eq_ignore_ascii_case("main")
                    || s.eq_ignore_ascii_case("temp") && name.eq_ignore_ascii_case("table_list"))
            }) {
                return Err(Error::Unsupported("attached or temporary schemas"));
            }
            if [
                "table_list",
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
                    schema,
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
                schema,
                name,
                value,
                argument,
            });
        }
        Err(self.expected("supported SQL statement"))
    }
    fn alias(&mut self) -> Result<Option<String>> {
        if self.eat("AS") {
            if self.is("RETURNING") {
                return Err(self.expected("alias"));
            }
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
            let materialized = if self.eat("NOT") {
                self.expect("MATERIALIZED")?;
                Some(false)
            } else {
                self.eat("MATERIALIZED").then_some(true)
            };
            self.expect("(")?;
            let query = Box::new(self.query()?);
            self.expect(")")?;
            tables.push(CommonTable {
                id,
                name,
                columns,
                query,
                materialized,
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
    fn returning(&mut self) -> Result<Vec<SelectItem>> {
        if self.eat("RETURNING") {
            self.select_items()
        } else {
            Ok(Vec::new())
        }
    }
    fn select_items(&mut self) -> Result<Vec<SelectItem>> {
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
        Ok(items)
    }
    fn parse_sources(&mut self) -> Result<Vec<Source>> {
        let mut sources = Vec::new();
        let mut left = false;
        let mut right = false;
        let mut natural = false;
        loop {
            let start = self.at;
            let mut nested = None;
            let query = if self.eat("(") {
                let query = if self.is("SELECT") || self.is("WITH") || self.is("VALUES") {
                    Some(Box::new(self.query()?))
                } else {
                    if self.recursion >= self.limits.max_expr_depth.min(32) {
                        return Err(Error::Limit("join nesting"));
                    }
                    self.recursion += 1;
                    let result = self.parse_sources();
                    self.recursion -= 1;
                    nested = Some(result?);
                    None
                };
                self.expect(")")?;
                query
            } else {
                None
            };
            let name = if query.is_some() || nested.is_some() {
                String::new()
            } else {
                self.table_name()?
            };
            let qualified = query.is_none() && self.at == start + 3;
            let explicit_alias = self.alias()?;
            let alias = explicit_alias.clone().unwrap_or_else(|| name.clone());
            let on = if self.eat("ON") {
                Some(self.expr(0)?)
            } else {
                None
            };
            let using = if self.eat("USING") {
                Some(self.names()?)
            } else {
                None
            };
            if (natural && (on.is_some() || using.is_some()))
                || (on.is_some() && using.is_some())
                || (sources.is_empty() && (on.is_some() || using.is_some()))
            {
                return Err(error("invalid join constraint"));
            }
            let mut source = Source {
                name,
                qualified,
                query,
                alias,
                left,
                right,
                natural,
                using,
                on,
            };
            if let Some(mut inside) = nested {
                if sources.is_empty()
                    && explicit_alias.is_none()
                    && source.on.is_none()
                    && source.using.is_none()
                {
                    sources.append(&mut inside);
                } else if inside.len() == 1 {
                    let inner = inside.remove(0);
                    source.alias = explicit_alias.unwrap_or_else(|| inner.name.clone());
                    source.name = inner.name;
                    source.qualified = inner.qualified;
                    source.query = inner.query;
                    sources.push(source);
                } else {
                    source.query = Some(Box::new(Query {
                        with: Vec::new(),
                        operators: Vec::new(),
                        order: Vec::new(),
                        limit: None,
                        offset: None,
                        cores: vec![QueryCore::Select(Box::new(Select {
                            nested_from: true,
                            items: Vec::new(),
                            sources: inside,
                            filter: None,
                            group: Vec::new(),
                            having: None,
                            order: Vec::new(),
                            limit: None,
                            offset: None,
                            distinct: false,
                        }))],
                    }));
                    sources.push(source);
                }
            } else {
                sources.push(source);
            }
            left = false;
            right = false;
            natural = false;
            if self.eat(",") {
                continue;
            }
            let mut outer = false;
            let mut inner = false;
            let mut count = 0;
            loop {
                if self.eat("NATURAL") {
                    natural = true;
                } else if self.eat("LEFT") {
                    left = true;
                    outer = true;
                } else if self.eat("RIGHT") {
                    right = true;
                    outer = true;
                } else if self.eat("FULL") {
                    left = true;
                    right = true;
                    outer = true;
                } else if self.eat("OUTER") {
                    outer = true;
                } else if self.eat("INNER") || self.eat("CROSS") {
                    inner = true;
                } else {
                    break;
                }
                count += 1;
                if count > 3 {
                    return Err(error("invalid join type"));
                }
            }
            if (inner && outer) || (outer && !left && !right) {
                return Err(error("invalid join type"));
            }
            if self.eat("JOIN") {
                continue;
            }
            if count != 0 {
                return Err(self.expected("JOIN"));
            }
            break;
        }
        Ok(sources)
    }
    fn select_core(&mut self) -> Result<Select> {
        let distinct = self.eat("DISTINCT");
        if !distinct {
            self.eat("ALL");
        }
        let items = self.select_items()?;
        let sources = if self.eat("FROM") {
            self.parse_sources()?
        } else {
            Vec::new()
        };
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
            nested_from: false,
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
    fn order_by(&mut self) -> Result<Vec<Ordering>> {
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
                if order.len() > 2000 {
                    return Err(Error::Limit("ORDER BY terms"));
                }
                if !self.eat(",") {
                    break;
                }
            }
        }
        Ok(order)
    }
    fn query_tail(&mut self) -> Result<(Vec<Ordering>, Option<Expr>, Option<Expr>)> {
        let order = self.order_by()?;
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
        let mut expr = Expr {
            kind,
            depth: 1,
            token: None,
        };
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
        let lhs = self.expr_prefix()?;
        self.expr_tail(lhs, min)
    }
    fn expr_prefix(&mut self) -> Result<Expr> {
        let lhs = if self.eat("+") {
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
                let values = self.expr_list()?;
                if values.len() == 1 {
                    values
                        .into_iter()
                        .next()
                        .ok_or(Error::Corrupt("empty parenthesized expression"))?
                } else {
                    if values.len() > 2000 {
                        return Err(Error::Limit("row value columns"));
                    }
                    self.make(ExprKind::Vector(values))?
                }
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
            let mut expr = self.make(ExprKind::Cast(
                Box::new(x),
                Affinity::from_type(&parts.join(" ")),
            ))?;
            expr.token = Some(parts.join(" "));
            expr
        } else {
            let token = self.peek().clone();
            self.at += 1;
            match token {
                Kind::String(s) => Expr::literal(Value::Text(Text::utf8(&s))),
                Kind::Blob(b) => {
                    let mut expr = Expr::literal(Value::Blob(b));
                    let token = &self.tokens[self.at - 1];
                    expr.token = Some(self.sql[token.start..token.end].into());
                    expr
                }
                Kind::Number(s) => {
                    if s.trim_start_matches('0') == "9223372036854775808" {
                        let mut expr = self.make(ExprKind::MinMagnitude)?;
                        expr.token = Some(s);
                        expr
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
                        let small = matches!(v, Value::Integer(n) if (0..=i64::from(i32::MAX)).contains(&n));
                        let mut expr = Expr::literal(v);
                        if !small {
                            expr.token = Some(s);
                        }
                        expr
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
                        let all = !distinct && self.eat("ALL");
                        let star = self.eat("*");
                        if star && (distinct || all) {
                            return Err(error("invalid star argument"));
                        }
                        let args = if star || self.is(")") || self.is("ORDER") {
                            Vec::new()
                        } else {
                            self.expr_list()?
                        };
                        let mut order = if star { Vec::new() } else { self.order_by()? };
                        // SQLite discards ORDER BY on calls without arguments,
                        // before resolving even the names in the ordering terms.
                        if args.is_empty() {
                            order.clear();
                        }
                        self.expect(")")?;
                        let filter = if self.is("FILTER")
                            && self
                                .tokens
                                .get(self.at + 1)
                                .is_some_and(|t| matches!(t.kind, Kind::Symbol("(")))
                        {
                            self.at += 2;
                            self.expect("WHERE")?;
                            let filter = self.expr(0)?;
                            self.expect(")")?;
                            Some(Box::new(filter))
                        } else {
                            None
                        };
                        if args.len() > 1000 {
                            return Err(Error::Limit("function arguments"));
                        }
                        self.make(ExprKind::Call {
                            name,
                            args,
                            star,
                            distinct,
                            order,
                            filter,
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
        Ok(lhs)
    }
    fn expr_tail(&mut self, mut lhs: Expr, min: u8) -> Result<Expr> {
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
                if self.is("BETWEEN") || self.is("IN") || self.is("LIKE") || self.is("GLOB") {
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
                lhs = self.in_expr(lhs, negated)?;
                continue;
            }
            if (self.is("LIKE") || self.is("GLOB")) && min <= 35 {
                let name = self.name()?.to_ascii_lowercase();
                lhs = self.pattern_expr(lhs, name, negated)?;
                continue;
            }
            let op = if self.is("OR") {
                Some((10, Binary::Or))
            } else if self.is("AND") {
                Some((20, Binary::And))
            } else if self.is("IS") {
                Some((35, Binary::Is))
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
    fn pattern_expr(&mut self, lhs: Expr, name: String, negated: bool) -> Result<Expr> {
        let pattern = self.expr(36)?;
        let mut args = vec![pattern, lhs];
        if self.eat("ESCAPE") {
            args.push(self.expr(46)?);
        }
        let call = self.make(ExprKind::Call {
            name,
            args,
            star: false,
            distinct: false,
            order: Vec::new(),
            filter: None,
        })?;
        if negated {
            self.make(ExprKind::Unary(Unary::Not, Box::new(call)))
        } else {
            Ok(call)
        }
    }
    // Keep IN construction out of the recursive expression frame.
    fn in_expr(&mut self, mut lhs: Expr, negated: bool) -> Result<Expr> {
        let id = self.tokens[self.at].start;
        if !self.is("(") {
            let start = self.at;
            let name = self.table_name()?;
            let qualified = self.at == start + 3;
            if self.eat("(") {
                self.expect(")")?;
            }
            let select = Select {
                nested_from: false,
                items: vec![SelectItem {
                    expr: None,
                    star: None,
                    alias: None,
                    label: String::new(),
                }],
                sources: vec![Source {
                    alias: name.clone(),
                    name,
                    qualified,
                    query: None,
                    left: false,
                    right: false,
                    natural: false,
                    using: None,
                    on: None,
                }],
                filter: None,
                group: Vec::new(),
                having: None,
                order: Vec::new(),
                limit: None,
                offset: None,
                distinct: false,
            };
            let query = self.in_source(id, QueryCore::Select(Box::new(select)))?;
            lhs = self.make(ExprKind::InQuery(Box::new(lhs), Box::new(query), negated))?;
            return Ok(lhs);
        }
        self.expect("(")?;
        if self.is("SELECT") || self.is("WITH") || self.is("VALUES") {
            let query = self.subquery_expr(QueryMode::Set)?;
            self.expect(")")?;
            lhs = self.make(ExprKind::InQuery(Box::new(lhs), Box::new(query), negated))?;
            return Ok(lhs);
        }
        let values = if self.is(")") {
            Vec::new()
        } else {
            self.expr_list()?
        };
        self.expect(")")?;
        if values.is_empty() {
            fn has_function(expr: &Expr) -> bool {
                matches!(expr.kind, ExprKind::Call { .. })
                    || expr.children().iter().any(|e| has_function(e))
            }
            if !has_function(&lhs) {
                lhs = Expr::literal(Value::Integer(i64::from(negated)));
                return Ok(lhs);
            }
        }
        if values.len() == 1 {
            if let ExprKind::Subquery(mut subquery) = values[0].kind.clone() {
                subquery.mode = QueryMode::Set;
                let query = self.make(ExprKind::Subquery(subquery))?;
                lhs = self.make(ExprKind::InQuery(Box::new(lhs), Box::new(query), negated))?;
                return Ok(lhs);
            }
        }
        if let ExprKind::Vector(columns) = &lhs.kind {
            if !values.is_empty() {
                let mut rows = Vec::new();
                for value in values {
                    let ExprKind::Vector(row) = value.kind else {
                        return Err(error("IN row value size mismatch"));
                    };
                    if row.len() != columns.len() {
                        return Err(error("IN row value size mismatch"));
                    }
                    rows.push(row);
                }
                let query = self.in_source(id, QueryCore::Values(rows))?;
                lhs = self.make(ExprKind::InQuery(Box::new(lhs), Box::new(query), negated))?;
                return Ok(lhs);
            }
        }
        lhs = self.make(ExprKind::In(Box::new(lhs), values, negated))?;
        Ok(lhs)
    }
    fn in_source(&self, id: usize, core: QueryCore) -> Result<Expr> {
        self.make(ExprKind::Subquery(Subquery {
            id,
            mode: QueryMode::Set,
            query: Rc::new(Query {
                with: Vec::new(),
                cores: vec![core],
                operators: Vec::new(),
                order: Vec::new(),
                limit: None,
                offset: None,
            }),
        }))
    }
}
