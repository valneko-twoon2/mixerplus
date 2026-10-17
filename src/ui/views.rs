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

/// Logical padding above the first mixer row.
const CONTENT_TOP: i32 = 8;

impl App {
    /// Default output and input rows, one row per application, and expanded device lists.
    fn snapshot(&self) -> Result<Snapshot> {
        let mut endpoints = Vec::new();
        let mut lists = Vec::new();
        for (flow, input) in [(eRender, false), (eCapture, true)] {
            if let Ok(row) = self.audio.endpoint(flow) {
                endpoints.push((input, row));
                if self.settings.shows_devices(input) {
                    lists.push((input, self.audio.devices(flow)?));
                }
            }
        }
        Ok((endpoints, self.audio.sessions()?, lists))
    }

    pub(super) fn refresh(&mut self, force: bool) -> Result<()> {
        if self.chrome_count == 0 {
            self.build_chrome()?;
        }
        match self.view {
            View::Mixer => {
                let (endpoints, mut apps, lists) = self.snapshot()?;
                let signature: Vec<_> = endpoints
                    .iter()
                    .map(|(_, r)| r)
                    .chain(&apps)
                    .map(|r| format!("{}:{}", r.key, r.name))
                    .chain(lists.iter().flat_map(|(input, devices)| {
                        devices
                            .iter()
                            .map(move |d| format!("{input}:{}:{}", d.id, d.name))
                    }))
                    .collect();
                if !force && signature == self.signature {
                    // A new default only moves the selection in a device list.
                    let devices = lists.into_iter().flat_map(|(_, devices)| devices);
                    for (device, (item, old)) in devices.zip(&mut self.devices) {
                        unsafe { crate::appearance::set_flag(*item, device.selected) };
                        old.selected = device.selected;
                    }
                    // Refresh COM handles as sessions can be replaced without changing their key.
                    let mut rows: Vec<_> = endpoints.into_iter().map(|(_, r)| r).collect();
                    rows.append(&mut apps);
                    for i in 0..self.controls.len() {
                        // Device rows share keys with endpoint rows and keep their own handles.
                        if self.controls[i].kind != Kind::Device
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
                self.quietly(|app| app.rebuild_mixer(endpoints, apps, lists))
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

    /// Adds a scroll step whose controls `build` creates relative to the step's top.
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
                    MapWindowPoints(None, Some(self.hwnd), &mut origin);
                }
                (hwnd, origin[0])
            })
            .collect();
        self.items.push(Item {
            height,
            windows,
            divider,
        });
        Ok(())
    }

    fn rebuild_mixer(
        &mut self,
        endpoints: Vec<(bool, Row)>,
        apps: Vec<Row>,
        mut lists: Vec<(bool, Vec<Device>)>,
    ) -> Result<()> {
        // Destroying the focused row leaves no focus for keyboard navigation.
        self.clear_rows();
        let restore_focus = unsafe { GetFocus().is_invalid() };
        let (title, icon, body) = (self.fonts.title, self.fonts.icon, self.fonts.body);
        let has_endpoints = !endpoints.is_empty();
        for (input, row) in endpoints {
            let expanded = self.settings.shows_devices(input);
            self.item(44, None, |app| {
                let i = app.controls.len();
                let mute = app.button(
                    "",
                    ROW_ID + i * 3 + 1,
                    [8, 4, 40, 36],
                    icon,
                    pack(GLYPH, 0, false),
                )?;
                let slider = app.slider(ROW_ID + i * 3, [52, 6, 216, 32])?;
                let value = app.label("", [268, 4, 40, 36], body, SECONDARY)?;
                let chevron = app.button(
                    "",
                    ROW_ID + i * 3 + 2,
                    [308, 4, 40, 36],
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
                        (false, false) => "出力デバイスを表示",
                        (false, true) => "出力デバイスを隠す",
                        (true, false) => "入力デバイスを表示",
                        (true, true) => "入力デバイスを隠す",
                    },
                );
                app.add_control(
                    row,
                    [slider, mute, value],
                    Kind::Endpoint { input },
                    HICON::default(),
                );
                Ok(())
            })?;
            if let Some(index) = lists.iter().position(|(flow, _)| *flow == input) {
                let (_, devices) = lists.remove(index);
                self.device_items(devices, input)?;
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
        if unsafe { GetForegroundWindow() } != self.hwnd {
            self.refocus = None;
            return Ok(());
        }
        let chevron = self.refocus.take().and_then(|input| {
            let index = self
                .controls
                .iter()
                .position(|c| c.kind == Kind::Endpoint { input })?;
            unsafe { GetDlgItem(Some(self.hwnd), (ROW_ID + index * 3 + 2) as i32) }.ok()
        });
        let focus = chevron.or_else(|| restore_focus.then(|| self.first_visible_stop()).flatten());
        if let Some(focus) = focus {
            unsafe {
                let _ = SetFocus(Some(focus));
            }
        }
        Ok(())
    }

    /// The default-device list under an endpoint row, with each device's own volume.
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
                    [8, 0, 344, 36],
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

    /// Logical height of the items shown when `first` is the top one.
    fn shown_height(&self, first: usize) -> i32 {
        let mut height = 0;
        for item in &self.items[first..] {
            if height > 0 && height + item.height > self.content_limit {
                break;
            }
            height += item.height;
        }
        height
    }

    /// The last scroll position: from there on, every remaining item fits.
    fn max_scroll(&self) -> usize {
        let mut rest = 0;
        for (i, item) in self.items.iter().enumerate().rev() {
            rest += item.height;
            if rest > self.content_limit {
                return i + 1;
            }
        }
        0
    }

    /// Shows the items from `scroll` on that fit, hides the rest, and sizes the popup.
    fn place_items(&mut self) -> Result<()> {
        let max_scroll = self.max_scroll();
        self.scroll = self.scroll.min(max_scroll);
        // The content keeps one height while scrolling, so the popup never jumps.
        let area = (0..=max_scroll)
            .map(|first| self.shown_height(first))
            .max()
            .unwrap_or(0);
        let shown = self.shown_height(self.scroll);
        let mut dividers = Vec::new();
        let mut y = CONTENT_TOP;
        for (i, item) in self.items.iter().enumerate() {
            let visible = i >= self.scroll && y + item.height <= CONTENT_TOP + shown;
            for &(hwnd, offset) in &item.windows {
                unsafe {
                    if visible {
                        SetWindowPos(
                            hwnd,
                            None,
                            offset.x,
                            self.px(y) + offset.y,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        )?;
                    }
                    let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
                }
            }
            if visible {
                if let Some(divider) = item.divider {
                    dividers.push(self.px(y + divider));
                }
                y += item.height;
            }
        }
        let scrollbar = (max_scroll > 0).then(|| {
            let total: i32 = self.items.iter().map(|item| item.height).sum();
            let before: i32 = self.items[..self.scroll]
                .iter()
                .map(|item| item.height)
                .sum();
            let top = CONTENT_TOP + area * before / total;
            (top, top + (area * shown / total).max(16))
        });
        self.finish_layout(CONTENT_TOP + area + 8, dividers, scrollbar)
    }

    /// Scrolls the mixer by `steps` items and reports whether it moved.
    pub(super) fn scroll_by(&mut self, steps: isize) -> Result<bool> {
        let scroll = self
            .scroll
            .saturating_add_signed(steps)
            .min(self.max_scroll());
        if self.view != View::Mixer || scroll == self.scroll {
            return Ok(false);
        }
        self.end_transition();
        self.scroll = scroll;
        let visible = unsafe { IsWindowVisible(self.hwnd).as_bool() };
        if visible {
            unsafe {
                SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
            }
        }
        let result = self.place_items();
        unsafe {
            // A focused row that scrolled out of view hands focus to the first one shown.
            let focus = GetFocus();
            if !focus.is_invalid()
                && GetParent(focus).ok() == Some(self.hwnd)
                && GetWindowLongW(focus, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0
                && let Some(first) = self.first_visible_stop()
            {
                let _ = SetFocus(Some(first));
            }
            if visible {
                SendMessageW(self.hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
                let _ = RedrawWindow(
                    Some(self.hwnd),
                    None,
                    None,
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
            }
        }
        result.map(|_| true)
    }

    /// The first shown row control that Tab can reach.
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

    /// Before Tab moves focus past the shown rows, scrolls the next row into view.
    pub(super) fn reveal_tab_target(&mut self, backward: bool) -> Result<()> {
        if self.view != View::Mixer {
            return Ok(());
        }
        let item_of = |app: &Self, hwnd: HWND| {
            app.items
                .iter()
                .position(|item| item.windows.iter().any(|&(w, _)| w == hwnd))
        };
        let focus = unsafe { GetFocus() };
        let Some(current) = item_of(self, focus) else {
            return Ok(());
        };
        loop {
            let next = unsafe { GetNextDlgTabItem(self.hwnd, Some(focus), backward) }
                .ok()
                .and_then(|next| item_of(self, next));
            let stays = match next {
                Some(next) if backward => next <= current,
                Some(next) => next >= current,
                None => false,
            };
            // Wrapping around or stepping to the footer would leave the rows; scroll instead.
            if stays || !self.scroll_by(if backward { -1 } else { 1 })? {
                return Ok(());
            }
        }
    }

    /// Places the footer at `footer` and anchors the popup above the taskbar. A
    /// scroll indicator spans `scrollbar`, a logical top and bottom.
    pub(super) fn finish_layout(
        &mut self,
        footer: i32,
        dividers: Vec<i32>,
        scrollbar: Option<(i32, i32)>,
    ) -> Result<()> {
        let scrollbar = scrollbar.map(|(top, bottom)| RECT {
            left: self.px(crate::config::POPUP_WIDTH - 6),
            top: self.px(top),
            right: self.px(crate::config::POPUP_WIDTH - 3),
            bottom: self.px(bottom),
        });
        crate::appearance::set_layout(self.px(footer), dividers, scrollbar);
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
        self.pending = self.capture_current(if view == View::Mixer { -1 } else { 1 });
        self.view = view;
        self.edit = None;
        self.armed_delete = None;
        crate::appearance::set_edit_frame(None);
        self.list_offset = 0;
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }
}
