//! Exclusive rollback-mode file storage for Linux/Android and macOS/iOS.
//!
//! A POSIX whole-file write lock overlaps SQLite's byte-range locks in other
//! processes. Connections in this crate also share an inode registry. Do not
//! open/close this database through unrelated code or a separately loaded SQLite
//! library in the same process: POSIX locks are process-associated and closing
//! any descriptor of an inode can release them. Filesystem paths must stay stable.
use rustix::{
    fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags},
    io::Errno,
};
use sqlite_safe::{pager::Storage, Error, Result};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{File, Metadata},
    io::Read,
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
    sync::Mutex,
};

type Identity = (u64, u64);
// A collision detected only after open must not close its descriptor while an
// older connection holds a POSIX lock. Retain it until that connection unlocks.
// After such a race, refuse further opens until the deferred descriptor is closed.
static REGISTRY: Mutex<BTreeMap<Identity, Vec<File>>> = Mutex::new(BTreeMap::new());
fn io_error(operation: &str, error: impl std::fmt::Display) -> Error {
    Error::Storage(format!("{operation}: {error}"))
}
fn identity(m: &Metadata) -> Identity {
    (m.dev(), m.ino())
}
#[allow(clippy::unnecessary_cast)] // Native stat field widths differ across targets.
fn stat_identity(s: &fs::Stat) -> Identity {
    (s.st_dev as u64, s.st_ino as u64)
}
fn plain(s: &fs::Stat) -> Result<()> {
    if !FileType::from_raw_mode(s.st_mode).is_file() || s.st_nlink != 1 {
        return Err(Error::Unsupported(
            "storage requires a regular file with one hard link",
        ));
    }
    Ok(())
}
fn sync_file(file: &File) -> Result<()> {
    fs::fsync(file).map_err(|e| io_error("fsync", e))?;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fs::fcntl_fullfsync(file).map_err(|e| io_error("F_FULLFSYNC", e))?;
    Ok(())
}

/// A filesystem adapter retaining exclusive access for the connection lifetime.
/// Symlink main files and hard-linked database/sidecar files are rejected. Native
/// SQLite interoperability applies to separate cooperating processes on local
/// filesystems using POSIX locks. Network filesystem guarantees are not established.
pub struct UnixStorage {
    parent: PathBuf,
    directory: File,
    directory_id: Identity,
    name: OsString,
    journal_name: OsString,
    wal_name: OsString,
    main: Option<File>,
    main_id: Option<Identity>,
    journal: Option<File>,
    journal_id: Option<Identity>,
}
impl UnixStorage {
    /// Prepare a path. Opening/creating the main file is deferred until the pager
    /// acquires its lock. The parent directory must already exist.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .ok_or(Error::InvalidInput("database filename"))?
            .to_os_string();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent
            .canonicalize()
            .map_err(|e| io_error("resolve database directory", e))?;
        let directory: File = fs::open(
            &parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| io_error("open database directory", e))?
        .into();
        let directory_id = identity(
            &directory
                .metadata()
                .map_err(|e| io_error("directory metadata", e))?,
        );
        let mut journal_name = name.clone();
        journal_name.push("-journal");
        let mut wal_name = name.clone();
        wal_name.push("-wal");
        Ok(Self {
            parent,
            directory,
            directory_id,
            name,
            journal_name,
            wal_name,
            main: None,
            main_id: None,
            journal: None,
            journal_id: None,
        })
    }
    fn stat(&self, name: &OsStr) -> Result<Option<fs::Stat>> {
        match fs::statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(s) => Ok(Some(s)),
            Err(Errno::NOENT) => Ok(None),
            Err(e) => Err(io_error("stat database path", e)),
        }
    }
    fn check_directory(&self) -> Result<()> {
        let metadata = std::fs::symlink_metadata(&self.parent)
            .map_err(|e| io_error("database directory identity", e))?;
        if !metadata.is_dir() || identity(&metadata) != self.directory_id {
            return Err(Error::Storage(
                "database directory was moved or replaced".into(),
            ));
        }
        Ok(())
    }
    fn main(&self) -> Result<&File> {
        self.check_directory()?;
        let file = self
            .main
            .as_ref()
            .ok_or(Error::Storage("storage is not locked".into()))?;
        let stat = self
            .stat(&self.name)?
            .ok_or(Error::Storage("database path was removed".into()))?;
        plain(&stat)?;
        if Some(stat_identity(&stat)) != self.main_id {
            return Err(Error::Storage("database path was replaced".into()));
        }
        Ok(file)
    }
    fn journal(&self) -> Result<&File> {
        self.main()?;
        let file = self
            .journal
            .as_ref()
            .ok_or(Error::Storage("journal is not open".into()))?;
        let stat = self
            .stat(&self.journal_name)?
            .ok_or(Error::Storage("journal path was removed".into()))?;
        plain(&stat)?;
        if Some(stat_identity(&stat)) != self.journal_id {
            return Err(Error::Storage("journal path was replaced".into()));
        }
        Ok(file)
    }
    fn read(file: &File, limit: usize) -> Result<Vec<u8>> {
        let length = usize::try_from(
            file.metadata()
                .map_err(|e| io_error("file length", e))?
                .len(),
        )
        .map_err(|_| Error::Limit("file bytes"))?;
        if length > limit {
            return Err(Error::Limit("file bytes"));
        }
        let mut data = Vec::new();
        data.try_reserve_exact(length)
            .map_err(|_| Error::Limit("allocation"))?;
        data.resize(length, 0);
        file.read_exact_at(&mut data, 0)
            .map_err(|e| io_error("read file", e))?;
        if file
            .metadata()
            .map_err(|e| io_error("file length", e))?
            .len()
            != length as u64
        {
            return Err(Error::Storage("file changed while locked".into()));
        }
        Ok(data)
    }
    fn open_existing_journal(&mut self) -> Result<bool> {
        self.main()?;
        if self.journal.is_some() {
            self.journal()?;
            return Ok(true);
        }
        let before = match self.stat(&self.journal_name)? {
            Some(s) => s,
            None => return Ok(false),
        };
        plain(&before)?;
        let mut registry = REGISTRY
            .lock()
            .map_err(|_| Error::Storage("inode registry poisoned".into()))?;
        if registry.contains_key(&stat_identity(&before)) {
            return Err(Error::Busy("journal aliases an open database"));
        }
        if registry.values().any(|files| !files.is_empty()) {
            return Err(Error::Busy("file identity race awaiting close"));
        }
        let file: File = fs::openat(
            &self.directory,
            &self.journal_name,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|e| io_error("open journal", e))?
        .into();
        let metadata = file
            .metadata()
            .map_err(|e| io_error("journal metadata", e))?;
        let id = identity(&metadata);
        if let Some(deferred) = registry.get_mut(&id) {
            deferred.push(file);
            return Err(Error::Storage(
                "journal identity changed during open".into(),
            ));
        }
        if !metadata.is_file() || metadata.nlink() != 1 || id != stat_identity(&before) {
            return Err(Error::Storage(
                "journal identity changed during open".into(),
            ));
        }
        self.journal = Some(file);
        self.journal_id = Some(id);
        Ok(true)
    }
}
impl Storage for UnixStorage {
    fn lock_exclusive(&mut self) -> Result<()> {
        if self.main.is_some() {
            return Err(Error::Busy("storage already locked"));
        }
        self.check_directory()?;
        let mut registry = REGISTRY
            .lock()
            .map_err(|_| Error::Storage("inode registry poisoned".into()))?;
        if registry.values().any(|files| !files.is_empty()) {
            return Err(Error::Busy("file identity race awaiting close"));
        }
        let before = self.stat(&self.name)?;
        if let Some(stat) = &before {
            plain(stat)?;
            if registry.contains_key(&stat_identity(stat)) {
                return Err(Error::Busy("database open in this process"));
            }
        }
        let mut flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
        if before.is_none() {
            flags |= OFlags::CREATE | OFlags::EXCL;
        }
        let file: File = fs::openat(&self.directory, &self.name, flags, Mode::RUSR | Mode::WUSR)
            .map_err(|e| {
                if e == Errno::EXIST {
                    Error::Busy("database created concurrently")
                } else {
                    io_error("open database", e)
                }
            })?
            .into();
        let metadata = file
            .metadata()
            .map_err(|e| io_error("database metadata", e))?;
        let id = identity(&metadata);
        if let Some(deferred) = registry.get_mut(&id) {
            deferred.push(file);
            return Err(Error::Busy("database changed during open"));
        }
        if !metadata.is_file()
            || metadata.nlink() != 1
            || before.as_ref().is_some_and(|s| stat_identity(s) != id)
        {
            return Err(Error::Storage(
                "database identity changed during open".into(),
            ));
        }
        fs::fcntl_lock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|e| {
            if e == Errno::AGAIN || e == Errno::ACCESS {
                Error::Busy("database locked by another process")
            } else {
                io_error("fcntl database lock", e)
            }
        })?;
        registry.insert(id, Vec::new());
        self.main = Some(file);
        self.main_id = Some(id);
        drop(registry);
        let result = (|| {
            self.main()?;
            if before.is_none() {
                self.sync_database()?;
                self.sync_directory()?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.unlock();
        }
        result
    }
    fn unlock(&mut self) {
        // Closing descriptors is serialized with future opens. POSIX releases all
        // of this process's locks on the inode on close, even after explicit unlock.
        let mut registry = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        self.journal.take();
        self.journal_id = None;
        if let Some(file) = self.main.take() {
            let _ = fs::fcntl_lock(&file, FlockOperation::NonBlockingUnlock);
            drop(file);
            if let Some(id) = self.main_id.take() {
                registry.remove(&id);
            }
        }
    }
    fn read_database(&mut self, limit: usize) -> Result<Vec<u8>> {
        Self::read(self.main()?, limit)
    }
    fn read_journal(&mut self, limit: usize) -> Result<Option<Vec<u8>>> {
        if !self.open_existing_journal()? {
            return Ok(None);
        }
        Self::read(self.journal()?, limit).map(Some)
    }
    fn has_wal(&mut self) -> Result<bool> {
        self.main()?;
        match self.stat(&self.wal_name)? {
            Some(s) => {
                plain(&s)?;
                Ok(s.st_size > 0)
            }
            None => Ok(false),
        }
    }
    fn sector_size(&self) -> usize {
        65536
    }
    fn random_salt(&mut self) -> Result<u32> {
        self.main()?;
        let mut bytes = [0; 4];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|e| io_error("journal entropy", e))?;
        Ok(u32::from_ne_bytes(bytes))
    }
    fn create_journal(&mut self, bytes: &[u8]) -> Result<()> {
        self.main()?;
        if self.journal.is_some() {
            return Err(Error::Storage("journal already open".into()));
        }
        let file: File = fs::openat(
            &self.directory,
            &self.journal_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|e| io_error("create journal", e))?
        .into();
        self.journal_id = Some(identity(
            &file
                .metadata()
                .map_err(|e| io_error("journal metadata", e))?,
        ));
        self.journal = Some(file);
        self.journal()?
            .write_all_at(bytes, 0)
            .map_err(|e| io_error("write journal", e))
    }
    fn write_journal_header(&mut self, header: &[u8; 12]) -> Result<()> {
        self.journal()?
            .write_all_at(header, 0)
            .map_err(|e| io_error("activate journal", e))
    }
    fn sync_journal(&mut self) -> Result<()> {
        sync_file(self.journal()?)
    }
    fn write_database(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        self.main()?
            .write_all_at(bytes, offset as u64)
            .map_err(|e| io_error("write database", e))
    }
    fn truncate_database(&mut self, length: usize) -> Result<()> {
        self.main()?
            .set_len(length as u64)
            .map_err(|e| io_error("truncate database", e))
    }
    fn sync_database(&mut self) -> Result<()> {
        sync_file(self.main()?)
    }
    fn remove_journal(&mut self) -> Result<()> {
        self.journal()?;
        fs::unlinkat(&self.directory, &self.journal_name, AtFlags::empty())
            .map_err(|e| io_error("remove journal", e))?;
        self.journal.take();
        self.journal_id = None;
        Ok(())
    }
    fn sync_directory(&mut self) -> Result<()> {
        self.main()?;
        sync_file(&self.directory)
    }
}
impl Drop for UnixStorage {
    fn drop(&mut self) {
        self.unlock();
    }
}
