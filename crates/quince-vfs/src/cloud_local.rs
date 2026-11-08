//! Changes made in File Explorer's iCloud Drive, sent to Apple: new files and folders, and
//! files saved over. (Deletes and renames come as Windows' own notices, in `cloud.rs`.)
//!
//! What is waiting is the files themselves: Windows keeps each placeholder's "in sync" mark on
//! disk, so a file saved here is out of sync until Apple has it, and a file never sent is no
//! placeholder at all. A scan at start finds whatever changed while Skykeep was closed, and
//! nothing else has to be remembered — no second record that could disagree with the disk.
//!
//! Each file is sent once it has been left alone a moment (a save is several writes), from a
//! copy taken first, so a program still writing can never send half a file. It is marked in
//! sync only if it is still exactly what was sent; otherwise it goes again. A file changed at
//! Apple meanwhile is kept beside it as a conflicted copy, never written over (`writes::send`).

use crate::cloud::{folder_item, identity_of, placeholder_state};
use crate::writes::{send, stays_local, SETTLE};
use crate::Remote;
use quince_core::drive::DriveItem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use windows::core::HSTRING;
use windows::Win32::Storage::CloudFilters::*;

/// The files Explorer and Windows keep for themselves in every folder, never sent: left "syncing"
/// for ever if they stay unmarked (Nextcloud #10995), so they are marked in sync where they are.
fn windows_own(name: &str) -> bool {
    name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db")
}

/// Never sent, never marked: editors' scratch files and Office's "~$" locks.
fn scratch(name: &str) -> bool {
    stays_local(name) || name.starts_with("~$")
}

/// Watch `root` and send what changes there, until `stop` is set. Starts with a scan.
pub fn watch(
    root: PathBuf,
    remote: Arc<dyn Remote>,
    staging: PathBuf,
    rt: tokio::runtime::Handle,
    stop: Arc<std::sync::atomic::AtomicBool>,
    add_only: bool,
) -> Option<std::thread::JoinHandle<()>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<PathBuf>();
    // Whatever changed while Skykeep was closed.
    for path in scan(&root) {
        let _ = tx.send(path);
    }
    rt.spawn(settle_and_send(rx, Arc::clone(&remote), staging, add_only));
    let watched = root.clone();
    std::thread::Builder::new()
        .name("quince:cloud-watch".into())
        .spawn(move || listen(&watched, &tx, &stop))
        .ok()
}

/// Every file and folder under `root` that is not in sync, without opening a folder never
/// listed (that would fetch it) or reading any file.
fn scan(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let pending = match placeholder_state(&path) {
                None => true,
                Some((state, _)) => state != CF_IN_SYNC_STATE_IN_SYNC,
            };
            if pending {
                out.push(path.clone());
            }
            // A folder placeholder still to be listed (recall on open) is all Apple's: nothing
            // of ours can be in it.
            use std::os::windows::fs::MetadataExt;
            const RECALL_ON_OPEN: u32 = 0x0004_0000;
            if meta.is_dir() && meta.file_attributes() & RECALL_ON_OPEN == 0 {
                dirs.push(path);
            }
        }
    }
    out
}

/// Windows' change notices for everything under `root`, as paths, until `stop`.
fn listen(
    root: &Path,
    tx: &tokio::sync::mpsc::UnboundedSender<PathBuf>,
    stop: &std::sync::atomic::AtomicBool,
) {
    use windows::Win32::Storage::FileSystem::*;
    // SAFETY: a path string alive for the call; the handle is closed when this returns.
    let Ok(dir) = (unsafe {
        CreateFileW(
            &HSTRING::from(root.as_os_str()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }) else {
        eprintln!("quince: watching {} for changes failed", root.display());
        return;
    };
    let _close = crate::cloud::Handle(dir);
    let mut buf = vec![0u32; 16 * 1024];
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let mut got = 0u32;
        // SAFETY: the buffer is ours and as long as said; the call waits for changes.
        let read = unsafe {
            ReadDirectoryChangesW(
                dir,
                buf.as_mut_ptr().cast(),
                (buf.len() * 4) as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES,
                Some(&mut got),
                None,
                None,
            )
        };
        if read.is_err() {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        if got == 0 {
            // More changed than fit: everything is looked at again.
            for path in scan(root) {
                let _ = tx.send(path);
            }
            continue;
        }
        let bytes = buf.as_ptr().cast::<u8>();
        let mut at = 0usize;
        loop {
            // SAFETY: Windows wrote a chain of FILE_NOTIFY_INFORMATION within `got` bytes.
            let info = unsafe { &*bytes.add(at).cast::<FILE_NOTIFY_INFORMATION>() };
            let len = info.FileNameLength as usize / 2;
            // SAFETY: the name follows the header, `len` UTF-16 units long.
            let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), len) };
            let path = root.join(String::from_utf16_lossy(name));
            if info.Action != FILE_ACTION_REMOVED && info.Action != FILE_ACTION_RENAMED_OLD_NAME {
                let _ = tx.send(path);
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            at += info.NextEntryOffset as usize;
        }
    }
}

/// Each path sent once nothing has touched it for SETTLE, one at a time, tried again later if
/// Apple could not take it.
async fn settle_and_send(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PathBuf>,
    remote: Arc<dyn Remote>,
    staging: PathBuf,
    add_only: bool,
) {
    let mut touched: HashMap<PathBuf, Instant> = HashMap::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            got = rx.recv() => match got {
                Some(path) => { touched.insert(path, Instant::now()); }
                None => return,
            },
            _ = tick.tick() => {
                let due: Vec<PathBuf> = touched
                    .iter()
                    .filter(|(_, at)| at.elapsed() >= SETTLE)
                    .map(|(p, _)| p.clone())
                    .collect();
                for path in due {
                    touched.remove(&path);
                    if let Err(e) = sync_path(remote.as_ref(), &staging, &path, add_only).await {
                        eprintln!("quince: sending {}: {e}", path.display());
                        // Tried again in a while; the file itself stays as it is.
                        touched.insert(path, Instant::now() + std::time::Duration::from_secs(25));
                    }
                }
            }
        }
    }
}

/// One path made Apple's: a new folder made there, a new or saved file sent.
async fn sync_path(
    remote: &dyn Remote,
    staging: &Path,
    path: &Path,
    add_only: bool,
) -> Result<(), String> {
    let Some(name) = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
    else {
        return Ok(());
    };
    if scratch(&name) || !path.exists() {
        return Ok(());
    }
    let state = placeholder_state(path);
    if windows_own(&name) {
        if state.is_none() {
            let _ = convert(path, None);
        }
        return Ok(());
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let folder = match folder_item(parent) {
        Some(f) => f,
        // Its folder isn't at Apple yet: it is sent with that folder's contents once it is.
        None => return Ok(()),
    };
    if path.is_dir() {
        if state.is_some() {
            // A folder of ours whose own attributes changed: kept or freed, with what is in it.
            let _ = mark_in_sync(path);
            keep_or_free(path).await;
            return Ok(());
        }
        if add_only {
            // Photos has no folders to make: it stays here, marked so it never shows waiting.
            let _ = convert(path, None);
            return Ok(());
        }
        let made = remote.mkdir(&folder, &name).await?;
        convert(path, Some(&made)).map_err(|e| e.to_string())?;
        // What was copied in with it.
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            Box::pin(sync_path(remote, staging, &entry.path(), add_only)).await?;
        }
        return Ok(());
    }
    let edited_here = matches!(&state, Some((st, _)) if *st != CF_IN_SYNC_STATE_IN_SYNC);
    if add_only && (edited_here || (state.is_none() && !remote.accepts(&folder))) {
        // Photos: a photo saved over is not sent as a change (a photo has no versions to
        // replace), and a view (Favorites, Videos) is no place to add one. Left as it is.
        return Ok(());
    }
    let base = match &state {
        Some((s, _)) if *s == CF_IN_SYNC_STATE_IN_SYNC => {
            // In sync: only "Free up space" or "Always keep on this device" can have changed.
            keep_or_free(path).await;
            return Ok(());
        }
        Some(_) => identity_of(path),
        None => None,
    };
    // A copy to send from, so bytes still being written never reach Apple half done.
    let before = stamp(path).ok_or("the file went away")?;
    let dir = staging.join(format!("{}-{}", std::process::id(), stamp_key(&before)));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let staged = dir.join(&name);
    std::fs::copy(path, &staged).map_err(|e| e.to_string())?;
    let sent = if add_only {
        // Only ever an addition: never a replacement of a photo found under the same name.
        remote
            .upload(&folder, &staged)
            .await
            .map(|item| crate::writes::Sent {
                item,
                conflicted: false,
            })
    } else {
        send(remote, &folder, &staged, base.as_ref(), base.as_ref()).await
    };
    let _ = std::fs::remove_dir_all(&dir);
    let sent = sent?;
    if stamp(path) != Some(before) {
        // Changed while it was being sent: it goes again, as it is now.
        return Err("changed while sending; sending again".into());
    }
    let mut here = path.to_path_buf();
    if sent.conflicted {
        // Apple's file had changed elsewhere: ours went beside it, under its conflicted name,
        // and is named so here too; Apple's comes down in its place with the next listing.
        here = path.with_file_name(sent.item.display_name());
        std::fs::rename(path, &here).map_err(|e| e.to_string())?;
    }
    if state.is_some() && !sent.conflicted {
        update(&here, &sent.item).map_err(|e| e.to_string())
    } else {
        convert(&here, Some(&sent.item)).map_err(|e| e.to_string())
    }
}

/// When and how long: what tells a file changed under us.
#[derive(PartialEq, Eq, Clone, Copy)]
struct Stamp(u64, u128);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    let at = m
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(Stamp(m.len(), at))
}

fn stamp_key(s: &Stamp) -> String {
    format!("{:x}{:x}", s.0, s.1)
}

fn open_for_update(path: &Path) -> windows::core::Result<crate::cloud::Handle> {
    use windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS;
    use windows::Win32::Storage::FileSystem::*;
    // SAFETY: a path string alive for the call.
    unsafe {
        CreateFileW(
            &HSTRING::from(path.as_os_str()),
            (FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES).0 | GENERIC_ACCESS_RIGHTS(0x4000_0000).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map(crate::cloud::Handle)
}

/// A file or folder of ours from now on: a placeholder for `item`, in sync (`None`: one that
/// stays here, marked in sync so Explorer never shows it waiting).
fn convert(path: &Path, item: Option<&DriveItem>) -> windows::core::Result<()> {
    let handle = open_for_update(path)?;
    let identity = item
        .map(|i| serde_json::to_vec(i).unwrap_or_default())
        .unwrap_or_default();
    // SAFETY: the identity is alive for the call; the handle is ours.
    unsafe {
        CfConvertToPlaceholder(
            handle.0,
            (!identity.is_empty()).then(|| identity.as_ptr().cast()),
            identity.len() as u32,
            CF_CONVERT_FLAG_MARK_IN_SYNC,
            None,
            None,
        )
    }
}

/// A placeholder now standing for `item` (its new version at Apple), in sync.
fn update(path: &Path, item: &DriveItem) -> windows::core::Result<()> {
    let handle = open_for_update(path)?;
    let identity = serde_json::to_vec(item).unwrap_or_default();
    // SAFETY: as in convert.
    unsafe {
        CfUpdatePlaceholder(
            handle.0,
            None,
            Some(identity.as_ptr().cast()),
            identity.len() as u32,
            None,
            CF_UPDATE_FLAG_MARK_IN_SYNC,
            None,
            None,
        )
    }
}

fn mark_in_sync(path: &Path) -> windows::core::Result<()> {
    let handle = open_for_update(path)?;
    // SAFETY: our handle.
    unsafe {
        CfSetInSyncState(
            handle.0,
            CF_IN_SYNC_STATE_IN_SYNC,
            CF_SET_IN_SYNC_FLAG_NONE,
            None,
        )
    }
}

const PINNED: u32 = 0x0008_0000;
const UNPINNED: u32 = 0x0010_0000;
const NOT_ALL_HERE: u32 = 0x0040_0000; // FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS

/// "Always keep on this device" (pinned): every byte fetched now and kept. "Free up space"
/// (unpinned): the bytes let go, the file still listed and in iCloud. Windows only marks the
/// file; doing it is ours (Microsoft's hydration-policy notes). A folder's choice is its files'
/// too, for the folders already listed in it; a file of a pinned folder made later is pinned
/// by Windows itself.
async fn keep_or_free(path: &Path) {
    use std::os::windows::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    let attrs = meta.file_attributes();
    if meta.is_dir() {
        if attrs & (PINNED | UNPINNED) == 0 {
            return;
        }
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            let child = entry.path();
            if entry
                .metadata()
                .is_ok_and(|m| !m.is_dir() || m.file_attributes() & 0x0004_0000 == 0)
            {
                Box::pin(keep_or_free(&child)).await;
            }
        }
        return;
    }
    let here = attrs & NOT_ALL_HERE == 0;
    if attrs & PINNED != 0 && !here {
        let file = path.to_path_buf();
        // Fetched through Skykeep's own requests: waited on off the runtime's threads, which
        // answer them.
        let fetched = tokio::task::spawn_blocking(move || hydrate(&file)).await;
        if let Ok(Err(e)) = fetched {
            eprintln!("quince: keeping {} on this device: {e}", path.display());
        }
    } else if attrs & UNPINNED != 0 && here {
        if let Err(e) = dehydrate(path) {
            // Open in a program now: freed once it is closed and touched again.
            eprintln!("quince: freeing {}: {e}", path.display());
        }
    }
}

fn hydrate(path: &Path) -> windows::core::Result<()> {
    let handle = crate::cloud::open_attributes(path)?;
    // SAFETY: our handle; the whole file (length -1).
    unsafe { CfHydratePlaceholder(handle.0, 0, -1, CF_HYDRATE_FLAG_NONE, None) }
}

/// The file's bytes let go, never under a program that has it open (opened alone).
fn dehydrate(path: &Path) -> windows::core::Result<()> {
    use windows::Win32::Foundation::GENERIC_ACCESS_RIGHTS;
    use windows::Win32::Storage::FileSystem::*;
    // SAFETY: a path string alive for the call; the handle is closed when dropped.
    let handle = crate::cloud::Handle(unsafe {
        CreateFileW(
            &HSTRING::from(path.as_os_str()),
            (FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES).0 | GENERIC_ACCESS_RIGHTS(0x4000_0000).0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }?);
    // SAFETY: our handle; the whole file.
    unsafe { CfDehydratePlaceholder(handle.0, 0, -1, CF_DEHYDRATE_FLAG_NONE, None) }
}
