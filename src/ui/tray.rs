//! The notification icon: adding it to Explorer, promoting it on Windows 11, and its context menu.

use super::*;
use windows::{
    Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
    core::PCSTR,
};

/// The icon's tooltip, and the tray menu's first entry, which opens the popup.
const TRAY_TITLE: &str = "mixerplus — 音量ミキサー";

/// Moves mixerplus out of the Windows 11 overflow area the first time Explorer records it.
/// Returns true once its entry exists; an existing IsPromoted value is the user's choice.
fn promote_tray_icon() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return true;
    };
    let executable = executable.to_string_lossy().to_lowercase();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\NotifyIconSettings"),
            None,
            KEY_READ | KEY_SET_VALUE,
            &mut root,
        )
        .is_err()
        {
            // Windows 10 has no per-icon promotion key.
            return true;
        }
        let mut found = false;
        let mut name = [0u16; 256];
        for index in 0.. {
            let mut length = name.len() as u32;
            if RegEnumKeyExW(
                root,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
            .is_err()
            {
                break;
            }
            let entry = PCWSTR(name.as_ptr());
            let path = registry_string(root, entry, w!("ExecutablePath"));
            if path.is_none_or(|path| expand_known_folder(&path).to_lowercase() != executable) {
                continue;
            }
            found = true;
            if registry_string(root, entry, w!("IsPromoted")).is_none() {
                let promoted = 1u32;
                let _ = RegSetKeyValueW(
                    root,
                    entry,
                    w!("IsPromoted"),
                    REG_DWORD.0,
                    Some((&promoted as *const u32).cast()),
                    size_of::<u32>() as u32,
                );
            }
        }
        let _ = RegCloseKey(root);
        found
    }
}

/// Reads a value as text; DWORD values are only checked for presence.
unsafe fn registry_string(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = size_of_val(&buffer) as u32;
    let mut kind = REG_VALUE_TYPE::default();
    unsafe {
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_SZ | RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    if kind != REG_SZ {
        return Some(String::new());
    }
    let length = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Explorer stores paths under known folders as `{FOLDERID}\relative\path`.
fn expand_known_folder(path: &str) -> String {
    let Some((folder, rest)) = path.split_once('\\') else {
        return path.to_owned();
    };
    let Ok(id) = GUID::try_from(folder.trim_start_matches('{').trim_end_matches('}')) else {
        return path.to_owned();
    };
    unsafe {
        let Ok(base) = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) else {
            return path.to_owned();
        };
        let expanded = base.to_string().map(|base| format!("{base}\\{rest}"));
        CoTaskMemFree(Some(base.0.cast()));
        expanded.unwrap_or_else(|_| path.to_owned())
    }
}

/// mixerplus's generated tray icon, or the stock audio icon if rendering fails.
pub(super) fn tray_icon() -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if let Some(icon) = crate::appearance::tray_icon(size) {
        return Ok(icon);
    }
    let mut stock = SHSTOCKICONINFO {
        cbSize: size_of::<SHSTOCKICONINFO>() as u32,
        ..Default::default()
    };
    unsafe { SHGetStockIconInfo(SIID_AUDIOFILES, SHGSI_ICON | SHGSI_SMALLICON, &mut stock)? };
    Ok(stock.hIcon)
}

impl App {
    pub(super) fn tray_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip = wide(TRAY_TITLE);
        // Keep the terminating NUL when a longer tip is ever configured.
        let length = tip.len().min(data.szTip.len());
        data.szTip[..length].copy_from_slice(&tip[..length]);
        if let Some(last) = data.szTip.last_mut() {
            *last = 0;
        }
        data
    }

    /// Adds the tray icon and promotes it once; retries while Explorer is not ready.
    pub(super) fn maintain_tray(&mut self) {
        unsafe {
            if !self.tray_added {
                // NIM_ADD can time out after succeeding, so probe with NIM_MODIFY first.
                self.tray_added = Shell_NotifyIconW(NIM_MODIFY, &self.tray_data()).as_bool()
                    || Shell_NotifyIconW(NIM_ADD, &self.tray_data()).as_bool();
            }
            if self.tray_added && self.promotion_attempts < PROMOTION_ATTEMPTS {
                self.promotion_attempts = if promote_tray_icon() {
                    PROMOTION_ATTEMPTS
                } else {
                    self.promotion_attempts + 1
                };
            }
            if self.tray_added && self.promotion_attempts >= PROMOTION_ATTEMPTS {
                let _ = KillTimer(Some(self.hwnd), TRAY_TIMER);
            } else {
                SetTimer(Some(self.hwnd), TRAY_TIMER, TRAY_RETRY_MS, None);
            }
        }
    }

    /// Lets menus follow the system dark mode. Menus that carry item bitmaps are drawn
    /// in the light theme otherwise. Both calls are unexported `uxtheme.dll` ordinals
    /// (135 `SetPreferredAppMode`, 136 `FlushMenuThemes`); a missing one changes nothing.
    fn follow_dark_mode() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let Ok(module) = LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
            else {
                return;
            };
            let set = GetProcAddress(module, PCSTR(135 as *const u8));
            let flush = GetProcAddress(module, PCSTR(136 as *const u8));
            if let (Some(set), Some(flush)) = (set, flush) {
                // 1 is AllowDark: follow the system setting rather than force a mode.
                let set: extern "system" fn(i32) -> i32 = std::mem::transmute(set);
                let flush: extern "system" fn() = std::mem::transmute(flush);
                set(1);
                flush();
            }
        });
    }

    /// Shows a menu of (id, text, icon glyph, icon color) entries at the cursor and
    /// returns the chosen id, or 0. Icons fill the menu's icon column like the shell's.
    fn menu(&self, entries: &[(usize, &str, u16, COLORREF)]) -> Result<usize> {
        Self::follow_dark_mode();
        unsafe {
            let menu = CreatePopupMenu()?;
            let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(self.hwnd).max(96));
            let mut icons = Vec::new();
            let result = (|| {
                for &(id, name, glyph, color) in entries {
                    let mut name = wide(name);
                    name.push(0);
                    let icon = crate::appearance::menu_icon(size, glyph, color);
                    icons.extend(icon);
                    let item = MENUITEMINFOW {
                        cbSize: size_of::<MENUITEMINFOW>() as u32,
                        fMask: MIIM_ID | MIIM_STRING | MIIM_BITMAP,
                        wID: id as u32,
                        dwTypeData: PWSTR(name.as_mut_ptr()),
                        hbmpItem: icon.unwrap_or_default(),
                        ..Default::default()
                    };
                    InsertMenuItemW(menu, u32::MAX, true, &item)?;
                }
                let mut point = POINT::default();
                GetCursorPos(&mut point)?;
                let _ = SetForegroundWindow(self.hwnd);
                // The cursor is on the taskbar, so anchor the menu to the edge of the work
                // area beside it, on whichever side the taskbar is, as the shell's menus are.
                let mut monitor = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let mut align = TPM_RIGHTALIGN | TPM_BOTTOMALIGN;
                if GetMonitorInfoW(
                    MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                )
                .as_bool()
                {
                    let work = monitor.rcWork;
                    if point.y < work.top {
                        align = (align & !TPM_BOTTOMALIGN) | TPM_TOPALIGN;
                    }
                    if point.x < work.left {
                        align = (align & !TPM_RIGHTALIGN) | TPM_LEFTALIGN;
                    }
                    // The popup keeps the same 12 px from the taskbar.
                    let margin = 12 * GetDpiForWindow(self.hwnd).max(96) as i32 / 96;
                    point.x = point.x.clamp(work.left + margin, work.right - margin);
                    point.y = point.y.clamp(work.top + margin, work.bottom - margin);
                }
                let chosen = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | align,
                    point.x,
                    point.y,
                    None,
                    self.hwnd,
                    None,
                )
                .0 as usize;
                // Lets a menu opened for a tray icon close when the user clicks away.
                let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                Ok(chosen)
            })();
            let _ = DestroyMenu(menu);
            // The menu does not own its item bitmaps.
            for icon in icons {
                let _ = DeleteObject(icon.into());
            }
            result
        }
    }

    /// The tray icon's context menu.
    pub(super) fn tray_menu(&mut self) -> Result<()> {
        // The menu runs its own message loop, which would stall a sliding popup.
        self.hide_now();
        let palette = crate::appearance::palette();
        let entries = [
            // The speaker is the tray icon's glyph, in its accent color.
            (4, TRAY_TITLE, 0xe995, palette.accent),
            (2, "About", 0xe946, palette.text),
            (1, "Exit", 0xe7e8, palette.text),
        ];
        match self.menu(&entries)? {
            // The first entry carries the icon's tooltip text and opens the popup.
            4 => self.open()?,
            // The About page opens outside the App borrow, from the message loop.
            2 => unsafe { PostMessageW(Some(self.hwnd), SHOW_ABOUT, WPARAM(0), LPARAM(0))? },
            1 => unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? },
            _ => {}
        }
        Ok(())
    }
}
