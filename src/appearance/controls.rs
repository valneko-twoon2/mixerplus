//! Owner-drawn buttons, sliders, and labels: their painting and hover tracking.

use super::*;

thread_local! {
    static HOVER: Cell<isize> = const { Cell::new(0) };
    static ICON_FONTS: Cell<(HFONT, HFONT)> =
        const { Cell::new((HFONT(std::ptr::null_mut()), HFONT(std::ptr::null_mut()))) };
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

/// Fonts for glyph buttons and the small mute badge.
pub fn set_icon_fonts(icon: HFONT, badge: HFONT) {
    ICON_FONTS.set((icon, badge));
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
