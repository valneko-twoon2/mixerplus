//! Geometry of the window's views, hit testing, and measuring the license text.

use super::*;

impl Ui {
    pub(super) fn px(&self, value: f32) -> i32 {
        (value * self.scale).round() as i32
    }

    pub(super) fn geo(&self, client: RECT) -> Geo {
        let px = |value| self.px(value);
        let top = px(88.0);
        let (card, view) = if self.view == View::Credits {
            (RECT::default(), rect(0, top, client.right, client.bottom))
        } else {
            // The About page has a section label above its card.
            let card_top = if self.view == View::About {
                top + px(36.0)
            } else {
                top
            };
            let card = rect(
                px(24.0),
                card_top,
                client.right - px(24.0),
                client.bottom - px(24.0),
            );
            (card, inflate(card, -1))
        };
        Geo {
            back: rect(px(16.0), px(22.0), px(52.0), px(58.0)),
            card,
            view,
            top,
        }
    }

    /// Height of one Credits card plus the gap below it.
    pub(super) fn pitch(&self) -> i32 {
        self.px(68.0)
    }

    pub(super) fn view_height(&self, client: RECT) -> i32 {
        let view = self.geo(client).view;
        (view.bottom - view.top).max(0)
    }

    pub(super) fn max_scroll(&self, client: RECT) -> i32 {
        (self.content - self.view_height(client)).max(0)
    }

    /// Jumps to a scroll offset without animating.
    pub(super) fn set_scroll(&mut self, client: RECT, value: i32) {
        self.scroll = value.clamp(0, self.max_scroll(client));
        self.position = self.scroll as f32;
        self.target = self.scroll;
    }

    /// Length of the scrollbar thumb for the current content.
    pub(super) fn thumb_length(&self, client: RECT) -> i32 {
        let height = self.view_height(client);
        (height * height / self.content.max(1)).max(self.px(36.0))
    }

    /// The scrollbar thumb, if the content overflows its region.
    pub(super) fn thumb(&self, client: RECT) -> Option<RECT> {
        let max = self.max_scroll(client);
        if max == 0 {
            return None;
        }
        let view = self.geo(client).view;
        let length = self.thumb_length(client);
        let top = view.top + (self.view_height(client) - length) * self.scroll / max;
        let width = if self.hover == Hit::Thumb || self.drag.is_some() {
            self.px(6.0)
        } else {
            self.px(3.0)
        };
        let left = view.right - self.px(8.0) - width / 2;
        Some(rect(left, top, left + width, top + length))
    }

    /// Card, license button and homepage button of a credit row whose card starts at
    /// `top`, spanning `left` to `right`.
    pub(super) fn row_layout(&self, left: i32, right: i32, top: i32) -> (RECT, RECT, RECT) {
        let row = rect(left, top, right, top + self.px(64.0));
        let button_h = self.px(32.0);
        let button_top = row.top + (row.bottom - row.top - button_h) / 2;
        let home = rect(
            row.right - self.px(16.0) - self.px(104.0),
            button_top,
            row.right - self.px(16.0),
            button_top + button_h,
        );
        let license = rect(
            home.left - self.px(8.0) - self.px(132.0),
            button_top,
            home.left - self.px(8.0),
            button_top + button_h,
        );
        (row, license, home)
    }

    /// A credit row in client coordinates at the current scroll offset.
    pub(super) fn row_rects(&self, client: RECT, index: usize) -> (RECT, RECT, RECT) {
        let view = self.geo(client).view;
        self.row_layout(
            self.px(24.0),
            client.right - self.px(24.0),
            view.top + index as i32 * self.pitch() - self.scroll,
        )
    }

    pub(super) fn hit_test(&self, client: RECT, x: i32, y: i32) -> Hit {
        let geo = self.geo(client);
        if matches!(self.view, View::Notice(_)) && contains(&geo.back, x, y) {
            return Hit::Back;
        }
        if contains(&geo.view, x, y) {
            if x >= geo.view.right - self.px(16.0)
                && let Some(thumb) = self.thumb(client)
            {
                let grab = rect(
                    geo.view.right - self.px(16.0),
                    thumb.top,
                    geo.view.right,
                    thumb.bottom,
                );
                return if contains(&grab, x, y) {
                    Hit::Thumb
                } else {
                    Hit::Track
                };
            }
            if self.view == View::Credits {
                for index in self.visible_rows(client) {
                    let (row, license, home) = self.row_rects(client, index);
                    if contains(&license, x, y) {
                        return Hit::License(index);
                    }
                    if contains(&home, x, y) {
                        return Hit::Homepage(index);
                    }
                    if contains(&row, x, y) {
                        return Hit::Row(index);
                    }
                }
            }
        }
        Hit::None
    }

    fn visible_rows(&self, client: RECT) -> std::ops::Range<usize> {
        let pitch = self.pitch().max(1);
        let first = (self.scroll.max(0) / pitch) as usize;
        let last = ((self.scroll + self.view_height(client)) / pitch + 1) as usize;
        first.min(self.credits.len())..last.min(self.credits.len())
    }

    /// Controls reachable with Tab.
    pub(super) fn focus_order(&self) -> &'static [Hit] {
        match self.view {
            View::Notice(_) => &[Hit::Back],
            _ => &[],
        }
    }

    pub(super) fn open(&mut self, view: View) {
        let restoring = matches!((self.view, view), (View::Notice(_), View::Credits));
        if matches!((self.view, view), (View::Credits, View::Notice(_))) {
            self.saved = (self.scroll, self.selected);
        }
        self.view = view;
        self.blocks = match view {
            View::About => blocks(LICENSE),
            View::Credits => Vec::new(),
            View::Notice(index) => self
                .credits
                .get(index)
                .map_or_else(Vec::new, |credit| blocks(credit.text)),
        };
        let (scroll, selected) = if restoring { self.saved } else { (0, None) };
        self.scroll = scroll;
        self.position = scroll as f32;
        self.target = scroll;
        self.tick = None;
        self.selected = selected;
        self.focus = Hit::None;
        self.hover = Hit::None;
        self.dirty = true;
    }

    /// The view shown by Back and Escape, or `None` to close the window.
    pub(super) fn parent(&self) -> Option<View> {
        match self.view {
            View::Notice(_) => Some(View::Credits),
            _ => None,
        }
    }

    fn text_width(&self, client: RECT) -> i32 {
        let card = self.geo(client).card;
        (card.right - card.left - self.px(16.0) - self.px(32.0)).max(self.px(120.0))
    }

    pub(super) fn indent(&self, block: &Block) -> i32 {
        ((block.indent.min(40) as f32) * 3.6 * self.scale) as i32
    }

    /// Re-measures the text blocks after a size, view, or DPI change.
    pub(super) unsafe fn layout(&mut self, dc: HDC, client: RECT) {
        unsafe {
            let size = (client.right, client.bottom);
            if !self.dirty && self.laid == size {
                return;
            }
            self.dirty = false;
            self.laid = size;
            if self.view == View::Credits {
                self.content = self.credits.len() as i32 * self.pitch() + self.px(16.0);
            } else {
                let width = self.text_width(client);
                let old = SelectObject(dc, self.fonts.body.into());
                self.heights.clear();
                let mut total = self.px(12.0) * 2;
                for block in &self.blocks {
                    let height = match block.kind {
                        Kind::Gap => self.px(8.0),
                        Kind::Heading => {
                            SelectObject(dc, self.fonts.title.into());
                            let height =
                                crate::drawing::measure(dc, &block.text, width) + self.px(10.0);
                            SelectObject(dc, self.fonts.body.into());
                            height
                        }
                        Kind::Body => {
                            crate::drawing::measure(dc, &block.text, width - self.indent(block))
                        }
                    };
                    self.heights.push(height);
                    total += height;
                }
                SelectObject(dc, old);
                self.content = total;
            }
            let scroll = self.scroll;
            self.set_scroll(client, scroll);
        }
    }

    pub(super) fn cache_key(&self, client: RECT, theme: Theme) -> CacheKey {
        let view = self.geo(client).view;
        CacheKey {
            view: self.view,
            size: (view.right - view.left, self.content),
            theme,
            scale: self.scale.to_bits(),
        }
    }
}
