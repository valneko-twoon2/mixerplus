//! The notification icon: adding it to Explorer, promoting it on Windows 11, and its context menu.

use super::*;

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

    fn menu(&self, entries: &[(usize, &str)]) -> Result<usize> {
        unsafe {
            let menu = CreatePopupMenu()?;
            let result = (|| {
                for (id, name) in entries {
                    let name = wide(name);
                    AppendMenuW(menu, MF_STRING, *id, PCWSTR(name.as_ptr()))?;
                }
                let mut point = POINT::default();
                GetCursorPos(&mut point)?;
                let _ = SetForegroundWindow(self.hwnd);
                let chosen = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
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
            result
        }
    }

    /// The tray icon's context menu.
    pub(super) fn tray_menu(&mut self) -> Result<()> {
        // The menu runs its own message loop, which would stall a sliding popup.
        self.hide_now();
        let entries = [(4, TRAY_TITLE), (2, "About"), (1, "Exit")];
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
