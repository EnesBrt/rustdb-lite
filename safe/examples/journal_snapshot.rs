//! Offline journal fixture codec used by the native differential tests.
#![forbid(unsafe_code)]
use sqlite_safe::{
    journal::{self, JournalLimits},
    Database,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().collect();
    let main = std::fs::read(&a[2])?;
    if a[1] == "encode" {
        let page_size = Database::parse(&main)?.header().page_size;
        std::fs::write(
            &a[3],
            journal::encode(&main, page_size, 512, 0x84929187, JournalLimits::default())?,
        )?;
    } else if a[1] == "recover" {
        let journal = std::fs::read(&a[3])?;
        let recovery = journal::recover(&main, &journal, JournalLimits::default())?;
        std::fs::write(&a[4], recovery.image)?;
    } else {
        return Err("expected encode or recover".into());
    }
    Ok(())
}
