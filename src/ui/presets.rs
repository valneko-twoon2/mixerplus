//! The preset page: rebuilding it and handling its buttons and name box.

use super::*;

impl App {
    pub(super) fn rebuild_presets(&mut self, device_name: String) -> Result<()> {
        self.clear_rows();
        crate::appearance::set_edit_frame(None);
        let (body, title, icon) = (self.fonts.body, self.fonts.title, self.fonts.icon);
        self.button(
            "\u{e72b}",
            BACK,
            [8, 8, 40, 32],
            icon,
            pack(GLYPH, 0, false),
        )?;
        self.label("プリセット", [56, 0, 280, 48], body, 0)?;
        self.label(&device_name, [16, 48, 328, 32], title, 0)?;
        let presets: Vec<_> = self.presets.all().to_vec();
        self.listed = presets.iter().map(|p| p.name.clone()).collect();
        // An edit whose preset has disappeared (renamed elsewhere, device changed) ends.
        if let Some(Some(target)) = self.edit.as_ref().map(|e| e.target.as_ref())
            && !self.listed.contains(target)
        {
            self.edit = None;
        }
        let editing = self.edit.as_ref().map(|e| e.target.clone());
        let mut focus = None;
        let mut y = 84;
        if editing == Some(None) {
            focus = Some(self.name_box(
                [8, y, 344, 40],
                &format!("プリセット {}", presets.len() + 1),
            )?);
        } else {
            self.button(
                "現在の音量を保存",
                NEW_PRESET,
                [8, y, 344, 40],
                body,
                pack(ITEM, ADD_GLYPH, false),
            )?;
        }
        y += 44;
        self.list_offset = self
            .list_offset
            .min(presets.len().saturating_sub(self.list_rows));
        if presets.is_empty() {
            self.label("プリセットがありません", [16, y, 328, 40], body, SECONDARY)?;
            y += 40;
        }
        for (i, preset) in presets
            .iter()
            .enumerate()
            .skip(self.list_offset)
            .take(self.list_rows)
        {
            let base = PRESET_ID + i * 4;
            if editing.as_ref().and_then(|e| e.as_deref()) == Some(preset.name.as_str()) {
                focus = Some(self.name_box([8, y, 244, 40], &preset.name)?);
            } else {
                let apply = self.button(
                    &preset.name,
                    base,
                    [8, y, 244, 40],
                    body,
                    pack(ITEM, PRESET_GLYPH, false),
                )?;
                self.tool(TTM_ADDTOOLW, apply, &preset.name);
            }
            let armed = self.armed_delete.as_deref() == Some(preset.name.as_str());
            for (offset, x, glyph, flag, tip) in [
                (
                    1,
                    256,
                    if preset.favorite {
                        STAR_FILLED_GLYPH
                    } else {
                        STAR_GLYPH
                    },
                    preset.favorite,
                    if preset.favorite {
                        "お気に入りを解除"
                    } else {
                        "お気に入りにする"
                    },
                ),
                (2, 288, RENAME_GLYPH, false, "名前を変更"),
                (
                    3,
                    320,
                    if armed { CONFIRM_GLYPH } else { DELETE_GLYPH },
                    armed,
                    if armed {
                        "もう一度クリックで削除"
                    } else {
                        "削除"
                    },
                ),
            ] {
                let button = self.button(
                    "",
                    base + offset,
                    [x, y + 4, 32, 32],
                    icon,
                    pack(GLYPH, glyph, flag),
                )?;
                self.tool(TTM_ADDTOOLW, button, tip);
            }
            y += 42;
        }
        if let (Some(state), Some(hwnd)) = (self.edit.as_mut(), focus) {
            state.hwnd = hwnd;
        }
        self.finish_layout(y + 8, Vec::new())?;
        if let Some(hwnd) = focus
            && unsafe { GetForegroundWindow() } == self.hwnd
        {
            unsafe {
                let _ = SetFocus(Some(hwnd));
            }
        }
        Ok(())
    }

    /// Saves the new preset or applies the rename typed in the name box.
    pub(super) fn commit_edit(&mut self) -> Result<()> {
        let Some(edit) = self.edit.take() else {
            return Ok(());
        };
        let mut text = [0u16; 256];
        let length = unsafe { GetWindowTextW(edit.hwnd, &mut text) } as usize;
        let Some(name) = sanitize_name(&String::from_utf16_lossy(&text[..length])) else {
            self.edit = Some(edit);
            unsafe {
                let _ = MessageBeep(MB_ICONWARNING);
            }
            return Ok(());
        };
        match &edit.target {
            None => {
                let preset = self.audio.capture_preset(name)?;
                self.presets.save(preset).map_err(io_error)?;
            }
            Some(old) => {
                if !self.presets.rename(old, &name).map_err(io_error)? {
                    // The name is taken; keep the box open so it can be changed.
                    self.edit = Some(edit);
                    unsafe {
                        let _ = MessageBeep(MB_ICONWARNING);
                    }
                    return Ok(());
                }
            }
        }
        crate::appearance::set_edit_frame(None);
        self.refresh(true)
    }

    pub(super) fn cancel_edit(&mut self) -> Result<()> {
        if self.edit.take().is_some() {
            crate::appearance::set_edit_frame(None);
            self.refresh(true)?;
        }
        Ok(())
    }

    pub(super) fn preset_command(&mut self, id: usize) -> Result<()> {
        let Some(name) = self.listed.get((id - PRESET_ID) / 4).cloned() else {
            return Ok(());
        };
        match (id - PRESET_ID) % 4 {
            0 => {
                self.armed_delete = None;
                let applied = match self.presets.get(&name).cloned() {
                    Some(preset) => self.audio.apply_preset(&preset),
                    None => Ok(()),
                };
                // Show the result: the mixer reflects every changed level. The view
                // change posts its own refresh, so a second one would cut the
                // transition short.
                self.show_view(View::Mixer)?;
                return applied;
            }
            1 => {
                let favorite = self.presets.get(&name).is_some_and(|p| p.favorite);
                self.presets
                    .set_favorite(&name, !favorite)
                    .map_err(io_error)?;
            }
            2 => {
                self.armed_delete = None;
                self.edit = Some(EditState {
                    hwnd: HWND::default(),
                    target: Some(name),
                });
                self.post_refresh()?;
            }
            _ => {
                if self.armed_delete.as_deref() == Some(name.as_str()) {
                    self.armed_delete = None;
                    self.presets.delete(&name).map_err(io_error)?;
                } else {
                    // Two clicks delete, so a stray click cannot lose a preset.
                    self.armed_delete = Some(name);
                    self.presets.revision += 1;
                }
            }
        }
        self.post_refresh()
    }

    pub(super) fn post_refresh(&self) -> Result<()> {
        unsafe { PostMessageW(Some(self.hwnd), REFRESH_PAGE, WPARAM(0), LPARAM(0)) }
    }
}
