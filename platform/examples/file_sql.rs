//! Exercise real-file SQL. One complete SQL statement per command-line argument.
//! Optional --continue after the path executes later statements after errors,
//! allowing tests to commit or roll back a retained transaction prefix.
#![forbid(unsafe_code)]
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sqlite_safe::{journal::JournalLimits, sql::JournaledConnection};
    let mut args = std::env::args_os().skip(1).peekable();
    let path = args.next().ok_or("expected database path")?;
    let keep_going = args.next_if(|s| s == "--continue").is_some();
    let storage = sqlite_safe_platform::unix::UnixStorage::new(path)?;
    let mut db = JournaledConnection::open(storage, 4096, JournalLimits::default())?;
    let mut failure = None;
    for sql in args {
        match db.execute(sql.to_str().ok_or("SQL must be UTF-8")?, &[]) {
            Ok(result) => println!("{result:?}"),
            Err(error) if keep_going => {
                eprintln!("{error}");
                failure = Some(error);
            }
            Err(error) => return Err(error.into()),
        }
    }
    if let Some(error) = failure {
        return Err(error.into());
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
