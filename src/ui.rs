use std::{cell::RefCell, mem::size_of, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Media::Audio::{eCapture, eRender},
        System::{
            Com::*,
            Diagnostics::Debug::MessageBeep,
            LibraryLoader::GetModuleHandleW,
            Registry::*,
            SystemServices::{SS_CENTER, SS_NOPREFIX},
            Threading::CreateMutexW,
        },
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::{BOOL, GUID, PCWSTR, PWSTR, Result, w},
};

use crate::{
    appearance::{GLYPH, ITEM, LINK, SECONDARY, pack},
    audio::{Audio, Device, Row},
    presets::{PresetStore, sanitize_name},
};

const TRAY: u32 = WM_APP + 1;
const REFRESH_PAGE: u32 = WM_APP + 2;
const DISMISS: u32 = WM_APP + 3;
/// Refreshes without forcing a rebuild, so a new default only moves the selection.
const REFRESH_STATE: u32 = WM_APP + 4;
const MORE: usize = 12;
const PREVIOUS: usize = 13;
const NEXT: usize = 14;
const BACK: usize = 15;
const SETTINGS_LINK: usize = 16;
/// Each mixer row uses three IDs: slider, mute button, and device chevron.
const ROW_ID: usize = 100;
const DEVICE_ID: usize = 1000;
const EDIT_ID: usize = 19;
const PRESETS_BUTTON: usize = 17;
const NEW_PRESET: usize = 18;
/// Each preset row uses four IDs: apply, favorite, rename, and delete.
const PRESET_ID: usize = 3000;
const COMMIT_EDIT: u32 = WM_APP + 5;
const SHOW_ABOUT: u32 = WM_APP + 6;
const EM_SETSEL: u32 = 0xb1;
const EM_SETLIMITTEXT: u32 = 0xc5;
const ES_AUTOHSCROLL_STYLE: u32 = 0x80;
const REFRESH_TIMER: usize = 1;
const TRAY_TIMER: usize = 2;
const ANIMATION_TIMER: usize = 3;
const METER_TIMER: usize = 4;
const TRANSITION_TIMER: usize = 5;
const METER_INTERVAL_MS: u32 = 33;
const TRANSITION_MS: f32 = 220.0;
const TRAY_RETRY_MS: u32 = 2_000;
const PROMOTION_ATTEMPTS: u32 = 15;
const TBM_GETPOS: u32 = WM_USER;
/// Hidden row windows kept for reuse; more than a few pages' worth are destroyed.
const MAX_SPARE: usize = 96;

// Segoe Fluent Icons code points.
const SPEAKER: u16 = 0xe7f5;
const MUTED: u16 = 0xe74f;
const MICROPHONE: u16 = 0xe720;
const MICROPHONE_OFF: u16 = 0xec54;
const PRESET_GLYPH: u16 = 0xf4c3;
const ADD_GLYPH: u16 = 0xe710;
const STAR_GLYPH: u16 = 0xe734;
const STAR_FILLED_GLYPH: u16 = 0xe735;
const RENAME_GLYPH: u16 = 0xe70f;
const DELETE_GLYPH: u16 = 0xe74d;
const CONFIRM_GLYPH: u16 = 0xe73e;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Mixer,
    Devices { input: bool },
    Presets,
}

/// An inline name box: for a new preset, or for renaming the named one.
struct EditState {
    hwnd: HWND,
    target: Option<String>,
}

/// Window classes of popup controls.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Static,
    Button,
    Trackbar,
    Edit,
}

impl Class {
    fn name(self) -> PCWSTR {
        match self {
            Self::Static => w!("STATIC"),
            Self::Button => w!("BUTTON"),
            Self::Trackbar => TRACKBAR_CLASSW,
            Self::Edit => w!("EDIT"),
        }
    }

    /// The class of a row window that can be reused; text boxes never are.
    fn of(hwnd: HWND) -> Option<Self> {
        let mut name = [0u16; 32];
        let length = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
        let name = String::from_utf16_lossy(&name[..length]);
        [Self::Static, Self::Button, Self::Trackbar]
            .into_iter()
            .find(|class| {
                unsafe { class.name().to_string() }.is_ok_and(|n| n.eq_ignore_ascii_case(&name))
            })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Default output or input row: glyph mute button and device chevron.
    Endpoint { input: bool },
    /// Application row: icon mute button with a badge.
    App,
    /// Volume of one device in the device list.
    Device,
}

struct Control {
    row: Row,
    slider: HWND,
    /// Mute button, or the device list item for device rows.
    mute: HWND,
    kind: Kind,
    /// Volume value beside the slider, a mixerplus addition to the shell layout.
    value: HWND,
    icon: HICON,
    tip: String,
    /// Smoothed peak level shown on the slider, from 0 to 1.
    peak: f32,
    muted: bool,
}

impl Drop for Control {
    fn drop(&mut self) {
        if !self.icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Navigation {
    previous: HWND,
    next: HWND,
    label: HWND,
}

struct Fonts {
    body: HFONT,
    title: HFONT,
    small: HFONT,
    icon: HFONT,
    badge: HFONT,
}

impl Fonts {
    fn new(scale: f32) -> Self {
        let icon_face = crate::drawing::icon_face();
        let font = |size: f32, weight: i32, face: PCWSTR| unsafe {
            CreateFontW(
                -(size * scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                face,
            )
        };
        let fonts = Self {
            body: font(14.0, 400, crate::drawing::FONT),
            title: font(14.0, 600, crate::drawing::FONT),
            small: font(12.0, 400, crate::drawing::FONT),
            icon: font(16.0, 400, icon_face),
            badge: font(10.0, 400, icon_face),
        };
        crate::appearance::set_icon_fonts(fonts.icon, fonts.badge);
        fonts
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.title, self.small, self.icon, self.badge] {
            unsafe {
                let _ = DeleteObject(font.into());
            }
        }
    }
}

/// Default endpoint rows tagged `input`, then application rows.
type Snapshot = (Vec<(bool, Row)>, Vec<Row>);

/// The old view's pixels and window rectangle, captured before a view change.
struct Pending {
    capture: crate::appearance::Capture,
    top: i32,
    height: i32,
    forward: bool,
}

/// A running cross-fade between views while the window resizes to fit the new one.
struct ViewTransition {
    start: Instant,
    from_top: i32,
    from_height: i32,
    to_top: i32,
    to_height: i32,
    left: i32,
    width: i32,
    /// Controls hidden while the popup paints the transition itself.
    hidden: Vec<HWND>,
}

struct Animation {
    start: Instant,
    from: i32,
    to: i32,
    x: i32,
    closing: bool,
}

struct App {
    audio: Audio,
    hwnd: HWND,
    tooltip: HWND,
    // Declared after the controls that use them so windows are destroyed first.
    controls: Vec<Control>,
    devices: Vec<(HWND, Device)>,
    children: Vec<HWND>,
    /// Hidden row windows from earlier pages, reused before creating new ones.
    spare: Vec<(Class, HWND)>,
    /// The button whose click is being handled, which is never reused.
    clicked: HWND,
    fonts: Fonts,
    icon: HICON,
    page: usize,
    pages: usize,
    signature: Vec<String>,
    taskbar_created: u32,
    scale: f32,
    page_size: usize,
    device_rows: usize,
    device_offset: usize,
    work_area: RECT,
    monitor: RECT,
    navigation: Option<Navigation>,
    chrome_count: usize,
    tray_added: bool,
    promotion_attempts: u32,
    view: View,
    animation: Option<Animation>,
    presets: PresetStore,
    /// Names shown on the preset page, in order; button IDs index into it.
    listed: Vec<String>,
    edit: Option<EditState>,
    /// The preset whose delete button has been clicked once.
    armed_delete: Option<String>,
    list_rows: usize,
    pending: Option<Pending>,
    transition: Option<ViewTransition>,
    /// Where the popup should end up once a pending transition has played.
    target: Option<RECT>,
    /// When the popup last started closing, to make a tray click toggle it.
    dismissed: Option<Instant>,
}

fn wide(text: &str) -> Vec<u16> {
    text.replace('\0', "")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

fn set_text(hwnd: HWND, text: &str) {
    let mut current = [0u16; 1024];
    let length = unsafe { GetWindowTextW(hwnd, &mut current) } as usize;
    let text = wide(text);
    // Skip unchanged captions so the one-second refresh does not repaint controls.
    if current[..length] != text[..text.len() - 1] {
        unsafe {
            let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
        }
    }
}

/// Moves mixerplus out of the Windows 11 overflow area the first time Explorer records it.
/// Returns true once its entry exists; an existing IsPromoted value is the user's choice.
fn promote_tray_icon() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return true;
    };
    let executable = executable.to_string_lossy().to_lowercase();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\NotifyIconSettings"),
            None,
            KEY_READ | KEY_SET_VALUE,
            &mut root,
        )
        .is_err()
        {
            // Windows 10 has no per-icon promotion key.
            return true;
        }
        let mut found = false;
        let mut name = [0u16; 256];
        for index in 0.. {
            let mut length = name.len() as u32;
            if RegEnumKeyExW(
                root,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
            .is_err()
            {
                break;
            }
            let entry = PCWSTR(name.as_ptr());
            let path = registry_string(root, entry, w!("ExecutablePath"));
            if path.is_none_or(|path| expand_known_folder(&path).to_lowercase() != executable) {
                continue;
            }
            found = true;
            if registry_string(root, entry, w!("IsPromoted")).is_none() {
                let promoted = 1u32;
                let _ = RegSetKeyValueW(
                    root,
                    entry,
                    w!("IsPromoted"),
                    REG_DWORD.0,
                    Some((&promoted as *const u32).cast()),
                    size_of::<u32>() as u32,
                );
            }
        }
        let _ = RegCloseKey(root);
        found
    }
}

/// Reads a value as text; DWORD values are only checked for presence.
unsafe fn registry_string(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = size_of_val(&buffer) as u32;
    let mut kind = REG_VALUE_TYPE::default();
    unsafe {
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_SZ | RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    if kind != REG_SZ {
        return Some(String::new());
    }
    let length = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Explorer stores paths under known folders as `{FOLDERID}\relative\path`.
fn expand_known_folder(path: &str) -> String {
    let Some((folder, rest)) = path.split_once('\\') else {
        return path.to_owned();
    };
    let Ok(id) = GUID::try_from(folder.trim_start_matches('{').trim_end_matches('}')) else {
        return path.to_owned();
    };
    unsafe {
        let Ok(base) = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) else {
            return path.to_owned();
        };
        let expanded = base.to_string().map(|base| format!("{base}\\{rest}"));
        CoTaskMemFree(Some(base.0.cast()));
        expanded.unwrap_or_else(|_| path.to_owned())
    }
}

/// Places the popup at the work area corner nearest the taskbar, like the shell flyouts.
fn popup_position(work: RECT, monitor: RECT, width: i32, height: i32, margin: i32) -> (i32, i32) {
    let top = if work.top > monitor.top {
        work.top + margin
    } else {
        work.bottom - margin - height
    };
    (
        (work.right - margin - width).max(work.left),
        top.clamp(work.top, (work.bottom - height).max(work.top)),
    )
}

pub fn show_error(message: &str) {
    let message = wide(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("mixerplus"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn run() -> Result<()> {
    unsafe {
        let mutex = CreateMutexW(None, false, w!("Local\\mixerplus.Tray.Rust"))?;
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        if already_running {
            if std::env::args().any(|arg| arg == "--check-ui" || arg == "--snapshot") {
                let _ = CloseHandle(mutex);
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(0x80004005u32 as i32),
                    "Exit mixerplus before running UI diagnostics.",
                ));
            }
            if std::env::args().any(|arg| arg == "--show")
                && let Ok(hwnd) = FindWindowW(w!("mixerplusTrayPopup"), None)
            {
                let _ = PostMessageW(Some(hwnd), TRAY, WPARAM(0), LPARAM(WM_LBUTTONUP as isize));
            }
            let _ = CloseHandle(mutex);
            return Ok(());
        }
        let result = run_inner();
        let _ = CloseHandle(mutex);
        result
    }
}

pub fn check_audio() -> Result<String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| {
            let audio = Audio::new()?;
            let outputs = audio.devices(eRender)?.len();
            let inputs = audio.devices(eCapture)?.len();
            let applications = audio.sessions()?.len();
            Ok(format!(
                "Audio check passed: {outputs} outputs, {inputs} inputs, {applications} application groups."
            ))
        })();
        CoUninitialize();
        result
    }
}

unsafe fn run_inner() -> Result<()> {
    unsafe {
        // All audio COM objects remain on this UI thread until shutdown.
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let drawing = match crate::drawing::Runtime::new() {
            Ok(drawing) => drawing,
            Err(error) => {
                CoUninitialize();
                return Err(error);
            }
        };
        let result = run_initialized();
        APP.with(|state| {
            state.borrow_mut().take();
        });
        drop(drawing);
        CoUninitialize();
        result
    }
}

unsafe fn run_initialized() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES,
        })
        .ok()?;
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("mixerplusTrayPopup"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let inspect = std::env::args().any(|arg| arg == "--inspect-ui");
        let hwnd = CreateWindowExW(
            if inspect {
                // Expose the popup to desktop inspection tools during development.
                WS_EX_APPWINDOW | WS_EX_TOPMOST
            } else {
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST
            },
            class.lpszClassName,
            // Task Manager lists visible windows with a title under the process.
            if inspect { w!("mixerplus") } else { w!("") },
            if inspect {
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN
            } else {
                WS_POPUP | WS_CLIPCHILDREN
            },
            0,
            0,
            crate::config::POPUP_WIDTH,
            400,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        crate::appearance::load_theme();
        crate::appearance::style_window(hwnd);
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            None,
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(hwnd),
            None,
            Some(instance.into()),
            None,
        )?;
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let icon = tray_icon()?;
        let app = App {
            audio: Audio::new()?,
            hwnd,
            tooltip,
            fonts: Fonts::new(scale),
            icon,
            controls: Vec::new(),
            devices: Vec::new(),
            children: Vec::new(),
            spare: Vec::new(),
            clicked: HWND::default(),
            page: 0,
            pages: 1,
            signature: Vec::new(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            scale,
            page_size: crate::config::MAX_APPLICATION_ROWS,
            device_rows: 8,
            device_offset: 0,
            work_area: RECT::default(),
            monitor: RECT::default(),
            navigation: None,
            chrome_count: 0,
            tray_added: false,
            promotion_attempts: 0,
            view: View::Mixer,
            animation: None,
            presets: PresetStore::open(
                PresetStore::default_path()
                    .unwrap_or_else(|| std::env::temp_dir().join("mixerplus-presets.json")),
            ),
            listed: Vec::new(),
            edit: None,
            armed_delete: None,
            list_rows: 6,
            pending: None,
            transition: None,
            target: None,
            dismissed: None,
        };
        APP.with(|state| *state.borrow_mut() = Some(app));
        if SetTimer(
            Some(hwnd),
            REFRESH_TIMER,
            crate::config::REFRESH_INTERVAL_MS,
            None,
        ) == 0
        {
            return Err(windows::core::Error::from_thread());
        }
        APP.with(|state| {
            if let Some(app) = state.borrow_mut().as_mut() {
                app.maintain_tray();
            }
        });
        let arguments: Vec<_> = std::env::args_os().collect();
        if arguments.iter().any(|arg| arg == "--check-ui") {
            check_page_navigation(hwnd)?;
            println!("UI check passed: next/previous preserve mixer rows and navigation handles.");
            return Ok(());
        }
        if let Some(pair) = arguments.windows(2).find(|pair| pair[0] == "--snapshot") {
            APP.with(|state| -> Result<()> {
                let mut state = state.borrow_mut();
                if let Some(app) = state.as_mut() {
                    app.open()?;
                    crate::appearance::snapshot(
                        hwnd,
                        &app.children,
                        std::path::Path::new(&pair[1]),
                    )?;
                }
                Ok(())
            })?;
            return Ok(());
        }
        if std::env::args().any(|arg| arg == "--show") {
            PostMessageW(Some(hwnd), TRAY, WPARAM(0), LPARAM(WM_LBUTTONUP as isize))?;
        }
        let mut message = MSG::default();
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status == -1 {
                return Err(windows::core::Error::from_thread());
            }
            if status == 0 {
                break;
            }
            if crate::about::dispatch(&message) {
                continue;
            }
            if message.message == WM_KEYDOWN && message.wParam.0 == VK_ESCAPE_CODE {
                let _ = PostMessageW(Some(hwnd), DISMISS, WPARAM(0), LPARAM(0));
                continue;
            }
            if message.message == WM_KEYDOWN && message.wParam.0 == VK_RETURN_CODE {
                let focus = GetFocus();
                if !focus.is_invalid() && GetDlgCtrlID(focus) as usize == EDIT_ID {
                    let _ = PostMessageW(Some(hwnd), COMMIT_EDIT, WPARAM(0), LPARAM(0));
                    continue;
                }
            }
            if !IsDialogMessageW(hwnd, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

const VK_ESCAPE_CODE: usize = 0x1b;
const VK_RETURN_CODE: usize = 0x0d;

fn check_page_navigation(hwnd: HWND) -> Result<()> {
    let fail =
        |message| windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), message);
    let (before, navigation, pages) = APP.with(|state| -> Result<_> {
        let mut state = state.borrow_mut();
        let app = state.as_mut().ok_or_else(|| fail("Mixer is missing"))?;
        app.open()?;
        Ok((
            app.controls
                .iter()
                .map(|c| c.row.key.clone())
                .collect::<Vec<_>>(),
            app.navigation
                .ok_or_else(|| fail("Navigation is missing"))?,
            app.pages,
        ))
    })?;
    if pages < 2 {
        println!("Pagination check skipped: fewer than two pages.");
        return Ok(());
    }
    for (button, expected_page) in [(navigation.next, 1), (navigation.previous, 0)] {
        unsafe {
            // Exercise the native notification path, including deferred row refresh.
            SendMessageW(button, BM_CLICK, None, None);
            let mut message = MSG::default();
            while PeekMessageW(
                &mut message,
                Some(hwnd),
                REFRESH_PAGE,
                REFRESH_PAGE,
                PM_REMOVE,
            )
            .as_bool()
            {
                DispatchMessageW(&message);
            }
        }
        APP.with(|state| -> Result<()> {
            let state = state.borrow();
            let app = state.as_ref().ok_or_else(|| fail("Mixer is missing"))?;
            let current = app
                .navigation
                .ok_or_else(|| fail("Navigation was destroyed"))?;
            if app.page != expected_page
                || current.previous != navigation.previous
                || current.next != navigation.next
            {
                return Err(fail(
                    "Page navigation changed control identity or selected the wrong page",
                ));
            }
            if expected_page == 0
                && app
                    .controls
                    .iter()
                    .map(|c| c.row.key.clone())
                    .collect::<Vec<_>>()
                    != before
            {
                return Err(fail("Returning to page one changed mixer rows"));
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn endpoint_glyph(input: bool, level: i32, muted: bool) -> u16 {
    match (input, muted, level) {
        (true, true, _) => MICROPHONE_OFF,
        (true, false, _) => MICROPHONE,
        (false, true, _) => MUTED,
        (false, false, 0) => 0xe992,
        (false, false, 1..=33) => 0xe993,
        (false, false, 34..=66) => 0xe994,
        _ => 0xe995,
    }
}

/// Glyphs for `EndpointFormFactor` values, as in the quick settings device list.
fn device_glyph(form: u32, input: bool) -> u16 {
    match form {
        1 => 0xe7f5,
        2 | 7 | 8 => 0xe95f,
        3 => 0xe7f6,
        4 => MICROPHONE,
        5 | 6 => 0xe95b,
        9 => 0xe7f4,
        _ if input => MICROPHONE,
        _ => SPEAKER,
    }
}

/// mixerplus's generated tray icon, or the stock audio icon if rendering fails.
fn tray_icon() -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if let Some(icon) = crate::appearance::tray_icon(size) {
        return Ok(icon);
    }
    let mut stock = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe { SHGetStockIconInfo(SIID_AUDIOFILES, SHGSI_ICON | SHGSI_SMALLICON, &mut stock)? };
    Ok(stock.hIcon)
}

/// Reports a preset file failure like any other operation failure.
fn io_error(error: std::io::Error) -> windows::core::Error {
    windows::core::Error::new(
        windows::core::HRESULT(0x80004005u32 as i32),
        error.to_string(),
    )
}

pub(crate) fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    enabled.as_bool()
}

/// The taskbar on the popup's monitor, so the popup can slide out from behind it.
fn taskbar_for(monitor: HMONITOR) -> Option<HWND> {
    unsafe {
        for class in [w!("Shell_TrayWnd"), w!("Shell_SecondaryTrayWnd")] {
            let mut after = None;
            while let Ok(taskbar) = FindWindowExW(None, after, class, None) {
                if MonitorFromWindow(taskbar, MONITOR_DEFAULTTONULL) == monitor {
                    return Some(taskbar);
                }
                after = Some(taskbar);
            }
        }
        None
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t.powi(3)
}

impl App {
    fn px(&self, value: i32) -> i32 {
        (value as f32 * self.scale).round() as i32
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip = wide("mixerplus — 音量ミキサー");
        // Keep the terminating NUL when a longer tip is ever configured.
        let length = tip.len().min(data.szTip.len());
        data.szTip[..length].copy_from_slice(&tip[..length]);
        if let Some(last) = data.szTip.last_mut() {
            *last = 0;
        }
        data
    }

    /// Adds the tray icon and promotes it once; retries while Explorer is not ready.
    fn maintain_tray(&mut self) {
        unsafe {
            if !self.tray_added {
                // NIM_ADD can time out after succeeding, so probe with NIM_MODIFY first.
                self.tray_added = Shell_NotifyIconW(NIM_MODIFY, &self.tray_data()).as_bool()
                    || Shell_NotifyIconW(NIM_ADD, &self.tray_data()).as_bool();
            }
            if self.tray_added && self.promotion_attempts < PROMOTION_ATTEMPTS {
                self.promotion_attempts = if promote_tray_icon() {
                    PROMOTION_ATTEMPTS
                } else {
                    self.promotion_attempts + 1
                };
            }
            if self.tray_added && self.promotion_attempts >= PROMOTION_ATTEMPTS {
                let _ = KillTimer(Some(self.hwnd), TRAY_TIMER);
            } else {
                SetTimer(Some(self.hwnd), TRAY_TIMER, TRAY_RETRY_MS, None);
            }
        }
    }

    fn child(
        &mut self,
        class: Class,
        text: &str,
        id: usize,
        rect: [i32; 4],
        style: WINDOW_STYLE,
        font: HFONT,
    ) -> Result<HWND> {
        if let Some(index) = self.spare.iter().position(|(c, _)| *c == class) {
            let (_, hwnd) = self.spare.swap_remove(index);
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_ID, id as isize);
                if class == Class::Static {
                    // Labels differ only in alignment, which their paint subclass reads.
                    let current = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                    let next = current & !SS_CENTER.0 | style.0 & SS_CENTER.0;
                    SetWindowLongW(hwnd, GWL_STYLE, next as i32);
                }
                SendMessageW(
                    hwnd,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(0)),
                );
                set_text(hwnd, text);
                // The bottom of the z-order is where a new window goes, so tab order
                // stays the creation order.
                SetWindowPos(
                    hwnd,
                    Some(HWND_BOTTOM),
                    self.px(rect[0]),
                    self.px(rect[1]),
                    self.px(rect[2]),
                    self.px(rect[3]),
                    SWP_NOACTIVATE,
                )?;
                // SWP_SHOWWINDOW and WS_VISIBLE at creation take a far slower path.
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
            self.children.push(hwnd);
            return Ok(hwnd);
        }
        let text = wide(text);
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class.name(),
                PCWSTR(text.as_ptr()),
                WS_CHILD | style,
                self.px(rect[0]),
                self.px(rect[1]),
                self.px(rect[2]),
                self.px(rect[3]),
                Some(self.hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )?;
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                // Without a redraw, a static cannot paint and validate itself before
                // its custom painting subclass is installed.
                Some(LPARAM(0)),
            );
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            self.children.push(hwnd);
            Ok(hwnd)
        }
    }

    fn label(&mut self, text: &str, rect: [i32; 4], font: HFONT, style: isize) -> Result<HWND> {
        let hwnd = self.child(
            Class::Static,
            text,
            0,
            rect,
            WINDOW_STYLE(SS_NOPREFIX.0 | if style == SECONDARY { SS_CENTER.0 } else { 0 }),
            font,
        )?;
        unsafe { crate::appearance::install_label(hwnd, style)? };
        Ok(hwnd)
    }

    fn button(
        &mut self,
        text: &str,
        id: usize,
        rect: [i32; 4],
        font: HFONT,
        data: isize,
    ) -> Result<HWND> {
        let hwnd = self.child(
            Class::Button,
            text,
            id,
            rect,
            WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
            font,
        )?;
        unsafe { crate::appearance::install_button(hwnd, data)? };
        Ok(hwnd)
    }

    fn tool(&self, message: u32, tool: HWND, text: &str) {
        let mut text = wide(text);
        let info = TTTOOLINFOW {
            cbSize: size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: self.hwnd,
            uId: tool.0 as usize,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                self.tooltip,
                message,
                None,
                Some(LPARAM(&info as *const TTTOOLINFOW as isize)),
            );
        }
    }

    /// Default output and input rows, then one row per application.
    fn snapshot(&self) -> Result<Snapshot> {
        let mut endpoints = Vec::new();
        for (flow, input) in [(eRender, false), (eCapture, true)] {
            if let Ok(row) = self.audio.endpoint(flow) {
                endpoints.push((input, row));
            }
        }
        Ok((endpoints, self.audio.sessions()?))
    }

    fn build_chrome(&mut self) -> Result<()> {
        let small = self.fonts.small;
        let icon = self.fonts.icon;
        let previous = self.button(
            "\u{e76b}",
            PREVIOUS,
            [8, 0, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        let label = self.label("", [56, 0, 248, 32], small, SECONDARY)?;
        let next = self.button(
            "\u{e76c}",
            NEXT,
            [312, 0, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.button(
            "その他の音量設定",
            SETTINGS_LINK,
            [8, 0, 220, 32],
            small,
            pack(LINK, 0, false),
        )?;
        self.navigation = Some(Navigation {
            previous,
            next,
            label,
        });
        self.chrome_count = self.children.len();
        Ok(())
    }

    /// Removes the current page's rows; chrome keeps its window identities. Rows are
    /// hidden for reuse by the next page, since creating and destroying windows
    /// dominated the cost of switching views.
    fn clear_rows(&mut self) {
        let rows: Vec<HWND> = self.children[self.chrome_count.min(self.children.len())..].to_vec();
        for row in &rows {
            self.tool(TTM_DELTOOLW, *row, "");
        }
        self.children
            .truncate(self.chrome_count.min(self.children.len()));
        unsafe {
            let focus = GetFocus();
            for row in rows {
                if focus == row {
                    // As when a focused window is destroyed: no control keeps focus.
                    let _ = SetFocus(None);
                }
                // The clicked button's notification may still be unwinding, so it is
                // never reused as a different control.
                match Class::of(row) {
                    Some(class) if row != self.clicked && self.spare.len() < MAX_SPARE => {
                        let _ = ShowWindow(row, SW_HIDE);
                        SetWindowLongPtrW(row, GWLP_ID, 0);
                        crate::appearance::recycle(row);
                        self.spare.push((class, row));
                    }
                    _ => {
                        let _ = DestroyWindow(row);
                    }
                }
            }
        }
        self.clicked = HWND::default();
        self.controls.clear();
        self.devices.clear();
    }

    fn refresh(&mut self, force: bool) -> Result<()> {
        if self.navigation.is_none() {
            self.build_chrome()?;
        }
        match self.view {
            View::Mixer => {
                let (endpoints, mut apps) = self.snapshot()?;
                let signature: Vec<_> = endpoints
                    .iter()
                    .map(|(_, r)| r)
                    .chain(&apps)
                    .map(|r| format!("{}:{}", r.key, r.name))
                    .collect();
                if !force && signature == self.signature {
                    // Refresh COM handles as sessions can be replaced without changing their key.
                    let mut rows: Vec<_> = endpoints.into_iter().map(|(_, r)| r).collect();
                    rows.append(&mut apps);
                    for i in 0..self.controls.len() {
                        if let Some(index) =
                            rows.iter().position(|r| r.key == self.controls[i].row.key)
                        {
                            self.controls[i].row = rows.remove(index);
                            self.update_control(i);
                        }
                    }
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_mixer(endpoints, apps))
            }
            View::Presets => {
                let (id, name) = self
                    .audio
                    .endpoint(eRender)
                    .map(|row| (row.key, row.name))
                    .unwrap_or_default();
                let signature = vec![
                    id.clone(),
                    self.presets.revision.to_string(),
                    self.device_offset.to_string(),
                ];
                if !force && signature == self.signature {
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_presets(name))
            }
            View::Devices { input } => {
                let devices = self.audio.devices(if input { eCapture } else { eRender })?;
                let signature: Vec<_> = devices
                    .iter()
                    .map(|d| format!("{}:{}:{}", d.id, d.name, d.selected))
                    .collect();
                // A new default only moves the selection; avoid rebuilding the list.
                let same_list = signature.len() == self.signature.len()
                    && devices
                        .iter()
                        .zip(&self.devices)
                        .all(|(d, (_, old))| d.id == old.id && d.name == old.name);
                if !force && same_list {
                    for (device, (item, old)) in devices.into_iter().zip(&mut self.devices) {
                        unsafe { crate::appearance::set_flag(*item, device.selected) };
                        old.selected = device.selected;
                    }
                    for i in 0..self.controls.len() {
                        self.update_control(i);
                    }
                    self.signature = signature;
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_devices(devices, input))
            }
        }
    }

    /// Rebuilds a visible popup without showing intermediate frames.
    fn quietly(&mut self, rebuild: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        if !visible {
            return rebuild(self);
        }
        unsafe {
            SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
        self.end_transition();
        let result = rebuild(self);
        // Cover the new layout with the old view before anything is painted.
        self.begin_transition();
        unsafe {
            // Re-enabling redraw sets WS_VISIBLE, so this runs only for a visible popup.
            SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
        result
    }

    fn rebuild_mixer(&mut self, endpoints: Vec<(bool, Row)>, apps: Vec<Row>) -> Result<()> {
        // Destroying the focused row leaves no focus for keyboard navigation.
        self.clear_rows();
        let restore_focus = unsafe { GetFocus().is_invalid() };
        let (title, icon) = (self.fonts.title, self.fonts.icon);
        let mut dividers = Vec::new();
        let mut y = 8;
        for (input, row) in endpoints {
            let i = self.controls.len();
            let mute = self.button(
                "",
                ROW_ID + i * 3 + 1,
                [8, y + 4, 40, 36],
                icon,
                pack(GLYPH, 0, false),
            )?;
            let slider = self.slider(ROW_ID + i * 3, [52, y + 6, 216, 32])?;
            let value = self.label("", [268, y + 4, 40, 36], self.fonts.body, SECONDARY)?;
            self.button(
                "\u{e76c}",
                ROW_ID + i * 3 + 2,
                [308, y + 4, 40, 36],
                icon,
                pack(GLYPH, 0, false),
            )?;
            self.add_control(
                row,
                [slider, mute, value],
                Kind::Endpoint { input },
                HICON::default(),
            );
            y += 44;
        }
        if !self.controls.is_empty() {
            dividers.push(self.px(y + 4));
            y += 9;
        }
        self.label("音量ミキサー", [16, y, 240, 40], title, 0)?;
        let presets_button = self.button(
            "\u{f4c3}",
            PRESETS_BUTTON,
            [264, y + 4, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.tool(TTM_ADDTOOLW, presets_button, "プリセット");
        self.button(
            "\u{e713}",
            MORE,
            [308, y + 4, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        y += 40;
        self.pages = apps.len().div_ceil(self.page_size).max(1);
        self.page = self.page.min(self.pages - 1);
        let page: Vec<_> = apps
            .into_iter()
            .skip(self.page * self.page_size)
            .take(self.page_size)
            .collect();
        if page.is_empty() {
            self.label(
                "音声を再生しているアプリはありません",
                [16, y, 328, 40],
                self.fonts.body,
                SECONDARY,
            )?;
            y += 40;
        }
        for row in page {
            let i = self.controls.len();
            let mut icon_handle = HICON::default();
            if let Some(path) = &row.icon_path {
                let path = wide(path);
                unsafe {
                    ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&mut icon_handle), None, 1);
                }
            }
            let mute = self.button(
                "",
                ROW_ID + i * 3 + 1,
                [8, y + 4, 40, 36],
                icon,
                pack(crate::appearance::APP, SPEAKER, false),
            )?;
            if !icon_handle.is_invalid() {
                unsafe { crate::appearance::set_app_icon(mute, icon_handle) };
            }
            let slider = self.slider(ROW_ID + i * 3, [52, y + 6, 256, 32])?;
            let value = self.label("", [308, y + 4, 40, 36], self.fonts.body, SECONDARY)?;
            self.add_control(row, [slider, mute, value], Kind::App, icon_handle);
            y += 44;
        }
        let pager = self.pages > 1;
        if pager {
            y += 4;
        }
        self.place_navigation(pager.then_some(y));
        if pager {
            y += 36;
        }
        self.finish_layout(y + 8, dividers)?;
        if restore_focus
            && unsafe { GetForegroundWindow() } == self.hwnd
            && let Some(control) = self.controls.first()
        {
            unsafe {
                let _ = SetFocus(Some(control.slider));
            }
        }
        Ok(())
    }

    fn slider(&mut self, id: usize, rect: [i32; 4]) -> Result<HWND> {
        let slider = self.child(
            Class::Trackbar,
            "",
            id,
            rect,
            WS_TABSTOP | WINDOW_STYLE(TBS_NOTICKS | TBS_TRANSPARENTBKGND),
            self.fonts.body,
        )?;
        unsafe {
            SendMessageW(
                slider,
                TBM_SETRANGE,
                Some(WPARAM(0)),
                Some(LPARAM(100 << 16)),
            );
            SendMessageW(slider, TBM_SETPAGESIZE, None, Some(LPARAM(10)));
            crate::appearance::install_slider(slider)?;
        }
        Ok(slider)
    }

    fn add_control(&mut self, row: Row, [slider, mute, value]: [HWND; 3], kind: Kind, icon: HICON) {
        self.tool(TTM_ADDTOOLW, slider, &row.name);
        self.tool(TTM_ADDTOOLW, mute, &row.name);
        self.controls.push(Control {
            row,
            slider,
            mute,
            kind,
            value,
            icon,
            tip: String::new(),
            peak: 0.0,
            muted: false,
        });
        self.update_control(self.controls.len() - 1);
    }

    /// An inline name box over `rect` (logical pixels), with its rounded frame.
    fn name_box(&mut self, rect: [i32; 4], text: &str) -> Result<HWND> {
        let edit = self.child(
            Class::Edit,
            text,
            EDIT_ID,
            [rect[0] + 12, rect[1] + 8, rect[2] - 24, rect[3] - 16],
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL_STYLE),
            self.fonts.body,
        )?;
        unsafe {
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(64)), None);
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        crate::appearance::set_edit_frame(Some(RECT {
            left: self.px(rect[0]),
            top: self.px(rect[1]),
            right: self.px(rect[0] + rect[2]),
            bottom: self.px(rect[1] + rect[3]),
        }));
        Ok(edit)
    }

    fn rebuild_presets(&mut self, device_name: String) -> Result<()> {
        self.clear_rows();
        crate::appearance::set_edit_frame(None);
        let (body, title, icon) = (self.fonts.body, self.fonts.title, self.fonts.icon);
        self.button(
            "\u{e72b}",
            BACK,
            [8, 8, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.label("プリセット", [56, 0, 280, 48], body, 0)?;
        self.label(&device_name, [16, 48, 328, 32], title, 0)?;
        let presets: Vec<_> = self.presets.all().to_vec();
        self.listed = presets.iter().map(|p| p.name.clone()).collect();
        // An edit whose preset has disappeared (renamed elsewhere, device changed) ends.
        if let Some(Some(target)) = self.edit.as_ref().map(|e| e.target.as_ref())
            && !self.listed.contains(target)
        {
            self.edit = None;
        }
        let editing = self.edit.as_ref().map(|e| e.target.clone());
        let mut focus = None;
        let mut y = 84;
        if editing == Some(None) {
            focus = Some(self.name_box(
                [8, y, 344, 40],
                &format!("プリセット {}", presets.len() + 1),
            )?);
        } else {
            self.button(
                "現在の音量を保存",
                NEW_PRESET,
                [8, y, 344, 40],
                body,
                pack(ITEM, ADD_GLYPH, false),
            )?;
        }
        y += 44;
        self.device_offset = self
            .device_offset
            .min(presets.len().saturating_sub(self.list_rows));
        if presets.is_empty() {
            self.label("プリセットがありません", [16, y, 328, 40], body, SECONDARY)?;
            y += 40;
        }
        for (i, preset) in presets
            .iter()
            .enumerate()
            .skip(self.device_offset)
            .take(self.list_rows)
        {
            let base = PRESET_ID + i * 4;
            if editing.as_ref().and_then(|e| e.as_deref()) == Some(preset.name.as_str()) {
                focus = Some(self.name_box([8, y, 244, 40], &preset.name)?);
            } else {
                let apply = self.button(
                    &preset.name,
                    base,
                    [8, y, 244, 40],
                    body,
                    pack(ITEM, PRESET_GLYPH, false),
                )?;
                self.tool(TTM_ADDTOOLW, apply, &preset.name);
            }
            let armed = self.armed_delete.as_deref() == Some(preset.name.as_str());
            for (offset, x, glyph, flag, tip) in [
                (
                    1,
                    256,
                    if preset.favorite {
                        STAR_FILLED_GLYPH
                    } else {
                        STAR_GLYPH
                    },
                    preset.favorite,
                    if preset.favorite {
                        "お気に入りを解除"
                    } else {
                        "お気に入りにする"
                    },
                ),
                (2, 288, RENAME_GLYPH, false, "名前を変更"),
                (
                    3,
                    320,
                    if armed { CONFIRM_GLYPH } else { DELETE_GLYPH },
                    armed,
                    if armed {
                        "もう一度クリックで削除"
                    } else {
                        "削除"
                    },
                ),
            ] {
                let button = self.button(
                    "",
                    base + offset,
                    [x, y + 4, 32, 32],
                    icon,
                    pack(GLYPH, glyph, flag),
                )?;
                self.tool(TTM_ADDTOOLW, button, tip);
            }
            y += 42;
        }
        if let (Some(state), Some(hwnd)) = (self.edit.as_mut(), focus) {
            state.hwnd = hwnd;
        }
        self.place_navigation(None);
        self.finish_layout(y + 8, Vec::new())?;
        if let Some(hwnd) = focus
            && unsafe { GetForegroundWindow() } == self.hwnd
        {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
        }
        Ok(())
    }

    /// Saves the new preset or applies the rename typed in the name box.
    fn commit_edit(&mut self) -> Result<()> {
        let Some(edit) = self.edit.take() else {
            return Ok(());
        };
        let mut text = [0u16; 256];
        let length = unsafe { GetWindowTextW(edit.hwnd, &mut text) } as usize;
        let Some(name) = sanitize_name(&String::from_utf16_lossy(&text[..length])) else {
            self.edit = Some(edit);
            unsafe {
                let _ = MessageBeep(MB_ICONWARNING);
            }
            return Ok(());
        };
        match &edit.target {
            None => {
                let preset = self.audio.capture_preset(name)?;
                self.presets.save(preset).map_err(io_error)?;
            }
            Some(old) => {
                if !self.presets.rename(old, &name).map_err(io_error)? {
                    // The name is taken; keep the box open so it can be changed.
                    self.edit = Some(edit);
                    unsafe {
                        let _ = MessageBeep(MB_ICONWARNING);
                    }
                    return Ok(());
                }
            }
        }
        crate::appearance::set_edit_frame(None);
        self.refresh(true)
    }

    fn cancel_edit(&mut self) -> Result<()> {
        if self.edit.take().is_some() {
            crate::appearance::set_edit_frame(None);
            self.refresh(true)?;
        }
        Ok(())
    }

    fn preset_command(&mut self, id: usize) -> Result<()> {
        let Some(name) = self.listed.get((id - PRESET_ID) / 4).cloned() else {
            return Ok(());
        };
        match (id - PRESET_ID) % 4 {
            0 => {
                self.armed_delete = None;
                let applied = match self.presets.get(&name).cloned() {
                    Some(preset) => self.audio.apply_preset(&preset),
                    None => Ok(()),
                };
                // Show the result: the mixer reflects every changed level. The view
                // change posts its own refresh, so a second one would cut the
                // transition short.
                self.show_view(View::Mixer)?;
                return applied;
            }
            1 => {
                let favorite = self.presets.get(&name).is_some_and(|p| p.favorite);
                self.presets
                    .set_favorite(&name, !favorite)
                    .map_err(io_error)?;
            }
            2 => {
                self.armed_delete = None;
                self.edit = Some(EditState {
                    hwnd: HWND::default(),
                    target: Some(name),
                });
                self.post_refresh()?;
            }
            _ => {
                if self.armed_delete.as_deref() == Some(name.as_str()) {
                    self.armed_delete = None;
                    self.presets.delete(&name).map_err(io_error)?;
                } else {
                    // Two clicks delete, so a stray click cannot lose a preset.
                    self.armed_delete = Some(name);
                    self.presets.revision += 1;
                }
            }
        }
        self.post_refresh()
    }

    fn post_refresh(&self) -> Result<()> {
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }

    fn rebuild_devices(&mut self, devices: Vec<Device>, input: bool) -> Result<()> {
        self.clear_rows();
        let (body, title, icon) = (self.fonts.body, self.fonts.title, self.fonts.icon);
        self.button(
            "\u{e72b}",
            BACK,
            [8, 8, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.label(
            if input {
                "音声入力"
            } else {
                "音声出力"
            },
            [56, 0, 280, 48],
            body,
            0,
        )?;
        self.label(
            if input {
                "入力デバイス"
            } else {
                "出力デバイス"
            },
            [16, 48, 328, 32],
            title,
            0,
        )?;
        let mut y = 84;
        self.device_offset = self
            .device_offset
            .min(devices.len().saturating_sub(self.device_rows));
        if devices.is_empty() {
            self.label("デバイスがありません", [16, y, 328, 40], body, SECONDARY)?;
            y += 40;
        }
        for (i, device) in devices
            .into_iter()
            .enumerate()
            .skip(self.device_offset)
            .take(self.device_rows)
        {
            let item = self.button(
                &device.name,
                DEVICE_ID + i,
                [8, y, 344, 36],
                body,
                pack(ITEM, device_glyph(device.form, input), device.selected),
            )?;
            // Each device keeps its own volume, independent of the default device.
            if let Some(volume) = device.volume.clone() {
                let index = self.controls.len();
                let slider = self.slider(ROW_ID + index * 3, [40, y + 36, 264, 28])?;
                let value = self.label("", [304, y + 34, 44, 32], body, SECONDARY)?;
                let row = Row {
                    key: device.id.clone(),
                    name: device.name.clone(),
                    volume,
                    icon_path: None,
                    meters: device.meter.clone().into_iter().collect(),
                    process: String::new(),
                };
                self.add_control(row, [slider, item, value], Kind::Device, HICON::default());
            }
            self.devices.push((item, device));
            y += 68;
        }
        self.place_navigation(None);
        self.finish_layout(y + 8, Vec::new())
    }

    /// Shows the pager at `y`, or hides it.
    fn place_navigation(&mut self, y: Option<i32>) {
        let Some(navigation) = self.navigation else {
            return;
        };
        set_text(
            navigation.label,
            &format!("{} / {}", self.page + 1, self.pages),
        );
        unsafe {
            for (hwnd, x, width) in [
                (navigation.previous, 8, 40),
                (navigation.label, 56, 248),
                (navigation.next, 312, 40),
            ] {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    self.px(x),
                    self.px(y.unwrap_or(0)),
                    self.px(width),
                    self.px(32),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                // Far cheaper than SWP_SHOWWINDOW and SWP_HIDEWINDOW.
                let _ = ShowWindow(hwnd, if y.is_some() { SW_SHOWNA } else { SW_HIDE });
            }
            let _ = EnableWindow(navigation.previous, self.page > 0);
            let _ = EnableWindow(navigation.next, self.page + 1 < self.pages);
        }
    }

    /// Places the footer at `footer` and anchors the popup above the taskbar.
    fn finish_layout(&mut self, footer: i32, dividers: Vec<i32>) -> Result<()> {
        crate::appearance::set_layout(self.px(footer), dividers);
        unsafe {
            if let Some(link) = self.children.get(self.chrome_count.saturating_sub(1)) {
                SetWindowPos(
                    *link,
                    None,
                    self.px(8),
                    self.px(footer + 8),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )?;
            }
            let width = self.px(crate::config::POPUP_WIDTH);
            let height = self.px(footer + 48);
            let (x, y) = popup_position(self.work_area, self.monitor, width, height, self.px(12));
            let (x, y) = match &mut self.animation {
                // Keep sliding; only the destination moves with the new height.
                Some(animation) if !animation.closing => {
                    animation.to = y;
                    animation.x = x;
                    let mut rect = RECT::default();
                    GetWindowRect(self.hwnd, &mut rect)?;
                    (x, rect.top)
                }
                _ => (x, y),
            };
            if self.pending.is_some() {
                // A view transition will resize the window itself.
                self.target = Some(RECT {
                    left: x,
                    top: y,
                    right: x + width,
                    bottom: y + height,
                });
                return Ok(());
            }
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
        Ok(())
    }

    fn update_control(&mut self, index: usize) {
        let control = &self.controls[index];
        let Ok((level, muted)) = control.row.volume.read() else {
            return;
        };
        unsafe {
            let current = SendMessageW(control.slider, TBM_GETPOS, None, None).0 as i32;
            if current != level {
                SendMessageW(
                    control.slider,
                    TBM_SETPOS,
                    Some(WPARAM(1)),
                    Some(LPARAM(level as isize)),
                );
                // The custom track fill spans beyond the thumb rect the trackbar invalidates.
                let _ = InvalidateRect(Some(control.slider), None, false);
            }
            set_text(control.value, &level.to_string());
            match control.kind {
                Kind::Endpoint { input } => {
                    crate::appearance::set_glyph(control.mute, endpoint_glyph(input, level, muted))
                }
                Kind::App => crate::appearance::set_flag(control.mute, muted),
                Kind::Device => {}
            }
        }
        let tip = format!(
            "{} · {}",
            control.row.name,
            if muted {
                "ミュート".to_owned()
            } else {
                format!("{level}%")
            }
        );
        if tip != control.tip {
            self.tool(TTM_UPDATETIPTEXTW, control.slider, &tip);
            self.tool(TTM_UPDATETIPTEXTW, control.mute, &tip);
            self.controls[index].tip = tip;
        }
        self.controls[index].muted = muted;
    }

    /// Moves each slider's peak bar toward the current output level.
    fn tick_meters(&mut self) {
        for control in &mut self.controls {
            let level = if control.muted {
                0.0
            } else {
                control.row.peak()
            };
            // Let the bar fall gradually so short peaks stay readable.
            control.peak = level.max(control.peak * 0.8);
            unsafe {
                crate::appearance::set_meter(
                    control.slider,
                    (control.peak * 1000.0).round() as isize,
                );
            }
        }
    }

    fn open(&mut self) -> Result<()> {
        self.pending = None;
        self.target = None;
        self.end_transition();
        unsafe {
            if let Some(animation) = self.animation.take() {
                let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
                if animation.closing {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
            // Endpoints are cached while the popup is open; each opening reads them again.
            self.audio.forget();
            crate::appearance::load_theme();
            crate::appearance::style_window(self.hwnd);
            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            let identifier = NOTIFYICONIDENTIFIER {
                cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            if let Ok(rect) = Shell_NotifyIconGetRect(&identifier) {
                point = POINT {
                    x: rect.right,
                    y: rect.top,
                };
            }
            let monitor_handle = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(monitor_handle, &mut monitor).ok()?;
            // Move to the target monitor before measuring its effective DPI.
            SetWindowPos(
                self.hwnd,
                None,
                monitor.rcWork.left,
                monitor.rcWork.top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            let mut scale = GetDpiForWindow(self.hwnd).max(96) as f32 / 96.0;
            let arguments: Vec<_> = std::env::args().collect();
            let diagnostic = arguments
                .iter()
                .any(|arg| arg == "--snapshot" || arg == "--check-ui");
            if arguments.iter().any(|arg| arg == "--snapshot")
                && let Some(pair) = arguments.windows(2).find(|pair| pair[0] == "--preview-dpi")
                && let Ok(dpi) = pair[1].parse::<u32>()
            {
                scale = dpi.clamp(96, 288) as f32 / 96.0;
            }
            if scale != self.scale {
                self.clear_rows();
                let spare = self.spare.drain(..).map(|(_, hwnd)| hwnd);
                for child in self.children.drain(..).chain(spare) {
                    let _ = DestroyWindow(child);
                }
                self.navigation = None;
                self.chrome_count = 0;
                self.fonts = Fonts::new(scale);
                self.scale = scale;
            }
            self.work_area = monitor.rcWork;
            self.monitor = monitor.rcMonitor;
            let height = (monitor.rcWork.bottom - monitor.rcWork.top) as f32 / scale;
            // Fixed parts: endpoints, section header, pager, footer, and margins.
            self.page_size =
                (((height - 300.0) / 44.0) as usize).clamp(1, crate::config::MAX_APPLICATION_ROWS);
            self.device_rows = (((height - 180.0) / 68.0) as usize).max(1);
            self.view = View::Mixer;
            self.edit = None;
            self.armed_delete = None;
            crate::appearance::set_edit_frame(None);
            self.list_rows = (((height - 210.0) / 42.0) as usize).max(1);
            self.page = 0;
            self.device_offset = 0;
            self.refresh(true)?;
            // Hide focus cues until the keyboard is used, as the shell flyouts do.
            SendMessageW(
                self.hwnd,
                WM_CHANGEUISTATE,
                Some(WPARAM((UISF_HIDEFOCUS << 16 | UIS_SET) as usize)),
                None,
            );
            let mut rect = RECT::default();
            GetWindowRect(self.hwnd, &mut rect)?;
            let target = rect.top;
            let start = self.hidden_top(rect.bottom - rect.top);
            let animate = !diagnostic && animations_enabled() && start != target;
            // Below the taskbar in z-order, the popup slides out from behind it.
            let after = taskbar_for(monitor_handle).unwrap_or(HWND_TOPMOST);
            SetWindowPos(
                self.hwnd,
                Some(after),
                rect.left,
                if animate { start } else { target },
                0,
                0,
                SWP_NOSIZE | SWP_SHOWWINDOW,
            )?;
            let _ = SetForegroundWindow(self.hwnd);
            SetTimer(Some(self.hwnd), METER_TIMER, METER_INTERVAL_MS, None);
            // Activation raises the popup to the top of the topmost band again.
            let _ = SetWindowPos(
                self.hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            if animate {
                self.animation = Some(Animation {
                    start: Instant::now(),
                    from: start,
                    to: target,
                    x: rect.left,
                    closing: false,
                });
                SetTimer(Some(self.hwnd), ANIMATION_TIMER, 10, None);
            }
            if let Some(control) = self.controls.first() {
                let _ = SetFocus(Some(control.slider));
            }
            Ok(())
        }
    }

    /// The popup top when fully tucked behind the taskbar edge.
    fn hidden_top(&self, height: i32) -> i32 {
        if self.work_area.bottom < self.monitor.bottom {
            self.work_area.bottom
        } else if self.work_area.top > self.monitor.top {
            self.work_area.top - height
        } else {
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut rect);
            }
            rect.top + self.px(48)
        }
    }

    /// Slides the popup back behind the taskbar, then hides it.
    fn dismiss(&mut self) {
        self.end_transition();
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool()
                || self.animation.as_ref().is_some_and(|a| a.closing)
            {
                return;
            }
            self.dismissed = Some(Instant::now());
            let mut rect = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut rect);
            if !animations_enabled() {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            self.animation = Some(Animation {
                start: Instant::now(),
                from: rect.top,
                to: self.hidden_top(rect.bottom - rect.top),
                x: rect.left,
                closing: true,
            });
            SetTimer(Some(self.hwnd), ANIMATION_TIMER, 10, None);
        }
    }

    fn animate(&mut self) {
        let Some(animation) = &self.animation else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
            }
            return;
        };
        let duration = if animation.closing { 150.0 } else { 250.0 };
        let t = (animation.start.elapsed().as_secs_f32() * 1000.0 / duration).min(1.0);
        let eased = if animation.closing {
            ease_in(t)
        } else {
            ease_out(t)
        };
        let y = animation.from + ((animation.to - animation.from) as f32 * eased).round() as i32;
        let (x, closing) = (animation.x, animation.closing);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if t >= 1.0 {
                self.animation = None;
                let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
                if closing {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
        }
    }

    fn menu(&self, entries: &[(usize, &str)]) -> Result<usize> {
        unsafe {
            let menu = CreatePopupMenu()?;
            let result = (|| {
                for (id, name) in entries {
                    let name = wide(name);
                    AppendMenuW(menu, MF_STRING, *id, PCWSTR(name.as_ptr()))?;
                }
                let mut point = POINT::default();
                GetCursorPos(&mut point)?;
                let _ = SetForegroundWindow(self.hwnd);
                let chosen = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
                    point.x,
                    point.y,
                    None,
                    self.hwnd,
                    None,
                )
                .0 as usize;
                // Lets a menu opened for a tray icon close when the user clicks away.
                let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                Ok(chosen)
            })();
            let _ = DestroyMenu(menu);
            result
        }
    }

    fn open_settings(&self, uri: PCWSTR) -> Result<()> {
        unsafe {
            let result = ShellExecuteW(Some(self.hwnd), w!("open"), uri, None, None, SW_SHOWNORMAL);
            if result.0 as usize <= 32 {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )));
            }
        }
        Ok(())
    }

    /// The visible view's pixels and window rectangle, if it is worth animating from.
    fn capture_current(&mut self, forward: bool) -> Option<Pending> {
        self.end_transition();
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool()
                || self.animation.is_some()
                || !animations_enabled()
            {
                return None;
            }
            let mut rect = RECT::default();
            GetWindowRect(self.hwnd, &mut rect).ok()?;
            let capture = crate::appearance::capture(
                self.hwnd,
                &self.children,
                rect.right - rect.left,
                rect.bottom - rect.top,
            )?;
            Some(Pending {
                capture,
                top: rect.top,
                height: rect.bottom - rect.top,
                forward,
            })
        }
    }

    /// Moves the popup to `rect` at once.
    fn place(&self, rect: RECT) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    /// Fades from the captured old view to the rebuilt one while the window resizes.
    fn begin_transition(&mut self) {
        let pending = self.pending.take();
        let Some(target) = self.target.take() else {
            return;
        };
        let (width, height) = (target.right - target.left, target.bottom - target.top);
        let new = pending
            .as_ref()
            .and_then(|_| crate::appearance::capture(self.hwnd, &self.children, width, height));
        let (Some(pending), Some(new)) = (pending, new) else {
            self.place(target);
            return;
        };
        unsafe {
            // The popup paints the transition itself, so the controls step aside.
            let mut hidden = Vec::new();
            for child in &self.children {
                if GetWindowLongW(*child, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 {
                    hidden.push(*child);
                    let _ = ShowWindow(*child, SW_HIDE);
                }
            }
            let direction = if pending.forward { 1 } else { -1 };
            crate::appearance::start_transition(pending.capture, new, self.px(28) * direction);
            self.transition = Some(ViewTransition {
                start: Instant::now(),
                from_top: pending.top,
                from_height: pending.height,
                to_top: target.top,
                to_height: height,
                left: target.left,
                width,
                hidden,
            });
            SetTimer(Some(self.hwnd), TRANSITION_TIMER, 10, None);
        }
    }

    fn transition_step(&mut self) {
        let Some(transition) = &self.transition else {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TRANSITION_TIMER);
            }
            return;
        };
        let progress = (transition.start.elapsed().as_secs_f32() * 1000.0 / TRANSITION_MS).min(1.0);
        let eased = ease_out(progress);
        let lerp = |from: i32, to: i32| from + ((to - from) as f32 * eased).round() as i32;
        let top = lerp(transition.from_top, transition.to_top);
        let height = lerp(transition.from_height, transition.to_height);
        let (left, width) = (transition.left, transition.width);
        crate::appearance::set_transition_progress(progress);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            let _ = UpdateWindow(self.hwnd);
        }
        if progress >= 1.0 {
            self.end_transition();
        }
    }

    /// Finishes a view transition immediately, leaving the window at its final size.
    fn end_transition(&mut self) {
        let Some(transition) = self.transition.take() else {
            return;
        };
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TRANSITION_TIMER);
            crate::appearance::end_transition();
            let _ = SetWindowPos(
                self.hwnd,
                None,
                transition.left,
                transition.to_top,
                transition.width,
                transition.to_height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            // Showing controls one by one in a visible popup repaints for each; inside
            // `quietly` the popup already reports itself hidden.
            let visible = IsWindowVisible(self.hwnd).as_bool();
            if visible {
                SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
            }
            for child in &transition.hidden {
                let _ = ShowWindow(*child, SW_SHOWNA);
            }
            if visible {
                SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
            }
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
    }

    /// Hides the popup at once, without sliding.
    fn hide_now(&mut self) {
        self.end_transition();
        self.animation = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// The tray icon's context menu.
    fn tray_menu(&mut self) -> Result<()> {
        // The menu runs its own message loop, which would stall a sliding popup.
        self.hide_now();
        let entries = [(2, "mixerplus について"), (3, "Credits"), (1, "終了")];
        match self.menu(&entries)? {
            // The About window is created outside the App borrow, from the message loop.
            id @ (2 | 3) => unsafe {
                PostMessageW(Some(self.hwnd), SHOW_ABOUT, WPARAM(id - 2), LPARAM(0))?
            },
            1 => unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? },
            _ => {}
        }
        Ok(())
    }

    /// Changes what the popup shows once the clicked button's notification unwinds.
    fn show_view(&mut self, view: View) -> Result<()> {
        self.pending = self.capture_current(view != View::Mixer);
        self.view = view;
        self.edit = None;
        self.armed_delete = None;
        crate::appearance::set_edit_frame(None);
        self.device_offset = 0;
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }

    fn command(&mut self, id: usize) -> Result<()> {
        match id {
            PREVIOUS | NEXT => {
                self.page = if id == PREVIOUS {
                    self.page.saturating_sub(1)
                } else {
                    (self.page + 1).min(self.pages - 1)
                };
                // Rebuild rows after the native button notification has unwound.
                unsafe {
                    PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0))?;
                }
            }
            BACK => self.show_view(View::Mixer)?,
            PRESETS_BUTTON => self.show_view(View::Presets)?,
            NEW_PRESET => {
                self.edit = Some(EditState {
                    hwnd: HWND::default(),
                    target: None,
                });
                self.post_refresh()?;
            }
            id if id >= PRESET_ID => self.preset_command(id)?,
            SETTINGS_LINK => {
                self.dismiss();
                self.open_settings(w!("ms-settings:sound"))?;
            }
            MORE => {
                // Like the shell flyout, the gear opens the volume mixer settings.
                self.dismiss();
                self.open_settings(w!("ms-settings:apps-volume"))?;
            }
            id if id >= DEVICE_ID => {
                if let Some((_, device)) = self
                    .devices
                    .iter()
                    .find(|(item, _)| unsafe { GetDlgCtrlID(*item) as usize == id })
                {
                    let result = self.audio.set_default(&device.id);
                    // Refresh even after a failure so the list shows the actual default.
                    unsafe {
                        PostMessageW(Some(self.hwnd), REFRESH_STATE, WPARAM(0), LPARAM(0))?;
                    }
                    result?;
                }
            }
            id if id >= ROW_ID => {
                let index = (id - ROW_ID) / 3;
                match (id - ROW_ID) % 3 {
                    1 if self
                        .controls
                        .get(index)
                        .is_some_and(|c| c.kind != Kind::Device) =>
                    {
                        self.controls[index].row.volume.toggle_mute()?;
                        self.update_control(index);
                    }
                    2 => {
                        if let Some(Kind::Endpoint { input }) =
                            self.controls.get(index).map(|c| c.kind)
                        {
                            self.show_view(View::Devices { input })?;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Scrolls the device list or turns application pages.
    fn wheel(&mut self, delta: i16) -> Result<()> {
        let forward = delta < 0;
        match self.view {
            View::Devices { .. } | View::Presets => {
                let offset = if forward {
                    self.device_offset + 1
                } else {
                    self.device_offset.saturating_sub(1)
                };
                if offset != self.device_offset {
                    self.device_offset = offset;
                    return self.refresh(true);
                }
            }
            View::Mixer => {
                let page = if forward {
                    (self.page + 1).min(self.pages - 1)
                } else {
                    self.page.saturating_sub(1)
                };
                if page != self.page {
                    self.page = page;
                    return self.refresh(true);
                }
            }
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), REFRESH_TIMER);
            let _ = KillTimer(Some(self.hwnd), TRAY_TIMER);
            let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
            let _ = KillTimer(Some(self.hwnd), METER_TIMER);
            let _ = KillTimer(Some(self.hwnd), TRANSITION_TIMER);
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            let _ = DestroyWindow(self.hwnd);
            let _ = DestroyIcon(self.icon);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_PRINTCLIENT => {
                crate::appearance::background(hwnd, HDC(wparam.0 as *mut _));
                return LRESULT(0);
            }
            WM_PAINT => {
                crate::appearance::paint(hwnd);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            SHOW_ABOUT => {
                if crate::about::show(wparam.0 == 1).is_err() {
                    show_error("ウィンドウを開けませんでした。");
                }
                return LRESULT(0);
            }
            WM_DRAWITEM if lparam.0 != 0 => {
                crate::appearance::draw_item(&*(lparam.0 as *const DRAWITEMSTRUCT));
                return LRESULT(1);
            }
            WM_CTLCOLOREDIT => {
                return crate::appearance::edit_colors(HDC(wparam.0 as *mut _));
            }
            WM_CTLCOLORSTATIC => {
                return crate::appearance::text_background(HDC(wparam.0 as *mut _));
            }
            _ => {}
        }
        // Child creation and native menus can dispatch nested messages.
        let handled = APP.with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return None;
            };
            let app = state.as_mut()?;
            let result = match message {
                TRAY if lparam.0 as u32 == WM_RBUTTONUP => app.tray_menu(),
                TRAY if lparam.0 as u32 == WM_LBUTTONUP => {
                    // A click on the icon first deactivates an open popup; treat it as a toggle.
                    if app
                        .dismissed
                        .is_some_and(|time| time.elapsed().as_millis() < 400)
                    {
                        app.dismissed = None;
                        Ok(())
                    } else {
                        app.open()
                    }
                }
                WM_COMMAND if wparam.0 >> 16 == BN_CLICKED as usize && lparam.0 != 0 => {
                    let id = wparam.0 & 0xffff;
                    let sender = HWND(lparam.0 as *mut _);
                    if GetDlgCtrlID(sender) as usize == id {
                        app.clicked = sender;
                        app.command(id)
                    } else {
                        Ok(())
                    }
                }
                REFRESH_PAGE => app.refresh(true),
                REFRESH_STATE => app.refresh(false),
                COMMIT_EDIT => app.commit_edit(),
                DISMISS => {
                    if app.edit.is_some() {
                        app.cancel_edit()
                    } else if app.view == View::Mixer {
                        app.dismiss();
                        Ok(())
                    } else {
                        app.show_view(View::Mixer)
                    }
                }
                WM_MOUSEWHEEL => app.wheel((wparam.0 >> 16) as i16),
                WM_HSCROLL => {
                    if let Some(index) = app
                        .controls
                        .iter()
                        .position(|c| c.slider.0 as isize == lparam.0)
                    {
                        let control = &app.controls[index];
                        let level = SendMessageW(
                            control.slider,
                            TBM_GETPOS,
                            Some(WPARAM(0)),
                            Some(LPARAM(0)),
                        )
                        .0 as i32;
                        control
                            .row
                            .volume
                            .set_level(level)
                            .map(|_| app.update_control(index))
                    } else {
                        Ok(())
                    }
                }
                WM_TIMER if wparam.0 == ANIMATION_TIMER => {
                    app.animate();
                    Ok(())
                }
                WM_TIMER if wparam.0 == TRANSITION_TIMER => {
                    app.transition_step();
                    Ok(())
                }
                WM_TIMER if wparam.0 == METER_TIMER => {
                    if IsWindowVisible(hwnd).as_bool() {
                        app.tick_meters();
                    } else {
                        let _ = KillTimer(Some(hwnd), METER_TIMER);
                    }
                    Ok(())
                }
                WM_TIMER if wparam.0 == TRAY_TIMER => {
                    app.maintain_tray();
                    Ok(())
                }
                WM_TIMER
                    if wparam.0 == REFRESH_TIMER
                        && IsWindowVisible(hwnd).as_bool()
                        && app.animation.is_none()
                        && app.transition.is_none()
                        && GetCapture().is_invalid() =>
                {
                    app.refresh(false)
                }
                WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => {
                    app.dismiss();
                    Ok(())
                }
                WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
                    crate::appearance::load_theme();
                    crate::appearance::style_window(hwnd);
                    // Match the tray icon to the new taskbar color.
                    if let Ok(icon) = tray_icon() {
                        let previous = std::mem::replace(&mut app.icon, icon);
                        let _ = Shell_NotifyIconW(NIM_MODIFY, &app.tray_data());
                        let _ = DestroyIcon(previous);
                    }
                    let _ = RedrawWindow(
                        Some(hwnd),
                        None,
                        None,
                        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                    );
                    Ok(())
                }
                WM_CLOSE => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    PostQuitMessage(0);
                    Ok(())
                }
                m if m == app.taskbar_created => {
                    app.tray_added = false;
                    app.maintain_tray();
                    Ok(())
                }
                _ => return None,
            };
            if let Err(error) = result {
                let _ = ShowWindow(hwnd, SW_HIDE);
                show_error(&format!(
                    "The audio operation failed. Reopen mixerplus to refresh devices.\n\n{error}"
                ));
            }
            Some(LRESULT(0))
        });
        handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_stays_on_monitors_with_negative_coordinates() {
        let work = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        let monitor = RECT {
            bottom: 1080,
            ..work
        };
        assert_eq!(popup_position(work, monitor, 436, 510, 12), (-448, 518));
    }

    #[test]
    fn oversized_popup_has_a_defined_position() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 320,
            bottom: 240,
        };
        assert_eq!(popup_position(work, work, 436, 510, 12), (0, 0));
    }

    #[test]
    fn popup_follows_a_top_taskbar() {
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let work = RECT { top: 48, ..monitor };
        assert_eq!(popup_position(work, monitor, 360, 400, 12), (1548, 60));
    }
}
