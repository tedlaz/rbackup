#![windows_subsystem = "windows"]

slint::include_modules!();

use slint::winit_030::{WinitWindowAccessor, winit};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};
use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
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
            let mut pending = 0;
            for src in &sources {
                sync_dir(src, &mirror_path(&dest, src), true, &mut pending, &mut Vec::new());
            }
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

/// Recursively copies `src` into `dst`, skipping unchanged files.
/// With `dry`, nothing is written: `count` gets the number of files that would be copied.
fn sync_dir(src: &Path, dst: &Path, dry: bool, count: &mut u64, errors: &mut Vec<String>) {
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
            sync_dir(&from, &to, dry, count, errors);
        } else if ft.is_file() && !unchanged(&from, &to) {
            if dry {
                *count += 1;
                continue;
            }
            match copy_file(&from, &to) {
                Ok(()) => *count += 1,
                Err(e) => errors.push(format!("{}: {e}", from.display())),
            }
        }
    }
}

/// fs::copy doesn't keep mtime on every OS; set it so `unchanged` works next run.
/// Read-only files: unlock the copy to overwrite it / set its mtime, then restore permissions.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = make_writable(to); // may not exist yet
    fs::copy(from, to)?;
    let meta = fs::metadata(from)?;
    make_writable(to)?;
    fs::File::options().write(true).open(to)?.set_modified(meta.modified()?)?;
    fs::set_permissions(to, meta.permissions())
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
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let (mut copied, mut errors) = (0, Vec::new());
        for dest in &dests {
            for src in &sources {
                let msg = format!("Backing up {} → {}…", src.display(), dest.display());
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
                sync_dir(src, &mirror_path(dest, src), false, &mut copied, &mut errors);
            }
        }
        let msg = match errors.first() {
            None => format!("Done — {copied} file(s) copied to {} destination(s)", dests.len()),
            Some(e) => format!("Done — {copied} copied, {} error(s). First: {e}", errors.len()),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_running(false);
            ui.set_status(msg.into());
            check(&ui);
        });
    });
}

fn main() -> Result<(), slint::PlatformError> {
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

        let (mut pending, mut errors) = (0, Vec::new());
        sync_dir(&src, &dst, true, &mut pending, &mut errors);
        assert_eq!(pending, 3, "dry run counts missing files");
        assert!(!dst.exists(), "dry run writes nothing");

        let mut copied = 0;
        sync_dir(&src, &dst, false, &mut copied, &mut errors);
        assert_eq!((copied, errors.clone()), (3, vec![]));
        assert_eq!(fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "b");

        pending = 0;
        sync_dir(&src, &dst, true, &mut pending, &mut errors);
        assert_eq!(pending, 0, "nothing pending after backup");

        copied = 0;
        sync_dir(&src, &dst, false, &mut copied, &mut errors);
        assert_eq!(copied, 0, "second run must skip unchanged files");

        fs::write(src.join("a.txt"), "changed").unwrap();
        // Changing a read-only file: overwriting its read-only copy must work too.
        perm.set_readonly(false);
        fs::set_permissions(src.join("ro.txt"), perm.clone()).unwrap();
        fs::write(src.join("ro.txt"), "ro changed").unwrap();
        perm.set_readonly(true);
        fs::set_permissions(src.join("ro.txt"), perm.clone()).unwrap();
        sync_dir(&src, &dst, true, &mut pending, &mut errors);
        assert_eq!(pending, 2, "dry run spots changed files");
        sync_dir(&src, &dst, false, &mut copied, &mut errors);
        assert_eq!((copied, errors), (2, vec![]));
        assert!(fs::metadata(dst.join("ro.txt")).unwrap().permissions().readonly());

        for p in [src.join("ro.txt"), dst.join("ro.txt")] {
            perm.set_readonly(false);
            fs::set_permissions(p, perm.clone()).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
