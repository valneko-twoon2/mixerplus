//! Opening, positioning, sliding, and dismissing the popup.

use super::*;

/// Places the popup at the work area corner nearest the taskbar, like the shell flyouts.
pub(super) fn popup_position(
    work: RECT,
    monitor: RECT,
    width: i32,
    height: i32,
    margin: i32,
) -> (i32, i32) {
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

impl App {
    pub(super) fn open(&mut self) -> Result<()> {
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
                self.chrome_count = 0;
                self.fonts = Fonts::new(scale);
                self.scale = scale;
            }
            self.work_area = monitor.rcWork;
            self.monitor = monitor.rcMonitor;
            let height = (monitor.rcWork.bottom - monitor.rcWork.top) as f32 / scale;
            // The rows get what the fixed parts and the pinned header leave.
            self.content_limit = (height as i32 - scroll::FIXED_HEIGHT - self.header_height())
                .clamp(scroll::MIN_CONTENT, crate::config::MAX_CONTENT_HEIGHT);
            self.view = View::Mixer;
            self.edit = None;
            self.armed_delete = None;
            crate::appearance::set_edit_frame(None);
            self.list_rows = (((height - 210.0) / 42.0) as usize).max(1);
            self.scroll = 0;
            self.glide = None;
            self.wheel_rest = 0;
            self.list_offset = 0;
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
            if let Some(first) = self.first_visible_stop() {
                let _ = SetFocus(Some(first));
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
    pub(super) fn dismiss(&mut self) {
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

    pub(super) fn animate(&mut self) {
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

    /// Hides the popup at once, without sliding.
    pub(super) fn hide_now(&mut self) {
        self.end_transition();
        self.animation = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
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
