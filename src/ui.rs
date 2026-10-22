//! The tray popup: shared state and types. The behavior lives in the submodules.

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
    appearance::{GLYPH, ITEM, KEYS, LINK, SECONDARY, pack},
    audio::{Audio, Device, Row, SPATIAL_OFF, Spatial, Volume},
    presets::{PresetStore, sanitize_name},
    settings::Settings,
};

mod controls;
mod popup;
mod presets;
mod run;
mod scroll;
mod transition;
mod tray;
mod views;
mod window;

pub(crate) use popup::animations_enabled;
use popup::{keep_within, popup_position};
pub use run::{check_audio, run, show_error};
use tray::tray_icon;
use window::window_proc;

const TRAY: u32 = WM_APP + 1;
const REFRESH_PAGE: u32 = WM_APP + 2;
const DISMISS: u32 = WM_APP + 3;
/// Refreshes without forcing a rebuild, so a new default only moves the selection.
const REFRESH_STATE: u32 = WM_APP + 4;
const MORE: usize = 12;
const BACK: usize = 15;
const SETTINGS_LINK: usize = 16;
/// Each mixer row uses two IDs: slider and mute button, three apart.
const ROW_ID: usize = 100;
const DEVICE_ID: usize = 1000;
const SPATIAL_ID: usize = 2000;
const EDIT_ID: usize = 19;
const OUTPUT_CHEVRON: usize = 20;
const INPUT_CHEVRON: usize = 21;
const HOTKEY: i32 = 1;
const OUTPUT_MUTE: usize = 22;
const INPUT_MUTE: usize = 23;
const PRESETS_BUTTON: usize = 17;
const NEW_PRESET: usize = 18;
/// Toggles free movement and keeping the popup on top.
const PIN: usize = 24;
/// Each preset row uses four IDs: apply, favorite, rename, and delete.
const PRESET_ID: usize = 3000;
const COMMIT_EDIT: u32 = WM_APP + 5;
const SHOW_ABOUT: u32 = WM_APP + 6;
/// Sent after the dialog manager has moved focus with the keyboard.
const TAB_KEY: u32 = WM_APP + 7;
/// Posted once per display refresh while the mixer glides.
const SCROLL_FRAME: u32 = WM_APP + 8;
const EM_SETSEL: u32 = 0xb1;
const EM_SETLIMITTEXT: u32 = 0xc5;
const ES_AUTOHSCROLL_STYLE: u32 = 0x80;
const REFRESH_TIMER: usize = 1;
const TRAY_TIMER: usize = 2;
const ANIMATION_TIMER: usize = 3;
const METER_TIMER: usize = 4;
const TRANSITION_TIMER: usize = 5;
/// Logical width of the strip at the popup's right edge that holds the scroll bar.
const SCROLLBAR_WIDTH: i32 = 8;
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
const CHEVRON_DOWN: u16 = 0xe70d;
const CHEVRON_UP: u16 = 0xe70e;
const PIN_GLYPH: u16 = 0xe718;
const PINNED_GLYPH: u16 = 0xe841;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Mixer,
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
    /// An output or input device; its list item makes it the default.
    Device,
    /// Application row: icon mute button with a badge.
    App,
}

/// The mute button in a group heading, which mutes the group's default device.
struct GroupMute {
    input: bool,
    button: HWND,
    volume: Volume,
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

/// One scroll step of the mixer: a row, or the section header.
struct Item {
    /// Logical top in the scrolling content.
    top: i32,
    height: i32,
    /// Windows with their physical position: x in the viewport, y from the item's top.
    windows: Vec<(HWND, POINT)>,
    /// A section divider this many logical pixels below the item's top.
    divider: Option<i32>,
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

/// What the mixer view shows.
struct MixerState {
    apps: Vec<Row>,
    /// Output then input devices tagged `input`: every device of an expanded group,
    /// only the default one of a collapsed group.
    lists: Vec<(bool, Vec<Device>)>,
    /// Spatial sound of the default output while its device list is expanded.
    spatial: Option<Spatial>,
}

/// The mixer's scroll bar while its rows overflow, in logical pixels.
#[derive(Clone, Copy)]
struct Track {
    top: i32,
    bottom: i32,
    thumb_top: i32,
    thumb_bottom: i32,
}

/// A wheel or page scroll easing from `from` to `to` physical pixels.
#[derive(Clone, Copy)]
struct Glide {
    from: i32,
    to: i32,
    start: Instant,
}

/// A scroll bar drag: the pointer and thumb tops when the thumb was grabbed.
#[derive(Clone, Copy)]
struct Drag {
    pointer: i32,
    thumb: i32,
}

/// The old view's pixels and window rectangle, captured before a view change.
struct Pending {
    capture: crate::appearance::Capture,
    top: i32,
    height: i32,
    /// Direction the new content slides in from: 1, -1, or 0 for a plain cross-fade.
    slide: i32,
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
    group_mutes: Vec<GroupMute>,
    /// Whether Win+Alt+V is registered, which the output heading then advertises.
    hotkey: bool,
    devices: Vec<(HWND, Device)>,
    /// Spatial sound buttons with their format subtypes.
    spatial: Vec<(HWND, String)>,
    /// The output that the spatial sound buttons configure.
    spatial_device: String,
    children: Vec<HWND>,
    /// Hidden row windows from earlier pages, reused before creating new ones.
    spare: Vec<(Class, HWND)>,
    /// The button whose click is being handled, which is never reused.
    clicked: HWND,
    fonts: Fonts,
    icon: HICON,
    /// The child window that holds and clips the rows; chrome stays outside it.
    viewport: HWND,
    /// Logical height of the pinned header above the viewport, 0 when it is hidden.
    header: i32,
    /// The header's title and key caps, shown only while the mixer has the header.
    header_windows: Vec<HWND>,
    /// Mixer rows and headers in order.
    items: Vec<Item>,
    /// How far the mixer rows are scrolled, and where a wheel glide is heading, in
    /// physical pixels.
    scroll: i32,
    scroll_target: i32,
    glide: Option<Glide>,
    /// Physical heights of all mixer content and of its visible part.
    content_height: i32,
    viewport_height: i32,
    /// Wheel movement below one notch on the preset page, kept until it adds up.
    wheel_rest: i32,
    /// Tallest the scrolling content may be, in logical pixels.
    content_limit: i32,
    drag: Option<Drag>,
    /// Whether the pointer is over the scroll bar, which widens it.
    track_hot: bool,
    signature: Vec<String>,
    taskbar_created: u32,
    scale: f32,
    /// First preset shown on the preset page.
    list_offset: usize,
    work_area: RECT,
    monitor: RECT,
    chrome_count: usize,
    tray_added: bool,
    promotion_attempts: u32,
    view: View,
    animation: Option<Animation>,
    presets: PresetStore,
    settings: Settings,
    settings_path: Option<std::path::PathBuf>,
    /// The endpoint row whose chevron should take focus after the next rebuild.
    refocus: Option<bool>,
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
    /// Whether the popup floats: it can be dragged anywhere, stays above other windows,
    /// and stays open when it loses focus. Only for the running session.
    pinned: bool,
    /// The footer button that toggles `pinned`.
    pin_button: HWND,
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

/// Reports a preset file failure like any other operation failure.
fn io_error(error: std::io::Error) -> windows::core::Error {
    windows::core::Error::new(
        windows::core::HRESULT(0x80004005u32 as i32),
        error.to_string(),
    )
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
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), REFRESH_TIMER);
            let _ = KillTimer(Some(self.hwnd), TRAY_TIMER);
            let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY);
            let _ = KillTimer(Some(self.hwnd), METER_TIMER);
            let _ = KillTimer(Some(self.hwnd), TRANSITION_TIMER);
            scroll::stop_pacing();
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            let _ = DestroyWindow(self.hwnd);
            let _ = DestroyIcon(self.icon);
        }
    }
}
