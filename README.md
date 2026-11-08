<div align="center">

<img src="packaging/icons/skykeep-1024.png" alt="Skykeep" width="104">

# Skykeep

### Your iPhone's iCloud, on Windows and Linux.

Notes, Reminders, Calendar, Contacts, iCloud Drive and Photos as real desktop apps.<br>Kept on your computer, so they open instantly and work offline. In sync with your iPhone, iPad and Mac.

<p><a href="https://apps.microsoft.com/detail/9P6B042JNK54"><img src="https://get.microsoft.com/images/en-us%20dark.svg" alt="Get it from Microsoft" height="52"></a>&nbsp;&nbsp;<a href="https://snapcraft.io/skykeep"><img src="https://snapcraft.io/static/images/badges/en/snap-store-black.svg" alt="Get it from the Snap Store" height="52"></a></p>

<sub><a href="https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-windows-x64-setup.exe">Windows installer</a> · <a href="https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-amd64.deb">.deb</a> · <a href="https://copr.fedorainfracloud.org/coprs/yashonfed/skykeep/">Fedora</a> · <a href="https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-x86_64.AppImage">AppImage</a> · <a href="#install">All options</a></sub>

<br>

<img src="docs/media/hero.webp" alt="Skykeep ticking off a reminder, switching to the week in Calendar, opening a note, then a contact card, an iCloud Drive folder and the photo library" width="880">

<p><a href="https://github.com/YashwanthKamireddi/Skykeep/releases/latest"><img src="https://img.shields.io/github/v/release/YashwanthKamireddi/Skykeep?label=release&color=2f6fd6" alt="Latest release"></a> <a href="https://github.com/YashwanthKamireddi/Skykeep/releases"><img src="https://img.shields.io/github/downloads/YashwanthKamireddi/Skykeep/total?label=downloads&color=2f6fd6" alt="Downloads"></a> <a href="https://github.com/YashwanthKamireddi/Skykeep/actions/workflows/ci.yml"><img src="https://github.com/YashwanthKamireddi/Skykeep/actions/workflows/ci.yml/badge.svg" alt="CI"></a> <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0-4a5568" alt="License: GPL-3.0"></a></p>

<sub>Free and open source · No Skykeep account · No analytics</sub>

</div>

## This fork: Skykeep for Mac, and a new look

This is a fork of [Skykeep](https://github.com/YashwanthKamireddi/Skykeep) by Yashwanth Kamireddi, maintained by **Matheus Faria** ([@matthfaria96](https://github.com/matthfaria96)), focused on usability. It adds:

- **Skykeep for Mac** ([`macos/`](macos/README.md)) — a native SwiftUI app that puts Mail, Contacts, Reminders, Calendar, Notes, Photos, iCloud Drive and Passwords in one window, with a home screen that previews all of them. Notes and Mail are read and written online in iCloud through the same Rust code this repository uses on Windows and Linux (checklists tick, notes are edited in place, mail is sent and filed), so no other app is ever opened. It also connects Chromium browsers such as Vivaldi to Apple's Passwords app.
- **A new icon** — a cloud kept by a keyhole, under a rising sun — for macOS, Windows and Linux alike.

The Windows and Linux apps are otherwise unchanged.

## Built to be trusted

Skykeep asks for your Apple Account, so it's built to deserve it.

- **Every line is public.** The whole app is open source under the GPL, so anyone can read exactly what it does with your account.
- **Your password is never sent.** Sign-in uses Apple's SRP protocol with Apple's own two-factor, the same way icloud.com signs in.
- **It talks to Apple.** Every connection is locked to its hosts in code. The only others are calendars you subscribe to yourself, and GitHub to check for Skykeep updates, which you can turn off.
- **No Skykeep servers, accounts, analytics or crash reports.** Your iCloud data goes between your computer and Apple only.
- **Your library stays on your disk,** readable only by you. Export everything to Markdown, iCalendar and vCard any time.
- **The Hidden album is locked** behind your Apple Account password.
- **Every release is built in public** by this repository's workflow, with SHA-256 checksums and signature-checked updates, and published through the Microsoft Store, the Snap Store and Chocolatey, each with its own checks before anyone can install it.

[SECURITY.md](SECURITY.md) shows the code behind each point.

## Six apps, one sign-in

<table>
  <tr>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/notes-dark.webp">
        <img src="docs/media/notes-light.webp" alt="Skykeep Notes showing folders, pinned notes and a trip note with a tag, a link, a picture, a checklist and a table">
      </picture>
      <b>Notes</b>
    </td>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/reminders-dark.webp">
        <img src="docs/media/reminders-light.webp" alt="Skykeep Reminders showing a list with sections, subtasks, flags, tags, a link and a place, beside coloured lists and smart lists">
      </picture>
      <b>Reminders</b>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/calendar-dark.webp">
        <img src="docs/media/calendar-light.webp" alt="Skykeep Calendar in week view with events from four calendars, an all-day trip, a birthday and reminders">
      </picture>
      <b>Calendar</b>
    </td>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/contacts-dark.webp">
        <img src="docs/media/contacts-light.webp" alt="Skykeep Contacts showing contacts with photos and a full card with phones, emails, related names, an address and a birthday">
      </picture>
      <b>Contacts</b>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/drive-dark.webp">
        <img src="docs/media/drive-light.webp" alt="Skykeep Drive showing a folder of pictures and documents with thumbnails in grid view">
      </picture>
      <b>iCloud Drive</b>
    </td>
    <td width="50%" align="center">
      <picture>
        <source media="(prefers-color-scheme: dark)" srcset="docs/media/photos-dark.webp">
        <img src="docs/media/photos-light.webp" alt="Skykeep Photos showing the library by month with favourites, videos and albums">
      </picture>
      <b>Photos</b>
    </td>
  </tr>
</table>

- **Instant.** Your library lives on your computer, so every app opens right away and your edits show the moment you make them.
- **Live.** A note or reminder from your iPhone, iPad or Mac usually arrives in about a second, over Apple's push channel.
- **Offline.** Keep writing with no Wi-Fi. Your changes sync when you're back online.
- **At home on your desktop.** Each app has its own launcher or Start-menu entry, with alerts, a tray icon and quick capture. On Linux, iCloud Drive and Photos are real folders in your file manager.

| | Linux | Windows |
|---|:-:|:-:|
| Notes, Reminders, Calendar, Contacts, iCloud Drive and Photos | ✅ | ✅ |
| Opens instantly and works offline, in sync with your iPhone | ✅ | ✅ |
| Reminder alerts with Complete and Snooze | ✅ | ✅ |
| Its own launcher or Start-menu entry for each app | ✅ | ✅ |
| Your family's Shared Library, shared albums, and a password-locked Hidden album | ✅ | ✅ |
| Export everything to Markdown, iCalendar and vCard | ✅ | ✅ |
| Keeps itself up to date | ✅ Snap, COPR, AUR, AppImage | ✅ |
| Apple Silicon Macs running Linux (64-bit ARM) | ✅ | |
| iCloud Drive and Photos as folders in your file manager | ✅ | Coming |
| Results in system search | ✅ GNOME, KDE | |

<details>
<summary><b>Everything each app does</b></summary>

- **Notes:** headings, styles, lists and checklists; pictures, files and tables; #tags, scans, PDFs and drawings in place; folders, pinned notes, search and Recently Deleted. Edits made here and on your phone at the same moment are merged, not overwritten.
- **Reminders:** lists with colours, icons and sections; Today, Scheduled, Flagged and smart lists; subtasks, due dates, repeats, flags, priorities, tags and early alerts. Type "Call Alex tomorrow at 5" and it's set. Desktop alerts with Complete and Snooze.
- **Calendar:** Day, Week, Month and Year; drag to create, move and resize; repeating events, alerts, time zones, birthdays and holidays; a Join button for video calls; your reminders on the schedule; `webcal://` subscriptions and `.ics` files.
- **Contacts:** nearly every field with Apple's labels, contact photos, lists, My Card, finding and merging duplicates; vCard and Google or Outlook CSV import, vCard export.
- **iCloud Drive:** list or grid with thumbnails; upload files and folders, rename, move, duplicate, Space to preview; download whole folders, which carry on where they stopped if interrupted.
- **Photos:** your library by month, albums, shared albums, Favourites and Videos; your family's Shared Library beside your own, with Both, Personal and Shared to choose between, as on iPhone; Live Photos and videos play; slideshow, upload, copy and original-quality download.
- **Everywhere:** search across every app, light and dark, results in GNOME and KDE search, and a jump list and taskbar button for each app on Windows.
</details>

## Install

| Your system | Get Skykeep |
|---|---|
| **Windows 10 and 11** | [Microsoft Store](https://apps.microsoft.com/detail/9P6B042JNK54) · [installer](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-windows-x64-setup.exe) · [Chocolatey](https://community.chocolatey.org/packages/skykeep): `choco install skykeep` · [Scoop](https://github.com/YashwanthKamireddi/scoop-bucket) |
| **Ubuntu, Debian, Mint** | [Snap Store](https://snapcraft.io/skykeep) · [`.deb`](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-amd64.deb) · [`.deb` for ARM](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-arm64.deb) |
| **Fedora**, including Fedora Asahi Remix | [COPR](https://copr.fedorainfracloud.org/coprs/yashonfed/skykeep/): `sudo dnf copr enable yashonfed/skykeep && sudo dnf install skykeep` |
| **openSUSE** | [`.rpm`](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-x86_64.rpm) · [`.rpm` for ARM](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-aarch64.rpm) |
| **Arch, CachyOS, Manjaro** | AUR: [`skykeep`](https://aur.archlinux.org/packages/skykeep) or [`skykeep-bin`](https://aur.archlinux.org/packages/skykeep-bin) · [more](#arch-linux) |
| **Any other Linux** | [AppImage](https://github.com/YashwanthKamireddi/Skykeep/releases/latest/download/Skykeep-linux-x86_64.AppImage) |

Then open **Skykeep**, sign in with your Apple Account, and approve the sign-in on your iPhone. Each app fills in as your library arrives. Every file, with checksums, is on the [latest release](https://github.com/YashwanthKamireddi/Skykeep/releases/latest).

<details>
<summary><b>Commands for each Linux file</b></summary>

```sh
sudo apt install ./Skykeep-linux-amd64.deb                          # Ubuntu 24.04+, Debian 13, Mint 22
sudo apt install ./Skykeep-linux-arm64.deb                          # the same, on 64-bit ARM
sudo dnf install ./Skykeep-linux-x86_64.rpm                         # Fedora 41+ (Skykeep-linux-aarch64.rpm on ARM)
sudo zypper install --allow-unsigned-rpm ./Skykeep-linux-x86_64.rpm # openSUSE Tumbleweed
chmod +x Skykeep-linux-x86_64.AppImage && ./Skykeep-linux-x86_64.AppImage
sudo snap install skykeep && sudo snap connect skykeep:password-manager-service
```

Run them in your Downloads folder (right-click an empty spot, **Open in Terminal**). The Snap's second command lets it keep you signed in through your keyring.
</details>

<details>
<summary><b>About the "Windows protected your PC" message</b></summary>

The [Microsoft Store](https://apps.microsoft.com/detail/9P6B042JNK54) copy is checked by Microsoft and installs straight away. The installer on this page is new to SmartScreen, so Windows asks you to confirm once: **More info**, then **Run anyway**.
</details>

<details>
<summary><b>iPhone photos (HEIC) on Fedora or openSUSE</b></summary>

Fedora and openSUSE leave out the decoder for Apple's photo format, for patent reasons. Everything else works without it. To see iPhone photos and videos:

```sh
# Fedora, with RPM Fusion
sudo dnf install https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm
sudo dnf swap ffmpeg-free ffmpeg --allowerasing && sudo dnf install libheif-freeworld

# openSUSE Tumbleweed, with Packman
sudo zypper ar -cfp 90 https://ftp.gwdg.de/pub/linux/misc/packman/suse/openSUSE_Tumbleweed/ packman
sudo zypper refresh && sudo zypper install --allow-vendor-change --from packman libavcodec62 libheif1 libde265-0 libheif-HEIF
```
</details>

<details>
<summary><b>Opening the AppImage on Ubuntu</b></summary>

Start it with `./Skykeep-linux-x86_64.AppImage --appimage-extract-and-run`, or install the library it needs once: `sudo apt install libfuse2t64` (`libfuse2` on Ubuntu 22.04).
</details>

<a id="arch-linux"></a>
<details>
<summary><b>Arch, CachyOS, EndeavourOS, Manjaro</b></summary>

From the AUR, built from source or as the released binary:

```sh
yay -S skykeep        # or: yay -S skykeep-bin
```

Or from this repository's own PKGBUILD:

```sh
git clone https://github.com/YashwanthKamireddi/Skykeep.git
cd Skykeep/packaging/arch && makepkg -si
```

For iCloud Drive and Photos as folders, also `sudo pacman -S fuse3`.
</details>

<details>
<summary><b>Updates and system requirements</b></summary>

The Windows installer and the AppImage update themselves: a notice offers **Update and Restart**, and a new version installs only after its signature check, never while a change is still on its way to iCloud. The Microsoft Store, Chocolatey, Scoop, Snap, COPR and the AUR keep their copies current. For the `.deb` or `.rpm`, install a new version over the old one; your library and sign-in stay.

Skykeep needs Windows 10 or 11, or a Linux desktop with WebKitGTK 4.1 and a keyring (GNOME Keyring or KWallet), on Wayland or X11, with glibc 2.39 or newer: Ubuntu 24.04, Mint 22, Debian 13, Fedora 41, openSUSE Tumbleweed or later, on 64-bit x86 or ARM (Apple Silicon Macs with Asahi Linux included). The AppImage is x86 only and runs on Ubuntu 22.04 too.
</details>

## FAQ

<details>
<summary><b>Is Skykeep made by Apple?</b></summary>

No. It's an independent, open-source app that works with iCloud. Apple, iCloud and iPhone are trademarks of Apple Inc.
</details>

<details>
<summary><b>Why does it need my Apple Account password?</b></summary>

Notes, Reminders, Drive and Photos open only to a full sign-in with two-factor, the same one icloud.com uses. Skykeep proves your password to Apple with SRP rather than sending it, and keeps it in your system keyring so it can sign back in by itself. Like any keyring entry, programs running as you can read it, and signing out erases it.
</details>

<details>
<summary><b>I use Advanced Data Protection. Will it work?</b></summary>

Yes, with **Access iCloud Data on the Web** turned on (iPhone: Settings → your name → iCloud). Calendar and Contacts open right away. For Notes, Reminders, Photos and iCloud Drive, approve Skykeep on your iPhone, iPad or Mac when it asks, as you would for icloud.com.
</details>

<details>
<summary><b>How is this different from iCloud for Windows?</b></summary>

Skykeep gives you Notes, Reminders, Calendar, Contacts, iCloud Drive and Photos as six apps of their own, each with its own Start-menu entry, taskbar button and offline library. And it runs on Linux too.
</details>

<details>
<summary><b>Is my library encrypted on disk?</b></summary>

Your library is in your own data folder, readable only by you. Turn on your system's disk encryption (BitLocker on Windows, LUKS on Linux) to protect it if your computer is lost, as you would for any app.
</details>

<details>
<summary><b>How do I send a diagnostics log?</b></summary>

Quit Skykeep, then start it with `SKYKEEP_LOG=1 skykeep` (Windows PowerShell: `$env:SKYKEEP_LOG=1`, then `skykeep.exe`). It writes `diagnostics.log` in its data folder with the addresses it asked and Apple's answer codes, never passwords, tokens, your name or anything from your library. Attach it to an [issue](https://github.com/YashwanthKamireddi/Skykeep/issues).
</details>

<details>
<summary><b>How does it work?</b></summary>

A Rust engine with a Tauri 2 window. Every screen reads a local SQLite library, never the network, which is why it's instant and works offline. Edits go into an outbox that shows them at once, sends them in order, waits when you're offline, and is safe to send twice.

- **Sign-in** is Apple's SRP and two-factor, as on icloud.com.
- **Notes and Reminders** are CloudKit records. A note's text sits in `ENCRYPTED_BYTES` fields that turn out to be gzip around a protobuf, and edits merge through Apple's own CRDT, so a note typed on both sides at once comes out whole.
- **Calendar, Contacts and Drive** use Apple's web services, **Photos** CloudKit, and changes arrive over Apple's push channel.
- **On Linux**, a FUSE mount puts iCloud Drive and Photos in your file manager.

Over 1,000 Rust tests hold it together. The full tour is in [ARCHITECTURE.md](ARCHITECTURE.md).
</details>

## Roadmap

- iCloud Drive and Photos as folders on Windows
- Replying to calendar invitations
- Shared notes and shared reminder lists
- Downloading a whole photo album or library to a folder
- Skykeep on Flathub and winget

Something you miss? [Open an issue](https://github.com/YashwanthKamireddi/Skykeep/issues). I read every one.

## Contributing

Skykeep is made by [Yashwanth](https://github.com/YashwanthKamireddi), who uses an iPhone with a Linux desktop every day. Skykeep for Mac and the new icon are by [Matheus Faria](https://github.com/matthfaria96), in this fork. Bug reports, packaging for your distro, design ideas and code are all welcome. [CONTRIBUTING.md](.github/CONTRIBUTING.md) has the setup and a map of the code; run `./check.sh` before a pull request.

```sh
git clone https://github.com/YashwanthKamireddi/Skykeep.git
cd Skykeep && npm ci && ./run.sh
```

Found a security problem? Please report it privately, as [SECURITY.md](SECURITY.md) explains.

If Skykeep saves you a trip to the browser, a ⭐ helps other iPhone, iPad and Mac owners find it.

## License

Copyright (C) 2026 Yashwanth Kamireddi; Skykeep for Mac, the new icon and other changes in this fork copyright (C) 2026 Matheus Faria. Released under [GPL-3.0](LICENSE): if you share a copy or a changed version, keep this notice and the same licence. Skykeep is an independent project, not affiliated with or endorsed by Apple Inc. iCloud is a trademark of Apple Inc.
