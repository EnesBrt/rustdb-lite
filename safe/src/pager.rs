//! Rollback-journal commit protocol over a caller-supplied storage implementation.
//!
//! OS adapters live in the separate sqlite-safe-platform crate. Durability and exclusion
//! depend on the adapter implementing every contract below. This is a bounded
//! full-image pager, not SQLite's page cache, shared-lock protocol, or WAL writer.
#![doc = include_str!("../PAGER.md")]
use crate::{
    journal::{self, JournalLimits},
    Database, Error, Result,
};
use alloc::vec::Vec;

/// Storage for one main database and its `-journal` file. Methods may fail after
/// partial writes. Sync operations must be real persistence barriers, including
/// directory entries, and successful writes must write the whole supplied slice.
/// An adapter must reject unsupported durability/locking guarantees.
///
/// Exclusive locking must exclude *all* readers and writers, including native
/// SQLite if interoperating with it. A sidecar lock or `flock` alone does not
/// implement SQLite's platform-specific byte-range locking. Lock acquisition
/// must be all-or-nothing; `unlock` releases all acquired locks.
///
/// The main file must already exist, with its directory entry made durable.
/// File identities and paths must stay stable throughout the exclusive lock.
pub trait Storage {
    fn lock_exclusive(&mut self) -> Result<()>;
    fn unlock(&mut self);
    /// Read at most `limit` bytes, returning an error if the file is larger.
    fn read_database(&mut self, limit: usize) -> Result<Vec<u8>>;
    fn read_journal(&mut self, limit: usize) -> Result<Option<Vec<u8>>>;
    /// True for any nonempty WAL sidecar; called while exclusively locked.
    fn has_wal(&mut self) -> Result<bool>;
    /// Power of two in 512..=65536, at least the storage's atomic sector size.
    fn sector_size(&self) -> usize;
    fn random_salt(&mut self) -> Result<u32>;
    /// Exclusive creation: fail if a journal already exists. Never overwrite it.
    fn create_journal(&mut self, bytes: &[u8]) -> Result<()>;
    fn write_journal_header(&mut self, header: &[u8; 12]) -> Result<()>;
    fn sync_journal(&mut self) -> Result<()>;
    fn write_database(&mut self, offset: usize, bytes: &[u8]) -> Result<()>;
    fn truncate_database(&mut self, length: usize) -> Result<()>;
    fn sync_database(&mut self) -> Result<()>;
    fn remove_journal(&mut self) -> Result<()>;
    fn sync_directory(&mut self) -> Result<()>;
}

/// Holds an exclusive lock until dropped. Errors during commit poison this
/// instance: reopen storage to recover, and determine whether an uncertain
/// commit took effect. Do not retry a statement blindly after an I/O failure.
pub struct Pager<S: Storage> {
    storage: S,
    image: Vec<u8>,
    limits: JournalLimits,
    poisoned: bool,
}
impl<S: Storage> Pager<S> {
    pub fn open(mut storage: S, limits: JournalLimits) -> Result<Self> {
        storage.lock_exclusive()?;
        let result = Self::recover_locked(&mut storage, limits);
        match result {
            Ok(image) => Ok(Self {
                storage,
                image,
                limits,
                poisoned: false,
            }),
            Err(error) => {
                storage.unlock();
                Err(error)
            }
        }
    }
    fn recover_locked(storage: &mut S, limits: JournalLimits) -> Result<Vec<u8>> {
        if storage.has_wal()? {
            return Err(Error::Unsupported("pager WAL sidecar"));
        }
        let mut image = storage.read_database(limits.max_image_bytes)?;
        if image.len() > limits.max_image_bytes {
            return Err(Error::Limit("pager image bytes"));
        }
        if let Some(journal) = storage.read_journal(limits.max_journal_bytes)? {
            let recovery = journal::recover(&image, &journal, limits)?;
            if recovery.active {
                storage.sync_journal()?;
                storage.sync_directory()?;
                image = recovery.image;
                let page_size = if image.is_empty() {
                    512
                } else {
                    Database::parse(&image)?.header().page_size
                };
                for (i, page) in image.chunks(page_size).enumerate() {
                    storage.write_database(i * page_size, page)?;
                }
                storage.truncate_database(image.len())?;
                storage.sync_database()?;
            }
            storage.remove_journal()?;
            storage.sync_directory()?;
        }
        if !image.is_empty() {
            Database::parse(&image)?;
            if image[18] != 1 || image[19] != 1 {
                return Err(Error::Unsupported("pager requires rollback mode"));
            }
        }
        Ok(image)
    }
    pub fn image(&self) -> Result<&[u8]> {
        if self.poisoned {
            return Err(Error::Storage(
                "pager requires reopen after failed commit".into(),
            ));
        }
        Ok(&self.image)
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    /// Commit a complete replacement image of the same page size. Only returns
    /// success after the database and journal deletion have been made durable.
    /// The change counter and schema cookie are advanced from the previous image.
    pub fn commit(&mut self, next: &[u8]) -> Result<()> {
        self.image()?;
        if next.len() > self.limits.max_image_bytes {
            return Err(Error::Limit("pager image bytes"));
        }
        let header = Database::parse(next)?.header().clone();
        if next[18] != 1 || next[19] != 1 {
            return Err(Error::Unsupported("pager requires rollback mode"));
        }
        if !self.image.is_empty() {
            let old = Database::parse(&self.image)?;
            if old.header().page_size != header.page_size {
                return Err(Error::Unsupported("pager page-size changes"));
            }
            if old.header().encoding != header.encoding
                || old.header().reserved_bytes != header.reserved_bytes
            {
                return Err(Error::Unsupported(
                    "pager encoding or reserved-byte changes",
                ));
            }
            if old.header().auto_vacuum != header.auto_vacuum {
                return Err(Error::Unsupported("pager auto-vacuum mode changes"));
            }
        }
        let sector = self.storage.sector_size();
        if !(512..=65536).contains(&sector) || !sector.is_power_of_two() {
            return Err(Error::Unsupported("storage sector size"));
        }
        let salt = self.storage.random_salt()?;
        let mut journal =
            journal::encode(&self.image, header.page_size, sector, salt, self.limits)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(next.len())
            .map_err(|_| Error::Limit("allocation"))?;
        owned.extend_from_slice(next);
        let counter = if self.image.is_empty() {
            1
        } else {
            crate::error::u32be(&self.image, 24)?.wrapping_add(1)
        };
        let schema = if self.image.is_empty() {
            1
        } else {
            crate::error::u32be(&self.image, 40)?.wrapping_add(1)
        };
        owned[24..28].copy_from_slice(&counter.to_be_bytes());
        owned[92..96].copy_from_slice(&counter.to_be_bytes());
        owned[40..44].copy_from_slice(&schema.to_be_bytes());
        let mut valid = [0u8; 12];
        valid.copy_from_slice(&journal[..12]);
        journal[..12].fill(0);
        // After this point any error leaves the object unusable. In particular,
        // an error after journal removal means the commit outcome is uncertain.
        self.poisoned = true;
        self.storage.create_journal(&journal)?;
        self.storage.sync_journal()?;
        self.storage.sync_directory()?;
        self.storage.write_journal_header(&valid)?;
        self.storage.sync_journal()?;
        for (i, page) in owned.chunks_exact(header.page_size).enumerate() {
            self.storage.write_database(i * header.page_size, page)?;
        }
        self.storage.truncate_database(owned.len())?;
        self.storage.sync_database()?;
        self.storage.remove_journal()?;
        self.storage.sync_directory()?;
        self.image = owned;
        self.poisoned = false;
        Ok(())
    }
}
impl<S: Storage> Drop for Pager<S> {
    fn drop(&mut self) {
        self.storage.unlock();
    }
}
