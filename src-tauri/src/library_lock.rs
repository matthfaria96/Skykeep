//! The library as every command shares it: one connection that writes, behind one lock, and
//! read-only connections beside it for the screens.
//!
//! A sync pass stores what it fetched with the library held: a full Notes or Photos pass, or a
//! first sync, holds it for seconds. Every screen used to read through that same lock, so
//! Notes, Reminders and Contacts drew nothing — and an autosave queued — until the pass was in.
//! WAL lets a second connection read what was committed while the writer works, so a read
//! goes to one of those instead whenever only a pass stands in its way.
//!
//! Only a pass. A change made in the window (a note saved, a reminder ticked) still comes
//! first: while anything other than a pass holds the lock or waits for it, a read waits too,
//! in turn, exactly as it always did — so a read asked for after a save never shows the
//! library from before it. A pass is one transaction (`Library::atomically`), so a reader sees
//! all of it or none of it, never half.

use quince_core::Library;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, MutexGuard};

/// The most read-only connections kept open between reads. More are opened when more reads
/// run at once; past this they are closed after use rather than kept.
const KEPT_READERS: usize = 4;

/// The shared library: the connection that writes, and the readers beside it.
pub struct LibraryLock {
    library: Mutex<Library>,
    /// Holders of the lock, and those waiting for it, that are not a sync pass: while there is
    /// one, a read waits its turn on the lock rather than reading beside it.
    others: AtomicUsize,
    /// Where the library's file is; `None` (an in-memory library, in tests) reads through the
    /// lock alone.
    dir: Option<PathBuf>,
    readers: std::sync::Mutex<Vec<Reader>>,
}

/// A read-only connection, and which file it was opened on: an account's library set aside
/// for another's moves the file under it, and a reader on the old one is not reused.
pub struct Reader {
    id: (u64, u64),
    library: Library,
}

impl Reader {
    /// A read-only connection on the library under `dir`, if it can be had.
    pub fn open(dir: &Path) -> Option<Self> {
        let id = identity(dir)?;
        let library = Library::open_reader(dir).ok()?;
        Some(Self { id, library })
    }

    /// Still on the file that is the library now.
    pub fn current(&self, dir: &Path) -> bool {
        identity(dir) == Some(self.id)
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    /// A number that moves each time another connection commits a change to the library, and
    /// only then (SQLite's `data_version`): what this reader last read is still what is there
    /// for as long as it stands.
    pub fn data_version(&self) -> Option<i64> {
        self.library
            .conn()
            .query_row("PRAGMA data_version", [], |r| r.get(0))
            .ok()
    }
}

fn identity(dir: &Path) -> Option<(u64, u64)> {
    let file = dir.join("library.db");
    std::fs::metadata(&file)
        .ok()
        .map(|m| crate::file_identity(&m))
}

/// The library held, as `tokio::sync::MutexGuard` holds it; counted, while it is held or
/// waited for, as something a read has to wait behind.
pub struct Held<'a> {
    guard: MutexGuard<'a, Library>,
    _turn: Turn<'a>,
}

impl Deref for Held<'_> {
    type Target = Library;
    fn deref(&self) -> &Library {
        &self.guard
    }
}

impl DerefMut for Held<'_> {
    fn deref_mut(&mut self) -> &mut Library {
        &mut self.guard
    }
}

/// One holder or waiter counted for as long as it lives, however it ends — dropped while still
/// waiting (its task cancelled) included.
struct Turn<'a>(&'a AtomicUsize);

impl<'a> Turn<'a> {
    fn take(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count)
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl LibraryLock {
    pub fn new(library: Library, dir: Option<PathBuf>) -> Self {
        Self {
            library: Mutex::new(library),
            others: AtomicUsize::new(0),
            dir,
            readers: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The library, to change it or to read it in turn with changes.
    pub async fn lock(&self) -> Held<'_> {
        let turn = Turn::take(&self.others);
        let guard = self.library.lock().await;
        Held { guard, _turn: turn }
    }

    /// [`Self::lock`], from a thread that is not one of the runtime's (a folder's own threads).
    pub fn blocking_lock(&self) -> Held<'_> {
        let turn = Turn::take(&self.others);
        let guard = self.library.blocking_lock();
        Held { guard, _turn: turn }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn try_lock(&self) -> Result<Held<'_>, tokio::sync::TryLockError> {
        let turn = Turn::take(&self.others);
        let guard = self.library.try_lock()?;
        Ok(Held { guard, _turn: turn })
    }

    /// The library, for a sync pass to store what it fetched as one transaction: reads carry on
    /// beside it on their own connections, seeing the library as it was until the pass is in.
    /// Only for a store made through `Library::atomically`, so no reader sees half of one.
    pub async fn lock_for_pass(&self) -> MutexGuard<'_, Library> {
        self.library.lock().await
    }

    /// Read the library: beside a pass on a read-only connection, or — while a change of the
    /// window's is held or waiting, or no reader can be had — through the lock, in turn.
    ///
    /// A read that fails on a reader is asked again through the lock, so a reader never makes
    /// a read fail that would not have failed before.
    pub async fn read<T>(
        &self,
        read: impl Fn(&Library) -> quince_core::Result<T>,
    ) -> quince_core::Result<T> {
        if let Some(Ok(found)) = self.beside(&read) {
            return Ok(found);
        }
        read(&*self.lock().await)
    }

    /// [`Self::read`], from a thread that is not one of the runtime's.
    pub fn blocking_read<T>(
        &self,
        read: impl Fn(&Library) -> quince_core::Result<T>,
    ) -> quince_core::Result<T> {
        if let Some(Ok(found)) = self.beside(&read) {
            return Ok(found);
        }
        read(&self.blocking_lock())
    }

    /// The read on a reader, when nothing but a pass stands in its way; `None` when it has to
    /// wait its turn.
    fn beside<T>(
        &self,
        read: &impl Fn(&Library) -> quince_core::Result<T>,
    ) -> Option<quince_core::Result<T>> {
        if self.others.load(Ordering::Acquire) != 0 {
            return None;
        }
        let dir = self.dir.as_deref()?;
        let reader = self.take_reader(dir)?;
        let found = read(reader.library());
        self.give_back(reader);
        Some(found)
    }

    fn take_reader(&self, dir: &Path) -> Option<Reader> {
        let kept = {
            let mut readers = self
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            readers.pop()
        };
        match kept {
            Some(r) if r.current(dir) => Some(r),
            // On a file that is not the library any more: let go, and a fresh one opened.
            _ => Reader::open(dir),
        }
    }

    fn give_back(&self, reader: Reader) {
        // Where an open file cannot be moved (Windows), none is kept open between reads: an
        // account's library set aside would otherwise fail to move.
        if cfg!(not(unix)) {
            return;
        }
        let mut readers = self
            .readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if readers.len() < KEPT_READERS {
            readers.push(reader);
        }
    }

    /// Let go of every kept reader: before the library's files are moved.
    pub fn forget_readers(&self) {
        self.readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn library(dir: &Path) -> LibraryLock {
        LibraryLock::new(Library::open(dir).unwrap(), Some(dir.to_path_buf()))
    }

    fn state(l: &Library) -> quince_core::Result<Option<String>> {
        l.state("k")
    }

    /// A read while a pass holds the library answers at once, with the library as it was
    /// before the pass — never waiting for it, and never half of it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_read_never_waits_on_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Arc::new(library(dir.path()));
        shared.lock().await.set_state("k", "before").unwrap();

        let mut pass = shared.lock_for_pass().await;
        pass.conn().execute_batch("SAVEPOINT p").unwrap();
        pass.set_state("k", "during").unwrap();

        let read = tokio::time::timeout(Duration::from_secs(2), shared.read(state))
            .await
            .expect("the read waited for the pass")
            .unwrap();
        assert_eq!(read.as_deref(), Some("before"));

        pass.conn().execute_batch("RELEASE p").unwrap();
        drop(pass);
        assert_eq!(shared.read(state).await.unwrap().as_deref(), Some("during"));
    }

    /// A change of the window's waiting behind a pass keeps its turn: a read asked for after
    /// it waits too, and sees it, as it did when everything read through the lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_read_after_a_change_sees_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Arc::new(library(dir.path()));
        shared.lock().await.set_state("k", "before").unwrap();

        let pass = shared.lock_for_pass().await;
        let writer = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                shared.lock().await.set_state("k", "saved").unwrap();
            })
        };
        // The change is queued behind the pass.
        while shared.others.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
        let reader = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move { shared.read(state).await.unwrap() })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!reader.is_finished(), "the read went ahead of the change");
        drop(pass);
        writer.await.unwrap();
        assert_eq!(reader.await.unwrap().as_deref(), Some("saved"));
    }

    /// A reader on a file that is no longer the library (an account's set aside, another's put
    /// in its place) is not used: the read is of the library that is there now.
    #[tokio::test]
    async fn a_reader_follows_the_library_to_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let shared = library(dir.path());
        shared.lock().await.set_state("k", "first").unwrap();
        assert_eq!(shared.read(state).await.unwrap().as_deref(), Some("first"));

        let elsewhere = tempfile::tempdir().unwrap();
        {
            let mut held = shared.lock().await;
            *held = Library::in_memory().unwrap();
            for name in ["library.db", "library.db-wal", "library.db-shm"] {
                let _ = std::fs::rename(dir.path().join(name), elsewhere.path().join(name));
            }
            *held = Library::open(dir.path()).unwrap();
            held.set_state("k", "second").unwrap();
        }
        assert_eq!(shared.read(state).await.unwrap().as_deref(), Some("second"));
    }

    /// The reader's version moves when the writer commits, and stands while it does not: the
    /// Drive folder keeps a listing for exactly as long as it stands.
    #[test]
    fn a_readers_version_moves_with_each_commit_and_only_then() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = Library::open(dir.path()).unwrap();
        let reader = Reader::open(dir.path()).unwrap();
        let first = reader.data_version().unwrap();
        assert_eq!(reader.data_version(), Some(first), "nothing committed");
        writer.set_state("k", "v").unwrap();
        let second = reader.data_version().unwrap();
        assert_ne!(second, first, "a commit moves it");
        assert_eq!(reader.data_version(), Some(second));
        // A pass's whole transaction moves it once it is in, not before.
        writer
            .atomically(|l| {
                l.set_state("k", "w")?;
                assert_eq!(reader.data_version(), Some(second), "not while it is open");
                Ok::<_, quince_core::Error>(())
            })
            .unwrap();
        assert_ne!(reader.data_version(), Some(second));
    }

    use std::time::Duration;
}

/// The one Skykeep that has this library, as long as it runs: an exclusive lock on
/// `skykeep.lock` beside it (`flock` on Linux, a file opened shared with nobody on Windows), let
/// go of by the system however the process ends.
///
/// The single-instance plugin finds a running Skykeep on the session bus, and with no session
/// bus (over SSH, after `su`, some kiosks) it found none and a second one ran: both sending
/// what waits for Apple — the same change twice — both mounting the folders, and each one's
/// writes failing on the other's lock. `None`: another Skykeep holds it.
pub fn hold(dir: &Path) -> std::io::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    // Windows: opened shared with nobody, which is the lock — a second open is refused with a
    // sharing violation until this handle closes.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = match options.open(dir.join("skykeep.lock")) {
        Ok(file) => file,
        #[cfg(windows)]
        Err(e) if e.raw_os_error() == Some(32) => return Ok(None),
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: flock on a descriptor this function owns; nothing else is touched.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(None)
            } else {
                Err(e)
            };
        }
    }
    Ok(Some(file))
}

/// What keeps a second Skykeep off this library for as long as this one runs.
#[derive(Debug)]
pub enum Hold {
    /// The lock on `skykeep.lock` ([`hold`]): held by being kept open, never read.
    Locked(#[allow(dead_code)] std::fs::File),
    /// Where no lock can be taken: `skykeep.pid`, made only if no other file of that name is
    /// there ([`mark`]).
    Marked(PathBuf),
}

/// [`hold`], and where the folder's disk refuses locks (a network share without them),
/// [`mark`] instead: never a Skykeep running beside another on one library because neither
/// could tell. `Ok(None)`: another Skykeep has it. `Err`: neither could be had, said in words
/// to show before quitting.
pub fn hold_or_mark(dir: &Path) -> Result<Option<Hold>, String> {
    match hold(dir) {
        Ok(Some(lock)) => Ok(Some(Hold::Locked(lock))),
        Ok(None) => Ok(None),
        Err(why) => mark(dir).map_err(|e| {
            format!(
                "Skykeep could not make sure it is the only copy using {}: the disk refused a \
                 lock ({why}), and the file that stands in for one could not be made ({e}).",
                dir.display()
            )
        }),
    }
}

/// `skykeep.pid` beside the library, created only if no file of that name exists, holding this
/// process's id: the lock where the disk takes none. One left by a Skykeep that ended without
/// letting go — its process gone, or another program's now — is taken over; one a running
/// Skykeep made answers `None`.
pub fn mark(dir: &Path) -> std::io::Result<Option<Hold>> {
    mark_as(dir, std::process::id(), is_skykeep)
}

/// How many times [`mark_as`] looks again before it gives up: each look waits a little longer,
/// a few seconds in all — long past the moment another Skykeep takes to write its mark.
const MARK_TRIES: u32 = 40;

/// A mark with nothing readable in it is one being made at this instant (created, its id not
/// written yet), never one left behind, until it is this old.
const MARK_MAKING: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a mark just made stands before it is counted as held (see [`mark_as`]).
const MARK_SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// [`mark`] as process `me`, a process taken as a running Skykeep where `running` says so.
///
/// Two Skykeeps starting at the same instant must not both run. So a mark only ever appears
/// whole: written to a file of this process's own and linked into place, which the system
/// does at once or refuses because one is there (where the disk has no links, created only
/// if absent and written — and one found empty is waited for, not cleared). A mark left
/// behind is cleared by renaming it aside under a name of this process's own, which only one
/// of two can do, and only if it is still the one judged left behind; then the mark is made
/// again, as any other. Whoever made the mark that stands is the one that runs.
fn mark_as(dir: &Path, me: u32, running: impl Fn(u32) -> bool) -> std::io::Result<Option<Hold>> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("skykeep.pid");
    for attempt in 0..MARK_TRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(
                5 * u64::from(attempt.min(20)),
            ));
        }
        if place_mark(dir, &path, me)? {
            // Ours once it has stood a moment: another clearing a mark left behind at this
            // same instant may have renamed ours aside, and puts it back or finds another in
            // its place — so the one still there after the moment is the one that runs.
            std::thread::sleep(MARK_SETTLE);
            if marked_by(&path) == Some(me) {
                return Ok(Some(Hold::Marked(path)));
            }
            continue;
        }
        // One is there: whose, as it stands now.
        let Ok(found) = std::fs::read(&path) else {
            continue; // gone since: look again
        };
        let by = std::str::from_utf8(&found)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok());
        match by {
            Some(pid) if pid == me => return Ok(Some(Hold::Marked(path))),
            Some(pid) if running(pid) => return Ok(None),
            // Where whose it is cannot be asked, it is the person's to say.
            Some(_) if !cfg!(target_os = "linux") => {
                return Err(std::io::Error::other(format!(
                    "{} is there. If no other Skykeep is running, remove it and open \
                     Skykeep again",
                    path.display()
                )))
            }
            Some(_) => clear_left_behind(dir, &path, me, &found)?,
            // Being made: wait for it, unless it has stood empty far longer than that takes.
            None => {
                let age = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok());
                if age.is_some_and(|a| a > MARK_MAKING) {
                    clear_left_behind(dir, &path, me, &found)?;
                }
            }
        }
    }
    Err(std::io::Error::other(
        "skykeep.pid kept changing while Skykeep looked at it",
    ))
}

/// Put this process's mark in place if none is there: `true` if it is now ours.
fn place_mark(dir: &Path, path: &Path, me: u32) -> std::io::Result<bool> {
    use std::io::Write;
    let own = dir.join(format!("skykeep.pid.{me}"));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&own)?;
        file.write_all(me.to_string().as_bytes())?;
        file.sync_all()
    })();
    let linked = written.and_then(|()| std::fs::hard_link(&own, path));
    let _ = std::fs::remove_file(&own);
    match linked {
        Ok(()) => return Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        // No links on this disk: made only if absent, and written.
        Err(_) => {}
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(me.to_string().as_bytes())?;
            file.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// Clear a mark judged left behind — holding `judged` — by renaming it aside: of two doing
/// this at once, one renames it and the other finds it gone. Renamed aside but holding
/// something else (made new by another Skykeep in between), it is put back.
fn clear_left_behind(dir: &Path, path: &Path, me: u32, judged: &[u8]) -> std::io::Result<()> {
    let aside = dir.join(format!("skykeep.pid.{me}.left"));
    match std::fs::rename(path, &aside) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    if std::fs::read(&aside).ok().as_deref() != Some(judged) {
        // Someone's new mark: back where it was, unless yet another stands there now.
        let _ = std::fs::hard_link(&aside, path).or_else(|_| {
            if path.exists() {
                Ok(())
            } else {
                std::fs::rename(&aside, path)
            }
        });
    }
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

fn marked_by(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Whether process `pid` is running and is a Skykeep: the same program as this one. Asked of
/// Linux's `/proc`; elsewhere it cannot be told, and `false` leaves it to [`mark`] to ask.
/// Its program file, or (one updated while it ran, its file replaced) its name.
fn is_skykeep(pid: u32) -> bool {
    let exe = |p: &str| std::fs::read_link(format!("/proc/{p}/exe")).ok();
    let name = |p: &str| std::fs::read_to_string(format!("/proc/{p}/comm")).ok();
    let theirs = pid.to_string();
    let same_file = matches!((exe(&theirs), exe("self")), (Some(a), Some(b)) if a == b);
    let same_name = matches!((name(&theirs), name("self")), (Some(a), Some(b)) if a == b);
    same_file || same_name
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Hold::Marked(path) = self {
            if marked_by(path) == Some(std::process::id()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    /// Where the disk takes no lock: the mark stands in. Made by this Skykeep, a second is
    /// told one is running; left by one that ended (its process gone), it is taken over; let
    /// go of, it is gone.
    #[cfg(target_os = "linux")]
    #[test]
    fn where_no_lock_can_be_taken_a_mark_stands_in() {
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("skykeep.pid");
        // A running Skykeep's mark (this test's own process stands in for one).
        let parent = std::os::unix::process::parent_id();
        std::fs::write(&pid, std::process::id().to_string()).unwrap();
        // Ours: taken again as ours.
        let held = mark(dir.path()).unwrap();
        assert!(matches!(held, Some(Hold::Marked(_))));
        drop(held);
        assert!(!pid.exists(), "let go of with the process");

        // Another running process of this same program: refused. A process that is not
        // this program (the test's parent, a shell or cargo), or none at all: taken over.
        // SAFETY: the child only sleeps and exits, both safe after fork in a threaded process.
        let other = unsafe { libc::fork() };
        if other == 0 {
            unsafe {
                libc::sleep(30);
                libc::_exit(0);
            }
        }
        assert!(other > 0);
        std::fs::write(&pid, other.to_string()).unwrap();
        let refused = mark(dir.path()).unwrap();
        // SAFETY: signals and reaps the child made above, nothing else.
        unsafe {
            libc::kill(other, libc::SIGKILL);
            libc::waitpid(other, std::ptr::null_mut(), 0);
        }
        assert!(refused.is_none(), "another Skykeep has it");
        std::fs::write(&pid, parent.to_string()).unwrap();
        assert!(
            mark(dir.path()).unwrap().is_some(),
            "another program's id: left behind"
        );
        std::fs::write(&pid, "4194999").unwrap();
        assert!(mark(dir.path()).unwrap().is_some(), "a process long gone");
    }

    /// Many Skykeeps starting at the same instant on a disk without locks — over no mark, and
    /// over one left behind that each of them would clear — and exactly one runs, the one
    /// whose id the mark holds. Before, two clearing the same mark, or one finding another's
    /// mark not yet written, could both run.
    #[test]
    fn marks_made_at_the_same_instant_let_only_one_run() {
        const STARTING: u32 = 12;
        for round in 0..30 {
            // A mark left behind is taken over only where whose it is can be asked (Linux's
            // /proc); elsewhere `mark` refuses and asks the person (see `mark_as`), so those
            // rounds hold on Linux only. Starting together over no mark holds everywhere.
            let left_behind = round % 2 == 1;
            if left_behind && !cfg!(target_os = "linux") {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            if left_behind {
                std::fs::write(dir.path().join("skykeep.pid"), "4194999").unwrap();
            }
            let all = std::sync::Arc::new(std::sync::Barrier::new(STARTING as usize));
            let held: Vec<(u32, bool)> = std::thread::scope(|s| {
                let starts: Vec<_> = (0..STARTING)
                    .map(|i| {
                        let (all, dir) = (all.clone(), dir.path());
                        s.spawn(move || {
                            let me = 1000 + i;
                            all.wait();
                            let hold =
                                mark_as(dir, me, |pid| (1000..1000 + STARTING).contains(&pid))
                                    .unwrap();
                            let ours = hold.is_some();
                            // Held until every start has looked.
                            std::mem::forget(hold);
                            (me, ours)
                        })
                    })
                    .collect();
                starts.into_iter().map(|t| t.join().unwrap()).collect()
            });
            let running: Vec<u32> = held.iter().filter(|(_, h)| *h).map(|(me, _)| *me).collect();
            assert_eq!(running.len(), 1, "round {round}: {running:?} all run");
            assert_eq!(
                marked_by(&dir.path().join("skykeep.pid")),
                Some(running[0]),
                "round {round}: the one that runs is the one marked"
            );
            let left: Vec<_> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(left.len(), 1, "round {round}: nothing else left: {left:?}");
        }
    }

    /// A mark with nothing in it yet is another Skykeep's, made a moment ago and about to say
    /// whose: it is waited for, never cleared as one left behind.
    #[test]
    fn a_mark_being_made_is_waited_for_not_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skykeep.pid");
        std::fs::write(&path, "").unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                std::fs::write(&path, "77").unwrap();
            })
        };
        let hold = mark_as(dir.path(), 5, |pid| pid == 77).unwrap();
        writer.join().unwrap();
        assert!(hold.is_none(), "the other Skykeep runs");
        assert_eq!(marked_by(&path), Some(77), "its mark stands");
    }

    #[test]
    fn only_one_holds_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let first = hold(dir.path()).unwrap().expect("the first holds it");
        assert!(
            hold(dir.path()).unwrap().is_none(),
            "a second is turned away"
        );
        drop(first);
        assert!(
            hold(dir.path()).unwrap().is_some(),
            "free again once the first is gone"
        );
    }
}
