<p align="center">
  <a href="https://github.com/matthfaria96/Skykeep/releases/latest">
    <img alt="Skykeep" src="Resources/AppIcon-1024.png" width="180">
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
  <a href="#installing-on-macos">How to install</a> ·
  <a href="#first-steps">First steps</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="#building">Building</a>
</p>

> [!NOTE]
> **No Skykeep account, no servers, no ads, no tracking.** Your data goes between your Mac and
> Apple only. Free and open source (GPL-3.0), part of [witchmymac](https://witchmymac.com/).

<p align="center">
  <img src="docs/calendar.jpg" alt="Skykeep for Mac: the Calendar in month view" width="860">
</p>

## What it does

**Your iCloud, without the tabs.** Nine apps' worth of iCloud in one place, opening instantly. Notes
and Mail are read and written online in iCloud, so a checklist ticked here is ticked on your iPhone,
and no other app is ever opened.

🏠 Home · ✉️ Mail · 👤 Contacts · ✅ Reminders · 📅 Calendar · 📝 Notes · 🖼️ Photos ·
☁️ iCloud Drive · 🔑 Passwords · 🔎 Search everything · 📍 Menu bar

- **Your day at a glance.** The home screen shows what's next on the calendar (with a Join button for video calls), today's reminders, unread mail, your latest notes, birthdays, photos from this day in other years and recent files.
- **Notes as they were born.** Titles, lists, colours, links, pictures and checklists you can tick, edited in place and saved to iCloud on their own, on every device.
- **Real mail.** Read, write, reply, forward, flag, archive and delete, on the iCloud server, with remote images and trackers blocked.
- **The Mac's own data.** Reminders, Calendar, Contacts, Photos and iCloud Drive straight from macOS, already in sync, with Quick Look and in-app maps.
- **Passwords everywhere.** Touch ID, a strong-password generator, and Vivaldi, Chrome, Brave, Edge, Arc and Opera connected to Apple's Passwords app, both ways.

<p align="center"><a href="https://github.com/matthfaria96/Skykeep/releases/latest"><b>Download Skykeep</b></a> · <a href="https://github.com/matthfaria96/Skykeep/tree/main/macos">Source code</a> · macOS 26 Tahoe or later</p>

## Installing on macOS

**The quickest way: one line in Terminal.**

```sh
curl -fsSL https://witchmymac.com/install.sh | bash -s skykeep
```

It downloads the latest release from GitHub, checks its SHA-256, installs Skykeep in Applications
and opens it, with no macOS warning (files downloaded by `curl` don't get the quarantine mark). Run
it again to update. The script is open: [install.sh](https://witchmymac.com/install.sh).

**Or by hand: three steps, thirty seconds.**

1. **Download the `.dmg`** from the [latest release](https://github.com/matthfaria96/Skykeep/releases/latest).
2. **Drag it to Applications.** Open the `.dmg` and drag Skykeep into the **Applications** folder.
3. **Open it.** The first time, macOS asks you to confirm. See below how to allow it.

### First launch: macOS will block the app. That's normal.

Skykeep is free and doesn't go through the App Store, so macOS asks you to confirm where it came
from once. After that it opens normally, for good.

| Step | What to do |
|:---:|---|
| **1** | Open Skykeep. The **"“Skykeep” Not Opened"** alert appears. Click **Done** (or **OK**). Never *Move to Trash*. |
| **2** | From the Apple menu (top-left corner), open **System Settings…** |
| **3** | Go to **Privacy & Security** and scroll down to **Security**. It will say *"“Skykeep” was blocked to protect your Mac"*. Click **Open Anyway**. |
| **4** | Confirm by clicking **Open Anyway** again and enter your password or use Touch ID. |
| **5** | Done! From now on, just open it from Applications or Spotlight. |

> [!TIP]
> The witchmymac website has an [animated visual guide](https://witchmymac.com/#liberar) showing each of these screens.

<details>
<summary><b>Prefer the Terminal?</b></summary>

After dragging Skykeep into Applications, remove the quarantine flag:

```sh
xattr -dr com.apple.quarantine /Applications/Skykeep.app
```

</details>

> [!NOTE]
> **Why does this happen?** Apple charges US$ 99 per year to "notarize" apps. Since Skykeep is
> free, macOS asks you to confirm its origin once.

### Requirements

| App | Minimum macOS | Processor |
|---|---|---|
| Skykeep for Mac | 26 Tahoe | Apple Silicon and Intel |

## First steps

| App | The first time |
|---|---|
| **Calendar, Reminders, Contacts, Photos** | Click **Allow access**. macOS asks once. |
| **Notes** | Sign in to iCloud with your Apple Account and the six-digit code your devices show. Asked once; the session stays in your Mac's keychain. |
| **Mail** | At [account.apple.com](https://account.apple.com) › Sign-In and Security › **App-Specific Passwords**, create one called "Skykeep" and paste it with your iCloud address. |
| **Passwords in other browsers** | Passwords › **Navegadores** › **Conectar ao App Senhas**, then restart the browser. Install Apple's "iCloud Passwords" extension if it isn't there. |
| **Pictures inside notes** | Optional: turn on **Full Disk Access** for Skykeep in System Settings › Privacy & Security. |

## FAQ

<details>
<summary><b>Is my Apple Account password safe?</b></summary>

It never leaves for anywhere but Apple. Sign-in uses Apple's own SRP protocol with two-factor, the
same way icloud.com signs in, so the password itself is never sent. The session is kept in your
Mac's keychain. There are no Skykeep servers or accounts.
</details>

<details>
<summary><b>Why does Mail ask for an app-specific password?</b></summary>

It's how Apple lets mail apps other than Mail reach iCloud Mail (IMAP and SMTP). It only works for
mail, and you can revoke it any time at account.apple.com.
</details>

<details>
<summary><b>Will it open Notes, Mail or other apps on its own?</b></summary>

No. Notes and Mail are read and written online in iCloud, maps and file previews open inside
Skykeep. Another app only opens when you ask for it: calling a contact, joining a video call or
clicking a link.
</details>

<details>
<summary><b>Can I tick checklists and edit my notes?</b></summary>

Yes, as in Notes: tick a checklist item, type in place (it saves by itself a moment later), set
titles, lists or checklists from the format menu, create, move and delete notes. Notes locked
with a password stay encrypted by Apple and open only in Notes.
</details>

<details>
<summary><b>Can it show my iCloud Keychain passwords?</b></summary>

Apple seals iCloud Keychain to its own apps. Skykeep shows the Mac's keychain, generates strong
passwords, and connects your other browsers to Apple's Passwords app so they all share the same
passwords.
</details>

<details>
<summary><b>Is there a Windows or Linux version?</b></summary>

Yes: Skykeep itself, by Yashwanth Kamireddi, for Windows and Linux, in the
[main README](../README.md#install). Skykeep for Mac uses the same Rust code for iCloud.
</details>

<details>
<summary><b>I found a bug or have an idea.</b></summary>

Open an [issue](https://github.com/matthfaria96/Skykeep/issues). Every suggestion is welcome.
</details>

## Building

Requires Xcode 26 or later and Rust (`rustup`). The Intel half of the iCloud helper needs
`rustup target add x86_64-apple-darwin`.

```sh
./build.sh                   # build/Skykeep.app (universal, ad hoc signature)
./build.sh --install         # and copies it to /Applications
./build.sh --dmg             # and build/Skykeep-<version>.dmg
```

```
macos/
  Sources/Skykeep/   SwiftUI app: EventKit, Contacts, PhotoKit, iCloud Drive, Keychain,
                     LocalAuthentication, MapKit, Quick Look
  bridge/            skykeep-cloud, the iCloud helper inside the app, in Rust, built from this
                     repository's quince-auth, quince-core and quince-icloud crates:
                     Notes over CloudKit, Mail over IMAP and SMTP
  Scripts/           the icon (make-icon.swift, make-svg.py), for macOS, Windows and Linux
```

Every push builds the app and its `.dmg` on GitHub Actions ([macos.yml](../.github/workflows/macos.yml)).

## Credits and license

Skykeep for Mac is made by [Matheus Faria](https://github.com/matthfaria96), in a fork of
[Skykeep](https://github.com/YashwanthKamireddi/Skykeep) by Yashwanth Kamireddi, whose iCloud code
it builds on. Free software under the [GNU General Public License v3.0](../LICENSE).
Copyright (C) 2026 Matheus Faria; Skykeep copyright (C) 2026 Yashwanth Kamireddi. Not affiliated
with or endorsed by Apple Inc.
