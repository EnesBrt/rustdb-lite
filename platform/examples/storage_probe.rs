//! Test driver for lock contention and process interruption, not a user utility.
#![forbid(unsafe_code)]
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
mod supported {
    use sqlite_safe::{
        journal::JournalLimits,
        pager::{Pager, Storage},
        Result,
    };
    use sqlite_safe_platform::unix::UnixStorage;
    use std::io::Write;
    struct Interrupted {
        inner: UnixStorage,
        after: usize,
        count: usize,
        partial: bool,
    }
    impl Interrupted {
        fn step(&mut self, result: Result<()>) -> Result<()> {
            result?;
            self.count += 1;
            if self.count == self.after {
                std::process::exit(99);
            }
            Ok(())
        }
    }
    impl Storage for Interrupted {
        fn lock_exclusive(&mut self) -> Result<()> {
            self.inner.lock_exclusive()
        }
        fn unlock(&mut self) {
            self.inner.unlock();
        }
        fn read_database(&mut self, n: usize) -> Result<Vec<u8>> {
            self.inner.read_database(n)
        }
        fn read_journal(&mut self, n: usize) -> Result<Option<Vec<u8>>> {
            self.inner.read_journal(n)
        }
        fn has_wal(&mut self) -> Result<bool> {
            self.inner.has_wal()
        }
        fn sector_size(&self) -> usize {
            self.inner.sector_size()
        }
        fn random_salt(&mut self) -> Result<u32> {
            self.inner.random_salt()
        }
        fn create_journal(&mut self, b: &[u8]) -> Result<()> {
            let r = self.inner.create_journal(b);
            self.step(r)
        }
        fn write_journal_header(&mut self, b: &[u8; 12]) -> Result<()> {
            let r = self.inner.write_journal_header(b);
            self.step(r)
        }
        fn sync_journal(&mut self) -> Result<()> {
            let r = self.inner.sync_journal();
            self.step(r)
        }
        fn write_database(&mut self, o: usize, b: &[u8]) -> Result<()> {
            if self.partial && self.count + 1 == self.after {
                self.inner.write_database(o, &b[..b.len() / 2])?;
                std::process::exit(99);
            }
            let r = self.inner.write_database(o, b);
            self.step(r)
        }
        fn truncate_database(&mut self, n: usize) -> Result<()> {
            let r = self.inner.truncate_database(n);
            self.step(r)
        }
        fn sync_database(&mut self) -> Result<()> {
            let r = self.inner.sync_database();
            self.step(r)
        }
        fn remove_journal(&mut self) -> Result<()> {
            let r = self.inner.remove_journal();
            self.step(r)
        }
        fn sync_directory(&mut self) -> Result<()> {
            let r = self.inner.sync_directory();
            self.step(r)
        }
    }
    pub fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let a: Vec<_> = std::env::args().collect();
        let storage = UnixStorage::new(&a[2])?;
        match a[1].as_str() {
            "hold" | "double-hold" => {
                let first = Pager::open(storage, JournalLimits::default())?;
                if a[1] == "double-hold" {
                    assert!(
                        Pager::open(UnixStorage::new(&a[2])?, JournalLimits::default()).is_err()
                    );
                }
                println!("LOCKED");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                drop(first);
            }
            "commit" => {
                let next = std::fs::read(&a[3])?;
                let storage = Interrupted {
                    inner: storage,
                    after: a[4].parse()?,
                    count: 0,
                    partial: a.get(5).is_some_and(|s| s == "partial"),
                };
                let mut pager = Pager::open(storage, JournalLimits::default())?;
                pager.commit(&next)?;
                println!("COMMITTED");
            }
            "recover" => {
                drop(Pager::open(storage, JournalLimits::default())?);
            }
            _ => return Err("unknown probe operation".into()),
        }
        Ok(())
    }
}
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    supported::run()
}
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn main() {
    eprintln!("adapter unsupported");
    std::process::exit(1);
}
