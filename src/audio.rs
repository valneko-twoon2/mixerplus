use crate::presets::{AppSetting, Preset};
use std::{
    cell::{OnceCell, Ref, RefCell},
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
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
    core::{GUID, HRESULT, IUnknown_Vtbl, Interface, PCWSTR, PWSTR, Result, implement},
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

/// An active endpoint. Each interface is activated on first use and then kept, since
/// activation is a round trip to audiosrv and most endpoints are never shown.
struct Endpoint {
    id: String,
    name: String,
    form: u32,
    device: IMMDevice,
    volume: OnceCell<Option<IAudioEndpointVolume>>,
    meter: OnceCell<Option<IAudioMeterInformation>>,
    sessions: OnceCell<Option<IAudioSessionManager2>>,
}

impl Endpoint {
    fn activate<T: Interface>(&self, cell: &OnceCell<Option<T>>) -> Option<T> {
        cell.get_or_init(|| unsafe { self.device.Activate(CLSCTX_ALL, None).ok() })
            .clone()
    }

    fn volume(&self) -> Option<IAudioEndpointVolume> {
        self.activate(&self.volume)
    }

    fn meter(&self) -> Option<IAudioMeterInformation> {
        self.activate(&self.meter)
    }

    fn sessions(&self) -> Option<IAudioSessionManager2> {
        self.activate(&self.sessions)
    }
}

/// Active endpoints and defaults of both flows. Asking audiosrv for the default
/// endpoint or the endpoint list takes about a millisecond each, so they are read
/// again only after Windows reports a change.
struct Endpoints {
    /// The change count the lists were read at.
    generation: u32,
    render: (String, Vec<Endpoint>),
    capture: (String, Vec<Endpoint>),
}

impl Endpoints {
    fn flow(&self, flow: EDataFlow) -> &(String, Vec<Endpoint>) {
        if flow == eCapture {
            &self.capture
        } else {
            &self.render
        }
    }
}

/// Counts endpoint changes reported by Windows. It runs on a Windows thread, so it
/// only bumps the counter.
#[implement(IMMNotificationClient)]
struct Changes(Arc<AtomicU32>);

impl IMMNotificationClient_Impl for Changes_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnPropertyValueChanged(&self, _: &PCWSTR, key: &PROPERTYKEY) -> Result<()> {
        // Drivers report other properties often; only the shown ones matter.
        let same = |other: &PROPERTYKEY| other.fmtid == key.fmtid && other.pid == key.pid;
        if same(&PKEY_Device_FriendlyName) || same(&PKEY_AudioEndpoint_FormFactor) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

pub struct Audio {
    enumerator: IMMDeviceEnumerator,
    // Display names are resolved once per executable; polling runs every second.
    names: RefCell<HashMap<String, String>>,
    changes: Arc<AtomicU32>,
    client: IMMNotificationClient,
    endpoints: RefCell<Option<Endpoints>>,
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}

impl Audio {
    pub fn new() -> Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let changes = Arc::new(AtomicU32::new(0));
        let client: IMMNotificationClient = Changes(changes.clone()).into();
        // Without notifications, the endpoints are read afresh every time.
        if unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }.is_err() {
            changes.store(u32::MAX, Ordering::Relaxed);
        }
        Ok(Self {
            enumerator,
            names: RefCell::default(),
            changes,
            client,
            endpoints: RefCell::new(None),
        })
    }

    /// Reads endpoints afresh on next use, for example when the popup opens.
    pub fn forget(&self) {
        if let Ok(mut endpoints) = self.endpoints.try_borrow_mut() {
            endpoints.take();
        }
    }

    /// The cached endpoints, read again when Windows has reported a change.
    fn endpoints(&self) -> Result<Ref<'_, Endpoints>> {
        let busy = || windows::core::Error::from_hresult(HRESULT(0x80004005u32 as i32));
        let generation = self.changes.load(Ordering::Relaxed);
        let current = self
            .endpoints
            .try_borrow()
            .map_err(|_| busy())?
            .as_ref()
            .is_some_and(|endpoints| endpoints.generation == generation && generation != u32::MAX);
        if !current {
            let endpoints = Endpoints {
                generation,
                render: self.read_endpoints(eRender)?,
                // A missing microphone is normal and must not hide the outputs.
                capture: self.read_endpoints(eCapture).unwrap_or_default(),
            };
            *self.endpoints.try_borrow_mut().map_err(|_| busy())? = Some(endpoints);
        }
        Ref::filter_map(
            self.endpoints.try_borrow().map_err(|_| busy())?,
            Option::as_ref,
        )
        .map_err(|_| busy())
    }

    /// The default endpoint's ID and every active endpoint of one flow.
    fn read_endpoints(&self, flow: EDataFlow) -> Result<(String, Vec<Endpoint>)> {
        unsafe {
            let default = self
                .enumerator
                .GetDefaultAudioEndpoint(flow, eMultimedia)
                .and_then(|d| owned_string(d.GetId()?))
                .unwrap_or_default();
            let collection = self
                .enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            let mut endpoints = Vec::new();
            for i in 0..collection.GetCount()? {
                let device = collection.Item(i)?;
                let id = owned_string(device.GetId()?)?;
                let (name, form) = properties(&device);
                endpoints.push(Endpoint {
                    name: name.unwrap_or_else(|| id.clone()),
                    form: form.unwrap_or(UnknownFormFactor.0 as u32),
                    device,
                    volume: OnceCell::new(),
                    meter: OnceCell::new(),
                    sessions: OnceCell::new(),
                    id,
                });
            }
            Ok((default, endpoints))
        }
    }

    pub fn devices(&self, flow: EDataFlow) -> Result<Vec<Device>> {
        let endpoints = self.endpoints()?;
        let (default, list) = endpoints.flow(flow);
        Ok(list
            .iter()
            .map(|endpoint| Device {
                id: endpoint.id.clone(),
                name: endpoint.name.clone(),
                selected: endpoint.id == *default,
                form: endpoint.form,
                volume: endpoint.volume().map(Volume::Endpoint),
                meter: endpoint.meter(),
            })
            .collect())
    }

    pub fn endpoint(&self, flow: EDataFlow) -> Result<Row> {
        let endpoints = self.endpoints()?;
        let (default, list) = endpoints.flow(flow);
        let endpoint = list
            .iter()
            .find(|endpoint| endpoint.id == *default)
            // ERROR_NOT_FOUND, like a missing default endpoint.
            .ok_or_else(|| windows::core::Error::from_hresult(HRESULT(0x80070490u32 as i32)))?;
        let volume = endpoint
            .volume()
            .ok_or_else(|| windows::core::Error::from_hresult(HRESULT(0x80070490u32 as i32)))?;
        Ok(Row {
            key: endpoint.id.clone(),
            name: endpoint.name.clone(),
            volume: Volume::Endpoint(volume),
            icon_path: None,
            meters: endpoint.meter().into_iter().collect(),
            process: String::new(),
        })
    }

    /// Lists applications once, combining their sessions on every active output.
    pub fn sessions(&self) -> Result<Vec<Row>> {
        let endpoints = self.endpoints()?;
        let (default, list) = endpoints.flow(eRender);
        // Sessions on the default output come first and supply the displayed level.
        let managers = list
            .iter()
            .filter(|endpoint| endpoint.id == *default)
            .chain(list.iter().filter(|endpoint| endpoint.id != *default))
            .filter_map(Endpoint::sessions);
        let mut groups: BTreeMap<String, Row> = BTreeMap::new();
        for manager in managers {
            // A disconnected device must not hide sessions on other devices.
            let _ = unsafe { self.collect_sessions(&manager, &mut groups) };
        }
        let mut rows: Vec<_> = groups.into_values().collect();
        // System sounds lead the list; applications follow alphabetically.
        rows.sort_by_key(|row| (row.key != "system", row.name.to_lowercase()));
        Ok(rows)
    }

    unsafe fn collect_sessions(
        &self,
        manager: &IAudioSessionManager2,
        groups: &mut BTreeMap<String, Row>,
    ) -> Result<()> {
        unsafe {
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
        // The change notification arrives later; the refresh after a switch reads now.
        self.forget();
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

/// The friendly name and `EndpointFormFactor` of a device, from one property store.
fn properties(device: &IMMDevice) -> (Option<String>, Option<u32>) {
    unsafe {
        let Ok(properties) = device.OpenPropertyStore(STGM_READ) else {
            return (None, None);
        };
        let name = properties
            .GetValue(&PKEY_Device_FriendlyName)
            .ok()
            .and_then(|mut value| {
                let text = PropVariantToStringAlloc(&value).and_then(|text| owned_string(text));
                let _ = PropVariantClear(&mut value);
                text.ok()
            });
        let form = properties
            .GetValue(&PKEY_AudioEndpoint_FormFactor)
            .ok()
            .and_then(|mut value| {
                let form = PropVariantToUInt32(&value);
                let _ = PropVariantClear(&mut value);
                form.ok()
            });
        (name, form)
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
