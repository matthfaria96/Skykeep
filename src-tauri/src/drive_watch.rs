//! iCloud Drive's changes, heard as they happen and applied folder by folder.
//!
//! Drive has no push (measured 2026-10-29), so a file added on the phone used to show only
//! when its folder was listed again: the open folder every ten seconds, the rest when opened,
//! and the Drive folder on this computer not until then either. Now one tiny read-only check —
//! whether Drive's CloudKit database moved — runs as often as the other services' checks do;
//! when it moves, Drive's own change feed says which items changed and where they are, and
//! only the folders they left or went into are listed again (quince_icloud::drive_changes).
//! Stored, every view reads them at once: the window is told which folders changed, and the
//! Drive folder on this computer sees the library move.
//!
//! Each app's own folder (Pages, Numbers, Shortcuts…) is a zone of its own, read the same way
//! when the database names it as moved.
//!
//! Each zone's position is kept in the library, so changes made while the app was closed are
//! read when it starts. The first run reads each zone through to now once — identifiers only,
//! a few hundred bytes a record — before it can tell what is new.

use crate::push::{poll_every, presence, sleep_or_wake, WAKE};
use crate::Data;
use quince_auth::Client;
use quince_icloud::drive_changes::{self, Changed};
use quince_icloud::Error;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

/// Told to the window with the folders listed again, so an open one redraws.
pub const CHANGED: &str = "quince://drive-changed";

/// Where each zone's feed was last read to, with the account it belongs to ([`Positions`]).
const POSITION: &str = "drive.positions";

/// The most pages one catch-up reads before it stores what it has and lets the next check go
/// on. A first read of a large Drive is a few dozen pages.
const PAGES_AT_ONCE: usize = 500;

pub async fn watch(app: tauri::AppHandle, client: Arc<Mutex<Client>>) {
    // The database's own position, in memory: asked with none, it names every zone there is,
    // so the first check after a start reads every zone from where it was left — changes made
    // while the app was closed included.
    let mut database: Option<String> = None;
    let mut backoff: Option<Duration> = None;
    let mut woken = WAKE.subscribe();
    loop {
        let session = {
            let guard = client.lock().await;
            let s = guard.session();
            s.webservices
                .is_some()
                .then(|| {
                    Some((
                        s.service_url("ckdatabasews")?,
                        s.service_url("drivews")?,
                        s.dsid.clone()?,
                    ))
                })
                .flatten()
                .map(|(ck, drivews, dsid)| {
                    (
                        guard.http().clone(),
                        guard.service_params(),
                        ck,
                        drivews,
                        dsid,
                    )
                })
        };
        let Some((http, params, ck, drivews, dsid)) = session else {
            database = None;
            sleep_or_wake(Duration::from_secs(30), &mut woken).await;
            continue;
        };
        let result =
            match drive_changes::zones_moved(&http, &ck, &params, database.as_deref()).await {
                Ok((zones, next)) => {
                    let read = if zones.is_empty() {
                        Ok(())
                    } else {
                        read_zones(&app, &http, &params, &ck, &drivews, &dsid, &zones).await
                    };
                    // Moved on only once what it named is read: a failure asks the same again.
                    if read.is_ok() {
                        database = next;
                    }
                    read
                }
                Err(Error::StaleCursor) => {
                    database = None;
                    Ok(())
                }
                Err(e) => Err(e),
            };
        backoff = match result {
            Ok(()) => None,
            Err(e) => {
                let next = backoff.map_or(Duration::from_secs(5), |b| {
                    (b * 2).min(Duration::from_secs(5 * 60))
                });
                if backoff.is_none() {
                    quiet!("drive watch: {e}; trying again in {}s", next.as_secs());
                }
                Some(next)
            }
        };
        let wait = backoff.unwrap_or_else(|| poll_every(presence(&app)));
        sleep_or_wake(wait, &mut woken).await;
    }
}

/// Where each zone's feed was last read to, for one account.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Positions {
    dsid: String,
    zones: std::collections::BTreeMap<String, String>,
}

/// Read each zone's feed from where it was left, list again the folders it touched, and keep
/// the new positions — only once they are stored, so a failure part way reads the same
/// changes again. A zone never read before (the first run, an app newly using iCloud) is read
/// through to now, and only its end position kept: everything there is, not news.
async fn read_zones(
    app: &tauri::AppHandle,
    http: &reqwest::Client,
    params: &[(String, String)],
    ck: &str,
    drivews: &str,
    dsid: &str,
    zones: &[String],
) -> quince_icloud::Result<()> {
    let data = app.state::<Data>();
    let kept: Option<String> = data.0.lock().await.state(POSITION).ok().flatten();
    let mut positions = kept
        .and_then(|v| serde_json::from_str::<Positions>(&v).ok())
        .filter(|p| p.dsid == dsid)
        .unwrap_or_else(|| Positions {
            dsid: dsid.to_string(),
            ..Positions::default()
        });
    let mut changed: Vec<Changed> = Vec::new();
    let mut failed = None;
    for zone in zones {
        let first = !positions.zones.contains_key(zone);
        let mut token = positions.zones.get(zone).cloned();
        let mut ok = true;
        for _ in 0..PAGES_AT_ONCE {
            match drive_changes::page(http, ck, params, zone, token.as_deref()).await {
                Ok(page) => {
                    if !first {
                        changed.extend(page.changed);
                    }
                    token = page.token.or(token);
                    if !page.more {
                        break;
                    }
                }
                Err(Error::StaleCursor) if !first => {
                    // Apple no longer knows this position (or the zone is gone): forgotten,
                    // so the next look reads it through as new. What changed in between is
                    // found when each folder is next listed.
                    quiet!("drive watch: {zone}'s position expired; reading it afresh");
                    positions.zones.remove(zone);
                    ok = false;
                    break;
                }
                Err(e) => {
                    failed = Some(e);
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            if let Some(t) = token {
                positions.zones.insert(zone.clone(), t);
            }
        }
    }
    let folders = {
        let library = data.0.lock().await;
        drive_changes::folders_to_list(
            &changed,
            |id| {
                library
                    .drive_item(id)
                    .ok()
                    .flatten()
                    .and_then(|i| i.parent_id)
            },
            |id| id == quince_core::drive::ROOT || library.drive_item(id).ok().flatten().is_some(),
        )
    };
    // What this account's session read goes only into this account's library: another
    // account signing in while the feed was read swapped the library under it.
    let account = crate::account_tag(dsid);
    let mine = |library: &quince_core::Library| {
        crate::owned_by(library, Some(&account)).map_err(Error::Refused)
    };
    let mut listed = Vec::new();
    for folder in folders {
        let pass = crate::drive_pass(&folder);
        let _one = pass.lock().await;
        match quince_icloud::drive::fetch_folder(http, drivews, params.to_vec(), &folder).await {
            Ok(items) => {
                let mut library = data.0.lock().await;
                mine(&library)?;
                quince_icloud::drive::store_folder(&mut library, &folder, &items)?;
            }
            // Gone since: forgotten here too, as when the window finds it gone.
            Err(Error::NotFound) => {
                let mut library = data.0.lock().await;
                mine(&library)?;
                library.forget_drive_folder(&folder)?;
            }
            Err(e) => return Err(e),
        }
        listed.push(folder);
    }
    if let Ok(json) = serde_json::to_string(&positions) {
        let mut library = data.0.lock().await;
        mine(&library)?;
        let _ = library.set_state(POSITION, &json);
    }
    if !listed.is_empty() {
        quiet!("drive watch: {} folder(s) changed", listed.len());
        app.emit(CHANGED, &listed).ok();
        // File Explorer's folders too (Windows), off the async threads: it touches the disk.
        let changed = listed.clone();
        tauri::async_runtime::spawn_blocking(move || crate::mount::drive_changed(&changed));
    }
    failed.map_or(Ok(()), Err)
}
