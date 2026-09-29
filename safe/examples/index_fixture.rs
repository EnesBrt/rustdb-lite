//! Test-data generator; C SQLite is used only by the external differential test.
#![forbid(unsafe_code)]
use sqlite_safe::{sql::Connection, AutoVacuum, Encoding, ImageBuilder, Text, Value};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let page_size: usize = args[2].parse()?;
    let mode = match args.get(3).map(String::as_str).unwrap_or("0") {
        "0" => AutoVacuum::None,
        "1" => AutoVacuum::Full,
        "2" => AutoVacuum::Incremental,
        _ => return Err("expected auto-vacuum mode 0, 1 or 2".into()),
    };
    let encoding = match args.get(4).map(String::as_str).unwrap_or("UTF-8") {
        "UTF-8" => Encoding::Utf8,
        "UTF-16le" => Encoding::Utf16Le,
        "UTF-16be" => Encoding::Utf16Be,
        _ => return Err("unknown encoding".into()),
    };
    let seed = ImageBuilder::new(page_size)?
        .auto_vacuum(mode)
        .encoding(encoding)?
        .finish()?;
    let mut c = Connection::from_image(&seed)?;
    c.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY,a,b TEXT COLLATE NOCASE,c BLOB)",
        &[],
    )?;
    let insert = c.prepare("INSERT INTO t VALUES(?,?,?,?)")?;
    for n in 0..1500i64 {
        let a = match n % 7 {
            0 => Value::Null,
            1 => Value::Text(Text::utf8(&format!("value{}", n % 19))),
            2 => Value::Blob(vec![n as u8, 0, 255]),
            3 => Value::Real(n as f64 / -3.0),
            _ => Value::Integer(n % 17),
        };
        let b = format!(
            "{}{}",
            if n % 2 == 0 { "aBc" } else { "AbC" },
            "🦀".repeat((n as usize * 47) % 650)
        );
        c.execute_prepared(
            &insert,
            &[
                Value::Integer(n - 750),
                a,
                Value::Text(Text::utf8(&b)),
                Value::Blob(vec![n as u8; (n as usize * 61) % 900]),
            ],
        )?;
    }
    c.execute_batch("CREATE INDEX mixed ON t(a DESC,b COLLATE RTRIM,c);CREATE UNIQUE INDEX uid ON t(b COLLATE BINARY DESC,id DESC);CREATE INDEX repeat_col ON t(a,a DESC,id);CREATE INDEX cover_id ON t(id);")?;
    for (n, definition) in [
        "x TEXT PRIMARY KEY DESC UNIQUE",
        "x TEXT UNIQUE PRIMARY KEY DESC",
        "x INTEGER PRIMARY KEY UNIQUE",
        "x INTEGER UNIQUE PRIMARY KEY DESC",
        "x INTEGER PRIMARY KEY DESC UNIQUE",
        "x TEXT UNIQUE COLLATE NOCASE",
        "x TEXT COLLATE RTRIM UNIQUE",
        "x UNIQUE,y UNIQUE,z TEXT PRIMARY KEY",
    ]
    .iter()
    .enumerate()
    {
        c.execute(&format!("CREATE TABLE auto{n}({definition})"), &[])?;
        c.execute(
            &format!("INSERT INTO auto{n}(x) VALUES(2),(1),(NULL),(NULL)"),
            &[],
        )?;
    }
    let image = c.to_image(page_size)?;
    std::fs::write(&args[1], image)?;
    Ok(())
}
