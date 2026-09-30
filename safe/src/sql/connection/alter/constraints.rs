//! Token-level constraint edits preserve names, comments and unrelated clauses.
//! Parenthesized expressions/type sizes are single units, so their keywords and
//! commas cannot be mistaken for constraints or table-column separators.
use super::*;

struct Unit {
    kind: Kind,
    prefix: usize,
    start: usize,
}
impl Unit {
    fn word(&self, name: &str) -> bool {
        matches!(&self.kind, Kind::Word(s, false) if s.eq_ignore_ascii_case(name))
    }
    fn boundary(&self) -> bool {
        matches!(self.kind, Kind::Symbol("," | ")") | Kind::End)
            || [
                "CONSTRAINT",
                "PRIMARY",
                "NOT",
                "UNIQUE",
                "CHECK",
                "DEFAULT",
                "COLLATE",
                "REFERENCES",
                "FOREIGN",
                "GENERATED",
                "AS",
            ]
            .iter()
            .any(|s| self.word(s))
    }
    fn name(&self) -> Option<&str> {
        match &self.kind {
            Kind::Word(s, _) | Kind::String(s) => Some(s),
            _ => None,
        }
    }
}
fn units(sql: &str, limits: SqlLimits, fuel: &mut Fuel) -> Result<Vec<Unit>> {
    let tokens = lex(sql, limits)?;
    let open = tokens
        .iter()
        .position(|t| t.kind == Kind::Symbol("("))
        .ok_or(Error::Corrupt("CREATE table body missing"))?;
    let mut at = open + 1;
    let mut prefix = tokens[open].end;
    let mut result = Vec::new();
    while let Some(token) = tokens.get(at) {
        fuel.spend()?;
        let mut end = token.end;
        at += 1;
        if token.kind == Kind::Symbol("(") {
            let mut depth = 1usize;
            while depth > 0 {
                fuel.spend()?;
                let next = tokens
                    .get(at)
                    .ok_or(Error::Corrupt("unclosed schema expression"))?;
                match next.kind {
                    Kind::Symbol("(") => depth += 1,
                    Kind::Symbol(")") => depth -= 1,
                    Kind::End => return Err(Error::Corrupt("unclosed schema expression")),
                    _ => {}
                }
                end = next.end;
                at += 1;
            }
        }
        result.push(Unit {
            kind: token.kind.clone(),
            prefix,
            start: token.start,
        });
        prefix = end;
        if token.kind == Kind::Symbol(")") {
            return Ok(result);
        }
    }
    Err(Error::Corrupt("unclosed CREATE table"))
}
pub(super) enum Target<'a> {
    NotNull(usize),
    Name(&'a str),
}

pub(super) fn has_name(sql: &str, name: &str, limits: SqlLimits, fuel: &mut Fuel) -> Result<bool> {
    let tokens = units(sql, limits, fuel)?;
    Ok(tokens.windows(2).any(|pair| {
        pair[0].word("CONSTRAINT") && pair[1].name().is_some_and(|n| n.eq_ignore_ascii_case(name))
    }))
}
pub(super) fn drop(
    sql: &str,
    target: Target<'_>,
    limits: SqlLimits,
    fuel: &mut Fuel,
) -> Result<String> {
    let tokens = units(sql, limits, fuel)?;
    let mut at = 0usize;
    let mut column = 0usize;
    while let Some(token) = tokens.get(at) {
        fuel.spend()?;
        let start = token.prefix;
        at += 1;
        let mut selected = false;
        if token.word("CONSTRAINT")
            && match target {
                Target::Name(_) => true,
                Target::NotNull(n) => n == column,
            }
        {
            let name = tokens
                .get(at)
                .and_then(Unit::name)
                .ok_or(Error::Corrupt("constraint name missing"))?;
            at += 1;
            let kind = tokens
                .get(at)
                .ok_or(Error::Corrupt("constraint body missing"))?;
            let label_only = matches!(kind.kind, Kind::Symbol("," | ")"))
                || ["CONSTRAINT", "DEFAULT", "COLLATE", "GENERATED", "AS"]
                    .iter()
                    .any(|s| kind.word(s));
            selected = match target {
                Target::Name(wanted) => name.eq_ignore_ascii_case(wanted),
                Target::NotNull(_) => kind.word("NOT"),
            };
            if selected && !label_only && !kind.word("NOT") && !kind.word("CHECK") {
                return Err(error("constraint may not be dropped"));
            }
            if !label_only {
                at += 1;
                while tokens.get(at).is_some_and(|t| !t.boundary()) {
                    fuel.spend()?;
                    at += 1;
                }
            }
        } else if token.word("NOT") && matches!(target, Target::NotNull(n) if n == column) {
            while tokens.get(at).is_some_and(|t| !t.boundary()) {
                fuel.spend()?;
                at += 1;
            }
            selected = true;
        } else if token.kind == Kind::Symbol(",") {
            column += 1;
        }
        if selected {
            let next = tokens
                .get(at)
                .ok_or(Error::Corrupt("constraint terminator missing"))?;
            let delimiter = matches!(next.kind, Kind::Symbol("," | ")"));
            let start = if delimiter && start > 0 && sql.as_bytes()[start - 1] == b',' {
                start - 1
            } else {
                start
            };
            let mut result = String::from(sql);
            result.replace_range(start..next.start, if delimiter { "" } else { " " });
            return Ok(result);
        }
    }
    match target {
        Target::NotNull(_) => Ok(sql.into()),
        Target::Name(name) => Err(error(format!("no such constraint: {name}"))),
    }
}
pub(super) fn add(
    sql: &str,
    constraint: &str,
    column: Option<usize>,
    limits: SqlLimits,
    fuel: &mut Fuel,
) -> Result<String> {
    let tokens = units(sql, limits, fuel)?;
    let mut current = 0usize;
    for token in tokens {
        fuel.spend()?;
        if matches!(token.kind, Kind::Symbol("," | ")")) {
            if column == Some(current) || (column.is_none() && token.kind == Kind::Symbol(")")) {
                let mut result = String::from(sql);
                result.insert_str(
                    token.start,
                    &format!("{}{constraint}", if column.is_some() { " " } else { ", " }),
                );
                return Ok(result);
            }
            current += 1;
        }
    }
    Err(Error::Corrupt("constraint insertion point missing"))
}
