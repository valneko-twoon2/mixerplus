//! The About and Credits window, laid out like a Windows 11 Settings page: a large
//! title, section labels, and rounded cards on the window background. One
//! custom-painted window shows mixerplus's own license, the list of bundled crates,
//! and each crate's license text; none of it touches audio.
//!
//! The scrolling content is rendered once into an offscreen bitmap and only copied
//! while it scrolls, so wheel and keyboard scrolling can animate smoothly.

use std::{cell::RefCell, mem::size_of, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                SetWindowThemeAttribute, WM_MOUSELEAVE, WTA_NONCLIENT, WTA_OPTIONS,
                WTNCA_NODRAWCAPTION, WTNCA_NODRAWICON,
            },
            HiDpi::*,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            },
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, PCWSTR, Result, w},
};

use crate::appearance::{fill, palette, solid_round};

mod input;
mod layout;
mod notices;
mod paint;
mod window;

use input::{activate, animate, cursor, key, perform, scroll_by, step};
use notices::{blocks, parse_credits};
use paint::{paint, theme};
use window::client;
pub use window::{dispatch, show};

const LICENSE: &str = include_str!("../LICENSE");
const NOTICES: &str = include_str!("../THIRD-PARTY-NOTICES.txt");
const CLASS: PCWSTR = w!("mixerplusAbout");

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0d;
const VK_ESCAPE: u32 = 0x1b;
const VK_SPACE: u32 = 0x20;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const SPI_GETWORKAREA: SYSTEM_PARAMETERS_INFO_ACTION = SYSTEM_PARAMETERS_INFO_ACTION(0x30);
const SCROLL_TIMER: usize = 1;
/// Time constant of the scroll easing; about 95% of the distance takes three of these.
const SCROLL_EASE_MS: f32 = 55.0;

// Segoe Fluent Icons code point.
const BACK_GLYPH: &str = "\u{e72b}";

/// One bundled crate, parsed from `THIRD-PARTY-NOTICES.txt`.
struct Credit {
    name: &'static str,
    version: &'static str,
    license: &'static str,
    text: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Body,
    Heading,
    Gap,
}

/// A paragraph of license text, reflowed to the window width.
struct Block {
    kind: Kind,
    /// Leading spaces of the source line, kept as a left indent.
    indent: i32,
    text: Vec<u16>,
}

/// The Settings app's surfaces: a flat window color with slightly lighter cards.
#[derive(Clone, Copy, PartialEq)]
struct Theme {
    background: COLORREF,
    card: COLORREF,
    border: COLORREF,
    hover: COLORREF,
    button: COLORREF,
    button_hover: COLORREF,
    button_pressed: COLORREF,
    button_border: COLORREF,
    text: COLORREF,
    secondary: COLORREF,
    accent: COLORREF,
    track: COLORREF,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    About,
    Credits,
    Notice(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Back,
    Row(usize),
    License(usize),
    Homepage(usize),
    Thumb,
    Track,
}

struct Fonts {
    body: HFONT,
    title: HFONT,
    page: HFONT,
    small: HFONT,
    icon: HFONT,
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
        Self {
            body: font(13.0, 400, crate::drawing::FONT),
            title: font(14.0, 600, crate::drawing::FONT),
            page: font(28.0, 600, crate::drawing::FONT),
            small: font(12.0, 400, crate::drawing::FONT),
            icon: font(16.0, 400, icon_face),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.title, self.page, self.small, self.icon] {
            unsafe {
                let _ = DeleteObject(font.into());
            }
        }
    }
}

/// What the cached content depends on; any change re-renders it. Pointer and
/// keyboard states are drawn over the copy instead, so hovering never re-renders.
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    view: View,
    size: (i32, i32),
    theme: Theme,
    scale: u32,
}

/// The whole scrolling content, rendered once into an offscreen bitmap.
struct Cache {
    key: CacheKey,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Cache {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Ui {
    view: View,
    credits: Vec<Credit>,
    /// The current view's license text, for the About and Notice views.
    blocks: Vec<Block>,
    heights: Vec<i32>,
    /// Client size the heights were laid out for.
    laid: (i32, i32),
    dirty: bool,
    content: i32,
    /// Shown scroll offset, and the exact position of a running scroll animation.
    scroll: i32,
    position: f32,
    /// Where the scroll animation is heading.
    target: i32,
    tick: Option<Instant>,
    /// The Credits row chosen by click or keyboard; none until the user picks one.
    selected: Option<usize>,
    /// Keyboard navigation is active, so focus cues are drawn.
    keyboard: bool,
    focus: Hit,
    hover: Hit,
    pressed: Hit,
    /// Mouse offset inside the scrollbar thumb while dragging it.
    drag: Option<i32>,
    /// Credits scroll position and selection kept while a Notice is open.
    saved: (i32, Option<usize>),
    scale: f32,
    fonts: Fonts,
    tracking: bool,
    cache: Option<Cache>,
    /// Hidden owner window: an owned window gets no taskbar button.
    owner: HWND,
}

/// Rectangles of the current view, in client pixels.
struct Geo {
    back: RECT,
    /// The license card of the text views.
    card: RECT,
    /// The scrolling region; content is clipped to it.
    view: RECT,
    /// Top of the content below the page title.
    top: i32,
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn inflate(r: RECT, by: i32) -> RECT {
    rect(r.left - by, r.top - by, r.right + by, r.bottom + by)
}

fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Hovered and pressed state of a control: (hot, down).
type Hot = (bool, bool);

enum Action {
    None,
    Close,
    Homepage(&'static str),
}
