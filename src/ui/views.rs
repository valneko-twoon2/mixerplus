//! Building and refreshing the mixer and device views, and laying out the popup.

use super::*;

/// Glyphs for `EndpointFormFactor` values, as in the quick settings device list.
fn device_glyph(form: u32, input: bool) -> u16 {
    match form {
        1 => 0xe7f5,
        2 | 7 | 8 => 0xe95f,
        3 => 0xe7f6,
        4 => MICROPHONE,
        5 | 6 => 0xe95b,
        9 => 0xe7f4,
        _ if input => MICROPHONE,
        _ => SPEAKER,
    }
}

impl App {
    /// Default output and input rows, then one row per application.
    fn snapshot(&self) -> Result<Snapshot> {
        let mut endpoints = Vec::new();
        for (flow, input) in [(eRender, false), (eCapture, true)] {
            if let Ok(row) = self.audio.endpoint(flow) {
                endpoints.push((input, row));
            }
        }
        Ok((endpoints, self.audio.sessions()?))
    }

    pub(super) fn refresh(&mut self, force: bool) -> Result<()> {
        if self.navigation.is_none() {
            self.build_chrome()?;
        }
        match self.view {
            View::Mixer => {
                let (endpoints, mut apps) = self.snapshot()?;
                let signature: Vec<_> = endpoints
                    .iter()
                    .map(|(_, r)| r)
                    .chain(&apps)
                    .map(|r| format!("{}:{}", r.key, r.name))
                    .collect();
                if !force && signature == self.signature {
                    // Refresh COM handles as sessions can be replaced without changing their key.
                    let mut rows: Vec<_> = endpoints.into_iter().map(|(_, r)| r).collect();
                    rows.append(&mut apps);
                    for i in 0..self.controls.len() {
                        if let Some(index) =
                            rows.iter().position(|r| r.key == self.controls[i].row.key)
                        {
                            self.controls[i].row = rows.remove(index);
                            self.update_control(i);
                        }
                    }
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_mixer(endpoints, apps))
            }
            View::Presets => {
                let (id, name) = self
                    .audio
                    .endpoint(eRender)
                    .map(|row| (row.key, row.name))
                    .unwrap_or_default();
                let signature = vec![
                    id.clone(),
                    self.presets.revision.to_string(),
                    self.device_offset.to_string(),
                ];
                if !force && signature == self.signature {
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_presets(name))
            }
            View::Devices { input } => {
                let devices = self.audio.devices(if input { eCapture } else { eRender })?;
                let signature: Vec<_> = devices
                    .iter()
                    .map(|d| format!("{}:{}:{}", d.id, d.name, d.selected))
                    .collect();
                // A new default only moves the selection; avoid rebuilding the list.
                let same_list = signature.len() == self.signature.len()
                    && devices
                        .iter()
                        .zip(&self.devices)
                        .all(|(d, (_, old))| d.id == old.id && d.name == old.name);
                if !force && same_list {
                    for (device, (item, old)) in devices.into_iter().zip(&mut self.devices) {
                        unsafe { crate::appearance::set_flag(*item, device.selected) };
                        old.selected = device.selected;
                    }
                    for i in 0..self.controls.len() {
                        self.update_control(i);
                    }
                    self.signature = signature;
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_devices(devices, input))
            }
        }
    }

    /// Rebuilds a visible popup without showing intermediate frames.
    fn quietly(&mut self, rebuild: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        let visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        if !visible {
            return rebuild(self);
        }
        unsafe {
            SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
        self.end_transition();
        let result = rebuild(self);
        // Cover the new layout with the old view before anything is painted.
        self.begin_transition();
        unsafe {
            // Re-enabling redraw sets WS_VISIBLE, so this runs only for a visible popup.
            SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
        result
    }

    fn rebuild_mixer(&mut self, endpoints: Vec<(bool, Row)>, apps: Vec<Row>) -> Result<()> {
        // Destroying the focused row leaves no focus for keyboard navigation.
        self.clear_rows();
        let restore_focus = unsafe { GetFocus().is_invalid() };
        let (title, icon) = (self.fonts.title, self.fonts.icon);
        let mut dividers = Vec::new();
        let mut y = 8;
        for (input, row) in endpoints {
            let i = self.controls.len();
            let mute = self.button(
                "",
                ROW_ID + i * 3 + 1,
                [8, y + 4, 40, 36],
                icon,
                pack(GLYPH, 0, false),
            )?;
            let slider = self.slider(ROW_ID + i * 3, [52, y + 6, 216, 32])?;
            let value = self.label("", [268, y + 4, 40, 36], self.fonts.body, SECONDARY)?;
            self.button(
                "\u{e76c}",
                ROW_ID + i * 3 + 2,
                [308, y + 4, 40, 36],
                icon,
                pack(GLYPH, 0, false),
            )?;
            self.add_control(
                row,
                [slider, mute, value],
                Kind::Endpoint { input },
                HICON::default(),
            );
            y += 44;
        }
        if !self.controls.is_empty() {
            dividers.push(self.px(y + 4));
            y += 9;
        }
        self.label("音量ミキサー", [16, y, 240, 40], title, 0)?;
        let presets_button = self.button(
            "\u{f4c3}",
            PRESETS_BUTTON,
            [264, y + 4, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.tool(TTM_ADDTOOLW, presets_button, "プリセット");
        self.button(
            "\u{e713}",
            MORE,
            [308, y + 4, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        y += 40;
        self.pages = apps.len().div_ceil(self.page_size).max(1);
        self.page = self.page.min(self.pages - 1);
        let page: Vec<_> = apps
            .into_iter()
            .skip(self.page * self.page_size)
            .take(self.page_size)
            .collect();
        if page.is_empty() {
            self.label(
                "音声を再生しているアプリはありません",
                [16, y, 328, 40],
                self.fonts.body,
                SECONDARY,
            )?;
            y += 40;
        }
        for row in page {
            let i = self.controls.len();
            let mut icon_handle = HICON::default();
            if let Some(path) = &row.icon_path {
                let path = wide(path);
                unsafe {
                    ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&mut icon_handle), None, 1);
                }
            }
            let mute = self.button(
                "",
                ROW_ID + i * 3 + 1,
                [8, y + 4, 40, 36],
                icon,
                pack(crate::appearance::APP, SPEAKER, false),
            )?;
            if !icon_handle.is_invalid() {
                unsafe { crate::appearance::set_app_icon(mute, icon_handle) };
            }
            let slider = self.slider(ROW_ID + i * 3, [52, y + 6, 256, 32])?;
            let value = self.label("", [308, y + 4, 40, 36], self.fonts.body, SECONDARY)?;
            self.add_control(row, [slider, mute, value], Kind::App, icon_handle);
            y += 44;
        }
        let pager = self.pages > 1;
        if pager {
            y += 4;
        }
        self.place_navigation(pager.then_some(y));
        if pager {
            y += 36;
        }
        self.finish_layout(y + 8, dividers)?;
        if restore_focus
            && unsafe { GetForegroundWindow() } == self.hwnd
            && let Some(control) = self.controls.first()
        {
            unsafe {
                let _ = SetFocus(Some(control.slider));
            }
        }
        Ok(())
    }

    fn rebuild_devices(&mut self, devices: Vec<Device>, input: bool) -> Result<()> {
        self.clear_rows();
        let (body, title, icon) = (self.fonts.body, self.fonts.title, self.fonts.icon);
        self.button(
            "\u{e72b}",
            BACK,
            [8, 8, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.label(
            if input {
                "音声入力"
            } else {
                "音声出力"
            },
            [56, 0, 280, 48],
            body,
            0,
        )?;
        self.label(
            if input {
                "入力デバイス"
            } else {
                "出力デバイス"
            },
            [16, 48, 328, 32],
            title,
            0,
        )?;
        let mut y = 84;
        self.device_offset = self
            .device_offset
            .min(devices.len().saturating_sub(self.device_rows));
        if devices.is_empty() {
            self.label("デバイスがありません", [16, y, 328, 40], body, SECONDARY)?;
            y += 40;
        }
        for (i, device) in devices
            .into_iter()
            .enumerate()
            .skip(self.device_offset)
            .take(self.device_rows)
        {
            let item = self.button(
                &device.name,
                DEVICE_ID + i,
                [8, y, 344, 36],
                body,
                pack(ITEM, device_glyph(device.form, input), device.selected),
            )?;
            // Each device keeps its own volume, independent of the default device.
            if let Some(volume) = device.volume.clone() {
                let index = self.controls.len();
                let slider = self.slider(ROW_ID + index * 3, [40, y + 36, 264, 28])?;
                let value = self.label("", [304, y + 34, 44, 32], body, SECONDARY)?;
                let row = Row {
                    key: device.id.clone(),
                    name: device.name.clone(),
                    volume,
                    icon_path: None,
                    meters: device.meter.clone().into_iter().collect(),
                    process: String::new(),
                };
                self.add_control(row, [slider, item, value], Kind::Device, HICON::default());
            }
            self.devices.push((item, device));
            y += 68;
        }
        self.place_navigation(None);
        self.finish_layout(y + 8, Vec::new())
    }

    /// Places the footer at `footer` and anchors the popup above the taskbar.
    pub(super) fn finish_layout(&mut self, footer: i32, dividers: Vec<i32>) -> Result<()> {
        crate::appearance::set_layout(self.px(footer), dividers);
        unsafe {
            if let Some(link) = self.children.get(self.chrome_count.saturating_sub(1)) {
                SetWindowPos(
                    *link,
                    None,
                    self.px(8),
                    self.px(footer + 8),
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                )?;
            }
            let width = self.px(crate::config::POPUP_WIDTH);
            let height = self.px(footer + 48);
            let (x, y) = popup_position(self.work_area, self.monitor, width, height, self.px(12));
            let (x, y) = match &mut self.animation {
                // Keep sliding; only the destination moves with the new height.
                Some(animation) if !animation.closing => {
                    animation.to = y;
                    animation.x = x;
                    let mut rect = RECT::default();
                    GetWindowRect(self.hwnd, &mut rect)?;
                    (x, rect.top)
                }
                _ => (x, y),
            };
            if self.pending.is_some() {
                // A view transition will resize the window itself.
                self.target = Some(RECT {
                    left: x,
                    top: y,
                    right: x + width,
                    bottom: y + height,
                });
                return Ok(());
            }
            SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
        Ok(())
    }

    /// Changes what the popup shows once the clicked button's notification unwinds.
    pub(super) fn show_view(&mut self, view: View) -> Result<()> {
        self.pending = self.capture_current(view != View::Mixer);
        self.view = view;
        self.edit = None;
        self.armed_delete = None;
        crate::appearance::set_edit_frame(None);
        self.device_offset = 0;
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }
}
