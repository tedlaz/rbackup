#![windows_subsystem = "windows"]

slint::include_modules!();

use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};
use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").unwrap_or_else(|| ".".into());
    PathBuf::from(base).join("rbackup").join("config.txt")
}

/// Lines starting with "> " are destinations, other lines are sources.
/// Old format (no "> " lines): first line is the single destination.
fn load_config() -> (Vec<String>, Vec<SharedString>) {
    let text = fs::read_to_string(config_path()).unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    let mut dests: Vec<String> = lines.iter().filter_map(|l| l.strip_prefix("> ")).map(String::from).collect();
    if dests.is_empty() && !lines.is_empty() {
        dests.push(lines.remove(0).to_string());
    }
    let sources = lines.into_iter().filter(|l| !l.starts_with("> ")).map(Into::into).collect();
    (dests, sources)
}

fn save_config(ui: &AppWindow) {
    let (dests, sources) = (ui.get_dests(), ui.get_sources());
    let lines = dests.iter().map(|d| format!("> {}", d.path)).chain(sources.iter().map(|s| s.to_string()));
    let text = lines.collect::<Vec<_>>().join("\n");
    let path = config_path();
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(path, text);
}

fn update_total(ui: &AppWindow) {
    let dests = ui.get_dests();
    ui.set_any_connected(dests.iter().any(|d| d.connected));
    ui.set_pending_total(dests.iter().filter(|d| d.connected).map(|d| d.pending.max(0)).sum());
}

/// Updates each destination's connected flag. Returns true if a drive was just plugged in.
fn refresh_connected(ui: &AppWindow) -> bool {
    let dests = ui.get_dests();
    let mut plugged = false;
    for (i, mut d) in dests.iter().enumerate() {
        let now = Path::new(d.path.as_str()).is_dir();
        if now != d.connected {
            plugged |= now;
            d.connected = now;
            d.pending = -1;
            dests.set_row_data(i, d);
        }
    }
    update_total(ui);
    plugged
}

/// Counts new/changed files for every connected destination in the background.
// ponytail: overlapping checks aren't cancelled; the last one to finish wins, which is fine for counts.
fn check(ui: &AppWindow) {
    if ui.get_running() {
        return; // the backup re-checks when it finishes
    }
    let sources = source_paths(ui);
    let dests: Vec<PathBuf> = ui.get_dests().iter().filter(|d| d.connected).map(|d| PathBuf::from(d.path.as_str())).collect();
    if dests.is_empty() {
        return;
    }
    ui.set_checking(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        for dest in dests {
            let mut pending = Tally::default();
            for src in &sources {
                sync_dir(src, &mirror_path(&dest, src), true, &mut pending, &mut Vec::new(), &mut |_, _, _| {});
            }
            let pending = pending.files;
            let path: SharedString = dest.display().to_string().into();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let dests = ui.get_dests();
                if let Some(i) = dests.iter().position(|d| d.path == path) {
                    let mut d = dests.row_data(i).unwrap();
                    d.pending = pending as i32;
                    dests.set_row_data(i, d);
                }
                update_total(&ui);
            });
        }
        let _ = weak.upgrade_in_event_loop(|ui| ui.set_checking(false));
    });
}

fn source_paths(ui: &AppWindow) -> Vec<PathBuf> {
    ui.get_sources().iter().map(|s| PathBuf::from(s.as_str())).collect()
}

/// Same size and mtime within 2s (FAT32/exFAT drives store coarse timestamps).
fn unchanged(src: &Path, dst: &Path) -> bool {
    let (Ok(a), Ok(b)) = (fs::metadata(src), fs::metadata(dst)) else { return false };
    let (Ok(ta), Ok(tb)) = (a.modified(), b.modified()) else { return false };
    let diff = ta.duration_since(tb).or_else(|_| tb.duration_since(ta)).unwrap_or_default();
    a.len() == b.len() && diff <= Duration::from_secs(2)
}

#[derive(Default, Debug, PartialEq)]
struct Tally {
    files: u64,
    bytes: u64,
}

/// Recursively copies `src` into `dst`, skipping unchanged files.
/// With `dry`, nothing is written: `tally` gets the files/bytes that would be copied.
/// `progress(file, bytes_written, file_done)` fires per 1 MB chunk and once per finished file.
fn sync_dir(
    src: &Path,
    dst: &Path,
    dry: bool,
    tally: &mut Tally,
    errors: &mut Vec<String>,
    progress: &mut dyn FnMut(&Path, u64, bool),
) {
    if !dry && let Err(e) = fs::create_dir_all(dst) {
        errors.push(format!("{}: {e}", dst.display()));
        return;
    }
    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => return errors.push(format!("{}: {e}", src.display())),
    };
    for entry in entries.flatten() {
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            sync_dir(&from, &to, dry, tally, errors, progress);
        } else if ft.is_file() && !unchanged(&from, &to) {
            if dry {
                tally.files += 1;
                // On Windows the size comes from the directory listing: no extra disk access.
                tally.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                continue;
            }
            match copy_file(&from, &to, &mut |n| progress(&from, n, false)) {
                Ok(n) => {
                    tally.files += 1;
                    tally.bytes += n;
                }
                Err(e) => errors.push(format!("{}: {e}", from.display())),
            }
            progress(&from, 0, true);
        }
    }
}

/// Chunked copy so progress moves inside big files. Returns bytes copied.
/// Restores mtime (so `unchanged` works next run) and permissions (read-only stays read-only).
fn copy_file(from: &Path, to: &Path, on_chunk: &mut dyn FnMut(u64)) -> std::io::Result<u64> {
    use std::io::{Read, Write};
    // Remove the old copy first: Windows refuses to overwrite read-only or hidden files in place.
    let _ = make_writable(to);
    let _ = fs::remove_file(to); // may not exist yet
    let (mut src, mut dst) = (fs::File::open(from)?, fs::File::create(to)?);
    let mut buf = vec![0; 1 << 20];
    let mut total = 0;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        total += n as u64;
        on_chunk(n as u64);
    }
    let meta = src.metadata()?;
    dst.set_modified(meta.modified()?)?;
    drop(dst);
    fs::set_permissions(to, meta.permissions())?;
    Ok(total)
}

fn fmt_bytes(b: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", units[i]) }
}

fn fmt_eta(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s left"),
        60..3600 => format!("{} min left", secs.div_ceil(60)),
        _ => format!("{}h {}m left", secs / 3600, secs % 3600 / 60),
    }
}

#[allow(clippy::permissions_set_readonly_false)] // source permissions are restored right after
fn make_writable(path: &Path) -> std::io::Result<()> {
    let mut perm = fs::metadata(path)?.permissions();
    if perm.readonly() {
        perm.set_readonly(false);
        fs::set_permissions(path, perm)?;
    }
    Ok(())
}

/// Mirrors the full source path under `dest`: `C:\Users\ted` → `dest\c\Users\ted`,
/// `\\nas\share\x` → `dest\nas\share\x`.
fn mirror_path(dest: &Path, src: &Path) -> PathBuf {
    use std::path::{Component, Prefix};
    let mut out = dest.to_path_buf();
    for c in src.components() {
        match c {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                    out.push((d as char).to_ascii_lowercase().to_string())
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    out.push(server);
                    out.push(share);
                }
                _ => {}
            },
            Component::Normal(x) => out.push(x),
            _ => {}
        }
    }
    out
}

fn run_backup(ui: &AppWindow) {
    let sources = source_paths(ui);
    let dests: Vec<PathBuf> = ui.get_dests().iter().filter(|d| d.connected).map(|d| PathBuf::from(d.path.as_str())).collect();
    ui.set_running(true);
    ui.set_progress(-1.0);
    ui.set_progress_text("Counting files…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        // Phase 1: count files and bytes to copy so the bar has a total.
        let mut total = Tally::default();
        for dest in &dests {
            for src in &sources {
                sync_dir(src, &mirror_path(dest, src), true, &mut total, &mut Vec::new(), &mut |_, _, _| {});
            }
        }

        // Phase 2: copy, reporting progress at most every 50 ms so tiny files don't flood the UI.
        let (mut copied, mut errors, mut done) = (Tally::default(), Vec::new(), Tally::default());
        let started = Instant::now();
        let mut last = started - Duration::from_secs(1);
        for dest in &dests {
            for src in &sources {
                sync_dir(src, &mirror_path(dest, src), false, &mut copied, &mut errors, &mut |file, n, file_done| {
                    done.bytes += n;
                    done.files += file_done as u64;
                    if last.elapsed() < Duration::from_millis(50) {
                        return;
                    }
                    last = Instant::now();
                    // Only empty files to copy: fall back to counting files.
                    let frac = if total.bytes > 0 {
                        done.bytes as f32 / total.bytes as f32
                    } else {
                        done.files as f32 / total.files.max(1) as f32
                    }
                    .min(1.0);
                    let text = format!(
                        "{} / {} · {}%",
                        fmt_bytes(done.bytes),
                        fmt_bytes(total.bytes),
                        (frac * 100.0) as u32
                    );
                    let secs = started.elapsed().as_secs_f64();
                    let speed = done.bytes as f64 / secs.max(0.001);
                    let mut status = format!(
                        "{} ({}/{})",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        done.files + 1,
                        total.files
                    );
                    if secs >= 1.0 && speed > 0.0 {
                        let eta = (total.bytes.saturating_sub(done.bytes) as f64 / speed) as u64;
                        status += &format!(" · {}/s · {}", fmt_bytes(speed as u64), fmt_eta(eta));
                    }
                    status += &format!(" → {}", dest.display());
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        ui.set_progress(frac);
                        ui.set_progress_text(text.into());
                        ui.set_status(status.into());
                    });
                });
            }
        }

        let msg = match errors.first() {
            None if total.files == 0 => "Done — everything was already backed up".to_string(),
            None => format!(
                "Done — {} file(s), {} copied to {} destination(s)",
                copied.files,
                fmt_bytes(copied.bytes),
                dests.len()
            ),
            Some(e) => format!("Done — {} copied, {} error(s). First: {e}", copied.files, errors.len()),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_running(false);
            ui.set_status(msg.into());
            check(&ui);
        });
    });
}

/// Exclusive lock on a file for the app's lifetime; the OS drops it on exit or crash.
/// Err = another instance holds it. Ok(None) = couldn't create the lock file, run anyway.
fn instance_lock() -> Result<Option<fs::File>, ()> {
    let path = config_path().with_file_name("instance.lock");
    let _ = fs::create_dir_all(path.parent().unwrap());
    let Ok(file) = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Err(()),
        Err(_) => Ok(None),
    }
}

/// Bring the already running window to the front (restoring it if minimized).
#[cfg(windows)]
fn focus_existing() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    // winit's window class + our title, so an Explorer window of a folder named "rbackup" doesn't match.
    let (class, title) = (wide("Window Class"), wide("rbackup"));
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
        if hwnd != 0 {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(windows))]
fn focus_existing() {}

/// Windows 11 doesn't round frameless windows by itself; ask DWM to. No-op on Windows 10.
#[cfg(windows)]
fn round_corners(w: &winit::window::Window) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(h) = w.window_handle()
        && let RawWindowHandle::Win32(h) = h.as_raw()
    {
        let pref = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(h.hwnd.get(), DWMWA_WINDOW_CORNER_PREFERENCE as u32, &pref as *const _ as _, 4);
        }
    }
}

#[cfg(not(windows))]
fn round_corners(_: &winit::window::Window) {}

fn main() -> Result<(), slint::PlatformError> {
    let Ok(_lock) = instance_lock() else {
        focus_existing();
        return Ok(());
    };
    let ui = AppWindow::new()?;
    let (dests, sources) = load_config();
    let sources = Rc::new(VecModel::from(sources));
    let dests = Rc::new(VecModel::from(
        dests.into_iter().map(|p| Dest { path: p.into(), connected: false, pending: -1 }).collect::<Vec<_>>(),
    ));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_sources(sources.clone().into());
    ui.set_dests(dests.clone().into());
    refresh_connected(&ui);
    check(&ui);

    ui.on_add_source({
        let (weak, sources) = (ui.as_weak(), sources.clone());
        move || {
            if weak.unwrap().get_running() {
                return; // lists stay fixed while a backup runs
            }
            if let Some(dir) = rfd::FileDialog::new().set_title("Add source folder").pick_folder() {
                let dir: SharedString = dir.display().to_string().into();
                if !sources.iter().any(|s| s == dir) {
                    sources.push(dir);
                    let ui = weak.unwrap();
                    save_config(&ui);
                    check(&ui);
                }
            }
        }
    });

    ui.on_remove_source({
        let (weak, sources) = (ui.as_weak(), sources.clone());
        move |i| {
            if weak.unwrap().get_running() {
                return; // lists stay fixed while a backup runs
            }
            if (i as usize) < sources.row_count() {
                sources.remove(i as usize);
                let ui = weak.unwrap();
                save_config(&ui);
                check(&ui);
            }
        }
    });

    ui.on_add_dest({
        let (weak, dests) = (ui.as_weak(), dests.clone());
        move || {
            if weak.unwrap().get_running() {
                return; // lists stay fixed while a backup runs
            }
            if let Some(dir) = rfd::FileDialog::new().set_title("Add backup destination").pick_folder() {
                let path: SharedString = dir.display().to_string().into();
                if !dests.iter().any(|d| d.path == path) {
                    dests.push(Dest { path, connected: false, pending: -1 });
                    let ui = weak.unwrap();
                    save_config(&ui);
                    refresh_connected(&ui);
                    check(&ui);
                }
            }
        }
    });

    ui.on_remove_dest({
        let (weak, dests) = (ui.as_weak(), dests.clone());
        move |i| {
            if weak.unwrap().get_running() {
                return; // lists stay fixed while a backup runs
            }
            if (i as usize) < dests.row_count() {
                dests.remove(i as usize);
                let ui = weak.unwrap();
                save_config(&ui);
                update_total(&ui);
            }
        }
    });

    ui.on_backup({
        let weak = ui.as_weak();
        move || run_backup(&weak.unwrap())
    });

    // Custom title bar: native drag/resize via winit keeps Windows snap layouts working.
    ui.on_drag({
        let weak = ui.as_weak();
        move || {
            weak.unwrap().window().with_winit_window(|w| w.drag_window().ok());
        }
    });
    ui.on_resize({
        let weak = ui.as_weak();
        move |dir| {
            use winit::window::ResizeDirection::*;
            let dir = [North, South, West, East, NorthWest, NorthEast, SouthWest, SouthEast][dir as usize];
            weak.unwrap().window().with_winit_window(|w| w.drag_resize_window(dir).ok());
        }
    });
    ui.on_close_window(|| {
        let _ = slint::quit_event_loop();
    });

    // Re-check when the user comes back to the window, e.g. after deleting or editing files in Explorer.
    // Also centers the window on its monitor and rounds its corners at the first event:
    // the winit window doesn't exist before that.
    ui.window().on_winit_window_event({
        let weak = ui.as_weak();
        let mut centered = false;
        move |window, event| {
            if !centered {
                centered = true;
                window.with_winit_window(|w| {
                    round_corners(w);
                    if let Some(m) = w.current_monitor() {
                        let (ms, ws, mp) = (m.size(), w.outer_size(), m.position());
                        let x = mp.x + (ms.width as i32 - ws.width as i32) / 2;
                        let y = mp.y + (ms.height as i32 - ws.height as i32) / 2;
                        w.set_outer_position(winit::dpi::PhysicalPosition::new(x.max(mp.x), y.max(mp.y)));
                    }
                });
            }
            if let winit::event::WindowEvent::Focused(true) = event
                && let Some(ui) = weak.upgrade()
                && !ui.get_checking()
            {
                check(&ui);
            }
            EventResult::Propagate
        }
    });

    // Poll drives; re-check for changes when one gets plugged in.
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(2), {
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade()
                && refresh_connected(&ui)
            {
                check(&ui);
            }
        }
    });

    ui.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn mirror_paths() {
        let d = Path::new(r"F:\b");
        assert_eq!(mirror_path(d, Path::new(r"C:\Users\ted\Documents")), Path::new(r"F:\b\c\Users\ted\Documents"));
        assert_eq!(mirror_path(d, Path::new(r"E:\")), Path::new(r"F:\b\e"));
        assert_eq!(mirror_path(d, Path::new(r"\\nas\share\x")), Path::new(r"F:\b\nas\share\x"));
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(fmt_eta(20), "20 s left");
        assert_eq!(fmt_eta(61), "2 min left");
        assert_eq!(fmt_eta(3900), "1h 5m left");
    }

    #[test]
    fn incremental_copy() {
        let root = std::env::temp_dir().join(format!("rbackup-test-{}", std::process::id()));
        let (src, dst) = (root.join("src"), root.join("dst"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("sub/b.txt"), "b").unwrap();
        fs::write(src.join("ro.txt"), "ro").unwrap();
        let mut perm = fs::metadata(src.join("ro.txt")).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(src.join("ro.txt"), perm.clone()).unwrap();

        let noop = &mut |_: &Path, _, _| {};
        let tally = |files, bytes| Tally { files, bytes };
        let mut errors = Vec::new();

        let mut pending = Tally::default();
        sync_dir(&src, &dst, true, &mut pending, &mut errors, noop);
        assert_eq!(pending, tally(3, 4), "dry run counts missing files and bytes");
        assert!(!dst.exists(), "dry run writes nothing");

        let (mut copied, mut seen) = (Tally::default(), Tally::default());
        sync_dir(&src, &dst, false, &mut copied, &mut errors, &mut |_, n, done| {
            seen.bytes += n;
            seen.files += done as u64;
        });
        assert_eq!((copied, errors.clone()), (tally(3, 4), vec![]));
        assert_eq!(seen, tally(3, 4), "progress reports every byte and every finished file");
        assert_eq!(fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "b");

        pending = Tally::default();
        sync_dir(&src, &dst, true, &mut pending, &mut errors, noop);
        assert_eq!(pending, tally(0, 0), "nothing pending after backup");

        copied = Tally::default();
        sync_dir(&src, &dst, false, &mut copied, &mut errors, noop);
        assert_eq!(copied, tally(0, 0), "second run must skip unchanged files");

        fs::write(src.join("a.txt"), "changed").unwrap();
        // Hidden backup copy: must still be overwritable.
        #[cfg(windows)]
        std::process::Command::new("attrib").arg("+h").arg(dst.join("a.txt")).status().unwrap();
        // Changing a read-only file: overwriting its read-only copy must work too.
        perm.set_readonly(false);
        fs::set_permissions(src.join("ro.txt"), perm.clone()).unwrap();
        fs::write(src.join("ro.txt"), "ro changed").unwrap();
        perm.set_readonly(true);
        fs::set_permissions(src.join("ro.txt"), perm.clone()).unwrap();
        sync_dir(&src, &dst, true, &mut pending, &mut errors, noop);
        assert_eq!(pending, tally(2, 17), "dry run spots changed files");
        sync_dir(&src, &dst, false, &mut copied, &mut errors, noop);
        assert_eq!((copied, errors), (tally(2, 17), vec![]));
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "changed");
        assert!(fs::metadata(dst.join("ro.txt")).unwrap().permissions().readonly());

        for p in [src.join("ro.txt"), dst.join("ro.txt")] {
            perm.set_readonly(false);
            fs::set_permissions(p, perm.clone()).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
