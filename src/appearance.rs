//! Native rendering in the style of the Windows 11 quick settings flyouts.

use std::{
    cell::{Cell, RefCell},
    mem::size_of,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::{Registry::*, SystemServices::SS_CENTER},
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetCapture, GetFocus, IsWindowEnabled, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            },
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, w},
};

mod controls;
mod icon;
mod transition;

pub use controls::{
    draw_item, install_button, install_label, install_slider, recycle, set_app_icon, set_flag,
    set_glyph, set_icon_fonts, set_meter,
};
pub use icon::tray_icon;
use transition::compose;
pub use transition::{
    Capture, capture, end_transition, set_transition_progress, snapshot, start_transition,
};

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Owner-drawn button kinds, stored in the low byte of `GWLP_USERDATA`.
pub const GLYPH: isize = 1;
pub const LINK: isize = 2;
pub const ITEM: isize = 3;
pub const APP: isize = 4;
/// Key caps: a non-interactive hint such as `Win+Alt+V`, drawn from the caption.
pub const KEYS: isize = 5;
/// Static text drawn in the secondary color.
pub const SECONDARY: isize = 1;

/// Packs a button kind, its Segoe Fluent Icons glyph, and a selection/mute flag.
pub const fn pack(kind: isize, glyph: u16, flag: bool) -> isize {
    kind | (glyph as isize) << 8 | (flag as isize) << 24
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub background: COLORREF,
    pub footer: COLORREF,
    pub divider: COLORREF,
    pub border: COLORREF,
    pub text: COLORREF,
    pub secondary: COLORREF,
    pub disabled: COLORREF,
    pub hover: COLORREF,
    pub pressed: COLORREF,
    pub selected: COLORREF,
    pub accent: COLORREF,
    pub track: COLORREF,
    pub thumb: COLORREF,
    pub thumb_border: COLORREF,
    /// Live output level drawn over the filled part of a slider.
    pub meter: COLORREF,
}

/// The current light or dark palette, for other native windows in the same style.
pub fn palette() -> Palette {
    PALETTE.get()
}

// Approximations of the flyout acrylic and the WinUI control fills on it.
const LIGHT: Palette = Palette {
    dark: false,
    background: rgb(238, 238, 238),
    footer: rgb(230, 230, 230),
    divider: rgb(217, 217, 217),
    border: rgb(212, 212, 212),
    text: rgb(27, 27, 27),
    secondary: rgb(95, 95, 95),
    disabled: rgb(160, 160, 160),
    hover: rgb(246, 246, 246),
    pressed: rgb(242, 242, 242),
    selected: rgb(249, 249, 249),
    accent: rgb(0, 95, 184),
    track: rgb(134, 134, 134),
    thumb: rgb(255, 255, 255),
    thumb_border: rgb(222, 222, 222),
    meter: rgb(0, 58, 112),
};

const DARK: Palette = Palette {
    dark: true,
    background: rgb(44, 44, 44),
    footer: rgb(34, 34, 34),
    divider: rgb(28, 28, 28),
    border: rgb(64, 64, 64),
    text: rgb(255, 255, 255),
    secondary: rgb(200, 200, 200),
    disabled: rgb(120, 120, 120),
    hover: rgb(55, 55, 55),
    pressed: rgb(50, 50, 50),
    selected: rgb(60, 60, 60),
    accent: rgb(96, 205, 255),
    track: rgb(158, 158, 158),
    thumb: rgb(69, 69, 69),
    thumb_border: rgb(58, 58, 58),
    meter: rgb(40, 124, 166),
};

#[derive(Default)]
struct Layout {
    footer: i32,
    /// Divider rows, relative to the top of the row viewport.
    dividers: Vec<i32>,
    /// Where the row viewport starts in the popup, below the pinned header.
    top: i32,
    /// The scroll position indicator at the popup's right edge.
    scrollbar: Option<RECT>,
    /// The rounded frame behind an inline text box, in popup client pixels.
    edit: Option<RECT>,
}

thread_local! {
    static PALETTE: Cell<Palette> = const { Cell::new(LIGHT) };
    static LAYOUT: RefCell<Layout> = RefCell::default();
    static EDIT_BRUSH: Cell<HBRUSH> = const { Cell::new(HBRUSH(std::ptr::null_mut())) };
    static VIEWPORT: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
}

/// Reads the system light/dark mode and accent color, as the shell flyouts do.
pub fn load_theme() {
    unsafe {
        let mut light = 1u32;
        let mut size = size_of::<u32>() as u32;
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut light as *mut u32).cast()),
            Some(&mut size),
        );
        let mut palette = if light != 0 { LIGHT } else { DARK };
        let mut accent = [0u8; 32];
        let mut size = accent.len() as u32;
        if RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(accent.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_ok()
            && size as usize == accent.len()
        {
            // Light mode uses the first darker shade; dark mode the second lighter one.
            let i = if palette.dark { 1 } else { 4 } * 4;
            palette.accent = rgb(accent[i], accent[i + 1], accent[i + 2]);
        }
        PALETTE.set(palette);
        // The text box brush follows the palette; it is recreated on next use.
        let stale = EDIT_BRUSH.replace(HBRUSH::default());
        if !stale.is_invalid() {
            let _ = DeleteObject(stale.into());
        }
    }
}

/// Records the footer top and section divider rows in popup client pixels, and
/// removes the scroll bar.
pub fn set_layout(footer: i32, dividers: Vec<i32>, top: i32) {
    LAYOUT.with(|layout| {
        let mut layout = layout.borrow_mut();
        layout.footer = footer;
        layout.dividers = dividers;
        layout.top = top;
        layout.scrollbar = None;
    });
}

/// Names the popup's row container, which paints the surface, dividers, and text box
/// frame like the popup itself. It sits at the popup's client origin.
pub fn set_viewport(hwnd: HWND) {
    VIEWPORT.set(hwnd);
}

/// Moves the section dividers, in popup client pixels, after scrolling.
pub fn set_dividers(dividers: Vec<i32>) {
    LAYOUT.with(|layout| layout.borrow_mut().dividers = dividers);
}

/// Draws the scroll bar thumb at `rect` in popup client pixels, or removes it.
pub fn set_scrollbar(rect: Option<RECT>) {
    LAYOUT.with(|layout| layout.borrow_mut().scrollbar = rect);
}

/// Draws a focused text box frame at `rect` behind an EDIT control, or removes it.
pub fn set_edit_frame(rect: Option<RECT>) {
    LAYOUT.with(|layout| layout.borrow_mut().edit = rect);
}

/// Colors for EDIT controls (`WM_CTLCOLOREDIT`), matching the frame behind them.
pub unsafe fn edit_colors(dc: HDC) -> LRESULT {
    unsafe {
        let palette = PALETTE.get();
        SetTextColor(dc, palette.text);
        SetBkColor(dc, palette.thumb);
        let mut brush = EDIT_BRUSH.get();
        if brush.is_invalid() {
            brush = CreateSolidBrush(palette.thumb);
            EDIT_BRUSH.set(brush);
        }
        LRESULT(brush.0 as isize)
    }
}

pub fn style_window(hwnd: HWND) {
    let palette = PALETTE.get();
    unsafe {
        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&preference as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        let dark = BOOL::from(palette.dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const BOOL).cast(),
            size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&palette.border as *const COLORREF).cast(),
            size_of::<COLORREF>() as u32,
        );
    }
}

pub(crate) unsafe fn fill(dc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

/// Paints the popup surface, or the part of it behind a child control.
pub unsafe fn background(hwnd: HWND, dc: HDC) {
    unsafe { background_sized(hwnd, dc, None) }
}

/// Like [`background`]; `size` overrides the popup's client size, for capturing a
/// layout before the window is resized to it.
unsafe fn background_sized(hwnd: HWND, dc: HDC, size: Option<(i32, i32)>) {
    unsafe {
        let palette = PALETTE.get();
        let mut local = RECT::default();
        let _ = GetClientRect(hwnd, &mut local);
        if let Some((width, height)) = size {
            local = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
        }
        let parent = GetParent(hwnd).unwrap_or(hwnd);
        LAYOUT.with(|layout| {
            let layout = layout.borrow();
            if parent == hwnd || hwnd == VIEWPORT.get() {
                fill(dc, local, palette.background);
                if layout.footer > 0 {
                    fill(
                        dc,
                        RECT {
                            top: layout.footer,
                            ..local
                        },
                        palette.footer,
                    );
                }
                // The viewport paints its own dividers; the popup's lie below the header.
                let offset = if hwnd == VIEWPORT.get() {
                    0
                } else {
                    layout.top
                };
                for y in layout
                    .dividers
                    .iter()
                    .map(|y| y + offset)
                    .chain([layout.footer])
                {
                    if y > 0 {
                        fill(
                            dc,
                            RECT {
                                top: y,
                                bottom: y + 1,
                                ..local
                            },
                            palette.divider,
                        );
                    }
                }
                if let Some(bar) = layout.scrollbar {
                    // Like the thin indicator a WinUI scroll bar collapses to.
                    solid_round(dc, bar, bar.right - bar.left, palette.track);
                }
                if let Some(edit) = layout.edit {
                    let scale = scale_of(hwnd);
                    let inset = (scale.round() as i32).max(1);
                    let radius = (8.0 * scale).round() as i32;
                    solid_round(dc, edit, radius, palette.accent);
                    let inner = RECT {
                        left: edit.left + inset,
                        top: edit.top + inset,
                        right: edit.right - inset,
                        bottom: edit.bottom - inset,
                    };
                    solid_round(dc, inner, radius - inset, palette.thumb);
                }
                return;
            }
            let mut screen = RECT::default();
            let _ = GetWindowRect(hwnd, &mut screen);
            let mut origin = POINT {
                x: screen.left,
                y: screen.top,
            };
            let _ = ScreenToClient(parent, &mut origin);
            let in_footer = layout.footer > 0 && origin.y >= layout.footer;
            fill(
                dc,
                local,
                if in_footer {
                    palette.footer
                } else {
                    palette.background
                },
            );
        });
    }
}

pub(crate) unsafe fn solid_round(dc: HDC, rect: RECT, radius: i32, color: COLORREF) {
    if let Some(canvas) = unsafe { crate::drawing::Canvas::new(dc) } {
        canvas.round(rect, radius, color);
    }
}

fn scale_of(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd).max(96) as f32 / 96.0 }
}

/// Offscreen frame so hover, drag, and refresh repaints never show partial drawing.
pub(crate) struct Frame {
    target: HDC,
    rect: RECT,
    pub(crate) dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Frame {
    pub(crate) unsafe fn new(target: HDC, rect: RECT) -> Self {
        unsafe {
            let memory = CreateCompatibleDC(Some(target));
            let bitmap =
                CreateCompatibleBitmap(target, rect.right - rect.left, rect.bottom - rect.top);
            if memory.is_invalid() || bitmap.is_invalid() {
                if !bitmap.is_invalid() {
                    let _ = DeleteObject(bitmap.into());
                }
                if !memory.is_invalid() {
                    let _ = DeleteDC(memory);
                }
                // Fall back to direct drawing rather than painting nothing.
                return Self {
                    target,
                    rect,
                    dc: target,
                    bitmap: HBITMAP::default(),
                    previous: HGDIOBJ::default(),
                };
            }
            let previous = SelectObject(memory, bitmap.into());
            let _ = SetViewportOrgEx(memory, -rect.left, -rect.top, None);
            Self {
                target,
                rect,
                dc: memory,
                bitmap,
                previous,
            }
        }
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if self.bitmap.is_invalid() {
            return;
        }
        unsafe {
            let _ = SetViewportOrgEx(self.dc, 0, 0, None);
            let _ = BitBlt(
                self.target,
                self.rect.left,
                self.rect.top,
                self.rect.right - self.rect.left,
                self.rect.bottom - self.rect.top,
                Some(self.dc),
                0,
                0,
                SRCCOPY,
            );
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut paint);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let frame = Frame::new(target, rect);
        if !compose(frame.dc, rect) {
            background(hwnd, frame.dc);
        }
        drop(frame);
        let _ = EndPaint(hwnd, &paint);
    }
}

/// Paints a window that shows only the popup surface, such as the row viewport.
pub unsafe fn paint_surface(hwnd: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut paint);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let frame = Frame::new(target, rect);
        background(hwnd, frame.dc);
        drop(frame);
        let _ = EndPaint(hwnd, &paint);
    }
}

pub unsafe fn text_background(dc: HDC) -> LRESULT {
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        LRESULT(GetStockObject(NULL_BRUSH).0 as isize)
    }
}
