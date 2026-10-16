//! The About and Credits window, laid out like a Windows 11 Settings page: a large
//! title, section labels, and rounded cards on the window background. One
//! custom-painted window shows MixerPlus's own license, the list of bundled crates,
//! and each crate's license text; none of it touches audio.
//!
//! The scrolling content is rendered once into an offscreen bitmap and only copied
//! while it scrolls, so wheel and keyboard scrolling can animate smoothly.

use std::{cell::RefCell, mem::size_of, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                SetWindowThemeAttribute, WM_MOUSELEAVE, WTA_NONCLIENT, WTA_OPTIONS,
                WTNCA_NODRAWCAPTION, WTNCA_NODRAWICON,
            },
            HiDpi::*,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
            },
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, PCWSTR, Result, w},
};

use crate::appearance::{fill, palette, solid_round};

const LICENSE: &str = include_str!("../LICENSE");
const NOTICES: &str = include_str!("../THIRD-PARTY-NOTICES.txt");
const CLASS: PCWSTR = w!("MixerPlusAbout");

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0d;
const VK_ESCAPE: u32 = 0x1b;
const VK_SPACE: u32 = 0x20;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const SPI_GETWORKAREA: SYSTEM_PARAMETERS_INFO_ACTION = SYSTEM_PARAMETERS_INFO_ACTION(0x30);
const SCROLL_TIMER: usize = 1;
/// Time constant of the scroll easing; about 95% of the distance takes three of these.
const SCROLL_EASE_MS: f32 = 55.0;

// Segoe Fluent Icons code point.
const BACK_GLYPH: &str = "\u{e72b}";

thread_local! {
    static ABOUT: RefCell<Option<HWND>> = const { RefCell::new(None) };
}

/// One bundled crate, parsed from `THIRD-PARTY-NOTICES.txt`.
struct Credit {
    name: &'static str,
    version: &'static str,
    license: &'static str,
    text: &'static str,
}

/// Parses the notices file: a header `name version - license` followed by
/// `--- file ---` sections, until the next header.
fn parse_credits(source: &'static str) -> Vec<Credit> {
    let mut lines = Vec::new();
    let mut start = 0;
    for line in source.split_inclusive('\n') {
        lines.push((start, line.trim_end()));
        start += line.len();
    }
    let mut heads = Vec::new();
    for index in 0..lines.len().saturating_sub(1) {
        if lines[index].1.starts_with("--- ") || !lines[index + 1].1.starts_with("--- ") {
            continue;
        }
        let mut parts = lines[index].1.splitn(4, ' ');
        if let (Some(name), Some(version), Some("-"), Some(license)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        {
            heads.push((lines[index].0, name, version, license));
        }
    }
    let mut credits = Vec::new();
    for (position, &(offset, name, version, license)) in heads.iter().enumerate() {
        let end = heads.get(position + 1).map_or(source.len(), |next| next.0);
        let section = source.get(offset..end).unwrap_or("");
        // Drop the header line itself.
        let text = section
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_end();
        credits.push(Credit {
            name,
            version,
            license,
            text,
        });
    }
    credits
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Body,
    Heading,
    Gap,
}

/// A paragraph of license text, reflowed to the window width.
struct Block {
    kind: Kind,
    /// Leading spaces of the source line, kept as a left indent.
    indent: i32,
    text: Vec<u16>,
}

/// Splits license text into paragraphs: hard-wrapped lines of one paragraph are joined,
/// short or differently indented lines stay apart, and `--- file ---` becomes a heading.
fn blocks(source: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut previous = 0;
    for line in source.lines() {
        let line = line.trim_end().replace('\t', "    ");
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if out.last().is_some_and(|block| block.kind != Kind::Gap) {
                out.push(Block {
                    kind: Kind::Gap,
                    indent: 0,
                    text: Vec::new(),
                });
            }
            previous = 0;
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("--- ")
            .and_then(|rest| rest.strip_suffix(" ---"))
        {
            while out.last().is_some_and(|block| block.kind == Kind::Gap) {
                out.pop();
            }
            out.push(Block {
                kind: Kind::Heading,
                indent: 0,
                text: name.encode_utf16().collect(),
            });
            previous = 0;
            continue;
        }
        let indent = (line.len() - trimmed.len()) as i32;
        let joined = previous >= 45
            && out
                .last_mut()
                .filter(|block| block.kind == Kind::Body && block.indent == indent)
                .map(|block| {
                    block.text.push(b' ' as u16);
                    block.text.extend(trimmed.encode_utf16());
                })
                .is_some();
        if !joined {
            out.push(Block {
                kind: Kind::Body,
                indent,
                text: trimmed.encode_utf16().collect(),
            });
        }
        previous = trimmed.chars().count();
    }
    while out.last().is_some_and(|block| block.kind == Kind::Gap) {
        out.pop();
    }
    out
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// The Settings app's surfaces: a flat window color with slightly lighter cards.
#[derive(Clone, Copy, PartialEq)]
struct Theme {
    background: COLORREF,
    card: COLORREF,
    border: COLORREF,
    hover: COLORREF,
    button: COLORREF,
    button_hover: COLORREF,
    button_pressed: COLORREF,
    button_border: COLORREF,
    text: COLORREF,
    secondary: COLORREF,
    accent: COLORREF,
    track: COLORREF,
}

fn theme() -> Theme {
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

#[derive(Clone, Copy, PartialEq)]
enum View {
    About,
    Credits,
    Notice(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Back,
    Row(usize),
    License(usize),
    Homepage(usize),
    Thumb,
    Track,
}

struct Fonts {
    body: HFONT,
    title: HFONT,
    page: HFONT,
    small: HFONT,
    icon: HFONT,
}

impl Fonts {
    fn new(scale: f32) -> Self {
        let icon_face = crate::drawing::icon_face();
        let font = |size: f32, weight: i32, face: PCWSTR| unsafe {
            CreateFontW(
                -(size * scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                face,
            )
        };
        Self {
            body: font(13.0, 400, crate::drawing::FONT),
            title: font(14.0, 600, crate::drawing::FONT),
            page: font(28.0, 600, crate::drawing::FONT),
            small: font(12.0, 400, crate::drawing::FONT),
            icon: font(16.0, 400, icon_face),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.title, self.page, self.small, self.icon] {
            unsafe {
                let _ = DeleteObject(font.into());
            }
        }
    }
}

/// What the cached content depends on; any change re-renders it. Pointer and
/// keyboard states are drawn over the copy instead, so hovering never re-renders.
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    view: View,
    size: (i32, i32),
    theme: Theme,
    scale: u32,
}

/// The whole scrolling content, rendered once into an offscreen bitmap.
struct Cache {
    key: CacheKey,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}

impl Drop for Cache {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Ui {
    view: View,
    credits: Vec<Credit>,
    /// The current view's license text, for the About and Notice views.
    blocks: Vec<Block>,
    heights: Vec<i32>,
    /// Client size the heights were laid out for.
    laid: (i32, i32),
    dirty: bool,
    content: i32,
    /// Shown scroll offset, and the exact position of a running scroll animation.
    scroll: i32,
    position: f32,
    /// Where the scroll animation is heading.
    target: i32,
    tick: Option<Instant>,
    /// The Credits row chosen by click or keyboard; none until the user picks one.
    selected: Option<usize>,
    /// Keyboard navigation is active, so focus cues are drawn.
    keyboard: bool,
    focus: Hit,
    hover: Hit,
    pressed: Hit,
    /// Mouse offset inside the scrollbar thumb while dragging it.
    drag: Option<i32>,
    /// Credits scroll position and selection kept while a Notice is open.
    saved: (i32, Option<usize>),
    scale: f32,
    fonts: Fonts,
    tracking: bool,
    cache: Option<Cache>,
    /// Hidden owner window: an owned window gets no taskbar button.
    owner: HWND,
}

/// Rectangles of the current view, in client pixels.
struct Geo {
    back: RECT,
    /// The license card of the text views.
    card: RECT,
    /// The scrolling region; content is clipped to it.
    view: RECT,
    /// Top of the content below the page title.
    top: i32,
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

fn inflate(r: RECT, by: i32) -> RECT {
    rect(r.left - by, r.top - by, r.right + by, r.bottom + by)
}

fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

impl Ui {
    fn px(&self, value: f32) -> i32 {
        (value * self.scale).round() as i32
    }

    fn geo(&self, client: RECT) -> Geo {
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
    fn pitch(&self) -> i32 {
        self.px(68.0)
    }

    fn view_height(&self, client: RECT) -> i32 {
        let view = self.geo(client).view;
        (view.bottom - view.top).max(0)
    }

    fn max_scroll(&self, client: RECT) -> i32 {
        (self.content - self.view_height(client)).max(0)
    }

    /// Jumps to a scroll offset without animating.
    fn set_scroll(&mut self, client: RECT, value: i32) {
        self.scroll = value.clamp(0, self.max_scroll(client));
        self.position = self.scroll as f32;
        self.target = self.scroll;
    }

    /// Length of the scrollbar thumb for the current content.
    fn thumb_length(&self, client: RECT) -> i32 {
        let height = self.view_height(client);
        (height * height / self.content.max(1)).max(self.px(36.0))
    }

    /// The scrollbar thumb, if the content overflows its region.
    fn thumb(&self, client: RECT) -> Option<RECT> {
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
    fn row_layout(&self, left: i32, right: i32, top: i32) -> (RECT, RECT, RECT) {
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
    fn row_rects(&self, client: RECT, index: usize) -> (RECT, RECT, RECT) {
        let view = self.geo(client).view;
        self.row_layout(
            self.px(24.0),
            client.right - self.px(24.0),
            view.top + index as i32 * self.pitch() - self.scroll,
        )
    }

    fn hit_test(&self, client: RECT, x: i32, y: i32) -> Hit {
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
    fn focus_order(&self) -> &'static [Hit] {
        match self.view {
            View::Notice(_) => &[Hit::Back],
            _ => &[],
        }
    }

    fn open(&mut self, view: View) {
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
    fn parent(&self) -> Option<View> {
        match self.view {
            View::Notice(_) => Some(View::Credits),
            _ => None,
        }
    }

    fn text_width(&self, client: RECT) -> i32 {
        let card = self.geo(client).card;
        (card.right - card.left - self.px(16.0) - self.px(32.0)).max(self.px(120.0))
    }

    fn indent(&self, block: &Block) -> i32 {
        ((block.indent.min(40) as f32) * 3.6 * self.scale) as i32
    }

    /// Re-measures the text blocks after a size, view, or DPI change.
    unsafe fn layout(&mut self, dc: HDC, client: RECT) {
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

    fn cache_key(&self, client: RECT, theme: Theme) -> CacheKey {
        let view = self.geo(client).view;
        CacheKey {
            view: self.view,
            size: (view.right - view.left, self.content),
            theme,
            scale: self.scale.to_bits(),
        }
    }
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
            w!("MixerPlus"),
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

fn client(hwnd: HWND) -> RECT {
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

unsafe fn paint(hwnd: HWND, ui: &mut Ui) {
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
                "MixerPlus について".to_string(),
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

/// Hovered and pressed state of a control: (hot, down).
type Hot = (bool, bool);

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

enum Action {
    None,
    Close,
    Homepage(&'static str),
}

/// Applies a click or keyboard activation; the caller acts on the returned action.
fn activate(ui: &mut Ui, hit: Hit) -> Action {
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

fn perform(hwnd: HWND, action: Action) {
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
fn scroll_by(ui: &mut Ui, bounds: RECT, delta: i32) {
    let target = ui.target.saturating_add(delta);
    scroll_to(ui, bounds, target);
}

fn scroll_to(ui: &mut Ui, bounds: RECT, target: i32) {
    ui.target = target.clamp(0, ui.max_scroll(bounds));
}

/// Starts easing toward the scroll target, or jumps there when animations are off.
fn animate(hwnd: HWND, ui: &mut Ui) {
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
fn step(ui: &mut Ui) -> bool {
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

fn key(hwnd: HWND, ui: &mut Ui, code: u32) -> Action {
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
fn cursor(hwnd: HWND) -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
        ScreenToClient(hwnd, &mut point).ok().ok()?;
    }
    Some((point.x, point.y))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_parse_into_credits_with_license_text() {
        let credits = parse_credits(NOTICES);
        let headers = NOTICES
            .lines()
            .filter(|line| line.contains(" - ") && !line.starts_with("--- "))
            .count();
        assert!(credits.len() >= 10 && credits.len() <= headers);
        assert!(credits.iter().any(|c| c.name == "windows"));
        assert!(
            credits
                .iter()
                .all(|c| !c.version.is_empty() && c.text.starts_with("--- "))
        );
    }

    #[test]
    fn license_paragraphs_are_reflowed() {
        let parts = blocks(LICENSE);
        // The MIT text joins hard-wrapped lines, so far fewer blocks than source lines.
        assert!(parts.len() < LICENSE.lines().count() / 2);
        assert!(parts.iter().any(|block| block.kind == Kind::Body));
    }
}
