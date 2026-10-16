//! Antialiased text, vector shapes, and high-quality icon resampling.

use std::{cell::RefCell, ptr::null_mut};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::*,
            GdiPlus::*,
        },
        UI::WindowsAndMessaging::HICON,
    },
    core::{PCWSTR, Result, w},
};

/// The Windows 11 shell font. DirectWrite falls back to Yu Gothic UI for Japanese
/// like the quick settings flyouts, whereas GDI font linking would pick Meiryo UI.
pub const FONT: PCWSTR = w!("Segoe UI Variable Text");
/// Windows 11 icon font, with the Windows 10 font that shares its code points.
pub const ICON_FONTS: [PCWSTR; 2] = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")];

struct Text {
    target: ID2D1DCRenderTarget,
    factory: IDWriteFactory,
}

thread_local! {
    // COM objects stay on the UI thread and are released before GDI+ and COM shut down.
    static TEXT: RefCell<Option<Text>> = const { RefCell::new(None) };
}

pub struct Runtime(usize);

impl Runtime {
    pub fn new() -> Result<Self> {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        let status = unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
        if status != Ok {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                "Graphics initialization failed",
            ));
        }
        let runtime = Self(token);
        let text = unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_IGNORE,
                },
                // Coordinates and font sizes are already in device pixels.
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            })?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Text {
                target,
                factory: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            }
        };
        TEXT.with(|state| *state.borrow_mut() = Some(text));
        std::result::Result::Ok(runtime)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        TEXT.with(|state| state.borrow_mut().take());
        unsafe {
            GdiplusShutdown(self.0);
        }
    }
}

pub struct Canvas(*mut GpGraphics);
struct Brush(*mut GpSolidFill);
struct Pen(*mut GpPen);
struct Path(*mut GpPath);

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            GdipFlush(self.0, FlushIntentionSync);
            GdipDeleteGraphics(self.0);
        }
    }
}
impl Drop for Brush {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteBrush(self.0.cast());
        }
    }
}
impl Drop for Pen {
    fn drop(&mut self) {
        unsafe {
            GdipDeletePen(self.0);
        }
    }
}
impl Drop for Path {
    fn drop(&mut self) {
        unsafe {
            GdipDeletePath(self.0);
        }
    }
}

fn argb(color: COLORREF) -> u32 {
    0xff000000 | (color.0 & 0xff) << 16 | color.0 & 0xff00 | (color.0 >> 16) & 0xff
}

impl Brush {
    fn new(color: COLORREF) -> Option<Self> {
        let mut brush = null_mut();
        (unsafe { GdipCreateSolidFill(argb(color), &mut brush) } == Ok).then_some(Self(brush))
    }
}

impl Pen {
    fn new(color: COLORREF, width: f32) -> Option<Self> {
        let mut pen = null_mut();
        if unsafe { GdipCreatePen1(argb(color), width, UnitPixel, &mut pen) } != Ok {
            return None;
        }
        unsafe {
            GdipSetPenLineJoin(pen, LineJoinRound);
            GdipSetPenStartCap(pen, LineCapRound);
            GdipSetPenEndCap(pen, LineCapRound);
        }
        Some(Self(pen))
    }
}

impl Canvas {
    pub unsafe fn new(dc: HDC) -> Option<Self> {
        unsafe {
            // Complete queued GDI backgrounds before GDI+ writes to the same DC.
            let _ = GdiFlush();
            let mut graphics = null_mut();
            if GdipCreateFromHDC(dc, &mut graphics) != Ok {
                return None;
            }
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias8x8);
            GdipSetPixelOffsetMode(graphics, PixelOffsetModeHalf);
            GdipSetInterpolationMode(graphics, InterpolationModeHighQualityBicubic);
            GdipSetTextRenderingHint(graphics, TextRenderingHintAntiAliasGridFit);
            Some(Self(graphics))
        }
    }

    pub fn round(&self, rect: RECT, diameter: i32, color: COLORREF) {
        let Some(brush) = Brush::new(color) else {
            return;
        };
        let x = rect.left as f32;
        let y = rect.top as f32;
        let w = (rect.right - rect.left) as f32;
        let h = (rect.bottom - rect.top) as f32;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let d = (diameter as f32).min(w).min(h).max(1.0);
        unsafe {
            if (w - h).abs() < 0.01 && d >= w {
                GdipFillEllipse(self.0, brush.0.cast(), x, y, w, h);
                return;
            }
            let mut path = null_mut();
            if GdipCreatePath(FillModeAlternate, &mut path) != Ok {
                return;
            }
            let path = Path(path);
            for (x, y, angle) in [
                (x, y, 180.0),
                (x + w - d, y, 270.0),
                (x + w - d, y + h - d, 0.0),
                (x, y + h - d, 90.0),
            ] {
                GdipAddPathArc(path.0, x, y, d, d, angle, 90.0);
            }
            GdipClosePathFigure(path.0);
            GdipFillPath(self.0, brush.0.cast(), path.0);
        }
    }

    pub fn ellipse(&self, rect: RECT, color: COLORREF, width: f32) {
        if let Some(pen) = Pen::new(color, width) {
            unsafe {
                GdipDrawEllipse(
                    self.0,
                    pen.0,
                    rect.left as f32,
                    rect.top as f32,
                    (rect.right - rect.left) as f32,
                    (rect.bottom - rect.top) as f32,
                );
            }
        }
    }

    pub unsafe fn icon(&self, icon: HICON, rect: RECT) {
        unsafe {
            let mut bitmap = null_mut();
            if GdipCreateBitmapFromHICON(icon, &mut bitmap) == Ok {
                GdipDrawImageRectI(
                    self.0,
                    bitmap.cast(),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                );
                GdipDisposeImage(bitmap.cast());
            }
        }
    }
}

/// The first installed icon font, Segoe Fluent Icons on Windows 11.
pub fn icon_face() -> PCWSTR {
    ICON_FONTS
        .into_iter()
        .find(|face| has_family(*face))
        .unwrap_or(ICON_FONTS[0])
}

/// Returns whether DirectWrite can find an installed font family.
pub fn has_family(name: PCWSTR) -> bool {
    TEXT.with(|state| {
        let std::result::Result::Ok(state) = state.try_borrow() else {
            return false;
        };
        let Some(state) = state.as_ref() else {
            return false;
        };
        unsafe {
            let mut collection = None;
            if state
                .factory
                .GetSystemFontCollection(&mut collection, false)
                .is_err()
            {
                return false;
            }
            let Some(collection) = collection else {
                return false;
            };
            let mut index = 0;
            let mut exists = windows_core::BOOL(0);
            collection
                .FindFamilyName(name, &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        }
    })
}

/// Draws one line with the shell's text stack, using the face, weight, and size of
/// the DC's selected font.
pub unsafe fn text(dc: HDC, text: &[u16], rect: RECT, center: bool, color: COLORREF) {
    unsafe { text_clipped(dc, text, rect, rect, center, color) }
}

/// The face, weight, and size of the DC's selected font.
unsafe fn selected_font(dc: HDC) -> Option<LOGFONTW> {
    unsafe {
        let mut font = LOGFONTW::default();
        (GetObjectW(
            GetCurrentObject(dc, OBJ_FONT),
            size_of::<LOGFONTW>() as i32,
            Some((&mut font as *mut LOGFONTW).cast()),
        ) != 0)
            .then_some(font)
    }
}

/// Like [`text`], laid out in `rect` but drawn only inside `clip`, so a row that is
/// partly scrolled out of view keeps its text centered.
pub unsafe fn text_clipped(
    dc: HDC,
    text: &[u16],
    rect: RECT,
    clip: RECT,
    center: bool,
    color: COLORREF,
) {
    unsafe {
        let Some(font) = selected_font(dc) else {
            return;
        };
        let bind = RECT {
            left: rect.left.max(clip.left),
            top: rect.top.max(clip.top),
            right: rect.right.min(clip.right),
            bottom: rect.bottom.min(clip.bottom),
        };
        if bind.right <= bind.left || bind.bottom <= bind.top {
            return;
        }
        TEXT.with(|state| {
            let std::result::Result::Ok(state) = state.try_borrow() else {
                return;
            };
            if let Some(state) = state.as_ref() {
                let _ = draw_text(state, dc, text, (rect, bind), center, color, &font);
            }
        });
    }
}

/// A word-wrapping text format in the given font.
unsafe fn wrap_format(state: &Text, font: &LOGFONTW) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = state.factory.CreateTextFormat(
            PCWSTR(font.lfFaceName.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT(font.lfWeight.clamp(1, 999)),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            font.lfHeight.unsigned_abs().max(1) as f32,
            w!(""),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        std::result::Result::Ok(format)
    }
}

/// Height of `text` wrapped to `width` pixels in the given font.
unsafe fn wrapped_height(state: &Text, text: &[u16], font: &LOGFONTW, width: i32) -> Result<i32> {
    unsafe {
        let format = wrap_format(state, font)?;
        let layout =
            state
                .factory
                .CreateTextLayout(text, &format, width.max(1) as f32, 100_000.0)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut metrics)?;
        std::result::Result::Ok(metrics.height.ceil() as i32)
    }
}

/// Height of `text` wrapped to `width` pixels in the DC's selected font.
pub unsafe fn measure(dc: HDC, text: &[u16], width: i32) -> i32 {
    unsafe {
        let Some(font) = selected_font(dc) else {
            return 0;
        };
        TEXT.with(|state| {
            let std::result::Result::Ok(state) = state.try_borrow() else {
                return 0;
            };
            state
                .as_ref()
                .and_then(|state| wrapped_height(state, text, &font, width).ok())
                .unwrap_or(0)
        })
    }
}

/// Draws word-wrapped `text` with its top-left corner at `left`, `top`, visible only
/// inside `clip`. Pair with [`measure`] for the block's height.
pub unsafe fn paragraph(
    dc: HDC,
    text: &[u16],
    clip: RECT,
    left: i32,
    top: i32,
    width: i32,
    color: COLORREF,
) {
    unsafe {
        let Some(font) = selected_font(dc) else {
            return;
        };
        if clip.right <= clip.left || clip.bottom <= clip.top {
            return;
        }
        TEXT.with(|state| {
            let std::result::Result::Ok(state) = state.try_borrow() else {
                return;
            };
            let Some(state) = state.as_ref() else {
                return;
            };
            let _ = (|| -> Result<()> {
                let format = wrap_format(state, &font)?;
                let _ = GdiFlush();
                state.target.BindDC(dc, &clip)?;
                let channel = |shift: u32| ((color.0 >> shift) & 0xff) as f32 / 255.0;
                let brush = state.target.CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: channel(0),
                        g: channel(8),
                        b: channel(16),
                        a: 1.0,
                    },
                    None,
                )?;
                state.target.BeginDraw();
                let x = (left - clip.left) as f32;
                let y = (top - clip.top) as f32;
                state.target.DrawText(
                    text,
                    &format,
                    &D2D_RECT_F {
                        left: x,
                        top: y,
                        right: x + width as f32,
                        bottom: y + 100_000.0,
                    },
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                state.target.EndDraw(None, None)
            })();
        });
    }
}

unsafe fn draw_text(
    state: &Text,
    dc: HDC,
    text: &[u16],
    (rect, bind): (RECT, RECT),
    center: bool,
    color: COLORREF,
    font: &LOGFONTW,
) -> Result<()> {
    unsafe {
        let format = state.factory.CreateTextFormat(
            PCWSTR(font.lfFaceName.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT(font.lfWeight.clamp(1, 999)),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            font.lfHeight.unsigned_abs().max(1) as f32,
            w!(""),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        format.SetTextAlignment(if center {
            DWRITE_TEXT_ALIGNMENT_CENTER
        } else {
            DWRITE_TEXT_ALIGNMENT_LEADING
        })?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let ellipsis = state.factory.CreateEllipsisTrimmingSign(&format)?;
        format.SetTrimming(
            &DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                ..Default::default()
            },
            &ellipsis,
        )?;
        // Finish queued GDI backgrounds before Direct2D writes to the same DC.
        let _ = GdiFlush();
        state.target.BindDC(dc, &bind)?;
        let channel = |shift: u32| ((color.0 >> shift) & 0xff) as f32 / 255.0;
        let brush = state.target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: channel(0),
                g: channel(8),
                b: channel(16),
                a: 1.0,
            },
            None,
        )?;
        state.target.BeginDraw();
        state.target.DrawText(
            text,
            &format,
            &D2D_RECT_F {
                left: (rect.left - bind.left) as f32,
                top: (rect.top - bind.top) as f32,
                right: (rect.right - bind.left) as f32,
                bottom: (rect.bottom - bind.top) as f32,
            },
            &brush,
            D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        state.target.EndDraw(None, None)
    }
}
