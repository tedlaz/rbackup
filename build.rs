fn main() {
    slint_build::compile("ui/app.slint").unwrap();
    // Explorer / taskbar icon for rbackup.exe.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        winresource::WindowsResource::new().set_icon("ui/icon.ico").compile().unwrap();
    }
}
