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
- **Custom title bar.** It has animated minimize and close buttons and still supports native Windows dragging, snapping and resizing.

## Usage

1. **Source folders:** click **+ Add** and choose the folders you want to back up.
2. **Destinations:** click **+ Add** and choose a folder on an external drive. You can add as many as you like.
3. Plug in a drive. Its dot turns green and so does **Back up now**.
4. Click **Back up now**.

Click the version label (**v0.1.0 ⓘ**) in the title bar to open the built-in help and About window.

Settings are saved in `%APPDATA%\rbackup\config.txt`.

## Building

Requires Rust 1.89 or newer (edition 2024) and the MSVC toolchain on Windows.

```
cargo run --release     # build and run
cargo test              # run the tests
```

The release build is set up for small size (`opt-level = "z"`, LTO, `panic = "abort"`, stripped). The output is `target/release/rbackup.exe`.

The app icon is `ui/icon.ico`, and `build.rs` embeds it in the `.exe`. The title-bar and window logo is `ui/logo.png`.

## Releases

`.github/workflows/release.yml` builds and publishes a release whenever a version tag is pushed:

1. Bump `version` in `Cargo.toml`. The app reads its version from this file.
2. Commit, then tag and push:
   ```
   git tag v0.1.0
   git push origin v0.1.0
   ```
3. GitHub Actions runs the tests, builds the release version on Windows, and creates a GitHub release with `rbackup.zip` (containing `rbackup.exe`) attached. The name is the same for every release, so `https://github.com/<owner>/<repo>/releases/latest/download/rbackup.zip` always downloads the newest version.

## Project layout

```
src/main.rs                    app logic: config, drive detection, change check, copying
ui/app.slint                   the whole user interface
ui/logo.png, ui/icon.ico       logo and application icon
build.rs                       compiles the UI and embeds the icon
.github/workflows/release.yml  release build
```

## Author

Designed and developed by **Ted Lazaros**. © 2026 Ted Lazaros.
