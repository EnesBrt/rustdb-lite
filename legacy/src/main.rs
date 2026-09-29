use sqlite_rust::{Connection, QueryResult, Value};
use std::ffi::CString;
use std::io::{self, IsTerminal, Read, Write};

#[derive(Clone, Copy)]
enum Mode {
    List,
    Json,
    Csv,
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(v) => v.to_string(),
        Value::Real(v) => {
            let s = v.to_string();
            if v.is_finite() && !s.contains(['.', 'e', 'E']) {
                format!("{s}.0")
            } else {
                s
            }
        }
        Value::Text(v) => v.clone(),
        Value::Blob(v) => format!("X'{}'", hex(v)),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Integer(v) => (*v).into(),
        Value::Real(v) => serde_json::json!(v),
        Value::Text(v) => v.clone().into(),
        Value::Blob(v) => serde_json::json!({"$blob": hex(v)}),
    }
}

fn print_result(result: QueryResult, mode: Mode, headers: bool) {
    if let Mode::Json = mode {
        let rows: Vec<_> = result
            .rows
            .iter()
            .map(|row| {
                result
                    .columns
                    .iter()
                    .zip(row)
                    .map(|(name, value)| (name.clone(), json(value)))
                    .collect::<serde_json::Map<_, _>>()
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&rows).expect("JSON serialization")
        );
        return;
    }
    let print_row = |fields: Vec<String>| {
        if let Mode::Csv = mode {
            println!(
                "{}",
                fields
                    .iter()
                    .map(|s| {
                        if s.contains([',', '"', '\n', '\r']) {
                            format!("\"{}\"", s.replace('"', "\"\""))
                        } else {
                            s.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            );
        } else {
            println!("{}", fields.join("|"));
        }
    };
    if headers {
        print_row(result.columns);
    }
    for row in result.rows {
        print_row(row.iter().map(text).collect());
    }
}

fn run(
    db: &Connection,
    sql: &str,
    mode: Mode,
    headers: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for result in db.run(sql)? {
        print_result(result, mode, headers);
    }
    Ok(())
}

fn main() {
    if let Err(e) = main_result() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn main_result() -> Result<(), Box<dyn std::error::Error>> {
    let mut mode = Mode::List;
    let mut headers = false;
    let mut positional = Vec::new();
    let mut options = true;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--" if options => options = false,
            "--json" | "-json" if options => mode = Mode::Json,
            "--csv" | "-csv" if options => mode = Mode::Csv,
            "--headers" | "-header" if options => headers = true,
            "--version" | "-version" if options => {
                println!("SQLite {} — Rust source port", Connection::version());
                return Ok(());
            }
            "--help" | "-help" | "-h" if options => {
                println!(
                    "Usage: sqlite-rust [--json|--csv] [--headers] [DATABASE [SQL]]\n\
                          DATABASE defaults to :memory:. SQL can also be piped on stdin.\n\
                          Interactive commands: .tables .schema .databases .read FILE\n\
                          .mode list|json|csv .headers on|off .help .quit"
                );
                return Ok(());
            }
            s if options && s.starts_with('-') => return Err(format!("unknown option: {s}").into()),
            _ => positional.push(arg),
        }
    }
    if positional.len() > 2 {
        return Err("expected DATABASE and optionally SQL".into());
    }
    let path = positional.first().map(String::as_str).unwrap_or(":memory:");
    let db = Connection::open(path)?;
    db.statement_scan_status(false)?;
    if let Some(sql) = positional.get(1) {
        return run(&db, sql, mode, headers);
    }
    if !io::stdin().is_terminal() {
        let mut sql = String::new();
        io::stdin().read_to_string(&mut sql)?;
        return run(&db, &sql, mode, headers);
    }
    println!(
        "SQLite {} — Rust source port. Enter .help for help.",
        Connection::version()
    );
    let mut pending = String::new();
    loop {
        print!(
            "{}",
            if pending.is_empty() {
                "sqlite-rust> "
            } else {
                "        ...> "
            }
        );
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            if !pending.trim().is_empty() {
                run(&db, &pending, mode, headers)?;
            }
            break;
        }
        if pending.is_empty() && line.trim_start().starts_with('.') {
            let (command, argument) = line
                .trim()
                .split_once(char::is_whitespace)
                .unwrap_or((line.trim(), ""));
            let result = match command {
                ".quit" | ".exit" => break,
                ".tables" => run(&db, "SELECT name FROM sqlite_schema WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name", mode, headers),
                ".schema" => run(&db, "SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY rowid", mode, false),
                ".databases" => run(&db, "PRAGMA database_list", mode, headers),
                ".read" => std::fs::read_to_string(argument.trim()).map_err(Into::into).and_then(|sql| run(&db, &sql, mode, headers)),
                ".mode" => match argument.trim() {
                    "json" => { mode = Mode::Json; Ok(()) },
                    "csv" => { mode = Mode::Csv; Ok(()) },
                    "list" => { mode = Mode::List; Ok(()) },
                    _ => Err("mode must be list, json, or csv".into()),
                },
                ".headers" => match argument.trim() {
                    "on" => { headers = true; Ok(()) },
                    "off" => { headers = false; Ok(()) },
                    _ => Err("headers must be on or off".into()),
                },
                ".help" => { println!(".tables .schema .databases .read FILE .mode list|json|csv .headers on|off .quit"); Ok(()) },
                _ => Err(format!("unknown command: {command}").into()),
            };
            if let Err(e) = result {
                eprintln!("Error: {e}");
            }
            continue;
        }
        pending.push_str(&line);
        let complete = CString::new(pending.as_str())
            .map(|s| unsafe { sqlite_rust::engine::sqlite3_complete(s.as_ptr()) != 0 })
            .unwrap_or(true);
        if complete {
            if let Err(e) = run(&db, &pending, mode, headers) {
                eprintln!("Error: {e}");
            }
            pending.clear();
        }
    }
    Ok(())
}
