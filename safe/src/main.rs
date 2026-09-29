#![forbid(unsafe_code)]
use sqlite_safe::{Database, Value};
use std::io::{self, Write};
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch < '\u{20}' => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
fn value(value: &Value) -> String {
    match value {
        Value::Null => "{\"type\":\"null\"}".into(),
        Value::Integer(n) => format!("{{\"type\":\"integer\",\"value\":{n}}}"),
        Value::Real(n) => format!("{{\"type\":\"real\",\"bits\":\"{:016X}\"}}", n.to_bits()),
        Value::Blob(b) => format!("{{\"type\":\"blob\",\"hex\":\"{}\"}}", hex(b)),
        Value::Text(text) => match text.to_string() {
            Ok(s) => format!("{{\"type\":\"text\",\"value\":{}}}", quoted(&s)),
            Err(_) => format!(
                "{{\"type\":\"text\",\"encoding\":\"{:?}\",\"hex\":\"{}\"}}",
                text.encoding,
                hex(&text.bytes)
            ),
        },
    }
}
fn read_limited(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Read;
    const MAX_FILE: u64 = 256 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("snapshot exceeds the inspector's 256 MiB limit".into());
    }
    Ok(bytes)
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args[0] == "--help" {
        println!("Usage: sqlite-safe-inspect DATABASE info|schema|rows [ROOT_PAGE] [--wal WAL_SNAPSHOT]\nReads immutable offline snapshots only. This is not a SQL shell or transaction engine.");
        return Ok(());
    }
    if args.len() < 2 {
        return Err("expected DATABASE and info, schema, or rows".into());
    }
    let path = Path::new(&args[0]);
    let command = args[1].to_str().ok_or("invalid command")?;
    let mut next = 2;
    let root = if command == "rows" {
        let root: u32 = args
            .get(next)
            .and_then(|s| s.to_str())
            .ok_or("rows requires a root page number")?
            .parse()?;
        next += 1;
        Some(root)
    } else {
        None
    };
    let wal = if args.get(next).is_some_and(|s| s == "--wal") {
        let path = args.get(next + 1).ok_or("--wal requires a snapshot path")?;
        next += 2;
        Some(Path::new(path))
    } else {
        None
    };
    if args.len() != next {
        return Err("unexpected arguments".into());
    }
    for suffix in ["-wal", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match std::fs::metadata(&sidecar) {
            Ok(m) if m.len() != 0 && !(suffix == "-wal" && wal.is_some()) => return Err("nonempty database sidecar: use a checkpointed offline copy, or explicitly provide an immutable WAL snapshot".into()),
            Ok(_) => {}, Err(e) if e.kind() == io::ErrorKind::NotFound => {}, Err(e) => return Err(e.into()),
        }
    }
    let mut bytes = read_limited(path)?;
    if let Some(wal) = wal {
        bytes = sqlite_safe::wal::recover(&bytes, &read_limited(wal)?, 256 * 1024 * 1024, 100_000)?
            .image;
    }
    let db = Database::parse(&bytes)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match command {
        "info" => writeln!(
            out,
            "{{\"page_size\":{},\"pages\":{},\"encoding\":\"{:?}\",\"schema_format\":{}}}",
            db.header().page_size,
            db.header().page_count,
            db.header().encoding,
            db.header().schema_format
        )?,
        "schema" => {
            for entry in db.schema()? {
                writeln!(
                    out,
                    "{{\"kind\":{},\"name\":{},\"table\":{},\"root_page\":{},\"sql\":{}}}",
                    quoted(&entry.kind),
                    quoted(&entry.name),
                    quoted(&entry.table_name),
                    entry.root_page,
                    entry
                        .sql
                        .as_deref()
                        .map(quoted)
                        .unwrap_or_else(|| "null".into())
                )?;
            }
        }
        "rows" => {
            let mut io_error = None;
            db.visit_rows(root.ok_or("missing root")?, |row| {
                if let Err(error) = writeln!(
                    out,
                    "{{\"rowid\":{},\"values\":[{}]}}",
                    row.rowid
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "null".into()),
                    row.values.iter().map(value).collect::<Vec<_>>().join(",")
                ) {
                    io_error = Some(error);
                    return Err(sqlite_safe::Error::InvalidInput("output failure"));
                }
                Ok(())
            })
            .map_err(|error| -> Box<dyn std::error::Error> {
                match io_error {
                    Some(io) => Box::new(io),
                    None => Box::new(error),
                }
            })?;
        }
        _ => return Err("unknown command; expected info, schema, or rows".into()),
    }
    Ok(())
}
