//! Keyboard, scrolling, and the actions that clicks and keys lead to.

use super::*;

/// Applies a click or keyboard activation; the caller acts on the returned action.
pub(super) fn activate(ui: &mut Ui, hit: Hit) -> Action {
    match hit {
        Hit::Back => match ui.parent() {
            Some(view) => ui.open(view),
            None => return Action::Close,
        },
        Hit::License(index) => ui.open(View::Notice(index)),
        Hit::Homepage(index) => {
            if let Some(credit) = ui.credits.get(index) {
                return Action::Homepage(credit.name);
            }
        }
        _ => {}
    }
    Action::None
}

pub(super) fn perform(hwnd: HWND, action: Action) {
    unsafe {
        match action {
            Action::None => {}
            Action::Close => {
                let _ = DestroyWindow(hwnd);
            }
            Action::Homepage(name) => {
                // Crate names are plain ASCII from the notices file; refuse anything else.
                if name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    let mut url = wide(&format!("https://crates.io/crates/{name}"));
                    url.push(0);
                    ShellExecuteW(
                        Some(hwnd),
                        w!("open"),
                        PCWSTR(url.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
            }
        }
    }
}

/// Moves the scroll target; [`animate`] then eases the view toward it.
pub(super) fn scroll_by(ui: &mut Ui, bounds: RECT, delta: i32) {
    let target = ui.target.saturating_add(delta);
    scroll_to(ui, bounds, target);
}

fn scroll_to(ui: &mut Ui, bounds: RECT, target: i32) {
    ui.target = target.clamp(0, ui.max_scroll(bounds));
}

/// Starts easing toward the scroll target, or jumps there when animations are off.
pub(super) fn animate(hwnd: HWND, ui: &mut Ui) {
    if ui.target == ui.scroll {
        return;
    }
    if !crate::ui::animations_enabled() {
        ui.scroll = ui.target;
        ui.position = ui.target as f32;
        return;
    }
    if ui.tick.is_none() {
        ui.tick = Some(Instant::now());
        unsafe {
            SetTimer(Some(hwnd), SCROLL_TIMER, 8, None);
        }
    }
}

/// One frame of the scroll animation; returns false once it has arrived.
pub(super) fn step(ui: &mut Ui) -> bool {
    let now = Instant::now();
    let elapsed = ui
        .tick
        .map_or(16.0, |tick| now.duration_since(tick).as_secs_f32() * 1000.0);
    ui.tick = Some(now);
    let remaining = ui.target as f32 - ui.position;
    ui.position += remaining * (1.0 - (-elapsed / SCROLL_EASE_MS).exp());
    if (ui.target as f32 - ui.position).abs() < 0.5 {
        ui.position = ui.target as f32;
        ui.scroll = ui.target;
        ui.tick = None;
        return false;
    }
    ui.scroll = ui.position.round() as i32;
    true
}

/// Moves the Credits selection and keeps it in view.
fn select(ui: &mut Ui, bounds: RECT, index: usize) {
    if ui.credits.is_empty() {
        return;
    }
    let index = index.min(ui.credits.len() - 1);
    ui.selected = Some(index);
    let top = index as i32 * ui.pitch();
    let view = ui.view_height(bounds);
    if top < ui.target {
        scroll_to(ui, bounds, top);
    } else if top + ui.pitch() > ui.target + view {
        scroll_to(ui, bounds, top + ui.pitch() - view);
    }
}

pub(super) fn key(hwnd: HWND, ui: &mut Ui, code: u32) -> Action {
    let bounds = client(hwnd);
    let page = ui.view_height(bounds) - ui.px(40.0);
    let line = ui.px(40.0);
    let credits = ui.view == View::Credits;
    ui.keyboard = true;
    match code {
        VK_ESCAPE | VK_BACK => return activate(ui, Hit::Back),
        VK_TAB => {
            ui.focus = match ui.focus_order().first() {
                Some(&hit) if ui.focus == Hit::None => hit,
                _ => Hit::None,
            };
        }
        VK_RETURN | VK_SPACE => {
            if ui.focus != Hit::None {
                return activate(ui, ui.focus);
            }
            if credits {
                return ui
                    .selected
                    .map_or(Action::None, |row| activate(ui, Hit::License(row)));
            }
        }
        VK_UP if credits => select(
            ui,
            bounds,
            ui.selected.map_or(0, |row| row.saturating_sub(1)),
        ),
        VK_DOWN if credits => select(ui, bounds, ui.selected.map_or(0, |row| row + 1)),
        VK_PRIOR if credits => select(
            ui,
            bounds,
            ui.selected.map_or(0, |row| row.saturating_sub(7)),
        ),
        VK_NEXT if credits => select(ui, bounds, ui.selected.map_or(0, |row| row + 7)),
        VK_HOME if credits => select(ui, bounds, 0),
        VK_END if credits => select(ui, bounds, usize::MAX / 2),
        VK_UP => scroll_by(ui, bounds, -line),
        VK_DOWN => scroll_by(ui, bounds, line),
        VK_PRIOR => scroll_by(ui, bounds, -page),
        VK_NEXT => scroll_by(ui, bounds, page),
        VK_HOME => scroll_to(ui, bounds, 0),
        VK_END => scroll_to(ui, bounds, i32::MAX),
        _ => return Action::None,
    }
    animate(hwnd, ui);
    Action::None
}

/// The pointer position in client coordinates.
pub(super) fn cursor(hwnd: HWND) -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
        ScreenToClient(hwnd, &mut point).ok().ok()?;
    }
    Some((point.x, point.y))
}
