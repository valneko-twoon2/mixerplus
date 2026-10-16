//! Painting: the Settings-style theme, the cached content bitmap, and everything drawn over it.

use super::*;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

pub(super) fn theme() -> Theme {
    let p = palette();
    if p.dark {
        Theme {
            background: rgb(32, 32, 32),
            card: rgb(43, 43, 43),
            border: rgb(58, 58, 58),
            hover: rgb(50, 50, 50),
            button: rgb(55, 55, 55),
            button_hover: rgb(62, 62, 62),
            button_pressed: rgb(48, 48, 48),
            button_border: rgb(70, 70, 70),
            text: p.text,
            secondary: p.secondary,
            accent: p.accent,
            track: p.track,
        }
    } else {
        Theme {
            background: rgb(243, 243, 243),
            card: rgb(251, 251, 251),
            border: rgb(229, 229, 229),
            hover: rgb(246, 246, 246),
            button: rgb(253, 253, 253),
            button_hover: rgb(249, 249, 249),
            button_pressed: rgb(245, 245, 245),
            button_border: rgb(219, 219, 219),
            text: p.text,
            secondary: p.secondary,
            accent: p.accent,
            track: p.track,
        }
    }
}

unsafe fn put(
    dc: HDC,
    font: HFONT,
    text: &str,
    at: RECT,
    clip: RECT,
    center: bool,
    color: COLORREF,
) {
    unsafe {
        let old = SelectObject(dc, font.into());
        crate::drawing::text_clipped(dc, &wide(text), at, clip, center, color);
        SelectObject(dc, old);
    }
}

/// A rounded rectangle with a one pixel border.
unsafe fn bordered(dc: HDC, bounds: RECT, diameter: i32, border: COLORREF, surface: COLORREF) {
    unsafe {
        solid_round(dc, bounds, diameter, border);
        solid_round(dc, inflate(bounds, -1), diameter - 2, surface);
    }
}

/// A Settings-style push button.
unsafe fn button(dc: HDC, ui: &Ui, t: &Theme, bounds: RECT, clip: RECT, label: &str, state: Hot) {
    unsafe {
        let (hot, down) = state;
        let surface = if down {
            t.button_pressed
        } else if hot {
            t.button_hover
        } else {
            t.button
        };
        bordered(dc, bounds, ui.px(8.0), t.button_border, surface);
        put(dc, ui.fonts.body, label, bounds, clip, true, t.text);
    }
}

/// Renders the whole scrolling content at scroll offset zero.
unsafe fn render(target: HDC, ui: &Ui, t: &Theme, client: RECT, key: CacheKey) -> Option<Cache> {
    unsafe {
        let view = ui.geo(client).view;
        let width = (view.right - view.left).max(1);
        let height = ui.content.max(view.bottom - view.top).max(1);
        let dc = CreateCompatibleDC(Some(target));
        let bitmap = CreateCompatibleBitmap(target, width, height);
        if dc.is_invalid() || bitmap.is_invalid() {
            if !bitmap.is_invalid() {
                let _ = DeleteObject(bitmap.into());
            }
            if !dc.is_invalid() {
                let _ = DeleteDC(dc);
            }
            return None;
        }
        let previous = SelectObject(dc, bitmap.into());
        let area = rect(0, 0, width, height);
        if ui.view == View::Credits {
            fill(dc, area, t.background);
            draw_credits(dc, ui, t, client, area);
        } else {
            fill(dc, area, t.card);
            draw_blocks(dc, ui, t, area);
        }
        Some(Cache {
            key,
            dc,
            bitmap,
            previous,
        })
    }
}

pub(super) unsafe fn paint(hwnd: HWND, ui: &mut Ui) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let target = BeginPaint(hwnd, &mut ps);
        let bounds = client(hwnd);
        ui.layout(target, bounds);
        let t = theme();
        let geo = ui.geo(bounds);
        let key = ui.cache_key(bounds, t);
        if ui.cache.as_ref().is_none_or(|cache| cache.key != key) {
            ui.cache = None;
            ui.cache = render(target, ui, &t, bounds, key);
        }
        let frame = crate::appearance::Frame::new(target, bounds);
        let dc = frame.dc;
        fill(dc, bounds, t.background);

        // The scrolling content, copied from the cache at the scroll offset.
        let view = geo.view;
        if ui.view != View::Credits {
            bordered(dc, geo.card, ui.px(8.0), t.border, t.card);
        }
        if let Some(cache) = &ui.cache {
            let _ = BitBlt(
                dc,
                view.left,
                view.top,
                view.right - view.left,
                view.bottom - view.top,
                Some(cache.dc),
                0,
                ui.scroll,
                SRCCOPY,
            );
        }
        if ui.view != View::Credits {
            // Restore the card's rounded corners over the copied rectangle.
            let saved = SaveDC(dc);
            ExcludeClipRect(
                dc,
                view.left + ui.px(6.0),
                view.top,
                view.right - ui.px(6.0),
                view.bottom,
            );
            ExcludeClipRect(
                dc,
                view.left,
                view.top + ui.px(6.0),
                view.right,
                view.bottom - ui.px(6.0),
            );
            fill(dc, inflate(geo.card, ui.px(1.0)), t.background);
            bordered(dc, geo.card, ui.px(8.0), t.border, t.card);
            let _ = RestoreDC(dc, saved);
        }
        if ui.view == View::Credits {
            // Only the hovered or keyboard-selected cards differ from the cache.
            let hovered = match ui.hover {
                Hit::Row(i) | Hit::License(i) | Hit::Homepage(i) => Some(i),
                _ => None,
            };
            let ringed = ui.selected.filter(|_| ui.keyboard);
            let saved = SaveDC(dc);
            IntersectClipRect(dc, view.left, view.top, view.right, view.bottom);
            for index in [ringed, hovered].into_iter().flatten() {
                draw_row(dc, ui, &t, index, ui.row_rects(bounds, index), view, true);
            }
            let _ = RestoreDC(dc, saved);
        }
        if let Some(thumb) = ui.thumb(bounds) {
            solid_round(dc, thumb, thumb.right - thumb.left, t.track);
        }

        // Page title, with a back button on a crate's license page.
        let left = if matches!(ui.view, View::Notice(_)) {
            ui.px(64.0)
        } else {
            ui.px(24.0)
        };
        let (title, subtitle) = match ui.view {
            View::About => (
                "mixerplus について".to_string(),
                format!("バージョン {}", env!("CARGO_PKG_VERSION")),
            ),
            View::Credits => (
                "Credits".to_string(),
                format!("{} 個のオープンソース ライブラリ", ui.credits.len()),
            ),
            View::Notice(index) => ui.credits.get(index).map_or_else(
                || (String::new(), String::new()),
                |credit| {
                    (
                        credit.name.to_string(),
                        format!("{} · {}", credit.version, credit.license),
                    )
                },
            ),
        };
        put(
            dc,
            ui.fonts.page,
            &title,
            rect(left, ui.px(16.0), bounds.right - ui.px(24.0), ui.px(60.0)),
            bounds,
            false,
            t.text,
        );
        put(
            dc,
            ui.fonts.small,
            &subtitle,
            rect(left, ui.px(58.0), bounds.right - ui.px(24.0), ui.px(78.0)),
            bounds,
            false,
            t.secondary,
        );
        if matches!(ui.view, View::Notice(_)) {
            let hot = ui.hover == Hit::Back;
            let focused = ui.focus == Hit::Back;
            if hot || focused {
                let surface = if ui.pressed == Hit::Back && hot {
                    t.button_pressed
                } else {
                    t.hover
                };
                if focused {
                    bordered(dc, geo.back, ui.px(8.0), t.accent, surface);
                } else {
                    solid_round(dc, geo.back, ui.px(8.0), surface);
                }
            }
            put(
                dc,
                ui.fonts.icon,
                BACK_GLYPH,
                geo.back,
                bounds,
                true,
                t.text,
            );
        }
        if ui.view == View::About {
            put(
                dc,
                ui.fonts.title,
                "ライセンス",
                rect(
                    ui.px(28.0),
                    geo.top,
                    bounds.right - ui.px(24.0),
                    geo.top + ui.px(32.0),
                ),
                bounds,
                false,
                t.text,
            );
        }
        drop(frame);
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Draws the license paragraphs into `area`, the card's inside at scroll offset zero.
unsafe fn draw_blocks(dc: HDC, ui: &Ui, t: &Theme, area: RECT) {
    unsafe {
        let left = area.left + ui.px(16.0);
        let width = area.right - area.left - ui.px(16.0) - ui.px(32.0);
        let mut y = area.top + ui.px(12.0);
        for (block, &height) in ui.blocks.iter().zip(&ui.heights) {
            if block.kind != Kind::Gap {
                let heading = block.kind == Kind::Heading;
                let old = SelectObject(
                    dc,
                    if heading {
                        ui.fonts.title
                    } else {
                        ui.fonts.body
                    }
                    .into(),
                );
                let indent = ui.indent(block);
                crate::drawing::paragraph(
                    dc,
                    &block.text,
                    area,
                    left + indent,
                    y + if heading { ui.px(10.0) } else { 0 },
                    width - indent,
                    if heading { t.secondary } else { t.text },
                );
                SelectObject(dc, old);
            }
            y += height;
        }
    }
}

/// Draws every credit card into `area`, the list at scroll offset zero.
unsafe fn draw_credits(dc: HDC, ui: &Ui, t: &Theme, client: RECT, area: RECT) {
    unsafe {
        let view = ui.geo(client).view;
        for index in 0..ui.credits.len() {
            let rects = ui.row_layout(
                ui.px(24.0) - view.left,
                client.right - ui.px(24.0) - view.left,
                area.top + index as i32 * ui.pitch(),
            );
            draw_row(dc, ui, t, index, rects, area, false);
        }
    }
}

/// Draws one credit card. With `live` false it is drawn at rest, for the cache;
/// with `live` true it shows the pointer and keyboard state, over the cached copy.
unsafe fn draw_row(
    dc: HDC,
    ui: &Ui,
    t: &Theme,
    index: usize,
    (row, license, home): (RECT, RECT, RECT),
    clip: RECT,
    live: bool,
) {
    unsafe {
        let Some(credit) = ui.credits.get(index) else {
            return;
        };
        let state = |hit: Hit| -> Hot {
            let hot = live && ui.hover == hit;
            (hot, hot && ui.pressed == hit)
        };
        let on_row = live
            && matches!(
                ui.hover,
                Hit::Row(i) | Hit::License(i) | Hit::Homepage(i) if i == index
            );
        if live && ui.keyboard && ui.selected == Some(index) {
            solid_round(dc, inflate(row, ui.px(2.0)), ui.px(12.0), t.accent);
            solid_round(dc, inflate(row, ui.px(1.0)), ui.px(10.0), t.background);
        }
        bordered(
            dc,
            row,
            ui.px(8.0),
            t.border,
            if on_row { t.hover } else { t.card },
        );
        let text_right = license.left - ui.px(12.0);
        let text_left = row.left + ui.px(16.0);
        put(
            dc,
            ui.fonts.title,
            credit.name,
            rect(
                text_left,
                row.top + ui.px(10.0),
                text_right,
                row.top + ui.px(32.0),
            ),
            clip,
            false,
            t.text,
        );
        put(
            dc,
            ui.fonts.small,
            &format!("{} · {}", credit.version, credit.license),
            rect(
                text_left,
                row.top + ui.px(32.0),
                text_right,
                row.top + ui.px(52.0),
            ),
            clip,
            false,
            t.secondary,
        );
        button(
            dc,
            ui,
            t,
            license,
            clip,
            "ライセンスを表示",
            state(Hit::License(index)),
        );
        button(
            dc,
            ui,
            t,
            home,
            clip,
            "ホームページ",
            state(Hit::Homepage(index)),
        );
    }
}
