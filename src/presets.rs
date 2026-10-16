//! Volume presets in `%AppData%\MixerPlus\presets.json`.
//!
//! The file layout matches the earlier .NET version of MixerPlus, so its presets keep
//! working; the input-device fields are optional additions. Presets apply to
//! whichever output is the default, so the file's `DeviceId` is only carried along
//! for that compatibility.

use std::{
    io,
    path::{Path, PathBuf},
};

use crate::json::Json;

const MAX_PRESETS: usize = 200;
const MAX_APPS: usize = 128;
const MAX_NAME_CHARS: usize = 64;
const MAX_FILE_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct AppSetting {
    /// Executable name without extension, compared case-insensitively.
    pub process: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preset {
    pub name: String,
    /// The output device the preset was made on; kept for the old version, unused here.
    pub device_id: String,
    pub master_volume: Option<f32>,
    pub master_muted: Option<bool>,
    pub input_device_id: Option<String>,
    pub input_volume: Option<f32>,
    pub input_muted: Option<bool>,
    pub favorite: bool,
    pub apps: Vec<AppSetting>,
}

/// Trims a user-entered name and drops control characters; None when nothing is left.
pub fn sanitize_name(raw: &str) -> Option<String> {
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn level(value: f64) -> f32 {
    (value.clamp(0.0, 1.0) * 10_000.0).round() as f32 / 10_000.0
}

impl Preset {
    fn from_json(json: &Json) -> Option<Preset> {
        let name = sanitize_name(json.get("Name")?.as_str()?)?;
        let device_id = json.get("DeviceId")?.as_str()?.to_owned();
        let number = |key: &str| json.get(key).and_then(Json::as_f64).map(level);
        let flag = |key: &str| json.get(key).and_then(Json::as_bool);
        let apps = json
            .get("Settings")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .take(MAX_APPS)
            .filter_map(|item| {
                Some(AppSetting {
                    process: item.get("ProcessName")?.as_str()?.to_owned(),
                    volume: level(item.get("Volume")?.as_f64()?),
                    muted: item.get("IsMuted").and_then(Json::as_bool).unwrap_or(false),
                })
            })
            .collect();
        Some(Preset {
            name,
            device_id,
            master_volume: number("MasterVolume"),
            master_muted: flag("IsMasterMuted"),
            input_device_id: json
                .get("InputDeviceId")
                .and_then(Json::as_str)
                .map(str::to_owned),
            input_volume: number("InputVolume"),
            input_muted: flag("InputMuted"),
            favorite: flag("IsFavorite").unwrap_or(false),
            apps,
        })
    }

    fn to_json(&self) -> Json {
        let text = |value: &str| Json::String(value.to_owned());
        // Four decimals keep the file tidy (0.94, not 0.9399999976158142).
        let number = |value: f32| Json::Number((f64::from(value) * 10_000.0).round() / 10_000.0);
        let mut members = vec![
            ("Name".to_owned(), text(&self.name)),
            ("DeviceId".to_owned(), text(&self.device_id)),
            (
                "MasterVolume".to_owned(),
                self.master_volume.map_or(Json::Null, number),
            ),
            (
                "IsMasterMuted".to_owned(),
                self.master_muted.map_or(Json::Null, Json::Bool),
            ),
            ("IsFavorite".to_owned(), Json::Bool(self.favorite)),
        ];
        if let Some(id) = &self.input_device_id {
            members.push(("InputDeviceId".to_owned(), text(id)));
            members.push((
                "InputVolume".to_owned(),
                self.input_volume.map_or(Json::Null, number),
            ));
            members.push((
                "InputMuted".to_owned(),
                self.input_muted.map_or(Json::Null, Json::Bool),
            ));
        }
        members.push((
            "Settings".to_owned(),
            Json::Array(
                self.apps
                    .iter()
                    .map(|app| {
                        Json::Object(vec![
                            ("ProcessName".to_owned(), text(&app.process)),
                            ("Volume".to_owned(), number(app.volume)),
                            ("IsMuted".to_owned(), Json::Bool(app.muted)),
                        ])
                    })
                    .collect(),
            ),
        ));
        Json::Object(members)
    }
}

pub struct PresetStore {
    path: PathBuf,
    presets: Vec<Preset>,
    /// Changes whenever the stored presets do, so the popup can tell when to rebuild.
    pub revision: u64,
}

impl PresetStore {
    pub fn default_path() -> Option<PathBuf> {
        use windows::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{FOLDERID_RoamingAppData, KF_FLAG_CREATE, SHGetKnownFolderPath},
        };
        unsafe {
            let folder =
                SHGetKnownFolderPath(&FOLDERID_RoamingAppData, KF_FLAG_CREATE, None).ok()?;
            let path = folder.to_string().ok();
            CoTaskMemFree(Some(folder.0.cast()));
            Some(PathBuf::from(path?).join("MixerPlus").join("presets.json"))
        }
    }

    /// Loads the file; a missing file is empty, and an unreadable one is set aside as
    /// `.bad` so the next save cannot destroy the user's data.
    pub fn open(path: PathBuf) -> Self {
        let presets = match Self::read(&path) {
            Ok(presets) => presets,
            Err(error) => {
                if error.kind() != io::ErrorKind::NotFound {
                    let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                }
                Vec::new()
            }
        };
        Self {
            path,
            presets,
            revision: 0,
        }
    }

    fn read(path: &Path) -> io::Result<Vec<Preset>> {
        if std::fs::metadata(path)?.len() > MAX_FILE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
        }
        let text = std::fs::read_to_string(path)?;
        let json = Json::parse(&text)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid JSON"))?;
        let items = json
            .as_array()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "expected an array"))?;
        let mut presets: Vec<Preset> = Vec::new();
        for preset in items.iter().filter_map(Preset::from_json) {
            // Names are unique; the first of a duplicated name wins.
            if presets.len() < MAX_PRESETS && !presets.iter().any(|p| p.name == preset.name) {
                presets.push(preset);
            }
        }
        Ok(presets)
    }

    fn persist(&mut self) -> io::Result<()> {
        self.revision += 1;
        let json = Json::Array(self.presets.iter().map(Preset::to_json).collect());
        if let Some(folder) = self.path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        // Write beside the target and rename, so a crash never leaves half a file.
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, json.write())?;
        std::fs::rename(&temporary, &self.path)
    }

    pub fn all(&self) -> &[Preset] {
        &self.presets
    }

    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Adds a preset, or replaces the one with the same name.
    pub fn save(&mut self, mut preset: Preset) -> io::Result<()> {
        let position = self.presets.iter().position(|p| p.name == preset.name);
        match position {
            Some(index) => {
                preset.favorite = self.presets[index].favorite;
                self.presets[index] = preset;
            }
            None if self.presets.len() >= MAX_PRESETS => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "too many presets",
                ));
            }
            None => self.presets.push(preset),
        }
        self.persist()
    }

    pub fn delete(&mut self, name: &str) -> io::Result<()> {
        self.presets.retain(|p| p.name != name);
        self.persist()
    }

    /// Renames a preset; false when the new name is already taken.
    pub fn rename(&mut self, old: &str, new: &str) -> io::Result<bool> {
        if old != new && self.get(new).is_some() {
            return Ok(false);
        }
        match self.presets.iter_mut().find(|p| p.name == old) {
            Some(preset) => preset.name = new.to_owned(),
            None => return Ok(false),
        }
        self.persist()?;
        Ok(true)
    }

    /// Marks or unmarks a favorite. A favorite is only a label: it changes nothing
    /// about when or how a preset applies.
    pub fn set_favorite(&mut self, name: &str, favorite: bool) -> io::Result<()> {
        if let Some(preset) = self.presets.iter_mut().find(|p| p.name == name) {
            preset.favorite = favorite;
        }
        self.persist()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_file(label: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("mixerplus-test-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder.join("presets.json")
    }

    fn preset(name: &str, device: &str) -> Preset {
        Preset {
            name: name.into(),
            device_id: device.into(),
            master_volume: Some(0.35),
            master_muted: Some(false),
            input_device_id: Some("mic".into()),
            input_volume: Some(0.8),
            input_muted: Some(true),
            apps: vec![AppSetting {
                process: "chrome".into(),
                volume: 0.5,
                muted: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn presets_survive_a_restart() {
        let path = temporary_file("restart");
        let mut store = PresetStore::open(path.clone());
        store.save(preset("Game", "speakers")).unwrap();
        store.set_favorite("Game", true).unwrap();
        let reopened = PresetStore::open(path);
        let loaded = reopened.get("Game").unwrap();
        assert!(loaded.favorite);
        assert_eq!(loaded.apps, preset("Game", "speakers").apps);
        assert_eq!(loaded.input_volume, Some(0.8));
        let text = std::fs::read_to_string(&reopened.path).unwrap();
        assert!(text.contains("\"InputVolume\": 0.8,"), "{text}");
    }

    #[test]
    fn reads_the_file_written_by_the_dotnet_version() {
        let path = temporary_file("legacy");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"[{"Name":"Night","DeviceId":"{0.0.0}.abc","MasterVolume":0.2,"IsMasterMuted":false,
            "IsFavorite":true,"Settings":[{"ProcessName":"Spotify","Volume":0.6,"IsMuted":false}]},
            {"Name":"Night","DeviceId":"other","MasterVolume":0.9}]"#,
        )
        .unwrap();
        let store = PresetStore::open(path);
        assert_eq!(store.all().len(), 1);
        let night = store.get("Night").unwrap();
        assert!(night.favorite);
        assert_eq!(night.master_volume, Some(0.2));
        assert_eq!(night.input_device_id, None);
        assert_eq!(night.apps[0].process, "Spotify");
    }

    #[test]
    fn names_favorites_and_unreadable_files_stay_consistent() {
        let path = temporary_file("rules");
        let mut store = PresetStore::open(path.clone());
        for name in ["A", "B"] {
            store.save(preset(name, "d")).unwrap();
        }
        store.set_favorite("A", true).unwrap();
        store.set_favorite("B", true).unwrap();
        assert!(store.all().iter().all(|p| p.favorite));
        // Saving over a name keeps its label.
        store.save(preset("A", "e")).unwrap();
        assert!(store.get("A").is_some_and(|p| p.favorite));
        assert_eq!(store.all().len(), 2);
        assert!(!store.rename("A", "B").unwrap());
        assert!(store.rename("A", "C").unwrap());
        assert_eq!(sanitize_name("  \u{7}Hi\n  "), Some("Hi".into()));
        assert_eq!(sanitize_name(" \t "), None);

        std::fs::write(&path, "not json").unwrap();
        let recovered = PresetStore::open(path.clone());
        assert!(recovered.all().is_empty());
        assert!(path.with_extension("json.bad").exists());
    }
}
