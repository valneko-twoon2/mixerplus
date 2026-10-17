//! Antialiased text, vector shapes, and high-quality icon resampling.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::null_mut,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{DirectWrite::*, Gdi::*, GdiPlus::*},
        UI::WindowsAndMessaging::HICON,
    },
    core::{BOOL, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, Result, implement, w},
};

/// The Windows 11 shell font. DirectWrite falls back to Yu Gothic UI for Japanese
/// like the quick settings flyouts, whereas GDI font linking would pick Meiryo UI.
pub const FONT: PCWSTR = w!("Segoe UI Variable Text");
/// Windows 11 icon font, with the Windows 10 font that shares its code points.
pub const ICON_FONTS: [PCWSTR; 2] = [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")];

/// How a cached text format lays out its line.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Flow {
    Leading,
    Centered,
    Wrapped,
}

/// Face, weight, pixel size, and flow of a cached text format.
type FormatKey = ([u16; 32], i32, u32, Flow);

struct Text {
    factory: IDWriteFactory,
    // Creating a format per string was a large part of text drawing; the windows use
    // only a handful of fonts, so formats are kept for the life of the factory.
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    /// DirectWrite's GDI-compatible bitmap, which text is composed into. A Direct2D DC
    /// render target spent far longer binding and flushing per string than drawing.
    target: IDWriteBitmapRenderTarget,
    /// Pixel size of `target`, which only grows.
    size: Cell<(i32, i32)>,
    renderer: windows_core::ComObject<Glyphs>,
}

/// Draws the glyph runs of a text layout into the bitmap target.
#[implement(IDWriteTextRenderer)]
struct Glyphs {
    target: IDWriteBitmapRenderTarget,
    params: IDWriteRenderingParams,
    color: Cell<COLORREF>,
}

impl IDWritePixelSnapping_Impl for Glyphs_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const c_void) -> Result<BOOL> {
        std::result::Result::Ok(false.into())
    }

    fn GetCurrentTransform(&self, _: *const c_void, transform: *mut DWRITE_MATRIX) -> Result<()> {
        unsafe { self.target.GetCurrentTransform(transform) }
    }

    fn GetPixelsPerDip(&self, _: *const c_void) -> Result<f32> {
        std::result::Result::Ok(unsafe { self.target.GetPixelsPerDip() })
    }
}

impl IDWriteTextRenderer_Impl for Glyphs_Impl {
    fn DrawGlyphRun(
        &self,
        _: *const c_void,
        x: f32,
        y: f32,
        mode: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        unsafe {
            self.target
                .DrawGlyphRun(x, y, mode, run, &self.params, self.color.get(), None)
        }
    }

    fn DrawUnderline(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        std::result::Result::Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _: *const c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        std::result::Result::Ok(())
    }

    /// The ellipsis that trims long lines is an inline object; it draws back through
    /// this renderer.
    fn DrawInlineObject(
        &self,
        context: *const c_void,
        x: f32,
        y: f32,
        object: Ref<IDWriteInlineObject>,
        sideways: BOOL,
        right_to_left: BOOL,
        effect: Ref<IUnknown>,
    ) -> Result<()> {
        let object = object.ok()?;
        unsafe {
            object.Draw(
                Some(context),
                &*self.as_interface::<IDWriteTextRenderer>(),
                x,
                y,
                sideways.as_bool(),
                right_to_left.as_bool(),
                effect.as_ref(),
            )
        }
    }
}

impl Text {
    unsafe fn format(&self, font: &LOGFONTW, flow: Flow) -> Result<IDWriteTextFormat> {
        let size = font.lfHeight.unsigned_abs().max(1);
        let weight = font.lfWeight.clamp(1, 999);
        let key = (font.lfFaceName, weight, size, flow);
        if let Some(format) = self.formats.borrow().get(&key) {
            return std::result::Result::Ok(format.clone());
        }
        unsafe {
            let format = self.factory.CreateTextFormat(
                PCWSTR(font.lfFaceName.as_ptr()),
                None,
                DWRITE_FONT_WEIGHT(weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size as f32,
                w!(""),
            )?;
            if flow == Flow::Wrapped {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            } else {
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                format.SetTextAlignment(if flow == Flow::Centered {
                    DWRITE_TEXT_ALIGNMENT_CENTER
                } else {
                    DWRITE_TEXT_ALIGNMENT_LEADING
                })?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let ellipsis = self.factory.CreateEllipsisTrimmingSign(&format)?;
                format.SetTrimming(
                    &DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        ..Default::default()
                    },
                    &ellipsis,
                )?;
            }
            self.formats.borrow_mut().insert(key, format.clone());
            std::result::Result::Ok(format)
        }
    }

    /// Draws `layout` with its origin at `x`, `y` inside `clip`, a rectangle in the
    /// DC's logical coordinates; text outside `clip` is cut off.
    unsafe fn draw(
        &self,
        dc: HDC,
        layout: &IDWriteTextLayout,
        clip: RECT,
        (x, y): (f32, f32),
        color: COLORREF,
    ) -> Result<()> {
        let width = clip.right - clip.left;
        let height = clip.bottom - clip.top;
        unsafe {
            let (current_width, current_height) = self.size.get();
            if width > current_width || height > current_height {
                let size = (width.max(current_width), height.max(current_height));
                self.target.Resize(size.0 as u32, size.1 as u32)?;
                self.size.set(size);
            }
            // Text is blended over what is already drawn, so the bitmap starts as a copy.
            let memory = self.target.GetMemoryDC();
            BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(dc),
                clip.left,
                clip.top,
                SRCCOPY,
            )?;
            // DirectWrite writes the bitmap directly, after any queued GDI work.
            let _ = GdiFlush();
            self.renderer.color.set(color);
            layout.Draw(
                None,
                &*self.renderer.as_interface::<IDWriteTextRenderer>(),
                x - clip.left as f32,
                y - clip.top as f32,
            )?;
            BitBlt(
                dc,
                clip.left,
                clip.top,
                width,
                height,
                Some(memory),
                0,
                0,
                SRCCOPY,
            )
        }
    }
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
            let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let target = factory
                .GetGdiInterop()?
                .CreateBitmapRenderTarget(None, 1, 1)?;
            // Grayscale antialiasing as the shell flyouts use, with the system's tuning.
            target
                .cast::<IDWriteBitmapRenderTarget1>()?
                .SetTextAntialiasMode(DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE)?;
            // Coordinates and font sizes are already in device pixels.
            target.SetPixelsPerDip(1.0)?;
            let renderer = windows_core::ComObject::new(Glyphs {
                target: target.clone(),
                params: factory.CreateRenderingParams()?,
                color: Cell::new(COLORREF(0)),
            });
            Text {
                factory,
                formats: RefCell::default(),
                target,
                size: Cell::new((1, 1)),
                renderer,
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
        let format = state.format(
            font,
            if center {
                Flow::Centered
            } else {
                Flow::Leading
            },
        )?;
        let layout = state.factory.CreateTextLayout(
            text,
            &format,
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
        )?;
        state.draw(
            dc,
            &layout,
            bind,
            (rect.left as f32, rect.top as f32),
            color,
        )
    }
}
