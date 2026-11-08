//! iCloud Drive as a folder on this computer (quince-vfs), served by the app itself: its
//! signed-in session, its library, and what it learns kept in the library for the window too.
//! Off until it is turned on in Settings. Everything a folder does: files written (kept here,
//! then sent), folders made, renames, moves, deletes into Recently Deleted. See
//! docs/mount-design.md.

// The folder itself is FUSE, on Linux only; on Windows it waits for the Cloud Files API.
#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_variables))]

use crate::{Auth, Data};
use quince_core::drive::DriveItem;
use quince_vfs::{Fut, Remote, Store};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// Whether the folder is wanted, kept in the library.
const ON_KEY: &str = "drive_folder_on";
/// Where it is mounted, when moved from the default.
const PATH_KEY: &str = "drive_folder_path";
/// How long a file's download link is used before asking Apple for a fresh one.
const LINK_LIFE: Duration = Duration::from_secs(20 * 60);

/// The folders while they are up; dropping one lets its folder go.
#[cfg(target_os = "linux")]
static SESSIONS: Mutex<Vec<(Which, PathBuf, fuser::BackgroundSession)>> = Mutex::new(Vec::new());

/// What the iCloud Drive folder has written and not yet sent, while it is up.
static WRITES: Mutex<Option<Arc<quince_vfs::writes::Writes>>> = Mutex::new(None);

/// What the iCloud Drive folder keeps on this computer, while it is up.
#[cfg(target_os = "linux")]
static KEEPER: Mutex<Option<quince_vfs::fuse::Keeper>> = Mutex::new(None);

struct AppRemote {
    app: AppHandle,
    links: Arc<Mutex<HashMap<String, (String, Instant)>>>,
}

/// The session's address for one Apple service, with the auth lock let go first.
async fn context(
    app: &AppHandle,
    service: &str,
) -> Result<(reqwest::Client, String, Vec<(String, String)>), String> {
    let auth = app.state::<Auth>();
    let client = auth.client.lock().await;
    let root = client
        .session()
        .service_url(service)
        .ok_or("Apple hasn't offered iCloud Drive for this account.")?;
    Ok((client.http().clone(), root, client.service_params()))
}

impl Remote for AppRemote {
    fn list(&self, folder: &str) -> Fut<Vec<DriveItem>> {
        let (app, folder) = (self.app.clone(), folder.to_string());
        Box::pin(async move {
            let (http, root, params) = context(&app, "drivews").await?;
            let asked_for = crate::asked_as(&params);
            let pass = crate::drive_pass(&folder);
            let _one = pass.lock().await;
            let items = quince_icloud::drive::fetch_folder(&http, &root, params, &folder)
                .await
                .map_err(|e| e.to_string())?;
            // Kept, so the window's Drive knows it too and the next look needs no network —
            // only in the library of the account whose session read it.
            let data = app.state::<Data>();
            let mut library = data.0.lock().await;
            crate::owned_by(&library, asked_for.as_deref())?;
            quince_icloud::drive::store_folder(&mut library, &folder, &items)
                .map_err(|e| e.to_string())?;
            Ok(items)
        })
    }

    fn mkdir(&self, parent: &DriveItem, name: &str) -> Fut<DriveItem> {
        let (app, parent, name) = (self.app.clone(), parent.drivewsid.clone(), name.to_string());
        Box::pin(async move {
            let (http, root, params) = context(&app, "drivews").await?;
            quince_icloud::drive::create_folder(&http, &root, params, &parent, &name)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn rename(&self, item: &DriveItem, name: &str) -> Fut<()> {
        let (app, item, name) = (self.app.clone(), item.clone(), name.to_string());
        Box::pin(async move {
            let (http, root, params) = context(&app, "drivews").await?;
            quince_icloud::drive::rename(&http, &root, params, &item, &name)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn move_to(&self, item: &DriveItem, folder: &DriveItem) -> Fut<()> {
        let (app, item, to) = (self.app.clone(), item.clone(), folder.drivewsid.clone());
        Box::pin(async move {
            let (http, root, params) = context(&app, "drivews").await?;
            quince_icloud::drive::move_to(&http, &root, params, &item, &to)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn upload(&self, folder: &DriveItem, path: &std::path::Path) -> Fut<DriveItem> {
        let (app, folder, path) = (
            self.app.clone(),
            folder.drivewsid.clone(),
            path.to_path_buf(),
        );
        Box::pin(async move {
            let (http, docws, params) = context(&app, "docws").await?;
            let drivews = context(&app, "drivews").await?.1;
            // iCloud with no room for it: said as that, and tried again as the folder retries,
            // so it goes up once space is freed — never an opaque refusal from Apple.
            let len = tokio::fs::metadata(&path).await.map_or(0, |m| m.len());
            let setup = app
                .state::<Auth>()
                .client
                .lock()
                .await
                .setup_url()
                .to_string();
            if let Ok(s) = quince_icloud::drive::storage(&http, &setup, params.clone()).await {
                if s.total_bytes > 0 && s.used_bytes.saturating_add(len) > s.total_bytes {
                    return Err("Your iCloud storage is full. It uploads when there's room.".into());
                }
            }
            let progress: quince_icloud::drive::Progress = Arc::new(|_, _| {});
            quince_icloud::drive::upload(&http, &docws, &drivews, params, &folder, &path, progress)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn space(&self) -> Fut<(u64, u64)> {
        let app = self.app.clone();
        Box::pin(async move {
            let (http, setup, params) = {
                let auth = app.state::<Auth>();
                let client = auth.client.lock().await;
                (
                    client.http().clone(),
                    client.setup_url().to_string(),
                    client.service_params(),
                )
            };
            let s = quince_icloud::drive::storage(&http, &setup, params)
                .await
                .map_err(|e| e.to_string())?;
            Ok((s.used_bytes, s.total_bytes))
        })
    }

    fn delete(&self, item: &DriveItem) -> Fut<()> {
        let (app, item) = (self.app.clone(), item.clone());
        Box::pin(async move {
            let (http, root, params) = context(&app, "drivews").await?;
            quince_icloud::drive::delete(&http, &root, params, &item)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn thumbnails(&self, folder: &str) -> Fut<HashMap<String, Vec<u8>>> {
        let (app, folder) = (self.app.clone(), folder.to_string());
        Box::pin(async move { crate::folder_thumbnails(&app, &folder).await })
    }

    fn read(&self, item: &DriveItem, offset: u64, len: u64) -> Fut<Vec<u8>> {
        let part = self.read_part(item, offset, len);
        Box::pin(async move { Ok(part.await?.bytes) })
    }

    fn read_part(&self, item: &DriveItem, offset: u64, len: u64) -> Fut<quince_vfs::Part> {
        let (app, links, item) = (self.app.clone(), Arc::clone(&self.links), item.clone());
        Box::pin(async move {
            let key = format!("{}~{}", item.drivewsid, item.etag.as_deref().unwrap_or(""));
            let known = links
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .filter(|(_, at)| at.elapsed() < LINK_LIFE)
                .map(|(u, _)| u.clone());
            // A link Apple no longer honours is asked for again, once.
            for fresh in [known.is_none(), true] {
                let url = match (&known, fresh) {
                    (Some(u), false) => u.clone(),
                    _ => {
                        let (http, docws, params) = context(&app, "docws").await?;
                        let u = quince_icloud::drive::download_url_of_version(
                            &http, &docws, params, &item,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        links
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(key.clone(), (u.clone(), Instant::now()));
                        u
                    }
                };
                let http = app.state::<Auth>().client.lock().await.http().clone();
                let r = http
                    .get(&url)
                    .header("Range", format!("bytes={offset}-{}", offset + len - 1))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = r.status().as_u16();
                if matches!(status, 401 | 403 | 404 | 410) && !fresh {
                    continue;
                }
                let whole = status == 200;
                if status != 206 && !whole && status != 416 {
                    return Err(format!("Apple answered {status}"));
                }
                // The length as served: a range answer's total (`bytes 0-0/52311`), a 416's
                // (`bytes */52311`: asked past the end of a file shorter than listed).
                let total = r
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(served_total);
                if status == 416 {
                    return match total {
                        Some(total) => Ok(quince_vfs::Part {
                            bytes: Vec::new(),
                            total: Some(total),
                        }),
                        None => Err(format!("Apple answered {status}")),
                    };
                }
                let bytes = r.bytes().await.map_err(|e| e.to_string())?;
                // A server that ignores the range sends the whole file: the part is cut from it,
                // and its length is the file's.
                return Ok(if whole {
                    let from = usize::try_from(offset)
                        .unwrap_or(usize::MAX)
                        .min(bytes.len());
                    let to = usize::try_from(offset + len)
                        .unwrap_or(usize::MAX)
                        .min(bytes.len());
                    quince_vfs::Part {
                        bytes: bytes[from..to].to_vec(),
                        total: Some(bytes.len() as u64),
                    }
                } else {
                    quince_vfs::Part {
                        bytes: bytes.to_vec(),
                        total,
                    }
                });
            }
            Err("Apple would not send the file".into())
        })
    }
}

/// The whole length a `Content-Range` gives: `bytes 0-9/52311` or `bytes */52311`. `None` for
/// one that says it is not known (`/*`) or cannot be read.
fn served_total(range: &str) -> Option<u64> {
    range
        .trim()
        .strip_prefix("bytes")?
        .rsplit_once('/')?
        .1
        .trim()
        .parse()
        .ok()
}

struct AppStore {
    app: AppHandle,
    /// The folder's own read-only connection on the library: what it lists never waits on the
    /// lock a sync pass or the window holds, and its version says when a listing is stale.
    reader: Mutex<MountReader>,
}

#[derive(Default)]
struct MountReader {
    reader: Option<crate::library_lock::Reader>,
    /// Which reader this is (opened again on another file, its version starts over) and what
    /// its version last said; `version` moves whenever either does.
    opened: u64,
    seen: Option<(u64, i64)>,
    version: u64,
}

impl AppStore {
    fn new(app: AppHandle) -> Self {
        Self {
            app,
            reader: Mutex::new(MountReader::default()),
        }
    }

    /// Run `read` on the folder's reader, opened (again) as the library's file needs.
    fn with_reader<T>(
        &self,
        read: impl FnOnce(&mut MountReader, &crate::library_lock::Reader) -> Option<T>,
    ) -> Option<T> {
        let dir = crate::state_dir(&self.app);
        let mut held = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !held.reader.as_ref().is_some_and(|r| r.current(&dir)) {
            held.reader = crate::library_lock::Reader::open(&dir);
            held.opened += 1;
        }
        let reader = held.reader.take()?;
        let found = read(&mut held, &reader);
        held.reader = Some(reader);
        found
    }
}

impl Store for AppStore {
    fn children(&self, folder: &str) -> Option<Vec<DriveItem>> {
        // Called on the folder's own threads, never the async runtime's: waiting is allowed —
        // but a file manager's every look at a name came here, and waited out whole sync passes
        // on the library's lock. On the folder's own reader it never waits on one.
        if let Some(Ok(items)) = self.with_reader(|_, r| Some(r.library().drive_children(folder))) {
            return Some(items);
        }
        let data = self.app.state::<Data>();
        data.0
            .blocking_read(|library| library.drive_children(folder))
            .ok()
    }

    fn version(&self) -> Option<u64> {
        self.with_reader(|held, r| {
            let now = (held.opened, r.data_version()?);
            if held.seen != Some(now) {
                held.seen = Some(now);
                held.version += 1;
            }
            Some(held.version)
        })
    }
}

/// The two folders: iCloud Drive and iCloud Photos, each its own switch and place, each
/// needing only its own service (someone with iCloud Photos off still has Drive).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Drive,
    Photos,
}

impl Which {
    fn on_key(self) -> &'static str {
        match self {
            Which::Drive => ON_KEY,
            Which::Photos => "photos_folder_on",
        }
    }
    fn path_key(self) -> &'static str {
        match self {
            Which::Drive => PATH_KEY,
            Which::Photos => "photos_folder_path",
        }
    }
    /// Its folder's name in the home folder, as iCloud for Windows names them.
    fn folder(self) -> &'static str {
        match self {
            Which::Drive => "iCloud Drive",
            Which::Photos => "iCloud Photos",
        }
    }
}

/// Where a folder goes: as moved, or its own name in the home folder.
fn place(app: &AppHandle, which: Which) -> PathBuf {
    let data = app.state::<Data>();
    let library = data.0.blocking_lock();
    library
        .state(which.path_key())
        .ok()
        .flatten()
        .map(PathBuf::from)
        .or_else(|| app.path().home_dir().ok().map(|h| h.join(which.folder())))
        .unwrap_or_else(|| PathBuf::from(which.folder()))
}

/// What Settings shows: whether it is up, where, and what stopped it if anything did.
#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub on: bool,
    pub path: String,
    pub problem: Option<String>,
    /// Files written into it and not yet at Apple (the Drive folder).
    pub waiting: Option<quince_vfs::writes::Waiting>,
    /// Whether this system can have the folder at all: Linux's FUSE only, until Windows has
    /// it through the Cloud Files API. Not offered where it cannot be, rather than offered and
    /// refused.
    pub available: bool,
}

#[cfg(target_os = "linux")]
fn mounted(which: Which) -> Option<PathBuf> {
    SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(w, ..)| *w == which)
        .map(|(_, p, _)| p.clone())
}

/// The most a cache may keep on this computer — the Drive folder's downloads, Photos' pictures:
/// 10 GB, or a fifth of the disk's free space when that is less — a full disk is never the price
/// of a cache.
pub(crate) fn budget(dir: &std::path::Path) -> u64 {
    const MOST: u64 = 10 * 1024 * 1024 * 1024;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return MOST;
        };
        // SAFETY: statvfs fills the struct it is given and reads only the path.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut s) } == 0 {
            // 64-bit here already, narrower on some systems: the conversion is for those.
            #[allow(clippy::useless_conversion)]
            let free = u64::from(s.f_bavail).saturating_mul(u64::from(s.f_frsize));
            return MOST.min(free / 5);
        }
    }
    MOST
}

/// Where every file manager looks for thumbnails (~/.cache/thumbnails).
fn thumbnail_cache(app: &AppHandle) -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| app.path().home_dir().ok().map(|h| h.join(".cache")))
        .map(|c| c.join("thumbnails"))
}

/// A folder brought up at `at`. Never over a folder with something in it: a mount would hide
/// what is there.
#[cfg(target_os = "linux")]
fn start(app: &AppHandle, which: Which, at: &std::path::Path) -> Result<(), String> {
    if mounted(which).is_some() {
        return Ok(());
    }
    if in_flatpak() {
        return Err("The folders aren’t part of the Flatpak version of Skykeep.".into());
    }
    if !std::path::Path::new("/dev/fuse").exists() {
        return Err("This computer has no FUSE, which the folder needs. Install your system's fuse3 package.".into());
    }
    // Left mounted by a Skykeep that was killed (an out-of-memory kill, SIGKILL, a crash
    // the panic hook never saw): nothing answers there, every look at the folder fails with
    // "Transport endpoint is not connected", and mounting over it failed the same way on
    // every launch after, until someone ran fusermount by hand. Let go of here first.
    unstick(at, |p| std::fs::metadata(p).map(|_| ()), lazy_unmount);
    if let Ok(mut entries) = std::fs::read_dir(at) {
        if entries.next().is_some() {
            return Err(format!(
                "{} already has files in it. Choose an empty folder.",
                at.display()
            ));
        }
    }
    let rt = tauri::async_runtime::handle().inner().clone();
    let mut fs = match which {
        Which::Drive => {
            let dir = crate::state_dir(app).join("drive-cache");
            let cache = quince_vfs::Cache::new(&dir)
                .map_err(|e| e.to_string())?
                .with_budget(budget(&dir));
            // What earlier days left beyond the budget is let go of now, not first thing tomorrow.
            cache.evict();
            let cache = Arc::new(cache);
            let remote = Arc::new(AppRemote {
                app: app.clone(),
                links: Arc::new(Mutex::new(HashMap::new())),
            });
            // What is written here waits beside the library (owner-only) until Apple has it.
            quince_vfs::fuse::DriveFs::new(remote, Arc::new(AppStore::new(app.clone())), cache, rt)
                .with_writes(&crate::state_dir(app).join("drive-writes"))
                .map_err(|e| e.to_string())?
        }
        Which::Photos => {
            // Read through the app's own photo cache; the folder keeps no second copy.
            let unused = crate::state_dir(app).join("photos-folder-cache");
            let cache = Arc::new(quince_vfs::Cache::new(&unused).map_err(|e| e.to_string())?);
            // Photos added here wait beside the library (owner-only) until Photos has them.
            quince_vfs::fuse::DriveFs::new(
                Arc::new(crate::mount_photos::PhotosRemote(app.clone())),
                Arc::new(crate::mount_photos::PhotosStore(app.clone())),
                cache,
                rt,
            )
            .add_only()
            .with_writes(&crate::state_dir(app).join("photos-folder-writes"))
            .map_err(|e| e.to_string())?
        }
    };
    // Pictures go where every file manager looks for thumbnails, so none of them downloads a
    // file to draw one.
    if let Some(thumbs) = thumbnail_cache(app) {
        fs = fs.with_thumbnails(at.to_path_buf(), thumbs);
    }
    if which == Which::Drive {
        *WRITES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = fs.writes();
        *KEEPER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(fs.keeper());
    }
    let session = quince_vfs::fuse::mount(fs, at)
        .map_err(|e| format!("The folder could not be mounted: {e}"))?;
    SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((which, at.to_path_buf(), session));
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop(which: Which) {
    let gone: Vec<_> = {
        let mut sessions = SESSIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (go, keep): (Vec<_>, Vec<_>) = sessions.drain(..).partition(|(w, ..)| *w == which);
        *sessions = keep;
        go
    };
    if which == Which::Drive {
        *WRITES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *KEEPER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    for (_, at, session) in gone {
        drop(session);
        // The empty folder the mount stood on is put away too, as it was made for it.
        let _ = std::fs::remove_dir(at);
    }
}

/// The folders in File Explorer (Cloud Files): each connected while Skykeep runs. Registered
/// once and kept registered across restarts, so Explorer keeps them and every file already
/// downloaded stays a plain file; unregistered only when turned off or at sign-out (`forget`).
#[cfg(windows)]
static CLOUD: Mutex<Vec<(Which, PathBuf, quince_vfs::cloud::CloudFolder)>> = Mutex::new(Vec::new());

/// Explorer's id for a folder, per Windows user (its SID) and kind.
#[cfg(windows)]
fn cloud_id(which: Which) -> Result<String, String> {
    quince_vfs::cloud::sync_root_id(match which {
        Which::Drive => "iCloudDrive",
        Which::Photos => "iCloudPhotos",
    })
}

#[cfg(windows)]
fn start(app: &AppHandle, which: Which, at: &std::path::Path) -> Result<(), String> {
    if mounted(which).is_some() {
        return Ok(());
    }
    let id = cloud_id(which)?;
    let icon = std::env::current_exe()
        .map(|exe| format!("{},0", exe.display()))
        .unwrap_or_default();
    let rt = tauri::async_runtime::handle().inner().clone();
    let (provider, name) = match which {
        Which::Drive => (
            quince_vfs::cloud::Provider::new(
                Arc::new(AppRemote {
                    app: app.clone(),
                    links: Arc::new(Mutex::new(HashMap::new())),
                }),
                Arc::new(AppStore::new(app.clone())),
                rt,
                // Saved files are copied here before they are sent, beside the library.
                crate::state_dir(app).join("drive-sending"),
            ),
            "iCloud Drive",
        ),
        // The same tree as the Photos folder on Linux (mount_photos): read through the app's
        // own photo cache, and only ever added to or deleted from.
        Which::Photos => (
            quince_vfs::cloud::Provider::new(
                Arc::new(crate::mount_photos::PhotosRemote(app.clone())),
                Arc::new(crate::mount_photos::PhotosStore(app.clone())),
                rt,
                crate::state_dir(app).join("photos-sending"),
            )
            .add_only(),
            "iCloud Photos",
        ),
    };
    let folder = quince_vfs::cloud::start(
        at,
        &quince_vfs::cloud::Shown {
            id: &id,
            name,
            icon: &icon,
        },
        provider,
    )
    .map_err(|e| format!("{name} could not be added to File Explorer: {e}"))?;
    CLOUD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((which, at.to_path_buf(), folder));
    Ok(())
}

/// Disconnected (quit, a library handed over): Explorer keeps the folder, and its files.
#[cfg(windows)]
fn stop(which: Which) {
    CLOUD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(w, ..)| *w != which);
}

/// Skykeep being uninstalled: its cloud folders unregistered, from where they are by default
/// (the shell's registration is by id; the bare one, where the shell refused, by path).
#[cfg(windows)]
pub fn forget_on_uninstall() {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    for which in [Which::Drive, Which::Photos] {
        if let Ok(id) = cloud_id(which) {
            quince_vfs::cloud::unregister(&home.join(which.folder()), &id);
        }
    }
}

/// Turned off, or signed out: no longer a cloud folder in Explorer at all.
#[cfg(windows)]
fn forget_folder(app: &AppHandle, which: Which) {
    stop(which);
    if let Ok(id) = cloud_id(which) {
        quince_vfs::cloud::unregister(&place(app, which), &id);
    }
}

#[cfg(windows)]
fn mounted(which: Which) -> Option<PathBuf> {
    CLOUD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(w, ..)| *w == which)
        .map(|(_, at, _)| at.clone())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn start(_app: &AppHandle, _which: Which, _at: &std::path::Path) -> Result<(), String> {
    Err("These folders are not on this system yet.".into())
}
#[cfg(not(any(target_os = "linux", windows)))]
fn stop(_which: Which) {}
#[cfg(not(any(target_os = "linux", windows)))]
fn mounted(_which: Which) -> Option<PathBuf> {
    None
}
#[cfg(not(windows))]
fn forget_folder(_app: &AppHandle, which: Which) {
    stop(which);
}

/// Drive folders Apple changed (the change feed's): brought up to date in File Explorer. On
/// Linux the mount reads the library afresh by itself.
pub fn drive_changed(folders: &[String]) {
    #[cfg(windows)]
    {
        let cloud = CLOUD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (which, _, folder) in cloud.iter() {
            if *which == Which::Drive {
                for id in folders {
                    folder.refresh(id);
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = folders;
}

/// Photos changed at Apple: every Photos folder File Explorer has listed brought up to date
/// (Windows). Its folders are views of the library (years, months, albums), so all of them are
/// looked at again, not one named folder.
pub fn photos_changed() {
    #[cfg(windows)]
    {
        let cloud = CLOUD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (which, _, folder) in cloud.iter() {
            if *which == Which::Photos {
                folder.refresh_all();
            }
        }
    }
}

/// At sign-out: each folder gone with the account — on Windows no longer in Explorer either.
pub fn forget(app: &AppHandle) {
    forget_folder(app, Which::Drive);
    forget_folder(app, Which::Photos);
}

fn status(app: &AppHandle, which: Which, problem: Option<String>) -> FolderStatus {
    let at = mounted(which).unwrap_or_else(|| place(app, which));
    let on = mounted(which).is_some();
    let waiting = if which == Which::Drive && on {
        WRITES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|w| w.waiting())
    } else {
        None
    };
    FolderStatus {
        on,
        path: at.to_string_lossy().into_owned(),
        problem,
        waiting,
        // Linux (not in the Flatpak, which has no FUSE), and Windows.
        available: (cfg!(target_os = "linux") && !in_flatpak()) || cfg!(windows),
    }
}

/// Running inside a Flatpak, where the folders are left out: the sandbox has no `/dev/fuse`,
/// and installing fuse3 on the host — what the error said — could not change that.
fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// A mount left behind by a run that ended without letting go of it: let go of now, lazily
/// (`-z`, as at quit). Answers whether there was one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unstick(
    at: &std::path::Path,
    look: impl Fn(&std::path::Path) -> std::io::Result<()>,
    mut unmount: impl FnMut(&std::path::Path) -> bool,
) -> bool {
    let stale = matches!(look(at), Err(e) if e.raw_os_error() == Some(ENOTCONN));
    stale && unmount(at)
}

/// "Transport endpoint is not connected": what a FUSE mount whose process is gone answers.
const ENOTCONN: i32 = 107;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn lazy_unmount(at: &std::path::Path) -> bool {
    ["fusermount3", "fusermount"].iter().any(|tool| {
        std::process::Command::new(tool)
            .arg("-uz")
            .arg(at)
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// At start: each brought up if it was on.
pub fn resume(app: &AppHandle) {
    for which in [Which::Drive, Which::Photos] {
        let wanted = {
            let data = app.state::<Data>();
            let library = data.0.blocking_lock();
            library.state(which.on_key()).ok().flatten().as_deref() == Some("1")
        };
        if wanted {
            if let Err(e) = start(app, which, &place(app, which)) {
                quiet!("a folder could not come up: {e}");
            }
        }
    }
}

/// Files written in the iCloud Drive folder and not yet sent: from the folder while it is up,
/// from what it wrote down when it went away while it is not.
pub fn unsent_files(data_dir: &std::path::Path) -> usize {
    let journal = |name: &str| {
        let dir = data_dir.join(name);
        if !dir.exists() {
            return 0;
        }
        quince_vfs::writes::Writes::new(&dir)
            .map(|w| {
                w.load()
                    .iter()
                    .filter(|(_, p)| !quince_vfs::writes::stays_local(&p.name))
                    .count()
            })
            .unwrap_or(0)
    };
    let drive = WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|w| w.waiting().files as usize);
    // The Photos folder's are counted from what it wrote down; once sending, they are in
    // Photos' upload queue as well, and counted there.
    drive.unwrap_or_else(|| journal("drive-writes")) + journal("photos-folder-writes")
}

/// Every thumbnail the folders left where file managers look for them, removed — with the
/// account they pictured, on a sign-out that keeps nothing. Only Quince's own; no other app's.
pub fn forget_thumbnails(app: &AppHandle) {
    let Some(root) = thumbnail_cache(app) else {
        return;
    };
    for size in ["normal", "large", "x-large", "xx-large"] {
        for f in std::fs::read_dir(root.join(size))
            .into_iter()
            .flatten()
            .flatten()
        {
            if std::fs::read(f.path()).is_ok_and(|b| quince_media::thumbnail_is_ours(&b)) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

/// Both folders let go of, waiting at most `limit`: a folder whose program is stuck in it (a
/// file manager mid-read on a lost connection) is detached instead (`fusermount3 -uz`), so
/// quitting never hangs on it and nothing is left answering "Transport endpoint is not
/// connected".
pub fn release_within(limit: std::time::Duration) {
    #[cfg(target_os = "linux")]
    let places: Vec<PathBuf> = SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|(_, at, _)| at.clone())
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        release();
        let _ = tx.send(());
    });
    if rx.recv_timeout(limit).is_err() {
        #[cfg(target_os = "linux")]
        for at in places {
            let _ = std::process::Command::new("fusermount3")
                .arg("-uz")
                .arg(&at)
                .status();
        }
    }
}

/// When signing out or quitting: both folders go with the session.
pub fn release() {
    stop(Which::Drive);
    stop(Which::Photos);
}

async fn set(app: AppHandle, which: Which, on: bool) -> Result<FolderStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let problem = if on {
            start(&app, which, &place(&app, which)).err()
        } else {
            forget_folder(&app, which);
            None
        };
        let data = app.state::<Data>();
        let mut library = data.0.blocking_lock();
        let _ = library.set_state(
            which.on_key(),
            if on && problem.is_none() { "1" } else { "0" },
        );
        drop(library);
        status(&app, which, problem)
    })
    .await
    .map_err(|e| e.to_string())
}

fn open(app: &AppHandle, which: Which) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let at = mounted(which).ok_or("That folder is off.")?;
    app.opener()
        .open_path(at.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn drive_folder_status(app: AppHandle) -> Result<FolderStatus, String> {
    tauri::async_runtime::spawn_blocking(move || status(&app, Which::Drive, None))
        .await
        .map_err(|e| e.to_string())
}

/// Turn the iCloud Drive folder on or off, remembered for the next start.
#[tauri::command]
pub async fn drive_folder_set(on: bool, app: AppHandle) -> Result<FolderStatus, String> {
    set(app, Which::Drive, on).await
}

/// The iCloud Drive folder opened in the system's file manager.
#[tauri::command]
pub async fn drive_folder_open(app: AppHandle) -> Result<(), String> {
    open(&app, Which::Drive)
}

#[tauri::command]
pub async fn photos_folder_status(app: AppHandle) -> Result<FolderStatus, String> {
    tauri::async_runtime::spawn_blocking(move || status(&app, Which::Photos, None))
        .await
        .map_err(|e| e.to_string())
}

/// Turn the iCloud Photos folder on or off, remembered for the next start.
#[tauri::command]
pub async fn photos_folder_set(on: bool, app: AppHandle) -> Result<FolderStatus, String> {
    set(app, Which::Photos, on).await
}

/// The iCloud Photos folder opened in the system's file manager.
#[tauri::command]
pub async fn photos_folder_open(app: AppHandle) -> Result<(), String> {
    open(&app, Which::Photos)
}

/// What the iCloud Drive folder keeps on this computer (Keep on This Computer): the files and
/// folders chosen, and the files inside a chosen folder. `None` while the folder is off —
/// there is nothing to keep downloaded without it.
#[tauri::command]
pub fn drive_kept() -> Option<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        KEEPER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(quince_vfs::fuse::Keeper::kept)
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// A file or folder kept on this computer — fetched whole behind, never trimmed — or let go
/// of, as Finder's Keep Downloaded. Answers what is kept now.
#[tauri::command]
pub fn drive_keep(drivewsid: String, on: bool) -> Result<Vec<String>, String> {
    #[cfg(target_os = "linux")]
    {
        let keeper = KEEPER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or("Turn on the Drive folder in Settings to keep files on this computer.")?;
        keeper.set(&drivewsid, on);
        Ok(keeper.kept())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (drivewsid, on);
        Err("Keeping files on this computer needs the iCloud Drive folder.".into())
    }
}

#[cfg(test)]
mod stale_tests {
    use super::*;

    /// The length a file is served at, from a range answer or a 416 past its end.
    #[test]
    fn a_served_length_is_read_from_content_range() {
        assert_eq!(served_total("bytes 0-0/52311"), Some(52_311));
        assert_eq!(served_total("bytes */52311"), Some(52_311));
        assert_eq!(served_total("bytes 0-9/*"), None);
        assert_eq!(served_total("items 0-9/10"), None);
    }

    /// A dead mount is let go of before a new one is made over it; a folder that answers, or
    /// is not there, is left as it is.
    #[test]
    fn a_mount_left_by_a_killed_run_is_let_go_of() {
        let at = std::path::Path::new("/somewhere/iCloud Drive");
        let dead = |_: &std::path::Path| Err(std::io::Error::from_raw_os_error(ENOTCONN));
        let mut asked = Vec::new();
        assert!(unstick(at, dead, |p| {
            asked.push(p.to_path_buf());
            true
        }));
        assert_eq!(asked, [at]);

        let fine = |_: &std::path::Path| Ok(());
        let missing = |_: &std::path::Path| Err(std::io::ErrorKind::NotFound.into());
        for look in [
            &fine as &dyn Fn(&std::path::Path) -> std::io::Result<()>,
            &missing,
        ] {
            assert!(!unstick(at, look, |_| panic!("nothing to let go of")));
        }
    }
}
