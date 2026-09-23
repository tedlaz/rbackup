# rbackup

A small, fast desktop backup tool for Windows. Choose the folders you want to protect and one or more folders on external drives. rbackup copies only files that are new or have changed.

Written in Rust with [Slint](https://slint.dev), using its CPU renderer, so it needs no OpenGL or GPU drivers. The app is a single `.exe` of about 8 MB with no installer and no runtime to install.

## Features

- **Several source folders and several destinations.** One click backs up every source to every connected destination.
- **Drive detection.** Each destination has a green or red dot, and the **Back up now** button turns green when a drive is plugged in. The app checks every 2 seconds.
- **Change check on startup.** When the app opens, and whenever a drive is plugged in, it counts new and changed files and shows the result in a banner, for example "12 file(s) new or changed since the last backup".
- **Incremental copies.** A file is copied only if its size or last-modified time differs from the backup copy. Times within 2 seconds count as equal, because FAT32 and exFAT drives store times coarsely.
- **Original paths kept.** `C:\Users\ted\Documents` is saved as `<destination>\c\Users\ted\Documents`, and network shares such as `\\nas\photos` as `<destination>\nas\photos`.
- **Read-only files** are copied correctly and stay read-only in the backup.
- **Nothing is ever deleted** from a backup. Files you remove from a source stay in the backup.
- **Progress bar.** While a backup runs, the **Back up now** button becomes a progress bar. It shows bytes copied, speed, time left and the current file.
- **Custom title bar.** It has animated minimize and close buttons and still supports native Windows dragging, snapping and resizing.
- **Native window feel.** The window opens centered on its monitor at any resolution or scaling. On Windows 11 it has the system's rounded corners.
- **8 themes.** Four are dark (Midnight, the default, plus Nord, Dracula and Forest) and four are light (Paper, Solarized, Rose, Sky). Pick one in the About window. It applies instantly and is remembered.
- **One-click updates.** When a newer release is out, an **Update to vX** pill appears in the title bar. See [Updates](#updates).
- **Single instance.** Starting the app again brings the open window to the front instead of opening a second copy.

## Usage

1. **Source folders:** click **+ Add** and choose the folders you want to back up.
2. **Destinations:** click **+ Add** and choose a folder on an external drive. You can add as many as you like.
3. Plug in a drive. Its dot turns green and so does **Back up now**.
4. Click **Back up now**.

Click the version label (**vX.Y.Z ⓘ**) in the title bar to open the built-in help and About window. The **THEME** section there switches between the 8 color themes.

Settings are saved in `%APPDATA%\rbackup\`:

- `config.txt`: source folders and destinations
- `theme.txt`: the chosen theme. If this file is missing or invalid, the app uses Midnight.

## Building

Requires Rust 1.89 or newer (edition 2024) and the MSVC toolchain on Windows.

```
cargo run --release     # build and run
cargo test              # run the tests
```

The release build is set up for small size (`opt-level = "z"`, LTO, `panic = "abort"`, stripped). The output is `target/release/rbackup.exe`.

The app icon is `ui/icon.ico`, and `build.rs` embeds it in the `.exe`. The window and taskbar logo is `ui/logo.png`.

The in-app logos `ui/logo-56.png` (title bar) and `ui/logo-72.png` (About window) are pre-shrunk to twice the size they're shown at. Slint's software renderer produces jagged edges when it shrinks a large image. If you change the logo, regenerate them from the high-resolution `logo.png` with a good resampling filter, such as bicubic.

Themes live in the `Theme` global at the top of `ui/app.slint`. Each theme is one `Palette` entry in its `all` list. To add a theme, append an entry and one more swatch to the About window.

## Releases

`.github/workflows/release.yml` builds and publishes a release whenever a version tag is pushed:

1. Bump `version` in `Cargo.toml`. The app reads its version from this file.
2. Commit, then tag and push:
   ```
   git tag v0.1.0
   git push origin v0.1.0
   ```
3. GitHub Actions runs the tests, builds the release version on Windows, and creates a GitHub release with `rbackup.zip` (containing `rbackup.exe`) attached. The name is the same for every release, so `https://github.com/<owner>/<repo>/releases/latest/download/rbackup.zip` always downloads the newest version. The bare `rbackup.exe` is attached as well, for the in-app updater.

## Updates

When the app starts, it checks the latest GitHub release. If that release is newer, an **Update to vX** pill appears in the title bar. Clicking it downloads the new `rbackup.exe` next to the current one, swaps the two, and restarts the app. The previous version is deleted on the next start.

- The app uses Windows' built-in `curl.exe`, so it needs Windows 10 1803 or newer. If the check fails, for example when offline, no pill appears.
- The download is trusted because it comes over HTTPS from this repository's releases. The exe is not code-signed.
- If the exe is in a folder the user can't write to, such as `Program Files`, the update fails. The status line then links to the releases page instead.

## Project layout

```
src/main.rs                    app logic: config, drive detection, change check, copying, updates
ui/app.slint                   the whole user interface and the color themes
ui/logo.png, ui/icon.ico       logo and application icon
ui/logo-56.png, ui/logo-72.png pre-shrunk in-app logos
build.rs                       compiles the UI and embeds the icon
.github/workflows/release.yml  release build
```

## Author

Designed and developed by **Ted Lazaros**. © 2026 Ted Lazaros.
