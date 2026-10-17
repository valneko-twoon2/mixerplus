//! Building, refreshing, and scrolling the mixer view, and laying out the popup.

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
    /// One row per application, the output and input devices (only the default one
    /// of a collapsed group), and the output's spatial sound when its group is expanded.
    fn snapshot(&self) -> Result<MixerState> {
        let mut state = MixerState {
            apps: self.audio.sessions()?,
            lists: Vec::new(),
            spatial: None,
        };
        for (flow, input) in [(eRender, false), (eCapture, true)] {
            let mut devices = self.audio.devices(flow)?;
            if devices.is_empty() {
                continue;
            }
            if !self.settings.shows_devices(input) {
                devices.retain(|device| device.selected);
            } else if !input && let Some(default) = devices.iter().find(|d| d.selected) {
                state.spatial = self.audio.spatial(&default.id);
            }
            state.lists.push((input, devices));
        }
        Ok(state)
    }

    pub(super) fn refresh(&mut self, force: bool) -> Result<()> {
        if self.chrome_count == 0 {
            self.build_chrome()?;
        }
        match self.view {
            View::Mixer => {
                let mut state = self.snapshot()?;
                let signature: Vec<_> = state
                    .apps
                    .iter()
                    .map(|r| format!("{}:{}", r.key, r.name))
                    .chain(state.lists.iter().flat_map(|(input, devices)| {
                        devices
                            .iter()
                            .map(move |d| format!("{input}:{}:{}", d.id, d.name))
                    }))
                    .chain(state.spatial.iter().flat_map(|spatial| {
                        spatial.formats.iter().map(|(f, _)| format!("spatial:{f}"))
                    }))
                    .collect();
                if !force && signature == self.signature {
                    // A new default only moves the selection in a device list, and the
                    // group mute buttons follow it.
                    for (input, devices) in &state.lists {
                        if let Some(volume) = devices
                            .iter()
                            .find(|d| d.selected)
                            .and_then(|d| d.volume.clone())
                            && let Some(mute) =
                                self.group_mutes.iter_mut().find(|m| m.input == *input)
                        {
                            mute.volume = volume;
                        }
                    }
                    self.update_group_mutes();
                    // A new default only moves the selection in a device list.
                    let devices = state.lists.into_iter().flat_map(|(_, devices)| devices);
                    for (device, (item, old)) in devices.zip(&mut self.devices) {
                        unsafe { crate::appearance::set_flag(*item, device.selected) };
                        old.selected = device.selected;
                        // An input's meter comes and goes as applications record.
                        if let Some(control) = self
                            .controls
                            .iter_mut()
                            .find(|c| c.kind != Kind::App && c.row.key == device.id)
                        {
                            control.row.meters = device.meter.clone().into_iter().collect();
                        }
                    }
                    if let Some(spatial) = &state.spatial {
                        for (button, format) in &self.spatial {
                            let selected = format.eq_ignore_ascii_case(&spatial.current);
                            unsafe { crate::appearance::set_flag(*button, selected) };
                        }
                    }
                    // Refresh COM handles as sessions can be replaced without changing their key.
                    let rows = &mut state.apps;
                    for i in 0..self.controls.len() {
                        // Device rows keep their endpoint handles.
                        if self.controls[i].kind == Kind::App
                            && let Some(index) =
                                rows.iter().position(|r| r.key == self.controls[i].row.key)
                        {
                            self.controls[i].row = rows.remove(index);
                        }
                        self.update_control(i);
                    }
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_mixer(state))
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
                    self.list_offset.to_string(),
                ];
                if !force && signature == self.signature {
                    return Ok(());
                }
                self.signature = signature;
                self.quietly(|app| app.rebuild_presets(name))
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

    /// Adds a mixer item whose controls `build` creates relative to the item's top.
    fn item(
        &mut self,
        height: i32,
        divider: Option<i32>,
        build: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<()> {
        let first = self.children.len();
        build(self)?;
        let windows = self.children[first..]
            .iter()
            .map(|&hwnd| {
                let mut rect = RECT::default();
                let mut origin = [POINT::default()];
                unsafe {
                    let _ = GetWindowRect(hwnd, &mut rect);
                    origin[0] = POINT {
                        x: rect.left,
                        y: rect.top,
                    };
                    MapWindowPoints(None, Some(self.viewport), &mut origin);
                }
                (hwnd, origin[0])
            })
            .collect();
        self.items.push(Item {
            top: 0,
            height,
            windows,
            divider,
        });
        Ok(())
    }

    fn rebuild_mixer(&mut self, state: MixerState) -> Result<()> {
        let MixerState {
            apps,
            lists,
            mut spatial,
        } = state;
        // Destroying the focused row leaves no focus for keyboard navigation.
        self.clear_rows();
        let restore_focus = unsafe { GetFocus().is_invalid() };
        let (title, icon, body) = (self.fonts.title, self.fonts.icon, self.fonts.body);
        let has_endpoints = !lists.is_empty();
        for (index, (input, devices)) in lists.into_iter().enumerate() {
            // The input group follows the output group below a divider.
            let default = devices.iter().find(|d| d.selected);
            self.group_heading(input, index > 0, default.and_then(|d| d.volume.clone()))?;
            if !input && let Some(default) = devices.iter().find(|d| d.selected) {
                self.spatial_device = default.id.clone();
            }
            self.device_items(devices, input)?;
            if !input && let Some(spatial) = spatial.take() {
                self.spatial_items(spatial)?;
            }
        }
        let (header_height, offset) = if has_endpoints { (49, 9) } else { (40, 0) };
        self.item(header_height, has_endpoints.then_some(4), |app| {
            app.label("音量ミキサー", [16, offset, 240, 40], title, 0)?;
            let presets_button = app.button(
                "\u{f4c3}",
                PRESETS_BUTTON,
                [264, offset + 4, 40, 32],
                icon,
                pack(GLYPH, 0, false),
            )?;
            app.tool(TTM_ADDTOOLW, presets_button, "プリセット");
            app.button(
                "\u{e713}",
                MORE,
                [308, offset + 4, 40, 32],
                icon,
                pack(GLYPH, 0, false),
            )?;
            Ok(())
        })?;
        if apps.is_empty() {
            self.item(40, None, |app| {
                app.label(
                    "音声を再生しているアプリはありません",
                    [16, 0, 328, 40],
                    body,
                    SECONDARY,
                )
                .map(|_| ())
            })?;
        }
        for row in apps {
            self.item(44, None, |app| {
                let i = app.controls.len();
                let mut icon_handle = HICON::default();
                if let Some(path) = &row.icon_path {
                    let path = wide(path);
                    unsafe {
                        ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&mut icon_handle), None, 1);
                    }
                }
                let mute = app.button(
                    "",
                    ROW_ID + i * 3 + 1,
                    [8, 4, 40, 36],
                    icon,
                    pack(crate::appearance::APP, SPEAKER, false),
                )?;
                if !icon_handle.is_invalid() {
                    unsafe { crate::appearance::set_app_icon(mute, icon_handle) };
                }
                let slider = app.slider(ROW_ID + i * 3, [52, 6, 256, 32])?;
                let value = app.label("", [308, 4, 40, 36], body, SECONDARY)?;
                app.add_control(row, [slider, mute, value], Kind::App, icon_handle);
                Ok(())
            })?;
        }
        self.place_items()?;
        self.update_group_mutes();
        if unsafe { GetForegroundWindow() } != self.hwnd {
            self.refocus = None;
            return Ok(());
        }
        let chevron = self.refocus.take().and_then(|input| {
            let id = if input { INPUT_CHEVRON } else { OUTPUT_CHEVRON };
            unsafe { GetDlgItem(Some(self.viewport), id as i32) }.ok()
        });
        let focus = chevron.or_else(|| restore_focus.then(|| self.first_visible_stop()).flatten());
        if let Some(focus) = focus {
            unsafe {
                let _ = SetFocus(Some(focus));
            }
        }
        Ok(())
    }

    /// A section title.
    fn heading(&mut self, text: &str) -> Result<()> {
        let title = self.fonts.title;
        self.item(38, None, |app| {
            app.label(text, [16, 2, 328, 36], title, 0).map(|_| ())
        })
    }

    /// The title of the output or input group, with the chevron that expands it from
    /// the default device to every device. Below a divider when `divided`.
    fn group_heading(&mut self, input: bool, divided: bool, volume: Option<Volume>) -> Result<()> {
        let (title, icon) = (self.fonts.title, self.fonts.icon);
        let expanded = self.settings.shows_devices(input);
        let top = if divided { 9 } else { 0 };
        self.item(top + 40, divided.then_some(4), |app| {
            let text = if input {
                "入力デバイス"
            } else {
                "出力デバイス"
            };
            app.label(text, [16, top + 2, 240, 36], title, 0)?;
            if let Some(volume) = volume {
                let button = app.button(
                    "",
                    if input { INPUT_MUTE } else { OUTPUT_MUTE },
                    [264, top + 2, 40, 36],
                    icon,
                    pack(GLYPH, 0, false),
                )?;
                app.tool(
                    TTM_ADDTOOLW,
                    button,
                    if input {
                        "入力をミュート／解除"
                    } else {
                        "出力をミュート／解除"
                    },
                );
                app.group_mutes.push(GroupMute {
                    input,
                    button,
                    volume,
                });
            }
            let chevron = app.button(
                "",
                if input { INPUT_CHEVRON } else { OUTPUT_CHEVRON },
                [308, top + 2, 40, 36],
                icon,
                pack(
                    GLYPH,
                    if expanded { CHEVRON_UP } else { CHEVRON_DOWN },
                    false,
                ),
            )?;
            app.tool(
                TTM_ADDTOOLW,
                chevron,
                match (input, expanded) {
                    (false, false) => "すべての出力デバイスを表示",
                    (false, true) => "既定の出力デバイスだけを表示",
                    (true, false) => "すべての入力デバイスを表示",
                    (true, true) => "既定の入力デバイスだけを表示",
                },
            );
            Ok(())
        })
    }

    /// The output's spatial sound formats, as in the quick settings output page.
    fn spatial_items(&mut self, spatial: Spatial) -> Result<()> {
        let body = self.fonts.body;
        self.heading("立体音響")?;
        for (format, name) in spatial.formats {
            let selected = format.eq_ignore_ascii_case(&spatial.current);
            let caption = match name {
                _ if format == SPATIAL_OFF => "オフ",
                "?" => "その他の形式",
                name => name,
            };
            self.item(40, None, |app| {
                let button = app.button(
                    caption,
                    SPATIAL_ID + app.spatial.len(),
                    [8, 2, 340, 36],
                    body,
                    pack(ITEM, 0, selected),
                )?;
                app.spatial.push((button, format));
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Device rows: an item that makes the device the default, and its own volume.
    fn device_items(&mut self, devices: Vec<Device>, input: bool) -> Result<()> {
        let body = self.fonts.body;
        if devices.is_empty() {
            return self.item(40, None, |app| {
                app.label("デバイスがありません", [16, 0, 328, 40], body, SECONDARY)
                    .map(|_| ())
            });
        }
        for device in devices {
            let height = if device.volume.is_some() { 68 } else { 40 };
            self.item(height, None, |app| {
                let item = app.button(
                    &device.name,
                    DEVICE_ID + app.devices.len(),
                    [8, 0, 340, 36],
                    body,
                    pack(ITEM, device_glyph(device.form, input), device.selected),
                )?;
                // Each device keeps its own volume, independent of the default device.
                if let Some(volume) = device.volume.clone() {
                    let index = app.controls.len();
                    let slider = app.slider(ROW_ID + index * 3, [40, 36, 264, 28])?;
                    let value = app.label("", [304, 34, 44, 32], body, SECONDARY)?;
                    let row = Row {
                        key: device.id.clone(),
                        name: device.name.clone(),
                        volume,
                        icon_path: None,
                        meters: device.meter.clone().into_iter().collect(),
                        process: String::new(),
                    };
                    app.add_control(row, [slider, item, value], Kind::Device, HICON::default());
                }
                app.devices.push((item, device));
                Ok(())
            })?;
        }
        Ok(())
    }

    /// Places the footer at `footer` and anchors the popup above the taskbar.
    pub(super) fn finish_layout(&mut self, footer: i32, dividers: Vec<i32>) -> Result<()> {
        crate::appearance::set_layout(self.px(footer), dividers);
        unsafe {
            // Rows scroll and clip inside the viewport; the scroll bar strip stays outside.
            SetWindowPos(
                self.viewport,
                None,
                0,
                0,
                self.px(crate::config::POPUP_WIDTH - SCROLLBAR_WIDTH),
                self.px(footer),
                SWP_NOZORDER | SWP_NOACTIVATE,
            )?;
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
        self.pending = self.capture_current(if view == View::Mixer { -1 } else { 1 });
        self.view = view;
        self.edit = None;
        self.armed_delete = None;
        crate::appearance::set_edit_frame(None);
        self.list_offset = 0;
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }
}
