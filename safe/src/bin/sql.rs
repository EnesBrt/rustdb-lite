#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, Header, Value};
use std::{
    io::{self, Read, Write},
    path::Path,
};
fn quote(s: &str) -> String {
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
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02X}")).collect()
}
fn value(v: &Value) -> Result<String, Box<dyn std::error::Error>> {
    Ok(match v {
        Value::Null => "{\"type\":\"null\"}".into(),
        Value::Integer(n) => format!("{{\"type\":\"integer\",\"value\":{n}}}"),
        Value::Real(n) => format!("{{\"type\":\"real\",\"bits\":\"{:016X}\"}}", n.to_bits()),
        Value::Text(t) => format!("{{\"type\":\"text\",\"hex\":\"{}\"}}", hex(&t.bytes)),
        Value::Blob(b) => format!("{{\"type\":\"blob\",\"hex\":\"{}\"}}", hex(b)),
    })
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut load = None;
    let mut save = None;
    let mut at = 0;
    while at < args.len() {
        if args[at] == "--help" {
            println!("Usage: sqlite-safe-sql [--load OFFLINE.db] [--save NEW.db] < statements.sql\nExperimental safe in-memory SQL engine. JSON records use tagged values.\nSnapshot import/export only; not the upstream sqlite3 shell or a live file database.");
            return Ok(());
        }
        if at + 1 == args.len() {
            return Err("expected path after option".into());
        }
        if args[at] == "--load" {
            load = Some(Path::new(&args[at + 1]));
        } else if args[at] == "--save" {
            save = Some(Path::new(&args[at + 1]));
        } else {
            return Err("unknown option".into());
        }
        at += 2;
    }
    let (mut connection, page_size) = if let Some(path) = load {
        for suffix in ["-wal", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            match std::fs::metadata(&sidecar) {
                Ok(m) if m.len() > 0 => return Err(
                    "input has a journal/WAL sidecar; use a consistent checkpointed offline copy"
                        .into(),
                ),
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 256 * 1024 * 1024 {
            return Err("snapshot exceeds 256 MiB".into());
        }
        let page_size = Header::parse(&bytes)?.page_size;
        (Connection::from_image(&bytes)?, page_size)
    } else {
        (Connection::new(), 4096)
    };
    let mut sql = String::new();
    io::stdin().take(1024 * 1024 + 1).read_to_string(&mut sql)?;
    let results = connection.execute_batch(&sql)?;
    let mut out = io::stdout().lock();
    for result in results {
        write!(
            out,
            "{{\"columns\":[{}],\"changes\":{},\"rows\":[",
            result
                .columns
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(","),
            result.changes
        )?;
        for (i, row) in result.rows.iter().enumerate() {
            if i != 0 {
                write!(out, ",")?;
            }
            write!(
                out,
                "[{}]",
                row.iter()
                    .map(value)
                    .collect::<Result<Vec<_>, _>>()?
                    .join(",")
            )?;
        }
        writeln!(out, "]}}")?;
    }
    if let Some(path) = save {
        if !connection.is_autocommit() {
            return Err("finish the transaction before saving a snapshot".into());
        }
        let bytes = connection.to_image(page_size)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    Ok(())
}
