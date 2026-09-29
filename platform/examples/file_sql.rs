//! Exercise real-file SQL. One complete SQL statement per command-line argument.
#![forbid(unsafe_code)]
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sqlite_safe::{journal::JournalLimits, sql::JournaledConnection};
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or("expected database path")?;
    let storage = sqlite_safe_platform::unix::UnixStorage::new(path)?;
    let mut db = JournaledConnection::open(storage, 4096, JournalLimits::default())?;
    for sql in args {
        let result = db.execute(sql.to_str().ok_or("SQL must be UTF-8")?, &[])?;
        println!("{:?}", result);
    }
    Ok(())
}
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn main() {
    eprintln!("No storage adapter for this target yet");
    std::process::exit(1);
}
