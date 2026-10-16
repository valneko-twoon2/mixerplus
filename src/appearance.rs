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

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Owner-drawn button kinds, stored in the low byte of `GWLP_USERDATA`.
pub const GLYPH: isize = 1;
pub const LINK: isize = 2;
pub const ITEM: isize = 3;
pub const APP: isize = 4;
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
    dividers: Vec<i32>,
    /// The rounded frame behind an inline text box, in popup client pixels.
    edit: Option<RECT>,
}

thread_local! {
    static PALETTE: Cell<Palette> = const { Cell::new(LIGHT) };
    static LAYOUT: RefCell<Layout> = RefCell::default();
    static HOVER: Cell<isize> = const { Cell::new(0) };
    static EDIT_BRUSH: Cell<HBRUSH> = const { Cell::new(HBRUSH(std::ptr::null_mut())) };
    static ICON_FONTS: Cell<(HFONT, HFONT)> =
        const { Cell::new((HFONT(std::ptr::null_mut()), HFONT(std::ptr::null_mut()))) };
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

/// Records the footer top and section divider rows in popup client pixels.
pub fn set_layout(footer: i32, dividers: Vec<i32>) {
    LAYOUT.with(|layout| {
        let mut layout = layout.borrow_mut();
        layout.footer = footer;
        layout.dividers = dividers;
    });
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

/// Fonts for glyph buttons and the small mute badge.
pub fn set_icon_fonts(icon: HFONT, badge: HFONT) {
    ICON_FONTS.set((icon, badge));
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
            if parent == hwnd {
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
                for &y in layout.dividers.iter().chain([layout.footer].iter()) {
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

fn hovered(hwnd: HWND) -> bool {
    HOVER.get() == hwnd.0 as isize
}

/// Tracks the control under the pointer so it can draw a hover fill.
unsafe fn track_hover(hwnd: HWND, message: u32) {
    unsafe {
        match message {
            WM_MOUSEMOVE if !hovered(hwnd) => {
                let previous = HWND(HOVER.replace(hwnd.0 as isize) as *mut _);
                if !previous.is_invalid() {
                    let _ = InvalidateRect(Some(previous), None, false);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
            }
            WM_MOUSELEAVE if hovered(hwnd) => {
                HOVER.set(0);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_NCDESTROY if hovered(hwnd) => HOVER.set(0),
            _ => {}
        }
    }
}

/// Keyboard focus cues are shown only after keyboard navigation, as in WinUI.
fn focus_visible(hwnd: HWND) -> bool {
    unsafe {
        GetFocus() == hwnd
            && SendMessageW(hwnd, WM_QUERYUISTATE, None, None).0 as u32 & UISF_HIDEFOCUS == 0
    }
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

pub unsafe fn draw_item(item: &DRAWITEMSTRUCT) {
    unsafe {
        let frame = Frame::new(item.hDC, item.rcItem);
        let dc = frame.dc;
        let rect = item.rcItem;
        let hwnd = item.hwndItem;
        background(hwnd, dc);
        let palette = PALETTE.get();
        let s = scale_of(hwnd);
        let px = |v: f32| (v * s).round() as i32;
        let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        let kind = data & 0xff;
        let glyph = [((data >> 8) & 0xffff) as u16];
        let flag = (data >> 24) & 1 != 0;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let fill = if kind == ITEM && flag {
            Some(palette.selected)
        } else if disabled {
            None
        } else if pressed {
            Some(palette.pressed)
        } else if hovered(hwnd) || focus_visible(hwnd) {
            Some(palette.hover)
        } else {
            None
        };
        if let Some(color) = fill {
            solid_round(dc, rect, px(8.0), color);
        }
        let color = if disabled {
            palette.disabled
        } else if kind == GLYPH && flag {
            // A glyph flagged on (a favorite, an armed delete) is drawn in the accent.
            palette.accent
        } else {
            palette.text
        };
        let (icon_font, badge_font) = ICON_FONTS.get();
        let font = HFONT(SendMessageW(hwnd, WM_GETFONT, None, None).0 as *mut _);
        let mut caption = [0u16; 512];
        let length = GetWindowTextW(hwnd, &mut caption) as usize;
        let caption = &caption[..length];
        let draw = |font: HFONT, text: &[u16], rect: RECT, center: bool, color: COLORREF| {
            let old = SelectObject(dc, font.into());
            crate::drawing::text(dc, text, rect, center, color);
            SelectObject(dc, old);
        };
        let cx = (rect.left + rect.right) / 2;
        let cy = (rect.top + rect.bottom) / 2;
        match kind {
            LINK => draw(
                font,
                caption,
                RECT {
                    left: rect.left + px(8.0),
                    ..rect
                },
                false,
                color,
            ),
            ITEM => {
                if flag {
                    solid_round(
                        dc,
                        RECT {
                            left: rect.left,
                            top: cy - px(8.0),
                            right: rect.left + px(3.0),
                            bottom: cy + px(8.0),
                        },
                        px(3.0),
                        palette.accent,
                    );
                }
                draw(
                    icon_font,
                    &glyph,
                    RECT {
                        left: rect.left + px(12.0),
                        right: rect.left + px(32.0),
                        ..rect
                    },
                    true,
                    color,
                );
                draw(
                    font,
                    caption,
                    RECT {
                        left: rect.left + px(44.0),
                        right: rect.right - px(8.0),
                        ..rect
                    },
                    false,
                    color,
                );
            }
            APP => {
                let icon = HICON(GetPropW(hwnd, w!("mixerplusIcon")).0);
                if icon.is_invalid() {
                    draw(icon_font, &glyph, rect, true, color);
                } else if let Some(canvas) = crate::drawing::Canvas::new(dc) {
                    canvas.icon(
                        icon,
                        RECT {
                            left: cx - px(10.0),
                            top: cy - px(10.0),
                            right: cx + px(10.0),
                            bottom: cy + px(10.0),
                        },
                    );
                }
                if flag {
                    // A small mute glyph below and right of the icon, like quick settings.
                    let badge = RECT {
                        left: cx + px(3.0),
                        top: cy + px(5.0),
                        right: cx + px(17.0),
                        bottom: cy + px(17.0),
                    };
                    solid_round(dc, badge, px(14.0), fill.unwrap_or(palette.background));
                    draw(badge_font, &[0xe74f], badge, true, color);
                }
            }
            // Glyph buttons show their packed glyph, or their caption when it is unset.
            _ => draw(
                if kind == GLYPH { icon_font } else { font },
                if glyph[0] != 0 { &glyph } else { caption },
                rect,
                true,
                color,
            ),
        }
        drop(frame);
    }
}

/// Owner-drawn button with hover feedback; `data` comes from [`pack`].
pub unsafe fn install_button(hwnd: HWND, data: isize) -> windows::core::Result<()> {
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, data);
        subclassed(hwnd, Some(button_proc), 3, 0)
    }
}

/// Gives an application button its icon; the caller keeps ownership of `icon`.
pub unsafe fn set_app_icon(hwnd: HWND, icon: HICON) {
    unsafe {
        let _ = SetPropW(hwnd, w!("mixerplusIcon"), Some(HANDLE(icon.0)));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Forgets a hidden control's per-use state so it can be reused as another control.
pub unsafe fn recycle(hwnd: HWND) {
    unsafe {
        let _ = RemovePropW(hwnd, w!("mixerplusIcon"));
        let _ = RemovePropW(hwnd, w!("mixerplusMeter"));
        if hovered(hwnd) {
            HOVER.set(0);
        }
    }
}

unsafe fn update_data(hwnd: HWND, mask: isize, value: isize) {
    unsafe {
        let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        let next = data & !mask | value & mask;
        if next != data {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, next);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

/// Updates a button's selection or mute flag, repainting only on change.
pub unsafe fn set_flag(hwnd: HWND, flag: bool) {
    unsafe { update_data(hwnd, 1 << 24, (flag as isize) << 24) }
}

/// Updates a button's glyph, repainting only on change.
pub unsafe fn set_glyph(hwnd: HWND, glyph: u16) {
    unsafe { update_data(hwnd, 0xffff << 8, (glyph as isize) << 8) }
}

unsafe extern "system" fn button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        track_hover(hwnd, message);
        match message {
            WM_ERASEBKGND => return LRESULT(1),
            WM_NCDESTROY => {
                let _ = RemovePropW(hwnd, w!("mixerplusIcon"));
                let _ = RemoveWindowSubclass(hwnd, Some(button_proc), 3);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// Installs custom painting and repaints anything drawn before it was in place.
unsafe fn subclassed(
    hwnd: HWND,
    proc: SUBCLASSPROC,
    id: usize,
    data: usize,
) -> windows::core::Result<()> {
    unsafe {
        SetWindowSubclass(hwnd, proc, id, data).ok()?;
        let _ = InvalidateRect(Some(hwnd), None, false);
        Ok(())
    }
}

/// Sets a slider's peak level in thousandths, repainting only on change.
pub unsafe fn set_meter(hwnd: HWND, permille: isize) {
    unsafe {
        if GetPropW(hwnd, w!("mixerplusMeter")).0 as isize != permille {
            let _ = SetPropW(hwnd, w!("mixerplusMeter"), Some(HANDLE(permille as *mut _)));
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

pub unsafe fn install_slider(hwnd: HWND) -> windows::core::Result<()> {
    unsafe { subclassed(hwnd, Some(slider_proc), 1, 0) }
}

/// Static text; `style` is 0 for primary text or [`SECONDARY`].
pub unsafe fn install_label(hwnd: HWND, style: isize) -> windows::core::Result<()> {
    unsafe { subclassed(hwnd, Some(label_proc), 2, style as usize) }
}

unsafe extern "system" fn label_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    style: usize,
) -> LRESULT {
    unsafe {
        if message == WM_ERASEBKGND {
            return LRESULT(1);
        }
        if message == WM_SETTEXT {
            // Static controls draw new text synchronously instead of posting WM_PAINT.
            // With a transparent brush, each update would stack glyphs over the old text.
            DefSubclassProc(hwnd, WM_SETREDRAW, WPARAM(0), LPARAM(0));
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            DefSubclassProc(hwnd, WM_SETREDRAW, WPARAM(1), LPARAM(0));
            let _ = InvalidateRect(Some(hwnd), None, false);
            return result;
        }
        if message == WM_PAINT || message == WM_PRINTCLIENT {
            let mut paint = PAINTSTRUCT::default();
            let target = if message == WM_PAINT {
                BeginPaint(hwnd, &mut paint)
            } else {
                HDC(wparam.0 as *mut _)
            };
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let frame = Frame::new(target, rect);
            let dc = frame.dc;
            background(hwnd, dc);
            let palette = PALETTE.get();
            let mut caption = [0u16; 1024];
            let length = GetWindowTextW(hwnd, &mut caption) as usize;
            let font = SendMessageW(hwnd, WM_GETFONT, None, None);
            let old = SelectObject(dc, HGDIOBJ(font.0 as *mut _));
            let center = GetWindowLongW(hwnd, GWL_STYLE) as u32 & SS_CENTER.0 != 0;
            let color = if style as isize == SECONDARY {
                palette.secondary
            } else {
                palette.text
            };
            crate::drawing::text(dc, &caption[..length], rect, center, color);
            SelectObject(dc, old);
            drop(frame);
            if message == WM_PAINT {
                let _ = EndPaint(hwnd, &paint);
            }
            return LRESULT(0);
        }
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(label_proc), 2);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

unsafe extern "system" fn slider_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        track_hover(hwnd, message);
        match message {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT | WM_PRINTCLIENT => {
                let mut paint = PAINTSTRUCT::default();
                let target = if message == WM_PAINT {
                    BeginPaint(hwnd, &mut paint)
                } else {
                    HDC(wparam.0 as *mut _)
                };
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let frame = Frame::new(target, rect);
                let dc = frame.dc;
                background(hwnd, dc);
                let palette = PALETTE.get();
                let s = scale_of(hwnd);
                let px = |v: f32| (v * s).round() as i32;
                let mut thumb = RECT::default();
                SendMessageW(
                    hwnd,
                    TBM_GETTHUMBRECT,
                    None,
                    Some(LPARAM((&mut thumb as *mut RECT) as isize)),
                );
                let x = (thumb.left + thumb.right) / 2;
                let y = rect.bottom / 2;
                let track = RECT {
                    left: px(10.0),
                    top: y - px(2.0),
                    right: rect.right - px(10.0),
                    bottom: y + px(2.0),
                };
                solid_round(dc, track, px(4.0), palette.track);
                solid_round(dc, RECT { right: x, ..track }, px(4.0), palette.accent);
                // The peak level, scaled into the filled part like the Settings mixer.
                let peak = GetPropW(hwnd, w!("mixerplusMeter")).0 as i64;
                let level = ((x - track.left).max(0) as i64 * peak / 1000) as i32;
                if level > 0 {
                    solid_round(
                        dc,
                        RECT {
                            right: track.left + level,
                            ..track
                        },
                        px(4.0),
                        palette.meter,
                    );
                }
                let circle = |radius: f32| RECT {
                    left: x - px(radius),
                    top: y - px(radius),
                    right: x + px(radius),
                    bottom: y + px(radius),
                };
                solid_round(dc, circle(10.0), px(20.0), palette.thumb_border);
                solid_round(dc, circle(9.0), px(18.0), palette.thumb);
                // WinUI grows the inner dot on hover and shrinks it while dragging.
                let dot = if GetCapture() == hwnd {
                    5.0
                } else if hovered(hwnd) {
                    7.0
                } else {
                    6.0
                };
                solid_round(dc, circle(dot), px(dot * 2.0), palette.accent);
                if focus_visible(hwnd)
                    && let Some(canvas) = crate::drawing::Canvas::new(dc)
                {
                    canvas.ellipse(circle(13.0), palette.text, 1.5 * s);
                }
                drop(frame);
                if message == WM_PAINT {
                    let _ = EndPaint(hwnd, &paint);
                }
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                let _ = RemovePropW(hwnd, w!("mixerplusMeter"));
                let _ = RemoveWindowSubclass(hwnd, Some(slider_proc), 1);
            }
            _ => {}
        }
        let result = DefSubclassProc(hwnd, message, wparam, lparam);
        if matches!(
            message,
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_KEYDOWN | WM_SETFOCUS | WM_KILLFOCUS
        ) {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        result
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

pub unsafe fn text_background(dc: HDC) -> LRESULT {
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        LRESULT(GetStockObject(NULL_BRUSH).0 as isize)
    }
}

/// Renders one glyph and returns its per-pixel coverage. It is drawn in the final
/// foreground over the matching taskbar shade so text contrast tuning applies the
/// same way as for the system icons.
unsafe fn glyph_coverage(
    size: i32,
    glyph: u16,
    em: f32,
    rect: RECT,
    dark: bool,
) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.into());
        if !dark {
            std::ptr::write_bytes(bits.cast::<u8>(), 0xff, (size * size * 4) as usize);
        }
        let font = CreateFontW(
            -(em.round() as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            crate::drawing::icon_face(),
        );
        let previous_font = SelectObject(dc, font.into());
        let ink = if dark { 255 } else { 0 };
        crate::drawing::text(dc, &[glyph], rect, true, rgb(ink, ink, ink));
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), (size * size * 4) as usize)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| if dark { pixel[2] } else { 255 - pixel[2] })
            .collect();
        SelectObject(dc, previous_font);
        SelectObject(dc, previous);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Some(pixels)
    }
}

/// The taskbar speaker glyph in the theme's accent color, with the weight of the
/// system icons for the taskbar's light or dark shade.
pub fn tray_icon(size: i32) -> Option<HICON> {
    let palette = PALETTE.get();
    let dark = palette.dark;
    unsafe {
        let full = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        let alpha = glyph_coverage(size, 0xe995, size as f32, full, dark)?;
        let accent = palette.accent;
        let (red, green, blue) = (
            accent.0 as u8,
            (accent.0 >> 8) as u8,
            (accent.0 >> 16) as u8,
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (size * size * 4) as usize);
        for (pixel, alpha) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
            pixel.copy_from_slice(&[blue, green, red, *alpha]);
        }
        let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok()
    }
}

/// Paints each visible child control into `dc`, in the popup's client coordinates.
unsafe fn paint_children(hwnd: HWND, dc: HDC, children: &[HWND]) {
    unsafe {
        for child in children {
            // Check the style bit; IsWindowVisible also depends on the popup itself.
            if GetWindowLongW(*child, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0 {
                continue;
            }
            let saved = SaveDC(dc);
            let mut screen = RECT::default();
            let _ = GetWindowRect(*child, &mut screen);
            let mut origin = POINT {
                x: screen.left,
                y: screen.top,
            };
            let _ = ScreenToClient(hwnd, &mut origin);
            let _ = SetViewportOrgEx(dc, origin.x, origin.y, None);
            let mut rect = RECT::default();
            let _ = GetClientRect(*child, &mut rect);
            let _ = IntersectClipRect(dc, 0, 0, rect.right, rect.bottom);
            let mut class = [0u16; 64];
            let length = GetClassNameW(*child, &mut class) as usize;
            if String::from_utf16_lossy(&class[..length]).eq_ignore_ascii_case("button") {
                draw_item(&DRAWITEMSTRUCT {
                    hwndItem: *child,
                    hDC: dc,
                    rcItem: rect,
                    itemState: if IsWindowEnabled(*child).as_bool() {
                        ODS_FLAGS(0)
                    } else {
                        ODS_DISABLED
                    },
                    ..Default::default()
                });
            } else {
                SendMessageW(
                    *child,
                    WM_PRINTCLIENT,
                    Some(WPARAM(dc.0 as usize)),
                    Some(LPARAM(PRF_CLIENT as isize)),
                );
            }
            let _ = RestoreDC(dc, saved);
        }
    }
}

/// A bitmap of the popup's content, kept for view transitions.
pub struct Capture {
    bitmap: HBITMAP,
    pub width: i32,
    pub height: i32,
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

/// Renders the popup's look at `width` by `height` into an offscreen bitmap.
pub fn capture(hwnd: HWND, children: &[HWND], width: i32, height: i32) -> Option<Capture> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.into());
        background_sized(hwnd, dc, Some((width, height)));
        paint_children(hwnd, dc, children);
        let _ = GdiFlush();
        SelectObject(dc, previous);
        let _ = DeleteDC(dc);
        Some(Capture {
            bitmap,
            width,
            height,
        })
    }
}

struct Transition {
    old: Capture,
    new: Capture,
    /// Horizontal distance the new content travels while it fades in.
    offset: i32,
    progress: f32,
}

thread_local! {
    static TRANSITION: RefCell<Option<Transition>> = const { RefCell::new(None) };
}

/// Starts a cross-fade from `old` to `new`; `offset` is the new content's slide-in distance.
pub fn start_transition(old: Capture, new: Capture, offset: i32) {
    TRANSITION.with(|state| {
        *state.borrow_mut() = Some(Transition {
            old,
            new,
            offset,
            progress: 0.0,
        });
    });
}

pub fn set_transition_progress(progress: f32) {
    TRANSITION.with(|state| {
        if let Some(transition) = state.borrow_mut().as_mut() {
            transition.progress = progress;
        }
    });
}

pub fn end_transition() {
    TRANSITION.with(|state| state.borrow_mut().take());
}

unsafe fn blend(dc: HDC, source: HDC, capture: &Capture, x: i32, alpha: f32) {
    let alpha = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    if alpha == 0 {
        return;
    }
    unsafe {
        SelectObject(source, capture.bitmap.into());
        let _ = AlphaBlend(
            dc,
            x,
            0,
            capture.width,
            capture.height,
            source,
            0,
            0,
            capture.width,
            capture.height,
            BLENDFUNCTION {
                BlendOp: 0,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: 0,
            },
        );
    }
}

/// Paints the running view transition into `dc`; false when none is running.
unsafe fn compose(dc: HDC, rect: RECT) -> bool {
    TRANSITION.with(|state| {
        let state = state.borrow();
        let Some(transition) = state.as_ref() else {
            return false;
        };
        unsafe {
            fill(dc, rect, PALETTE.get().background);
            let source = CreateCompatibleDC(Some(dc));
            let p = transition.progress;
            // The old view fades out first, then the new one slides in and fades up.
            blend(dc, source, &transition.old, 0, 1.0 - p / 0.4);
            let ease = 1.0 - (1.0 - p).powi(3);
            let x = ((1.0 - ease) * transition.offset as f32).round() as i32;
            blend(dc, source, &transition.new, x, (p - 0.25) / 0.75);
            let _ = DeleteDC(source);
        }
        true
    })
}

pub fn snapshot(
    hwnd: HWND,
    children: &[HWND],
    path: &std::path::Path,
) -> windows::core::Result<()> {
    unsafe {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client)?;
        let width = client.right;
        let height = client.bottom;
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(error) => {
                let _ = DeleteDC(dc);
                return Err(error);
            }
        };
        let previous = SelectObject(dc, bitmap.into());
        background(hwnd, dc);
        paint_children(hwnd, dc, children);
        let _ = GdiFlush();
        let pixel_count = (width * height * 4) as usize;
        let mut pixels = std::slice::from_raw_parts(bits.cast::<u8>(), pixel_count).to_vec();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        let mut file = Vec::with_capacity(54 + pixel_count);
        file.extend_from_slice(b"BM");
        file.extend_from_slice(&((54 + pixel_count) as u32).to_le_bytes());
        file.extend_from_slice(&[0u8; 4]);
        file.extend_from_slice(&54u32.to_le_bytes());
        file.extend_from_slice(&40u32.to_le_bytes());
        file.extend_from_slice(&width.to_le_bytes());
        file.extend_from_slice(&(-height).to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        file.extend_from_slice(&(pixel_count as u32).to_le_bytes());
        file.extend_from_slice(&[0u8; 16]);
        file.extend_from_slice(&pixels);
        std::fs::write(path, file).map_err(|error| {
            windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                error.to_string(),
            )
        })
    }
}
