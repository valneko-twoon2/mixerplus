//! Captured bitmaps of the popup, the cross-fade between them, and the preview export.

use super::*;

thread_local! {
    static TRANSITION: RefCell<Option<Transition>> = const { RefCell::new(None) };
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
            // Rows inside the viewport are clipped to it, as on screen.
            if let Ok(parent) = GetParent(*child)
                && parent != hwnd
            {
                let mut clip = RECT::default();
                let _ = GetWindowRect(parent, &mut clip);
                let mut corners = [
                    POINT {
                        x: clip.left,
                        y: clip.top,
                    },
                    POINT {
                        x: clip.right,
                        y: clip.bottom,
                    },
                ];
                MapWindowPoints(None, Some(hwnd), &mut corners);
                let _ =
                    IntersectClipRect(dc, corners[0].x, corners[0].y, corners[1].x, corners[1].y);
            }
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
pub(super) unsafe fn compose(dc: HDC, rect: RECT) -> bool {
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
