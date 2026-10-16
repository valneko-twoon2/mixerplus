//! The cross-fade between views, which the popup paints itself while it resizes.

use super::*;

impl App {
    /// The visible view's pixels and window rectangle, if it is worth animating from.
    pub(super) fn capture_current(&mut self, forward: bool) -> Option<Pending> {
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
    pub(super) fn begin_transition(&mut self) {
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

    pub(super) fn transition_step(&mut self) {
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
    pub(super) fn end_transition(&mut self) {
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
}
