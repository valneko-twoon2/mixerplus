//! Creating, pooling, and updating the popup's child controls.

use super::*;

/// The glyph of a group's mute button: the speaker's waves follow the level.
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

impl App {
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
                // Pooled windows differ in whether Tab reaches them.
                let current = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                let tabs = current & !WS_TABSTOP.0 | style.0 & WS_TABSTOP.0;
                SetWindowLongW(hwnd, GWL_STYLE, tabs as i32);
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
        // Chrome is built before the viewport has rows; everything after goes inside it.
        let parent = if self.chrome_count == 0 {
            self.hwnd
        } else {
            self.viewport
        };
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
                Some(parent),
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

    pub(super) fn label(
        &mut self,
        text: &str,
        rect: [i32; 4],
        font: HFONT,
        style: isize,
    ) -> Result<HWND> {
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

    pub(super) fn button(
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

    /// A key cap hint such as `Win+Alt+V`, which neither clicks nor takes focus.
    pub(super) fn keycaps(&mut self, keys: &str, rect: [i32; 4]) -> Result<HWND> {
        let hwnd = self.child(
            Class::Button,
            keys,
            0,
            rect,
            WINDOW_STYLE(BS_OWNERDRAW as u32),
            self.fonts.small,
        )?;
        unsafe { crate::appearance::install_button(hwnd, pack(KEYS, 0, false))? };
        Ok(hwnd)
    }

    pub(super) fn tool(&self, message: u32, tool: HWND, text: &str) {
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

    pub(super) fn slider(&mut self, id: usize, rect: [i32; 4]) -> Result<HWND> {
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

    pub(super) fn add_control(
        &mut self,
        row: Row,
        [slider, mute, value]: [HWND; 3],
        kind: Kind,
        icon: HICON,
    ) {
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
    pub(super) fn name_box(&mut self, rect: [i32; 4], text: &str) -> Result<HWND> {
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

    pub(super) fn build_chrome(&mut self) -> Result<()> {
        // A control parent, so dialog keyboard navigation reaches the rows inside.
        self.viewport = unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                w!("mixerplusViewport"),
                None,
                WS_CHILD | WS_CLIPCHILDREN,
                0,
                0,
                0,
                0,
                Some(self.hwnd),
                None,
                None,
                None,
            )?
        };
        unsafe {
            let _ = ShowWindow(self.viewport, SW_SHOWNA);
        }
        crate::appearance::set_viewport(self.viewport);
        // Painted before the rows in captures, as on screen.
        self.children.push(self.viewport);
        // The pinned header, like the shell's output page: it stays while rows scroll.
        // Its title is regular weight, as the shell's page title is; the section
        // headings below it are semibold.
        let title = self.label("音声出力", [16, 2, 72, 36], self.fonts.body, 0)?;
        let keys = self.keycaps("Win+Alt+V", [92, 2, 140, 36])?;
        // Hidden until a view shows it, so `show_header` can skip unchanged states.
        for hwnd in [title, keys] {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        self.header_windows = vec![title, keys];
        self.header = 0;
        self.pin_button = self.button(
            "",
            PIN,
            [308, 0, 40, 32],
            self.fonts.icon,
            pack(GLYPH, PIN_GLYPH, false),
        )?;
        self.tool(TTM_ADDTOOLW, self.pin_button, "");
        self.show_pin();
        self.button(
            "その他の音量設定",
            SETTINGS_LINK,
            [8, 0, 220, 32],
            self.fonts.small,
            pack(LINK, 0, false),
        )?;
        self.chrome_count = self.children.len();
        Ok(())
    }

    /// Shows `pinned` on the footer button: a filled pin in the accent color.
    pub(super) fn show_pin(&self) {
        unsafe {
            crate::appearance::set_glyph(
                self.pin_button,
                if self.pinned { PINNED_GLYPH } else { PIN_GLYPH },
            );
            crate::appearance::set_flag(self.pin_button, self.pinned);
        }
        self.tool(
            TTM_UPDATETIPTEXTW,
            self.pin_button,
            if self.pinned {
                "最前面と自由移動を解除"
            } else {
                "最前面に表示して自由に移動"
            },
        );
    }

    /// Shows the pinned header with this logical `height`, or hides it for 0.
    pub(super) fn show_header(&mut self, height: i32) {
        if (self.header > 0) == (height > 0) {
            self.header = height;
            return;
        }
        self.header = height;
        for &hwnd in &self.header_windows {
            unsafe {
                let _ = ShowWindow(hwnd, if height > 0 { SW_SHOWNA } else { SW_HIDE });
            }
        }
    }

    /// Removes the current view's rows; chrome keeps its window identities. Rows are
    /// hidden for reuse by the next view, since creating and destroying windows
    /// dominated the cost of switching views.
    pub(super) fn clear_rows(&mut self) {
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
        self.spatial.clear();
        self.group_mutes.clear();
        self.items.clear();
    }

    /// Shows each group's mute state on its heading button.
    pub(super) fn update_group_mutes(&self) {
        for mute in &self.group_mutes {
            if let Ok((level, muted)) = mute.volume.read() {
                unsafe {
                    crate::appearance::set_glyph(
                        mute.button,
                        endpoint_glyph(mute.input, level, muted),
                    );
                    crate::appearance::set_flag(mute.button, muted);
                }
            }
        }
    }

    pub(super) fn update_control(&mut self, index: usize) {
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
    pub(super) fn tick_meters(&mut self) {
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
}
