//! One refresh for the whole account.
//!
//! Four services used to each own a button, a spinner and a "last updated" line, so the
//! same work was asked for four times and any one of them could be quietly out of date.
//! There is one pass now: it refreshes everything, records what each service did, and the
//! window states a single honest freshness — the age of the stalest part.
//!
//! Two rules hold the whole thing together.
//!
//! **The network half never holds the library.** Every service fetches first and stores
//! second, so the screens reading SQLite are never blocked behind a slow endpoint.
//!
//! **One service failing changes nothing for the others.** A refresh returns a report per
//! service; the failure is recorded against that service alone and its last good data stays
//! on screen.

use crate::library_lock::LibraryLock;
use quince_auth::Client;
use quince_core::changes::Items;
use quince_core::Library;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// How a service's refresh ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// Refreshed cleanly.
    Ok,
    /// Apple does not offer this service for the account, so there is nothing to refresh.
    Unavailable,
    /// It was tried and failed. What is on screen is the last good data.
    Failed,
    /// Advanced Data Protection keeps it until the person next approves on one of their
    /// devices: not asked for, not failed. What is on screen is the copy here, and changes
    /// made meanwhile wait to be sent.
    Paused,
    /// Apple is still preparing it (a Photos library just turned on): nothing was read, and
    /// it is looked at again in a while. Not empty, and not failed.
    Preparing,
}

/// What one service's refresh did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceReport {
    pub service: &'static str,
    pub label: &'static str,
    pub outcome: Outcome,
    /// A sentence for the person: what came back, or what went wrong.
    pub detail: Option<String>,
    pub last_synced_at: Option<i64>,
    /// It stored something a screen shows — not only where it got to. Told to the screens with
    /// the pass, never sent with the report itself.
    #[serde(skip)]
    pub changed: bool,
    /// Which of the app's items it changed, when that could be placed: its screen redraws those
    /// rows alone. `None`, it reads its list whole.
    #[serde(skip)]
    pub items: Option<Items>,
}

/// Everything one refresh pass did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    pub services: Vec<ServiceReport>,
    /// The stalest successful refresh across the services that are actually available —
    /// the only freshness the window can honestly claim. `None` while something has never
    /// synced.
    pub as_of: Option<i64>,
    pub took_ms: u64,
    /// The services whose stored rows this changed, when that is known: a screen reads again
    /// only for its own. Absent, every screen reads again, as it always did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<Vec<&'static str>>,
    /// For services in `changed` whose items could be placed, which ones: added, changed and
    /// removed, by id. A service in `changed` and not here is read whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<BTreeMap<&'static str, Items>>,
}

/// More items than this changed in one go and the screen reads its list whole: one read of
/// everything is quicker than asking after each.
const MOST_ITEMS: usize = 500;

/// How often a service reconciles against everything Apple has, rather than the changes
/// since last time.
///
/// A delta is what almost every refresh should be — seconds rather than a minute. But a
/// delta can only apply what Apple chose to send, and a tombstone is sent once, so a record
/// whose removal was missed would stay forever. Once a day is often enough to be a floor
/// and rare enough that nobody notices the cost.
const FULL_SYNC_EVERY: i64 = 24 * 60 * 60;

/// The services one pass covers, in the order a person would notice them going stale.
const COVERED: [(&str, &str); 6] = [
    ("calendar", "Calendar"),
    ("reminders", "Reminders"),
    ("notes", "Notes"),
    ("contacts", "Contacts"),
    ("drive", "Drive"),
    ("photos", "Photos"),
];

/// The calendar window a background refresh keeps warm: the month behind, so last week is
/// still there, and a quarter ahead, which is as far as anyone scrolls without meaning to.
fn default_window() -> (String, String, String) {
    let zone = jiff::tz::TimeZone::system();
    let today = jiff::Timestamp::now().to_zoned(zone.clone()).date();
    let start = today
        .first_of_month()
        .checked_sub(jiff::Span::new().months(1))
        .unwrap_or(today);
    let end = today
        .checked_add(jiff::Span::new().months(3))
        .unwrap_or(today)
        .last_of_month();
    (
        start.to_string(),
        end.to_string(),
        zone.iana_name().unwrap_or("UTC").to_string(),
    )
}

/// Apple's web service each app's data comes from, by the app's sidebar key: what a refresh
/// reads it through, and so what the sidebar shows it for. Notes, Reminders and Photos all live
/// in CloudKit (`ckdatabasews`). The sidebar once looked for each app's own name in Apple's
/// list, and some accounts' lists have no `notes` though CloudKit is there: Notes synced, and
/// its row never showed (#9). One list for both, so they can't disagree.
pub(crate) const APP_SERVICES: &[(&str, &str)] = &[
    ("reminders", CLOUDKIT),
    ("notes", CLOUDKIT),
    ("drivews", DRIVE),
    ("photos", CLOUDKIT),
    ("calendar", CALENDAR),
    ("contacts", CONTACTS),
];

const CALENDAR: &str = "calendar";
const CONTACTS: &str = "contacts";
const CLOUDKIT: &str = "ckdatabasews";
const DRIVE: &str = "drivews";

/// The apps whose data Apple's web services for this account can reach, in the sidebar's keys.
pub(crate) fn apps_offered(webservices: &[String]) -> Vec<String> {
    APP_SERVICES
        .iter()
        .filter(|(_, service)| webservices.iter().any(|w| w == service))
        .map(|(app, _)| (*app).to_string())
        .collect()
}

/// What each service needs from the session, taken in one go so the auth lock is released
/// before any network work starts.
struct Context {
    http: reqwest::Client,
    params: Vec<(String, String)>,
    calendar: Option<String>,
    contacts: Option<String>,
    cloudkit: Option<String>,
    drive: Option<String>,
    /// The account it is all for (`account_tag`): nothing it brings is kept in a library that
    /// is not that account's.
    account: Option<String>,
    /// Services Advanced Data Protection keeps until the next approval: left alone this pass.
    waiting: Vec<&'static str>,
}

async fn context(client: &Arc<Mutex<Client>>) -> Context {
    let guard = client.lock().await;
    let session = guard.session();
    Context {
        http: guard.http().clone(),
        params: guard.service_params(),
        calendar: session.service_url(CALENDAR),
        contacts: session.service_url(CONTACTS),
        cloudkit: session.service_url(CLOUDKIT),
        drive: session.service_url(DRIVE),
        account: session.dsid.as_deref().map(crate::account_tag),
        waiting: crate::data_access::waiting(&guard),
    }
}

/// A service Advanced Data Protection keeps until the next approval, reported as waiting —
/// with when it last synced — and not asked of Apple: asked, it would only be refused.
async fn paused(service: &'static str, label: &'static str, data: &LibraryLock) -> ServiceReport {
    ServiceReport {
        service,
        label,
        outcome: Outcome::Paused,
        detail: Some(PAUSED_WORDS.into()),
        last_synced_at: data
            .read(|library| Ok(library.last_synced(service).ok().flatten()))
            .await
            .ok()
            .flatten(),
        changed: false,
        items: None,
    }
}

/// What a waiting service says.
pub const PAUSED_WORDS: &str = "Syncs when you approve Skykeep on your iPhone, iPad or Mac.";

/// Whether this pass leaves `service` alone for Advanced Data Protection.
fn waits(service: &str, waiting: &[&'static str]) -> bool {
    waiting.contains(&service)
}

/// Told each time one service's part of a pass has been stored, before the rest are done,
/// with whether it changed anything a screen shows, and which items when that is known.
type Landed = Box<dyn Fn(&'static str, bool, Option<Items>) + Send + Sync>;
static LANDED: std::sync::OnceLock<Landed> = std::sync::OnceLock::new();

/// What to do when a service lands: set once, by the app, to tell the open screens.
pub fn on_landed(f: impl Fn(&'static str, bool, Option<Items>) + Send + Sync + 'static) {
    let _ = LANDED.set(Box::new(f));
}

fn landed(report: &ServiceReport) {
    if let Some(f) = LANDED.get() {
        f(report.service, report.changed, report.items.clone());
    }
}

/// Where one service's passes have got to: how many have started, and the last one's report.
struct Turns {
    started: AtomicU64,
    last: std::sync::Mutex<Option<(u64, ServiceReport)>>,
}

impl Turns {
    const fn new() -> Self {
        Self {
            started: AtomicU64::new(0),
            last: std::sync::Mutex::new(None),
        }
    }
}

static TURNS: [Turns; COVERED.len()] = [
    Turns::new(),
    Turns::new(),
    Turns::new(),
    Turns::new(),
    Turns::new(),
    Turns::new(),
];

fn turns(service: &str) -> &'static Turns {
    let at = COVERED.iter().position(|(s, _)| *s == service).unwrap_or(0);
    &TURNS[at]
}

/// Run one service's pass under its pass lock — or, when a pass of it started after this was
/// asked for and has finished while this waited, take that one's report instead of running
/// another.
///
/// Focus in each window, the five-minute timer, every push burst and each window opening all
/// ask for a pass; each used to wait for the one before and then run its own, so a burst of
/// five asked Apple the same questions five times over. A pass that started after the asking
/// has read everything the asking could want, so it answers for all of them: a burst costs at
/// most the pass already running and one more. The second value says whether this ran it.
async fn once<F, Fut>(
    service: &'static str,
    lock: &tokio::sync::Mutex<()>,
    pass: F,
) -> (ServiceReport, bool)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ServiceReport>,
{
    let turns = turns(service);
    let asked = turns.started.load(Ordering::SeqCst);
    let _one = lock.lock().await;
    let done = turns
        .last
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some((n, report)) = done {
        if n > asked {
            return (report, false);
        }
    }
    // Counted before the pass reads anything, so whoever asks from here on waits for the next.
    let n = turns.started.fetch_add(1, Ordering::SeqCst) + 1;
    let report = pass().await;
    *turns
        .last
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((n, report.clone()));
    (report, true)
}

/// A pass's report, told to the screens as it lands when this pass ran it. One taken from a
/// pass that already ran was told then, and counts as no change here: the screens have read it.
fn arrived((mut report, ran): (ServiceReport, bool)) -> ServiceReport {
    if ran {
        landed(&report);
    } else {
        report.changed = false;
        report.items = None;
    }
    report
}

/// Refresh every service the account has, and report on each.
///
/// Never returns an error: a refresh that partly failed is a normal outcome with a partial
/// result, not an exception, and the interface has to be able to show exactly that.
pub async fn all(client: &Arc<Mutex<Client>>, data: &LibraryLock) -> Refresh {
    run(client, data, &COVERED.map(|(service, _)| service)).await
}

/// Refresh only the services `wanted` names — what a push from Apple says changed — and
/// report on those alone.
pub async fn some(client: &Arc<Mutex<Client>>, data: &LibraryLock, wanted: &[&str]) -> Refresh {
    run(client, data, wanted).await
}

async fn run(client: &Arc<Mutex<Client>>, data: &LibraryLock, services: &[&str]) -> Refresh {
    let wanted = |service: &str| services.contains(&service);
    let started = std::time::Instant::now();
    let ctx = context(client).await;
    let (from, to, zone) = default_window();

    // The services fetched side by side, not one after another: each has its own pass lock,
    // and waiting on Photos' whole library before Reminders was asked anything left every
    // screen empty through a first sync. Each one says it has landed as it does, so its
    // screen fills then rather than when the slowest is done.

    let calendar = async {
        if !wanted("calendar") {
            return None;
        }
        let report = match ctx.calendar.as_deref() {
            None => arrived((unavailable("calendar", "Calendar"), true)),
            Some(root) => arrived(
                once("calendar", &crate::CALENDAR_PASS, || async {
                    // Fetch and store as one, never across a save: see `CALENDAR_PASS`.
                    let fetched = quince_icloud::sync::fetch_calendar(
                        &ctx.http,
                        root,
                        ctx.params.clone(),
                        &from,
                        &to,
                        &zone,
                    )
                    .await;
                    store(
                        "calendar",
                        "Calendar",
                        data,
                        ctx.account.as_deref(),
                        fetched,
                        |library, snapshot| {
                            let summary = quince_icloud::sync::apply_calendar(library, snapshot)?;
                            crate::outbox::replay(library, crate::outbox::CALENDAR)?;
                            Ok(format!(
                                "{} event{} in {} calendar{}",
                                summary.events,
                                plural(summary.events),
                                summary.calendars,
                                plural(summary.calendars)
                            ))
                        },
                    )
                    .await
                })
                .await,
            ),
        };
        Some(report)
    };

    let reminders = async {
        if !wanted("reminders") {
            return None;
        }
        let report = match ctx.cloudkit.as_deref() {
            None => arrived((unavailable("reminders", "Reminders"), true)),
            Some(_) if waits("reminders", &ctx.waiting) => {
                paused("reminders", "Reminders", data).await
            }
            Some(root) => arrived(
                once("reminders", &crate::REMINDERS_PASS, || async {
                    let since = token(data, "reminders").await;
                    let full = since.is_none();
                    let fetched = quince_icloud::reminders::fetch_cloudkit(
                        &ctx.http,
                        root,
                        ctx.params.clone(),
                        since.as_deref(),
                    )
                    .await;
                    store(
                        "reminders",
                        "Reminders",
                        data,
                        ctx.account.as_deref(),
                        fetched,
                        move |library, f| {
                            let (lists, count, _) =
                                quince_icloud::reminders::apply_cloudkit(library, f)?;
                            // Changes still on their way to Apple stay as made here: without this
                            // a reminder ticked offline showed open again after every pass, and
                            // its alert could ring for something already done.
                            crate::outbox::replay_reminders(library)?;
                            if full {
                                library.mark_full_sync("reminders")?;
                            }
                            Ok(format!(
                                "{count} reminder{} in {lists} list{}",
                                plural(count),
                                plural(lists)
                            ))
                        },
                    )
                    .await
                })
                .await,
            ),
        };
        Some(report)
    };

    let notes = async {
        if !wanted("notes") {
            return None;
        }
        let report = match ctx.cloudkit.as_deref() {
            None => arrived((unavailable("notes", "Notes"), true)),
            Some(_) if waits("notes", &ctx.waiting) => paused("notes", "Notes", data).await,
            Some(root) => arrived(
                once("notes", &crate::NOTES_PASS, || async {
                    // One notes pass at a time, from reading the position through storing what came
                    // back — see `NOTES_PASS`.
                    let since = token(data, "notes").await;
                    let full = since.is_none();
                    let fetched = quince_icloud::notes::fetch_cloudkit(
                        &ctx.http,
                        root,
                        ctx.params.clone(),
                        since.as_deref(),
                    )
                    .await;
                    store(
                        "notes",
                        "Notes",
                        data,
                        ctx.account.as_deref(),
                        fetched,
                        move |library, f| {
                            let (folders, notes, _) =
                                quince_icloud::notes::apply_cloudkit(library, f)?;
                            // Edits still on their way to Apple stay as typed here.
                            crate::outbox::replay(library, crate::outbox::NOTES)?;
                            if full {
                                library.mark_full_sync("notes")?;
                            }
                            Ok(format!(
                                "{notes} note{} in {folders} folder{}",
                                plural(notes),
                                plural(folders)
                            ))
                        },
                    )
                    .await
                })
                .await,
            ),
        };
        Some(report)
    };

    let contacts = async {
        if !wanted("contacts") {
            return None;
        }
        let report = match ctx.contacts.as_deref() {
            None => arrived((unavailable("contacts", "Contacts"), true)),
            Some(root) => arrived(
                once("contacts", &crate::CONTACTS_PASS, || async {
                    // Fetch and store as one, never across a save: see `CONTACTS_PASS`. The book
                    // is read only when Apple says it changed (contacts::pass).
                    let since = token(data, "contacts")
                        .await
                        .and_then(|t| quince_icloud::contacts::Tokens::from_stored(&t));
                    let full = since.is_none();
                    let fetched =
                        quince_icloud::contacts::pass(&ctx.http, root, ctx.params.clone(), since)
                            .await;
                    store(
                        "contacts",
                        "Contacts",
                        data,
                        ctx.account.as_deref(),
                        fetched,
                        move |library, pass| {
                            use quince_icloud::contacts::Pass;
                            let count = match pass {
                                Pass::Read(book, tokens) => {
                                    let n = quince_icloud::contacts::apply(library, book)?;
                                    library.mark_synced("contacts", Some(&tokens.stored()))?;
                                    n
                                }
                                Pass::Unchanged(tokens) => {
                                    library.mark_synced("contacts", Some(&tokens.stored()))?;
                                    let n: i64 = library
                                        .conn()
                                        .query_row("SELECT count(*) FROM contacts", [], |r| {
                                            r.get(0)
                                        })
                                        .map_err(quince_core::Error::from)?;
                                    usize::try_from(n).unwrap_or(0)
                                }
                            };
                            // Cards saved here and still on their way to Apple stay as saved.
                            crate::outbox::replay(library, crate::outbox::CONTACTS)?;
                            if full {
                                library.mark_full_sync("contacts")?;
                            }
                            Ok(format!("{count} contact{}", plural(count)))
                        },
                    )
                    .await
                })
                .await,
            ),
        };
        Some(report)
    };

    // Drive is read a folder at a time as it is opened; the pass keeps its top level current,
    // so what changed there on another device is here before anyone looks, and its freshness
    // is stated with the rest. Fetched and stored as one — see `drive_pass`.
    let drive = async {
        if !wanted("drive") {
            return None;
        }
        let report = match ctx.drive.as_deref() {
            None => arrived((unavailable("drive", "Drive"), true)),
            Some(_) if waits("drive", &ctx.waiting) => paused("drive", "Drive", data).await,
            Some(root) => {
                let pass = crate::drive_pass(quince_core::drive::ROOT);
                arrived(
                    once("drive", &pass, || async {
                        let fetched = quince_icloud::drive::fetch_folder(
                            &ctx.http,
                            root,
                            ctx.params.clone(),
                            quince_core::drive::ROOT,
                        )
                        .await;
                        store(
                            "drive",
                            "Drive",
                            data,
                            ctx.account.as_deref(),
                            fetched,
                            |library, items| {
                                let n = quince_icloud::drive::store_folder(
                                    library,
                                    quince_core::drive::ROOT,
                                    &items,
                                )?;
                                Ok(format!("{n} item{} at the top of iCloud Drive", plural(n)))
                            },
                        )
                        .await
                    })
                    .await,
                )
            }
        };
        Some(report)
    };

    // Photos: the whole library through the change feed once, then only what changed — under
    // half a second when nothing did. Fetched and stored as one: see `PHOTOS_PASS`.
    let photos = async {
        if !wanted("photos") {
            return None;
        }
        let report = match ctx.cloudkit.as_deref() {
            None => arrived((unavailable("photos", "Photos"), true)),
            Some(_) if waits("photos", &ctx.waiting) => paused("photos", "Photos", data).await,
            Some(root) => arrived(
                once("photos", &crate::PHOTOS_PASS, || async {
                    // A large library is read a few pages at a time, each stored before the next
                    // is asked for: the walk holds no more of it in memory than those pages, and
                    // goes as far in one pass as it did when it held all of them at once.
                    let mut changed = false;
                    // What every round changed, put together; read whole once any round can't say.
                    let mut items = Some(Items::default());
                    let mut rounds = 0;
                    loop {
                        rounds += 1;
                        let since = token(data, quince_icloud::photos::SERVICE).await;
                        let full = since.is_none();
                        // A whole read of a library Apple is still preparing finds it empty or
                        // part-made — and a whole read takes what it never sees as gone. Asked
                        // first (as icloud.com asks), and while it is preparing, not read.
                        if full && rounds == 1 {
                            let ready = quince_icloud::photos::readiness(
                                &ctx.http,
                                root,
                                ctx.params.clone(),
                            )
                            .await;
                            if let Some(report) = preparing(data, ready).await {
                                break report;
                            }
                        }
                        let fetched = quince_icloud::photos::fetch(
                            &ctx.http,
                            root,
                            ctx.params.clone(),
                            since.as_deref(),
                        )
                        .await;
                        let more = fetched.as_ref().is_ok_and(|f| !f.complete);
                        let mut report = store(
                            "photos",
                            "Photos",
                            data,
                            ctx.account.as_deref(),
                            fetched,
                            move |library, f| {
                                let records = f.records.len();
                                let (_, links) = quince_icloud::photos::apply(library, f)?;
                                // Marked, or every pass after walked the whole library again:
                                // the daily full read is due when none was ever recorded.
                                if full {
                                    library.mark_full_sync(quince_icloud::photos::SERVICE)?;
                                }
                                crate::photos::remember(links);
                                // Said from what the pass brought: counting the library to
                                // say it held the writer's lock through each of a refresh's ten
                                // rounds (4.4 s each at a million).
                                Ok(if records == 0 {
                                    "Up to date".to_string()
                                } else {
                                    format!("{records} change{} from iCloud", plural(records))
                                })
                            },
                        )
                        .await;
                        changed |= report.changed;
                        if report.changed {
                            items = match (items, report.items.take()) {
                                (Some(all), Some(round)) => joined(all, round),
                                _ => None,
                            };
                        }
                        if report.outcome != Outcome::Ok
                            || !more
                            || rounds >= quince_icloud::photos::PASS_ROUNDS
                        {
                            report.changed = changed;
                            report.items = if changed { items } else { None };
                            // Shared Albums after the library, on a pass that went well: what
                            // changed there redraws Photos whole (it has no rows of its own in
                            // the library's change tracking).
                            if report.outcome == Outcome::Ok
                                && shared_albums_pass(&ctx, root, data).await
                            {
                                report.changed = true;
                                report.items = None;
                            }
                            // The Shared Library's photos are the library's own rows: what
                            // changed redraws Photos whole, as the albums' do.
                            if report.outcome == Outcome::Ok
                                && shared_library_pass(&ctx, root, data).await
                            {
                                report.changed = true;
                                report.items = None;
                            }
                            break report;
                        }
                    }
                })
                .await,
            ),
        };
        Some(report)
    };

    let (calendar, reminders, notes, contacts, drive, photos) =
        tokio::join!(calendar, reminders, notes, contacts, drive, photos);
    // Apple answered: whatever waits to be sent goes now, not on the outbox's own timer.
    if [&calendar, &reminders, &notes, &contacts]
        .iter()
        .any(|r| r.as_ref().is_some_and(|r| r.outcome == Outcome::Ok))
    {
        crate::outbox::kick();
    }
    let reports: Vec<ServiceReport> = [calendar, reminders, notes, contacts, drive, photos]
        .into_iter()
        .flatten()
        .collect();

    let available: Vec<&str> = reports
        .iter()
        .filter(|r| r.outcome != Outcome::Unavailable)
        .map(|r| r.service)
        .collect();
    let as_of = data
        .read(|library| library.oldest_sync(&available))
        .await
        .ok()
        .flatten();
    let changed: Vec<_> = reports
        .iter()
        .filter(|r| r.changed)
        .map(|r| r.service)
        .collect();
    // New or changed photos reach the Photos folder in File Explorer too (Windows), whether or
    // not a window is open; off the async threads, as it touches the disk.
    if changed.contains(&"photos") {
        tauri::async_runtime::spawn_blocking(crate::mount::photos_changed);
    }
    let items = reports
        .iter()
        .filter(|r| r.changed)
        .filter_map(|r| Some((r.service, r.items.clone()?)))
        .collect();

    Refresh {
        services: reports,
        as_of,
        took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        changed: Some(changed),
        items: Some(items),
    }
}

/// [`status`], saying whose rows changed: only those apps' screens read their lists again, and
/// with none named, none does — the freshness alone is new. Sent without it, every open screen
/// read its whole list again (Photos its whole index) for a calendar feed fetched, a
/// subscription loaded or a pass already told as it landed.
pub async fn status_changed(data: &LibraryLock, changed: &[&'static str]) -> Refresh {
    Refresh {
        changed: Some(changed.to_vec()),
        ..status(data).await
    }
}

/// [`status`], for one service that has just landed: whose rows changed and, when known, which
/// of its items — so its screen redraws those rows and every other screen stays as it is.
pub async fn status_landed(
    data: &LibraryLock,
    service: &'static str,
    changed: bool,
    items: Option<Items>,
) -> Refresh {
    Refresh {
        changed: Some(if changed { vec![service] } else { Vec::new() }),
        items: Some(match items {
            Some(items) if changed => BTreeMap::from([(service, items)]),
            _ => BTreeMap::new(),
        }),
        ..status(data).await
    }
}

/// What the library already knows, without asking Apple anything.
///
/// The window renders from this the instant it opens, so freshness is on screen before the
/// first refresh has finished rather than after it.
pub async fn status(data: &LibraryLock) -> Refresh {
    let covered: Vec<&str> = COVERED.iter().map(|(service, _)| *service).collect();
    // Read beside a pass, not behind it: asked as each service lands, it used to wait for the
    // next service's store to finish.
    let (health, as_of) = data
        .read(|library| {
            Ok((
                library.sync_health().unwrap_or_default(),
                library.oldest_sync(&covered).ok().flatten(),
            ))
        })
        .await
        .unwrap_or_default();
    let services = COVERED
        .iter()
        .map(|(service, label)| {
            let found = health.iter().find(|h| h.service == *service);
            let error = found.and_then(|h| h.last_error.clone());
            ServiceReport {
                service,
                label,
                outcome: if error.is_some() {
                    Outcome::Failed
                } else {
                    Outcome::Ok
                },
                detail: error,
                last_synced_at: found.and_then(|h| h.last_synced_at),
                changed: false,
                items: None,
            }
        })
        .collect();
    Refresh {
        services,
        as_of,
        took_ms: 0,
        changed: None,
        items: None,
    }
}

/// Two rounds' items as one: an item two rounds touched is counted once, as changed — its
/// screen reads it again either way. More than are worth placing, and it is read whole.
fn joined(mut all: Items, round: Items) -> Option<Items> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in all.added.iter().chain(&all.changed).chain(&all.removed) {
        seen.insert(id.clone());
    }
    for (id, list) in round
        .added
        .into_iter()
        .map(|id| (id, 0))
        .chain(round.changed.into_iter().map(|id| (id, 1)))
        .chain(round.removed.into_iter().map(|id| (id, 2)))
    {
        if seen.contains(&id) {
            all.added.retain(|x| *x != id);
            all.removed.retain(|x| *x != id);
            if !all.changed.contains(&id) {
                all.changed.push(id);
            }
            continue;
        }
        seen.insert(id.clone());
        match list {
            0 => all.added.push(id),
            1 => all.changed.push(id),
            _ => all.removed.push(id),
        }
    }
    (all.len() <= MOST_ITEMS).then_some(all)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// How long until a library Apple is still preparing is looked at again.
pub const PREPARING_AGAIN: std::time::Duration = std::time::Duration::from_secs(120);

/// Said while Apple prepares the library: calm, and true.
pub const PREPARING_WORDS: &str =
    "Apple is still preparing your photo library. It will appear here when it's ready.";

/// The Photos report while Apple says the library is still being prepared, with another look
/// set for later; `None` when it is ready — or Apple would not say, which leaves the read to
/// go ahead as it always did.
async fn preparing(
    data: &LibraryLock,
    ready: quince_icloud::Result<quince_icloud::photos::Readiness>,
) -> Option<ServiceReport> {
    let Ok(quince_icloud::photos::Readiness::Preparing { .. }) = ready else {
        return None;
    };
    crate::push::again_in("photos", PREPARING_AGAIN);
    let last_synced_at = data
        .read(|library| library.last_synced(quince_icloud::photos::SERVICE))
        .await
        .ok()
        .flatten();
    Some(ServiceReport {
        service: "photos",
        label: "Photos",
        outcome: Outcome::Preparing,
        detail: Some(PREPARING_WORDS.into()),
        last_synced_at,
        changed: false,
        items: None,
    })
}

fn unavailable(service: &'static str, label: &'static str) -> ServiceReport {
    ServiceReport {
        service,
        label,
        outcome: Outcome::Unavailable,
        detail: None,
        last_synced_at: None,
        changed: false,
        items: None,
    }
}

/// Where the last sync got to — or `None` when this service is due a full pass, which is
/// what makes the next fetch walk everything.
async fn token(data: &LibraryLock, service: &str) -> Option<String> {
    // Read beside the other services' stores, which the passes run alongside.
    data.read(|library| {
        if library
            .needs_full_sync(service, FULL_SYNC_EVERY)
            .unwrap_or(false)
        {
            return Ok(None);
        }
        Ok(library.sync_token(service).ok().flatten())
    })
    .await
    .ok()
    .flatten()
}

/// Shared Albums (iOS 27's shared collections): every album found, each read whole when its
/// change feed says it changed, and any no longer there let go of. Answers whether anything
/// was stored. Quiet on failure — the library's own pass speaks for Photos — and a failure
/// leaves every album as it was: nothing is let go of unless Apple's whole list was read.
async fn shared_albums_pass(ctx: &Context, root: &str, data: &LibraryLock) -> bool {
    use quince_icloud::shared_collections::{changed, places, read};
    let found = match places(&ctx.http, root, ctx.params.clone()).await {
        Ok(found) => found,
        Err(e) => {
            quiet!("shared albums: couldn't list them ({e})");
            return false;
        }
    };
    let mut reads = Vec::new();
    for place in &found {
        let token = data
            .read(|l| l.shared_album_token(&place.zone))
            .await
            .ok()
            .flatten();
        match changed(&ctx.http, root, ctx.params.clone(), place, token.as_deref()).await {
            Ok(false) => continue,
            Ok(true) => {}
            Err(e) => {
                quiet!("shared albums: {} ({e})", place.zone);
                continue;
            }
        }
        match read(&ctx.http, root, ctx.params.clone(), place).await {
            Ok(r) => reads.push((place.clone(), r)),
            Err(e) => quiet!("shared albums: couldn't read {} ({e})", place.zone),
        }
    }
    let mut library = data.lock_for_pass().await;
    // Read for an account that is no longer the library's: dropped, as `store` drops it.
    let owner = library.state(crate::ACCOUNT_KEY).ok().flatten();
    if ctx.account.is_none() || owner.as_deref() != ctx.account.as_deref() {
        return false;
    }
    let before = library.shared_albums().unwrap_or_default();
    let zones: Vec<&str> = found.iter().map(|p| p.zone.as_str()).collect();
    let mut links = std::collections::HashMap::new();
    let stored = tokio::task::block_in_place(|| {
        library.atomically(|library| {
            for (place, r) in &reads {
                let album = quince_core::shared::SharedAlbum {
                    zone: place.zone.clone(),
                    database: if place.owned() { "private" } else { "shared" }.into(),
                    owner: place.owner.clone(),
                    title: r.about.title.clone(),
                    expires_at: r.about.expires_at,
                    count: 0,
                    cover: None,
                };
                library.put_shared_album(&album, &r.photos, r.token.as_deref())?;
            }
            library.keep_shared_albums(&zones)
        })
    });
    if let Err(e) = stored {
        quiet!("shared albums: couldn't store them ({e})");
        return false;
    }
    for (_, r) in reads.iter_mut() {
        links.extend(std::mem::take(&mut r.links));
    }
    crate::photos::remember(links);
    !reads.is_empty()
        || library
            .shared_albums()
            .map_or(true, |after| after.len() != before.len())
}

/// The Shared Libraries (family photo libraries) after the personal one: each read through its
/// own change feed, from its own place, a few pages per refresh as the personal library is.
/// Its photos join the library's own rows, marked with its zone. A library this account left
/// or that was ended goes with its photos — but only when Apple's list of them was read: a
/// list that failed lets nothing go. Answers whether anything changed.
async fn shared_library_pass(ctx: &Context, root: &str, data: &LibraryLock) -> bool {
    use quince_icloud::photos::{apply, fetch_in, service_of, shared_libraries, PASS_ROUNDS};
    let found = match shared_libraries(&ctx.http, root, ctx.params.clone()).await {
        Ok(found) => found,
        Err(e) => {
            quiet!("shared library: couldn't list them ({e})");
            return false;
        }
    };
    let same_account = |library: &Library| {
        let owner = library.state(crate::ACCOUNT_KEY).ok().flatten();
        ctx.account.is_some() && owner.as_deref() == ctx.account.as_deref()
    };
    let mut changed = false;
    {
        let mut library = data.lock_for_pass().await;
        if !same_account(&library) {
            return false;
        }
        let before = library.shared_libraries().unwrap_or_default().len();
        let zones: Vec<&str> = found.iter().map(|p| p.zone.as_str()).collect();
        let kept = tokio::task::block_in_place(|| {
            library.atomically(|library| {
                for place in &found {
                    library.put_shared_library(&quince_core::shared::SharedLibrary {
                        zone: place.zone.clone(),
                        database: if place.owned() { "private" } else { "shared" }.into(),
                        owner: place.owner.clone(),
                        title: None,
                    })?;
                }
                library.keep_shared_libraries(&zones)
            })
        });
        if let Err(e) = kept {
            quiet!("shared library: couldn't store the list ({e})");
            return false;
        }
        changed |= before != found.len();
    }
    for place in &found {
        let service = service_of(Some(&place.zone));
        for _ in 0..PASS_ROUNDS {
            let since = data.read(|l| l.sync_token(&service)).await.ok().flatten();
            let fetched = match fetch_in(
                &ctx.http,
                root,
                ctx.params.clone(),
                Some(place),
                since.as_deref(),
            )
            .await
            {
                Ok(f) => f,
                Err(e) => {
                    quiet!("shared library: couldn't read {} ({e})", place.zone);
                    break;
                }
            };
            let more = !fetched.complete;
            let brought = fetched.records.len();
            let mut library = data.lock_for_pass().await;
            if !same_account(&library) {
                return changed;
            }
            match tokio::task::block_in_place(|| apply(&mut library, fetched)) {
                Ok((n, links)) => {
                    changed |= n > 0;
                    crate::photos::remember(links);
                }
                Err(e) => {
                    quiet!("shared library: couldn't store {} ({e})", place.zone);
                    break;
                }
            }
            if !more || brought == 0 {
                break;
            }
        }
    }
    changed
}

/// What every pass writes whatever came back: where it got to, and when.
const BOOKKEEPING: &[&str] = &["sync_state"];

/// Store what a fetch returned, and turn either half's failure into a report.
///
/// The library is taken only here — after the network work is done — and released as soon
/// as the rows are in. Taken as a pass takes it: what it stores is one transaction, and the
/// screens read beside it meanwhile (`LibraryLock::lock_for_pass`).
async fn store<T, F>(
    service: &'static str,
    label: &'static str,
    data: &LibraryLock,
    account: Option<&str>,
    fetched: quince_icloud::Result<T>,
    apply: F,
) -> ServiceReport
where
    F: FnOnce(&mut Library, T) -> quince_icloud::Result<String>,
{
    let mut library = data.lock_for_pass().await;
    // Fetched for an account that is no longer the library's — signed out, or another signed
    // in, while it was on the way: dropped, never laid over someone else's data.
    let owner = library.state(crate::ACCOUNT_KEY).ok().flatten();
    if account.is_none() || owner.as_deref() != account {
        return ServiceReport {
            service,
            label,
            outcome: Outcome::Failed,
            detail: Some("Signed out while this was being read.".into()),
            last_synced_at: None,
            changed: false,
            items: None,
        };
    }
    // SQLite is synchronous: a full pass's apply (a whole photo library, every reminder and its
    // alarms) is CPU and disk work that would otherwise sit on one of the runtime's few
    // workers, stalling every task queued behind it there — the pictures the window is asking
    // for among them. Run in place, its worker hands those tasks on first.
    let started = std::time::Instant::now();
    // And as one change: the records, the changes still waiting laid over them and the new
    // position land together, or — failing, or the app dying part way — none of them do.
    // Whether it stored anything beyond its position is noted on the way, so a pass that found
    // nothing new does not have every screen read its whole list again.
    //
    // Which items it changed is noted too — rows stored again just as they were cancel out —
    // so the screen redraws those rows alone, and a pass that rewrote everything as it was
    // (a whole address book read again for one card's sake) leaves the rest untouched.
    let (result, seen) = tokio::task::block_in_place(|| {
        library.changing_items(BOOKKEEPING, |library| {
            fetched.and_then(|value| library.atomically(|library| apply(library, value)))
        })
    });
    let items = seen.for_service(service).filter(|i| i.len() <= MOST_ITEMS);
    // Written, but nothing it shows is different: no screen has anything to read.
    let changed = seen.written && items.as_ref().is_none_or(|i| !i.is_empty());
    let took = started.elapsed();
    if took > std::time::Duration::from_millis(50) {
        quiet!("{service}: stored in {} ms", took.as_millis());
    }
    match result {
        Ok(detail) => ServiceReport {
            service,
            label,
            outcome: Outcome::Ok,
            detail: Some(detail),
            last_synced_at: library.last_synced(service).ok().flatten(),
            changed,
            items: if changed { items } else { None },
        },
        Err(e) => {
            // Advanced Data Protection keeping it out: the approval is asked for after the pass,
            // and the service read again once it is given (`data_access`).
            if matches!(e, quince_icloud::Error::NeedsDeviceApproval) {
                crate::data_access::refused(service);
                return ServiceReport {
                    service,
                    label,
                    outcome: Outcome::Paused,
                    detail: Some(PAUSED_WORDS.into()),
                    last_synced_at: library.last_synced(service).ok().flatten(),
                    changed: false,
                    items: None,
                };
            }
            let message = e.to_string();
            library.mark_sync_failed(service, &message).ok();
            ServiceReport {
                service,
                label,
                outcome: Outcome::Failed,
                detail: Some(message),
                last_synced_at: library.last_synced(service).ok().flatten(),
                changed: false,
                items: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {

    /// The sidebar's apps (Library.tsx's SCREENS) and the services they read (APP_SERVICES)
    /// are one list: an app added or renamed in one and not the other fails here, never in a
    /// person's sidebar.
    #[test]
    fn every_sidebar_app_has_its_service_and_no_other() {
        let screens = include_str!("../../src/screens/Library.tsx");
        let start = screens
            .find("const SCREENS")
            .expect("SCREENS in Library.tsx");
        let list = &screens[start..start + screens[start..].find("];").expect("end of SCREENS")];
        let keys: Vec<&str> = list
            .split("key: \"")
            .skip(1)
            .filter_map(|k| k.split('"').next())
            .collect();
        let ours: Vec<&str> = APP_SERVICES.iter().map(|(app, _)| *app).collect();
        assert_eq!(
            keys, ours,
            "Library.tsx's SCREENS and APP_SERVICES must list the same apps"
        );
    }

    /// An app shows when the service its data comes from is offered, whatever else is listed:
    /// an account whose list has CloudKit but no `notes` still gets Notes (#9); one without
    /// CloudKit gets none of the three that live there.
    #[test]
    fn an_app_shows_when_the_service_it_reads_is_there() {
        let list = |w: &[&str]| w.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let offered = apps_offered(&list(&[
            "ckdatabasews",
            "reminders",
            "photos",
            "calendar",
            "contacts",
            "drivews",
        ]));
        assert_eq!(
            offered,
            [
                "reminders",
                "notes",
                "drivews",
                "photos",
                "calendar",
                "contacts"
            ]
        );
        let offered = apps_offered(&list(&["calendar", "contacts", "notes", "reminders"]));
        assert_eq!(offered, ["calendar", "contacts"]);
    }
    use super::*;

    /// Apple still preparing the library: the pass reads nothing (so stores no empty library
    /// over the one being made), says so calmly as its own state rather than a failure or an
    /// empty library, and sets another look for later. Ready, or Apple not saying, the read
    /// goes ahead.
    #[tokio::test]
    async fn a_library_apple_is_still_preparing_waits_and_says_so() {
        use quince_icloud::photos::Readiness;
        let data = LibraryLock::new(quince_core::Library::in_memory().unwrap(), None);
        let report = preparing(&data, Ok(Readiness::Preparing { progress: Some(0) }))
            .await
            .expect("not read while preparing");
        assert_eq!(report.outcome, Outcome::Preparing);
        assert_eq!(
            report.detail.as_deref(),
            Some(
                "Apple is still preparing your photo library. It will appear here when it's ready."
            )
        );
        assert!(!report.changed);
        assert_eq!(
            serde_json::to_value(&report).unwrap()["outcome"],
            "preparing",
            "the window's name for it"
        );
        assert!(
            crate::push::looks_set().contains(&"photos"),
            "looked at again later"
        );
        assert!(preparing(&data, Ok(Readiness::Ready)).await.is_none());
        assert!(preparing(&data, Err(quince_icloud::Error::Busy))
            .await
            .is_none());
    }

    /// A freshness sent with the services it changed names exactly those, and one that
    /// changed nothing names none — never "unknown", which has every screen read again.
    #[tokio::test]
    async fn a_status_sent_names_whose_rows_changed() {
        let data = LibraryLock::new(quince_core::Library::in_memory().unwrap(), None);
        let told = status_changed(&data, &["calendar"]).await;
        assert_eq!(told.changed, Some(vec!["calendar"]));
        let json = serde_json::to_value(&told).unwrap();
        assert_eq!(json["changed"], serde_json::json!(["calendar"]));
        let quiet = status_changed(&data, &[]).await;
        assert_eq!(quiet.changed, Some(Vec::new()));
        assert_eq!(
            serde_json::to_value(&quiet).unwrap()["changed"],
            serde_json::json!([])
        );
        assert_eq!(quiet.services.len(), status(&data).await.services.len());
    }

    /// A service landing names its items with it — for its own screen alone — and one that
    /// stored nothing new names none.
    #[tokio::test]
    async fn a_landing_names_the_items_it_changed() {
        let data = LibraryLock::new(quince_core::Library::in_memory().unwrap(), None);
        let items = Items {
            added: vec!["A".into()],
            changed: vec!["B".into()],
            removed: vec![],
        };
        let told = status_landed(&data, "contacts", true, Some(items)).await;
        let json = serde_json::to_value(&told).unwrap();
        assert_eq!(json["changed"], serde_json::json!(["contacts"]));
        assert_eq!(
            json["items"],
            serde_json::json!({"contacts": {"added": ["A"], "changed": ["B"], "removed": []}})
        );
        let whole = status_landed(&data, "contacts", true, None).await;
        assert_eq!(
            serde_json::to_value(&whole).unwrap()["items"],
            serde_json::json!({}),
            "changed, with no items named: read whole"
        );
        let quiet = status_landed(&data, "contacts", false, None).await;
        assert_eq!(
            serde_json::to_value(&quiet).unwrap()["changed"],
            serde_json::json!([])
        );
    }

    /// Photos' rounds put together: an item two rounds touched is one change, and too many
    /// are read whole.
    #[test]
    fn rounds_join_into_one_set_of_items() {
        let ids = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let first = Items {
            added: ids(&["a"]),
            changed: ids(&["b"]),
            removed: vec![],
        };
        let second = Items {
            added: ids(&["c"]),
            changed: ids(&["a"]),
            removed: ids(&["d"]),
        };
        let all = joined(first, second).unwrap();
        assert_eq!(all.added, ids(&["c"]));
        assert_eq!(all.changed, ids(&["b", "a"]));
        assert_eq!(all.removed, ids(&["d"]));
        let many = Items {
            added: (0..=MOST_ITEMS).map(|n| n.to_string()).collect(),
            changed: vec![],
            removed: vec![],
        };
        assert_eq!(joined(Items::default(), many), None);
    }

    #[test]
    fn the_default_window_reaches_back_a_month_and_forward_a_quarter() {
        let (from, to, _zone) = default_window();
        let start: jiff::civil::Date = from.parse().unwrap();
        let end: jiff::civil::Date = to.parse().unwrap();
        let today = jiff::Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::system())
            .date();

        assert!(
            start < today,
            "the window must include what already happened"
        );
        assert!(end > today, "and what is coming");
        assert_eq!(start.day(), 1, "a month starts on the first");
        let days = (end - start).get_days();
        assert!(
            (110..=160).contains(&days),
            "about five months of window, got {days}"
        );
    }

    /// The window is the same shape wherever the machine is, so a refresh never silently
    /// covers less for someone in a different timezone.
    #[test]
    fn the_window_is_a_whole_number_of_months() {
        let (from, to, _) = default_window();
        let start: jiff::civil::Date = from.parse().unwrap();
        let end: jiff::civil::Date = to.parse().unwrap();
        assert_eq!(start.day(), 1);
        assert_eq!(
            end.day(),
            end.last_of_month().day(),
            "the window ends on a month boundary"
        );
    }

    #[test]
    fn counts_read_as_english() {
        assert_eq!(plural(1), "");
        assert_eq!(plural(0), "s");
        assert_eq!(plural(2), "s");
    }

    /// Five asks while a pass is running cost that pass and one more, not five: each that
    /// waited takes the report of the pass that started after it asked. An ask after that
    /// one has finished runs a pass of its own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_burst_of_asks_costs_the_running_pass_and_one_more() {
        use std::sync::atomic::AtomicUsize;
        static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        static RUNS: AtomicUsize = AtomicUsize::new(0);
        let gate = Arc::new(tokio::sync::Notify::new());
        let pass = |gate: Option<Arc<tokio::sync::Notify>>| {
            || async move {
                let n = RUNS.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                let mut report = unavailable("contacts", "Contacts");
                report.detail = Some(format!("pass {n}"));
                report
            }
        };
        let first = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { once("contacts", &LOCK, pass(Some(gate))).await })
        };
        while RUNS.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        let waiting: Vec<_> = (0..5)
            .map(|_| tokio::spawn(async move { once("contacts", &LOCK, pass(None)).await }))
            .collect();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        gate.notify_one();
        let (report, ran) = first.await.unwrap();
        assert!(ran);
        assert_eq!(report.detail.as_deref(), Some("pass 1"));
        let mut ran_here = 0;
        for w in waiting {
            let (report, ran) = w.await.unwrap();
            ran_here += usize::from(ran);
            assert_eq!(
                report.detail.as_deref(),
                Some("pass 2"),
                "every waiting ask is answered by the pass after the one running"
            );
        }
        assert_eq!(ran_here, 1);
        assert_eq!(RUNS.load(Ordering::SeqCst), 2);

        let (report, ran) = once("contacts", &LOCK, pass(None)).await;
        assert!(ran, "an ask after the burst runs its own pass");
        assert_eq!(report.detail.as_deref(), Some("pass 3"));
    }
}

#[cfg(test)]
mod paused_tests {
    use super::*;

    #[test]
    fn a_waiting_service_is_left_alone_and_the_rest_are_not() {
        let waiting = ["notes", "reminders", "photos", "drive"];
        for s in waiting {
            assert!(waits(s, &waiting));
        }
        for s in ["calendar", "contacts"] {
            assert!(
                !waits(s, &waiting),
                "{s} is never held by Advanced Data Protection"
            );
        }
        assert!(!waits("notes", &[]), "approved: nothing is left alone");
    }

    #[test]
    fn waiting_reads_as_waiting_not_failure() {
        assert_eq!(serde_json::to_value(Outcome::Paused).unwrap(), "paused");
        assert!(!PAUSED_WORDS.to_lowercase().contains("error"));
    }
}
