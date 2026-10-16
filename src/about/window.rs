//! Creating the About window, its style, and its window procedure.

use super::*;

thread_local! {
    static ABOUT: RefCell<Option<HWND>> = const { RefCell::new(None) };
}

/// Shows the About page (`credits` false) or the Credits list, reusing an open window.
pub fn show(credits: bool) -> Result<()> {
    unsafe {
        let view = if credits { View::Credits } else { View::About };
        if let Some(existing) = ABOUT.with(|about| *about.borrow())
            && IsWindow(Some(existing)).as_bool()
        {
            if let Some(cell) = state(existing)
                && let Ok(mut ui) = cell.try_borrow_mut()
            {
                ui.open(view);
            }
            invalidate(existing);
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            return Ok(());
        }
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            // The embedded application icon (ID 1, from build.rs), shown in Alt+Tab.
            hIcon: LoadIconW(
                Some(instance.into()),
                PCWSTR(std::ptr::without_provenance(1)),
            )
            .unwrap_or_default(),
            ..Default::default()
        };
        // Registering twice fails harmlessly with ERROR_CLASS_ALREADY_EXISTS.
        RegisterClassW(&class);
        crate::appearance::load_theme();
        let owner = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            None,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let mut ui = Ui {
            view: View::About,
            credits: parse_credits(NOTICES),
            blocks: Vec::new(),
            heights: Vec::new(),
            laid: (0, 0),
            dirty: true,
            content: 0,
            scroll: 0,
            position: 0.0,
            target: 0,
            tick: None,
            selected: None,
            keyboard: false,
            focus: Hit::None,
            hover: Hit::None,
            pressed: Hit::None,
            drag: None,
            saved: (0, None),
            scale: 1.0,
            fonts: Fonts::new(1.0),
            tracking: false,
            cache: None,
            owner,
        };
        ui.open(view);
        let state = Box::into_raw(Box::new(RefCell::new(ui)));
        // The caption stays for Alt+Tab and accessibility; `style` hides it on screen.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("mixerplus"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(owner),
            None,
            Some(instance.into()),
            Some(state.cast()),
        );
        let Ok(hwnd) = hwnd else {
            let _ = DestroyWindow(owner);
            return hwnd.map(|_| ());
        };
        style(hwnd);
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let mut area = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut area as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let width = ((640.0 * scale) as i32).min(area.right - area.left);
        let height = ((680.0 * scale) as i32).min(area.bottom - area.top);
        let _ = SetWindowPos(
            hwnd,
            None,
            area.left + (area.right - area.left - width) / 2,
            area.top + (area.bottom - area.top - height) / 2,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        ABOUT.with(|about| *about.borrow_mut() = Some(hwnd));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        Ok(())
    }
}

/// Gives keystrokes to the About window ahead of the popup's own handling.
pub fn dispatch(message: &MSG) -> bool {
    let Some(about) = ABOUT.with(|about| *about.borrow()) else {
        return false;
    };
    if message.hwnd != about {
        return false;
    }
    unsafe {
        let _ = TranslateMessage(message);
        DispatchMessageW(message);
    }
    true
}

/// A title bar in the window color with only the caption buttons, like Settings.
unsafe fn style(hwnd: HWND) {
    unsafe {
        crate::appearance::style_window(hwnd);
        let t = theme();
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, t.background),
            (DWMWA_TEXT_COLOR, t.text),
            (DWMWA_BORDER_COLOR, t.border),
        ] {
            let _ = DwmSetWindowAttribute(
                hwnd,
                attribute,
                (&color as *const COLORREF).cast(),
                size_of::<COLORREF>() as u32,
            );
        }
        let dark = BOOL::from(palette().dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const BOOL).cast(),
            size_of::<BOOL>() as u32,
        );
        let options = WTA_OPTIONS {
            dwFlags: WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON,
            dwMask: WTNCA_NODRAWCAPTION | WTNCA_NODRAWICON,
        };
        let _ = SetWindowThemeAttribute(
            hwnd,
            WTA_NONCLIENT,
            (&options as *const WTA_OPTIONS).cast(),
            size_of::<WTA_OPTIONS>() as u32,
        );
    }
}

fn state<'a>(hwnd: HWND) -> Option<&'a RefCell<Ui>> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Ui>).as_ref() }
}

pub(super) fn client(hwnd: HWND) -> RECT {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    client
}

fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let Some(cell) = state(hwnd) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let point = (
            (lparam.0 & 0xffff) as i16 as i32,
            ((lparam.0 >> 16) & 0xffff) as i16 as i32,
        );
        match message {
            WM_CREATE => {
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    ui.scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
                    ui.fonts = Fonts::new(ui.scale);
                    ui.dirty = true;
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    paint(hwnd, &mut ui);
                }
                return LRESULT(0);
            }
            WM_SIZE => {
                invalidate(hwnd);
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == SCROLL_TIMER => {
                let bounds = client(hwnd);
                if let Ok(mut ui) = cell.try_borrow_mut()
                    && !step(&mut ui)
                {
                    let _ = KillTimer(Some(hwnd), SCROLL_TIMER);
                    // The pointer may rest over a different card now.
                    if let Some((x, y)) = cursor(hwnd) {
                        ui.hover = ui.hit_test(bounds, x, y);
                    }
                }
                invalidate(hwnd);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let suggested = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    ui.scale = (wparam.0 & 0xffff) as f32 / 96.0;
                    ui.fonts = Fonts::new(ui.scale);
                    ui.dirty = true;
                    ui.cache = None;
                }
                invalidate(hwnd);
                return LRESULT(0);
            }
            WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED | WM_THEMECHANGED => {
                crate::appearance::load_theme();
                style(hwnd);
                invalidate(hwnd);
            }
            WM_MOUSEMOVE => {
                let bounds = client(hwnd);
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    if let Some(offset) = ui.drag {
                        let top = point.1 - offset - ui.geo(bounds).view.top;
                        let travel = (ui.view_height(bounds) - ui.thumb_length(bounds)).max(1);
                        let target =
                            (top as i64 * ui.max_scroll(bounds) as i64 / travel as i64) as i32;
                        ui.set_scroll(bounds, target);
                        invalidate(hwnd);
                        return LRESULT(0);
                    }
                    let hit = ui.hit_test(bounds, point.0, point.1);
                    if hit != ui.hover {
                        ui.hover = hit;
                        invalidate(hwnd);
                    }
                    if !ui.tracking {
                        ui.tracking = true;
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut track);
                    }
                }
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    ui.tracking = false;
                    if ui.drag.is_none() && ui.hover != Hit::None {
                        ui.hover = Hit::None;
                        invalidate(hwnd);
                    }
                }
                return LRESULT(0);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let bounds = client(hwnd);
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    let hit = ui.hit_test(bounds, point.0, point.1);
                    ui.hover = hit;
                    ui.pressed = hit;
                    ui.focus = Hit::None;
                    ui.keyboard = false;
                    match hit {
                        Hit::Thumb => {
                            if let Some(thumb) = ui.thumb(bounds) {
                                ui.drag = Some(point.1 - thumb.top);
                                let scroll = ui.scroll;
                                ui.set_scroll(bounds, scroll);
                            }
                        }
                        Hit::Track => {
                            let page = ui.view_height(bounds) - ui.px(40.0);
                            let up = ui.thumb(bounds).is_some_and(|thumb| point.1 < thumb.top);
                            scroll_by(&mut ui, bounds, if up { -page } else { page });
                            animate(hwnd, &mut ui);
                        }
                        Hit::Row(index) => {
                            ui.selected = Some(index);
                            if message == WM_LBUTTONDBLCLK {
                                ui.open(View::Notice(index));
                                ui.pressed = Hit::None;
                            }
                        }
                        _ => {}
                    }
                    invalidate(hwnd);
                    if ui.pressed != Hit::None {
                        SetCapture(hwnd);
                    }
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                let bounds = client(hwnd);
                let mut action = Action::None;
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    let hit = ui.hit_test(bounds, point.0, point.1);
                    let pressed = std::mem::replace(&mut ui.pressed, Hit::None);
                    if ui.drag.take().is_none() && hit == pressed {
                        action = activate(&mut ui, hit);
                    }
                    ui.hover = ui.hit_test(bounds, point.0, point.1);
                    invalidate(hwnd);
                }
                let _ = ReleaseCapture();
                perform(hwnd, action);
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let bounds = client(hwnd);
                let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as i32;
                if let Ok(mut ui) = cell.try_borrow_mut() {
                    let step = ui.px(100.0) * delta / 120;
                    scroll_by(&mut ui, bounds, -step);
                    animate(hwnd, &mut ui);
                }
                invalidate(hwnd);
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                let action = match cell.try_borrow_mut() {
                    Ok(mut ui) => key(hwnd, &mut ui, wparam.0 as u32),
                    Err(_) => Action::None,
                };
                invalidate(hwnd);
                perform(hwnd, action);
                return LRESULT(0);
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                ABOUT.with(|about| {
                    let mut about = about.borrow_mut();
                    if *about == Some(hwnd) {
                        *about = None;
                    }
                });
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut RefCell<Ui>;
                if !pointer.is_null() {
                    let state = Box::from_raw(pointer);
                    let owner = state.borrow().owner;
                    drop(state);
                    let _ = DestroyWindow(owner);
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
