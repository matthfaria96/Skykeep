// Quince — desktop shell.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// What went wrong, said on the terminal in a development build only, and never with anything
/// of the person's in it — no names, record ids, addresses or Apple's answers. A release build
/// says nothing at all: Quince keeps no diagnostics and sends none anywhere.
macro_rules! quiet {
    ($($t:tt)*) => {
        if cfg!(debug_assertions) {
            eprintln!($($t)*);
        }
    };
}

mod alerts;
mod appearance;
mod apps;
mod clipboard;
mod contacts_cmds;
mod contacts_merge;
mod cpu;
mod data_access;
mod diagnostics;
mod drive_watch;
mod export;
mod folder_download;
mod grants;
mod hello;
mod intake;
mod library_lock;
mod links;
mod login;
mod migrate;
mod mount;
mod mount_photos;
mod notes_cmds;
#[cfg(target_os = "linux")]
mod notices;
mod outbox;
// Whether this copy is an MSIX package's (the Microsoft Store), and its app IDs if so.
mod package;
mod photo_cache;
mod photos;
#[cfg(target_os = "linux")]
use quince_media::poster;
mod power;
#[cfg(not(target_os = "linux"))]
mod poster {
    //! Elsewhere than Linux there is no GStreamer to read a frame with: Apple's own poster stands.
    pub fn first_seen_frame(_path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
mod push;
mod refresh;
mod reminders_cmds;
mod saving;
// Skykeep's results in GNOME's and KDE's own search, on the session bus.
#[cfg(target_os = "linux")]
mod search_provider;
mod settings;
mod startup_guard;
// Each app its own taskbar button on Windows, and the jump list's tasks.
#[cfg(windows)]
mod taskbar;
mod timing;
mod updates;
mod video;

use quince_auth::{Client, SignInOutcome, TwoFactorOptions};
use quince_core::calendar::{Calendar, Event};
use quince_core::drive::DriveItem;
use quince_core::notes::{Note, NoteFolder};
use quince_core::reminders::{Reminder, ReminderList, SmartList};
use quince_core::Library;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex;

struct Auth {
    client: Arc<Mutex<Client>>,
    /// A sign-in is in flight. The mutex would serialise a second click rather than reject
    /// it, so it would run a whole second SRP exchange and trigger another code push.
    signing_in: AtomicBool,
    /// What `auth_status` last read of the session, for a window that opens while the session
    /// is busy — being taken up, or checked by the timer — behind its lock: read from here it
    /// opens at once, never waiting out a round trip to Apple first.
    known: std::sync::Mutex<SessionFacts>,
}

/// What a window asks of the session as it opens.
#[derive(Clone, Default)]
struct SessionFacts {
    has_stored_session: bool,
    signed_in: bool,
    /// The account it is (`account_tag`).
    account: Option<String>,
    /// The apps the sidebar shows, those whose service Apple offers it (`account_services`).
    services: Vec<String>,
    /// Who it is, as Apple names them (`account_info`).
    name: Option<String>,
    email: Option<String>,
    web_access_allowed: Option<bool>,
    data_protection: Option<bool>,
    /// Apple has new iCloud terms, or wants the account finished, at icloud.com first.
    terms_update_needed: bool,
    repair_needed: bool,
    /// Signed in, but no keyring would keep the session: it lasts as long as Skykeep runs.
    unsaved: bool,
}

impl SessionFacts {
    fn of(client: &Client) -> Self {
        let s = client.session();
        SessionFacts {
            has_stored_session: client.has_stored_session(),
            signed_in: s.webservices.is_some(),
            account: s.dsid.as_deref().map(account_tag),
            // The apps the sidebar shows: each where the service its data comes from is offered.
            services: refresh::apps_offered(
                &s.webservices
                    .as_ref()
                    .and_then(|w| w.as_object())
                    .map(|o| o.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            ),
            name: s.account_name.clone(),
            email: s.account_email.clone().or_else(|| s.apple_id.clone()),
            web_access_allowed: s.gate.web_access_allowed(),
            data_protection: s.data_protection,
            terms_update_needed: s.gate.terms_update_needed(),
            repair_needed: s.gate.repair_needed(),
            unsaved: client.unsaved().is_some(),
        }
    }
}

/// The session's facts now: read from the client when nothing holds it, and kept; what was
/// last kept when something does — a resume, a sign-in, the heartbeat at the start of every
/// sync — rather than a wait behind a round trip to Apple. The sidebar and the account card
/// ask as the window opens, and waiting here left a launch on "Loading…" until Apple answered.
fn session_facts(auth: &Auth) -> SessionFacts {
    let fresh = auth
        .client
        .try_lock()
        .ok()
        .map(|client| SessionFacts::of(&client));
    let mut kept = auth
        .known
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match fresh {
        Some(facts) => {
            *kept = facts.clone();
            facts
        }
        None => kept.clone(),
    }
}

/// Clears the in-flight flag however the guarded block exits.
struct InFlight<'a>(&'a AtomicBool);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    /// A stored session exists and is worth trying to resume.
    has_stored_session: bool,
    /// Apple's service map is loaded, i.e. we are actually signed in right now.
    signed_in: bool,
    /// The stored session's account is the one the library already belongs to: the library
    /// can open on what it holds at once, while the session is taken up again behind it.
    library_ready: bool,
    /// Signed in for as long as Skykeep runs only: no keyring would keep the session.
    not_kept: bool,
}

struct Data(library_lock::LibraryLock);

/// Errors cross to the UI as plain sentences a person can act on.
type UiResult<T> = Result<T, String>;

/// The HTTP client, CloudKit root and query parameters, with the auth lock released before
/// any network work so a slow sync cannot block the heartbeat or a sign-out.
/// Re-read reminders after a write, so what is on screen is what Apple stored.
///
/// A delta from the stored sync token: a list change moves records other than the one written,
/// and the walk since the token brings those back too, with each changed list's sections. A
/// whole-zone pass here walked years of completed reminders and every list's membership file
/// after each tag, link, reorder or repeat, and the reminder's next edit queued behind it. A
/// token Apple refused starts the walk over from the beginning on its own.
async fn resync_reminders(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
) -> Result<(), String> {
    refresh_reminders(http, root, params, data, false).await
}

/// Ask Apple what changed in Reminders and store it — without holding the library while
/// Apple answers.
///
/// Every screen reads that library. A refresh that held it across the network made every
/// screen wait on Apple: when CloudKit was slow or overloaded (503, retry in 10s), Reminders,
/// Notes and the rest drew nothing at all until it gave up. The fetch runs unlocked; the
/// library is held only to store what came back.
/// One reminders pass at a time, for the reason [`NOTES_PASS`] gives: an older pass storing
/// after a newer write put the reminder back as it was before the write.
pub static REMINDERS_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One calendar pass or write at a time. A refresh that read Apple before a save and stored
/// after it pruned the new event, put an edited one back with its old etag — so the next
/// edit failed as a conflict — and brought a just-deleted one back.
pub static CALENDAR_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One contacts pass or write at a time, for the reason [`CALENDAR_PASS`] gives: a refresh
/// that read Apple before a save and stored after it put the old card back, and the next
/// edit, made from it, was refused as a conflict.
pub static CONTACTS_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One fetch and store of a Drive folder at a time, per folder. A folder opened just before a
/// rename is fetched with the old name; finishing after the rename's own re-read, it stored
/// that old listing last and the rename looked undone. Held from fetch to store, the re-read
/// after a write — which starts only once the write is made — always lands last.
///
/// Per folder, not one for all of Drive: one folder's slow answer held up every other folder
/// opened, the window's ten-second look and the Drive folder's listings behind it. The race
/// is a folder's own. A move touches two, and every move re-reads the folder it left before
/// the one it went to — each re-read waiting out an older look at its own folder — so the
/// destination's, which files the item where it now is, is still the last stored.
pub fn drive_pass(folder: &str) -> Arc<tokio::sync::Mutex<()>> {
    static PASSES: std::sync::Mutex<
        Option<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::Mutex::new(None);
    let mut passes = PASSES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let passes = passes.get_or_insert_with(std::collections::HashMap::new);
    // Those nobody holds or waits for are let go: the map is as large as what is in use.
    passes.retain(|_, pass| Arc::strong_count(pass) > 1);
    Arc::clone(passes.entry(folder.to_string()).or_default())
}
/// One Photos pass at a time, from reading its sync position through storing what came back.
pub static PHOTOS_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn refresh_reminders(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
    whole: bool,
) -> Result<(), String> {
    let _one = REMINDERS_PASS.lock().await;
    let since = if whole {
        None
    } else {
        data.0
            .lock()
            .await
            .sync_token("reminders")
            .map_err(|e| e.to_string())?
    };
    let asked_for = asked_as(&params);
    let fetched = quince_icloud::reminders::fetch_cloudkit(http, root, params, since.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    {
        // Stored as one transaction, so the screens read beside it (`lock_for_pass`).
        let mut library = data.0.lock_for_pass().await;
        owned_by(&library, asked_for.as_deref())?;
        // Changes still on their way to Apple stay as made here — stored with the pass, as one.
        // In place, as `refresh::store` does: a whole list's records and alarms are disk and
        // CPU work that would otherwise hold up every task queued on this worker.
        tokio::task::block_in_place(|| {
            library.atomically(|library| {
                quince_icloud::reminders::apply_cloudkit(library, fetched)?;
                outbox::replay_reminders(library)?;
                Ok::<_, quince_icloud::Error>(())
            })
        })
        .map_err(|e| e.to_string())?;
    }
    alerts::changed();
    Ok(())
}

/// The same for Notes.
/// One notes pass at a time, held from reading the sync position through storing the answer.
///
/// Passes fetch without holding the library, so two could overlap — the timer's and the one
/// after a save — and the slower, older one would store last: text from before the save put
/// back over it, and a note just made here pruned as unknown. This is not the library's lock;
/// every other screen keeps reading while a pass is out.
pub static NOTES_PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn refresh_notes(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
) -> Result<(), String> {
    let _one = NOTES_PASS.lock().await;
    let since = data
        .0
        .lock()
        .await
        .sync_token("notes")
        .map_err(|e| e.to_string())?;
    let asked_for = asked_as(&params);
    let fetched = quince_icloud::notes::fetch_cloudkit(http, root, params, since.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    // Stored as one transaction, so the screens read beside it (`lock_for_pass`).
    let mut library = data.0.lock_for_pass().await;
    owned_by(&library, asked_for.as_deref())?;
    // Edits still on their way to Apple stay as typed here — stored with the pass, as one.
    tokio::task::block_in_place(|| {
        library.atomically(|library| {
            quince_icloud::notes::apply_cloudkit(library, fetched)?;
            outbox::replay(library, outbox::NOTES)?;
            Ok::<_, quince_icloud::Error>(())
        })
    })
    .map_err(|e| e.to_string())
}

async fn cloudkit_context(
    auth: &State<'_, Auth>,
) -> Result<(reqwest::Client, String, Vec<(String, String)>), String> {
    let client = auth.client.lock().await;
    let root = client
        .session()
        .service_url("ckdatabasews")
        .ok_or("Apple hasn't offered CloudKit for this account.")?;
    Ok((client.http().clone(), root, client.service_params()))
}

/// Undo percent-encoding in a path segment. Enough for the ids this scheme carries.
/// The note attachment, and which of its pictures, a `quince://` request asks for:
/// `note/<attachment id>/preview` or `note/<attachment id>/file`, escaped as Tauri escapes it.
fn note_part(path: &str) -> Option<(String, quince_icloud::note_media::Part)> {
    let path = percent_decode(path.trim_start_matches('/'));
    let rest = path.strip_prefix("note/")?;
    let (id, part) = rest.rsplit_once('/')?;
    let part = match part {
        "preview" => quince_icloud::note_media::Part::Preview,
        "file" | "inline" => quince_icloud::note_media::Part::File,
        "drawing" | "drawing-dark" => quince_icloud::note_media::Part::Drawing,
        _ => return None,
    };
    (!id.is_empty()).then(|| (id.to_string(), part))
}

/// The reminder picture a `quince://` request asks for: `rimg/<UUID>`, the `Attachment/`
/// record it stands for.
fn reminder_image_part(path: &str) -> Option<String> {
    let path = percent_decode(path.trim_start_matches('/'));
    let id = path.strip_prefix("rimg/")?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .then(|| format!("Attachment/{id}"))
}

/// A reminder's picture as the window can draw it: from disk when fetched before, from Apple
/// when not, and a HEIC (what an iPhone takes) made a JPEG once — the engine draws no HEIC.
async fn reminder_image_shown(app: &tauri::AppHandle, attachment: &str) -> Result<Vec<u8>, String> {
    let dir = state_dir(app).join("reminder-media");
    let safe = media_file_id(attachment);
    let shown = dir.join(format!("{safe}-shown"));
    if let Ok(bytes) = tokio::fs::read(&shown).await {
        return Ok(bytes);
    }
    let (http, root, params) = {
        let auth = app.state::<Auth>();
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("ckdatabasews")
            .ok_or("no CloudKit")?;
        (client.http().clone(), root, client.service_params())
    };
    let bytes = quince_icloud::reminders::more::attachment_file(&http, &root, params, attachment)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no such picture")?;
    let bytes = if quince_media::is_heif(&bytes) {
        crate::cpu::run(move || quince_media::heic_to_jpeg(&bytes))
            .await
            .map_err(|e| e.to_string())??
    } else {
        bytes
    };
    tokio::fs::create_dir_all(&dir).await.ok();
    let part = shown.with_extension("part");
    if tokio::fs::write(&part, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&part, &shown).await;
    }
    Ok(bytes)
}

/// A note attachment's id as part of a file name in `note-media`: letters, digits and dashes,
/// so an id from a request is never a path.
fn media_file_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A note attachment's picture, from disk when it has been fetched before, from Apple when
/// not — kept, so a note opens on its pictures at once the next time.
async fn note_media_cached(
    app: &tauri::AppHandle,
    id: &str,
    part: quince_icloud::note_media::Part,
) -> Result<Vec<u8>, String> {
    let safe = media_file_id(id);
    let name = match part {
        quince_icloud::note_media::Part::Preview => "preview",
        quince_icloud::note_media::Part::File => "file",
        quince_icloud::note_media::Part::Drawing => "drawing",
    };
    let path = state_dir(app)
        .join("note-media")
        .join(format!("{safe}-{name}"));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok(bytes);
    }
    let (http, root, params) = {
        let auth = app.state::<Auth>();
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("ckdatabasews")
            .ok_or("no CloudKit")?;
        (client.http().clone(), root, client.service_params())
    };
    let bytes = quince_icloud::note_media::bytes(&http, &root, params, id, part)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.ok();
    }
    // Whole or not at all: written beside it and renamed in, so a second request for the same
    // picture (a drawing's light and dark ink, the viewer) never reads half a file — which
    // the window took for a broken picture and showed as a file for the rest of the session.
    let part = path.with_extension("part");
    if tokio::fs::write(&part, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&part, &path).await;
    }
    Ok(bytes)
}

/// A note's picture as the window can draw it: a HEIC original (what an iPhone takes) made a
/// JPEG on this computer once and kept beside it — the engine draws no HEIC, and the picture
/// fell back to a file chip. Opening it elsewhere still hands over the original.
///
/// A drawing is shown as its ink alone, for the page's look (`dark`): see
/// `quince_media::drawing_ink`.
async fn note_media_shown(
    app: &tauri::AppHandle,
    id: &str,
    part: quince_icloud::note_media::Part,
    dark: bool,
) -> Result<Vec<u8>, String> {
    let bytes = note_media_cached(app, id, part).await?;
    let drawing = part == quince_icloud::note_media::Part::Drawing;
    if !drawing && !quince_media::is_heif(&bytes) {
        return Ok(bytes);
    }
    let safe = media_file_id(id);
    let name = match (drawing, dark) {
        (true, true) => format!("{safe}-ink-dark.png"),
        (true, false) => format!("{safe}-ink.png"),
        _ => format!("{safe}-shown.jpg"),
    };
    let shown = state_dir(app).join("note-media").join(name);
    if let Ok(made) = tokio::fs::read(&shown).await {
        return Ok(made);
    }
    let jpeg = crate::cpu::run(move || {
        if drawing {
            quince_media::drawing_ink(&bytes, dark)
        } else {
            quince_media::heic_to_jpeg(&bytes)
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let part_path = shown.with_extension("part");
    if tokio::fs::write(&part_path, &jpeg).await.is_ok() {
        let _ = tokio::fs::rename(&part_path, &shown).await;
    }
    Ok(jpeg)
}

/// The longest side a note's picture is drawn at in the note. A photo from a phone is 12 to 48
/// megapixels; drawn in the note at its full size, a long note of them was a decode of each
/// whole picture while scrolling, and the memory to hold them. The viewer and Open still have
/// the whole picture.
const INLINE_LONG: u32 = 1600;

/// A picture made the size a note draws it, or `None` when it already is (or is one that must
/// not be redrawn: a GIF would lose its motion).
fn inline_picture(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(b"GIF8") {
        return None;
    }
    quince_media::fit(bytes, INLINE_LONG).ok().flatten()
}

/// A note's picture as drawn in the note: `note_media_shown`'s picture made no larger than
/// `INLINE_LONG`, made once and kept beside it. One small enough already is served as it is.
async fn note_media_inline(app: &tauri::AppHandle, id: &str) -> Result<Vec<u8>, String> {
    let made = state_dir(app)
        .join("note-media")
        .join(format!("{}-inline", media_file_id(id)));
    if let Ok(bytes) = tokio::fs::read(&made).await {
        return Ok(bytes);
    }
    let shown = note_media_shown(app, id, quince_icloud::note_media::Part::File, false).await?;
    let (shown, small) = crate::cpu::run(move || {
        let small = inline_picture(&shown);
        (shown, small)
    })
    .await?;
    let Some(small) = small else {
        return Ok(shown);
    };
    let part = made.with_extension("part");
    if tokio::fs::write(&part, &small).await.is_ok() {
        let _ = tokio::fs::rename(&part, &made).await;
    }
    Ok(small)
}

/// The edge of the small square picture the notes list shows beside a note: three times its
/// 34 points, sharp on a high-density screen.
const NOTE_THUMB: u32 = 102;

/// Where a note's picture already sits on this computer, to make the list's small one from:
/// the picture as the note draws it, a drawing as Apple renders it, or a document's rendering
/// — each kept the first time the note drew it. Never fetched for the list.
fn note_thumb_source(dir: &std::path::Path, id: &str) -> Option<std::path::PathBuf> {
    let safe = media_file_id(id);
    ["inline", "drawing", "preview"]
        .iter()
        .map(|part| dir.join(format!("{safe}-{part}")))
        .find(|p| p.is_file())
}

/// The note attachment whose small list picture a `quince://` request asks for:
/// `note/<attachment id>/thumb`.
fn note_thumb_part(path: &str) -> Option<String> {
    let path = percent_decode(path.trim_start_matches('/'));
    let id = path.strip_prefix("note/")?.strip_suffix("/thumb")?;
    (!id.is_empty() && !id.contains('/')).then(|| id.to_string())
}

/// A note's small list picture: made once from the picture already kept on this computer
/// (`note_thumb_source`), cut to a square as Notes shows it, and kept. Nothing is asked of
/// Apple — a picture not kept yet is no picture, and the list shows none.
async fn note_media_thumb(app: &tauri::AppHandle, id: &str) -> Result<Vec<u8>, String> {
    let dir = state_dir(app).join("note-media");
    let made = dir.join(format!("{}-thumb", media_file_id(id)));
    if let Ok(bytes) = tokio::fs::read(&made).await {
        return Ok(bytes);
    }
    let source = note_thumb_source(&dir, id).ok_or("not kept on this computer")?;
    let bytes = tokio::fs::read(&source).await.map_err(|e| e.to_string())?;
    let small = crate::cpu::run(move || quince_media::avatar(&bytes, None, NOTE_THUMB))
        .await?
        .map_err(|e| e.to_string())?;
    let part = made.with_extension("part");
    if tokio::fs::write(&part, &small).await.is_ok() {
        let _ = tokio::fs::rename(&part, &made).await;
    }
    Ok(small)
}

/// Each note's first picture that this computer already holds, by note, for the small picture
/// the list shows beside it (`note/<id>/thumb`). Read from the library and the files already
/// kept: nothing is fetched for it, so a note whose pictures were never drawn here shows none
/// until they are.
#[tauri::command]
async fn note_pictures(
    app: tauri::AppHandle,
    data: State<'_, Data>,
) -> UiResult<std::collections::HashMap<String, String>> {
    let first = data
        .0
        .read(quince_core::Library::note_first_pictures)
        .await
        .map_err(|e| e.to_string())?;
    let dir = state_dir(&app).join("note-media");
    crate::cpu::run(move || {
        first
            .into_iter()
            .filter(|(_, id)| note_thumb_source(&dir, id).is_some())
            .collect()
    })
    .await
}

/// Show a file the app saved in its folder, in the file manager, chosen: a "Saved to
/// Downloads" toast's Show in Folder. Only a file in Downloads — what the app itself saves
/// there — never any other path the page might name.
#[tauri::command]
async fn download_reveal(path: String, app: tauri::AppHandle) -> UiResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let downloads = app.path().download_dir().map_err(|e| e.to_string())?;
    let downloads = std::fs::canonicalize(&downloads).map_err(|e| e.to_string())?;
    let file =
        std::fs::canonicalize(&path).map_err(|_| "That file isn’t there any more.".to_string())?;
    if !file.starts_with(&downloads) {
        return Err("Only a file in Downloads can be shown.".into());
    }
    app.opener()
        .reveal_item_in_dir(&file)
        .map_err(|e| e.to_string())
}

/// A plain report of how this install is doing, for a bug report: versions, the system, and
/// each service's last sync and last error — never a name, an address, or anything from a
/// note, card or file. Written to Downloads for the person to read and send themselves;
/// Quince sends nothing anywhere.
#[tauri::command]
async fn diagnostics_save(app: tauri::AppHandle, data: State<'_, Data>) -> UiResult<String> {
    let health = {
        let library = data.0.lock().await;
        library.sync_health().map_err(|e| e.to_string())?
    };
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find(|l| l.starts_with("PRETTY_NAME=")).map(|l| {
                l.trim_start_matches("PRETTY_NAME=")
                    .trim_matches('"')
                    .to_string()
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    // Addresses and ids that an error might quote are taken out.
    let scrub = |t: &str| -> String {
        t.split_whitespace()
            .map(|w| {
                if w.contains('@') || w.len() > 24 && w.chars().any(|c| c.is_ascii_digit()) {
                    "[…]"
                } else {
                    w
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(out, "Skykeep diagnostic report");
    let _ = writeln!(
        out,
        "Skykeep {} · {} · {} {}",
        app.package_info().version,
        os,
        std::env::consts::ARCH,
        kernel.trim()
    );
    let _ = writeln!(
        out,
        "Desktop: {} ({})",
        if desktop.is_empty() {
            "unknown"
        } else {
            &desktop
        },
        if session.is_empty() {
            "unknown"
        } else {
            &session
        }
    );
    let _ = writeln!(out, "Written: {}", jiff::Timestamp::now());
    let _ = writeln!(out, "\nSync, by service:");
    for h in &health {
        let when = h
            .last_synced_at
            .and_then(|t| jiff::Timestamp::from_second(t).ok())
            .map_or_else(|| "never".into(), |t| t.to_string());
        let _ = writeln!(
            out,
            "  {:10} last synced {when}{}",
            h.service,
            h.last_error
                .as_deref()
                .map(|e| format!(" — last error: {}", scrub(e)))
                .unwrap_or_default()
        );
    }
    let dir = app.path().download_dir().map_err(|e| e.to_string())?;
    let path = dir.join(format!(
        "Skykeep diagnostics {}.txt",
        jiff::Zoned::now().strftime("%Y-%m-%d %H.%M")
    ));
    tokio::fs::write(&path, out)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// What a note's attachment is, to draw it where it sits. Asked of Apple once and kept in the
/// library — an attachment never changes under its id — so a note opens on its pictures and
/// cards at once, offline too, instead of two lookups per attachment every time it is drawn.
/// Read only: nothing is written to Apple.
#[tauri::command]
async fn note_attachment_info(
    id: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<serde_json::Value> {
    let key = format!("notes.attachment.{id}");
    if let Ok(Some(kept)) = data.0.lock().await.state(&key) {
        if let Ok(info) = serde_json::from_str::<serde_json::Value>(&kept) {
            return Ok(info);
        }
    }
    let (http, root, params) = cloudkit_context(&auth).await?;
    let info = quince_icloud::note_media::info(&http, &root, params, &id)
        .await
        .map_err(|e| e.to_string())?;
    let info = serde_json::to_value(&info).map_err(|e| e.to_string())?;
    if let Ok(json) = serde_json::to_string(&info) {
        let _ = data.0.lock().await.set_state(&key, &json);
    }
    Ok(info)
}

/// Open a note's attachment in the app this computer uses for it: the file is fetched,
/// saved beside the others under its own name, and handed over. Opened before, it is handed
/// over from where it was saved at once — an attachment never changes under its id — with
/// nothing asked of Apple: asking its name again first, every time, kept each open waiting a
/// round trip, and behind the session's lock whenever a check of the session held it.
#[tauri::command]
async fn note_attachment_open(
    id: String,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
) -> UiResult<()> {
    timing::mark("attachment asked");
    let folder: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    if folder.is_empty() {
        return Err("That attachment is not one Skykeep knows.".into());
    }
    let dir = state_dir(&app).join("note-files").join(&folder);
    if let Some(path) = saved_attachment(&dir).await {
        return hand_over_attachment(&app, &path).await;
    }
    // Its name and its bytes asked for together, not one after the other.
    let (http, root, params) = cloudkit_context(&auth).await?;
    let (info, bytes) = tokio::join!(
        quince_icloud::note_media::info(&http, &root, params, &id),
        note_media_cached(&app, &id, quince_icloud::note_media::Part::File),
    );
    let info = info.map_err(|e| e.to_string())?;
    let bytes = bytes?;
    timing::mark("attachment fetched");
    // The name is whoever shared the note's to choose: made a plain file name here, never a
    // path, and never one that pretends to be something else.
    let name = attachment_name(info.filename.as_deref(), &folder);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    // Written beside it and renamed into place: a file cut short by a crash is never the one a
    // later open finds and hands over. Hidden (a leading dot) until whole, which no saved name
    // has.
    let partial = dir.join(".partial");
    tokio::fs::write(&partial, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    // Readable by this user alone, and never runnable, whatever was there before.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|e| e.to_string())?;
    }
    move_into_place(&partial, &path)
        .await
        .map_err(|e| e.to_string())?;
    saving::mark_from_internet(&path);
    hand_over_attachment(&app, &path).await
}

/// The attachment saved in `dir` by an earlier open: the one whole file there.
async fn saved_attachment(dir: &std::path::Path) -> Option<PathBuf> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    let mut found = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if !entry.file_type().await.is_ok_and(|t| t.is_file()) || found.is_some() {
            return None;
        }
        found = Some(entry.path());
    }
    found
}

/// A saved attachment handed to the app this computer uses for it — or, if it is a program,
/// a script or a launcher, shown in its folder instead: opening it would run it, and a shared
/// note can carry one from anyone.
async fn hand_over_attachment(app: &tauri::AppHandle, path: &std::path::Path) -> UiResult<()> {
    use tauri_plugin_opener::OpenerExt;
    use tokio::io::AsyncReadExt;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // What `runs_code` reads of a file: its first bytes.
    let mut head = Vec::with_capacity(64);
    tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?
        .take(64)
        .read_to_end(&mut head)
        .await
        .map_err(|e| e.to_string())?;
    timing::mark("attachment handed over");
    if runs_code(&name, &head) {
        return app
            .opener()
            .reveal_item_in_dir(path)
            .map_err(|e| e.to_string());
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// A note attachment's file name as it is saved: Apple's name for it with anything that could
/// make it a path (`/`, `\\`, `:`, `..`), hide it (a leading dot), or show it as something it
/// is not (direction marks that turn `exe.pdf` around) taken out; a name Windows keeps for a
/// device made harmless. Nothing left, and it is named after the attachment.
pub(crate) fn attachment_name(given: Option<&str>, id: &str) -> String {
    let cleaned: String = given
        .unwrap_or_default()
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '/' | '\\'
                        | ':'
                        | '*'
                        | '?'
                        | '"'
                        | '<'
                        | '>'
                        | '|'
                        | '\u{200E}'
                        | '\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{FEFF}'
                )
        })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    // Long enough for any real name, short enough for every file system — and, cut short, it
    // still ends in neither a dot nor a space, which Windows drops (turning `a.pdf.` into
    // another file than the one checked).
    //
    // Counted in bytes, as Linux counts a name (255 at most, with room left for " 2" and
    // ".part"), and cut from the stem so the extension stays: 120 characters dropped ".pdf",
    // and 120 CJK characters or emoji were over the limit and could not be saved at all.
    const LONGEST: usize = 120;
    let cut: String = if trimmed.len() <= LONGEST {
        trimmed.to_string()
    } else {
        let (stem, ext) = name_parts(trimmed);
        let room = LONGEST.saturating_sub(ext.len()).max(1);
        let mut at = room.min(stem.len());
        while !stem.is_char_boundary(at) {
            at -= 1;
        }
        format!("{}{ext}", stem[..at].trim_end_matches(['.', ' ']))
    };
    let mut name = cut.trim_end_matches(['.', ' ']).to_string();
    // Windows reads a device in the stem whatever follows it (`CON.txt`, `CON .txt`), and
    // counts superscript digits as digits (`COM¹`).
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            let mut n = n.chars();
            matches!(
                (n.next(), n.next()),
                (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
            )
        })
    };
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || numbered("COM")
        || numbered("LPT");
    if device {
        name.insert(0, '_');
    }
    if name.is_empty() {
        name = format!("Attachment {}", id.chars().take(8).collect::<String>());
    }
    name
}

/// Whether opening this file would run it rather than show it: a program, a script, a
/// launcher or an installer, by its name or by what its first bytes say it is.
pub(crate) fn runs_code(name: &str, bytes: &[u8]) -> bool {
    const RUN: &[&str] = &[
        // Linux
        "desktop",
        "kdelnk",
        "sh",
        "bash",
        "zsh",
        "csh",
        "ksh",
        "fish",
        "run",
        "bin",
        "out",
        "elf",
        "so",
        "appimage",
        "flatpak",
        "flatpakref",
        "flatpakrepo",
        "snap",
        "deb",
        "rpm",
        "pkg",
        "py",
        "pyw",
        "pyc",
        "pl",
        "rb",
        "php",
        "lua",
        "tcl",
        "jar",
        "jnlp",
        "class",
        // Windows
        "exe",
        "com",
        "scr",
        "pif",
        "msi",
        "msix",
        "msp",
        "msc",
        "appx",
        "bat",
        "cmd",
        "ps1",
        "psm1",
        "psd1",
        "vbs",
        "vbe",
        "js",
        "jse",
        "wsf",
        "wsh",
        "ws",
        "hta",
        "cpl",
        "inf",
        "reg",
        "lnk",
        "url",
        "scf",
        "chm",
        "application",
        "appref-ms",
        "gadget",
        "library-ms",
        "settingcontent-ms",
        "dll",
        "sys",
        "iso",
        "img",
        "vhd",
        "vhdx",
        "appxbundle",
        "msixbundle",
        "appinstaller",
        "search-ms",
        "searchconnector-ms",
        "diagcab",
        "rdp",
        "website",
        "wsc",
        "sct",
        "msh",
        "msh1",
        "msh2",
        "mshxml",
        "msh1xml",
        "msh2xml",
        "ps1xml",
        "ps2",
        "ps2xml",
        "psc1",
        "psc2",
        "vb",
        "grp",
        "hlp",
        "ins",
        "isp",
        "theme",
        "themepack",
        "deskthemepack",
        "cab",
        "xbap",
        "jsx",
        // Office documents that carry macros or add-ins, and OneNote's (a favourite carrier)
        "docm",
        "dotm",
        "xlsm",
        "xltm",
        "xlam",
        "xla",
        "xll",
        "pptm",
        "potm",
        "ppsm",
        "ppam",
        "sldm",
        "one",
        "onepkg",
        "onetoc2",
        "mdb",
        "mde",
        "accde",
        "ade",
        "adp",
        "mam",
        "mda",
        // Pages a browser runs from the disk, with their scripts
        "html",
        "htm",
        "xhtml",
        "xht",
        "shtml",
        "mht",
        "mhtml",
        "svg",
        "svgz",
        // macOS
        "app",
        "command",
        "tool",
        "workflow",
        "scpt",
        "terminal",
        "webloc",
        "inetloc",
        "dmg",
        "mpkg",
    ];
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    RUN.contains(&ext.as_str())
        || bytes.starts_with(b"#!")
        || bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(b"MZ")
        || bytes.starts_with(b"[Desktop Entry]")
        || bytes.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
        || bytes.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
}

/// The contact a `quince://` request asks for the photo of.
///
/// WebKitGTK reads `quince://photo/<id>` with "photo" as the host, while Tauri's own helper
/// builds `quince://localhost/photo%2F<id>` — the whole path escaped, slash and all. The path
/// is decoded before it is read: matched undecoded, no request ever found its photo, and every
/// contact showed initials with nothing to say why.
fn photo_guid(host: Option<&str>, path: &str) -> Option<String> {
    let path = percent_decode(path.trim_start_matches('/'));
    let guid = if host == Some("photo") {
        path
    } else {
        path.strip_prefix("photo/")?.to_string()
    };
    (!guid.is_empty()).then_some(guid)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Everything Quince keeps — the library, every photo, the session's cookies — in a folder only
/// its owner can open. Made with the system's usual permissions it was readable by every user
/// of the computer, and the cookies are a signed-in iCloud session. Everything inside is covered
/// at once: nothing in a folder can be reached without opening the folder.
///
/// What is directly inside is made the owner's alone too — the library and its journal, each
/// folder of photos, notes' files, faces — so a copy or a restore that loses the outer folder's
/// mode does not leave them open to everyone. Called again once the library is open, for the
/// files the database makes as it opens.
fn keep_private(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // Never through a link: its target is not this folder's to change.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let mode = if kind.is_dir() {
                0o700
            } else if kind.is_file() {
                0o600
            } else {
                continue;
            };
            let path = entry.path();
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|m| m.permissions().mode() & 0o777 != mode)
            {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode));
            }
        }
    }
}

/// The library could not be opened: said in a window of its own, with where it is, and the app
/// ends — never a panic. A release build on Windows has no console, so a launch that failed
/// here showed nothing at all, every time.
fn cannot_start(dir: &std::path::Path, why: &str) -> ! {
    let text = format!(
        "{why}\n\nNothing in it has been changed or removed. Your library is in:\n{}",
        dir.display()
    );
    eprintln!("Skykeep couldn’t start: {text}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Skykeep couldn’t open your library")
        .set_description(&text)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    std::process::exit(1)
}

/// Why the library would not open, as a person can act on it.
fn library_problem(e: &quince_core::Error) -> String {
    e.trouble().map_or_else(|| e.to_string(), trouble_words)
}

/// [`library_problem`] for the folder itself.
fn folder_problem(e: &std::io::Error) -> String {
    quince_core::io_trouble(e).map_or_else(
        || format!("Skykeep’s folder couldn’t be used: {e}"),
        trouble_words,
    )
}

fn trouble_words(t: quince_core::Trouble) -> String {
    match t {
        quince_core::Trouble::Full => "The disk is full. Free some space, then open Skykeep again.",
        quince_core::Trouble::ReadOnly => {
            "Skykeep can’t write to its folder. Check that the folder below, and the disk it is \
             on, can be written to, then open Skykeep again."
        }
        quince_core::Trouble::Busy => {
            "Another program is holding the library — most likely another Skykeep still \
             closing. Wait a moment, then open Skykeep again."
        }
    }
    .to_string()
}

fn state_dir(app: &tauri::AppHandle) -> PathBuf {
    let roaming = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join("quince"));
    // Windows: Local app data, once the library has moved there (migrate::to_local); still
    // Roaming while a move could not be made.
    #[cfg(windows)]
    if let Ok(local) = app.path().app_local_data_dir() {
        if local.join("library.db").exists() || !roaming.join("library.db").exists() {
            return local;
        }
    }
    roaming
}

#[tauri::command]
async fn auth_status(auth: State<'_, Auth>, data: State<'_, Data>) -> UiResult<Status> {
    timing::mark("auth_status asked");
    // Busy (a resume, a sign-in, the timer's check): what was last known, rather than a wait.
    let facts = session_facts(&auth);
    let owner = data.0.lock().await.state(ACCOUNT_KEY).ok().flatten();
    timing::mark("auth_status answered");
    Ok(Status {
        has_stored_session: facts.has_stored_session,
        signed_in: facts.signed_in,
        library_ready: facts.has_stored_session
            && facts.account.is_some()
            && owner == facts.account,
        not_kept: facts.signed_in && facts.unsaved,
    })
}

/// Try the stored session. `false` just means a sign-in is needed — not an error.
#[tauri::command]
async fn auth_resume(app: tauri::AppHandle, auth: State<'_, Auth>) -> UiResult<&'static str> {
    timing::mark("auth_resume asked");
    if LIVE.load(Ordering::SeqCst) {
        return Ok("library");
    }
    let outcome = {
        let mut client = auth.client.lock().await;
        // Another window took the session up while this one waited for it (each window asks
        // as it opens): asking Apple again would only be a second wait.
        if LIVE.load(Ordering::SeqCst) {
            return Ok("library");
        }
        let outcome = resume_session(&mut client).await?;
        timing::mark("auth_resume: session resumed");
        if outcome == "library" {
            signed_in(&app, &mut client).await?;
            timing::mark("auth_resume: library claimed");
        }
        outcome
    };
    if outcome == "library" {
        folders_up(&app).await;
        timing::mark("auth_resume: folders up");
    }
    Ok(outcome)
}

async fn resume_session(client: &mut Client) -> UiResult<&'static str> {
    let resumed = match client.resume().await {
        Ok(resumed) => resumed,
        // Offline with a session kept: the library opens on what it holds, as a local-first
        // app should, and the next refresh — on the timer, or when the network is back —
        // signs back in. Apple busy, or asking to wait, is the same: not now, not signed out.
        Err(
            quince_auth::Error::Network(_)
            | quince_auth::Error::Unavailable(_)
            | quince_auth::Error::RateLimited
            | quince_auth::Error::CoolingDown { .. },
        ) if client.has_stored_session() => true,
        Err(e) => return Err(e.to_string()),
    };
    // Back in; or back in but for the code Apple wants before trusting this computer again;
    // or out, with nothing kept to sign in with.
    Ok(if resumed {
        "library"
    } else if client.awaiting_code() {
        "code"
    } else {
        "signin"
    })
}

/// The folders on this computer brought up, once the library is the signed-in account's: they
/// are served by the session, so they are never up without one.
async fn folders_up(app: &tauri::AppHandle) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || mount::resume(&app)).await;
}

/// Emitted when a lapsed session signed itself back in only as far as Apple's code.
const NEEDS_CODE: &str = "quince://needs-code";
/// A session Apple will take back only with the password.
const NEEDS_SIGNIN: &str = "quince://needs-signin";

/// Make sure the session is alive before a refresh uses it, signing back in if it lapsed.
///
/// A window left open past the life of Apple's cookies, or of its session token, had every
/// refresh failing quietly from then on. Checked here, a lapse is mended before it is felt;
/// one that needs a code brings the code screen up instead.
async fn keep_signed_in(app: &tauri::AppHandle, client: &Arc<Mutex<Client>>) {
    // Said once per lapse, not on every pass after it.
    static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // And once per wait at icloud.com, apart from a lapse: one never hides the other.
    static TOLD_ICLOUD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let mut guard = client.lock().await;
    if guard.session().webservices.is_none() {
        return;
    }
    match guard.heartbeat().await {
        // Apple wants a code before it trusts this computer again. None has been asked for:
        // the window says so calmly, and the code is sent once the person opens the code
        // screen from there — never pushed to their phone from a background pass.
        Ok(false) if guard.awaiting_code() => {
            LIVE.store(false, Ordering::SeqCst);
            app.emit(NEEDS_CODE, ()).ok();
            if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                alerts::tell(
                    app,
                    "Sign in again to keep syncing",
                    "Open Skykeep to get a code from Apple. Your copy on this computer is still here.",
                );
            }
        }
        // Apple will not take the session back without the password: every sync would fail
        // quietly from here on. The window says so where it is open, and the desktop once,
        // for someone with only the tray running.
        Ok(false) => {
            LIVE.store(false, Ordering::SeqCst);
            app.emit(NEEDS_SIGNIN, ()).ok();
            if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                alerts::tell(
                    app,
                    "Sign in to keep iCloud up to date",
                    "Apple signed Skykeep out. Your copy on this computer is still here.",
                );
            }
        }
        Ok(true) => {
            TOLD.store(false, std::sync::atomic::Ordering::Relaxed);
            // New iCloud terms waiting at icloud.com (or the account to finish there): asked
            // again each pass, so the whole session opens by itself once the person has agreed
            // — one request every few minutes, and only while they are waiting. The session
            // and password stay as they are: signing in again would meet the same terms.
            if guard.waits_on_icloud_com() {
                let _ = guard.refresh_account().await;
            }
            if guard.waits_on_icloud_com() {
                // The window shows its own page for it; the desktop is told once, for someone
                // with only the tray running. Never "Apple signed you out": it did not.
                if !TOLD_ICLOUD.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    let (title, body) = if guard.standing() == quince_auth::state::Standing::Terms {
                        (
                            "Agree to Apple's new iCloud terms",
                            "Sign in at icloud.com and agree to them. Your copy on this computer is still here.",
                        )
                    } else {
                        (
                            "Finish setting up iCloud",
                            "Sign in at icloud.com to finish. Your copy on this computer is still here.",
                        )
                    };
                    alerts::tell(app, title, body);
                }
            } else {
                TOLD_ICLOUD.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            // Keys that ran out while the approval still holds are attached again before the
            // pass needs them — never a prompt on the person's phone from here.
            drop(guard);
            data_access::check(app, client, data_access::Ask::Background).await;
        }
        // Not reaching Apple says nothing about the session.
        Err(_) => {}
    }
}

/// Whose data the library holds: a hash of Apple's account id, never the id itself.
pub const ACCOUNT_KEY: &str = "account";

/// What of an account lives in the data folder beside the session: its library, and every
/// cache and queue made from it. Moved as one when another account signs in.
const ACCOUNT_FILES: [&str; 13] = [
    "library.db",
    "library.db-wal",
    "library.db-shm",
    "photos",
    "photos-folder-cache",
    "drive-cache",
    "drive-writes",
    "photos-folder-writes",
    "thumbnails",
    "note-files",
    "note-media",
    "faces",
    // A reminder's pictures: one account's, set aside and removed with the rest of it.
    "reminder-media",
];

/// Where the accounts not signed in are kept, each under its own tag.
const SET_ASIDE: &str = "accounts";

/// An account as the library names it.
pub fn account_tag(dsid: &str) -> String {
    use sha2::Digest;
    format!(
        "{:x}",
        sha2::Sha256::digest(format!("quince-account:{dsid}"))
    )
}

/// The library made the signed-in account's, before anything else can use the session: called
/// with the auth lock still held, so no sync, send or upload ever sees one account's session
/// beside another's library.
///
/// Someone else's library here — a sign-out kept a copy, then another Apple ID signed in — is
/// set aside whole under `accounts/`, never deleted and never mixed, and this account's own,
/// if it was set aside before, comes back where it was: its records, its caches, what it had
/// still to send. A library that names no owner but holds an account's records — whose they
/// are, nothing here can say — is set aside the same way, as unclaimed, rather than handed to
/// whoever signs in next; only an empty one is simply claimed.
async fn claim_library(app: &tauri::AppHandle, client: &Client) -> Result<(), String> {
    let Some(dsid) = client.session().dsid.as_deref() else {
        return Ok(());
    };
    let tag = account_tag(dsid);
    let data = app.state::<Data>();
    let mut library = data.0.lock().await;
    let dir = state_dir(app);
    claim_as(&mut library, &dir, &tag, &AppHandover(&data.0)).await
}

/// What goes on around a library changing hands, beyond its files: the folders the last
/// account's session served, the screens' readers of the file, the links to its pictures.
trait Handover {
    /// The Drive and Photos folders let go of.
    async fn release_folders(&self) -> Result<(), String>;
    /// The screens' read-only connections to the library file closed.
    fn forget_readers(&self);
    /// The window's links to the last library's pictures forgotten.
    fn forget_links(&self);
}

/// The running app's [`Handover`].
struct AppHandover<'a>(&'a library_lock::LibraryLock);

impl Handover for AppHandover<'_> {
    async fn release_folders(&self) -> Result<(), String> {
        tauri::async_runtime::spawn_blocking(|| mount::release_within(Duration::from_secs(3)))
            .await
            .map_err(|e| e.to_string())
    }
    fn forget_readers(&self) {
        self.0.forget_readers();
    }
    fn forget_links(&self) {
        photos::forget_links();
    }
}

/// [`claim_library`] for account `tag`, the library at `dir`.
async fn claim_as(
    library: &mut Library,
    dir: &std::path::Path,
    tag: &str,
    around: &impl Handover,
) -> Result<(), String> {
    let known = library.state(ACCOUNT_KEY).map_err(|e| e.to_string())?;
    if known.as_deref() == Some(tag) {
        return Ok(());
    }
    let home = dir.join(SET_ASIDE).join(tag);
    if must_set_aside(library, known.as_deref(), home.exists())? {
        let now = jiff::Timestamp::now().as_second();
        let away = known.unwrap_or_else(|| format!("unclaimed-{now}"));
        // The folders the last account's session served go before its library does: left
        // mounted, they went on answering from the next account's library and session.
        around.release_folders().await?;
        // The screens' readers let go of the file before it moves.
        around.forget_readers();
        switch_library(library, dir, &away, &home)?;
        around.forget_links();
    }
    library
        .set_state(ACCOUNT_KEY, tag)
        .map_err(|e| e.to_string())
}

/// Whether the library here must be set aside before `account` takes it: it is another
/// account's, `account`'s own is waiting to come back, or it names no owner but holds records
/// all the same (a library whose owner was lost) — never given to an account it may not be.
fn must_set_aside(
    library: &Library,
    owner: Option<&str>,
    home_waits: bool,
) -> Result<bool, String> {
    Ok(owner.is_some() || home_waits || library.holds_account().map_err(|e| e.to_string())?)
}

/// One account's library and caches set aside under `away`, and the ones at `home` (if any)
/// brought back in their place. The library is let go of while its files move and opened
/// again after, whatever happened; this computer's settings stay with the computer.
fn switch_library(
    library: &mut Library,
    dir: &std::path::Path,
    away: &str,
    home: &std::path::Path,
) -> Result<(), String> {
    let kept = library.computer_state().map_err(|e| e.to_string())?;
    let empty = !library.holds_account().map_err(|e| e.to_string())?;
    let shelf = dir.join(SET_ASIDE).join(away);
    *library = Library::in_memory().map_err(|e| e.to_string())?;
    let moved = move_account(dir, &shelf).and_then(|()| {
        if home.exists() {
            move_account(home, dir)?;
            std::fs::remove_dir(home)?;
        }
        Ok(())
    });
    *library = Library::open(dir).map_err(|e| e.to_string())?;
    for (key, value) in kept {
        library.set_state(&key, &value).map_err(|e| e.to_string())?;
    }
    moved.map_err(|e| format!("The last account's data could not be set aside: {e}"))?;
    // A library that held nothing — one emptied by a sign-out that kept nothing — is not kept.
    if empty && away.starts_with("unclaimed-") {
        let _ = std::fs::remove_dir_all(&shelf);
    }
    Ok(())
}

/// Every name in `dir` that is one account's: [`ACCOUNT_FILES`], and each damaged library
/// `Library::open_or_recover` set aside (`library.corrupt-<when>.db`, its log and index) —
/// whole copies of the account's records, as much its data as the library itself.
fn account_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = ACCOUNT_FILES.iter().map(|n| (*n).to_string()).collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut damaged: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("library.corrupt-"))
            .collect();
        damaged.sort();
        names.extend(damaged);
    }
    names
}

/// "Remove copy": every record and file of the account gone from this computer, not just its
/// rows. Deleted rows stay in SQLite's free pages and its log until they are written over, so
/// the library is let go of and its file, log and index deleted — with every damaged copy set
/// aside beside it and every cache — and a fresh library opened, this computer's own settings
/// carried over. A file that cannot be deleted (held open elsewhere, on Windows) is said; its
/// records are emptied all the same.
fn remove_account(library: &mut Library, dir: &std::path::Path) -> Result<(), String> {
    let kept = library.computer_state().map_err(|e| e.to_string())?;
    *library = Library::in_memory().map_err(|e| e.to_string())?;
    *SEARCH_READER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let mut failed: Option<(String, std::io::Error)> = None;
    for name in account_names(dir) {
        let path = dir.join(&name);
        let gone = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(e) => Err(e),
        };
        match gone {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                failed.get_or_insert((name, e));
            }
            _ => {}
        }
    }
    *library = Library::open(dir).map_err(|e| e.to_string())?;
    if failed.is_some() {
        library.forget_account().map_err(|e| e.to_string())?;
    }
    for (key, value) in kept {
        library.set_state(&key, &value).map_err(|e| e.to_string())?;
    }
    match failed {
        Some((name, e)) => Err(format!(
            "Signed out, but {name} could not be removed from this computer: {e}"
        )),
        None => Ok(()),
    }
}

/// An account's files moved from one folder to another, all or none: whatever had moved goes
/// back if one cannot, and nothing is moved over something already there.
fn move_account(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let names = account_names(from);
    if let Some(taken) = names.iter().find(|n| to.join(n).exists()) {
        return Err(std::io::Error::other(format!("{taken} is already there")));
    }
    let mut done = Vec::new();
    for name in &names {
        if !from.join(name).exists() {
            continue;
        }
        if let Err(e) = std::fs::rename(from.join(name), to.join(name)) {
            for back in done {
                let _ = std::fs::rename(to.join(back), from.join(back));
            }
            return Err(e);
        }
        done.push(name);
    }
    Ok(())
}

/// Whether the session and the library are the same account's — asked by everything that sends
/// what the library holds to Apple, so that nothing of one account is ever sent with another's
/// session (a switch that failed half-way, a session left from a crash).
pub async fn same_account(app: &tauri::AppHandle) -> bool {
    let session = app
        .state::<Auth>()
        .client
        .lock()
        .await
        .session()
        .dsid
        .as_deref()
        .map(account_tag);
    let owner = app
        .state::<Data>()
        .0
        .lock()
        .await
        .state(ACCOUNT_KEY)
        .ok()
        .flatten();
    session.is_some() && session == owner
}

/// The account the session is signed in to now, as the library records its owner.
async fn account_now(auth: &Auth) -> Option<String> {
    auth.client
        .lock()
        .await
        .session()
        .dsid
        .as_deref()
        .map(account_tag)
}

/// The account a request's parameters ask as (its `dsid`), as the library names accounts.
fn asked_as(params: &[(String, String)]) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k == "dsid")
        .map(|(_, dsid)| account_tag(dsid))
}

/// Whether what was read with one account's session may be stored in this library: only in
/// the library that account owns. A pass after a write read with the session the write was
/// sent with; another Apple ID signing in meanwhile swapped the library under it, and that
/// pass stored the first account's notes, reminders and sync position in the second's. A
/// library that names no owner (signed out with nothing kept, or not yet claimed) is no
/// account's to fill, and a request that names no account is no account's to store.
fn owned_by(library: &quince_core::Library, asked_for: Option<&str>) -> Result<(), String> {
    let owner = library.state(ACCOUNT_KEY).ok().flatten();
    match (owner.as_deref(), asked_for) {
        (Some(owner), Some(asked)) if owner == asked => Ok(()),
        _ => Err(
            "Signed in to another account while this was on its way, so nothing was stored.".into(),
        ),
    }
}

#[cfg(test)]
mod account_write_tests {
    use super::*;

    #[test]
    fn what_one_account_read_is_never_stored_in_anothers_library() {
        let mut lib = Library::in_memory().unwrap();
        let params = |dsid: &str| vec![("dsid".to_string(), dsid.to_string())];
        lib.set_state(ACCOUNT_KEY, &account_tag("111")).unwrap();
        let a = asked_as(&params("111"));
        let b = asked_as(&params("222"));
        assert!(owned_by(&lib, a.as_deref()).is_ok());
        assert!(
            owned_by(&lib, b.as_deref()).is_err(),
            "the other account's read is refused"
        );
        // A library that names no owner is no account's to fill, and a request that names no
        // account stores nothing.
        assert!(owned_by(&Library::in_memory().unwrap(), b.as_deref()).is_err());
        assert!(owned_by(&lib, None).is_err());
    }

    /// A library is handed to the account signing in only when it is that account's or holds
    /// nothing. One that names no owner but holds records — its owner lost — is set aside as
    /// unclaimed, never given to whoever signs in next; an empty one (a Remove Copy, a first
    /// run), only the id it made for itself as it opened, is simply claimed.
    #[test]
    fn a_library_with_no_owner_but_records_is_set_aside() {
        let mut lib = Library::in_memory().unwrap();
        lib.set_state("documents.writer", "W").unwrap();
        lib.set_state("settings.alerts.events", "0").unwrap();
        assert!(
            !must_set_aside(&lib, None, false).unwrap(),
            "empty: claimed"
        );
        assert!(
            must_set_aside(&lib, None, true).unwrap(),
            "the account's own waits to come back"
        );
        assert!(must_set_aside(&lib, Some("other"), false).unwrap());
        lib.outbox_push("reminders", "R1", "done", "{}").unwrap();
        assert!(
            must_set_aside(&lib, None, false).unwrap(),
            "a change waiting, whose nobody can say"
        );
    }
}

/// Whether the library still belongs to `account`, taken when a command began: a result that
/// came back after a sign-out, or another account's sign-in, is never stored over its data.
fn still_owned(library: &quince_core::Library, account: Option<&str>) -> Result<(), String> {
    let owner = library.state(ACCOUNT_KEY).ok().flatten();
    if account.is_some() && owner.as_deref() == account {
        Ok(())
    } else {
        Err("Signed out while this was on its way, so nothing was stored.".into())
    }
}

/// Signed in, then the library made this account's; if it could not be, signed out again with
/// the reason, never left signed in over someone else's data.
async fn signed_in(app: &tauri::AppHandle, client: &mut Client) -> Result<(), String> {
    signed_in_as(app, client, data_access::Ask::Background).await
}

/// [`signed_in`], saying who it was for: the person signing in by hand is an action of theirs,
/// and Advanced Data Protection's approval is asked for with it; a session taken up again at a
/// launch — perhaps at login, the window in the tray — asks Apple nothing about it.
async fn signed_in_as(
    app: &tauri::AppHandle,
    client: &mut Client,
    ask: data_access::Ask,
) -> Result<(), String> {
    if let Err(e) = claim_library(app, client).await {
        let _ = client.sign_out();
        return Err(e);
    }
    LIVE.store(true, Ordering::SeqCst);
    if let Some(auth) = app.try_state::<Auth>() {
        *auth
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionFacts::of(client);
        // Under Advanced Data Protection, Notes, Reminders, Photos and iCloud Drive wait on an
        // approval on the person's devices: asked for when they sign in by hand, read from what
        // is held here otherwise — once the caller has let the session go, which this step
        // takes for itself.
        data_access::forget(app);
        let app = app.clone();
        let client = Arc::clone(&auth.client);
        tauri::async_runtime::spawn(async move {
            data_access::check(&app, &client, ask).await;
        });
    }
    Ok(())
}

/// This process holds a signed-in session, its library claimed: a window opened now (an app's
/// own, from the tray or its launcher) is told so at once. Asking Apple again first, behind the
/// sign-in lock a running sync may hold, kept every app window waiting about five seconds.
static LIVE: AtomicBool = AtomicBool::new(false);

#[tauri::command]
async fn auth_sign_in(
    apple_id: String,
    password: String,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
) -> UiResult<&'static str> {
    // Reject rather than queue. A second attempt would mean a second code sent to the
    // user's devices and another step toward Apple's rate limit.
    if auth.signing_in.swap(true, Ordering::AcqRel) {
        return Err("Already signing in.".into());
    }
    let _guard = InFlight(&auth.signing_in);

    let mut client = auth.client.lock().await;
    match client.sign_in(&apple_id, &password).await {
        Ok(SignInOutcome::Complete) => {
            signed_in_as(&app, &mut client, data_access::Ask::Person).await?;
            drop(client);
            folders_up(&app).await;
            Ok("complete")
        }
        Ok(SignInOutcome::NeedsTwoFactor) => Ok("needs_two_factor"),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn auth_submit_code(
    code: String,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
) -> UiResult<()> {
    let mut client = auth.client.lock().await;
    client
        .submit_two_factor(&code)
        .await
        .map_err(|e| e.to_string())?;
    signed_in_as(&app, &mut client, data_access::Ask::Person).await?;
    drop(client);
    folders_up(&app).await;
    Ok(())
}

/// Where Apple can send a code, and whether it already pushed one.
/// A `trustedDeviceCount` of zero means nothing was sent and waiting is pointless.
#[tauri::command]
async fn auth_two_factor_options(auth: State<'_, Auth>) -> UiResult<TwoFactorOptions> {
    let mut client = auth.client.lock().await;
    client.two_factor_options().await.map_err(|e| e.to_string())
}

/// The code screen opened: Apple is asked for the code now, unless this sign-in already asked
/// (the person typed their password a moment ago). A lapse mended in the background leaves it
/// unasked, so nothing reaches the person's phone until they are here for it.
#[tauri::command]
async fn auth_request_code(auth: State<'_, Auth>) -> UiResult<()> {
    let mut client = auth.client.lock().await;
    client.request_code().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn auth_resend_code(auth: State<'_, Auth>) -> UiResult<()> {
    let mut client = auth.client.lock().await;
    client.resend_to_devices().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn auth_send_sms(phone_id: u64, auth: State<'_, Auth>) -> UiResult<()> {
    let mut client = auth.client.lock().await;
    client.send_sms(phone_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn auth_submit_sms_code(
    phone_id: u64,
    code: String,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
) -> UiResult<()> {
    let mut client = auth.client.lock().await;
    client
        .submit_sms_code(phone_id, &code)
        .await
        .map_err(|e| e.to_string())?;
    signed_in_as(&app, &mut client, data_access::Ask::Person).await?;
    drop(client);
    folders_up(&app).await;
    Ok(())
}

/// What one search found: the apps that found something, a few of each — or, for "Show All",
/// the next rows of one — and the number the window gave the question, so an answer that
/// arrives after a newer question's is known for stale and dropped.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Found {
    seq: u64,
    groups: Vec<quince_core::search::Group>,
}

/// The newest question asked: one still waiting for the reader when a newer one arrives is
/// not run at all, so a burst of keystrokes costs one search, not one per key.
static SEARCH_LATEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The library as search reads it: its own read-only connection, so searching never waits for
/// the lock a sync pass holds over the one that writes. Opened on the first search and again
/// whenever `library.db` is another file than it was (an account's library set aside for
/// another's moves the files under it).
static SEARCH_READER: std::sync::Mutex<Option<SearchReader>> = std::sync::Mutex::new(None);
/// The reader's file, which file that was (see `file_identity`), and the reader.
type SearchReader = (PathBuf, (u64, u64), Library);

/// Runs `read` on the search reader; `None` when there is no reader to be had (the library
/// is being moved, say), and the caller reads through the shared library instead.
fn read_for_search<T>(
    dir: &std::path::Path,
    read: impl FnOnce(&Library) -> quince_core::Result<T>,
) -> Option<Result<T, String>> {
    let file = dir.join("library.db");
    let id = std::fs::metadata(&file).ok().map(|m| file_identity(&m))?;
    let mut reader = SEARCH_READER.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(&*reader, Some((f, i, _)) if *f == file && *i == id) {
        *reader = Library::open_reader(dir).ok().map(|l| (file, id, l));
    }
    let (_, _, library) = reader.as_ref()?;
    let found = read(library).map_err(|e| e.to_string());
    // Where an open file cannot be moved (Windows), it is not kept open between searches: an
    // account's library set aside would otherwise fail to move.
    #[cfg(not(unix))]
    {
        *reader = None;
    }
    Some(found)
}

/// Which file this is, so one moved into its place is told from it.
fn file_identity(m: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    }
    #[cfg(not(unix))]
    {
        let born = m
            .created()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        (born, 0)
    }
}

/// One search across every app's library — notes, reminders, events, contacts, files, photos —
/// the best few of each and how many in all; or, with `only`, the next `each` of one app from
/// its `skip`th. Offline: read from the library alone, on its own connection, off the async
/// runtime. `None` when a newer question has been asked since.
#[tauri::command]
async fn search_everything(
    query: String,
    seq: Option<u64>,
    each: Option<usize>,
    only: Option<String>,
    skip: Option<usize>,
    app: tauri::AppHandle,
    data: State<'_, Data>,
) -> UiResult<Option<Found>> {
    use std::sync::atomic::Ordering::SeqCst;
    let seq = seq.unwrap_or(0);
    SEARCH_LATEST.fetch_max(seq, SeqCst);
    let each = each.unwrap_or(5).clamp(1, 500);
    let skip = skip.unwrap_or(0);
    let dir = state_dir(&app);
    let (q, o) = (query.clone(), only.clone());
    let read = tauri::async_runtime::spawn_blocking(move || {
        if seq < SEARCH_LATEST.load(SeqCst) {
            return Some(Ok(None));
        }
        read_for_search(&dir, |l| l.search(&q, each, o.as_deref(), skip)).map(|r| r.map(Some))
    })
    .await
    .map_err(|e| e.to_string())?;
    let groups = match read {
        Some(found) => found?,
        None => Some(
            data.0
                .lock()
                .await
                .search(&query, each, only.as_deref(), skip)
                .map_err(|e| e.to_string())?,
        ),
    };
    Ok(groups.map(|groups| Found { seq, groups }))
}

/// What of the signed-in account's has not reached Apple yet: asked before a sign-out, so a
/// choice to keep nothing on this computer is made knowing what it loses.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Unsent {
    /// Changes queued for Apple (a reminder ticked offline, say).
    changes: usize,
    /// Files written in the iCloud Drive folder and not yet uploaded.
    files: usize,
    /// Photos added and not yet uploaded.
    photos: usize,
}

#[tauri::command]
async fn account_unsent(app: tauri::AppHandle, data: State<'_, Data>) -> UiResult<Unsent> {
    let (changes, photos) = {
        let library = data.0.lock().await;
        (
            library.outbox_count().map_err(|e| e.to_string())?,
            library
                .unfinished_uploads()
                .map_err(|e| e.to_string())?
                .len(),
        )
    };
    Ok(Unsent {
        changes,
        photos,
        files: mount::unsent_files(&state_dir(&app)),
    })
}

/// Signed out, keeping a copy of the account here or not, as the person chose.
///
/// Kept, everything stays as it was — the library, its caches, what waits to be sent — and is
/// there again when the account signs back in; another account signing in first finds it set
/// aside, not mixed in (see `claim_library`). Not kept, every record and cached file of the
/// account is removed from this computer. Either way nothing is deleted from iCloud.
#[tauri::command]
async fn auth_sign_out(keep: bool, app: tauri::AppHandle, auth: State<'_, Auth>) -> UiResult<()> {
    // The folders go with the session they were served by — before the auth lock is taken,
    // since what they are still answering may be waiting on it.
    LIVE.store(false, Ordering::SeqCst);
    // A notes password typed this session goes with the session.
    notes_cmds::forget_passwords();
    // Gone with the account: on Windows no longer in File Explorer either (mount::forget).
    let leaving = app.clone();
    tauri::async_runtime::spawn_blocking(move || mount::forget(&leaving))
        .await
        .map_err(|e| e.to_string())?;
    let mut client = auth.client.lock().await;
    client.sign_out().map_err(|e| e.to_string())?;
    data_access::forget(&app);
    if !keep {
        let data = app.state::<Data>();
        let mut library = data.0.lock().await;
        // The screens' readers let go of the file before it is deleted.
        data.0.forget_readers();
        let removed = remove_account(&mut library, &state_dir(&app));
        photos::forget_links();
        drop(library);
        drop(client);
        let app = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || mount::forget_thumbnails(&app)).await;
        removed?;
    }
    Ok(())
}

/// Services Apple advertises for this account, for the sidebar.
/// Who is signed in, as Apple names them: for the account in the sidebar and in Settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountInfo {
    name: Option<String>,
    email: Option<String>,
    /// `false` when the account keeps its data from the web: nothing can be read until
    /// the person turns on "Access iCloud Data on the Web".
    web_access_allowed: Option<bool>,
    /// Advanced Data Protection.
    data_protection: Option<bool>,
    /// Apple has new iCloud terms waiting to be agreed to at icloud.com; until then it opens
    /// only part of a web session and nothing can be read (issue #5).
    terms_update_needed: bool,
    /// Apple wants the account finished at icloud.com first.
    repair_needed: bool,
    /// The person's own card, when it has a photo: theirs to show as the account's picture,
    /// served like any contact's.
    photo_contact: Option<String>,
}

#[tauri::command]
async fn account_info(auth: State<'_, Auth>, data: State<'_, Data>) -> UiResult<AccountInfo> {
    // The picture is the person's own card's — the card Contacts marks as theirs.
    let photo_contact = {
        let library = data.0.lock().await;
        library
            .state(quince_icloud::contacts::ME_KEY)
            .ok()
            .flatten()
            .filter(|guid| !guid.is_empty())
            .filter(|guid| library.contact_photo_source(guid).ok().flatten().is_some())
    };
    // Never behind the session's lock: see `session_facts`.
    let s = session_facts(&auth);
    Ok(AccountInfo {
        name: s.name,
        email: s.email,
        web_access_allowed: s.web_access_allowed,
        data_protection: s.data_protection,
        terms_update_needed: s.terms_update_needed,
        repair_needed: s.repair_needed,
        photo_contact,
    })
}

/// Ask Apple for the account again — the person agreed to new iCloud terms at icloud.com and
/// pressed Try Again — then read everything that was waiting on it.
#[tauri::command]
async fn account_refresh(
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<AccountInfo> {
    let waiting = {
        let mut client = auth.client.lock().await;
        client.refresh_account().await.map_err(|e| e.to_string())?;
        *auth
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionFacts::of(&client);
        client.waits_on_icloud_com()
    };
    if !waiting {
        let client = Arc::clone(&auth.client);
        let result = refresh::all(&client, &data.0).await;
        alerts::changed();
        app.emit(SYNCED, told(&result)).ok();
    }
    account_info(auth, data).await
}

/// Settings' switches, as the backend obeys them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    reminder_alerts: bool,
    event_alerts: bool,
}

#[tauri::command]
async fn settings_get(data: State<'_, Data>) -> UiResult<Settings> {
    let library = data.0.lock().await;
    Ok(Settings {
        reminder_alerts: alerts::allowed(&library, alerts::REMINDER_ALERTS),
        event_alerts: alerts::allowed(&library, alerts::EVENT_ALERTS),
    })
}

/// Turn a kind of alert on or off. Only the switches Settings shows can be set.
#[tauri::command]
async fn settings_set(key: String, on: bool, data: State<'_, Data>) -> UiResult<()> {
    let state = match key.as_str() {
        "reminderAlerts" => alerts::REMINDER_ALERTS,
        "eventAlerts" => alerts::EVENT_ALERTS,
        _ => return Err(format!("There is no setting called {key}.")),
    };
    let mut library = data.0.lock().await;
    library
        .set_state(state, if on { "on" } else { "off" })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn account_services(auth: State<'_, Auth>) -> UiResult<Vec<String>> {
    // Apple's list is kept with the session, so it is known before the session is taken up
    // again: never a wait behind the lock a resume or a sync's heartbeat holds.
    Ok(session_facts(&auth).services)
}

/// Keep the session warm.
///
/// Apple resets auth cookies after a long idle gap, so a session left untouched can die well
/// before its nominal life. Roughly hourly contact is what keeps it alive in practice; 45
/// minutes leaves margin without being chatty enough to look like polling.
fn spawn_heartbeat(client: Arc<Mutex<Client>>) {
    tauri::async_runtime::spawn(async move {
        const INTERVAL: Duration = Duration::from_secs(45 * 60);
        // The session that lapsed, by its token: not pinged again — but a new one, signed in
        // since, is. Ended at the first lapse instead, a tray left running for weeks never kept
        // the next sign-in warm.
        let mut lapsed: Option<Option<String>> = None;
        loop {
            tokio::time::sleep(INTERVAL).await;
            let mut guard = client.lock().await;
            let session = guard.session();
            let token = session.session_token.clone();
            if !worth_a_ping(session.webservices.is_some(), &token, lapsed.as_ref()) {
                continue;
            }
            // A lapsed session is expected roughly every two months. The UI discovers it on
            // next use and asks for a code; nothing to do here but stop pinging a dead one.
            // A transient error is not proof of that, so only a clean `false` counts. Each
            // ping also writes the session again if a keyring would not take it before.
            match guard.heartbeat().await {
                Ok(false) => lapsed = Some(token),
                Ok(true) => lapsed = None,
                Err(_) => {}
            }
        }
    });
}

/// Whether the heartbeat pings the session now: signed in, and not the one that lapsed.
fn worth_a_ping(signed_in: bool, token: &Option<String>, lapsed: Option<&Option<String>>) -> bool {
    signed_in && lapsed != Some(token)
}

/// Refresh the calendar window `[from, to]` (both `YYYY-MM-DD`) from Apple.
///
/// The interface does not wait on this to render: it reads the library, which still holds
/// the last good data while a refresh is in flight or failing.
#[tauri::command]
async fn calendar_sync(
    from: String,
    to: String,
    timezone: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<quince_icloud::sync::SyncSummary> {
    // Take what the sync needs and release the auth lock before the network work, so a slow
    // refresh cannot block the heartbeat or a sign-out.
    let account = account_now(&auth).await;
    let (http, root, params) = {
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("calendar")
            .ok_or("Apple hasn't offered a calendar service for this account.")?;
        (client.http().clone(), root, client.service_params())
    };

    // Asked of Apple unlocked; the library is held only to store the answer.
    let _one = CALENDAR_PASS.lock().await;
    let fetched = quince_icloud::sync::fetch_calendar(&http, &root, params, &from, &to, &timezone)
        .await
        .map_err(|e| e.to_string())?;
    let summary = {
        // Stored as one transaction, so the screens read beside it (`lock_for_pass`).
        let mut library = data.0.lock_for_pass().await;
        still_owned(&library, account.as_deref())?;
        // Changes still on their way to Apple stay as made here — stored with the pass, as one.
        // In place, as `refresh::store` does, so the worker's other tasks are handed on.
        tokio::task::block_in_place(|| {
            library.atomically(|library| {
                let summary = quince_icloud::sync::apply_calendar(library, fetched)?;
                outbox::replay(library, outbox::CALENDAR)?;
                Ok::<_, quince_icloud::Error>(summary)
            })
        })
        .map_err(|e| e.to_string())?
    };
    // Apple answered: whatever waits to be sent goes now.
    outbox::kick();
    Ok(summary)
}

/// What the interface sends to create or change an event.
///
/// The guid is what decides which it is: absent means a new event, present means saving
/// changes to one Apple already has.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventDraft {
    guid: Option<String>,
    calendar_guid: String,
    title: String,
    location: Option<String>,
    starts_at: i64,
    ends_at: i64,
    all_day: bool,
    timezone: Option<String>,
    notes: Option<String>,
    url: Option<String>,
    /// Set the alerts to exactly these, minutes before the start (`0` at the time); absent,
    /// the event's alerts are left as Apple holds them.
    #[serde(default)]
    alerts: Option<Vec<i64>>,
    /// For one occurrence of a repeating event: which events the change is for.
    #[serde(default)]
    scope: Option<quince_icloud::calendar::Scope>,
    /// Set the repeat to this — `None` for never — when `repeat_changed`; otherwise the event
    /// keeps the repeat Apple holds.
    #[serde(default)]
    repeat: Option<quince_icloud::calendar::EventRepeat>,
    #[serde(default)]
    repeat_changed: bool,
    /// The event as the form opened it, Apple's tag with it: what this save was made from.
    /// Absent (an older window, or a create), the library's copy at save time stands in.
    #[serde(default)]
    base: Option<Event>,
}

/// Everything an event write needs from the session, with the auth lock released first.
async fn calendar_context(
    auth: &State<'_, Auth>,
) -> Result<(reqwest::Client, String, Vec<(String, String)>), String> {
    let client = auth.client.lock().await;
    let root = client
        .session()
        .service_url("calendar")
        .ok_or("Apple hasn't offered a calendar service for this account.")?;
    Ok((client.http().clone(), root, client.service_params()))
}

/// Create an event, or save changes to one.
///
/// Apple takes the whole event on every write, so an edit sends the event as it should end
/// up rather than a patch. What comes back from Apple is what gets stored.
/// An event's alerts as Apple holds them: minutes before its start, `0` at the time.
#[tauri::command]
async fn event_alerts(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<i64>> {
    let event = {
        let library = data.0.lock().await;
        library.event(&guid).map_err(|e| e.to_string())?
    };
    let Some(event) = event else {
        return Ok(Vec::new());
    };
    let (http, root, params) = calendar_context(&auth).await?;
    quince_icloud::calendar::CalendarClient::new(&http, &root, params)
        .alerts(&event)
        .await
        .map_err(|e| e.to_string())
}

/// An event's guests and their answers, as Apple holds them, the organizer first. Read only.
#[tauri::command]
async fn event_guests(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<quince_icloud::calendar::Guest>> {
    let event = {
        let library = data.0.lock().await;
        library.event(&guid).map_err(|e| e.to_string())?
    };
    let Some(event) = event else {
        return Ok(Vec::new());
    };
    let (http, root, params) = calendar_context(&auth).await?;
    quince_icloud::calendar::CalendarClient::new(&http, &root, params)
        .guests(&event)
        .await
        .map_err(|e| e.to_string())
}

/// How an event repeats, as Apple holds it.
#[tauri::command]
async fn event_repeat(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Option<quince_icloud::calendar::EventRepeat>> {
    let event = {
        let library = data.0.lock().await;
        library.event(&guid).map_err(|e| e.to_string())?
    };
    let Some(event) = event else { return Ok(None) };
    let (http, root, params) = calendar_context(&auth).await?;
    quince_icloud::calendar::CalendarClient::new(&http, &root, params)
        .repeat_of(&event)
        .await
        .map_err(|e| e.to_string())
}

/// Create an event, or save changes to one: in the library at once, and sent behind
/// (outbox.rs) — where a change made on another device since is merged, as Calendar settles
/// two edits, one occurrence of a repeating event included. Offline, it waits, across a
/// restart. Answers the event's id, chosen here for a new one.
#[tauri::command]
async fn event_save(draft: EventDraft, data: State<'_, Data>) -> UiResult<String> {
    let title = draft.title.trim();
    if title.is_empty() {
        return Err("An event needs a title.".into());
    }
    if draft.ends_at < draft.starts_at {
        return Err("An event can't end before it starts.".into());
    }
    // Apple's guids are uppercase UUIDs; a new event needs one before it can be addressed.
    let was_saved = draft.guid.is_some();
    let guid = draft
        .guid
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string().to_uppercase());
    {
        let mut library = data.0.lock().await;
        let existing = library.event(&guid).map_err(|e| e.to_string())?;
        // An event opened as one already saved, and no longer here: deleted on another device
        // since. Saved, it came back as new on every device, without its alerts or guests.
        if was_saved && existing.is_none() && !quince_icloud::calendar::is_occurrence(&guid) {
            return Err("This event was deleted on another device, so it wasn’t saved.".into());
        }
        // Made from the event as the form showed it, not as the library holds it now: a pass
        // that landed while the form was open brought another device's change, and a base
        // taken now would make this save overwrite it.
        let opened = draft.base.filter(|_| was_saved);
        let base = outbox::save_base(&guid, opened, existing.clone());
        let event = Event {
            guid: guid.clone(),
            calendar_guid: draft.calendar_guid,
            title: title.to_string(),
            location: draft.location.filter(|s| !s.trim().is_empty()),
            starts_at: draft.starts_at,
            ends_at: draft.ends_at,
            all_day: draft.all_day,
            timezone: draft.timezone,
            url: draft.url.filter(|s| !s.trim().is_empty()),
            notes: draft.notes.filter(|s| !s.trim().is_empty()),
            etag: base.as_ref().and_then(|e| e.etag.clone()),
        };
        outbox::record_event_save(
            &mut library,
            outbox::EventSave {
                event,
                create: existing.is_none() && !was_saved,
                base,
                alerts: draft.alerts,
                repeat_changed: draft.repeat_changed,
                repeat: draft.repeat,
                scope: draft.scope,
            },
        )
        .map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(guid)
}

/// Delete an event — for one of a repeating series, with its scope said. Gone here at once
/// (the whole series, or this day and those after, as asked), deleted at Apple behind.
#[tauri::command]
async fn event_delete(
    guid: String,
    scope: Option<quince_icloud::calendar::Scope>,
    data: State<'_, Data>,
) -> UiResult<()> {
    if quince_icloud::calendar::is_occurrence(&guid) && scope.is_none() {
        return Err(
            "This is one occurrence of a repeating event. Say whether to delete this event alone."
                .into(),
        );
    }
    {
        let mut library = data.0.lock().await;
        let event = library
            .event(&guid)
            .map_err(|e| e.to_string())?
            .ok_or("That event is no longer in the local library.")?;
        outbox::record_event_delete(&mut library, &event, scope).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(())
}

/// Events overlapping `[from, to]`, as unix seconds. Read straight from the library.
#[tauri::command]
async fn calendar_events(
    from: i64,
    to: i64,
    data: State<'_, Data>,
    occasions: State<'_, Occasions>,
    feeds_state: State<'_, Feeds>,
) -> UiResult<Vec<Event>> {
    let (mut events, people) = data
        .0
        .read(|library| {
            Ok((
                library.events_between(from, to)?,
                // Only who has a birthday, and their names: every card built whole was most of
                // the cost of reading a month.
                library.contacts_with_birthdays()?,
            ))
        })
        .await
        .map_err(|e| e.to_string())?;
    // Birthdays and holidays, as a phone draws them: neither is an iCloud calendar.
    events.extend(quince_icloud::occasions::birthdays(&people, from, to));
    events.extend(quince_icloud::occasions::holidays(
        &occasions.feed.lock().await.1,
        from,
        to,
    ));
    events.extend(subscribed_events(&feeds_state, from, to).await);
    events.sort_by(|a, b| {
        a.starts_at
            .cmp(&b.starts_at)
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(events)
}

/// Events holding every word of `query`, soonest first — and birthdays and holidays in the
/// year either side of now, which are found the same way.
#[tauri::command]
async fn calendar_search(
    query: String,
    data: State<'_, Data>,
    occasions: State<'_, Occasions>,
    feeds_state: State<'_, Feeds>,
) -> UiResult<Vec<Event>> {
    let words = quince_core::fold::words(&query);
    let (mut found, people) = {
        let library = data.0.lock().await;
        (
            library.search_events(&query).map_err(|e| e.to_string())?,
            library
                .contacts_with_birthdays()
                .map_err(|e| e.to_string())?,
        )
    };
    if !words.is_empty() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let (from, to) = (now - 366 * 86_400, now + 366 * 86_400);
        let feed = occasions.feed.lock().await;
        let subscribed = subscribed_events(&feeds_state, from, to).await;
        let extra = quince_icloud::occasions::birthdays(&people, from, to)
            .into_iter()
            .chain(quince_icloud::occasions::holidays(&feed.1, from, to))
            .chain(subscribed)
            .filter(|e| quince_core::fold::holds_all(&quince_core::fold::fold(&e.title), &words));
        found.extend(extra);
        found.sort_by_key(|e| e.starts_at);
    }
    Ok(found)
}

/// The holiday feed for this machine's region, and its name, kept beside the library.
struct Occasions {
    feed: Mutex<(Option<String>, Vec<quince_icloud::occasions::Holiday>)>,
}

/// How long a copy of the holiday feed is used before asking Apple for a fresh one.
const HOLIDAYS_KEPT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Keep the holiday feed: read the copy on disk at once, and fetch a fresh one when that copy
/// is a week old or missing — then once a day after, in case it ages while Quince runs.
fn spawn_holidays(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let path = state_dir(&app).join("holidays.ics");
        if let Ok(text) = std::fs::read_to_string(&path) {
            *app.state::<Occasions>().feed.lock().await =
                quince_icloud::occasions::parse_feed(&text);
        }
        let Ok(http) = quince_auth::apple_only().build() else {
            return;
        };
        loop {
            let stale = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map_or(true, |t| {
                    t.elapsed().map_or(true, |age| age > HOLIDAYS_KEPT)
                });
            if stale {
                if let Some((region, lang)) = quince_icloud::occasions::this_machines_feed() {
                    if let Ok(text) =
                        quince_icloud::occasions::fetch_feed(&http, &region, &lang).await
                    {
                        std::fs::write(&path, &text).ok();
                        *app.state::<Occasions>().feed.lock().await =
                            quince_icloud::occasions::parse_feed(&text);
                        // Holidays are the calendar's; no other screen reads them.
                        app.emit(
                            SYNCED,
                            refresh::status_changed(&app.state::<Data>().0, &["calendar"]).await,
                        )
                        .ok();
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}

/* ---- calendars subscribed to by address ----------------------------------------------- */

use quince_icloud::calendar_feeds::{self as feeds, Subscription};

/// Calendars subscribed to by address, each with its file as last read. Kept in
/// `subscriptions.json` beside the library, each file in `subscriptions/<id>.ics`.
struct Feeds {
    subs: Mutex<Vec<(Subscription, Option<feeds::Feed>)>>,
}

/// Apple's calendar colours, for a subscription whose file names none (icloud.com's order).
const FEED_COLOURS: [&str; 7] = [
    "#ff9500", "#34aadc", "#65db39", "#cc73e1", "#ffcc00", "#a2845e", "#ff2d55",
];

fn feeds_dir(app: &tauri::AppHandle) -> PathBuf {
    state_dir(app).join("subscriptions")
}

fn save_subscriptions(
    app: &tauri::AppHandle,
    subs: &[(Subscription, Option<feeds::Feed>)],
) -> UiResult<()> {
    let list: Vec<&Subscription> = subs.iter().map(|(s, _)| s).collect();
    let text = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    let path = state_dir(app).join("subscriptions.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// What Skykeep can reach, for Settings: Apple's domains as the network lock holds them
/// (`quince_auth::reachable_domains`, the list the resolver checks — Apple's regional ones too
/// only for an account kept there), the host of each calendar
/// subscribed to, which only its own client can reach, and GitHub's release servers, which
/// only the updater's client can reach (`quince_auth::releases::RELEASE_HOSTS`) — listed only
/// for a copy that updates itself, since no other ever contacts them.
#[derive(serde::Serialize)]
struct Reach {
    apple: Vec<&'static str>,
    subscribed: Vec<String>,
    updates: Vec<&'static str>,
}

#[tauri::command]
async fn network_reach(app: tauri::AppHandle, feeds: State<'_, Feeds>) -> UiResult<Reach> {
    let mut subscribed: Vec<String> = feeds
        .subs
        .lock()
        .await
        .iter()
        .filter_map(|(s, _)| feeds::host_of(&s.url))
        .collect();
    subscribed.sort();
    subscribed.dedup();
    Ok(Reach {
        apple: quince_auth::reachable_domains(),
        subscribed,
        updates: if updates::this_install(&app) == updates::Install::Here {
            quince_auth::releases::RELEASE_HOSTS.to_vec()
        } else {
            Vec::new()
        },
    })
}

/// The `https://` form of a plain `http://` feed address, to be tried first; `None` for one
/// already secure.
fn secure_feed_url(url: &str) -> Option<String> {
    let mut u = reqwest::Url::parse(url).ok()?;
    if u.scheme() != "http" {
        return None;
    }
    // A port written for http is http's; the secure one is the scheme's own.
    if u.port().is_some() {
        u.set_port(None).ok()?;
    }
    u.set_scheme("https").ok()?;
    Some(u.to_string())
}

/// Fetch one subscription's file with the client that reaches its host and nothing else.
async fn fetch_subscription(url: &str) -> UiResult<(String, feeds::Feed)> {
    let host = feeds::host_of(url).ok_or("That address has no server in it.")?;
    let http = quince_auth::listed_only(&[host])
        .user_agent(concat!("Skykeep/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let text = feeds::fetch(&http, url).await.map_err(|e| e.to_string())?;
    let here = jiff::tz::TimeZone::system();
    let feed = feeds::parse(&text, &here).map_err(|e| e.to_string())?;
    Ok((text, feed))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Read the subscriptions kept on disk, then fetch each when it is due — never before the
/// person has subscribed to something, so a new install reaches no host but Apple's.
fn spawn_subscriptions(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let here = jiff::tz::TimeZone::system();
        let kept: Vec<Subscription> =
            std::fs::read_to_string(state_dir(&app).join("subscriptions.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        {
            let state = app.state::<Feeds>();
            let mut subs = state.subs.lock().await;
            *subs = kept
                .into_iter()
                .map(|s| {
                    let feed =
                        std::fs::read_to_string(feeds_dir(&app).join(format!("{}.ics", s.id)))
                            .ok()
                            .and_then(|t| feeds::parse(&t, &here).ok());
                    (s, feed)
                })
                .collect();
        }
        // The subscribed calendars are the calendar's alone to read again.
        app.emit(
            SYNCED,
            refresh::status_changed(&app.state::<Data>().0, &["calendar"]).await,
        )
        .ok();
        loop {
            let due: Vec<Subscription> = app
                .state::<Feeds>()
                .subs
                .lock()
                .await
                .iter()
                .filter(|(s, _)| s.due(now_unix()))
                .map(|(s, _)| s.clone())
                .collect();
            for s in due {
                refresh_subscription(&app, &s.id).await.ok();
            }
            tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        }
    });
}

/// Fetch one subscription again, keeping the copy it had when the fetch does not land.
async fn refresh_subscription(app: &tauri::AppHandle, id: &str) -> UiResult<Subscription> {
    let state = app.state::<Feeds>();
    let url = {
        let subs = state.subs.lock().await;
        subs.iter()
            .find(|(s, _)| s.id == id)
            .map(|(s, _)| s.url.clone())
            .ok_or("That subscription is no longer here.")?
    };
    let fetched = fetch_subscription(&url).await;
    let mut subs = state.subs.lock().await;
    let Some(entry) = subs.iter_mut().find(|(s, _)| s.id == id) else {
        return Err("That subscription is no longer here.".into());
    };
    let outcome = match fetched {
        Ok((text, feed)) => {
            std::fs::create_dir_all(feeds_dir(app)).ok();
            std::fs::write(feeds_dir(app).join(format!("{id}.ics")), text).ok();
            entry.0.fetched_at = Some(now_unix());
            entry.0.problem = None;
            entry.1 = Some(feed);
            Ok(entry.0.clone())
        }
        Err(e) => {
            // Tried, so not tried again until the interval comes round; the old copy stays.
            entry.0.fetched_at = Some(now_unix());
            entry.0.problem = Some(e.clone());
            Err(e)
        }
    };
    save_subscriptions(app, &subs)?;
    drop(subs);
    app.emit(
        SYNCED,
        refresh::status_changed(&app.state::<Data>().0, &["calendar"]).await,
    )
    .ok();
    outcome
}

/// The subscriptions, as the Calendar sidebar lists them.
#[tauri::command]
async fn calendar_subscriptions(feeds: State<'_, Feeds>) -> UiResult<Vec<Subscription>> {
    Ok(feeds
        .subs
        .lock()
        .await
        .iter()
        .map(|(s, _)| s.clone())
        .collect())
}

/// New Calendar Subscription: the address fetched once, here and now, so a wrong one is said
/// at once; its name and colour from the file when it gives them.
#[tauri::command]
async fn calendar_subscribe(
    url: String,
    app: tauri::AppHandle,
    feeds_state: State<'_, Feeds>,
) -> UiResult<Subscription> {
    let url = feeds::normalise_url(&url).map_err(|e| e.to_string())?;
    // A public calendar is out on the internet; an address on this computer or the local
    // network is refused here, said plainly (the client refuses it besides, and a name that
    // leads there: quince_auth::listed_only).
    if let Ok(ip) = url
        .host_str()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
    {
        if !quince_auth::is_public_ip(ip) {
            return Err(
                "That address is on this computer or your local network, not a public calendar."
                    .into(),
            );
        }
    }
    let url = url.to_string();
    // Over https where the server offers it: a plain http feed can be changed on its way by
    // anyone on the network, events and their links with it.
    let secure = secure_feed_url(&url);
    if feeds_state
        .subs
        .lock()
        .await
        .iter()
        .any(|(s, _)| s.url == url || secure.as_ref() == Some(&s.url))
    {
        return Err("You’re already subscribed to that calendar.".into());
    }
    let secured = match &secure {
        Some(s) => fetch_subscription(s).await.ok().map(|got| (s.clone(), got)),
        None => None,
    };
    let (url, (text, feed)) = match secured {
        Some(got) => got,
        None => {
            let got = fetch_subscription(&url).await?;
            (url, got)
        }
    };
    let mut subs = feeds_state.subs.lock().await;
    let taken: Vec<String> = subs.iter().map(|(s, _)| s.colour.clone()).collect();
    let colour = feed.colour.clone().unwrap_or_else(|| {
        FEED_COLOURS
            .iter()
            .find(|c| !taken.iter().any(|t| t == *c))
            .unwrap_or(&FEED_COLOURS[subs.len() % FEED_COLOURS.len()])
            .to_string()
    });
    let sub = Subscription {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        title: feed
            .name
            .clone()
            .or_else(|| feeds::host_of(&url))
            .unwrap_or_else(|| "Subscription".into()),
        url,
        colour,
        // As often as the publisher asks, at least hourly; daily when it does not say.
        refresh_minutes: feed.refresh_minutes.map_or(24 * 60, |m| m.max(60)),
        fetched_at: Some(now_unix()),
        problem: None,
    };
    std::fs::create_dir_all(feeds_dir(&app)).map_err(|e| e.to_string())?;
    std::fs::write(feeds_dir(&app).join(format!("{}.ics", sub.id)), text)
        .map_err(|e| e.to_string())?;
    subs.push((sub.clone(), Some(feed)));
    save_subscriptions(&app, &subs)?;
    Ok(sub)
}

/// Rename, recolour or change how often a subscription is fetched.
#[tauri::command]
async fn calendar_subscription_update(
    id: String,
    title: Option<String>,
    colour: Option<String>,
    refresh_minutes: Option<u32>,
    app: tauri::AppHandle,
    feeds_state: State<'_, Feeds>,
) -> UiResult<()> {
    let mut subs = feeds_state.subs.lock().await;
    let (s, _) = subs
        .iter_mut()
        .find(|(s, _)| s.id == id)
        .ok_or("That subscription is no longer here.")?;
    if let Some(t) = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        s.title = t;
    }
    if let Some(c) = colour.filter(|c| c.len() == 7 && c.starts_with('#')) {
        s.colour = c.to_ascii_lowercase();
    }
    if let Some(m) = refresh_minutes {
        s.refresh_minutes = m;
    }
    save_subscriptions(&app, &subs)
}

/// Unsubscribe: the calendar and its copy leave this computer; nothing was ever the account's.
#[tauri::command]
async fn calendar_unsubscribe(
    id: String,
    app: tauri::AppHandle,
    feeds_state: State<'_, Feeds>,
) -> UiResult<()> {
    let mut subs = feeds_state.subs.lock().await;
    subs.retain(|(s, _)| s.id != id);
    std::fs::remove_file(feeds_dir(&app).join(format!("{id}.ics"))).ok();
    save_subscriptions(&app, &subs)
}

/// Fetch a subscription now, as Refresh Calendars does.
#[tauri::command]
async fn calendar_subscription_refresh(
    id: String,
    app: tauri::AppHandle,
) -> UiResult<Subscription> {
    refresh_subscription(&app, &id).await
}

/// Every subscription's events in `[from, to)`.
async fn subscribed_events(feeds_state: &Feeds, from: i64, to: i64) -> Vec<Event> {
    feeds_state
        .subs
        .lock()
        .await
        .iter()
        .filter_map(|(s, f)| {
            f.as_ref()
                .map(|f| feeds::events(f, &s.calendar_guid(), from, to))
        })
        .flatten()
        .collect()
}

/// After a calendar was made, changed or removed: Apple's own list of calendars, kept here,
/// so the sidebar shows what iCloud now holds rather than what was asked for.
async fn calendars_from_apple(
    client: &quince_icloud::calendar::CalendarClient<'_>,
    data: &Data,
) -> UiResult<()> {
    let today = jiff::Zoned::now().date().to_string();
    let snap = client
        .startup(&today, &today, "UTC")
        .await
        .map_err(|e| e.to_string())?;
    if snap.calendars.is_empty() {
        // An empty answer is never taken as "every calendar is gone".
        return Ok(());
    }
    data.0
        .lock()
        .await
        .put_calendars(&snap.calendars)
        .map_err(|e| e.to_string())
}

/// New Calendar, as on icloud.com: Apple's next unused colour, a name no other calendar has.
#[tauri::command]
async fn calendar_create(
    title: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<String> {
    let _one = CALENDAR_PASS.lock().await;
    let (http, root, params) = calendar_context(&auth).await?;
    let client = quince_icloud::calendar::CalendarClient::new(&http, &root, params);
    let existing = data.0.lock().await.calendars().map_err(|e| e.to_string())?;
    let guid = client
        .create_calendar(&title, &existing)
        .await
        .map_err(|e| e.to_string())?;
    calendars_from_apple(&client, &data).await?;
    Ok(guid)
}

/// Rename or recolour a calendar.
#[tauri::command]
async fn calendar_update(
    guid: String,
    title: Option<String>,
    colour: Option<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let _one = CALENDAR_PASS.lock().await;
    let (http, root, params) = calendar_context(&auth).await?;
    let client = quince_icloud::calendar::CalendarClient::new(&http, &root, params);
    let existing = data.0.lock().await.calendars().map_err(|e| e.to_string())?;
    client
        .update_calendar(&guid, title.as_deref(), colour.as_deref(), &existing)
        .await
        .map_err(|e| e.to_string())?;
    calendars_from_apple(&client, &data).await
}

/// Delete a calendar and its events, once the person has confirmed it in the window.
#[tauri::command]
async fn calendar_delete(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let _one = CALENDAR_PASS.lock().await;
    let (http, root, params) = calendar_context(&auth).await?;
    let client = quince_icloud::calendar::CalendarClient::new(&http, &root, params);
    let calendar = data
        .0
        .lock()
        .await
        .calendars()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|c| c.guid == guid)
        .ok_or("That calendar is no longer here.")?;
    client
        .delete_calendar(&calendar)
        .await
        .map_err(|e| e.to_string())?;
    calendars_from_apple(&client, &data).await
}

#[tauri::command]
async fn calendar_list(
    data: State<'_, Data>,
    occasions: State<'_, Occasions>,
    feeds_state: State<'_, Feeds>,
) -> UiResult<Vec<Calendar>> {
    // The library is let go before the holiday feed and subscriptions are waited for: held
    // across them, every other screen's read queued behind those locks too.
    let (mut calendars, any_birthdays) = data
        .0
        .read(|library| Ok((library.calendars()?, library.any_birthday()?)))
        .await
        .map_err(|e| e.to_string())?;
    let holidays = {
        let feed = occasions.feed.lock().await;
        (!feed.1.is_empty()).then(|| feed.0.clone().unwrap_or_else(|| "Holidays".into()))
    };
    calendars.extend(quince_icloud::occasions::calendars(
        holidays.as_deref(),
        any_birthdays,
    ));
    calendars.extend(
        feeds_state
            .subs
            .lock()
            .await
            .iter()
            .zip(0..)
            .map(|((s, _), i)| s.calendar(i)),
    );
    Ok(calendars)
}

/// What every entry in the Reminders sidebar is holding.
#[tauri::command]
async fn reminder_counts(
    until: i64,
    data: State<'_, Data>,
) -> UiResult<quince_core::reminders::ReminderCounts> {
    data.0
        .read(|library| library.reminder_counts(until))
        .await
        .map_err(|e| e.to_string())
}

/// Reminders still to do holding every word of `query`, across every enabled list.
#[tauri::command]
async fn reminders_search(
    query: String,
    app: tauri::AppHandle,
    data: State<'_, Data>,
) -> UiResult<Vec<Reminder>> {
    // On the search reader, so a sync pass holding the library does not hold the list up.
    let (dir, q) = (state_dir(&app), query.clone());
    let read = tauri::async_runtime::spawn_blocking(move || {
        read_for_search(&dir, |l| l.search_reminders(&q))
    })
    .await
    .map_err(|e| e.to_string())?;
    match read {
        Some(found) => found,
        None => data
            .0
            .lock()
            .await
            .search_reminders(&query)
            .map_err(|e| e.to_string()),
    }
}

/// The headings inside one reminders list, in the order they were made.
#[tauri::command]
async fn reminder_sections(
    list_guid: String,
    data: State<'_, Data>,
) -> UiResult<Vec<quince_core::reminders::ReminderSection>> {
    data.0
        .read(|library| library.reminder_sections(&list_guid))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reminder_lists(data: State<'_, Data>) -> UiResult<Vec<ReminderList>> {
    data.0
        .read(Library::reminder_lists)
        .await
        .map_err(|e| e.to_string())
}

/// The folders lists are filed under.
#[tauri::command]
async fn reminder_groups(
    data: State<'_, Data>,
) -> UiResult<Vec<quince_core::reminders::ReminderGroup>> {
    data.0
        .read(Library::reminder_groups)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reminders_in(
    list_guid: String,
    include_done: bool,
    data: State<'_, Data>,
) -> UiResult<Vec<Reminder>> {
    data.0
        .read(|library| library.reminders_in(&list_guid, include_done))
        .await
        .map_err(|e| e.to_string())
}

/// Tick a reminder off, or put it back.
///
/// Writes to Apple first and only then refreshes locally, so the interface never shows a
/// state Apple has not accepted.
#[tauri::command]
async fn reminder_set_done(
    guid: String,
    list_guid: String,
    done: bool,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    set_done(&auth, &data, &guid, &list_guid, done).await
}

/// What the window is told when a reminder it acted on is not in the library at all: deleted
/// on another device and synced away, or still a stand-in for one being added. Not a sentence
/// for a person — the window answers it by quietly reading the list again (RemindersView).
pub const REMINDER_GONE: &str = "reminder-gone";

/// The stored reminder a write is made against, found by its id alone.
///
/// The window also sends the list it saw the reminder in, and that used to be required to
/// match. It goes stale without anything being wrong: moved to another list on the phone and
/// synced in, or moved here with a tick queued behind the move, the reminder was still there
/// and still the one meant — and the write was refused with a red "no longer in the local
/// library" over a list that plainly showed it. A record id belongs to one reminder whatever
/// list holds it, and the write goes to the list the record is in now.
fn reminder_to_write(
    library: &Library,
    guid: &str,
) -> Result<quince_core::reminders::Reminder, String> {
    library
        .reminder(guid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| REMINDER_GONE.to_string())
}

#[cfg(test)]
mod reminder_write_tests {
    use super::*;
    use quince_core::reminders::{Reminder, ReminderList};

    fn library() -> Library {
        let mut lib = Library::in_memory().unwrap();
        let list = |g: &str| ReminderList {
            guid: g.into(),
            title: g.into(),
            enabled: true,
            ..Default::default()
        };
        lib.put_reminder_lists(&[list("A"), list("B")]).unwrap();
        lib.put_reminders(&[Reminder {
            guid: "r".into(),
            list_guid: "A".into(),
            title: "r".into(),
            ..Default::default()
        }])
        .unwrap();
        lib
    }

    #[test]
    fn a_reminder_is_found_by_its_id_whatever_list_holds_it() {
        let mut lib = library();
        assert_eq!(reminder_to_write(&lib, "r").unwrap().list_guid, "A");
        // Moved on the phone and synced in while the window still showed it under A.
        lib.put_reminders(&[Reminder {
            guid: "r".into(),
            list_guid: "B".into(),
            title: "r".into(),
            ..Default::default()
        }])
        .unwrap();
        let found = reminder_to_write(&lib, "r").unwrap();
        assert_eq!(
            found.list_guid, "B",
            "the write goes where the record is now"
        );
    }

    #[test]
    fn one_truly_gone_is_reported_for_a_quiet_reread_not_as_a_sentence() {
        let mut lib = library();
        lib.forget_reminders(&["r".into()], &[]).unwrap();
        assert_eq!(reminder_to_write(&lib, "r").unwrap_err(), REMINDER_GONE);
        // A stand-in for a reminder still being added is not in the library either.
        assert_eq!(
            reminder_to_write(&lib, "pending:1").unwrap_err(),
            REMINDER_GONE
        );
    }
}

/// Tick a reminder off or put it back — from the window, or from an alert's button.
///
/// In the library at once and queued for Apple (see outbox.rs): the window never waits on the
/// network. A repeating one ticked moves on to its next date here at once; Apple, sent the
/// tick behind, keeps the day's history and moves its own copy on — once.
async fn set_done(
    _auth: &State<'_, Auth>,
    data: &State<'_, Data>,
    guid: &str,
    // Where the caller saw it; the record says where it is now (reminder_to_write).
    _list_guid: &str,
    done: bool,
) -> UiResult<()> {
    let (reminder, last) = {
        let library = data.0.lock().await;
        let reminder = reminder_to_write(&library, guid)?;
        // The last tick of a repeat that ends: completed, as the phone completes it, rather
        // than moved past its End Repeat date.
        let end = library
            .reminder_repeat_end(guid)
            .map_err(|e| e.to_string())?;
        let days = library
            .reminder_repeat_days(guid)
            .map_err(|e| e.to_string())?;
        let last =
            done && reminder.tick_ends_series_on(end, &days, jiff::Timestamp::now().as_second());
        (reminder, last)
    };
    if last || !(done && reminder.repeats()) {
        let now = jiff::Timestamp::now().as_second();
        let payload = serde_json::to_string(&outbox::Done {
            done,
            at: now,
            base: quince_icloud::reminders::TickBase::of(&reminder),
        })
        .map_err(|e| e.to_string())?;
        {
            // Shown and queued together, or neither: a tick shown with nothing queued would
            // be put back by the next pass.
            let mut library = data.0.lock().await;
            library
                .atomically(|l| {
                    l.set_reminder_completed(guid, done.then_some(now))?;
                    l.outbox_push("reminders", guid, "done", &payload)
                })
                .map_err(|e| e.to_string())?;
        }
        outbox::kick();
        alerts::changed();
        return Ok(());
    }
    {
        let mut library = data.0.lock().await;
        outbox::record_reminder_advance(
            &mut library,
            &reminder,
            jiff::Timestamp::now().as_second(),
        )
        .map_err(|e| e.to_string())?;
    }
    outbox::kick();
    alerts::changed();
    Ok(())
}

/// Set or clear how often a reminder comes back.
///
/// Kept apart from the ordinary edit because a repeat is not a field: it is its own record
/// that has to be written, linked and retired in the right order.
#[tauri::command]
async fn reminder_set_repeat(
    guid: String,
    // The list the window saw it in: a hint that can be stale, never checked (reminder_to_write).
    #[allow(unused_variables)] list_guid: String,
    repeat: Option<quince_icloud::reminders::Repeat>,
    // End Repeat On Date, in seconds; absent repeats for ever.
    end_at: Option<i64>,
    // A weekly repeat's weekdays, 1 Sunday … 7 Saturday; absent falls on the due date's day.
    days_of_week: Option<Vec<i64>>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;

    let reminder = {
        let library = data.0.lock().await;
        reminder_to_write(&library, &guid)?
    };

    quince_icloud::reminders::set_repeat_until_cloudkit(
        &http,
        &root,
        params.clone(),
        &reminder,
        repeat,
        end_at,
        days_of_week.as_deref().unwrap_or(&[]),
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

/// Dated reminders inside a window, for the calendar grid.
#[tauri::command]
async fn reminders_due_between(
    from: i64,
    to: i64,
    data: State<'_, Data>,
) -> UiResult<Vec<Reminder>> {
    data.0
        .read(|library| library.reminders_due_between(from, to))
        .await
        .map_err(|e| e.to_string())
}

/// Add a reminder to a list.
///
/// Writes to Apple first, then refreshes locally, so nothing appears on screen that Apple
/// has not accepted.
#[tauri::command]
async fn reminder_create(
    list_guid: String,
    title: String,
    // What the view it was typed into implies, as on the phone: added in Today it is due
    // today, in Flagged it is flagged. Without it the new reminder belonged to neither, and
    // vanished from the view it was typed into the moment it was made.
    due_at: Option<i64>,
    all_day: Option<bool>,
    flagged: Option<bool>,
    data: State<'_, Data>,
) -> UiResult<String> {
    // Said now, not when the queue reaches Apple: nothing to send would be refused there.
    if title.trim().is_empty() {
        return Err("A reminder needs a title.".into());
    }
    if list_guid.trim().is_empty() {
        return Err("A reminder has to go in a list, and this account has none yet.".into());
    }
    // In the library at once under a name chosen here, and queued for Apple (outbox.rs): made
    // offline, it is there now and reaches Apple when Apple can be reached — made once.
    let guid = {
        let mut library = data.0.lock().await;
        outbox::record_reminder_create(
            &mut library,
            &outbox::Create {
                list_guid,
                title,
                due_at,
                all_day,
                flagged,
            },
        )
        .map_err(|e| e.to_string())?
    };
    outbox::kick();
    alerts::changed();
    Ok(guid)
}

#[tauri::command]
async fn reminder_delete(
    guid: String,
    // The list the window saw it in: a hint that can be stale, never checked (reminder_to_write).
    #[allow(unused_variables)] list_guid: String,
    data: State<'_, Data>,
) -> UiResult<()> {
    // Gone from the library at once, its subtasks with it, as on the phone; queued for Apple.
    {
        let mut library = data.0.lock().await;
        let reminder = reminder_to_write(&library, &guid)?;
        outbox::record_reminder_delete(&mut library, &reminder).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    alerts::changed();
    Ok(())
}

/// Apply an edit — title, notes, due date, flag, completion — in one write.
#[tauri::command]
async fn reminder_edit(
    guid: String,
    // The list the window saw it in: a hint that can be stale, never checked (reminder_to_write).
    #[allow(unused_variables)] list_guid: String,
    edit: quince_icloud::reminders::ReminderEdit,
    data: State<'_, Data>,
) -> UiResult<()> {
    if edit.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
        return Err("A reminder needs a title.".into());
    }
    // In the library at once and queued for Apple (outbox.rs). Sent, it is one write of the
    // fields changed, its subtasks follow a move, and a date carries its alert, as on the
    // phone — whenever Apple can be reached.
    {
        let mut library = data.0.lock().await;
        let reminder = reminder_to_write(&library, &guid)?;
        outbox::record_reminder_edit(&mut library, &reminder, &edit).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    alerts::changed();
    Ok(())
}

/// One of Apple's smart lists. `until` is the end of today in the viewer's own timezone,
/// which the interface knows and the backend does not.
#[tauri::command]
async fn reminders_smart(
    kind: SmartList,
    until: i64,
    data: State<'_, Data>,
) -> UiResult<Vec<Reminder>> {
    data.0
        .read(|library| library.reminders_smart(kind, until))
        .await
        .map_err(|e| e.to_string())
}

/// Create, rename or delete a list, then re-read so the sidebar matches Apple.
#[tauri::command]
async fn reminder_list_create(
    title: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::reminders::create_list_cloudkit(&http, &root, params.clone(), &title)
        .await
        .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

#[tauri::command]
async fn reminder_list_rename(
    list_guid: String,
    title: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::reminders::rename_list_cloudkit(
        &http,
        &root,
        params.clone(),
        &list_guid,
        &title,
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

/// Move a reminder within its list, before another or to the end, as a drag does.
#[tauri::command]
async fn reminder_reorder(
    list_guid: String,
    guid: String,
    before: Option<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::reminders::reorder_cloudkit(
        &http,
        &root,
        params.clone(),
        &list_guid,
        &guid,
        before.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

/// A list's colour, one of Apple's twelve, as Show List Info sets it.
#[tauri::command]
async fn reminder_list_colour(
    list_guid: String,
    colour: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::reminders::set_list_colour_cloudkit(
        &http,
        &root,
        params.clone(),
        &list_guid,
        &colour,
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

#[tauri::command]
async fn reminder_list_delete(
    list_guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::reminders::delete_list_cloudkit(&http, &root, params.clone(), &list_guid)
        .await
        .map_err(|e| e.to_string())?;
    settle(resync_reminders(&http, &root, params, &data).await)
}

/// A refresh after a write that reached Apple. Failing, it is not the write failing: the
/// change is made, and saying otherwise had people retry it — a second folder, a second
/// upload. It is noted, and the next refresh brings the screen up to date.
fn settle(refreshed: Result<(), String>) -> Result<(), String> {
    if let Err(e) = refreshed {
        quiet!("refresh after a write: {e}");
    }
    Ok(())
}

/// Re-read notes after a write, so what is on screen is what Apple stored.
async fn resync_notes(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
) -> Result<(), String> {
    refresh_notes(http, root, params, data).await
}

/// One note, by record name. `None` once Apple no longer has it.
async fn note_by_guid(data: &State<'_, Data>, guid: &str) -> Result<Note, String> {
    let library = data.0.lock().await;
    // Wherever it is filed, Recently Deleted included: Delete Now and Recover act on notes
    // there, and looking only in "All" answered each of them "no longer in the library".
    library
        .note(guid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That note is no longer in the local library.".into())
}

/// Save a note's body.
///
/// Writes to Apple first and re-reads after, so the editor never shows a version Apple has
/// not accepted. A note whose document cannot be rewritten safely is refused with the
/// reason rather than flattened.
/// One note's text, whole.
///
/// A list of notes carries no bodies — five thousand ordinary ones come to nearly ten
/// megabytes, and sending that into the window on every refresh to draw text nobody reads is
/// the unbounded payload the plan rules out. The note being read asks for its own.
#[tauri::command]
async fn note_body(guid: String, data: State<'_, Data>) -> UiResult<Option<String>> {
    data.0
        .read(|library| library.note_body(&guid))
        .await
        .map_err(|e| e.to_string())
}

/// One note's row as the library lists it — title, preview, time edited — without its text:
/// what a save just made of it, for the list to show at once without reading every row again.
#[tauri::command]
async fn note_row(guid: String, data: State<'_, Data>) -> UiResult<Option<Note>> {
    data.0
        .read(|library| library.note(&guid))
        .await
        .map_err(|e| e.to_string())
}

/// A note written out as Markdown at `path` (chosen in the save dialog): its text as it is on
/// screen, typing not yet sent included, set as it is in Notes — headings, lists,
/// checklists, code, quotes, bold, italic, links.
#[tauri::command]
async fn note_export_markdown(
    body: String,
    runs: Vec<quince_icloud::ckdocument::Run>,
    path: String,
) -> UiResult<()> {
    grants::take_write(&path)?;
    let text = quince_icloud::markdown::note_to_markdown(&body, &runs);
    tokio::fs::write(&path, text)
        .await
        .map_err(|e| format!("Could not save it there: {e}"))
}

/// Set one paragraph of a note: `style` from the Aa menu, or `checked` for a checklist tick.
#[tauri::command]
async fn note_paragraph(
    guid: String,
    start: u64,
    style: Option<u64>,
    checked: Option<bool>,
    // The line's text as it was shown, so a tick cannot land on a line that moved there.
    expect: Option<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    use quince_icloud::ckdocument::ParagraphChange;
    let change = match (style, checked) {
        (Some(s), _) => ParagraphChange::Style(s),
        (None, Some(c)) => ParagraphChange::Checked(c),
        (None, None) => return Ok(()),
    };
    let (http, root, params) = cloudkit_context(&auth).await?;
    let note = note_by_guid(&data, &guid).await?;
    quince_icloud::notes::set_paragraph_cloudkit(
        &http,
        &root,
        params.clone(),
        &note,
        start,
        change,
        expect.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_notes(&http, &root, params, &data).await)
}

/// How a note's text is set — Apple's attribute runs, in order — as stored at the last sync.
#[tauri::command]
async fn note_runs(guid: String, data: State<'_, Data>) -> UiResult<serde_json::Value> {
    let stored = data
        .0
        .read(|library| library.note_runs(&guid))
        .await
        .map_err(|e| e.to_string())?;
    Ok(stored
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Array(Vec::new())))
}

/// A table's cells, row by row, as the last pass read them — `None` when none are known.
/// Whether Quince writes to Notes tables.
///
/// Cells: ON. On 2026-10-21 cell, row and column writes showed on his iPhone but left two
/// conflict copies with the text doubled — this app's writer had been appended to the table's
/// writer list out of uuid order. With the list kept sorted, a cell written here (2026-10-22,
/// supervised, on his account) showed once on his iPhone with no conflict copy.
///
/// Rows and columns: still OFF. Only a cell write was tried live after the fix; adding or
/// removing a row or column has not been watched on a phone since.
const TABLE_CELL_WRITES: bool = true;
const TABLE_LINE_WRITES: bool = false;
const TABLE_WRITES_OFF: &str =
    "Tables are read-only in Skykeep for now: edits made here didn’t merge cleanly on iPhone yet.";

/// Add an empty row to a Notes table at `at` (0 is the top), as Notes does, and keep the
/// table as Apple now holds it.
#[tauri::command]
async fn note_table_add_row(
    id: String,
    at: usize,
    column: Option<bool>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<Vec<String>>> {
    if !TABLE_LINE_WRITES {
        return Err(TABLE_WRITES_OFF.into());
    }
    let (http, root, params) = cloudkit_context(&auth).await?;
    let cells = quince_icloud::notes::add_table_line_cloudkit(
        &http,
        &root,
        params,
        &id,
        column.unwrap_or(false),
        at,
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Ok(json) = serde_json::to_string(&cells) {
        data.0
            .lock()
            .await
            .set_state(&quince_icloud::table::state_key(&id), &json)
            .map_err(|e| e.to_string())?;
    }
    Ok(cells)
}

/// Delete a row (or, with `column`, a column) of a Notes table, and keep the table as Apple
/// now holds it.
#[tauri::command]
async fn note_table_remove(
    id: String,
    at: usize,
    column: bool,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<Vec<String>>> {
    if !TABLE_LINE_WRITES {
        return Err(TABLE_WRITES_OFF.into());
    }
    let (http, root, params) = cloudkit_context(&auth).await?;
    let cells =
        quince_icloud::notes::remove_table_line_cloudkit(&http, &root, params, &id, column, at)
            .await
            .map_err(|e| e.to_string())?;
    if let Ok(json) = serde_json::to_string(&cells) {
        data.0
            .lock()
            .await
            .set_state(&quince_icloud::table::state_key(&id), &json)
            .map_err(|e| e.to_string())?;
    }
    Ok(cells)
}

/// Change one cell of a Notes table, and keep the table as Apple now holds it.
#[tauri::command]
async fn note_table_set_cell(
    id: String,
    row: usize,
    column: usize,
    text: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<Vec<String>>> {
    if !TABLE_CELL_WRITES {
        return Err(TABLE_WRITES_OFF.into());
    }
    let (http, root, params) = cloudkit_context(&auth).await?;
    let cells = quince_icloud::notes::set_table_cell_cloudkit(
        &http, &root, params, &id, row, column, &text,
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut library = data.0.lock().await;
    if let Ok(json) = serde_json::to_string(&cells) {
        library
            .set_state(&quince_icloud::table::state_key(&id), &json)
            .map_err(|e| e.to_string())?;
    }
    Ok(cells)
}

#[tauri::command]
async fn note_table(
    id: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Option<Vec<Vec<String>>>> {
    if let Some(cells) = quince_icloud::table::stored(&*data.0.lock().await, &id) {
        return Ok(Some(cells));
    }
    // Not stored yet — a table made before this app read tables, which a pass of changes
    // does not bring — so asked of Apple now, and kept.
    let (http, root, params) = cloudkit_context(&auth).await?;
    let client = quince_icloud::cloudkit::ZoneClient::new(&http, &root, "com.apple.notes", params);
    let record = client
        .lookup_record("Notes", &id)
        .await
        .map_err(|e| e.to_string())?;
    let cells = record
        .as_ref()
        .and_then(|r| r.pointer("/fields/MergeableDataEncrypted/value"))
        .and_then(serde_json::Value::as_str)
        .and_then(quince_icloud::table::cells_from_base64);
    if let Some(cells) = &cells {
        let mut library = data.0.lock().await;
        if let Ok(json) = serde_json::to_string(cells) {
            library
                .set_state(&quince_icloud::table::state_key(&id), &json)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(cells)
}

#[tauri::command]
async fn note_save(
    guid: String,
    body: String,
    // The text the editor began from. With it a save can tell an edit made on another device
    // since, and keep both rather than write over it.
    base: Option<String>,
    // The note's formatting, when it is edited formatted: runs over `body`.
    runs: Option<Vec<quince_icloud::ckdocument::Run>>,
    // The formatting the editor began from, over `base`: a line not restyled since keeps
    // what Apple holds now, so a tick made on the phone is not undone.
    base_runs: Option<Vec<quince_icloud::ckdocument::Run>>,
    data: State<'_, Data>,
) -> UiResult<String> {
    let note = note_by_guid(&data, &guid).await?;
    if !note.editable {
        return Err(
            "This note holds formatting Skykeep can't rewrite safely, so it stays read-only."
                .into(),
        );
    }
    // Kept in the library first — durable, and what every read shows — then sent behind
    // (outbox.rs), merged with whatever Apple holds by then against the text it was typed
    // over. Offline, it waits there, across a restart, until Apple can be reached.
    {
        let mut library = data.0.lock().await;
        outbox::record_note_save(&mut library, &guid, &body, base, runs, base_runs)
            .map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(body)
}

/// Write a new note and return the record name it has — chosen here, before Apple hears of
/// it. In the library at once and sent behind (outbox.rs): offline, it waits there, and the
/// first edits to it queue behind the create.
#[tauri::command]
async fn note_create(
    folder_guid: Option<String>,
    body: String,
    data: State<'_, Data>,
) -> UiResult<String> {
    if body.trim().is_empty() {
        return Err("A note needs something in it.".into());
    }
    let guid = {
        let mut library = data.0.lock().await;
        outbox::record_note_create(&mut library, folder_guid, &body).map_err(|e| e.to_string())?
    };
    outbox::kick();
    Ok(guid)
}

/// Delete a note: to Recently Deleted, or from there for good. At once here, sent behind.
#[tauri::command]
async fn note_delete(guid: String, data: State<'_, Data>) -> UiResult<()> {
    let note = note_by_guid(&data, &guid).await?;
    {
        let mut library = data.0.lock().await;
        outbox::record_note_delete(&mut library, &note).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(())
}

/// Move a note into a folder, or out of every folder when `folder_guid` is absent. At once
/// here, sent behind.
#[tauri::command]
async fn note_move(
    guid: String,
    folder_guid: Option<String>,
    data: State<'_, Data>,
) -> UiResult<()> {
    note_by_guid(&data, &guid).await?;
    {
        let mut library = data.0.lock().await;
        outbox::record_note_move(&mut library, &guid, folder_guid).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(())
}

/// Pin a note to the top of its list, or unpin it. At once here, sent behind.
#[tauri::command]
async fn note_pin(guid: String, pinned: bool, data: State<'_, Data>) -> UiResult<()> {
    note_by_guid(&data, &guid).await?;
    {
        let mut library = data.0.lock().await;
        outbox::record_note_pin(&mut library, &guid, pinned).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(())
}

/// A new Notes folder, inside another when `parent` is given.
#[tauri::command]
async fn note_folder_create(
    title: String,
    parent: Option<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<String> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    let guid = quince_icloud::notes::create_folder_cloudkit(
        &http,
        &root,
        params.clone(),
        &title,
        parent.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_notes(&http, &root, params, &data).await)?;
    Ok(guid)
}

#[tauri::command]
async fn note_folder_rename(
    guid: String,
    title: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    quince_icloud::notes::rename_folder_cloudkit(&http, &root, params.clone(), &guid, &title)
        .await
        .map_err(|e| e.to_string())?;
    settle(resync_notes(&http, &root, params, &data).await)
}

/// Delete a folder: its notes, and those of every folder inside it, to Recently Deleted.
#[tauri::command]
async fn note_folder_delete(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = cloudkit_context(&auth).await?;
    // Read what is needed, then let go of the library before any network call.
    let (folders, notes) = {
        let library = data.0.lock().await;
        (
            library.note_folders().map_err(|e| e.to_string())?,
            library.notes_in(None).map_err(|e| e.to_string())?,
        )
    };
    quince_icloud::notes::delete_folder_cloudkit(
        &http,
        &root,
        params.clone(),
        &folders,
        &notes,
        &guid,
    )
    .await
    .map_err(|e| e.to_string())?;
    settle(resync_notes(&http, &root, params, &data).await)
}

#[tauri::command]
async fn note_folders(data: State<'_, Data>) -> UiResult<Vec<NoteFolder>> {
    data.0
        .read(Library::note_folders)
        .await
        .map_err(|e| e.to_string())
}

/// Notes holding every word of `query`, from every folder but Recently Deleted.
#[tauri::command]
async fn notes_search(
    query: String,
    app: tauri::AppHandle,
    data: State<'_, Data>,
) -> UiResult<Vec<Note>> {
    // On the search reader, so a sync pass holding the library does not hold the list up.
    let (dir, q) = (state_dir(&app), query.clone());
    let read =
        tauri::async_runtime::spawn_blocking(move || read_for_search(&dir, |l| l.search_notes(&q)))
            .await
            .map_err(|e| e.to_string())?;
    match read {
        Some(found) => found,
        None => data
            .0
            .lock()
            .await
            .search_notes(&query)
            .map_err(|e| e.to_string()),
    }
}

/// How many notes each folder holds, and all of them but Recently Deleted.
#[tauri::command]
async fn note_counts(data: State<'_, Data>) -> UiResult<(i64, Vec<(String, i64)>)> {
    data.0
        .read(Library::note_counts)
        .await
        .map_err(|e| e.to_string())
}

/// Notes in a folder, or every note when no folder is given.
#[tauri::command]
async fn notes_in(folder_guid: Option<String>, data: State<'_, Data>) -> UiResult<Vec<Note>> {
    data.0
        .read(|library| library.notes_in(folder_guid.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

/// Everything a contacts write needs from the session, with the auth lock released first.
async fn contacts_context(
    auth: &State<'_, Auth>,
) -> Result<(reqwest::Client, String, Vec<(String, String)>), String> {
    let client = auth.client.lock().await;
    let root = client
        .session()
        .service_url("contacts")
        .ok_or("Apple hasn't offered a contacts service for this account.")?;
    Ok((client.http().clone(), root, client.service_params()))
}

/// Re-read the address book after a write, so what is on screen is what Apple stored.
async fn resync_contacts(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
) -> Result<(), String> {
    let asked_for = asked_as(&params);
    let people = quince_icloud::contacts::fetch(http, root, params)
        .await
        .map_err(|e| e.to_string())?;
    let mut library = data.0.lock().await;
    owned_by(&library, asked_for.as_deref())?;
    library
        .atomically(|library| {
            quince_icloud::contacts::apply(library, people)?;
            outbox::replay(library, outbox::CONTACTS)?;
            Ok::<_, quince_icloud::Error>(())
        })
        .map_err(|e| e.to_string())
}

/// The Contacts lists, as the last pass stored them.
#[tauri::command]
async fn contact_lists(
    data: State<'_, Data>,
) -> UiResult<Vec<quince_icloud::contacts::ContactList>> {
    Ok(data
        .0
        .read(|library| Ok(quince_icloud::contacts::stored_lists(library)))
        .await
        .unwrap_or_default())
}

/// One Contacts list write at a time. Each reads the list as Apple holds it and sends it
/// back whole; two at once would both read the same etag and one would be refused.
static CONTACT_LISTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Change a Contacts list: make one (no `guid`), rename it, add people to it or take them
/// out, or delete it. Answers with the list's id.
///
/// Apple only takes a list whole, so membership is sent as what to add and remove and laid
/// over the list as Apple holds it at the moment of writing — someone added on the phone a
/// second ago stays in. A write that loses to one made elsewhere is read again and retried.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn contact_list_write(
    guid: Option<String>,
    name: Option<String>,
    add: Option<Vec<String>>,
    remove: Option<Vec<String>>,
    delete: Option<bool>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<String> {
    let _one = CONTACTS_PASS.lock().await;
    let _one = CONTACT_LISTS.lock().await;
    let (http, root, params) = contacts_context(&auth).await?;
    let add = add.unwrap_or_default();
    let remove = remove.unwrap_or_default();
    let mut attempt = 0;
    let done = loop {
        attempt += 1;
        let (book, tokens) =
            quince_icloud::contacts::fetch_book_with_tokens(&http, &root, params.clone())
                .await
                .map_err(|e| e.to_string())?;
        let tokens = quince_icloud::contacts::Ledger::holding(tokens);
        let written = match guid.as_deref().filter(|g| !g.is_empty()) {
            None => {
                quince_icloud::contacts::create_list(
                    &http,
                    &root,
                    params.clone(),
                    &tokens,
                    name.as_deref().unwrap_or("Untitled List"),
                    &add,
                )
                .await
            }
            Some(guid) => {
                let Some(mut list) = book
                    .lists
                    .into_iter()
                    .find(|l| l.guid.eq_ignore_ascii_case(guid))
                else {
                    // Already gone — deleted here a moment ago, or on another device.
                    break guid.to_string();
                };
                if delete.unwrap_or(false) {
                    quince_icloud::contacts::delete_list(
                        &http,
                        &root,
                        params.clone(),
                        &tokens,
                        &list.guid,
                    )
                    .await
                } else {
                    if let Some(name) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                        list.name = name.to_string();
                    }
                    list.members
                        .retain(|m| !remove.iter().any(|r| r.eq_ignore_ascii_case(m)));
                    for person in &add {
                        if !list.members.iter().any(|m| m.eq_ignore_ascii_case(person)) {
                            list.members.push(person.clone());
                        }
                    }
                    quince_icloud::contacts::update_list(
                        &http,
                        &root,
                        params.clone(),
                        &tokens,
                        &list,
                    )
                    .await
                }
                .map(|()| list.guid)
            }
        };
        match written {
            Ok(guid) => break guid,
            Err(quince_icloud::Error::Conflict) if attempt < 3 => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    settle(resync_contacts(&http, &root, params, &data).await)?;
    Ok(done)
}

/// Create a contact, or save changes to one. Returns the id Apple knows it by.
///
/// An edit starts from Apple's own card, so everything this app does not model — a photo,
/// related names, social profiles — is written back untouched rather than dropped.
#[tauri::command]
async fn contact_save(
    contact: quince_core::contacts::Card,
    // The card as the form opened it: only what differs from it is written.
    base: Option<quince_core::contacts::Card>,
    data: State<'_, Data>,
) -> UiResult<String> {
    if contact.contact.guid.is_empty()
        && quince_icloud::contacts::blank_card(&contact.contact)
        && contact.details.is_empty()
    {
        return Err("Add a name, a phone number or an email address first.".into());
    }
    // In the library at once — a new card under the id it will have at Apple — and queued
    // (outbox.rs). Sent, an edit starts from Apple's own card, so everything this app does not
    // model — a photo, related names, social profiles — is written back untouched.
    let guid = {
        let mut library = data.0.lock().await;
        outbox::record_card_save(&mut library, &contact, base).map_err(|e| e.to_string())?
    };
    outbox::kick();
    Ok(guid)
}

/// Delete a card: gone here at once, deleted at Apple behind (outbox.rs).
#[tauri::command]
async fn contact_delete(guid: String, data: State<'_, Data>) -> UiResult<()> {
    {
        let mut library = data.0.lock().await;
        // The one card, not the whole book: deleting fifty people read every card fifty times,
        // each under the library's lock.
        let contact = library
            .cards_by_guid(std::slice::from_ref(&guid))
            .map_err(|e| e.to_string())?
            .into_iter()
            .next()
            .ok_or("That contact is no longer in the local library.")?
            .contact;
        outbox::record_contact_delete(&mut library, &contact).map_err(|e| e.to_string())?;
    }
    outbox::kick();
    Ok(())
}

/// A path in `dir` for `name` that holds nothing yet: a number goes on the end rather than
/// writing over a file already there — the rule a browser follows.
fn free_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let (stem, ext) = name_parts(name);
    let mut path = dir.join(name);
    // "Report 2.pdf" for the second copy, as Finder and a folder download (folder_download's
    // `Taken`) number it, so every save names a second copy the same way.
    let mut suffix = 2;
    // Anything at all under the name counts as taken — a link too, even one that leads
    // nowhere: written through, it would put the file wherever the link points.
    while path.symlink_metadata().is_ok() {
        path = dir.join(format!("{stem} {suffix}{ext}"));
        suffix += 1;
    }
    path
}

/// A file name as its stem and its extension (with the dot, or empty): the whole of a
/// compressed tar's (".tar.gz"), and only something that reads as one — short, with no space —
/// so "v1.2 notes" has none.
pub(crate) fn name_parts(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for double in [".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst"] {
        if lower.len() > double.len() && lower.ends_with(double) {
            let at = name.len() - double.len();
            if name.is_char_boundary(at) {
                return (&name[..at], &name[at..]);
            }
        }
    }
    match name.rfind('.') {
        Some(at)
            if at > 0
                && name.len() - at - 1 <= 16
                && !name[at..].contains(' ')
                && at + 1 < name.len() =>
        {
            (&name[..at], &name[at..])
        }
        _ => (name, ""),
    }
}

/// These people as a vCard file in Downloads, written by Apple. Returns where it went.
#[tauri::command]
async fn contacts_export(
    guids: Vec<String>,
    file_name: String,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
) -> UiResult<String> {
    if guids.is_empty() {
        return Err("Choose someone to export.".into());
    }
    let (http, root, params) = contacts_context(&auth).await?;
    // An export changes nothing; its tokens are read with the book on the way.
    let tokens = quince_icloud::contacts::Ledger::new();
    let vcard =
        quince_icloud::contacts::export_vcards(&http, &root, params, &tokens, &guids, &file_name)
            .await
            .map_err(|e| e.to_string())?;
    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| "Couldn't find your Downloads folder.".to_string())?;
    // A name fit for a file: no folders, streams or devices smuggled in, never empty.
    let clean = if file_name.trim().trim_end_matches(".vcf").is_empty() {
        "iCloud vCards.vcf"
    } else {
        file_name.as_str()
    };
    let path = saving::save_path(&downloads, clean, "iCloud vCards.vcf")?;
    std::fs::write(&path, vcard).map_err(|e| format!("Couldn't save the vCard: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Add everyone in a vCard file to Contacts. Answers how many came in, how many could not be
/// read, and how many were left out because they are in Contacts already.
#[tauri::command]
async fn contacts_import(
    paths: Vec<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<(usize, usize, usize)> {
    for path in &paths {
        grants::take_read(path)?;
    }
    let _one = CONTACTS_PASS.lock().await;
    // Every file chosen, vCard or CSV (Google Contacts, Outlook), read into cards first, so a
    // file that cannot be read stops the import before anything is sent.
    let mut cards = Vec::new();
    let mut unreadable = 0;
    for path in &paths {
        let text = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "Couldn't read {}: {e}",
                std::path::Path::new(path)
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned())
            )
        })?;
        if path.to_ascii_lowercase().ends_with(".csv") {
            let (from_csv, skipped) = quince_icloud::contacts::csv_to_vcards(&text);
            cards.extend(from_csv);
            unreadable += skipped;
        } else {
            cards.extend(quince_icloud::contacts::split_vcards(&text));
        }
    }
    if cards.is_empty() {
        return Err("Those files hold no contacts.".into());
    }
    let (http, root, params) = contacts_context(&auth).await?;
    let (book, tokens) = quince_icloud::contacts::fetch_with_tokens(&http, &root, params.clone())
        .await
        .map_err(|e| e.to_string())?;
    // Each batch is a write that moves the book on: the next carries the tokens Apple
    // answered it with, or reads them again — never the first batch's spent ones.
    let tokens = quince_icloud::contacts::Ledger::holding(tokens);
    // Someone already in Contacts is left out, as Apple's Contacts leaves them: an export of
    // this same book imported back once doubled every person in it.
    let before = cards.len();
    cards.retain(|card| !quince_icloud::contacts::already_in(card, &book));
    let known = before - cards.len();
    if cards.is_empty() {
        return Ok((0, unreadable, known));
    }
    // In batches Apple takes in one import: well under its 10 MB, a few hundred cards each.
    let mut imported = 0;
    for batch in quince_icloud::contacts::batch_vcards(&cards, 8 * 1024 * 1024, 400) {
        let (ids, failed) =
            quince_icloud::contacts::import_vcards(&http, &root, params.clone(), &tokens, &batch)
                .await
                .map_err(|e| e.to_string())?;
        imported += ids.len();
        unreadable += failed;
    }
    settle(resync_contacts(&http, &root, params, &data).await)?;
    Ok((imported, unreadable, known))
}

/// The card that is this person's own, as the last pass stored it.
#[tauri::command]
async fn contact_me(data: State<'_, Data>) -> UiResult<Option<String>> {
    Ok(data
        .0
        .read(|library| library.state(quince_icloud::contacts::ME_KEY))
        .await
        .ok()
        .flatten()
        .filter(|s| !s.is_empty()))
}

/// Make this card "My Card", on every device.
#[tauri::command]
async fn contact_set_me(
    guid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let _one = CONTACTS_PASS.lock().await;
    let (http, root, params) = contacts_context(&auth).await?;
    let tokens = quince_icloud::contacts::Ledger::new();
    quince_icloud::contacts::set_me_card(&http, &root, params.clone(), &tokens, &guid)
        .await
        .map_err(|e| e.to_string())?;
    settle(resync_contacts(&http, &root, params, &data).await)
}

/// The address book, optionally filtered by name or by any stored number or address.
#[tauri::command]
async fn contacts_list(
    search: Option<String>,
    data: State<'_, Data>,
) -> UiResult<Vec<quince_core::contacts::Card>> {
    data.0
        .read(|library| library.cards(search.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

/// These cards alone, whole: what Contacts reads when a pass names the cards it changed.
#[tauri::command]
async fn contacts_get(
    guids: Vec<String>,
    data: State<'_, Data>,
) -> UiResult<Vec<quince_core::contacts::Card>> {
    data.0
        .read(move |library| library.cards_by_guid(&guids))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn drive_sync_folder(
    folder_id: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<usize> {
    let (http, root, params) = {
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("drivews")
            .ok_or("Apple hasn't offered iCloud Drive for this account.")?;
        (client.http().clone(), root, client.service_params())
    };
    let pass = drive_pass(&folder_id);
    let _one = pass.lock().await;
    let items = match quince_icloud::drive::fetch_folder(&http, &root, params, &folder_id).await {
        Ok(items) => items,
        // Deleted or moved on another device: forgotten here too, so the screen can step back
        // out of it rather than show a folder that is not there.
        Err(quince_icloud::Error::NotFound) => {
            let mut library = data.0.lock().await;
            library
                .forget_drive_folder(&folder_id)
                .map_err(|e| e.to_string())?;
            return Err("This folder was deleted or moved on another device.".into());
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut library = data.0.lock().await;
    quince_icloud::drive::store_folder(&mut library, &folder_id, &items).map_err(|e| e.to_string())
}

#[tauri::command]
async fn drive_children(folder_id: String, data: State<'_, Data>) -> UiResult<Vec<DriveItem>> {
    data.0
        .read(|library| library.drive_children(&folder_id))
        .await
        .map_err(|e| e.to_string())
}

/// One item, for building the breadcrumb trail back to the root.
#[tauri::command]
async fn drive_item(drivewsid: String, data: State<'_, Data>) -> UiResult<Option<DriveItem>> {
    data.0
        .read(|library| library.drive_item(&drivewsid))
        .await
        .map_err(|e| e.to_string())
}

/// Everything a Drive write needs from the session, with the auth lock released first.
async fn drive_context(
    auth: &State<'_, Auth>,
) -> Result<(reqwest::Client, String, Vec<(String, String)>), String> {
    let client = auth.client.lock().await;
    let root = client
        .session()
        .service_url("drivews")
        .ok_or("Apple hasn't offered iCloud Drive for this account.")?;
    Ok((client.http().clone(), root, client.service_params()))
}

/// Re-read one folder after a write, so what is on screen is what Apple stored.
async fn resync_drive_folder(
    http: &reqwest::Client,
    root: &str,
    params: Vec<(String, String)>,
    data: &State<'_, Data>,
    folder_id: &str,
) -> Result<(), String> {
    let pass = drive_pass(folder_id);
    let _one = pass.lock().await;
    let asked_for = asked_as(&params);
    let items = quince_icloud::drive::fetch_folder(http, root, params, folder_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut library = data.0.lock().await;
    owned_by(&library, asked_for.as_deref())?;
    quince_icloud::drive::store_folder(&mut library, folder_id, &items)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Save a file's bytes to the user's Downloads folder.
///
/// Apple's own name is kept, with a number appended rather than silently overwriting
/// something already there — the same rule a browser follows. Returns the path saved to.
#[tauri::command]
async fn drive_download(
    drivewsid: String,
    item: Option<quince_core::drive::DriveItem>,
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<String> {
    // A file seen only in Recents was never in a folder this library has read, so the item
    // comes with the request; the library's own copy wins when there is one.
    let stored = {
        let library = data.0.lock().await;
        library.drive_item(&drivewsid).map_err(|e| e.to_string())?
    };
    let item = stored
        .or(item.filter(|i| i.drivewsid == drivewsid))
        .ok_or("That item is no longer in the local library.")?;
    if item.is_container() {
        return Err("That's a folder, not a file.".into());
    }

    let (http, root, params) = {
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("docws")
            .ok_or("Apple hasn't offered downloads for this account.")?;
        (client.http().clone(), root, client.service_params())
    };

    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| "Couldn't find your Downloads folder.".to_string())?;
    // The name is whoever shared the folder's to choose (a `..\\` or a `C:` in it would reach
    // out of Downloads on Windows): saved as one plain name in Downloads, or not at all.
    tokio::fs::create_dir_all(&downloads)
        .await
        .map_err(|e| e.to_string())?;
    let destination = saving::save_path(&downloads, &item.display_name(), "Download")?;

    quince_icloud::drive::download_to(&http, &root, params, &item, &destination)
        .await
        .map_err(|e| e.to_string())?;
    saving::mark_from_internet(&destination);
    Ok(destination.to_string_lossy().into_owned())
}

/// Downloads that can still be stopped, by batch.
fn downloads_running(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, folder_download::Stop>> {
    static RUNNING: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, folder_download::Stop>>,
    > = std::sync::OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// The downloads running now, by what they are known by (`Journal::key`): the same one twice
/// at once would write into the one folder from two sides.
fn downloads_by_key() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Where unfinished folder downloads are remembered, so downloading the same again resumes.
fn download_journal(app: &tauri::AppHandle) -> folder_download::Journal {
    folder_download::Journal::new(state_dir(app).join("downloads"))
}

/// An unfinished download from before, for the window to offer to go on with.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UnfinishedDownload {
    key: String,
    title: String,
    items: Vec<quince_core::drive::DriveItem>,
    /// What of it is in Downloads now.
    saved: Vec<String>,
}

/// Downloads into Downloads that did not finish — cancelled, stopped short, or cut off by
/// quitting — oldest first.
#[tauri::command]
fn drive_downloads_unfinished(app: tauri::AppHandle) -> Vec<UnfinishedDownload> {
    let running = downloads_by_key()
        .lock()
        .map(|k| k.clone())
        .unwrap_or_default();
    download_journal(&app)
        .unfinished()
        .into_iter()
        .filter(|r| !running.contains(&r.key))
        .map(|r| UnfinishedDownload {
            saved: r
                .placed
                .iter()
                .filter(|p| std::fs::symlink_metadata(&p.path).is_ok())
                .map(|p| p.path.to_string_lossy().into_owned())
                .collect(),
            key: r.key,
            title: r.title,
            items: r.items,
        })
        .collect()
}

/// Let an unfinished download go: it is no longer offered, and downloading the same again
/// starts afresh. What it saved stays where it is.
#[tauri::command]
fn drive_download_forget(app: tauri::AppHandle, key: String) {
    download_journal(&app).forget(&key);
}

/// Save folders — and a selection of files and folders — from Drive into Downloads, as the
/// same tree (see `folder_download`), or go on with the same download where it was left.
/// Progress arrives as `quince://download` events; the answer is what was saved, where, what
/// was not, and whether it can be gone on with.
///
/// The items come from the window only to be found: the library's own copy of each wins when
/// it has one, as a single download's does, and nothing the window says is ever a path here —
/// every name is made a plain one, in a folder made for it in Downloads.
#[tauri::command]
async fn drive_download_items(
    app: tauri::AppHandle,
    batch: String,
    title: String,
    items: Vec<quince_core::drive::DriveItem>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<folder_download::DownloadReport> {
    if items.is_empty() {
        return Err("Nothing was chosen to download.".into());
    }
    let chosen = {
        let library = data.0.lock().await;
        let mut chosen = Vec::with_capacity(items.len());
        for item in items {
            let stored = library
                .drive_item(&item.drivewsid)
                .map_err(|e| e.to_string())?;
            chosen.push(stored.unwrap_or(item));
        }
        chosen
    };
    let (http, drivews, params) = drive_context(&auth).await?;
    let docws = {
        let client = auth.client.lock().await;
        client
            .session()
            .service_url("docws")
            .ok_or("Apple hasn't offered downloads for this account.")?
    };
    let downloads = app
        .path()
        .download_dir()
        .map_err(|_| "Couldn't find your Downloads folder.".to_string())?;
    tokio::fs::create_dir_all(&downloads)
        .await
        .map_err(|e| e.to_string())?;

    let key = folder_download::Journal::key(&chosen);
    let fresh = downloads_by_key()
        .lock()
        .is_ok_and(|mut k| k.insert(key.clone()));
    if !fresh {
        return Err("That is downloading already.".into());
    }
    let stop = folder_download::Stop::default();
    downloads_running()
        .lock()
        .map(|mut m| m.insert(batch.clone(), stop.clone()))
        .ok();
    let drive = folder_download::Drive {
        http,
        drivews,
        docws,
        params,
        retry_after: std::time::Duration::from_secs(1),
        free_space: folder_download::free_space,
        room_every: folder_download::ROOM_EVERY,
    };
    let emit: folder_download::Emit = {
        let app = app.clone();
        std::sync::Arc::new(move |event| {
            app.emit(folder_download::DOWNLOAD, &event).ok();
        })
    };
    let journal = download_journal(&app);
    let request = folder_download::Request {
        batch: &batch,
        title: &title,
        items: chosen,
        into: &downloads,
        journal: Some(&journal),
    };
    let report = folder_download::run(&drive, request, emit, &stop).await;
    downloads_running()
        .lock()
        .map(|mut m| m.remove(&batch))
        .ok();
    downloads_by_key().lock().map(|mut k| k.remove(&key)).ok();
    report
}

/// Stop downloads: a whole batch, or the batches these files are in. What has arrived stays;
/// what is arriving is dropped with its partial file, and nothing more begins.
#[tauri::command]
fn drive_download_cancel(ids: Vec<String>, batch: Option<String>) {
    let mut batches: Vec<String> = ids
        .iter()
        .filter_map(|id| id.split_once(':').map(|(b, _)| b.to_string()))
        .collect();
    batches.extend(batch);
    if let Ok(running) = downloads_running().lock() {
        for b in batches {
            if let Some(stop) = running.get(&b) {
                stop.stop();
            }
        }
    }
}

/// Where an upload's progress is announced, one event per file as it moves.
const UPLOAD: &str = "quince://upload";

/// One file's upload, as the window shows it.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UploadEvent {
    /// Unique within the batch the window started.
    id: String,
    batch: String,
    name: String,
    /// The folders it sits in below the one it was dropped on, for a folder upload.
    within: String,
    sent: u64,
    total: u64,
    /// `preparing` (a folder being made on Apple), `waiting`, `sending`, `verifying`, `done`,
    /// `failed` or `cancelled`; and once per batch `planned`, when every file in it is known.
    state: &'static str,
    /// A folder rather than a file: its row stands for making the folder, not for bytes.
    #[serde(default)]
    folder: bool,
    error: Option<String>,
}

/// Uploads that can still be stopped, by id: waiting, or sending their bytes.
fn uploads_running(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, futures_util::future::AbortHandle>>
{
    static RUNNING: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, futures_util::future::AbortHandle>>,
    > = std::sync::OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// Batches stopped as a whole: no more of their folders are made or files queued.
fn cancelled_batches() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static CANCELLED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    CANCELLED.get_or_init(Default::default)
}

fn cancelled(batch: &str) -> bool {
    cancelled_batches().lock().is_ok_and(|b| b.contains(batch))
}

/// Files stopped one by one with their ✕, by id: one stopped while its folder upload was still
/// being walked had no stop registered yet, so it was sent anyway once the walk was done.
fn cancelled_uploads() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static CANCELLED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    CANCELLED.get_or_init(Default::default)
}

/// Whether this file was stopped before it began — on its own, or with its whole batch. Asked
/// as each file is about to send: Cancel pressed while a folder upload is still being planned
/// stops the planning, and must stop the files already found too.
fn upload_stopped(batch: &str, id: &str) -> bool {
    cancelled(batch) || cancelled_uploads().lock().is_ok_and(|c| c.contains(id))
}

/// A batch is over: what was remembered about stopping it is let go.
fn forget_cancelled(batch: &str) {
    cancelled_batches().lock().map(|mut b| b.remove(batch)).ok();
    let mine = format!("{batch}:");
    cancelled_uploads()
        .lock()
        .map(|mut c| c.retain(|id| !id.starts_with(&mine)))
        .ok();
}

/// Stop uploads: these files, or a whole batch. A file already being filed in iCloud is past
/// stopping and finishes; anything waiting or still sending stops, with nothing of it kept.
#[tauri::command]
fn drive_upload_cancel(ids: Vec<String>, batch: Option<String>) {
    if let Some(batch) = &batch {
        cancelled_batches()
            .lock()
            .map(|mut b| b.insert(batch.clone()))
            .ok();
    }
    cancelled_uploads()
        .lock()
        .map(|mut c| c.extend(ids.iter().cloned()))
        .ok();
    if let Ok(running) = uploads_running().lock() {
        for (id, handle) in running.iter() {
            let in_batch = batch
                .as_deref()
                .is_some_and(|b| id.starts_with(&format!("{b}:")));
            if in_batch || ids.contains(id) {
                handle.abort();
            }
        }
    }
}

/// Something in an upload that will not go up, said in the tray with its own row as well as in
/// the answer: found while the upload was being planned — a file that could not be read, a link,
/// a folder Apple would not make — it had no row, and the tray said "Uploaded" over it.
fn upload_failed(
    app: &tauri::AppHandle,
    batch: &str,
    name: &str,
    within: &str,
    folder: bool,
    error: String,
    failed: &mut Vec<String>,
) {
    app.emit(
        UPLOAD,
        &UploadEvent {
            id: format!("{batch}:x:{}", failed.len()),
            batch: batch.to_string(),
            name: name.to_string(),
            within: within.to_string(),
            sent: 0,
            total: 0,
            state: "failed",
            folder,
            error: Some(error.clone()),
        },
    )
    .ok();
    failed.push(format!("{name}: {error}"));
}

/// One file to send, and the Drive folder it goes into.
struct UploadJob {
    event: UploadEvent,
    path: std::path::PathBuf,
    folder: String,
}

/// The Drive folder called `name` inside `parent`: the one already there, or a new one.
///
/// Dropping a folder that is already in iCloud adds to it, as Finder offers to, rather than
/// making "Photos 2" beside it. Files inside that already exist are kept too: Apple renames
/// the newcomer rather than overwrite.
async fn folder_for(
    http: &reqwest::Client,
    root: &str,
    params: &[(String, String)],
    data: &State<'_, Data>,
    parent: &str,
    name: &str,
) -> Result<(String, bool), String> {
    settle(resync_drive_folder(http, root, params.to_vec(), data, parent).await)?;
    let existing = {
        let library = data.0.lock().await;
        library
            .drive_children(parent)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|i| i.is_container() && i.name.eq_ignore_ascii_case(name))
    };
    if let Some(folder) = existing {
        return Ok((folder.drivewsid, false));
    }
    quince_icloud::drive::create_folder(http, root, params.to_vec(), parent, name)
        .await
        .map(|f| (f.drivewsid, true))
        .map_err(|e| e.to_string())
}

/// Every file under the paths dropped or chosen, with folders made to match on the way.
///
/// Links are not followed: one that points back up the tree would never finish, and one that
/// points outside it would upload something nobody chose.
#[allow(clippy::too_many_arguments)]
async fn plan_upload(
    app: &tauri::AppHandle,
    http: &reqwest::Client,
    root: &str,
    params: &[(String, String)],
    data: &State<'_, Data>,
    batch: &str,
    folder_id: &str,
    paths: &[String],
    failed: &mut Vec<String>,
) -> Vec<UploadJob> {
    let mut jobs = Vec::new();
    let mut folders_made = 0;
    // (path on disk, Drive folder it goes into, folders below the drop target)
    // (path on disk, Drive folder it goes into, folders below the drop target, and for a
    // folder already made on Apple its id and whether this upload made it)
    #[allow(clippy::type_complexity)]
    let mut pending: Vec<(std::path::PathBuf, String, String, Option<(String, bool)>)> = paths
        .iter()
        .map(|p| {
            (
                std::path::PathBuf::from(p),
                folder_id.to_string(),
                String::new(),
                None,
            )
        })
        .collect();
    while let Some((path, into, within, resolved)) = pending.pop() {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into(),
        );
        // What was chosen is followed if it is a link (a Music folder linked from another
        // disk); only what is found inside is not.
        let chosen = within.is_empty() && resolved.is_none();
        let meta = match if chosen {
            tokio::fs::metadata(&path).await
        } else {
            tokio::fs::symlink_metadata(&path).await
        } {
            Ok(meta) => meta,
            Err(e) => {
                upload_failed(app, batch, &name, &within, false, e.to_string(), failed);
                continue;
            }
        };
        if cancelled(batch) {
            break;
        }
        if meta.is_dir() {
            // Said at once, with its own row: making a folder tree on Apple takes a moment
            // per folder, and a folder upload used to show nothing at all until every folder
            // in it was made.
            let mut shown = UploadEvent {
                id: format!("{batch}:dir:{}", folders_made),
                batch: batch.to_string(),
                name: name.clone(),
                within: within.clone(),
                sent: 0,
                total: 0,
                state: "preparing",
                folder: true,
                error: None,
            };
            folders_made += 1;
            app.emit(UPLOAD, &shown).ok();
            // Made already, with its siblings in one request, when its parent was made by this
            // upload; otherwise found or made by name — dropped into a folder that may hold one.
            let found = match resolved {
                Some(done) => Ok(done),
                None => folder_for(http, root, params, data, &into, &name).await,
            };
            let (folder, fresh) = match found {
                Ok(folder) => {
                    shown.state = "done";
                    app.emit(UPLOAD, &shown).ok();
                    folder
                }
                Err(e) => {
                    shown.state = "failed";
                    shown.error = Some(e.to_string());
                    app.emit(UPLOAD, &shown).ok();
                    failed.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let below = if within.is_empty() {
                name.clone()
            } else {
                format!("{within}/{name}")
            };
            let mut entries = match tokio::fs::read_dir(&path).await {
                Ok(entries) => entries,
                Err(e) => {
                    shown.state = "failed";
                    shown.error = Some(e.to_string());
                    app.emit(UPLOAD, &shown).ok();
                    failed.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let mut children = Vec::new();
            while let Ok(Some(entry)) = entries.next_entry().await {
                children.push(entry.path());
            }
            // Popped from the end, so reversed to go in name order.
            children.sort();
            // A folder this upload just made holds nothing yet, so its folders cannot clash
            // with anything there: all of them are made in one request, rather than a read of
            // the parent and a request each — hundreds of round trips for a git repository.
            let mut made: std::collections::HashMap<String, Result<String, String>> =
                std::collections::HashMap::new();
            if fresh {
                let dirs: Vec<String> = {
                    let mut d = Vec::new();
                    for child in &children {
                        if tokio::fs::symlink_metadata(child)
                            .await
                            .is_ok_and(|m| m.is_dir())
                        {
                            d.push(
                                child
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            );
                        }
                    }
                    d
                };
                if !dirs.is_empty() {
                    match quince_icloud::drive::create_folders(
                        http,
                        root,
                        params.to_vec(),
                        &folder,
                        &dirs,
                    )
                    .await
                    {
                        Ok(results) => made.extend(results),
                        Err(e) => {
                            for d in dirs {
                                made.insert(d, Err(e.to_string()));
                            }
                        }
                    }
                }
            }
            for child in children.into_iter().rev() {
                let child_name = child
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match made.remove(&child_name) {
                    Some(Ok(id)) => {
                        pending.push((child, folder.clone(), below.clone(), Some((id, true))))
                    }
                    Some(Err(e)) => upload_failed(app, batch, &child_name, &below, true, e, failed),
                    None => pending.push((child, folder.clone(), below.clone(), None)),
                }
            }
        } else if meta.is_file() {
            let event = UploadEvent {
                id: format!("{batch}:{}", jobs.len()),
                batch: batch.to_string(),
                name,
                within,
                sent: 0,
                total: meta.len(),
                state: "waiting",
                folder: false,
                error: None,
            };
            // Shown the moment it is found, not once the whole tree has been walked.
            app.emit(UPLOAD, &event).ok();
            jobs.push(UploadJob {
                event,
                path,
                folder: into,
            });
        } else {
            upload_failed(
                app,
                batch,
                &name,
                &within,
                false,
                "a link or a special file, which isn\u{2019}t uploaded".into(),
                failed,
            );
        }
    }
    jobs
}

/// Put files from this machine into a Drive folder.
///
/// Paths, never bytes. The interface hands over what the person chose — from the file dialog
/// or from a drag onto the window — and the file is read here and streamed straight to Apple,
/// so putting a film in Drive costs no more memory than putting a note in it.
///
/// Each file is reported on separately: one that Apple refuses must not take the rest of a
/// dragged selection down with it.
///
/// Folders are uploaded whole, and each file's progress is announced as it moves.
///
/// Three at a time: enough to keep the connection busy through each file's round trips
/// without asking Apple for dozens of upload slots at once. Every file is proven intact
/// before it counts as done — see `quince_icloud::drive::upload`. Returns what failed.
#[tauri::command]
async fn drive_upload(
    app: tauri::AppHandle,
    batch: String,
    folder_id: String,
    paths: Vec<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<Vec<String>> {
    use futures_util::StreamExt;

    for path in &paths {
        grants::take_read(path)?;
    }
    let (http, root, params) = drive_context(&auth).await?;
    let docws = {
        let client = auth.client.lock().await;
        client
            .session()
            .service_url("docws")
            .ok_or("Apple hasn't offered uploads for this account.")?
    };

    let mut failed = Vec::new();
    let jobs = plan_upload(
        &app,
        &http,
        &root,
        &params,
        &data,
        &batch,
        &folder_id,
        &paths,
        &mut failed,
    )
    .await;
    // Everything in it has been found: the window can count to a total that will not move.
    app.emit(
        UPLOAD,
        &UploadEvent {
            id: format!("{batch}:planned"),
            batch: batch.clone(),
            name: String::new(),
            within: String::new(),
            sent: 0,
            total: jobs.iter().map(|j| j.event.total).sum(),
            state: "planned",
            folder: true,
            error: None,
        },
    )
    .ok();

    // Every file's stop is made and registered before any starts, so Cancel — the batch's, or
    // one waiting file's ✕ — reaches the files still waiting. Made as each began (three at a
    // time), only those three could be stopped and every other planned file went on up.
    let prepared: Vec<_> = jobs
        .into_iter()
        .map(|job| {
            let (app, http, docws, root, params) = (
                app.clone(),
                http.clone(),
                docws.clone(),
                root.clone(),
                params.clone(),
            );
            let (abort, registration) = futures_util::future::AbortHandle::new_pair();
            // An empty file has no bytes to report, so nothing ever took its stop away before it
            // was filed: a Cancel then could cut its filing short and call it cancelled while it
            // was in iCloud. Waiting, it is still stopped (`upload_stopped`); begun, it finishes.
            if job.event.total > 0 {
                uploads_running()
                    .lock()
                    .map(|mut m| m.insert(job.event.id.clone(), abort))
                    .ok();
            }
            let id = job.event.id.clone();
            let cancelled_event = {
                let mut e = job.event.clone();
                e.state = "cancelled";
                e
            };
            let app_for_cancel = app.clone();
            let stopped_event = cancelled_event.clone();
            let run = async move {
                let mut event = job.event.clone();
                if upload_stopped(&event.batch, &event.id) {
                    app.emit(UPLOAD, &stopped_event).ok();
                    return None;
                }
                event.state = "sending";
                app.emit(UPLOAD, &event).ok();

                // Told about every quarter-megabyte, passed on at every whole percent: a
                // smooth bar without a flood of messages for a large file.
                let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                let progress: quince_icloud::drive::Progress = {
                    let (app, event, last) = (app.clone(), event.clone(), last.clone());
                    std::sync::Arc::new(move |sent: u64, total: u64| {
                        let step = (total / 100).max(1);
                        let before = last.load(std::sync::atomic::Ordering::Relaxed);
                        if sent / step != before / step || sent == total {
                            last.store(sent, std::sync::atomic::Ordering::Relaxed);
                            let mut e = event.clone();
                            e.sent = sent;
                            e.state = if sent >= total {
                                // Every byte is up and it is being filed: too late to stop
                                // cleanly, so it can no longer be cancelled.
                                uploads_running().lock().map(|mut m| m.remove(&e.id)).ok();
                                "verifying"
                            } else {
                                "sending"
                            };
                            app.emit(UPLOAD, &e).ok();
                        }
                    })
                };
                let outcome = quince_icloud::drive::upload(
                    &http,
                    &docws,
                    &root,
                    params,
                    &job.folder,
                    &job.path,
                    progress,
                )
                .await;
                event.sent = event.total;
                match outcome {
                    Ok(_) => {
                        event.state = "done";
                        app.emit(UPLOAD, &event).ok();
                        None
                    }
                    Err(e) => {
                        event.state = "failed";
                        event.error = Some(e.to_string());
                        app.emit(UPLOAD, &event).ok();
                        Some(format!("{}: {e}", event.name))
                    }
                }
            };
            async move {
                let outcome = futures_util::future::Abortable::new(run, registration).await;
                uploads_running().lock().map(|mut m| m.remove(&id)).ok();
                match outcome {
                    Ok(result) => result,
                    // Stopped before its bytes were filed: nothing of it is in iCloud.
                    Err(_) => {
                        app_for_cancel.emit(UPLOAD, &cancelled_event).ok();
                        None
                    }
                }
            }
        })
        .collect();
    let results: Vec<Option<String>> = futures_util::stream::iter(prepared)
        .buffer_unordered(3)
        .collect()
        .await;
    failed.extend(results.into_iter().flatten());
    forget_cancelled(&batch);

    settle(resync_drive_folder(&http, &root, params, &data, &folder_id).await)?;
    Ok(failed)
}

/// Create a folder, and reflect it locally without waiting for the next full sync.
#[tauri::command]
async fn drive_create_folder(
    parent_id: String,
    name: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::create_folder(&http, &root, params.clone(), &parent_id, &name)
        .await
        .map_err(|e| e.to_string())?;
    settle(resync_drive_folder(&http, &root, params, &data, &parent_id).await)
}

#[tauri::command]
async fn drive_rename(
    drivewsid: String,
    name: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let item = {
        let library = data.0.lock().await;
        library
            .drive_item(&drivewsid)
            .map_err(|e| e.to_string())?
            .ok_or("That item is no longer in the local library.")?
    };
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::rename(&http, &root, params.clone(), &item, &name)
        .await
        .map_err(|e| e.to_string())?;
    let parent = item
        .parent_id
        .as_deref()
        .unwrap_or(quince_core::drive::ROOT);
    settle(resync_drive_folder(&http, &root, params, &data, parent).await)
}

/// Delete from the window, only what it showed (see `drive::delete_as_shown`): a file as the
/// version shown, a folder only as the version shown or the one a question since showed
/// (`expect`). A folder that is not comes back as it now stands, to be asked about.
#[tauri::command]
async fn drive_delete(
    drivewsid: String,
    expect: Option<String>,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<quince_icloud::drive::Removed> {
    let (item, shown) = {
        let library = data.0.lock().await;
        let item = library
            .drive_item(&drivewsid)
            .map_err(|e| e.to_string())?
            .ok_or("That item is no longer in the local library.")?;
        let shown = if item.is_container() {
            library
                .drive_children(&item.drivewsid)
                .map_err(|e| e.to_string())?
        } else {
            Vec::new()
        };
        (item, shown)
    };
    let (http, root, params) = drive_context(&auth).await?;
    let removed = quince_icloud::drive::delete_as_shown(
        &http,
        &root,
        params.clone(),
        &item,
        &shown,
        expect.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let parent = item
        .parent_id
        .as_deref()
        .unwrap_or(quince_core::drive::ROOT);
    settle(resync_drive_folder(&http, &root, params, &data, parent).await)?;
    Ok(removed)
}

/// Duplicate a file into its own folder, named as Finder names a copy.
#[tauri::command]
async fn drive_duplicate(
    drivewsid: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let item = {
        let library = data.0.lock().await;
        library
            .drive_item(&drivewsid)
            .map_err(|e| e.to_string())?
            .ok_or("That item is no longer in the local library.")?
    };
    let (http, drivews, docws, params) = drive_hosts(&auth).await?;
    quince_icloud::drive::duplicate(&http, &docws, &drivews, params.clone(), &item)
        .await
        .map_err(|e| e.to_string())?;
    let parent = item
        .parent_id
        .as_deref()
        .unwrap_or(quince_core::drive::ROOT);
    settle(resync_drive_folder(&http, &drivews, params, &data, parent).await)
}

/// Drive's two hosts — `drivews` lists and writes, `docws` copies — and what goes with them.
async fn drive_hosts(
    auth: &State<'_, Auth>,
) -> Result<(reqwest::Client, String, String, Vec<(String, String)>), String> {
    let client = auth.client.lock().await;
    let session = client.session();
    Ok((
        client.http().clone(),
        session
            .service_url("drivews")
            .ok_or("Apple hasn't offered iCloud Drive for this account.")?,
        session
            .service_url("docws")
            .ok_or("Apple hasn't offered iCloud Drive for this account.")?,
        client.service_params(),
    ))
}

/// Copy a file or folder into another folder, as Finder's Copy and Paste: named as Finder
/// names a copy where its name is taken there (quince_icloud::drive::copy_to). A folder is
/// never pasted into itself or into a folder inside it, as Finder will not.
#[tauri::command]
async fn drive_copy(
    drivewsid: String,
    destination_id: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let item = {
        let library = data.0.lock().await;
        let item = library
            .drive_item(&drivewsid)
            .map_err(|e| e.to_string())?
            .ok_or("That item is no longer in the local library.")?;
        // Up from the destination through the folders the library knows: the folder copied
        // must not be one of them.
        let mut at = Some(destination_id.clone());
        for _ in 0..64 {
            let Some(id) = at.take() else { break };
            if id == item.drivewsid {
                return Err(format!(
                    "“{}” can’t be copied into a folder inside itself.",
                    item.display_name()
                ));
            }
            at = library
                .drive_item(&id)
                .ok()
                .flatten()
                .and_then(|f| f.parent_id);
        }
        item
    };
    let (http, drivews, docws, params) = drive_hosts(&auth).await?;
    quince_icloud::drive::copy_to(
        &http,
        &docws,
        &drivews,
        params.clone(),
        &item,
        &destination_id,
    )
    .await
    .map_err(|e| e.to_string())?;
    let source = item
        .parent_id
        .as_deref()
        .unwrap_or(quince_core::drive::ROOT);
    // The folder it was copied beside changed for a moment too, and is read again with it.
    settle(resync_drive_folder(&http, &drivews, params.clone(), &data, &destination_id).await)?;
    if source != destination_id {
        settle(resync_drive_folder(&http, &drivews, params, &data, source).await)?;
    }
    Ok(())
}

/// Apple's thumbnails for one folder's files, keyed by drivewsid: each an address the
/// window's `<img>` loads from the cache on disk itself (`quince://…/dthumb/<file>`).
///
/// Kept on disk beside the library, named by item and version, so a folder opened again
/// shows its pictures at once and only a changed file is fetched anew. Each was read, encoded
/// as a `data:` URL and sent through IPC on every look at the folder — some megabytes of text
/// every ten seconds for a folder of photos, parsed on the window's own thread.
#[tauri::command]
async fn drive_thumbnails(
    folder_id: String,
    app: tauri::AppHandle,
) -> UiResult<std::collections::HashMap<String, String>> {
    Ok(folder_thumbnail_files(&app, &folder_id)
        .await?
        .into_iter()
        .filter_map(|(id, file)| {
            let name = file.file_name()?.to_str()?.to_string();
            Some((id, drive_thumb_src(&name)))
        })
        .collect())
}

/// Where the window loads a cached thumbnail from: the address `convertFileSrc` gives
/// `dthumb/<file>` on the `quince` scheme on this platform.
fn drive_thumb_src(file: &str) -> String {
    if cfg!(any(windows, target_os = "android")) {
        format!("http://quince.localhost/dthumb/{file}")
    } else {
        format!("quince://localhost/dthumb/{file}")
    }
}

/// The cached thumbnail a `quince://` request asks for: `dthumb/<file>`, a name in the
/// thumbnail cache — letters, digits, `_` and `-` only, so never a path anywhere else.
fn drive_thumb_part(path: &str) -> Option<String> {
    let path = percent_decode(path.trim_start_matches('/'));
    let file = path.strip_prefix("dthumb/")?;
    (!file.is_empty()
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then(|| file.to_string())
}

/// A cached thumbnail's bytes, for the `quince://` handler. The name holds the file's version,
/// so what is served under one name never changes.
async fn drive_thumb_shown(app: &tauri::AppHandle, file: &str) -> Result<Vec<u8>, String> {
    let bytes = tokio::fs::read(state_dir(app).join("thumbnails").join(file))
        .await
        .map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("no picture".into());
    }
    Ok(bytes)
}

/// Apple's pictures of a folder's files, by item id: for the iCloud Drive folder's thumbnails
/// (mount.rs), from the same cache as the window's grid.
pub(crate) async fn folder_thumbnails(
    app: &tauri::AppHandle,
    folder_id: &str,
) -> Result<std::collections::HashMap<String, Vec<u8>>, String> {
    let files = folder_thumbnail_files(app, folder_id).await?;
    tauri::async_runtime::spawn_blocking(move || {
        files
            .into_iter()
            .filter_map(|(id, file)| std::fs::read(file).ok().map(|bytes| (id, bytes)))
            .filter(|(_, bytes)| !bytes.is_empty())
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// Where in the cache `file` keeps its thumbnail: named by item and version.
fn thumbnail_file(cache: &std::path::Path, file: &quince_core::drive::DriveItem) -> PathBuf {
    let safe = |s: &str| {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect::<String>()
    };
    cache.join(format!(
        "{}-{}",
        safe(&file.drivewsid),
        safe(file.etag.as_deref().unwrap_or(""))
    ))
}

/// Which of `files` have a picture in the cache (by id, where it is), and which were never
/// asked about at this version. One empty file records "Apple has none": in neither.
fn cached_thumbnails(
    cache: &std::path::Path,
    files: &[quince_core::drive::DriveItem],
) -> (
    std::collections::HashMap<String, PathBuf>,
    Vec<quince_core::drive::DriveItem>,
) {
    let mut found = std::collections::HashMap::new();
    let mut missing = Vec::new();
    for f in files.iter().filter(|f| f.kind == "FILE") {
        let at = thumbnail_file(cache, f);
        match std::fs::metadata(&at) {
            Ok(m) if m.len() == 0 => {} // known to have none
            Ok(_) => {
                found.insert(f.drivewsid.clone(), at);
            }
            Err(_) => missing.push(f.clone()),
        }
    }
    (found, missing)
}

/// Apple's pictures of a folder's files, by item id, as files in the cache: those already
/// there, and those not yet asked about fetched from Apple once and kept.
async fn folder_thumbnail_files(
    app: &tauri::AppHandle,
    folder_id: &str,
) -> Result<std::collections::HashMap<String, PathBuf>, String> {
    let files = app
        .state::<Data>()
        .0
        .read(|library| library.drive_children(folder_id))
        .await
        .map_err(|e| e.to_string())?;
    let cache = state_dir(app).join("thumbnails");
    // Looked over off the async threads: a folder of thousands is that many looks at the disk.
    let (mut out, missing) = {
        let cache = cache.clone();
        tauri::async_runtime::spawn_blocking(move || {
            std::fs::create_dir_all(&cache).ok();
            cached_thumbnails(&cache, &files)
        })
        .await
        .map_err(|e| e.to_string())?
    };
    if !missing.is_empty() {
        let (http, docws, params) = {
            let auth = app.state::<Auth>();
            let client = auth.client.lock().await;
            (
                client.http().clone(),
                client
                    .session()
                    .service_url("docws")
                    .ok_or("Apple hasn't offered iCloud Drive for this account.")?,
                client.service_params(),
            )
        };
        let got = quince_icloud::drive::thumbnails(&http, &docws, params, &missing)
            .await
            .map_err(|e| e.to_string())?;
        for f in &missing {
            let bytes = got
                .iter()
                .find(|(id, _)| id == &f.drivewsid)
                .map(|(_, b)| b.clone());
            let at = thumbnail_file(&cache, f);
            // An empty file records "Apple has no picture for this version", so it is not asked again.
            let written = write_whole(&at, bytes.as_deref().unwrap_or(&[])).await;
            if bytes.is_some() && written.is_ok() {
                out.insert(f.drivewsid.clone(), at);
            }
        }
    }
    Ok(out)
}

/// A thumbnail's type, read from its first bytes.
fn thumb_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(b"RIFF") {
        "image/webp"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

/// How much iCloud storage is used and how much there is, for "X GB available".
#[tauri::command]
async fn storage_info(auth: State<'_, Auth>) -> UiResult<quince_icloud::drive::Storage> {
    let (http, setup, params) = {
        let client = auth.client.lock().await;
        (
            client.http().clone(),
            client.setup_url().to_string(),
            client.service_params(),
        )
    };
    quince_icloud::drive::storage(&http, &setup, params)
        .await
        .map_err(|e| e.to_string())
}

/// Drive's Recently Deleted, read live: Apple decides how long things stay there, so a copy
/// kept here would only ever be a wrong count.
#[tauri::command]
async fn drive_trash(auth: State<'_, Auth>) -> UiResult<Vec<quince_icloud::drive::TrashItem>> {
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::trash_items(&http, &root, params)
        .await
        .map_err(|e| e.to_string())
}

/// The files opened most recently, newest first — Drive's Recents.
#[tauri::command]
async fn drive_recents(auth: State<'_, Auth>) -> UiResult<Vec<quince_icloud::drive::RecentItem>> {
    let (http, root, params) = {
        let client = auth.client.lock().await;
        let root = client
            .session()
            .service_url("docws")
            .ok_or("Apple hasn't offered Recents for this account.")?;
        (client.http().clone(), root, client.service_params())
    };
    quince_icloud::drive::recents(&http, &root, params)
        .await
        .map_err(|e| e.to_string())
}

/// Recover an item from Recently Deleted to where it was.
#[tauri::command]
async fn drive_put_back(
    item: quince_core::drive::DriveItem,
    auth: State<'_, Auth>,
) -> UiResult<()> {
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::put_back(&http, &root, params, &item)
        .await
        .map_err(|e| e.to_string())
}

/// Delete an item in Recently Deleted for good. The interface asks first.
#[tauri::command]
async fn drive_delete_forever(
    item: quince_core::drive::DriveItem,
    auth: State<'_, Auth>,
) -> UiResult<()> {
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::delete_forever(&http, &root, params, &item)
        .await
        .map_err(|e| e.to_string())
}

/// Recover or delete for good every one of these, in one request, as Recover All and
/// Delete All do on icloud.com. Answers what could not be done, each with why — the rest are
/// done and gone from Recently Deleted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TrashRefusal {
    drivewsid: String,
    reason: String,
}

#[tauri::command]
async fn drive_trash_out(
    items: Vec<quince_core::drive::DriveItem>,
    recover: bool,
    auth: State<'_, Auth>,
) -> UiResult<Vec<TrashRefusal>> {
    let (http, root, params) = drive_context(&auth).await?;
    let failed = if recover {
        quince_icloud::drive::put_back_many(&http, &root, params, &items).await
    } else {
        quince_icloud::drive::delete_forever_many(&http, &root, params, &items).await
    }
    .map_err(|e| e.to_string())?;
    Ok(failed
        .into_iter()
        .map(|(drivewsid, reason)| TrashRefusal { drivewsid, reason })
        .collect())
}

/// Move an item into a different folder, refreshing both the old and new location.
#[tauri::command]
async fn drive_move(
    drivewsid: String,
    destination_id: String,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
) -> UiResult<()> {
    let item = {
        let library = data.0.lock().await;
        library
            .drive_item(&drivewsid)
            .map_err(|e| e.to_string())?
            .ok_or("That item is no longer in the local library.")?
    };
    let (http, root, params) = drive_context(&auth).await?;
    quince_icloud::drive::move_to(&http, &root, params.clone(), &item, &destination_id)
        .await
        .map_err(|e| e.to_string())?;
    let old_parent = item
        .parent_id
        .as_deref()
        .unwrap_or(quince_core::drive::ROOT);
    settle(resync_drive_folder(&http, &root, params.clone(), &data, old_parent).await)?;
    if old_parent != destination_id {
        settle(resync_drive_folder(&http, &root, params, &data, &destination_id).await)?;
    }
    Ok(())
}

/// The event a finished refresh emits, so every open screen re-reads at once.
const SYNCED: &str = "quince://synced";

/// How often the library refreshes itself.
///
/// Short enough that a reminder ticked on the phone is here before it is missed, long
/// enough to stay far away from anything Apple would call hammering. Nobody ever has to
/// press refresh; the control exists for impatience, not for correctness.
const AUTO_SYNC: Duration = Duration::from_secs(5 * 60);

/// Refresh every service at once.
///
/// Never fails as a whole: the result says what each service did, so one endpoint being
/// down leaves the rest current instead of taking the refresh with it.
#[tauri::command]
async fn sync_all(
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
    asked: Option<bool>,
) -> UiResult<refresh::Refresh> {
    let client = Arc::clone(&auth.client);
    keep_signed_in(&app, &client).await;
    // The person pressed Sync Now: Advanced Data Protection's approval is asked for with it.
    // A pass the window starts by itself (opening, coming back) asks Apple nothing about it.
    if asked == Some(true) {
        data_access::check(&app, &client, data_access::Ask::Person).await;
    }
    let result = refresh::all(&client, &data.0).await;
    data_access::after_pass(&app, &client).await;
    alerts::changed();
    warm_contact_photos(app.clone());
    // Screens read the library, not the network, so they only need to be told to look again —
    // and were, service by service, as each one landed with something new.
    app.emit(SYNCED, told(&result)).ok();
    Ok(result)
}

/// Refresh only the services named — one app's "update now" — and report on those.
///
/// The event carries the whole library's freshness, not this partial report: every open
/// screen and the account's status read the event as the complete picture, and a report of
/// one service would have made the others look never synced.
#[tauri::command]
async fn sync_some(
    app: tauri::AppHandle,
    auth: State<'_, Auth>,
    data: State<'_, Data>,
    services: Vec<String>,
    asked: Option<bool>,
) -> UiResult<refresh::Refresh> {
    let client = Arc::clone(&auth.client);
    keep_signed_in(&app, &client).await;
    if asked == Some(true) {
        data_access::check(&app, &client, data_access::Ask::Person).await;
    }
    let names: Vec<&str> = services.iter().map(String::as_str).collect();
    let result = refresh::some(&client, &data.0, &names).await;
    data_access::after_pass(&app, &client).await;
    alerts::changed();
    if names.contains(&"contacts") {
        warm_contact_photos(app.clone());
    }
    // What changed was told to the screens as each service landed; this brings the freshness.
    let mut status = refresh::status(&data.0).await;
    status.changed = Some(Vec::new());
    app.emit(SYNCED, status).ok();
    Ok(result)
}

/// What the library already knows about its own freshness, with nothing fetched.
#[tauri::command]
async fn sync_status(data: State<'_, Data>) -> UiResult<refresh::Refresh> {
    Ok(refresh::status(&data.0).await)
}

/// Keep the library current on its own.
///
/// A pass that fails changes nothing about the schedule — the next one simply tries again.
fn spawn_auto_sync(app: tauri::AppHandle, client: Arc<Mutex<Client>>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(AUTO_SYNC).await;
            let signed_in = { client.lock().await.session().webservices.is_some() };
            if !signed_in {
                continue;
            }
            keep_signed_in(&app, &client).await;
            let data = app.state::<Data>();
            let result = refresh::all(&client, &data.0).await;
            data_access::after_pass(&app, &client).await;
            alerts::changed();
            warm_contact_photos(app.clone());
            app.emit(SYNCED, told(&result)).ok();
        }
    });
}

/// A finished pass as the screens are told it at the end: its report and freshness, with
/// nothing more to read again — each service that stored something said so as it landed.
fn told(result: &refresh::Refresh) -> refresh::Refresh {
    refresh::Refresh {
        changed: Some(Vec::new()),
        items: None,
        ..result.clone()
    }
}

/// Where a contact's picture is cached on disk, named by the signature Apple gives it so a
/// changed picture lands at a new path rather than behind a stale one.
/// The edge of a served avatar.
///
/// Drawn at 26px in a list and 68px on a card, so 256 covers both on a display of any density
/// while staying a few kilobytes rather than the few hundred Apple stores.
const AVATAR_PX: u32 = 256;

/// What a picture actually is, read from its own first bytes.
///
/// Apple serves contact pictures as PNG today and said nothing about keeping to it, and a
/// cached copy on disk carries no header to consult — so the bytes are asked directly rather
/// than a type being assumed. An unrecognised one is still handed over: the webview sniffs
/// too, and the interface already falls back to initials if it cannot draw it.
fn image_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "image/heic",
        _ => "application/octet-stream",
    }
}

fn photo_path(dir: &std::path::Path, guid: &str, signature: &str) -> PathBuf {
    // The signature is base64 and can hold characters a filename cannot.
    let safe: String = signature
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let guid_safe: String = guid
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // No extension: the format is Apple's to choose, and is read back from the bytes.
    // "faces": the folder before held pictures cut with the crop read upside down, and is
    // never read again.
    dir.join("faces").join(format!("{guid_safe}-{safe}"))
}

/// A file just written renamed to its own name, waiting while Windows holds it — the virus
/// scanner looks at each new file as it is closed, and a rename meanwhile is refused
/// (quince_core::files). Off the async threads: the wait is a sleep.
pub(crate) async fn move_into_place(
    from: &std::path::Path,
    to: &std::path::Path,
) -> std::io::Result<()> {
    let (from, to) = (from.to_path_buf(), to.to_path_buf());
    tokio::task::spawn_blocking(move || quince_core::files::rename(&from, &to))
        .await
        .map_err(std::io::Error::other)?
}

/// Write `bytes` to `path` whole or not at all: to a file of its own beside it, then renamed
/// over it. Written in place, a crash or a full disk part way left a cut-off picture that was
/// served from then on — the read side only asks whether the file is there.
async fn write_whole(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let part = path.with_file_name(format!(
        ".{}.{}.part",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let written = async {
        tokio::fs::write(&part, bytes).await?;
        tokio::fs::rename(&part, path).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    written
}

/// A contact's earlier pictures, once a new one is kept: one file per signature, and every
/// changed picture used to leave the last one behind for good.
fn forget_older_faces(kept: &std::path::Path) {
    let (Some(dir), Some(name)) = (kept.parent(), kept.file_name().and_then(|n| n.to_str())) else {
        return;
    };
    // `<guid>-<signature>`, and the signature can hold dashes of its own: the guid is told by
    // its length. A name not of that shape is left alone.
    let Some(guid) = guid_of_face(name) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let other = e.file_name();
        let Some(other) = other.to_str() else {
            continue;
        };
        if other != name && guid_of_face(other) == Some(guid) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The contact part of a face file's name (see [`photo_path`]): a contact's guid is a UUID,
/// 36 characters.
fn guid_of_face(name: &str) -> Option<&str> {
    const UUID: usize = 36;
    (name.len() > UUID + 1 && name.as_bytes()[UUID] == b'-').then(|| &name[..UUID])
}

/// Serve a contact's picture to the interface.
///
/// Through a scheme of our own rather than over IPC: the webview loads it the way it loads
/// any image, into its own cache, so a list of a hundred people does not push a hundred
/// images through a message channel on every scroll. Fetched from Apple once and kept on
/// disk after that.
/// A picture ready to serve, and whether it is safe to let the webview keep it.
struct Photo {
    bytes: Vec<u8>,
    durable: bool,
}

async fn contact_photo(
    app: &tauri::AppHandle,
    guid: &str,
) -> Result<Photo, Box<dyn std::error::Error + Send + Sync>> {
    let photo = {
        let data = app.state::<Data>();
        let library = data.0.lock().await;
        library
            .contact_photo_source(guid)?
            .ok_or("that contact has no picture")?
    };

    // A picture shared by the person it belongs to arrives with no signature, and Apple's
    // own preference on those is that they update themselves. With nothing to tell a cached
    // copy it has gone stale, none is kept: a stale face is worse than a second fetch.
    let dir = state_dir(app);
    let cached = photo
        .signature
        .as_deref()
        .map(|signature| photo_path(&dir, guid, signature));
    let durable = cached.is_some();
    if let Some(path) = &cached {
        if let Ok(bytes) = tokio::fs::read(path).await {
            return Ok(Photo { bytes, durable });
        }
    }

    // A picture chosen here and not yet at Apple is the image itself, as icloud.com sends it.
    let bytes = if let Some(data) = photo.url.strip_prefix("data:") {
        use base64::Engine;
        let (_, encoded) = data
            .split_once(',')
            .ok_or("that picture could not be read")?;
        base64::engine::general_purpose::STANDARD.decode(encoded)?
    } else {
        let (http, params) = {
            let auth = app.state::<Auth>();
            let client = auth.client.lock().await;
            (client.http().clone(), client.service_params())
        };
        let response = http.get(&photo.url).query(&params).send().await?;
        if !response.status().is_success() {
            return Err(format!("Apple returned {}", response.status()).into());
        }
        response.bytes().await?.to_vec()
    };

    // Cut to the square its owner chose and sized for a face, off the interface's thread.
    // Apple stores these whole — one of them is 1696x2528 — and a list of them has to scroll.
    let crop = photo.crop.as_deref().and_then(quince_media::Crop::parse);
    let drawn = tokio::task::spawn_blocking(move || quince_media::avatar(&bytes, crop, AVATAR_PX))
        .await?
        .map_err(|e| format!("that picture could not be read: {e}"))?;

    if let Some(path) = &cached {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        if write_whole(path, &drawn).await.is_ok() {
            forget_older_faces(path);
        }
    }
    Ok(Photo {
        bytes: drawn,
        durable,
    })
}

/// Fetch, in the background, every contact photo not yet on disk, so a face is already here
/// when its row is drawn rather than fetched from Apple as it scrolls into view. Three at a
/// time; one that fails is simply tried again after the next sync.
pub fn warm_contact_photos(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Who has a picture, and which — not every card whole with every method, under the
        // lock, after every pass — and then, with the library let go, which are not on disk.
        let pictured = app
            .state::<Data>()
            .0
            .read(Library::contact_photo_signatures)
            .await
            .unwrap_or_default();
        let dir = state_dir(&app);
        let wanted: Vec<String> = tauri::async_runtime::spawn_blocking(move || {
            pictured
                .into_iter()
                .filter(|(guid, sig)| !photo_path(&dir, guid, sig).exists())
                .map(|(guid, _)| guid)
                .collect()
        })
        .await
        .unwrap_or_default();
        use futures_util::StreamExt;
        futures_util::stream::iter(wanted)
            .for_each_concurrent(3, |guid| {
                let app = app.clone();
                async move {
                    let _ = contact_photo(&app, &guid).await;
                }
            })
            .await;
    });
}

/// Where this installation's writer id is kept.
const WRITER_KEY: &str = "documents.writer";

/// Give note and reminder edits this installation's own writer id.
///
/// Apple's devices merge a note by who wrote which characters, so every edit made here is
/// signed. The same id every run keeps a note's list of writers from growing by one each time
/// the app starts; a different id on every computer keeps two of them from numbering
/// characters the same way. If it cannot be stored, a fresh one for this run is still correct.
fn adopt_writer(library: &mut Library) {
    let stored = library
        .state(WRITER_KEY)
        .ok()
        .flatten()
        .and_then(|s| uuid::Uuid::parse_str(&s).ok());
    let id = stored.unwrap_or_else(|| {
        let fresh = uuid::Uuid::new_v4();
        let _ = library.set_state(WRITER_KEY, &fresh.to_string());
        fresh
    });
    quince_icloud::topotext::set_writer(*id.as_bytes());
}

/// Memory freed after work goes back to the system instead of staying with the process.
///
/// glibc gives each busy thread its own arena and keeps whatever an arena ever held. Preparing
/// photos decodes each on a blocking thread, and a decoded photo is about 23 MB, so every one of
/// the twelve threads on a 12-core machine kept one photo's worth after the work ended: measured
/// (issue #11), the engine stood at 408 MB with 33 MB of it in use, and never came down.
/// `malloc_trim` won't fix that after the fact: it gave back 19 of the 280 MB. Prevented instead:
///
/// - two arenas, so freed memory is reused rather than spread over a dozen;
/// - anything of 4 MB or more (a decoded photo, a resize buffer) mapped on its own and given back
///   the moment it is freed, never left in an arena; glibc's own threshold rises to 32 MB after
///   the first such free, which put every later photo in an arena;
/// - the top of the heap returned once 8 MB of it is free.
///
/// The same photo work then held 195 MB, and prepared 302 photos in two minutes against 275:
/// the decode buffers a thread waited on before are mapped fresh, which is cheaper than it was.
/// Set first in `main`, before any thread exists, since arenas are made as threads first
/// allocate. A `MALLOC_*` setting of the person's own (glibc reads those before `main`) wins.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn return_freed_memory() {
    let mine = |name: &str| std::env::var_os(name).is_some();
    // SAFETY: mallopt only adjusts glibc's allocator settings; called before any other thread.
    unsafe {
        if !mine("MALLOC_ARENA_MAX") {
            libc::mallopt(libc::M_ARENA_MAX, 2);
        }
        if !mine("MALLOC_MMAP_THRESHOLD_") {
            libc::mallopt(libc::M_MMAP_THRESHOLD, 4 << 20);
        }
        if !mine("MALLOC_TRIM_THRESHOLD_") {
            libc::mallopt(libc::M_TRIM_THRESHOLD, 8 << 20);
        }
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn return_freed_memory() {}

/// How WebKitGTK hands its frames to the window. Three ways, measured on his Iris Xe at his
/// 1920×1080 (grid scroll / zoomed pan):
///
/// - GPU buffers (DMA-BUF) straight to the compositor: 59 / 60 fps, but on Hyprland the frames
///   arrive out of order — in his recordings the grid and the open viewer alternate frame by
///   frame, a flicker on every open, close and resize;
/// - no GPU hand-off at all (`WEBKIT_DISABLE_DMABUF_RENDERER`): steady, but 38 / 34 fps;
/// - drawn on the GPU, handed over in shared memory (`WEBKIT_DMABUF_RENDERER_FORCE_SHM`):
///   58 / 59 fps, and the frames reach the window in order.
///
/// The last, unless the person has chosen: any WebKit setting of their own is left alone, and
/// `QUINCE_DMABUF=1` keeps GPU buffers, `QUINCE_DMABUF=0` turns the GPU hand-off off.
fn steady_drawing() {
    let set = |k: &str| std::env::var_os(k).is_some();
    if set("WEBKIT_DISABLE_DMABUF_RENDERER") || set("WEBKIT_DMABUF_RENDERER_FORCE_SHM") {
        return;
    }
    // Before any thread starts: nothing else can be reading the environment yet.
    match std::env::var("QUINCE_DMABUF").as_deref() {
        Ok("1") => {}
        Ok("0") => std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
        _ => std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1"),
    }
}

/// Who paints the page's pixels when there is no GPU to paint them. WebKitGTK paints on the GPU
/// through OpenGL; with no GPU (a virtual machine's plain display, a computer whose driver gives
/// no render node) Mesa answers OpenGL on the processor instead (llvmpipe), and every frame
/// waits for it. WebKit's own painter on the processor (Skia, spread over every core) is far
/// quicker there. Measured in a QEMU guest on llvmpipe, 1280×800, WebKitGTK 2.54, a mouse
/// wheel spun over each list for 2.4 s, two runs each, frames per second and the slowest 5%:
///
/// - Photos: 29 → 47 fps, 183 → 63 ms;
/// - Drive's list: 48 → 57 fps, 40 → 26 ms;
/// - Reminders' list of 1,000: 39 → 43 fps, 52 → 47 ms; Notes' list: 59 fps either way.
///
/// GPU buffers (DMA-BUF) gain nothing there: with no GPU, they are drawn by llvmpipe too.
/// Only where nothing draws on a GPU: with a render node, GPU painting stays (59 fps on the
/// Iris Xe measured above). The person's own `WEBKIT_SKIA_ENABLE_CPU_RENDERING` is left
/// alone: `=0` keeps GPU painting even here, `=1` uses the processor everywhere.
#[cfg(target_os = "linux")]
fn paint_without_gpu() {
    if std::env::var_os("WEBKIT_SKIA_ENABLE_CPU_RENDERING").is_some() {
        return;
    }
    let env = |k: &str| std::env::var(k).ok();
    let render_node = std::fs::read_dir("/dev/dri").is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
    });
    if !gpu_to_paint_on(
        render_node,
        env("LIBGL_ALWAYS_SOFTWARE").as_deref(),
        env("GALLIUM_DRIVER").as_deref(),
    ) {
        // Before any thread starts: nothing else can be reading the environment yet.
        std::env::set_var("WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1");
    }
}

#[cfg(not(target_os = "linux"))]
fn paint_without_gpu() {}

/// Whether OpenGL here reaches a GPU: a DRM render node to open, and Mesa not told to answer
/// on the processor (`LIBGL_ALWAYS_SOFTWARE`, or a software Gallium driver).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gpu_to_paint_on(
    render_node: bool,
    always_software: Option<&str>,
    gallium: Option<&str>,
) -> bool {
    let software = always_software.is_some_and(|v| !v.is_empty() && v != "0" && v != "false")
        || gallium.is_some_and(|d| matches!(d, "llvmpipe" | "softpipe" | "swrast"));
    render_node && !software
}

/// WebKitGTK's player on its older pipeline (playbin) buffers a video read over HTTP to disk
/// and then stalls: "Pausing stream for buffering" after the whole file has arrived, the
/// picture frozen on its first frame. Its newer pipeline (playbin3, WebKit's own switch) plays
/// and seeks the same file smoothly — proven on this machine's WebKitGTK 2.54. Only when the
/// person has not chosen themselves.
fn smooth_video() {
    if std::env::var_os("WEBKIT_GST_USE_PLAYBIN3").is_none() {
        // Before any thread starts: nothing else can be reading the environment yet.
        std::env::set_var("WEBKIT_GST_USE_PLAYBIN3", "1");
    }
}

/// The window's own colour, set to what the page paints at its edges (see src/lib/tint.ts):
/// what shows for the frames a resize runs ahead of the page.
#[tauri::command]
fn window_tint(window: tauri::WebviewWindow, rgb: [u8; 3]) -> UiResult<()> {
    window
        .set_background_color(Some(tauri::window::Color(rgb[0], rgb[1], rgb[2], 255)))
        .map_err(|e| e.to_string())
}

/// The window full screen, or back, as the photo viewer's button asks (icloud.com's); answers
/// whether it now is.
#[tauri::command]
fn window_fullscreen(window: tauri::WebviewWindow, on: bool) -> UiResult<bool> {
    window.set_fullscreen(on).map_err(|e| e.to_string())?;
    window.is_fullscreen().map_err(|e| e.to_string())
}

/// Whether this session runs under a tiling Wayland compositor, which manages windows itself.
pub(crate) fn tiling_compositor() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if set("HYPRLAND_INSTANCE_SIGNATURE") || set("SWAYSOCK") || set("NIRI_SOCKET") {
        return true;
    }
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| {
        d.split(':').any(|name| {
            [
                "hyprland", "sway", "niri", "river", "i3", "qtile", "dwl", "wayfire",
            ]
            .contains(&name.to_ascii_lowercase().as_str())
        })
    })
}

/// The window, shown once its first frame is drawn. It is made hidden, so a launch never shows
/// an empty or half-drawn window that then flickers into the app: the page says when it has
/// painted, and the window appears whole. Shown anyway after a few seconds, so a page that
/// never says so still leaves a window to use.
#[tauri::command]
fn window_ready(app: tauri::AppHandle, window: tauri::WebviewWindow, needs_you: Option<bool>) {
    timing::mark(format_args!("window_ready {}", window.label()));
    // An app's own window (Notes, Reminders…): shown once it has drawn.
    if window.label() != "main" {
        apps::show(&window);
        return;
    }
    // Opened as one app from its launcher entry: the whole-library window stays put away —
    // unless it was asked for since, and made for that (show_window).
    if LAUNCHED_AS_APP.load(Ordering::SeqCst) && !MAIN_ASKED.load(Ordering::SeqCst) {
        return;
    }
    // Started at login, with nothing for the person to do: everything runs, the window waits
    // in the tray.
    // Never without the tray to come back from.
    // Asked in this order: started any other way the answer is no, and the tray's question —
    // a call on the session bus — was asked on the main thread just before every launch's show.
    if login::stay_hidden(needs_you.unwrap_or(true), || tray_reachable(&app)) {
        // Its page has done what a launch needs of it (the session taken up again): put away,
        // as a closed window is — opened from the tray soon, it is there at once; otherwise
        // its web view is given back and the engine runs on behind the tray (#11).
        apps::put_away(&window.as_ref().window());
        return;
    }
    show_window(&app);
}

/// Started from one app's launcher entry (`--app notes`): that app's window is what shows.
static LAUNCHED_AS_APP: AtomicBool = AtomicBool::new(false);

/// Quince ended, the one way every exit takes — Quit in the tray, Ctrl-C, a logout's signal,
/// the last window closed: the window gone at once, no more work started ahead, the folders
/// let go of (never waited on past three seconds), then the process ended on the spot.
///
/// Ended through `exit()`, the libraries' own teardown ran while a worker was still decoding a
/// HEIC — libheif unloaded under it, and Quince crashed on the way out (his, 2026-10-20: SIGSEGV
/// in libheif, main thread in exit). Nothing needs that teardown: every change is in the
/// library the moment it is made (SQLite commits as it goes), and what waits to be sent is
/// written down as it waits.
fn shutdown(app: &tauri::AppHandle, code: i32) -> ! {
    wind_down(app);
    // SAFETY: ends the process at once; nothing after it runs.
    unsafe { libc::_exit(code) }
}

/// What ending Skykeep does first, however it ends — Quit, a signal, or an update restarting
/// it: once, however many of them arrive.
fn wind_down(app: &tauri::AppHandle) {
    static ENDING: AtomicBool = AtomicBool::new(false);
    if !ENDING.swap(true, Ordering::SeqCst) {
        // Where each window was, kept before the quick exit below skips the usual teardown.
        {
            use tauri_plugin_window_state::AppHandleExt;
            app.save_window_state(window_state_kept()).ok();
        }
        for w in app.webview_windows().values() {
            w.hide().ok();
        }
        cpu::stop();
        mount::release_within(Duration::from_secs(3));
    }
}

/// What is kept of a window between runs: its size, place and maximised state — never whether
/// it shows.
fn window_state_kept() -> tauri_plugin_window_state::StateFlags {
    use tauri_plugin_window_state::StateFlags;
    StateFlags::all() - StateFlags::VISIBLE
}

fn show_when_ready(app: &tauri::AppHandle) {
    // Started at login: the window is the first window_ready's to show, or not. Started as one
    // app: that app's window shows instead.
    if login::background() || login::search_only() || LAUNCHED_AS_APP.load(Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if let Some(w) = app.get_webview_window("main") {
            if !w.is_visible().unwrap_or(true) {
                show_window(&app);
            }
        }
    });
}

/// The whole-library window was asked for after a launch as one app, which does not make it.
static MAIN_ASKED: AtomicBool = AtomicBool::new(false);

/// The whole-library window, made hidden (tauri.conf.json's "main"; shown by its window_ready
/// once drawn). Not made at startup by Tauri: a launch as one app (`--app notes`) built it
/// anyway, a second web view loading the whole library, resuming the session and starting a
/// refresh behind the one window asked for — contending with it for every step of its launch.
fn make_main(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(window) = app.get_webview_window("main") {
        return Some(window);
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")?
        .clone();
    let window = match tauri::WebviewWindowBuilder::from_config(app, &config)
        .and_then(tauri::WebviewWindowBuilder::build)
    {
        Ok(window) => window,
        Err(e) => {
            quiet!("the window could not be made: {e}");
            startup_guard::no_window(&e);
            return None;
        }
    };
    timing::mark("main made");
    // A tiling compositor places, sizes and closes windows itself; a title bar with minimise
    // and maximise there only takes room and does nothing it does not. On a desktop that
    // floats windows — GNOME, KDE — it is how a window is moved, so it stays.
    if tiling_compositor() {
        window.set_decorations(false).ok();
    }
    Some(window)
}

/// Bring the window back, wherever it went; made first if this launch had not made it. Answers
/// whether it had to be made — its page is then still loading, not yet listening.
fn show_window(app: &tauri::AppHandle) -> bool {
    login::shown();
    if let Some(window) = app.get_webview_window("main") {
        timing::mark("main shown");
        window.show().ok();
        window.unminimize().ok();
        window.set_focus().ok();
        return false;
    }
    MAIN_ASKED.store(true, Ordering::SeqCst);
    let Some(window) = make_main(app) else {
        return false;
    };
    // Shown by its window_ready once drawn; shown anyway if that never comes.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if !window.is_visible().unwrap_or(true) {
            window.show().ok();
            window.set_focus().ok();
        }
    });
    true
}

/// `then`, once the whole-library window can hear it: at once if it was open, or once a window
/// just made has drawn (its page is up, and listening, by then) — never sent to a page still
/// loading, which would drop it.
pub(crate) fn show_window_then(
    app: &tauri::AppHandle,
    then: impl FnOnce(&tauri::AppHandle) + Send + 'static,
) {
    if !show_window(app) {
        then(app);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < until {
            if app
                .get_webview_window("main")
                .is_some_and(|w| w.is_visible().unwrap_or(false))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        then(&app);
    });
}

/// The web views' memory cache, sized for an app rather than a browser. WebKitGTK's default
/// (the "web browser" model) keeps up to half a gigabyte of decoded pictures on a 16 GB
/// machine, for pages a browser expects to be revisited. Here every picture is this app's own,
/// served from disk in a few milliseconds by the `quince` scheme, so keeping them gained
/// nothing: measured on 1,589 photos (the Hidden album), one scroll through left the web
/// process 650 MB larger, and it stayed so after leaving Photos. The "document viewer" model
/// keeps what is on screen and near it, gives the rest back, and keeps no disk cache of a
/// page that is never fetched over the network. Set on the one web context every window
/// shares, so once is enough; each new window sets it again harmlessly.
#[cfg(target_os = "linux")]
fn app_cache(webview: &tauri::Webview) {
    let done = webview.with_webview(|platform| {
        use webkit2gtk::{WebContextExt, WebViewExt};
        if let Some(context) = platform.inner().context() {
            context.set_cache_model(webkit2gtk::CacheModel::DocumentViewer);
        }
    });
    if let Err(e) = done {
        quiet!("the web view's cache could not be set: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
fn app_cache(_webview: &tauri::Webview) {}

/// Whether the tray icon can be seen, so a closed window can be put away behind it. On Linux a
/// tray icon is a StatusNotifierItem, shown only where a host is on the session bus (KDE's
/// panel, waybar, Quickshell, Ubuntu's AppIndicator extension…). Stock GNOME (Fedora, Debian)
/// has none: the icon is made and never shown, and a window put away there left Skykeep running
/// with no way back but its launcher and no way to quit at all. Where no host is there, closing
/// the window quits, as it does where no tray could be made.
///
/// Asked when it matters rather than once at start, because a panel started at login can
/// register after Skykeep. A bus that cannot be asked is taken as a tray that is there.
#[cfg(target_os = "linux")]
fn tray_reachable(_app: &tauri::AppHandle) -> bool {
    linux_tray::made() && TRAY_HOST.reachable(|| status_notifier_host().unwrap_or(true))
}

/// Whether a tray host is there, as last asked. It is asked where a window shows or closes —
/// on the main thread, where a call on the session bus froze every window for as long as it
/// took (up to a second) — so the answer kept is used there, and asked again behind it for the
/// next time. A panel coming or going is heard of from ksni too (`watcher_online`/`_offline`).
/// Only the very first question, with nothing kept yet, waits for its answer.
#[cfg(target_os = "linux")]
static TRAY_HOST: HostSeen = HostSeen::new();

#[cfg(target_os = "linux")]
struct HostSeen {
    /// 0 not asked yet, 1 no host, 2 a host.
    seen: std::sync::atomic::AtomicU8,
    asking: AtomicBool,
}

#[cfg(target_os = "linux")]
impl HostSeen {
    const fn new() -> Self {
        Self {
            seen: std::sync::atomic::AtomicU8::new(0),
            asking: AtomicBool::new(false),
        }
    }

    fn keep(&self, host: bool) {
        self.seen.store(if host { 2 } else { 1 }, Ordering::SeqCst);
    }

    /// The answer kept, asked again in the background; asked now only when none is kept.
    fn reachable(&'static self, ask: fn() -> bool) -> bool {
        match self.seen.load(Ordering::SeqCst) {
            0 => {
                let host = ask();
                self.keep(host);
                host
            }
            seen => {
                self.ask_again(ask);
                seen == 2
            }
        }
    }

    /// Asked off the main thread, one question at a time.
    fn ask_again(&'static self, ask: fn() -> bool) {
        if self.asking.swap(true, Ordering::SeqCst) {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("tray-host".into())
            .spawn(move || {
                self.keep(ask());
                self.asking.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.asking.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn tray_reachable(app: &tauri::AppHandle) -> bool {
    app.tray_by_id("quince").is_some()
}

/// Whether a StatusNotifierItem host is registered: `None` when the session bus cannot be
/// asked, `Some(false)` when no watcher, or no host behind it, is there.
#[cfg(target_os = "linux")]
fn status_notifier_host() -> Option<bool> {
    let bus = zbus::blocking::connection::Builder::session()
        .ok()?
        .method_timeout(Duration::from_secs(1))
        .build()
        .ok()?;
    let reply = bus.call_method(
        Some("org.kde.StatusNotifierWatcher"),
        "/StatusNotifierWatcher",
        Some("org.freedesktop.DBus.Properties"),
        "Get",
        &(
            "org.kde.StatusNotifierWatcher",
            "IsStatusNotifierHostRegistered",
        ),
    );
    let reply = match reply {
        Ok(reply) => reply,
        // No watcher on the bus at all: nothing shows tray icons here.
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
        {
            return Some(false)
        }
        Err(_) => return None,
    };
    let value: zbus::zvariant::OwnedValue = reply.body().deserialize().ok()?;
    Some(bool::try_from(value).unwrap_or(false))
}

/// The tray on Linux: a StatusNotifierItem spoken over D-Bus by ksni, not Tauri's tray.
/// Tauri's goes through libayatana-appindicator, which never passes a click on — a left click
/// could only open the menu. Here a left click (the host's Activate) opens the window at once,
/// and the right click's menu is the one Windows and macOS have.
///
/// Made even where no host is there yet (`assume_sni_available`): a panel that starts after
/// Skykeep at login picks the icon up when it registers. Where none ever does (stock GNOME),
/// `tray_reachable` says so, and closing the window quits.
#[cfg(target_os = "linux")]
mod linux_tray {
    use super::{intake, show_window, shutdown};
    use ksni::TrayMethods;
    use std::sync::atomic::{AtomicBool, Ordering};

    static MADE: AtomicBool = AtomicBool::new(false);

    /// Whether the tray's D-Bus service is up.
    pub fn made() -> bool {
        MADE.load(Ordering::SeqCst)
    }

    struct SkykeepTray {
        app: tauri::AppHandle,
        icon: Vec<ksni::Icon>,
    }

    impl SkykeepTray {
        /// `then` on the main thread, where the windows live; the tray's calls arrive on
        /// ksni's own.
        fn on_main(&self, then: fn(&tauri::AppHandle)) {
            let app = self.app.clone();
            self.app.run_on_main_thread(move || then(&app)).ok();
        }
    }

    impl ksni::Tray for SkykeepTray {
        fn id(&self) -> String {
            "skykeep".into()
        }
        fn title(&self) -> String {
            "Skykeep".into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Skykeep".into(),
                ..Default::default()
            }
        }
        // The left click.
        fn activate(&mut self, _x: i32, _y: i32) {
            self.on_main(|app| {
                show_window(app);
            });
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            use ksni::MenuItem;
            let item = |label: &str, then: fn(&tauri::AppHandle)| -> MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |tray: &mut Self| tray.on_main(then)),
                    ..Default::default()
                }
                .into()
            };
            // The same four lines as Tauri's tray on Windows and macOS (build_tray).
            vec![
                item("Open Skykeep", |app| {
                    show_window(app);
                }),
                MenuItem::Separator,
                item("New Reminder", |app| intake::capture(app, "reminder")),
                item("New Note", |app| intake::capture(app, "note")),
                MenuItem::Separator,
                item("Quit Skykeep", |app| shutdown(app, 0)),
            ]
        }
        // A panel restarting, or not there yet: the icon comes back when it does. Either way
        // whether a host is there is asked again, so closing the window knows (TRAY_HOST).
        fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
            super::TRAY_HOST.ask_again(|| super::status_notifier_host().unwrap_or(true));
            true
        }
        fn watcher_online(&self) {
            super::TRAY_HOST.ask_again(|| super::status_notifier_host().unwrap_or(true));
        }
    }

    /// The tray's mark at every size a panel draws one, each made from the vector master with
    /// a margin all round (as Vesktop's), as StatusNotifierItem wants it: ARGB32, in network
    /// byte order. Given only 64px, a panel shrank it itself — soft, and the k clipped at an
    /// edge it rounded off.
    fn pixmap() -> Vec<ksni::Icon> {
        let sizes: [&[u8]; 6] = [
            include_bytes!("../icons/tray-16.png"),
            include_bytes!("../icons/tray-22.png"),
            include_bytes!("../icons/tray-24.png"),
            include_bytes!("../icons/tray-32.png"),
            include_bytes!("../icons/tray-48.png"),
            include_bytes!("../icons/tray-64.png"),
        ];
        sizes
            .iter()
            .filter_map(|bytes| tauri::image::Image::from_bytes(bytes).ok())
            .map(|image| ksni::Icon {
                width: image.width() as i32,
                height: image.height() as i32,
                data: image
                    .rgba()
                    .chunks_exact(4)
                    .flat_map(|p| [p[3], p[0], p[1], p[2]])
                    .collect(),
            })
            .collect()
    }

    /// The tray's handle, held so ksni's service keeps a reason to wait (see `spawn`).
    static HANDLE: std::sync::OnceLock<ksni::Handle<SkykeepTray>> = std::sync::OnceLock::new();

    pub fn spawn(app: tauri::AppHandle) {
        let tray = SkykeepTray {
            app,
            icon: pixmap(),
        };
        // Inside the Flatpak sandbox the per-process bus name the spec asks for cannot be
        // owned; the watcher takes the item by its unique name instead.
        let sandboxed = std::path::Path::new("/.flatpak-info").exists();
        tauri::async_runtime::spawn(async move {
            match tray
                .disable_dbus_name(sandboxed)
                .assume_sni_available(true)
                .spawn()
                .await
            {
                // Kept for as long as the app runs. Dropped, it closed ksni's command channel,
                // and when the session bus then went too (logging out, a session ending) both
                // of ksni's waits were over and its loop panicked ("all futures in select!
                // were completed").
                Ok(handle) => {
                    let _ = HANDLE.set(handle);
                    MADE.store(true, Ordering::SeqCst);
                }
                Err(e) => quiet!("no tray on this desktop, so closing the window quits: {e}"),
            }
        });
    }
}

#[cfg(target_os = "linux")]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    linux_tray::spawn(app.handle().clone());
    // Asked now, off the main thread, so the first window shown or closed has its answer kept.
    TRAY_HOST.ask_again(|| status_notifier_host().unwrap_or(true));
    Ok(())
}

/// A place in the tray, so Quince stays running — and ringing reminders — with its window
/// closed, and can be opened again or quit from there.
#[cfg(not(target_os = "linux"))]
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::PredefinedMenuItem;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    // Four plain lines, nothing nested: the window, the two quick notes a person reaches for
    // from anywhere, and Quit. A submenu was slow to draw in the panel and one more step;
    // each app has its own launcher and is a click away in the window.
    let open = MenuItem::with_id(app, "open", "Open Skykeep", true, None::<&str>)?;
    let new_reminder = MenuItem::with_id(app, "new:reminder", "New Reminder", true, None::<&str>)?;
    let new_note = MenuItem::with_id(app, "new:note", "New Note", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let line2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Skykeep", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &line, &new_reminder, &new_note, &line2, &quit],
    )?;
    let mut tray = TrayIconBuilder::with_id("quince")
        .tooltip("Skykeep")
        .menu(&menu)
        // A click opens the window, as a tray icon does on Windows (and in most Linux panels);
        // the menu is the right click's. A Mac menu-bar extra opens its menu on any click.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                show_window(app);
            }
            "quit" => shutdown(app, 0),
            "new:reminder" => intake::capture(app, "reminder"),
            "new:note" => intake::capture(app, "note"),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if !cfg!(target_os = "macos") {
                    show_window(tray.app_handle());
                }
            }
        });
    // The tray's own mark: the letters without their circle, outlined so they hold on any panel
    // or wallpaper, and sized to sit inside a circle — some trays clip their icons round, which
    // cut the k off. 64px, so every tray scales it down rather than up; the window's icon (on
    // Windows the .ico's 256px entry) shrank into a blur here.
    match tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")) {
        Ok(icon) => tray = tray.icon(icon),
        Err(_) => {
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
        }
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    // First, before any thread: how freed memory goes back to the system (#11).
    return_freed_memory();
    // The uninstaller's last word (packaging/windows/installer-hooks.nsh): iCloud Drive no
    // longer a cloud folder in File Explorer. Nothing else is started.
    #[cfg(windows)]
    if std::env::args().any(|a| a == "--forget-cloud-folders") {
        mount::forget_on_uninstall();
        return;
    }
    // Started by an update: the version it replaced finishes exiting first, so this launch is
    // not handed to it as a second one (single instance, below).
    updates::after_update();
    // On Windows, before anything else: a window WebView2 cannot draw is offered its repair,
    // never a launch that ends in nothing (startup_guard.rs).
    startup_guard::ensure_webview();
    timing::start();
    login::note_launch();
    steady_drawing();
    paint_without_gpu();
    smooth_video();
    // From an AppImage: HEIC photos through the HEVC decoder it carries (quince-media's
    // appimage.rs), set before libheif is first used and before any thread starts.
    #[cfg(target_os = "linux")]
    quince_media::appimage::use_bundled_heif_plugins();
    // After those above (they set the environment, which only a single thread may) and
    // before anything opens the library, the webview its storage, or the window-state plugin
    // its file: the folders moved from the old identifier, or — if they could not be — this
    // launch run under the old one, from them as they are (migrate.rs).
    let mut context = tauri::generate_context!();
    let current = context.config().identifier.clone();
    let identifier = migrate::at_launch(&current);
    timing::mark("identifier settled");
    if identifier != current {
        context.config_mut().identifier = identifier;
    }
    // On Windows, before any window: everything from here on is Skykeep's on the taskbar, as
    // its Start-menu shortcut is, and the jump list is written (taskbar.rs).
    #[cfg(windows)]
    taskbar::claim(&current);
    // The name the desktop knows the main window by: on Wayland GTK sends the program name as
    // the window's app ID, which was the binary's ("quince-app") — so a taskbar or dock could
    // not match it to Skykeep's launcher entry, its name or its icon. Named for the identifier,
    // it is the launcher's own ID (each app's window says its own; apps.rs).
    #[cfg(target_os = "linux")]
    {
        glib::set_prgname(Some(context.config().identifier.as_str()));
        glib::set_application_name("Skykeep");
    }
    tauri::Builder::default()
        // One Quince at a time: opening it again brings back the one already running, which
        // may be sitting in the tray with its window closed.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            timing::mark("handed a launch");
            // A file opened with Skykeep, or a quick capture (`--new reminder`): handled
            // here, in the one already running.
            if intake::arrived(app, &args, std::path::Path::new(&cwd)) {
                return;
            }
            // `--app notes` from a launcher entry: that app's window, in this same process.
            match apps::asked(&args) {
                Some(key) => apps::open(app, key),
                None => {
                    show_window(app);
                }
            }
        }))
        // Each window — the library, and every app's own — reopens at the size and place it was
        // left, as Apple's apps do. Not whether it shows: that stays Skykeep's (hidden at
        // login, shown once drawn).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_kept())
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // Skykeep's own updates (updates.rs). Driven from Rust only: the window is granted none
        // of the plugin's commands (capabilities/default.json).
        .plugin(tauri_plugin_updater::Builder::new().build())
        // The window shows Quince and nothing else. A link to anywhere outside it — a phone
        // number, an address, a web page in a note or an event — goes to the app this
        // computer uses for it. Followed in place, a `tel:` link replaced the whole window
        // with "URL can't be shown", and there was no way back. Only the kinds a person means
        // by "a link" go out (links.rs): a `file:` or `search-ms:` link in a shared note is
        // someone else's choice, and the opener would run it.
        .plugin(
            tauri::plugin::Builder::<tauri::Wry, ()>::new("links")
                .on_navigation(|webview, url| {
                    // The app's own page only: Windows serves it as tauri.localhost. Any other
                    // address on this computer (a server some other program runs on localhost)
                    // is somewhere else, opened outside; only a development build, whose page
                    // comes from the dev server, counts localhost as its own.
                    match links::navigation(url, cfg!(debug_assertions)) {
                        links::Navigation::Inside => true,
                        links::Navigation::Outside => {
                            use tauri_plugin_opener::OpenerExt;
                            if let Err(e) = webview
                                .app_handle()
                                .opener()
                                .open_url(url.as_str(), None::<&str>)
                            {
                                quiet!("a link could not be opened: {e}");
                            }
                            false
                        }
                        links::Navigation::Refused => {
                            quiet!("a link of a kind Skykeep does not open: {}", url.scheme());
                            false
                        }
                    }
                })
                .on_webview_ready(|webview| app_cache(&webview))
                .build(),
        )
        // Closing the window hides it rather than quitting, so reminders still ring. Quit is
        // in the tray. Where no tray could be made, or none is shown (tray_reachable), there
        // is nowhere to come back from, so there closing quits as it always did.
        // Files dropped on a window are the person's to hand over: granted to the commands
        // that read them (grants.rs) here, before the page hears of the drop.
        .on_webview_event(|_, event| {
            if let tauri::WebviewEvent::DragDrop(drop) = event {
                grants::dropped(drop);
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(drop) = event {
                grants::dropped(drop);
            }
            // A window gone for good: the update notice, if it showed it, moves to another.
            if let tauri::WindowEvent::Destroyed = event {
                updates::put_away(window.app_handle(), window.label());
            }
            // Coming back to a window is when a change made elsewhere is looked for at once.
            if let tauri::WindowEvent::Focused(true) = event {
                push::wake();
            }
            // The whole-library window is put away, as the engine keeps running behind the
            // tray. An app's own window is put away too, so opening it again is instant rather
            // than a new web view built from nothing — and given back for good once it has
            // gone unused for a while, so six put-away windows do not sit on memory.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !tray_reachable(window.app_handle()) {
                    return;
                }
                if window.label() == "main" {
                    api.prevent_close();
                    // Put away as the apps' windows are (apps::KEPT): reopened soon it is back
                    // at once, and after that its web view is given back. Kept for good, it
                    // held about 190 MB in the tray all day (#11).
                    window.hide().ok();
                    apps::put_away(window);
                    updates::put_away(window.app_handle(), window.label());
                } else if window.label().starts_with("app-") {
                    api.prevent_close();
                    window.hide().ok();
                    apps::put_away(window);
                    updates::put_away(window.app_handle(), window.label());
                }
            }
        })
        // Images are served, not messaged. `quince://photo/<contact id>` is loaded by the
        // webview itself, which is what keeps a scrolling list cheap.
        .register_asynchronous_uri_scheme_protocol("quince", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            // Every answer carries the same guard (links.rs): its bytes are never sniffed into
            // something active, and never run as a page of their own.
            let responder = links::Guarded(responder);
            // WebKitGTK reads `quince://photo/<id>` with "photo" as the host, while Tauri's
            // own helper produces `quince://localhost/photo/<id>`. Accept either, rather than
            // depending on which one the caller happened to build.
            let uri = request.uri();
            let wanted = photo_guid(uri.host(), uri.path());
            let attachment = note_part(uri.path());
            let note_thumb = note_thumb_part(uri.path());
            let reminder_image = reminder_image_part(uri.path());
            let drive_thumb = drive_thumb_part(uri.path());
            // A drawing's ink for a dark page, rather than a light one.
            let ink_dark = uri.path().ends_with("drawing-dark");
            // A note's picture as the note draws it, rather than whole.
            let inline = uri.path().ends_with("/inline");
            let picture = photos::requested(uri.path());
            let fit = photos::requested_fit(uri.path());
            let queued = photos::requested_upload(uri.path());
            tauri::async_runtime::spawn(async move {
                // A file on its way into Photos, small, for the upload panel.
                if let Some(id) = queued {
                    let response = match photos::upload_picture(&app, &id).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A photo's picture made the size it is shown at.
                if let Some((id, long, marked)) = fit {
                    let response = match photos::fitted(&app, &id, long).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            .header("Cache-Control", photos::fit_cache(marked))
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A photo's picture: from the disk cache, or fetched from Apple once and kept.
                if let Some((id, size)) = picture {
                    let response = match photos::picture(&app, &id, size).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", photos::content_type(&bytes, size))
                            .header("Cache-Control", "private, max-age=86400")
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A Drive file's thumbnail, from the cache on disk.
                if let Some(file) = drive_thumb {
                    let response = match drive_thumb_shown(&app, &file).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", thumb_type(&bytes))
                            .header("Cache-Control", "private, max-age=86400")
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A picture attached to a reminder.
                if let Some(id) = reminder_image {
                    let response = match reminder_image_shown(&app, &id).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            .header("Cache-Control", "private, max-age=86400")
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A note's small picture in the list: from what is kept, or none.
                if let Some(id) = note_thumb {
                    let response = match note_media_thumb(&app, &id).await {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            .header("Cache-Control", "private, max-age=86400")
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                // A note's picture, or the rendering of a document or film in it.
                if let Some((id, part)) = attachment {
                    let shown = if inline {
                        note_media_inline(&app, &id).await
                    } else {
                        note_media_shown(&app, &id, part, ink_dark).await
                    };
                    let response = match shown {
                        Ok(bytes) => tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            .header("Cache-Control", "private, max-age=86400")
                            .body(bytes),
                        Err(_) => tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new()),
                    };
                    responder.respond(response.unwrap_or_default());
                    return;
                }
                let Some(guid) = wanted else {
                    responder.respond(
                        tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new())
                            .unwrap_or_default(),
                    );
                    return;
                };
                match contact_photo(&app, &guid).await {
                    Ok(Photo { bytes, durable }) => responder.respond(
                        tauri::http::Response::builder()
                            .status(200)
                            .header("Content-Type", image_type(&bytes))
                            // A picture with a signature cannot change without the signature
                            // changing too, so the webview may hold it. One without can change
                            // under us, so it is only held long enough to scroll a list.
                            .header(
                                "Cache-Control",
                                if durable {
                                    "private, max-age=604800"
                                } else {
                                    "private, max-age=300"
                                },
                            )
                            .body(bytes)
                            .unwrap_or_default(),
                    ),
                    Err(_) => responder.respond(
                        tauri::http::Response::builder()
                            .status(404)
                            .body(Vec::new())
                            .unwrap_or_default(),
                    ),
                }
            });
        })
        .setup(|app| {
            timing::mark("setup");
            video::serve(app.handle().clone());
            let dir = state_dir(app.handle());
            keep_private(&dir);
            // A second Skykeep the session bus could not stop (there is none): it ends here,
            // before it touches the library, sends anything or mounts a folder.
            static HELD: std::sync::OnceLock<library_lock::Hold> = std::sync::OnceLock::new();
            match library_lock::hold_or_mark(&dir) {
                Ok(Some(lock)) => {
                    let _ = HELD.set(lock);
                }
                Ok(None) => {
                    eprintln!("Skykeep is already running.");
                    std::process::exit(0);
                }
                // Neither a lock nor its stand-in: never run beside another unseen.
                Err(why) => cannot_start(&dir, &why),
            }
            // Before anything reaches Apple, so a run with diagnostics on has all of it.
            diagnostics::start(&dir);
            let client = match Client::new(&dir) {
                Ok(client) => client,
                Err(quince_auth::Error::Io(e)) => cannot_start(&dir, &folder_problem(&e)),
                Err(e) => cannot_start(&dir, &e.to_string()),
            };
            let known = std::sync::Mutex::new(SessionFacts::of(&client));
            let client = Arc::new(Mutex::new(client));
            app.manage(Auth {
                client: Arc::clone(&client),
                signing_in: AtomicBool::new(false),
                known,
            });
            // The local library. Opened once at startup so every command shares one
            // connection and the WAL behaves.
            let opened = match Library::open_or_recover(&dir) {
                Ok(opened) => opened,
                Err(e) => cannot_start(&dir, &library_problem(&e)),
            };
            if let Some(aside) = &opened.set_aside {
                quiet!(
                    "the library was damaged: set aside at {} and made again ({} rows kept)",
                    aside.display(),
                    opened.salvaged
                );
            }
            let mut library = opened.library;
            keep_private(&dir);
            timing::mark("library open");
            adopt_writer(&mut library);
            app.manage(Data(library_lock::LibraryLock::new(
                library,
                Some(dir.clone()),
            )));
            // The sender of edits made offline, started once what it sends with — the session
            // and the library — is in place: started first, its opening pass found neither and
            // its task died, and nothing waiting was sent all session.
            outbox::start(app.handle().clone());
            appearance::watch(app.handle().clone());
            // Skykeep's own updates: how it was installed decides whether it updates itself.
            app.manage(updates::Updates::new(
                updates::this_install(app.handle()),
                &updates::load_prefs(app.handle()),
            ));
            updates::spawn(app.handle().clone());
            // A photo cache past what this computer can spare now — the disk filled since —
            // is brought back under its budget first thing.
            let trim = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || photos::trim_cache(&trim));
            // And the caches beside it, each to a size of its own (photo_cache::SIDE).
            photo_cache::keep_side_caches(app.handle());
            // Logging out or shutting down ends the app with a signal, not through its own
            // quit, and a crash ends it with neither: either way the iCloud Drive folder is
            // let go of first, never left behind answering "Transport endpoint is not
            // connected".
            #[cfg(unix)]
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tokio::signal::unix::{signal, SignalKind};
                    let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                        signal(SignalKind::terminate()),
                        signal(SignalKind::interrupt()),
                        signal(SignalKind::hangup()),
                    ) else {
                        return;
                    };
                    // The first: ended as Quit ends it. A second while that is under way (Ctrl-C
                    // pressed again): ended at once.
                    let mut first = true;
                    loop {
                        tokio::select! {
                            _ = term.recv() => {}
                            _ = int.recv() => {}
                            _ = hup.recv() => {}
                        }
                        if !first {
                            // SAFETY: ends the process at once; nothing after it runs.
                            unsafe { libc::_exit(130) };
                        }
                        first = false;
                        let app = handle.clone();
                        std::thread::spawn(move || shutdown(&app, 0));
                    }
                });
                let before = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    mount::release();
                    before(info);
                }));
            }
            app.manage(Occasions {
                feed: Mutex::new((None, Vec::new())),
            });
            spawn_holidays(app.handle().clone());
            app.manage(Feeds {
                subs: Mutex::new(Vec::new()),
            });
            spawn_subscriptions(app.handle().clone());
            spawn_heartbeat(Arc::clone(&client));
            push::spawn(app.handle().clone(), Arc::clone(&client));
            spawn_auto_sync(app.handle().clone(), client);
            // Each service's part of a pass, told to the open screens as it is stored.
            let landing = app.handle().clone();
            refresh::on_landed(move |service, changed, items| {
                let app = landing.clone();
                tauri::async_runtime::spawn(async move {
                    // Its freshness for every screen; a read again only for its own, only when
                    // it stored something — and of only the items it changed, when known.
                    let status =
                        refresh::status_landed(&app.state::<Data>().0, service, changed, items)
                            .await;
                    app.emit(SYNCED, status).ok();
                });
            });
            // In the desktop's own search (GNOME Shell, KRunner) for as long as it runs.
            #[cfg(target_os = "linux")]
            {
                search_provider::serve(app.handle().clone());
                if login::search_only() {
                    search_provider::end_when_idle(app.handle().clone());
                }
            }
            alerts::spawn(app.handle().clone());
            alerts::spawn_events(app.handle().clone());
            if let Err(e) = build_tray(app) {
                quiet!("no tray on this desktop, so closing the window quits: {e}");
            }
            let launched: Vec<String> = std::env::args().collect();
            let here = std::env::current_dir().unwrap_or_default();
            // Started to open a file, or for a quick capture: that window is what shows.
            if intake::arrived(app.handle(), &launched, &here) {
                LAUNCHED_AS_APP.store(true, Ordering::SeqCst);
            } else if let Some(key) = apps::asked(&launched) {
                LAUNCHED_AS_APP.store(true, Ordering::SeqCst);
                apps::open(app.handle(), key);
            }
            // The whole-library window, unless this launch is one app's or one file's: then it is
            // made when it is first asked for (the tray, its launcher), about half a second the
            // first time and at once after. Made hidden beside the app's window instead, it kept a
            // second web view of about 115 MB all day for people who never opened it (#11);
            // every window takes up the session itself (App.tsx), so nothing waited on it.
            if !LAUNCHED_AS_APP.load(Ordering::SeqCst) {
                make_main(app.handle());
            }
            show_when_ready(app.handle());
            timing::mark("setup done");
            // Photos uploads a quit left half done carry on.
            photos::work(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            data_access::data_access_status,
            data_access::data_access_ask,
            account_refresh,
            grants::pick_open,
            grants::pick_save,
            export::library_export_start,
            export::library_export_cancel,
            export::library_export_reveal,
            outbox::outbox_status,
            outbox::outbox_resend,
            outbox::outbox_discard,
            outbox::outbox_wake,
            appearance::system_contrast,
            appearance::system_time_locale,
            network_reach,
            updates::update_status,
            updates::update_check,
            updates::update_apply,
            updates::update_later,
            updates::update_auto_set,
            updates::update_typing,
            auth_status,
            auth_resume,
            auth_sign_in,
            auth_submit_code,
            auth_two_factor_options,
            auth_resend_code,
            auth_request_code,
            auth_send_sms,
            auth_submit_sms_code,
            auth_sign_out,
            account_unsent,
            search_everything,
            apps::app_open,
            intake::intake_pending,
            intake::intake_capture_close,
            intake::intake_open_main,
            login::login_item_status,
            settings::local_storage,
            settings::local_storage_clear,
            settings::alerts_shown,
            mount::drive_kept,
            mount::drive_keep,
            login::login_item_set,
            account_services,
            account_info,
            settings_get,
            settings_set,
            sync_all,
            sync_some,
            sync_status,
            calendar_sync,
            calendar_events,
            event_save,
            event_alerts,
            event_guests,
            event_repeat,
            event_delete,
            calendar_create,
            calendar_update,
            calendar_delete,
            calendar_list,
            calendar_search,
            calendar_subscriptions,
            calendar_subscribe,
            calendar_subscription_update,
            calendar_unsubscribe,
            calendar_subscription_refresh,
            reminder_lists,
            reminder_groups,
            reminder_sections,
            reminders_search,
            reminder_counts,
            reminders_in,
            reminders_due_between,
            reminder_create,
            reminder_delete,
            reminder_edit,
            reminders_smart,
            reminder_list_create,
            reminder_list_rename,
            reminder_list_delete,
            reminder_list_colour,
            reminder_reorder,
            note_folders,
            note_folder_create,
            note_folder_rename,
            note_folder_delete,
            note_body,
            note_row,
            note_export_markdown,
            note_runs,
            note_table,
            note_table_set_cell,
            window_ready,
            photos::photos_list,
            photos::photos_counts,
            photos::photos_albums,
            photos::photos_in_album,
            photos::photo_set_flag,
            photos::photo_play,
            mount::drive_folder_status,
            mount::drive_folder_set,
            mount::drive_folder_open,
            mount::photos_folder_status,
            mount::photos_folder_set,
            mount::photos_folder_open,
            photos::photo_video_turn,
            photos::photo_video_zoom,
            photos::photos_set_flag,
            photos::photos_burst,
            photos::photos_shared_albums,
            photos::photos_library,
            photos::photos_set_library,
            photos::photos_unlock_hidden,
            photos::photos_lock_hidden,
            photos::photos_hello_available,
            photos::photos_unlock_hidden_hello,
            photos::photos_prepare,
            photos::photos_index,
            photos::photos_months,
            photos::photos_page,
            photos::photos_get,
            photo_cache::photos_cache_status,
            photo_cache::photos_cache_set,
            photo_cache::photos_cache_clear,
            video::video_base,
            video::video_support,
            window_tint,
            window_fullscreen,
            photos::photos_upload,
            photos::photos_upload_cancel,
            photos::photos_album_create,
            photos::photos_album_rename,
            photos::photos_album_delete,
            photos::photos_album_add,
            photos::photos_album_remove,
            photos::photo_set_text,
            photos::photo_set_rating,
            photos::photo_set_keywords,
            photos::photo_open_map,
            photos::photo_set_date,
            photos::photos_search,
            photos::photo_download,
            photos::photo_copy,
            note_table_add_row,
            note_table_remove,
            note_attachment_info,
            note_pictures,
            note_attachment_open,
            diagnostics_save,
            download_reveal,
            note_paragraph,
            note_save,
            note_create,
            note_delete,
            note_move,
            note_pin,
            notes_in,
            notes_search,
            note_counts,
            contacts_list,
            contacts_get,
            contact_save,
            contact_lists,
            contacts_export,
            contacts_import,
            contact_me,
            contact_set_me,
            contact_list_write,
            contact_delete,
            contacts_cmds::contact_addresses,
            contacts_cmds::contact_address_edit,
            contacts_merge::contacts_duplicates,
            contacts_merge::contacts_merge,
            contacts_merge::contacts_merge_undo,
            notes_cmds::note_unlock,
            notes_cmds::note_lock_hint,
            notes_cmds::note_lock_forget,
            notes_cmds::note_attach_file,
            notes_cmds::note_attach_preview,
            notes_cmds::note_attach_enabled,
            reminders_cmds::reminder_detail,
            reminders_cmds::reminder_more,
            reminders_cmds::reminders_ending,
            reminders_cmds::reminder_marks,
            reminders_cmds::reminder_extras,
            reminders_cmds::reminder_tags,
            reminders_cmds::reminders_tagged,
            reminders_cmds::reminder_smart_lists,
            reminders_cmds::reminders_in_smart_list,
            reminders_cmds::reminder_tag_add,
            reminders_cmds::reminder_tag_remove,
            reminders_cmds::reminder_url_set,
            reminders_cmds::reminder_early_alert_set,
            reminders_cmds::reminder_section_create,
            reminders_cmds::reminder_section_rename,
            reminders_cmds::reminder_section_move,
            drive_sync_folder,
            drive_children,
            drive_download,
            drive_download_items,
            drive_download_cancel,
            drive_downloads_unfinished,
            drive_download_forget,
            drive_upload,
            drive_create_folder,
            drive_rename,
            drive_delete,
            drive_trash,
            drive_recents,
            storage_info,
            drive_duplicate,
            drive_copy,
            clipboard::clipboard_read,
            clipboard::clipboard_paste,
            drive_thumbnails,
            drive_put_back,
            drive_delete_forever,
            drive_trash_out,
            drive_upload_cancel,
            drive_move,
            drive_item,
            reminder_set_done,
            reminder_set_repeat,
        ])
        .build(context)
        .unwrap_or_else(|e| startup_guard::failed(&e))
        .run(|_app, event| {
            // The last window given back (put away, then closed for good — see apps::put_away)
            // is not the app ending: it goes on behind the tray, ringing alerts and keeping
            // the library current. Quit, a logout or a signal ends it through `shutdown`;
            // with no tray to come back from, closing the window still quits.
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = &event
            {
                if tray_reachable(_app) {
                    api.prevent_exit();
                    return;
                }
            }
            // The iCloud Drive folder goes with the app: left mounted, it would answer
            // nothing ("Transport endpoint is not connected").
            if let tauri::RunEvent::Exit = event {
                shutdown(_app, 0);
            }
        });
}

#[cfg(test)]
mod tests {

    /// A render node and nothing saying otherwise is a GPU; no render node, or Mesa told to
    /// answer on the processor, is not — and only then does the page paint on the processor.
    #[test]
    fn the_page_paints_on_the_processor_only_where_there_is_no_gpu() {
        use super::gpu_to_paint_on as gpu;
        assert!(gpu(true, None, None));
        assert!(gpu(true, Some("0"), None));
        assert!(gpu(true, Some(""), Some("iris")));
        assert!(!gpu(false, None, None));
        assert!(!gpu(true, Some("1"), None));
        assert!(!gpu(true, Some("true"), None));
        assert!(!gpu(true, None, Some("llvmpipe")));
        assert!(!gpu(true, None, Some("softpipe")));
    }

    /// Everything directly in the data folder is its owner's alone, whatever it was made with.
    #[cfg(unix)]
    #[test]
    fn the_library_and_its_folders_are_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(data.join("photos").join("thumb")).unwrap();
        std::fs::create_dir_all(data.join("note-media")).unwrap();
        for f in ["library.db", "library.db-wal", "library.db-shm"] {
            std::fs::write(data.join(f), b"x").unwrap();
            std::fs::set_permissions(data.join(f), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        std::fs::set_permissions(data.join("photos"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        // A link inside is left alone, and so is what it leads to.
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, b"y").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&outside, data.join("link")).unwrap();
        super::keep_private(&data);
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data), 0o700);
        for f in ["library.db", "library.db-wal", "library.db-shm"] {
            assert_eq!(mode(&data.join(f)), 0o600, "{f}");
        }
        assert_eq!(mode(&data.join("photos")), 0o700);
        assert_eq!(mode(&data.join("note-media")), 0o700);
        assert_eq!(mode(&outside), 0o644);
    }

    /// A long name is cut to fit every disk, counted in bytes as Linux counts them, and keeps
    /// its extension: 120 characters of it dropped ".pdf", and 120 CJK characters or emoji,
    /// with " 2" and ".part" added on the way, were over ext4's 255 bytes and failed to save.
    #[test]
    fn a_long_name_keeps_its_extension_and_fits_the_disk() {
        use super::attachment_name;
        for given in [
            format!("{}.pdf", "a".repeat(200)),
            format!("{}.pdf", "\u{6f22}".repeat(100)),
            format!("{}.pdf", "\u{1f600}".repeat(64)),
        ] {
            let name = attachment_name(Some(&given), "A");
            assert!(name.ends_with(".pdf"), "{name}");
            assert!(name.len() <= 120, "{} bytes: {name}", name.len());
            assert!(format!("{name} 99.part").len() <= 255);
        }
        // Short names, and the parts of one that look like an extension and are not, as before.
        assert_eq!(
            attachment_name(Some("Report 2026.pdf"), "A"),
            "Report 2026.pdf"
        );
        let plain = attachment_name(Some(&format!("{}.v2 final draft", "b".repeat(150))), "A");
        assert!(plain.len() <= 120 && plain.starts_with('b'), "{plain}");
    }

    /// The number a second copy takes goes before the whole extension, as a browser puts it:
    /// "archive.tar.gz" was saved as "archive.tar 2.gz", and "v1.2 notes" as "v1 2.2 notes".
    #[test]
    fn a_second_copy_is_numbered_before_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["archive.tar.gz", "v1.2 notes", "Report.pdf", "Makefile"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let next = |n: &str| {
            super::free_path(dir.path(), n)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(next("archive.tar.gz"), "archive 2.tar.gz");
        assert_eq!(next("v1.2 notes"), "v1.2 notes 2");
        assert_eq!(next("Report.pdf"), "Report 2.pdf");
        assert_eq!(next("Makefile"), "Makefile 2");
        assert_eq!(next("new.txt"), "new.txt");
    }

    /// Cancel pressed while a folder upload is still being walked — before any file has a stop
    /// of its own — reaches every file found by then, as does one file's ✕; the next batch
    /// starts clean.
    #[test]
    fn an_upload_cancelled_before_it_began_is_never_sent() {
        use super::{drive_upload_cancel, forget_cancelled, upload_stopped};
        drive_upload_cancel(Vec::new(), Some("planning".into()));
        assert!(upload_stopped("planning", "planning:0"));
        assert!(upload_stopped("planning", "planning:41"));
        assert!(!upload_stopped("other", "other:0"));
        forget_cancelled("planning");
        assert!(!upload_stopped("planning", "planning:0"));

        drive_upload_cancel(vec!["one:2".into()], None);
        assert!(upload_stopped("one", "one:2"));
        assert!(!upload_stopped("one", "one:3"));
        forget_cancelled("one");
        assert!(!upload_stopped("one", "one:2"));
    }

    /// A plain http feed is tried over https first; a secure one is left as it is.
    #[test]
    fn a_plain_feed_is_tried_secure_first() {
        use super::secure_feed_url;
        assert_eq!(
            secure_feed_url("http://calendar.example.org/team.ics?k=1").as_deref(),
            Some("https://calendar.example.org/team.ics?k=1")
        );
        assert_eq!(
            secure_feed_url("http://calendar.example.org:8080/a.ics").as_deref(),
            Some("https://calendar.example.org/a.ics")
        );
        assert_eq!(secure_feed_url("https://calendar.example.org/a.ics"), None);
    }

    /// The sidebar's services and the account's name are answered while the session is busy —
    /// taken up again, or checked at the start of a sync — from what was kept, never a wait.
    #[test]
    fn account_facts_answer_while_the_session_is_busy() {
        use super::{session_facts, Auth, SessionFacts};
        let dir = tempfile::tempdir().unwrap();
        let client = quince_auth::Client::new(dir.path()).unwrap();
        let auth = Auth {
            client: std::sync::Arc::new(tokio::sync::Mutex::new(client)),
            signing_in: std::sync::atomic::AtomicBool::new(false),
            known: std::sync::Mutex::new(SessionFacts {
                services: vec!["notes".into(), "reminders".into()],
                name: Some("Kept Name".into()),
                ..SessionFacts::default()
            }),
        };
        let busy = auth.client.try_lock().unwrap();
        let facts = session_facts(&auth);
        assert_eq!(facts.services, ["notes", "reminders"]);
        assert_eq!(facts.name.as_deref(), Some("Kept Name"));
        drop(busy);
        // Free, it is read from the session itself (signed out here: nothing) and kept.
        assert!(session_facts(&auth).services.is_empty());
        assert!(auth.known.lock().unwrap().services.is_empty());
    }

    /// Whether a tray host is there is asked once, then answered from what was kept while it
    /// is asked again behind — never a wait on the session bus after the first.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_tray_host_is_asked_off_the_main_thread_after_the_first_time() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static HOST: super::HostSeen = super::HostSeen::new();
        static ASKED: AtomicUsize = AtomicUsize::new(0);
        fn ask() -> bool {
            ASKED.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        }
        let first = std::time::Instant::now();
        assert!(!HOST.reachable(ask));
        assert!(first.elapsed() >= std::time::Duration::from_millis(50));
        HOST.keep(true);
        let again = std::time::Instant::now();
        assert!(HOST.reachable(ask), "the kept answer");
        assert!(again.elapsed() < std::time::Duration::from_millis(40));
        // The question asked again behind lands, and is what the next answer says.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ASKED.load(Ordering::SeqCst) < 2 || HOST.asking.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!HOST.reachable(ask));
    }

    /// A phone's photo is drawn in its note no larger than the note needs; a small one, and a
    /// GIF (which would lose its motion), as they are.
    #[test]
    fn a_note_picture_is_drawn_at_the_size_the_note_needs() {
        let jpeg = |w: u32, h: u32| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::RgbImage::from_pixel(w, h, image::Rgb([200, 120, 40]))
                .write_to(&mut out, image::ImageFormat::Jpeg)
                .unwrap();
            out.into_inner()
        };
        let small = super::inline_picture(&jpeg(4032, 3024)).expect("made smaller");
        let drawn = image::load_from_memory(&small).unwrap();
        assert_eq!((drawn.width(), drawn.height()), (super::INLINE_LONG, 1200));
        assert!(super::inline_picture(&jpeg(1200, 800)).is_none());
        assert!(super::inline_picture(b"GIF89a....").is_none());
    }

    /// A shared note's attachment is saved under a plain name in its own folder, whatever its
    /// sender called it, and a program in one is shown in its folder, never run.
    #[test]
    fn a_note_attachment_cannot_escape_or_run() {
        use super::{attachment_name, runs_code};
        for (given, want) in [
            (Some("Report.pdf"), "Report.pdf"),
            (Some("../../.bashrc"), "bashrc"),
            (Some("..\\..\\x.txt"), "x.txt"),
            (Some("C:evil.txt"), "Cevil.txt"),
            (Some(".hidden"), "hidden"),
            (Some("invoice\u{202E}fdp.exe"), "invoicefdp.exe"),
            (Some("a\nb\0c.pdf"), "abc.pdf"),
            (Some("CON.txt"), "_CON.txt"),
            (Some("com1"), "_com1"),
            (Some("trailing. . "), "trailing"),
            (Some("/"), "Attachment ABC"),
            (Some(".."), "Attachment ABC"),
            (None, "Attachment ABC"),
        ] {
            let name = attachment_name(given, "ABC");
            assert_eq!(name, want, "{given:?}");
            let dir = std::path::Path::new("/data/note-files/ABC");
            assert_eq!(dir.join(&name).parent(), Some(dir), "{given:?}");
        }
        assert!(attachment_name(Some(&"x".repeat(1000)), "A").len() <= 120);
        for runs in [
            "evil.desktop",
            "x.SH",
            "setup.exe",
            "a.pdf.jar",
            "Run.AppImage",
            "s.lnk",
            "invoice.one",
            "budget.xlsm",
            "letter.docm",
            "deck.pptm",
            "addin.xll",
            "q.search-ms",
            "q.searchConnector-ms",
            "fix.diagcab",
            "office.rdp",
            "app.appinstaller",
            "app.msix",
            "app.appx",
            "lib.library-ms",
            "go.url",
            "go.website",
            "s.settingcontent-ms",
            "page.hta",
            "page.html",
            "image.svg",
        ] {
            assert!(runs_code(runs, b"%PDF-"), "{runs}");
        }
        assert!(runs_code("notes.txt", b"#!/bin/sh\nrm -rf ~"));
        assert!(runs_code("photo.jpg", b"\x7fELF\x02\x01"));
        assert!(runs_code("Attachment 1", b"[Desktop Entry]\nExec=sh"));
        assert!(!runs_code("Report.pdf", b"%PDF-1.7"));
        assert!(!runs_code("IMG_0001.HEIC", b"\0\0\0\x18ftypheic"));
    }

    /// No `quince://` request reaches a file outside the folders its kind is kept in.
    #[test]
    fn the_scheme_reads_nothing_outside_its_folders() {
        for path in [
            "/pic/..%2F..%2Fcookies.json/original",
            "/pic/../original",
            "/pic/a%2Fb/original",
            "/pic//original",
            "/pic/%2e%2e/fit512",
        ] {
            assert_eq!(super::photos::requested(path), None, "{path}");
            assert_eq!(super::photos::requested_fit(path), None, "{path}");
        }
        assert_eq!(super::photos::requested_upload("/up/..%2Fx"), None);
        let dir = std::path::Path::new("/data");
        let guid = super::photo_guid(Some("photo"), "/..%2F..%2Fetc%2Fpasswd").unwrap();
        let at = super::photo_path(dir, &guid, "../../sig/..");
        assert_eq!(at.parent(), Some(dir.join("faces").as_path()));
        let (id, _) = super::note_part("/note/..%2F..%2Fcookies.json/file").unwrap();
        let safe = super::media_file_id(&id);
        assert_eq!(safe, "______cookies_json");
    }

    /// A stand-in for the folders and screens around a switch: what was done, in order, and
    /// whether the last account's library was still in place when its folders were let go of.
    struct Recorded {
        dir: std::path::PathBuf,
        away: &'static str,
        done: std::sync::Mutex<Vec<String>>,
    }

    impl Handover for Recorded {
        async fn release_folders(&self) -> Result<(), String> {
            let set_aside = self.dir.join(SET_ASIDE).join(self.away).exists();
            let held = self.dir.join("library.db").exists();
            self.done.lock().unwrap().push(format!(
                "folders released (library in place: {}, set aside yet: {set_aside})",
                held && !set_aside
            ));
            Ok(())
        }
        fn forget_readers(&self) {
            self.done.lock().unwrap().push("readers forgotten".into());
        }
        fn forget_links(&self) {
            self.done.lock().unwrap().push("links forgotten".into());
        }
    }

    /// Another Apple ID signing in: the Drive and Photos folders the last account's session
    /// served are let go of first — while its library is still the one in place — then the
    /// screens' readers, then the library moves; the same account again moves nothing and
    /// releases nothing.
    #[test]
    fn a_switch_releases_the_folders_before_the_library_moves() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut lib = Library::open(dir).unwrap();
        lib.set_state(ACCOUNT_KEY, "a").unwrap();
        lib.set_state("contacts.me", "A's card").unwrap();
        let around = Recorded {
            dir: dir.to_path_buf(),
            away: "a",
            done: std::sync::Mutex::new(Vec::new()),
        };
        tauri::async_runtime::block_on(claim_as(&mut lib, dir, "b", &around)).unwrap();
        assert_eq!(
            *around.done.lock().unwrap(),
            [
                "folders released (library in place: true, set aside yet: false)",
                "readers forgotten",
                "links forgotten"
            ]
        );
        assert!(dir.join(SET_ASIDE).join("a").join("library.db").exists());
        assert_eq!(lib.state(ACCOUNT_KEY).unwrap().as_deref(), Some("b"));
        assert_eq!(lib.state("contacts.me").unwrap(), None);

        // B again: nothing changes hands, nothing is let go of.
        around.done.lock().unwrap().clear();
        tauri::async_runtime::block_on(claim_as(&mut lib, dir, "b", &around)).unwrap();
        assert!(around.done.lock().unwrap().is_empty());
    }

    /// A folder that would not let go stops the switch: the library stays the last
    /// account's, in place, rather than moving out from under a folder still serving it.
    #[test]
    fn a_switch_stops_when_the_folders_cannot_be_released() {
        struct Stuck;
        impl Handover for Stuck {
            async fn release_folders(&self) -> Result<(), String> {
                Err("stuck".into())
            }
            fn forget_readers(&self) {
                panic!("readers forgotten after a failed release");
            }
            fn forget_links(&self) {
                panic!("links forgotten after a failed release");
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut lib = Library::open(dir).unwrap();
        lib.set_state(ACCOUNT_KEY, "a").unwrap();
        assert!(tauri::async_runtime::block_on(claim_as(&mut lib, dir, "b", &Stuck)).is_err());
        assert_eq!(lib.state(ACCOUNT_KEY).unwrap().as_deref(), Some("a"));
        assert!(!dir.join(SET_ASIDE).join("a").exists());
    }

    /// Two Apple IDs on one computer: each one's library and caches set aside whole while the
    /// other is signed in, and back exactly as they were — never mixed, never deleted — with
    /// this computer's settings staying put through both switches.
    #[test]
    fn accounts_are_set_aside_and_brought_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut lib = Library::open(dir).unwrap();
        lib.set_state(ACCOUNT_KEY, "a").unwrap();
        lib.set_state("contacts.me", "A's card").unwrap();
        lib.set_state("settings.alerts.events", "0").unwrap();
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        std::fs::write(dir.join("photos").join("one"), b"A's photo").unwrap();

        // B signs in: A goes aside, B starts clean.
        switch_library(&mut lib, dir, "a", &dir.join(SET_ASIDE).join("b")).unwrap();
        assert_eq!(
            lib.state("contacts.me").unwrap(),
            None,
            "nothing of A's in B's library"
        );
        assert!(
            !dir.join("photos").join("one").exists(),
            "nor in its caches"
        );
        assert_eq!(
            lib.state("settings.alerts.events").unwrap().as_deref(),
            Some("0")
        );
        assert!(dir.join(SET_ASIDE).join("a").join("library.db").exists());
        lib.set_state(ACCOUNT_KEY, "b").unwrap();
        lib.set_state("contacts.me", "B's card").unwrap();
        lib.set_state("settings.alerts.events", "1").unwrap();

        // A again: B aside, A back as it was.
        switch_library(&mut lib, dir, "b", &dir.join(SET_ASIDE).join("a")).unwrap();
        assert_eq!(lib.state(ACCOUNT_KEY).unwrap().as_deref(), Some("a"));
        assert_eq!(
            lib.state("contacts.me").unwrap().as_deref(),
            Some("A's card")
        );
        assert_eq!(
            std::fs::read(dir.join("photos").join("one")).unwrap(),
            b"A's photo"
        );
        assert_eq!(
            lib.state("settings.alerts.events").unwrap().as_deref(),
            Some("1"),
            "the computer's latest setting, not the one A left"
        );
        assert!(
            !dir.join(SET_ASIDE).join("a").exists(),
            "its shelf put away"
        );
        assert!(dir.join(SET_ASIDE).join("b").join("library.db").exists());
    }

    /// A move that cannot finish undoes itself, and never lands over what is already there.
    #[test]
    fn a_move_is_all_or_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::create_dir_all(from.join("photos")).unwrap();
        std::fs::create_dir_all(to.join("faces")).unwrap();
        std::fs::write(from.join("library.db"), b"mine").unwrap();
        assert!(move_account(&from, &to).is_err(), "something already there");
        assert!(
            from.join("library.db").exists() && from.join("photos").exists(),
            "nothing moved"
        );
    }

    /// Every file under `dir` that holds `marker`, anywhere in its bytes.
    fn holding(dir: &std::path::Path, marker: &[u8]) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut todo = vec![dir.to_path_buf()];
        while let Some(at) = todo.pop() {
            for entry in std::fs::read_dir(&at).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    todo.push(path);
                } else if std::fs::read(&path)
                    .unwrap()
                    .windows(marker.len())
                    .any(|w| w == marker)
                {
                    found.push(path);
                }
            }
        }
        found
    }

    /// "Remove copy" leaves nothing of the account on disk: not its rows in SQLite's free pages
    /// or log, not a damaged library set aside beside it, not a cache. Deleting the rows alone
    /// left the account's words readable in `library.db` and its `-wal`. This computer's own
    /// settings stay.
    #[test]
    fn removing_the_copy_leaves_no_trace_on_disk() {
        use quince_core::calendar::{Calendar, Event};
        const MARKER: &str = "zq-marker-7d1c";
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut lib = Library::open(dir).unwrap();
        lib.set_state(ACCOUNT_KEY, "a").unwrap();
        lib.set_state("contacts.me", MARKER).unwrap();
        lib.set_state("settings.alerts.events", "0").unwrap();
        lib.put_calendars(&[Calendar {
            guid: "work".into(),
            title: "Work".into(),
            ..Default::default()
        }])
        .unwrap();
        let events: Vec<Event> = (0..300)
            .map(|i| Event {
                guid: format!("E{i}"),
                calendar_guid: "work".into(),
                title: format!("{MARKER} {i}"),
                notes: Some(format!("{MARKER} notes {i}")),
                starts_at: 1_000_000 + i,
                ends_at: 1_003_600 + i,
                ..Default::default()
            })
            .collect();
        lib.put_events(&events).unwrap();
        // A damaged library set aside earlier, with its log, and a cached file.
        std::fs::write(dir.join("library.corrupt-1.db"), MARKER).unwrap();
        std::fs::write(dir.join("library.corrupt-1.db-wal"), MARKER).unwrap();
        std::fs::create_dir_all(dir.join("drive-cache")).unwrap();
        std::fs::write(dir.join("drive-cache").join("f"), MARKER).unwrap();
        assert!(!holding(dir, MARKER.as_bytes()).is_empty());

        remove_account(&mut lib, dir).unwrap();
        assert_eq!(
            holding(dir, MARKER.as_bytes()),
            Vec::<std::path::PathBuf>::new(),
            "no file holds the account"
        );
        drop(lib);
        assert_eq!(
            holding(dir, MARKER.as_bytes()),
            Vec::<std::path::PathBuf>::new()
        );
        let lib = Library::open(dir).unwrap();
        assert!(!lib.holds_account().unwrap(), "a fresh, empty library");
        assert_eq!(
            lib.state("settings.alerts.events").unwrap().as_deref(),
            Some("0"),
            "this computer's settings kept"
        );
    }

    /// A damaged library set aside is the account's: it goes aside with it when another
    /// account signs in, and comes back with it.
    #[test]
    fn a_damaged_copy_moves_with_its_account() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let mut lib = Library::open(dir).unwrap();
        lib.set_state(ACCOUNT_KEY, "a").unwrap();
        std::fs::write(dir.join("library.corrupt-1.db"), b"A's").unwrap();
        switch_library(&mut lib, dir, "a", &dir.join(SET_ASIDE).join("b")).unwrap();
        assert!(!dir.join("library.corrupt-1.db").exists());
        assert!(dir
            .join(SET_ASIDE)
            .join("a")
            .join("library.corrupt-1.db")
            .exists());
        switch_library(&mut lib, dir, "b", &dir.join(SET_ASIDE).join("a")).unwrap();
        assert_eq!(
            std::fs::read(dir.join("library.corrupt-1.db")).unwrap(),
            b"A's"
        );
    }

    use super::*;

    /// Apple serves contact pictures as PNG, which is not what the first version of this
    /// assumed. The type is read from the bytes so a change of format is not a broken image.
    #[test]
    fn an_image_is_recognised_by_its_own_first_bytes() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), "image/png");
        assert_eq!(image_type(b"\xff\xd8\xff\xe0JFIF"), "image/jpeg");
        assert_eq!(image_type(b"GIF89a"), "image/gif");
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
        assert_eq!(image_type(b"\0\0\0\x18ftypheic"), "image/heic");
        assert_eq!(image_type(b"not a picture"), "application/octet-stream");
        assert_eq!(image_type(b""), "application/octet-stream", "and no panic");
    }

    /// A signature is base64 and holds `/` and `+`; a filename cannot. Two different pictures
    /// must still land on two different files.
    #[test]
    fn a_cached_picture_is_named_safely_and_uniquely() {
        let dir = std::path::Path::new("/tmp/quince");
        let one = photo_path(dir, "A-B/C", "FAhjvQxoOa/G338LIq+u7A==");
        let two = photo_path(dir, "A-B/C", "ZZhjvQxoOa/G338LIq+u7A==");
        let name = one.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "a filename Apple's base64 cannot break: {name}"
        );
        assert_ne!(one, two, "a new picture must not read the old one's cache");
    }

    /// The address bar of the interface is not a path: a contact id arrives encoded.
    #[test]
    fn a_percent_encoded_id_comes_back_as_itself() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        // What convertFileSrc("photo/<id>", "quince") builds, and what WebKitGTK reads.
        let id = "9C2B1E7A-0F4D-4E25-8A61-3C1D2E4F5A6B";
        assert_eq!(
            photo_guid(Some("localhost"), &format!("/photo%2F{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(
            photo_guid(Some("localhost"), &format!("/photo/{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(
            photo_guid(Some("photo"), &format!("/{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(photo_guid(Some("localhost"), "/other%2Fx"), None);
        assert_eq!(photo_guid(Some("localhost"), "/photo%2F"), None);
        // A note's attachments, as convertFileSrc("note/<id>/preview", "quince") builds them.
        assert!(
            matches!(note_part("/note%2FABC-1%2Fpreview"), Some((ref id, quince_icloud::note_media::Part::Preview)) if id == "ABC-1")
        );
        assert!(matches!(
            note_part("/note%2FABC-1%2Ffile"),
            Some((_, quince_icloud::note_media::Part::File))
        ));
        assert!(note_part("/note%2FABC-1%2Fother").is_none());
        // The list's small picture is its own route, never one that could fetch.
        assert!(note_part("/note%2FABC-1%2Fthumb").is_none());
        assert_eq!(
            super::note_thumb_part("/note%2FABC-1%2Fthumb").as_deref(),
            Some("ABC-1")
        );
        assert_eq!(super::note_thumb_part("/note%2FABC-1%2Finline"), None);
        assert_eq!(super::note_thumb_part("/note%2Fa%2Fb%2Fthumb"), None);
        // Made only from a picture already kept: the one the note draws first, and nothing
        // when none is.
        let dir = std::env::temp_dir().join(format!("skykeep-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(super::note_thumb_source(&dir, "P-1"), None);
        std::fs::write(dir.join("P-1-preview"), b"x").unwrap();
        std::fs::write(dir.join("P-1-inline"), b"x").unwrap();
        assert_eq!(
            super::note_thumb_source(&dir, "P-1"),
            Some(dir.join("P-1-inline"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(percent_decode("plain-uuid"), "plain-uuid");
        assert_eq!(percent_decode("%zz"), "%zz", "malformed stays as it was");
    }

    /// A Drive thumbnail is asked for by its name in the cache, and nothing else: no path to
    /// anywhere outside it gets through.
    #[test]
    fn a_drive_thumbnail_is_served_from_the_cache_alone() {
        use super::{drive_thumb_part, drive_thumb_src};
        let name = "FILE__com_apple_CloudDocs__ABC-etag_1";
        assert_eq!(
            drive_thumb_part(&format!("/dthumb/{name}")).as_deref(),
            Some(name)
        );
        assert!(drive_thumb_src(name).ends_with(&format!("/dthumb/{name}")));
        for bad in [
            "/dthumb/",
            "/dthumb/../library.db",
            "/dthumb/%2E%2E%2Flibrary.db",
            "/dthumb/a/b",
            "/dthumb/a.b",
            "/photo/abc",
        ] {
            assert!(drive_thumb_part(bad).is_none(), "{bad}");
        }
    }

    /// What a look at a folder's pictures finds in the cache: those kept, by where they are;
    /// those Apple has none for, nowhere; and those never asked about, to fetch. Folders have
    /// no picture and are not asked about.
    #[test]
    fn the_thumbnail_cache_says_what_it_holds_without_reading_it() {
        use super::{cached_thumbnails, thumbnail_file};
        use quince_core::drive::DriveItem;
        let cache = tempfile::tempdir().unwrap();
        let item = |id: &str, kind: &str| DriveItem {
            drivewsid: id.into(),
            parent_id: None,
            name: id.into(),
            extension: None,
            kind: kind.into(),
            size: None,
            child_count: None,
            created_at: None,
            modified_at: None,
            etag: Some("e1".into()),
        };
        let kept = item("FILE::kept", "FILE");
        let none = item("FILE::none", "FILE");
        let new = item("FILE::new", "FILE");
        std::fs::write(thumbnail_file(cache.path(), &kept), b"\xff\xd8\xffpicture").unwrap();
        std::fs::write(thumbnail_file(cache.path(), &none), b"").unwrap();
        let files = [kept, none, new, item("FOLDER::trip", "FOLDER")];
        let (found, missing) = cached_thumbnails(cache.path(), &files);
        assert_eq!(found.len(), 1);
        assert_eq!(found["FILE::kept"], thumbnail_file(cache.path(), &files[0]));
        let missing: Vec<&str> = missing.iter().map(|f| f.drivewsid.as_str()).collect();
        assert_eq!(missing, ["FILE::new"]);
    }

    /// One folder's pass waits only for that folder's: another folder is read meanwhile, and
    /// a second look at the same folder waits its turn.
    #[tokio::test]
    async fn drive_folders_are_fetched_side_by_side() {
        let a = super::drive_pass("FOLDER::a");
        let held = a.lock().await;
        let b = super::drive_pass("FOLDER::b");
        assert!(b.try_lock().is_ok(), "another folder is not held up");
        let again = super::drive_pass("FOLDER::a");
        assert!(
            again.try_lock().is_err(),
            "the same folder still takes turns"
        );
        drop(held);
        assert!(again.try_lock().is_ok());
    }
}

#[cfg(test)]
mod start_tests {
    use super::*;

    #[test]
    fn a_library_that_will_not_open_is_explained() {
        let io = std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem);
        assert!(folder_problem(&io).contains("can’t write to its folder"));
        let full = quince_core::Error::Io(std::io::ErrorKind::StorageFull.into());
        assert!(library_problem(&full).starts_with("The disk is full"));
        let newer = quince_core::Error::Newer {
            found: 99,
            known: 48,
        };
        assert!(library_problem(&newer).contains("newer version of Skykeep"));
    }

    /// A picture is written whole or not at all, and a contact keeps only its newest.
    #[tokio::test]
    async fn pictures_are_written_whole_and_one_face_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let guid = "0D8D2C3B-6C1E-4C9B-9E57-3A1B2C3D4E5F";
        let other = "1D8D2C3B-6C1E-4C9B-9E57-3A1B2C3D4E5F";
        let old = photo_path(dir.path(), guid, "first");
        let theirs = photo_path(dir.path(), other, "x");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        write_whole(&old, b"before").await.unwrap();
        write_whole(&theirs, b"someone else").await.unwrap();
        let new = photo_path(dir.path(), guid, "second");
        write_whole(&new, b"after").await.unwrap();
        forget_older_faces(&new);
        assert!(!old.exists(), "the older picture goes");
        assert_eq!(std::fs::read(&new).unwrap(), b"after");
        assert!(theirs.exists(), "another contact's stays");
        let left: Vec<_> = std::fs::read_dir(new.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 2, "no part files left behind");
        // A write that cannot land leaves nothing, not a cut-off file.
        let nowhere = dir.path().join("missing").join("x");
        assert!(write_whole(&nowhere, b"z").await.is_err());
        assert!(!nowhere.exists());
    }

    /// After a lapse the heartbeat waits for a new sign-in, and keeps that one warm.
    #[test]
    fn the_heartbeat_comes_back_for_the_next_sign_in() {
        let old = Some("old".to_string());
        assert!(worth_a_ping(true, &old, None));
        assert!(
            !worth_a_ping(true, &old, Some(&old)),
            "the lapsed one rests"
        );
        assert!(
            worth_a_ping(true, &Some("new".into()), Some(&old)),
            "a new one is kept warm"
        );
        assert!(!worth_a_ping(false, &None, None));
    }

    /// Every folder of one account's pictures goes with the account: a reminder's too, which
    /// stayed behind for the next person to sign in on this computer.
    #[test]
    fn an_accounts_reminder_pictures_go_with_it() {
        assert!(ACCOUNT_FILES.contains(&"reminder-media"));
    }

    /// The packages say what the binary needs, so an older system is refused at install with
    /// the package manager's message, not at launch with `GLIBC_2.39 not found`.
    #[test]
    fn the_packages_declare_the_floor_they_are_built_for() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let depends = |kind: &str| -> Vec<String> {
            conf["bundle"]["linux"][kind]["depends"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_str().unwrap().to_string())
                .collect()
        };
        let deb = depends("deb");
        assert!(deb.contains(&"libc6 (>= 2.39)".to_string()), "{deb:?}");
        assert!(deb.contains(&"libheif1 (>= 1.17)".to_string()), "{deb:?}");
        let rpm = depends("rpm");
        // In parentheses, a versioned rich dependency: bare, the bundler wrote a package literally
        // named "glibc >= 2.39", which nothing provides, and Fedora and openSUSE refused it.
        assert!(rpm.contains(&"(glibc >= 2.39)".to_string()), "{rpm:?}");
        assert!(rpm.iter().any(|d| d.contains("libheif >= 1.17")), "{rpm:?}");
    }
}
