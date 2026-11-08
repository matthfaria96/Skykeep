//! Open at Login: Quince started in the background when the person logs in — its window
//! hidden, everything behind it running — so reminders ring, changes sync and the folders are
//! up without the window ever being opened. Kept the standard way, as an XDG autostart entry
//! (`~/.config/autostart`), which the desktops run themselves; nothing of Quince's own runs it.
//!
//! Some compositors run no autostart entries (Hyprland, Sway and the like, unless started
//! through uwsm). There Settings says so, with the one line that does the same in their
//! own startup — written by the person, never by Quince into their config.
//!
//! On Windows it is the standard place too: a value under the user's own `Run` key, which
//! Windows reads at sign-in (and lists in Task Manager's Startup apps, where it can be turned
//! off as any other). From an MSIX package (the Microsoft Store, package.rs) the registry is
//! the package's own, which Windows never reads at sign-in: there it is the package's startup
//! task (`SkykeepStartup` in packaging/windows/msix/AppxManifest.xml, started with
//! `--background`), turned on and off through Windows' StartupTask API, and its state —
//! including turned off in Task Manager or by an organization's policy — is what Settings shows.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

/// Started by the login entry: the window stays hidden unless something needs the person.
static BACKGROUND: AtomicBool = AtomicBool::new(false);
/// Whether the first window_ready has been answered: only that one may keep it hidden.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Started by the session bus for the desktop's search (`--search-provider`): it answers
/// searches and shows nothing until it is asked to.
static SEARCH_ONLY: AtomicBool = AtomicBool::new(false);
/// A window has been shown this run — asked for by the person, never by a launch.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// What the login entry passes to start hidden.
const FLAG: &str = "--background";
/// What the search provider's bus activation passes. Not `--background`: on GNOME there is no
/// tray, and a background launch with nowhere to wait shows its window — so every search in
/// the overview after the person had quit Skykeep brought its window up, or the sign-in screen.
pub const SEARCH_FLAG: &str = "--search-provider";

/// Read once at launch, from the command line.
pub fn note_launch() {
    BACKGROUND.store(std::env::args().any(|a| a == FLAG), Ordering::SeqCst);
    SEARCH_ONLY.store(std::env::args().any(|a| a == SEARCH_FLAG), Ordering::SeqCst);
}

/// Whether this run was started at login, hidden.
pub fn background() -> bool {
    BACKGROUND.load(Ordering::SeqCst)
}

/// Whether this run was started by a search in the desktop's overview.
pub fn search_only() -> bool {
    SEARCH_ONLY.load(Ordering::SeqCst)
}

/// A window was shown.
pub fn shown() {
    SHOWN.store(true, Ordering::SeqCst);
}

/// Whether any window has been shown this run.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn was_shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// Whether the window should stay hidden now: started at login and nothing needs the person
/// (a code to enter, a sign-in), with a tray to come back from; or started for a search,
/// whatever it needs — nobody asked for a window. Only the first answer can be yes; after
/// that the window shows as usual.
pub fn stay_hidden(needs_you: bool, tray: impl FnOnce() -> bool) -> bool {
    if !(background() || search_only()) || ASKED.swap(true, Ordering::SeqCst) {
        return false;
    }
    hides(search_only(), needs_you, tray)
}

/// [`stay_hidden`]'s answer for a first launch started hidden. The tray is asked last, and
/// only when it matters: it is a call on the session bus.
fn hides(search_only: bool, needs_you: bool, tray: impl FnOnce() -> bool) -> bool {
    search_only || (!needs_you && tray())
}

/// How long a Skykeep started for a search goes on with no window shown and no search asked
/// before it ends: the person quit it, and a search does not change that.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const SEARCH_IDLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether a run started for a search is done: no window shown, nothing asked for a while.
#[must_use]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn search_done(shown: bool, idle: std::time::Duration) -> bool {
    !shown && idle >= SEARCH_IDLE
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItem {
    pub on: bool,
    /// The command the entry runs, for a startup file that does not read autostart entries.
    pub command: String,
    /// Whether this desktop runs autostart entries at login. `false`: the entry alone does
    /// nothing here, and `command` has to go in the compositor's own startup.
    pub honoured: bool,
    /// Whether Skykeep can keep the entry itself. Not in a Flatpak: the entry was written in the
    /// sandbox's own config folder, which the desktop never reads, and the switch said "on"
    /// over nothing at all. There Settings says how to add it instead.
    pub available: bool,
    /// Why the switch cannot change it here: Windows holds the package's startup task turned
    /// off in Task Manager, or on or off by policy. `None` anywhere else.
    pub elsewhere: Option<String>,
}

/// What Windows says of a package's startup task: its `StartupTaskState`.
#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    Disabled,
    DisabledByUser,
    Enabled,
    DisabledByPolicy,
    EnabledByPolicy,
}

#[cfg(any(windows, test))]
impl Startup {
    /// From `StartupTaskState`'s value; `None` for one this build does not know.
    pub fn from_state(state: i32) -> Option<Self> {
        Some(match state {
            0 => Self::Disabled,
            1 => Self::DisabledByUser,
            2 => Self::Enabled,
            3 => Self::DisabledByPolicy,
            4 => Self::EnabledByPolicy,
            _ => return None,
        })
    }

    /// Whether Skykeep opens at login, and why the switch cannot change that, if it cannot. A
    /// task turned off in Task Manager can only be turned on there again: Windows refuses the
    /// app's own request.
    pub fn item(self) -> (bool, Option<&'static str>) {
        match self {
            Self::Disabled => (false, None),
            Self::Enabled => (true, None),
            Self::DisabledByUser => (
                false,
                Some("Turned off in Task Manager. To open Skykeep at login, turn it on in Task Manager’s Startup apps."),
            ),
            Self::DisabledByPolicy => (false, Some("Turned off by your organization.")),
            Self::EnabledByPolicy => (true, Some("Turned on by your organization.")),
        }
    }
}

/// How this copy opens at login on Windows: the package's startup task when it runs with a
/// package identity (only then does the StartupTask API answer), else the `Run` key.
#[cfg(any(windows, test))]
fn uses_startup_task(package_identity: bool) -> bool {
    package_identity
}

/// Running inside a Flatpak.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

#[cfg(not(windows))]
fn entry(app: &AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path().config_dir().ok().map(|c| {
        c.join("autostart")
            .join(format!("{}.desktop", app.config().identifier))
    })
}

/// The command that starts this Quince hidden: the AppImage itself when run from one (its
/// own binary lives in a mount that is gone after a restart), else the program running now.
fn command() -> String {
    let program = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok().map(|p| still_installed(&p)))
        .unwrap_or_else(|| PathBuf::from("quince-app"));
    // Windows reads the Run value as a command line: the path in plain quotes.
    if cfg!(windows) {
        return format!("\"{}\" {FLAG}", program.to_string_lossy());
    }
    format!("{} {FLAG}", quoted(&program.to_string_lossy()))
}

/// The program as installed: replaced by a package upgrade while running, Linux names the
/// running one `/usr/bin/skykeep (deleted)`, and an entry written then pointed at nothing.
fn still_installed(path: &std::path::Path) -> PathBuf {
    path.to_str()
        .and_then(|p| p.strip_suffix(" (deleted)"))
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

/// A program path as the Desktop Entry spec wants it in Exec: in double quotes, with `"`,
/// `` ` ``, `$` and `\` escaped, and `%` doubled — alone it starts a field code (`%f`, `%u`),
/// which the launcher would replace with something else.
fn quoted(path: &str) -> String {
    let mut out = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        if c == '%' {
            out.push('%');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(not(windows))]
fn desktop_file(command: &str, icon: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Skykeep\nComment=Keeps iCloud in sync in the background\nExec={command}\nIcon={icon}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
    )
}

#[cfg(not(windows))]
/// Desktops that run autostart entries themselves.
const RUNS_AUTOSTART: [&str; 10] = [
    "GNOME",
    "KDE",
    "XFCE",
    "X-Cinnamon",
    "MATE",
    "LXQt",
    "LXDE",
    "Budgie",
    "COSMIC",
    "Unity",
];

#[cfg(not(windows))]
/// Whether autostart entries run at login here: a desktop that runs them, or a session that
/// started systemd's autostart target (uwsm, and the desktops that use it).
fn honoured() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop
        .split(':')
        .any(|d| RUNS_AUTOSTART.iter().any(|k| d.eq_ignore_ascii_case(k)))
    {
        return true;
    }
    std::process::Command::new("systemctl")
        .args([
            "--user",
            "is-active",
            "--quiet",
            "xdg-desktop-autostart.target",
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// The user's own `Run` key, and the value Quince keeps there.
#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "Skykeep";

/// Windows' own `reg` tool, with no window flashing up.
#[cfg(windows)]
fn reg(args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
}

/// The package's startup task, through Windows' own API. Each call blocks on Windows' answer,
/// so it is made off the async runtime.
#[cfg(windows)]
mod startup_task {
    use super::Startup;
    use windows::core::HSTRING;
    use windows::ApplicationModel::{StartupTask, StartupTaskState};

    /// The TaskId in the package's manifest.
    const ID: &str = "SkykeepStartup";

    fn task() -> windows::core::Result<StartupTask> {
        StartupTask::GetAsync(&HSTRING::from(ID))?.join()
    }

    fn known(state: StartupTaskState) -> Result<Startup, String> {
        Startup::from_state(state.0)
            .ok_or_else(|| format!("Windows says the startup task is in state {}.", state.0))
    }

    pub fn state() -> Result<Startup, String> {
        let task = task().map_err(|e| e.message())?;
        known(task.State().map_err(|e| e.message())?)
    }

    /// Asked on or off; what Windows then says, which may be other than asked (turned off in
    /// Task Manager, or held by policy).
    pub fn set(on: bool) -> Result<Startup, String> {
        let task = task().map_err(|e| e.message())?;
        if on {
            let state = task
                .RequestEnableAsync()
                .and_then(|op| op.join())
                .map_err(|e| e.message())?;
            return known(state);
        }
        // Held on by policy, Disable is refused; the state read next says why.
        if let Err(e) = task.Disable() {
            quiet!("the startup task was not turned off: {e}");
        }
        known(task.State().map_err(|e| e.message())?)
    }
}

/// The item Settings shows for the package's startup task in `state`.
#[cfg(windows)]
fn startup_item(state: Startup) -> LoginItem {
    let (on, elsewhere) = state.item();
    LoginItem {
        on,
        command: command(),
        honoured: true,
        available: true,
        elsewhere: elsewhere.map(str::to_string),
    }
}

#[cfg(windows)]
#[tauri::command]
pub async fn login_item_status(_app: AppHandle) -> Result<LoginItem, String> {
    if uses_startup_task(crate::package::family_name().is_some()) {
        let state = tauri::async_runtime::spawn_blocking(startup_task::state)
            .await
            .map_err(|e| e.to_string())??;
        return Ok(startup_item(state));
    }
    let on = tauri::async_runtime::spawn_blocking(|| {
        reg(&["query", RUN_KEY, "/v", RUN_VALUE]).is_ok_and(|s| s.success())
    })
    .await
    .unwrap_or(false);
    Ok(LoginItem {
        on,
        command: command(),
        honoured: true,
        available: true,
        elsewhere: None,
    })
}

#[cfg(windows)]
#[tauri::command]
pub async fn login_item_set(on: bool, app: AppHandle) -> Result<LoginItem, String> {
    if uses_startup_task(crate::package::family_name().is_some()) {
        let state = tauri::async_runtime::spawn_blocking(move || startup_task::set(on))
            .await
            .map_err(|e| e.to_string())??;
        return Ok(startup_item(state));
    }
    let line = command();
    let done = tauri::async_runtime::spawn_blocking(move || {
        if on {
            reg(&[
                "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &line, "/f",
            ])
        } else {
            reg(&["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    // Turning off what was never on is not a failure.
    if on && !done.success() {
        return Err("Windows did not keep the startup entry.".into());
    }
    login_item_status(app).await
}

#[cfg(not(windows))]
#[tauri::command]
pub async fn login_item_status(app: AppHandle) -> Result<LoginItem, String> {
    if in_flatpak() {
        return Ok(LoginItem {
            on: false,
            command: format!("flatpak run {}", app.config().identifier),
            honoured: true,
            available: false,
            elsewhere: None,
        });
    }
    let on = entry(&app).is_some_and(|p| p.exists());
    let honoured = tauri::async_runtime::spawn_blocking(honoured)
        .await
        .unwrap_or(false);
    Ok(LoginItem {
        on,
        command: command(),
        honoured,
        available: true,
        elsewhere: None,
    })
}

#[cfg(not(windows))]
#[tauri::command]
pub async fn login_item_set(on: bool, app: AppHandle) -> Result<LoginItem, String> {
    if in_flatpak() {
        return Err("Add Skykeep to your desktop’s startup apps to open it at login.".into());
    }
    let path = entry(&app).ok_or("There is no configuration folder to keep it in.")?;
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, desktop_file(&command(), &app.config().identifier))
            .map_err(|e| e.to_string())?;
    } else if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    login_item_status(app).await
}

// Its one check is of the Linux autostart entry, which Windows does not have.
#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    /// The Exec line survives any path: spaces quoted, the spec's special characters escaped.
    #[test]
    fn the_exec_line_is_quoted_as_the_spec_says() {
        assert_eq!(
            quoted("/opt/Quince App/quince"),
            "\"/opt/Quince App/quince\""
        );
        assert_eq!(quoted("/a/$HOME\"x`\\"), "\"/a/\\$HOME\\\"x\\`\\\\\"");
        assert_eq!(quoted("/opt/100%u/q"), "\"/opt/100%%u/q\"");
        let file = desktop_file("\"/usr/bin/quince-app\" --background", "org.example.App");
        assert!(file.contains("Exec=\"/usr/bin/quince-app\" --background\n"));
        assert!(file.starts_with("[Desktop Entry]\nType=Application\n"));
    }
}

#[cfg(test)]
mod search_launch_tests {
    use super::*;

    /// Every state Windows reports for the package's startup task, as Settings shows it: on or
    /// off, and — when Task Manager or a policy holds it — why the switch cannot change it.
    #[test]
    fn the_startup_task_state_as_settings_shows_it() {
        let states: Vec<_> = (0..5).map(|i| Startup::from_state(i).unwrap()).collect();
        assert_eq!(
            states,
            [
                Startup::Disabled,
                Startup::DisabledByUser,
                Startup::Enabled,
                Startup::DisabledByPolicy,
                Startup::EnabledByPolicy
            ]
        );
        assert_eq!(Startup::from_state(5), None);
        assert_eq!(Startup::from_state(-1), None);
        assert_eq!(Startup::Enabled.item(), (true, None));
        assert_eq!(Startup::Disabled.item(), (false, None));
        let (on, why) = Startup::DisabledByUser.item();
        assert!(!on && why.unwrap().contains("Task Manager"));
        let (on, why) = Startup::DisabledByPolicy.item();
        assert!(!on && why.unwrap().contains("organization"));
        let (on, why) = Startup::EnabledByPolicy.item();
        assert!(on && why.unwrap().contains("organization"));
    }

    /// Only a copy with a package identity uses the startup task; the .exe and the .msi keep
    /// the Run key.
    #[test]
    fn only_a_package_uses_the_startup_task() {
        assert!(uses_startup_task(true));
        assert!(!uses_startup_task(false));
    }

    /// Started by a search on GNOME (no tray): no window, signed in or not. It used to show
    /// the main window — or the sign-in screen — on every search after a quit.
    #[test]
    fn a_launch_for_a_search_shows_nothing() {
        assert!(hides(true, true, || false));
        assert!(hides(true, false, || false));
        // At login: hidden only with a tray to come back from, and nothing needing the person.
        assert!(hides(false, false, || true));
        assert!(!hides(false, false, || false));
        assert!(!hides(false, true, || true));
    }

    #[test]
    fn a_search_launch_ends_once_idle_unless_a_window_was_asked_for() {
        assert!(!search_done(false, SEARCH_IDLE / 2));
        assert!(search_done(false, SEARCH_IDLE));
        assert!(!search_done(true, SEARCH_IDLE * 10));
    }

    #[test]
    fn a_program_upgraded_while_running_is_named_as_installed() {
        assert_eq!(
            still_installed(std::path::Path::new("/usr/bin/skykeep (deleted)")),
            PathBuf::from("/usr/bin/skykeep")
        );
        assert_eq!(
            still_installed(std::path::Path::new("/opt/Sky keep/skykeep")),
            PathBuf::from("/opt/Sky keep/skykeep")
        );
    }

    /// The bus starts it with the search flag, never the login one.
    #[test]
    fn the_bus_activation_starts_it_for_search() {
        let service = include_str!(
            "../../packaging/io.github.yashwanthkamireddi.Skykeep.SearchProvider.service"
        );
        let exec = service
            .lines()
            .find_map(|l| l.strip_prefix("Exec="))
            .unwrap();
        assert!(exec.ends_with(SEARCH_FLAG), "{exec}");
        assert!(!exec.contains(FLAG), "{exec}");
    }
}
