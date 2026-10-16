//! Process entry points: the single-instance check, startup, diagnostics, and the message loop.

use super::*;

pub fn show_error(message: &str) {
    let message = wide(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("mixerplus"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn run() -> Result<()> {
    unsafe {
        let mutex = CreateMutexW(None, false, w!("Local\\mixerplus.Tray.Rust"))?;
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        if already_running {
            if std::env::args().any(|arg| arg == "--check-ui" || arg == "--snapshot") {
                let _ = CloseHandle(mutex);
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(0x80004005u32 as i32),
                    "Exit mixerplus before running UI diagnostics.",
                ));
            }
            if std::env::args().any(|arg| arg == "--show")
                && let Ok(hwnd) = FindWindowW(w!("mixerplusTrayPopup"), None)
            {
                let _ = PostMessageW(Some(hwnd), TRAY, WPARAM(0), LPARAM(WM_LBUTTONUP as isize));
            }
            let _ = CloseHandle(mutex);
            return Ok(());
        }
        let result = run_inner();
        let _ = CloseHandle(mutex);
        result
    }
}

pub fn check_audio() -> Result<String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = (|| {
            let audio = Audio::new()?;
            let outputs = audio.devices(eRender)?.len();
            let inputs = audio.devices(eCapture)?.len();
            let applications = audio.sessions()?.len();
            Ok(format!(
                "Audio check passed: {outputs} outputs, {inputs} inputs, {applications} application groups."
            ))
        })();
        CoUninitialize();
        result
    }
}

unsafe fn run_inner() -> Result<()> {
    unsafe {
        // All audio COM objects remain on this UI thread until shutdown.
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let drawing = match crate::drawing::Runtime::new() {
            Ok(drawing) => drawing,
            Err(error) => {
                CoUninitialize();
                return Err(error);
            }
        };
        let result = run_initialized();
        APP.with(|state| {
            state.borrow_mut().take();
        });
        drop(drawing);
        CoUninitialize();
        result
    }
}

unsafe fn run_initialized() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES,
        })
        .ok()?;
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("mixerplusTrayPopup"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let inspect = std::env::args().any(|arg| arg == "--inspect-ui");
        let hwnd = CreateWindowExW(
            if inspect {
                // Expose the popup to desktop inspection tools during development.
                WS_EX_APPWINDOW | WS_EX_TOPMOST
            } else {
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST
            },
            class.lpszClassName,
            // Task Manager lists visible windows with a title under the process.
            if inspect { w!("mixerplus") } else { w!("") },
            if inspect {
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN
            } else {
                WS_POPUP | WS_CLIPCHILDREN
            },
            0,
            0,
            crate::config::POPUP_WIDTH,
            400,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        crate::appearance::load_theme();
        crate::appearance::style_window(hwnd);
        let tooltip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            None,
            WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(hwnd),
            None,
            Some(instance.into()),
            None,
        )?;
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let icon = tray_icon()?;
        let app = App {
            audio: Audio::new()?,
            hwnd,
            tooltip,
            fonts: Fonts::new(scale),
            icon,
            controls: Vec::new(),
            devices: Vec::new(),
            children: Vec::new(),
            spare: Vec::new(),
            clicked: HWND::default(),
            page: 0,
            pages: 1,
            signature: Vec::new(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            scale,
            page_size: crate::config::MAX_APPLICATION_ROWS,
            device_rows: 8,
            device_offset: 0,
            work_area: RECT::default(),
            monitor: RECT::default(),
            navigation: None,
            chrome_count: 0,
            tray_added: false,
            promotion_attempts: 0,
            view: View::Mixer,
            animation: None,
            presets: PresetStore::open(
                PresetStore::default_path()
                    .unwrap_or_else(|| std::env::temp_dir().join("mixerplus-presets.json")),
            ),
            listed: Vec::new(),
            edit: None,
            armed_delete: None,
            list_rows: 6,
            pending: None,
            transition: None,
            target: None,
            dismissed: None,
        };
        APP.with(|state| *state.borrow_mut() = Some(app));
        if SetTimer(
            Some(hwnd),
            REFRESH_TIMER,
            crate::config::REFRESH_INTERVAL_MS,
            None,
        ) == 0
        {
            return Err(windows::core::Error::from_thread());
        }
        APP.with(|state| {
            if let Some(app) = state.borrow_mut().as_mut() {
                app.maintain_tray();
            }
        });
        let arguments: Vec<_> = std::env::args_os().collect();
        if arguments.iter().any(|arg| arg == "--check-ui") {
            check_page_navigation(hwnd)?;
            println!("UI check passed: next/previous preserve mixer rows and navigation handles.");
            return Ok(());
        }
        if let Some(pair) = arguments.windows(2).find(|pair| pair[0] == "--snapshot") {
            APP.with(|state| -> Result<()> {
                let mut state = state.borrow_mut();
                if let Some(app) = state.as_mut() {
                    app.open()?;
                    crate::appearance::snapshot(
                        hwnd,
                        &app.children,
                        std::path::Path::new(&pair[1]),
                    )?;
                }
                Ok(())
            })?;
            return Ok(());
        }
        if std::env::args().any(|arg| arg == "--show") {
            PostMessageW(Some(hwnd), TRAY, WPARAM(0), LPARAM(WM_LBUTTONUP as isize))?;
        }
        let mut message = MSG::default();
        loop {
            let status = GetMessageW(&mut message, None, 0, 0).0;
            if status == -1 {
                return Err(windows::core::Error::from_thread());
            }
            if status == 0 {
                break;
            }
            if crate::about::dispatch(&message) {
                continue;
            }
            if message.message == WM_KEYDOWN && message.wParam.0 == VK_ESCAPE_CODE {
                let _ = PostMessageW(Some(hwnd), DISMISS, WPARAM(0), LPARAM(0));
                continue;
            }
            if message.message == WM_KEYDOWN && message.wParam.0 == VK_RETURN_CODE {
                let focus = GetFocus();
                if !focus.is_invalid() && GetDlgCtrlID(focus) as usize == EDIT_ID {
                    let _ = PostMessageW(Some(hwnd), COMMIT_EDIT, WPARAM(0), LPARAM(0));
                    continue;
                }
            }
            if !IsDialogMessageW(hwnd, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }
}

const VK_ESCAPE_CODE: usize = 0x1b;

const VK_RETURN_CODE: usize = 0x0d;

fn check_page_navigation(hwnd: HWND) -> Result<()> {
    let fail =
        |message| windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), message);
    let (before, navigation, pages) = APP.with(|state| -> Result<_> {
        let mut state = state.borrow_mut();
        let app = state.as_mut().ok_or_else(|| fail("Mixer is missing"))?;
        app.open()?;
        Ok((
            app.controls
                .iter()
                .map(|c| c.row.key.clone())
                .collect::<Vec<_>>(),
            app.navigation
                .ok_or_else(|| fail("Navigation is missing"))?,
            app.pages,
        ))
    })?;
    if pages < 2 {
        println!("Pagination check skipped: fewer than two pages.");
        return Ok(());
    }
    for (button, expected_page) in [(navigation.next, 1), (navigation.previous, 0)] {
        unsafe {
            // Exercise the native notification path, including deferred row refresh.
            SendMessageW(button, BM_CLICK, None, None);
            let mut message = MSG::default();
            while PeekMessageW(
                &mut message,
                Some(hwnd),
                REFRESH_PAGE,
                REFRESH_PAGE,
                PM_REMOVE,
            )
            .as_bool()
            {
                DispatchMessageW(&message);
            }
        }
        APP.with(|state| -> Result<()> {
            let state = state.borrow();
            let app = state.as_ref().ok_or_else(|| fail("Mixer is missing"))?;
            let current = app
                .navigation
                .ok_or_else(|| fail("Navigation was destroyed"))?;
            if app.page != expected_page
                || current.previous != navigation.previous
                || current.next != navigation.next
            {
                return Err(fail(
                    "Page navigation changed control identity or selected the wrong page",
                ));
            }
            if expected_page == 0
                && app
                    .controls
                    .iter()
                    .map(|c| c.row.key.clone())
                    .collect::<Vec<_>>()
                    != before
            {
                return Err(fail("Returning to page one changed mixer rows"));
            }
            Ok(())
        })?;
    }
    Ok(())
}
