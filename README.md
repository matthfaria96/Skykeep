<p align="center">
  <a href="https://github.com/matthfaria96/Skykeep/releases/latest">
    <img alt="Skykeep" src="macos/Resources/AppIcon-1024.png" width="180">
  </a>
</p>

<h1 align="center">Skykeep for Mac</h1>

<p align="center"><b>English</b> · <a href="README.pt-BR.md">Português</a></p>

<p align="center">
  <b>Your whole iCloud, in one window.</b><br>
  Mail, Contacts, Reminders, Calendar, Notes, Photos, iCloud Drive and Passwords<br>
  in a single native app, with a home screen that shows your day at a glance.
</p>

<p align="center">
  <img alt="Native Swift" src="https://img.shields.io/badge/Swift-native-F05138?style=flat-square&logo=swift&logoColor=white">
  <img alt="macOS 26+" src="https://img.shields.io/badge/macOS%2026%2B-Apple%20Silicon%20%C2%B7%20Intel-26282e?style=flat-square&logo=apple">
  <img alt="Free" src="https://img.shields.io/badge/price-free-2ea44f?style=flat-square">
  <img alt="No ads, no tracking" src="https://img.shields.io/badge/no-ads%20%C2%B7%20tracking-3a3a3a?style=flat-square">
</p>

<p align="center">
  <a href="https://github.com/matthfaria96/Skykeep/releases/latest"><b>Download</b></a> ·
  <a href="#what-it-does">What it does</a> ·
  <a href="#what-this-fork-adds">What this fork adds</a> ·
  <a href="#installing-on-macos">How to install</a> ·
  <a href="#credits-and-license">Credits</a>
</p>

> [!NOTE]
> A fork of [Skykeep](https://github.com/YashwanthKamireddi/Skykeep) by Yashwanth Kamireddi (iCloud for
> Windows and Linux), brought to the Mac by [Matheus Faria](https://github.com/matthfaria96).
> No Skykeep account, no servers, no ads, no tracking. Part of [witchmymac](https://witchmymac.com/).

<p align="center">
  <img src="macos/docs/calendar.jpg" alt="Skykeep for Mac: the Calendar in month view" width="860">
</p>

## What it does

**Your iCloud, without the tabs.** Nine apps' worth of iCloud in one place, opening instantly. Notes
and Mail are read and written online in iCloud, so a checklist ticked here is ticked on your iPhone,
and no other app is ever opened.

🏠 Home · ✉️ Mail · 👤 Contacts · ✅ Reminders · 📅 Calendar · 📝 Notes · 🖼️ Photos ·
☁️ iCloud Drive · 🔑 Passwords · 🔎 Search everything · 📍 Menu bar

- **Your day at a glance.** What's next on the calendar (with a Join button for video calls), today's reminders, unread mail, the latest notes, birthdays, photos from this day in other years and recent files.
- **Notes as they were born.** Titles, lists, colours, links, pictures and checklists you can tick, edited in place and saved to iCloud on their own.
- **Real mail.** Read, write, reply, forward, flag, archive and delete on the iCloud server, with remote images and trackers blocked.
- **The Mac's own data.** Reminders, Calendar, Contacts, Photos and iCloud Drive straight from macOS, with Quick Look and in-app maps.
- **Passwords everywhere.** Touch ID, a strong-password generator, and your other browsers connected to Apple's Passwords app.

## What this fork adds

| | Change |
|:---:|---|
| 🍎 | **Skykeep for Mac** ([`macos/`](macos/README.md)): a native SwiftUI app with all nine apps in one window and the *Início* home screen. |
| ☁️ | **Notes and Mail online**, through `skykeep-cloud` ([`macos/bridge/`](macos/bridge)), a helper built from the original project's Rust iCloud code: Notes over CloudKit, Mail over IMAP and SMTP. |
| 🔑 | **Browsers in Apple's Passwords app:** Vivaldi, Chrome, Brave, Edge, Arc and Opera connected to it, and their saved passwords imported. |
| 🎨 | **A new icon** (a cloud kept by a keyhole, under a rising sun) for macOS, Windows and Linux: app, tray and notification icons. |
| ⚙️ | **macOS builds on GitHub Actions** ([macos.yml](.github/workflows/macos.yml)): every push builds the app and its `.dmg`. |

The Windows and Linux code is the original project's, kept up to date with it.

## Installing on macOS

Requires macOS 26 Tahoe or later (Apple Silicon and Intel). The quickest way is one line in Terminal:

```sh
curl -fsSL https://witchmymac.com/install.sh | bash -s skykeep
```

It downloads the latest release, checks its SHA-256, installs Skykeep in Applications and opens it,
with no macOS warning. Run it again to update.

Or download the `.dmg` from the [latest release](https://github.com/matthfaria96/Skykeep/releases/latest),
drag Skykeep to Applications and allow it once:

```sh
xattr -dr com.apple.quarantine /Applications/Skykeep.app
```

The full guide (first launch step by step, first steps in each app, FAQ, building) is in
[macos/README.md](macos/README.md).

> [!TIP]
> **On Windows or Linux?** Use the original [Skykeep](https://github.com/YashwanthKamireddi/Skykeep#install).

## Credits and license

Skykeep for Mac is made by [Matheus Faria](https://github.com/matthfaria96). Skykeep, and the iCloud
code this app builds on, is by [Yashwanth Kamireddi](https://github.com/YashwanthKamireddi).

Free software under the [GNU General Public License v3.0](LICENSE). Copyright (C) 2026 Yashwanth
Kamireddi; Skykeep for Mac, the new icon and the other changes in this fork copyright (C) 2026
Matheus Faria. Not affiliated with or endorsed by Apple Inc.
