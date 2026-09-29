//! Test driver: continue after each statement error and expose connection state.
#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, Value};
use std::io::{self, BufRead};

fn json(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < '\u{20}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
fn value(v: &Value) -> String {
    match v {
        Value::Null => "{\"type\":\"null\"}".into(),
        Value::Integer(i) => format!("{{\"type\":\"integer\",\"value\":{i}}}"),
        Value::Real(r) => format!("{{\"type\":\"real\",\"bits\":\"{:016X}\"}}", r.to_bits()),
        Value::Text(t) => format!("{{\"type\":\"text\",\"hex\":\"{}\"}}", hex(&t.bytes)),
        Value::Blob(b) => format!("{{\"type\":\"blob\",\"hex\":\"{}\"}}", hex(b)),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Connection::new();
    for line in io::stdin().lock().lines() {
        let result = db.execute(&line?, &[]);
        let (columns, rows) = match &result {
            Ok(r) => (
                r.columns
                    .iter()
                    .map(|c| json(c))
                    .collect::<Vec<_>>()
                    .join(","),
                r.rows
                    .iter()
                    .map(|row| format!("[{}]", row.iter().map(value).collect::<Vec<_>>().join(",")))
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            Err(_) => (String::new(), String::new()),
        };
        println!("{{\"ok\":{},\"autocommit\":{},\"changes\":{},\"total\":{},\"last_rowid\":{},\"columns\":[{}],\"rows\":[{}]}}",result.is_ok(),db.is_autocommit(),db.changes(),db.total_changes(),db.last_insert_rowid(),columns,rows);
    }
    Ok(())
}
