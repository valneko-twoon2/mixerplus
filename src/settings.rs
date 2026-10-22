//! Popup preferences kept in `settings.json` beside the preset file.

use std::path::{Path, PathBuf};

use crate::json::Json;

const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The roaming application data folder, `%APPDATA%\mixerplus`.
pub fn data_folder() -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_CREATE, SHGetKnownFolderPath},
    };
    unsafe {
        let folder = SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_CREATE, None).ok()?;
        let path = folder.to_string().ok();
        CoTaskMemFree(Some(folder.0.cast()));
        Some(PathBuf::from(path?).join("mixerplus"))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    /// Whether the output device list is expanded under the output row.
    pub show_outputs: bool,
    /// Whether the input device list is expanded under the input row.
    pub show_inputs: bool,
    /// Whether the popup floats on top and can be dragged.
    pub pinned: bool,
    /// Where the floating popup's top-left corner was left, in screen coordinates.
    pub position: Option<(i32, i32)>,
}

impl Settings {
    pub fn default_path() -> Option<PathBuf> {
        data_folder().map(|folder| folder.join("settings.json"))
    }

    /// Reads the file; a missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> Self {
        let json = std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.len() <= MAX_FILE_BYTES)
            .and_then(|_| std::fs::read_to_string(path).ok())
            .and_then(|text| Json::parse(&text));
        let flag = |key| {
            json.as_ref()
                .and_then(|json| json.get(key))
                .and_then(Json::as_bool)
                .unwrap_or(false)
        };
        let coordinate = |key| {
            json.as_ref()
                .and_then(|json| json.get(key))
                .and_then(Json::as_f64)
                .filter(|value| value.abs() < 1.0e6)
                .map(|value| value as i32)
        };
        Self {
            show_outputs: flag("ShowOutputDevices"),
            show_inputs: flag("ShowInputDevices"),
            pinned: flag("PopupPinned"),
            position: coordinate("PopupX").zip(coordinate("PopupY")),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut entries = vec![
            ("ShowOutputDevices".into(), Json::Bool(self.show_outputs)),
            ("ShowInputDevices".into(), Json::Bool(self.show_inputs)),
            ("PopupPinned".into(), Json::Bool(self.pinned)),
        ];
        if let Some((x, y)) = self.position {
            entries.push(("PopupX".into(), Json::Number(x as f64)));
            entries.push(("PopupY".into(), Json::Number(y as f64)));
        }
        let json = Json::Object(entries);
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        // Write beside the target and rename, so a crash never leaves half a file.
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json.write())?;
        std::fs::rename(&temporary, path)
    }

    pub fn shows_devices(&self, input: bool) -> bool {
        if input {
            self.show_inputs
        } else {
            self.show_outputs
        }
    }

    pub fn toggle_devices(&mut self, input: bool) {
        if input {
            self.show_inputs = !self.show_inputs;
        } else {
            self.show_outputs = !self.show_outputs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_lists_stay_expanded_after_a_restart() {
        let folder =
            std::env::temp_dir().join(format!("mixerplus-test-{}-settings", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let path = folder.join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        let mut settings = Settings::default();
        settings.toggle_devices(true);
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path);
        assert!(loaded.show_inputs && !loaded.show_outputs);
        settings.pinned = true;
        settings.position = Some((-1500, 240));
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
