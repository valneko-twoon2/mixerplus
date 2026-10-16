#![windows_subsystem = "windows"]

mod about;
mod appearance;
mod audio;
mod config;
mod drawing;
mod json;
mod presets;
mod ui;

/// Restricts where code can be loaded from before any other work happens.
fn harden() {
    use windows::Win32::System::{
        LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories},
        SystemServices::{
            PROCESS_MITIGATION_IMAGE_LOAD_POLICY, PROCESS_MITIGATION_IMAGE_LOAD_POLICY_0,
        },
        Threading::{ProcessImageLoadPolicy, SetProcessMitigationPolicy},
    };
    unsafe {
        // Later LoadLibrary calls never search the application or working directory.
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32);
        // No images from network shares or low-integrity locations, and prefer System32.
        let policy = PROCESS_MITIGATION_IMAGE_LOAD_POLICY {
            Anonymous: PROCESS_MITIGATION_IMAGE_LOAD_POLICY_0 { Flags: 0b111 },
        };
        let _ = SetProcessMitigationPolicy(
            ProcessImageLoadPolicy,
            (&policy as *const PROCESS_MITIGATION_IMAGE_LOAD_POLICY).cast(),
            size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
        );
    }
}

fn main() {
    harden();
    // Read by the installer for the Settings > Apps entry; the executable has no
    // version resource.
    if std::env::args().any(|arg| arg == "--version") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if std::env::args().any(|arg| arg == "--check-ui") {
        if let Err(error) = ui::run() {
            eprintln!("UI check failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--check-audio") {
        match ui::check_audio() {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("Audio check failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Err(error) = ui::run() {
        ui::show_error(&format!("mixerplus could not start.\n\n{error}"));
    }
}
