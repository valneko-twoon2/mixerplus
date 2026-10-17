//! The popup's window procedure and its command handling.

use super::*;

pub(super) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_PRINTCLIENT => {
                crate::appearance::background(hwnd, HDC(wparam.0 as *mut _));
                return LRESULT(0);
            }
            WM_PAINT => {
                crate::appearance::paint(hwnd);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            SHOW_ABOUT => {
                if crate::about::show(wparam.0 == 1).is_err() {
                    show_error("ウィンドウを開けませんでした。");
                }
                return LRESULT(0);
            }
            WM_DRAWITEM if lparam.0 != 0 => {
                crate::appearance::draw_item(&*(lparam.0 as *const DRAWITEMSTRUCT));
                return LRESULT(1);
            }
            WM_CTLCOLOREDIT => {
                return crate::appearance::edit_colors(HDC(wparam.0 as *mut _));
            }
            WM_CTLCOLORSTATIC => {
                return crate::appearance::text_background(HDC(wparam.0 as *mut _));
            }
            _ => {}
        }
        // Child creation and native menus can dispatch nested messages.
        let handled = APP.with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return None;
            };
            let app = state.as_mut()?;
            let result = match message {
                TRAY if lparam.0 as u32 == WM_RBUTTONUP => app.tray_menu(),
                TRAY if lparam.0 as u32 == WM_LBUTTONUP => {
                    // A click on the icon first deactivates an open popup; treat it as a toggle.
                    if app
                        .dismissed
                        .is_some_and(|time| time.elapsed().as_millis() < 400)
                    {
                        app.dismissed = None;
                        Ok(())
                    } else {
                        app.open()
                    }
                }
                WM_COMMAND if wparam.0 >> 16 == BN_CLICKED as usize && lparam.0 != 0 => {
                    let id = wparam.0 & 0xffff;
                    let sender = HWND(lparam.0 as *mut _);
                    if GetDlgCtrlID(sender) as usize == id {
                        app.clicked = sender;
                        app.command(id)
                    } else {
                        Ok(())
                    }
                }
                REFRESH_PAGE => app.refresh(true),
                TAB_KEY => {
                    app.reveal_focus();
                    Ok(())
                }
                REFRESH_STATE => app.refresh(false),
                COMMIT_EDIT => app.commit_edit(),
                DISMISS => {
                    if app.edit.is_some() {
                        app.cancel_edit()
                    } else if app.view == View::Mixer {
                        app.dismiss();
                        Ok(())
                    } else {
                        app.show_view(View::Mixer)
                    }
                }
                WM_MOUSEWHEEL => app.wheel((wparam.0 >> 16) as i16),
                WM_LBUTTONDOWN => app.track_press(lparam),
                WM_MOUSEMOVE => app.track_move(lparam),
                WM_LBUTTONUP => {
                    app.track_release(app.track_hot);
                    Ok(())
                }
                WM_CAPTURECHANGED => {
                    // Capture taken away by another window ends the drag without releasing.
                    app.drag = None;
                    app.track_release(app.track_hot);
                    Ok(())
                }
                WM_MOUSELEAVE if app.drag.is_none() => {
                    app.track_release(false);
                    Ok(())
                }
                WM_HSCROLL => {
                    if let Some(index) = app
                        .controls
                        .iter()
                        .position(|c| c.slider.0 as isize == lparam.0)
                    {
                        let control = &app.controls[index];
                        let level = SendMessageW(
                            control.slider,
                            TBM_GETPOS,
                            Some(WPARAM(0)),
                            Some(LPARAM(0)),
                        )
                        .0 as i32;
                        control
                            .row
                            .volume
                            .set_level(level)
                            .map(|_| app.update_control(index))
                    } else {
                        Ok(())
                    }
                }
                WM_TIMER if wparam.0 == ANIMATION_TIMER => {
                    app.animate();
                    Ok(())
                }
                SCROLL_FRAME => {
                    app.scroll_frame();
                    Ok(())
                }
                WM_TIMER if wparam.0 == TRANSITION_TIMER => {
                    app.transition_step();
                    Ok(())
                }
                WM_TIMER if wparam.0 == METER_TIMER => {
                    if IsWindowVisible(hwnd).as_bool() {
                        app.tick_meters();
                    } else {
                        let _ = KillTimer(Some(hwnd), METER_TIMER);
                    }
                    Ok(())
                }
                WM_TIMER if wparam.0 == TRAY_TIMER => {
                    app.maintain_tray();
                    Ok(())
                }
                WM_TIMER
                    if wparam.0 == REFRESH_TIMER
                        && IsWindowVisible(hwnd).as_bool()
                        && app.animation.is_none()
                        && app.transition.is_none()
                        && GetCapture().is_invalid() =>
                {
                    app.refresh(false)
                }
                WM_ACTIVATE if wparam.0 & 0xffff == WA_INACTIVE as usize => {
                    app.dismiss();
                    Ok(())
                }
                WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
                    crate::appearance::load_theme();
                    crate::appearance::style_window(hwnd);
                    // Match the tray icon to the new taskbar color.
                    if let Ok(icon) = tray_icon() {
                        let previous = std::mem::replace(&mut app.icon, icon);
                        let _ = Shell_NotifyIconW(NIM_MODIFY, &app.tray_data());
                        let _ = DestroyIcon(previous);
                    }
                    let _ = RedrawWindow(
                        Some(hwnd),
                        None,
                        None,
                        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                    );
                    Ok(())
                }
                WM_CLOSE => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    PostQuitMessage(0);
                    Ok(())
                }
                m if m == app.taskbar_created => {
                    app.tray_added = false;
                    app.maintain_tray();
                    Ok(())
                }
                _ => return None,
            };
            if let Err(error) = result {
                let _ = ShowWindow(hwnd, SW_HIDE);
                show_error(&format!(
                    "The audio operation failed. Reopen mixerplus to refresh devices.\n\n{error}"
                ));
            }
            Some(LRESULT(0))
        });
        handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam))
    }
}

impl App {
    fn command(&mut self, id: usize) -> Result<()> {
        match id {
            BACK => self.show_view(View::Mixer)?,
            OUTPUT_CHEVRON | INPUT_CHEVRON => self.toggle_devices(id == INPUT_CHEVRON)?,
            PRESETS_BUTTON => self.show_view(View::Presets)?,
            NEW_PRESET => {
                self.edit = Some(EditState {
                    hwnd: HWND::default(),
                    target: None,
                });
                self.post_refresh()?;
            }
            id if id >= PRESET_ID => self.preset_command(id)?,
            SETTINGS_LINK => {
                self.dismiss();
                self.open_settings(w!("ms-settings:sound"))?;
            }
            MORE => {
                // Like the shell flyout, the gear opens the volume mixer settings.
                self.dismiss();
                self.open_settings(w!("ms-settings:apps-volume"))?;
            }
            id if id >= SPATIAL_ID => {
                if let Some((_, format)) = self
                    .spatial
                    .iter()
                    .find(|(button, _)| unsafe { GetDlgCtrlID(*button) as usize == id })
                {
                    let result = self.audio.set_spatial(&self.spatial_device, format);
                    // Refresh even after a failure so the list shows the actual format.
                    unsafe {
                        PostMessageW(Some(self.hwnd), REFRESH_STATE, WPARAM(0), LPARAM(0))?;
                    }
                    result?;
                }
            }
            id if id >= DEVICE_ID => {
                if let Some((_, device)) = self
                    .devices
                    .iter()
                    .find(|(item, _)| unsafe { GetDlgCtrlID(*item) as usize == id })
                    .filter(|(_, device)| !device.selected)
                {
                    let result = self.audio.set_default(&device.id);
                    // Refresh even after a failure so the list shows the actual default.
                    unsafe {
                        PostMessageW(Some(self.hwnd), REFRESH_STATE, WPARAM(0), LPARAM(0))?;
                    }
                    result?;
                }
            }
            id if id >= ROW_ID => {
                let index = (id - ROW_ID) / 3;
                match (id - ROW_ID) % 3 {
                    1 if self
                        .controls
                        .get(index)
                        .is_some_and(|c| c.kind == Kind::App) =>
                    {
                        self.controls[index].row.volume.toggle_mute()?;
                        self.update_control(index);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Expands or collapses the device list under an endpoint row and remembers it.
    fn toggle_devices(&mut self, input: bool) -> Result<()> {
        self.settings.toggle_devices(input);
        if let Some(path) = &self.settings_path {
            // An unsaved preference only lasts until exit; it is not worth an error.
            let _ = self.settings.save(path);
        }
        self.pending = self.capture_current(0);
        self.refocus = Some(input);
        // Rebuild rows after the native button notification has unwound.
        self.post_refresh()
    }

    /// Scrolls the mixer rows, or the preset list one row per wheel notch.
    fn wheel(&mut self, delta: i16) -> Result<()> {
        if self.view == View::Mixer {
            self.wheel_scroll(delta);
            return Ok(());
        }
        self.wheel_rest += delta as i32;
        // Positive deltas scroll toward the top.
        let steps = -(self.wheel_rest / WHEEL_DELTA as i32) as isize;
        self.wheel_rest %= WHEEL_DELTA as i32;
        if steps == 0 {
            return Ok(());
        }
        let offset = self.list_offset.saturating_add_signed(steps);
        if offset != self.list_offset {
            self.list_offset = offset;
            return self.refresh(true);
        }
        Ok(())
    }

    fn open_settings(&self, uri: PCWSTR) -> Result<()> {
        unsafe {
            let result = ShellExecuteW(Some(self.hwnd), w!("open"), uri, None, None, SW_SHOWNORMAL);
            if result.0 as usize <= 32 {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                    0x80004005u32 as i32,
                )));
            }
        }
        Ok(())
    }
}
