#![forbid(unsafe_code)]
use sqlite_safe::{
    journal::JournalLimits,
    pager::{Pager, Storage},
    sql::Connection,
    Error, Result,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone)]
struct Sim(Rc<RefCell<Disk>>);
struct Disk {
    main: Vec<u8>,
    stable_main: Vec<u8>,
    journal: Option<Vec<u8>>,
    stable_journal: Option<Vec<u8>>,
    durable_name: bool,
    locked: bool,
    step: usize,
    fail: Option<(usize, bool)>,
    eager: bool,
    partial: bool,
}
impl Sim {
    fn new(main: Vec<u8>) -> Self {
        Self(Rc::new(RefCell::new(Disk {
            main: main.clone(),
            stable_main: main,
            journal: None,
            stable_journal: None,
            durable_name: false,
            locked: false,
            step: 0,
            fail: None,
            eager: false,
            partial: false,
        })))
    }
    fn event(&self, action: impl FnOnce(&mut Disk)) -> Result<()> {
        let mut d = self.0.borrow_mut();
        d.step += 1;
        if d.fail == Some((d.step, false)) {
            return Err(Error::Storage("injected before operation".into()));
        }
        action(&mut d);
        if d.fail == Some((d.step, true)) {
            return Err(Error::Storage("injected after operation".into()));
        }
        Ok(())
    }
    fn crash(&self) {
        let mut d = self.0.borrow_mut();
        d.main = d.stable_main.clone();
        d.journal = if d.durable_name {
            d.stable_journal.clone()
        } else {
            None
        };
        d.locked = false;
        d.fail = None;
        d.step = 0;
        d.partial = false;
    }
    fn inject(&self, step: usize, after: bool, eager: bool, partial: bool) {
        let mut d = self.0.borrow_mut();
        d.step = 0;
        d.fail = Some((step, after));
        d.eager = eager;
        d.partial = partial;
    }
}
fn write(at: usize, bytes: &[u8], target: &mut Vec<u8>) {
    target.resize(target.len().max(at + bytes.len()), 0);
    target[at..at + bytes.len()].copy_from_slice(bytes);
}
impl Storage for Sim {
    fn lock_exclusive(&mut self) -> Result<()> {
        let mut d = self.0.borrow_mut();
        if d.locked {
            return Err(Error::Storage("busy".into()));
        }
        d.locked = true;
        Ok(())
    }
    fn unlock(&mut self) {
        self.0.borrow_mut().locked = false;
    }
    fn read_database(&mut self, _: usize) -> Result<Vec<u8>> {
        Ok(self.0.borrow().main.clone())
    }
    fn read_journal(&mut self, _: usize) -> Result<Option<Vec<u8>>> {
        Ok(self.0.borrow().journal.clone())
    }
    fn has_wal(&mut self) -> Result<bool> {
        Ok(false)
    }
    fn sector_size(&self) -> usize {
        512
    }
    fn random_salt(&mut self) -> Result<u32> {
        Ok(99)
    }
    fn create_journal(&mut self, bytes: &[u8]) -> Result<()> {
        if self.0.borrow().journal.is_some() {
            return Err(Error::Storage("journal exists".into()));
        }
        self.event(|d| {
            let count = if d.partial && d.fail == Some((d.step, true)) {
                bytes.len() / 2
            } else {
                bytes.len()
            };
            d.journal = Some(bytes[..count].to_vec());
            if d.eager {
                d.stable_journal = d.journal.clone();
                d.durable_name = true;
            }
        })
    }
    fn write_journal_header(&mut self, header: &[u8; 12]) -> Result<()> {
        self.event(|d| {
            let count = if d.partial && d.fail == Some((d.step, true)) {
                4
            } else {
                12
            };
            d.journal.as_mut().unwrap()[..count].copy_from_slice(&header[..count]);
            if d.eager {
                d.stable_journal = d.journal.clone();
            }
        })
    }
    fn sync_journal(&mut self) -> Result<()> {
        self.event(|d| d.stable_journal = d.journal.clone())
    }
    fn write_database(&mut self, offset: usize, bytes: &[u8]) -> Result<()> {
        self.event(|d| {
            let count = if d.partial && d.fail == Some((d.step, true)) {
                bytes.len() / 2
            } else {
                bytes.len()
            };
            write(offset, &bytes[..count], &mut d.main);
            if d.eager {
                write(offset, &bytes[..count], &mut d.stable_main);
            }
        })
    }
    fn truncate_database(&mut self, length: usize) -> Result<()> {
        self.event(|d| {
            d.main.resize(length, 0);
            if d.eager {
                d.stable_main.resize(length, 0);
            }
        })
    }
    fn sync_database(&mut self) -> Result<()> {
        self.event(|d| d.stable_main = d.main.clone())
    }
    fn remove_journal(&mut self) -> Result<()> {
        self.event(|d| {
            d.journal = None;
            if d.eager {
                d.durable_name = false;
            }
        })
    }
    fn sync_directory(&mut self) -> Result<()> {
        self.event(|d| d.durable_name = d.journal.is_some())
    }
}
fn image(count: i64) -> Vec<u8> {
    let mut c = Connection::new();
    c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE);")
        .unwrap();
    for id in 0..count {
        c.execute(
            &format!("INSERT INTO t VALUES({id},'row-{id}-{}')", "x".repeat(180)),
            &[],
        )
        .unwrap();
    }
    c.to_image(512).unwrap()
}
fn committed(old: &[u8], new: &[u8]) -> (Vec<u8>, usize) {
    let sim = Sim::new(old.to_vec());
    let mut pager = Pager::open(sim.clone(), JournalLimits::default()).unwrap();
    pager.commit(new).unwrap();
    let image = pager.image().unwrap().to_vec();
    let steps = sim.0.borrow().step;
    drop(pager);
    sim.crash();
    assert_eq!(
        Pager::open(sim, JournalLimits::default())
            .unwrap()
            .image()
            .unwrap(),
        image
    );
    (image, steps)
}

#[test]
fn every_commit_boundary_is_atomic_after_crash_or_fails_closed() {
    let mut cases = 0;
    for (old, new) in [
        (image(3), image(14)),
        (image(14), image(2)),
        (Vec::new(), image(6)),
    ] {
        let (expected, steps) = committed(&old, &new);
        for step in 1..=steps {
            for after in [false, true] {
                for eager in [false, true] {
                    for partial in [false, true] {
                        let sim = Sim::new(old.clone());
                        let mut pager = Pager::open(sim.clone(), JournalLimits::default()).unwrap();
                        sim.inject(step, after, eager, partial);
                        assert!(pager.commit(&new).is_err());
                        assert!(pager.is_poisoned());
                        assert!(pager.image().is_err());
                        assert!(pager.commit(&new).is_err());
                        drop(pager);
                        sim.crash();
                        match Pager::open(sim.clone(), JournalLimits::default()) {
                            Ok(p) => assert!(
                                p.image().unwrap() == old || p.image().unwrap() == expected,
                                "step {step} after {after} eager {eager} partial {partial}"
                            ),
                            Err(_) => {
                                // A torn activation header is deliberately retained for
                                // diagnosis. Main-file writes cannot yet have begun.
                                assert_eq!(sim.0.borrow().main, old);
                                assert!(sim.0.borrow().journal.is_some());
                            }
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert!(cases > 300);
    println!("checked {cases} commit failure/crash variants");
}

#[test]
fn interrupted_recovery_can_be_repeated_and_locks_are_released() {
    let old = image(14);
    let new = image(2);
    let sim = Sim::new(old.clone());
    let mut pager = Pager::open(sim.clone(), JournalLimits::default()).unwrap();
    assert!(Pager::open(sim.clone(), JournalLimits::default()).is_err());
    assert!(sim.0.borrow().locked);
    // Fail before database sync: all writes/truncation may already be persistent,
    // but the complete original image is protected by the durable journal.
    let sync_step = 5 + new.len() / 512 + 2;
    sim.inject(sync_step, false, true, false);
    assert!(pager.commit(&new).is_err());
    drop(pager);
    sim.crash();
    let dirty = sim.0.borrow().main.clone();
    let journal = sim.0.borrow().journal.clone().unwrap();
    for step in 1..=old.len() / 512 + 6 {
        for after in [false, true] {
            let sim = Sim::new(dirty.clone());
            {
                let mut d = sim.0.borrow_mut();
                d.journal = Some(journal.clone());
                d.stable_journal = Some(journal.clone());
                d.durable_name = true;
            }
            sim.inject(step, after, true, true);
            let opened = Pager::open(sim.clone(), JournalLimits::default());
            drop(opened);
            assert!(!sim.0.borrow().locked);
            sim.crash();
            let recovered = Pager::open(sim.clone(), JournalLimits::default()).unwrap();
            assert_eq!(recovered.image().unwrap(), old);
        }
    }
}

#[test]
fn failed_validation_does_not_touch_storage_and_success_advances_counters() {
    let old = image(3);
    let sim = Sim::new(old.clone());
    let mut p = Pager::open(sim.clone(), JournalLimits::default()).unwrap();
    assert!(p.commit(&[0; 512]).is_err());
    assert!(!p.is_poisoned());
    assert_eq!(sim.0.borrow().step, 0);
    p.commit(&image(4)).unwrap();
    p.commit(&image(5)).unwrap();
    assert_eq!(&p.image().unwrap()[24..28], &3u32.to_be_bytes());
    assert_eq!(&p.image().unwrap()[40..44], &3u32.to_be_bytes());
    assert!(sim.0.borrow().journal.is_none());
}

#[test]
fn sql_autocommit_transactions_savepoints_and_reopen() {
    use sqlite_safe::{sql::JournaledConnection, Value};
    let sim = Sim::new(Vec::new());
    let mut c = JournaledConnection::open(sim.clone(), 512, JournalLimits::default()).unwrap();
    for sql in [
        "CREATE TABLE t(id INTEGER PRIMARY KEY,v TEXT UNIQUE)",
        "INSERT INTO t VALUES(1,'one')",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    let stable = sim.0.borrow().stable_main.clone();
    for sql in [
        "BEGIN",
        "INSERT INTO t VALUES(2,'two')",
        "SAVEPOINT s",
        "DELETE FROM t",
        "ROLLBACK TO s",
        "RELEASE s",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    assert_eq!(sim.0.borrow().stable_main, stable);
    c.execute("COMMIT", &[]).unwrap();
    assert_ne!(sim.0.borrow().stable_main, stable);
    for sql in ["BEGIN", "DELETE FROM t", "ROLLBACK"] {
        c.execute(sql, &[]).unwrap();
    }
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(2)]]
    );
    c.execute("BEGIN", &[]).unwrap();
    c.execute("DELETE FROM t", &[]).unwrap();
    drop(c);
    sim.crash();
    let mut c = JournaledConnection::open(sim.clone(), 4096, JournalLimits::default()).unwrap();
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(2)]]
    );
    assert!(c.execute("INSERT INTO t VALUES(3,'one')", &[]).is_err());
    for sql in [
        "SAVEPOINT outer",
        "INSERT INTO t VALUES(3,'three')",
        "RELEASE outer",
    ] {
        c.execute(sql, &[]).unwrap();
    }
    drop(c);
    sim.crash();
    let mut c = JournaledConnection::open(sim.clone(), 512, JournalLimits::default()).unwrap();
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(3)]]
    );
    let read_steps = sim.0.borrow().step;
    c.execute("SELECT 1", &[]).unwrap();
    assert_eq!(sim.0.borrow().step, read_steps);
    sim.inject(6, true, true, true); // Torn first main-page write after journal sync.
    assert!(c.execute("INSERT INTO t VALUES(4,'four')", &[]).is_err());
    assert!(c.execute("SELECT 1", &[]).is_err());
    drop(c);
    sim.crash();
    let mut c = JournaledConnection::open(sim, 512, JournalLimits::default()).unwrap();
    assert_eq!(
        c.execute("SELECT count(*) FROM t", &[]).unwrap().rows,
        vec![vec![Value::Integer(3)]]
    );
}

#[test]
fn fail_policy_prefix_commit_errors_poison_and_recover_atomically() {
    use sqlite_safe::{sql::JournaledConnection, Value};
    let mut db = Connection::new();
    db.execute_batch("CREATE TABLE t(x UNIQUE);INSERT INTO t VALUES(1);")
        .unwrap();
    let old = db.to_image(512).unwrap();
    let sql = "INSERT OR FAIL INTO t VALUES(2),(1),(3)";
    let sim = Sim::new(old.clone());
    let mut c = JournaledConnection::open(sim.clone(), 512, JournalLimits::default()).unwrap();
    assert!(matches!(c.execute(sql, &[]), Err(Error::Constraint(_))));
    assert_eq!(c.changes().unwrap(), 1);
    let steps = sim.0.borrow().step;
    assert!(steps > 0);
    drop(c);
    let expected = sim.0.borrow().stable_main.clone();
    for step in 1..=steps {
        for after in [false, true] {
            let sim = Sim::new(old.clone());
            let mut c =
                JournaledConnection::open(sim.clone(), 512, JournalLimits::default()).unwrap();
            sim.inject(step, after, true, true);
            assert!(matches!(c.execute(sql, &[]), Err(Error::Storage(_))));
            assert!(c.execute("SELECT 1", &[]).is_err());
            assert!(c.changes().is_err());
            drop(c);
            sim.crash();
            match JournaledConnection::open(sim.clone(), 512, JournalLimits::default()) {
                Ok(mut c) => {
                    let persisted = sim.0.borrow().main.clone();
                    assert!(persisted == old || persisted == expected);
                    let rows = c.execute("SELECT x FROM t ORDER BY x", &[]).unwrap().rows;
                    assert_eq!(
                        rows,
                        if persisted == old {
                            vec![vec![Value::Integer(1)]]
                        } else {
                            vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
                        }
                    );
                }
                Err(_) => assert_eq!(sim.0.borrow().main, old),
            }
        }
    }
}
