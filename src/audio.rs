use crate::presets::{AppSetting, Preset};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
};
use windows::{
    Win32::UI::Shell::{IShellItem2, SHCreateItemFromParsingName, SHLoadIndirectString},
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{CloseHandle, PROPERTYKEY},
        Media::Audio::{
            Endpoints::{IAudioEndpointVolume, IAudioMeterInformation},
            *,
        },
        System::{
            Com::{
                CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ,
                StructuredStorage::{
                    PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
                },
            },
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
    },
    core::{GUID, HRESULT, IUnknown_Vtbl, Interface, PCWSTR, PWSTR, Result},
};

#[derive(Clone)]
pub enum Volume {
    Endpoint(IAudioEndpointVolume),
    Sessions(Vec<ISimpleAudioVolume>),
}

impl Volume {
    pub fn read(&self) -> Result<(i32, bool)> {
        unsafe {
            let (level, mute) = match self {
                Self::Endpoint(v) => (v.GetMasterVolumeLevelScalar()?, v.GetMute()?.as_bool()),
                Self::Sessions(v) => {
                    // The first session represents the row; a row is never built empty.
                    let first = v.first().ok_or_else(|| {
                        windows::core::Error::from_hresult(HRESULT(0x80004005u32 as i32))
                    })?;
                    (first.GetMasterVolume()?, first.GetMute()?.as_bool())
                }
            };
            Ok(((level * 100.0).round() as i32, mute))
        }
    }

    pub fn set_level(&self, level: i32) -> Result<()> {
        let level = level.clamp(0, 100) as f32 / 100.0;
        unsafe {
            match self {
                Self::Endpoint(v) => v.SetMasterVolumeLevelScalar(level, std::ptr::null()),
                Self::Sessions(volumes) => {
                    for v in volumes {
                        v.SetMasterVolume(level, std::ptr::null())?;
                    }
                    Ok(())
                }
            }
        }
    }

    pub fn toggle_mute(&self) -> Result<()> {
        self.set_mute(!self.read()?.1)
    }

    pub fn set_mute(&self, mute: bool) -> Result<()> {
        unsafe {
            match self {
                Self::Endpoint(v) => v.SetMute(mute, std::ptr::null()),
                Self::Sessions(volumes) => {
                    for v in volumes {
                        v.SetMute(mute, std::ptr::null())?;
                    }
                    Ok(())
                }
            }
        }
    }
}

pub struct Device {
    pub id: String,
    pub name: String,
    pub selected: bool,
    /// `EndpointFormFactor` value, used to pick the device glyph.
    pub form: u32,
    pub volume: Option<Volume>,
    pub meter: Option<IAudioMeterInformation>,
}

pub struct Row {
    pub key: String,
    pub name: String,
    pub volume: Volume,
    pub icon_path: Option<String>,
    /// Peak meters of the endpoint or of each session in the row.
    pub meters: Vec<IAudioMeterInformation>,
    /// Executable name without extension, which presets use to find the application.
    /// Empty when it is unknown.
    pub process: String,
}

impl Row {
    /// The current output peak between 0 and 1, or 0 when it is unavailable.
    pub fn peak(&self) -> f32 {
        self.meters
            .iter()
            .filter_map(|meter| unsafe { meter.GetPeakValue().ok() })
            .fold(0.0, f32::max)
            .clamp(0.0, 1.0)
    }
}

pub struct Audio {
    enumerator: IMMDeviceEnumerator,
    // Display names are resolved once per executable; polling runs every second.
    names: RefCell<HashMap<String, String>>,
}

impl Audio {
    pub fn new() -> Result<Self> {
        Ok(Self {
            enumerator: unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? },
            names: RefCell::default(),
        })
    }

    pub fn devices(&self, flow: EDataFlow) -> Result<Vec<Device>> {
        unsafe {
            let default = self
                .enumerator
                .GetDefaultAudioEndpoint(flow, eMultimedia)
                .and_then(|d| owned_string(d.GetId()?))
                .unwrap_or_default();
            let collection = self
                .enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            let mut devices = Vec::new();
            for i in 0..collection.GetCount()? {
                let device = collection.Item(i)?;
                let id = owned_string(device.GetId()?)?;
                devices.push(Device {
                    selected: id == default,
                    name: device_name(&device).unwrap_or_else(|_| id.clone()),
                    form: form_factor(&device).unwrap_or(UnknownFormFactor.0 as u32),
                    volume: device.Activate(CLSCTX_ALL, None).ok().map(Volume::Endpoint),
                    meter: device.Activate(CLSCTX_ALL, None).ok(),
                    id,
                });
            }
            Ok(devices)
        }
    }

    pub fn endpoint(&self, flow: EDataFlow) -> Result<Row> {
        unsafe {
            let device = self.enumerator.GetDefaultAudioEndpoint(flow, eMultimedia)?;
            Ok(Row {
                key: owned_string(device.GetId()?)?,
                name: device_name(&device)?,
                volume: Volume::Endpoint(device.Activate(CLSCTX_ALL, None)?),
                icon_path: None,
                meters: device.Activate(CLSCTX_ALL, None).ok().into_iter().collect(),
                process: String::new(),
            })
        }
    }

    /// Lists applications once, combining their sessions on every active output.
    pub fn sessions(&self) -> Result<Vec<Row>> {
        unsafe {
            let default = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .and_then(|d| owned_string(d.GetId()?))
                .unwrap_or_default();
            let collection = self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut devices = Vec::new();
            for i in 0..collection.GetCount()? {
                let device = collection.Item(i)?;
                let id = owned_string(device.GetId()?).unwrap_or_default();
                devices.push((id != default, device));
            }
            // Sessions on the default output come first and supply the displayed level.
            devices.sort_by_key(|(other, _)| *other);
            let mut groups: BTreeMap<String, Row> = BTreeMap::new();
            for (_, device) in &devices {
                // A disconnected device must not hide sessions on other devices.
                let _ = self.collect_sessions(device, &mut groups);
            }
            let mut rows: Vec<_> = groups.into_values().collect();
            // System sounds lead the list; applications follow alphabetically.
            rows.sort_by_key(|row| (row.key != "system", row.name.to_lowercase()));
            Ok(rows)
        }
    }

    unsafe fn collect_sessions(
        &self,
        device: &IMMDevice,
        groups: &mut BTreeMap<String, Row>,
    ) -> Result<()> {
        unsafe {
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for i in 0..sessions.GetCount()? {
                let control = sessions.GetSession(i)?;
                if control.GetState()? == AudioSessionStateExpired {
                    continue;
                }
                let control: IAudioSessionControl2 = control.cast()?;
                let pid = control.GetProcessId()?;
                let volume: ISimpleAudioVolume = control.cast()?;
                let meter: Option<IAudioMeterInformation> = control.cast().ok();
                let icon_path = if pid == 0 { None } else { process_path(pid) };
                // Group by executable so multi-process applications share one row.
                let group = if pid == 0 {
                    "system".to_owned()
                } else {
                    icon_path
                        .as_deref()
                        .map(str::to_lowercase)
                        .unwrap_or_else(|| pid.to_string())
                };
                if let Some(row) = groups.get_mut(&group) {
                    if let Volume::Sessions(volumes) = &mut row.volume {
                        volumes.push(volume);
                    }
                    row.meters.extend(meter);
                    continue;
                }
                let process = if pid == 0 {
                    "System Sounds".to_owned()
                } else {
                    icon_path
                        .as_deref()
                        .and_then(|path| path.rsplit('\\').next())
                        .map(|file| file.trim_end_matches(".exe").trim_end_matches(".EXE"))
                        .unwrap_or_default()
                        .to_owned()
                };
                let name = self
                    .names
                    .borrow_mut()
                    .entry(group.clone())
                    .or_insert_with(|| display_name(&control, pid, icon_path.as_deref()))
                    .clone();
                groups.insert(
                    group.clone(),
                    Row {
                        key: group,
                        name,
                        volume: Volume::Sessions(vec![volume]),
                        icon_path,
                        meters: meter.into_iter().collect(),
                        process,
                    },
                );
            }
            Ok(())
        }
    }

    /// A snapshot of the default output and input volumes and every application.
    pub fn capture_preset(&self, name: String) -> Result<Preset> {
        let output = self.endpoint(eRender)?;
        let (level, muted) = output.volume.read()?;
        let mut preset = Preset {
            name,
            device_id: output.key,
            master_volume: Some(level as f32 / 100.0),
            master_muted: Some(muted),
            ..Default::default()
        };
        if let Ok(input) = self.endpoint(eCapture)
            && let Ok((level, muted)) = input.volume.read()
        {
            preset.input_device_id = Some(input.key);
            preset.input_volume = Some(level as f32 / 100.0);
            preset.input_muted = Some(muted);
        }
        for row in self.sessions()? {
            if !row.process.is_empty()
                && let Ok((level, muted)) = row.volume.read()
            {
                preset.apps.push(AppSetting {
                    process: row.process,
                    volume: level as f32 / 100.0,
                    muted,
                });
            }
        }
        Ok(preset)
    }

    /// Applies a preset. Absent devices and applications are normal and skipped; any
    /// other failure is reported after everything else has been attempted.
    pub fn apply_preset(&self, preset: &Preset) -> Result<()> {
        let mut failure = None;
        let mut note = |result: Result<()>| {
            if let Err(error) = result
                && failure.is_none()
            {
                failure = Some(error);
            }
        };
        // Presets are not tied to a device: the master level goes to the current output.
        match self.endpoint(eRender) {
            Ok(output) => {
                note(self.set_endpoint(&output.key, preset.master_volume, preset.master_muted))
            }
            Err(error) => note(Err(error)),
        }
        if let Some(id) = &preset.input_device_id {
            note(self.set_endpoint(id, preset.input_volume, preset.input_muted));
        }
        match self.sessions() {
            Ok(rows) => {
                for row in rows {
                    let Some(setting) = preset
                        .apps
                        .iter()
                        .find(|app| app.process.eq_ignore_ascii_case(&row.process))
                    else {
                        continue;
                    };
                    note(
                        row.volume
                            .set_level((setting.volume * 100.0).round() as i32),
                    );
                    note(row.volume.set_mute(setting.muted));
                }
            }
            Err(error) => note(Err(error)),
        }
        failure.map_or(Ok(()), Err)
    }

    fn set_endpoint(&self, id: &str, volume: Option<f32>, muted: Option<bool>) -> Result<()> {
        if volume.is_none() && muted.is_none() {
            return Ok(());
        }
        let id: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let Ok(device) = self.enumerator.GetDevice(PCWSTR(id.as_ptr())) else {
                return Ok(());
            };
            if device.GetState()? != DEVICE_STATE_ACTIVE {
                return Ok(());
            }
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            if let Some(volume) = volume {
                endpoint.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null())?;
            }
            if let Some(muted) = muted {
                endpoint.SetMute(muted, std::ptr::null())?;
            }
        }
        Ok(())
    }

    pub fn set_default(&self, id: &str) -> Result<()> {
        let id: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let policy: IPolicyConfig = CoCreateInstance(
                &GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9),
                None,
                CLSCTX_ALL,
            )?;
            for role in [eConsole, eMultimedia, eCommunications] {
                (policy.vtable().set_default_endpoint)(
                    policy.as_raw(),
                    PCWSTR(id.as_ptr()),
                    role.0,
                )
                .ok()?;
            }
            Ok(())
        }
    }
}

/// Names applications like the Windows volume mixer: the session's own name, then
/// the executable's file description, then the executable name.
fn display_name(control: &IAudioSessionControl2, pid: u32, path: Option<&str>) -> String {
    unsafe {
        let mut name = control
            .GetDisplayName()
            .and_then(|text| owned_string(text))
            .unwrap_or_default();
        if name.starts_with('@') {
            // Resource references such as the system sounds session name.
            let source: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let mut buffer = [0u16; 512];
            name = SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut buffer, None)
                .map(|_| PCWSTR(buffer.as_ptr()).to_string().unwrap_or_default())
                .unwrap_or_default();
        }
        if name.trim().is_empty()
            && let Some(path) = path
        {
            let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            name = SHCreateItemFromParsingName::<_, _, IShellItem2>(PCWSTR(wide.as_ptr()), None)
                .and_then(|item| item.GetString(&PKEY_FILE_DESCRIPTION))
                .and_then(|text| owned_string(text))
                .unwrap_or_default();
        }
        if !name.trim().is_empty() {
            return name.trim().to_owned();
        }
        match (pid, path) {
            (0, _) => "System Sounds".into(),
            (_, Some(path)) => path
                .rsplit('\\')
                .next()
                .unwrap_or(path)
                .trim_end_matches(".exe")
                .to_owned(),
            _ => format!("Application {pid}"),
        }
    }
}

const PKEY_FILE_DESCRIPTION: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x0cef7d53_fa64_11d1_a203_0000f81fedee),
    pid: 3,
};

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        Some(path)
    }
}

fn form_factor(device: &IMMDevice) -> Result<u32> {
    unsafe {
        let properties = device.OpenPropertyStore(STGM_READ)?;
        let mut value = properties.GetValue(&PKEY_AudioEndpoint_FormFactor)?;
        let result = PropVariantToUInt32(&value);
        PropVariantClear(&mut value)?;
        result
    }
}

fn device_name(device: &IMMDevice) -> Result<String> {
    unsafe {
        let properties = device.OpenPropertyStore(STGM_READ)?;
        let mut value = properties.GetValue(&PKEY_Device_FriendlyName)?;
        let result = PropVariantToStringAlloc(&value).and_then(|text| owned_string(text));
        PropVariantClear(&mut value)?;
        result
    }
}

unsafe fn owned_string(value: PWSTR) -> Result<String> {
    unsafe {
        let result = value.to_string();
        CoTaskMemFree(Some(value.0.cast()));
        Ok(result?)
    }
}

// Undocumented Windows policy ABI, isolated from the supported Core Audio APIs.
// The ten slots before SetDefaultEndpoint must retain their original order.
windows::core::imp::define_interface!(
    IPolicyConfig,
    IPolicyConfig_Vtbl,
    0xf8679f50_850a_41cf_9c72_430f290290c8
);
#[repr(C)]
pub struct IPolicyConfig_Vtbl {
    base: IUnknown_Vtbl,
    reserved: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HRESULT,
    set_endpoint_visibility: usize,
}
