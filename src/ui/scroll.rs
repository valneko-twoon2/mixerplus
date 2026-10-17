//! Pixel scrolling of the mixer rows inside the viewport, and the popup's scroll bar.

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Graphics::Dwm::DwmFlush;

/// Logical padding above the first mixer row and below the last.
pub(super) const CONTENT_TOP: i32 = 8;
const CONTENT_BOTTOM: i32 = 8;
/// Logical height of the pinned header with the shortcut hint.
pub(super) const HEADER: i32 = 40;
/// Logical distance one wheel notch scrolls, about one row like the shell flyout.
const NOTCH: i32 = 48;
/// How long a wheel or page scroll eases out.
const GLIDE_MS: f32 = 200.0;

/// Whether the frame pacing thread should keep running.
static PACING: AtomicBool = AtomicBool::new(false);

/// Posts `SCROLL_FRAME` once per desktop composition until pacing stops. Timer
/// messages arrive every 15.6 ms out of step with the display, which made glides stutter.
fn start_pacing(hwnd: HWND) {
    if PACING.swap(true, Ordering::AcqRel) {
        return;
    }
    // Window handles are plain values; the thread only posts to the popup.
    let target = hwnd.0 as isize;
    let started = std::thread::Builder::new()
        .name("scroll pacing".into())
        .spawn(move || {
            while PACING.load(Ordering::Acquire) {
                if unsafe { DwmFlush() }.is_err() {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                }
                let hwnd = HWND(target as *mut _);
                if unsafe { PostMessageW(Some(hwnd), SCROLL_FRAME, WPARAM(0), LPARAM(0)) }.is_err()
                {
                    PACING.store(false, Ordering::Release);
                }
            }
        });
    if started.is_err() {
        PACING.store(false, Ordering::Release);
    }
}

pub(super) fn stop_pacing() {
    PACING.store(false, Ordering::Release);
}

/// Forwards the viewport's child notifications to the popup, which owns the rows.
pub(super) unsafe extern "system" fn viewport_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                crate::appearance::paint_surface(hwnd);
                LRESULT(0)
            }
            WM_PRINTCLIENT => {
                crate::appearance::background(hwnd, HDC(wparam.0 as *mut _));
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_COMMAND | WM_HSCROLL | WM_DRAWITEM | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT
            | WM_NOTIFY | WM_MOUSEWHEEL => match GetParent(hwnd) {
                Ok(parent) => SendMessageW(parent, message, Some(wparam), Some(lparam)),
                Err(_) => LRESULT(0),
            },
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

impl App {
    /// The largest scroll offset in physical pixels.
    fn max_scroll(&self) -> i32 {
        (self.content_height - self.viewport_height).max(0)
    }

    /// Positions every mixer item below the scroll offset and sizes the popup to the
    /// visible part; the viewport clips rows that are partly scrolled out.
    pub(super) fn place_items(&mut self) -> Result<()> {
        let total: i32 = self.items.iter().map(|item| item.height).sum();
        // Without the shortcut there is nothing to pin.
        self.show_header(if self.hotkey { HEADER } else { 0 });
        let footer = CONTENT_TOP + total.min(self.content_limit) + CONTENT_BOTTOM;
        self.content_height = self.px(CONTENT_TOP + total + CONTENT_BOTTOM);
        self.viewport_height = self.px(footer);
        self.scroll = self.scroll.clamp(0, self.max_scroll());
        self.scroll_target = self.scroll;
        let mut top = CONTENT_TOP;
        for item in &mut self.items {
            item.top = top;
            top += item.height;
        }
        for item in &self.items {
            for &(hwnd, offset) in &item.windows {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        offset.x,
                        self.px(item.top) + offset.y - self.scroll,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    )?;
                }
            }
        }
        self.finish_layout(self.header + footer, self.dividers())?;
        self.show_scrollbar();
        Ok(())
    }

    /// Section dividers at the current scroll offset, in popup client pixels.
    fn dividers(&self) -> Vec<i32> {
        self.items
            .iter()
            .filter_map(|item| item.divider.map(|d| self.px(item.top + d) - self.scroll))
            .filter(|y| (0..self.viewport_height).contains(y))
            .collect()
    }

    /// Moves the rows to `target` physical pixels at once; reports whether they moved.
    pub(super) fn scroll_to(&mut self, target: i32) -> bool {
        let target = target.clamp(0, self.max_scroll());
        if self.view != View::Mixer || target == self.scroll {
            return false;
        }
        self.end_transition();
        let delta = self.scroll - target;
        self.scroll = target;
        unsafe {
            // Move the pixels and child windows together, then paint what came into view.
            let exposed = CreateRectRgn(0, 0, 0, 0);
            let _ = ScrollWindowEx(
                self.viewport,
                0,
                delta,
                None,
                None,
                Some(exposed),
                None,
                SW_SCROLLCHILDREN,
            );
            crate::appearance::set_dividers(self.dividers());
            let _ = RedrawWindow(
                Some(self.viewport),
                None,
                Some(exposed),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
            let _ = DeleteObject(exposed.into());
        }
        self.show_scrollbar();
        true
    }

    /// Eases the rows toward `target`, or jumps there when animations are off. A new
    /// target restarts the ease from the current position.
    fn glide_to(&mut self, target: i32) {
        self.scroll_target = target.clamp(0, self.max_scroll());
        if !animations_enabled() {
            self.scroll_to(self.scroll_target);
            return;
        }
        self.glide = Some(Glide {
            from: self.scroll,
            to: self.scroll_target,
            start: Instant::now(),
        });
        start_pacing(self.hwnd);
    }

    /// Places the rows where the glide is at this display frame.
    pub(super) fn scroll_frame(&mut self) {
        let Some(glide) = self.glide else {
            stop_pacing();
            return;
        };
        let progress = (glide.start.elapsed().as_secs_f32() * 1000.0 / GLIDE_MS).min(1.0);
        let position =
            glide.from + ((glide.to - glide.from) as f32 * ease_out(progress)).round() as i32;
        self.scroll_to(position);
        if progress >= 1.0 || self.view != View::Mixer {
            self.glide = None;
            self.scroll_target = self.scroll;
            stop_pacing();
        }
    }

    /// Stops a running glide where it is.
    pub(super) fn stop_glide(&mut self) {
        self.glide = None;
        self.scroll_target = self.scroll;
        stop_pacing();
    }

    /// Scrolls the mixer for a wheel movement; positive deltas scroll toward the top.
    pub(super) fn wheel_scroll(&mut self, delta: i16) {
        let distance = self.px(NOTCH) * delta as i32 / WHEEL_DELTA as i32;
        self.glide_to(self.scroll_target - distance);
    }

    /// Scrolls the focused row fully into view, after keyboard navigation.
    pub(super) fn reveal_focus(&mut self) {
        let focus = unsafe { GetFocus() };
        let Some(item) = self
            .items
            .iter()
            .find(|item| item.windows.iter().any(|&(hwnd, _)| hwnd == focus))
        else {
            return;
        };
        let top = self.px(item.top - CONTENT_TOP);
        let bottom = self.px(item.top + item.height + CONTENT_BOTTOM);
        self.stop_glide();
        if top < self.scroll {
            self.scroll_to(top);
        } else if bottom > self.scroll + self.viewport_height {
            self.scroll_to(bottom - self.viewport_height);
        }
        self.scroll_target = self.scroll;
    }

    /// The first row control that Tab can reach.
    pub(super) fn first_visible_stop(&self) -> Option<HWND> {
        self.items
            .iter()
            .flat_map(|item| &item.windows)
            .map(|&(hwnd, _)| hwnd)
            .find(|&hwnd| {
                let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
                let wanted = WS_VISIBLE.0 | WS_TABSTOP.0;
                style & wanted == wanted
            })
    }

    /// The scroll bar's track and thumb for the current offset, while rows overflow.
    fn track(&self) -> Option<Track> {
        let max = self.max_scroll();
        if self.view != View::Mixer || max == 0 {
            return None;
        }
        let top = self.header + 4;
        let bottom = self.header + (self.viewport_height as f32 / self.scale) as i32 - 4;
        let length = bottom - top;
        let thumb = (length * self.viewport_height / self.content_height.max(1)).max(16);
        let thumb_top = top + (length - thumb) * self.scroll / max;
        Some(Track {
            top,
            bottom,
            thumb_top,
            thumb_bottom: thumb_top + thumb,
        })
    }

    /// Draws the scroll bar: a thin line, wider under the pointer or while dragged.
    pub(super) fn show_scrollbar(&self) {
        let wide = self.track_hot || self.drag.is_some();
        let (left, right) = if wide { (7, 2) } else { (5, 3) };
        let width = crate::config::POPUP_WIDTH;
        crate::appearance::set_scrollbar(self.track().map(|track| RECT {
            left: self.px(width - left),
            top: self.px(track.thumb_top),
            right: self.px(width - right),
            bottom: self.px(track.thumb_bottom),
        }));
        unsafe {
            let strip = RECT {
                left: self.px(width - SCROLLBAR_WIDTH),
                top: 0,
                right: self.px(width),
                bottom: self.px(self.header) + self.viewport_height,
            };
            let _ = InvalidateRect(Some(self.hwnd), Some(&strip), false);
        }
    }

    /// Whether the logical point lies on the scroll bar's track.
    fn on_track(&self, x: i32, y: i32) -> bool {
        self.track().is_some_and(|track| {
            x >= crate::config::POPUP_WIDTH - SCROLLBAR_WIDTH
                && (track.top..track.bottom).contains(&y)
        })
    }

    /// Converts popup client coordinates in `lparam` to logical pixels.
    fn logical_point(&self, lparam: LPARAM) -> (i32, i32) {
        let x = (lparam.0 & 0xffff) as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
        ((x / self.scale) as i32, (y / self.scale) as i32)
    }

    /// Grabs the thumb, or pages toward the pointer when the track is pressed.
    pub(super) fn track_press(&mut self, lparam: LPARAM) -> Result<()> {
        let (x, y) = self.logical_point(lparam);
        let Some(track) = self.track().filter(|_| self.on_track(x, y)) else {
            return Ok(());
        };
        if (track.thumb_top..track.thumb_bottom).contains(&y) {
            self.drag = Some(Drag {
                pointer: y,
                thumb: track.thumb_top,
            });
            unsafe {
                SetCapture(self.hwnd);
            }
            self.show_scrollbar();
            return Ok(());
        }
        let page = self.viewport_height * 9 / 10;
        self.glide_to(if y < track.thumb_top {
            self.scroll - page
        } else {
            self.scroll + page
        });
        Ok(())
    }

    /// Follows a thumb drag, or widens the bar under the pointer.
    pub(super) fn track_move(&mut self, lparam: LPARAM) -> Result<()> {
        let (x, y) = self.logical_point(lparam);
        if let (Some(drag), Some(track)) = (self.drag, self.track()) {
            let room = (track.bottom - track.top) - (track.thumb_bottom - track.thumb_top);
            let thumb = (drag.thumb + y - drag.pointer - track.top).clamp(0, room.max(0));
            let target = self.max_scroll() * thumb / room.max(1);
            self.stop_glide();
            self.scroll_to(target);
            self.scroll_target = self.scroll;
            return Ok(());
        }
        let hot = self.on_track(x, y);
        if hot != self.track_hot {
            self.track_hot = hot;
            if hot {
                let mut leave = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut leave);
                }
            }
            self.show_scrollbar();
        }
        Ok(())
    }

    /// Ends a thumb drag or the pointer's hover over the bar.
    pub(super) fn track_release(&mut self, hot: bool) {
        let dragging = self.drag.take().is_some();
        if dragging {
            unsafe {
                // Capture changes come back here, so the drag has already ended.
                let _ = ReleaseCapture();
            }
        }
        if dragging || self.track_hot != hot {
            self.track_hot = hot;
            self.show_scrollbar();
        }
    }
}
