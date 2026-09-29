#![forbid(unsafe_code)]
use sqlite_safe::{ImageBuilder, Table, Text, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .ok_or("expected output filename and page size")?;
    let page_size: usize = args.get(1).ok_or("expected page size")?.parse()?;
    let mut builder = ImageBuilder::new(page_size)?;
    let mut table = Table::new("items", &["number", "text", "bytes", "float", "nothing"]);
    for n in 0..2000i64 {
        table.rows.push((
            n - 1000,
            vec![
                Value::Integer(n),
                Value::Text(Text::utf8(&format!("héllo {n} 🦀\0尾"))),
                Value::Blob(vec![(n % 256) as u8; if n % 17 == 0 { 10000 } else { 8 }]),
                Value::Real(n as f64 + 0.25),
                Value::Null,
            ],
        ));
    }
    builder.add_table(table)?;
    for n in 0..150 {
        builder.add_table(Table::new(&format!("empty {n}"), &["quoted\"column"]))?;
    }
    let image = builder.finish()?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    output.write_all(&image)?;
    output.sync_all()?;
    Ok(())
}
