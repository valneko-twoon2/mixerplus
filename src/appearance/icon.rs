//! The generated tray icon.

use super::*;

/// Renders one glyph and returns its per-pixel coverage. It is drawn in the final
/// foreground over the matching taskbar shade so text contrast tuning applies the
/// same way as for the system icons.
unsafe fn glyph_coverage(
    size: i32,
    glyph: u16,
    em: f32,
    rect: RECT,
    dark: bool,
) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.into());
        if !dark {
            std::ptr::write_bytes(bits.cast::<u8>(), 0xff, (size * size * 4) as usize);
        }
        let font = CreateFontW(
            -(em.round() as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            crate::drawing::icon_face(),
        );
        let previous_font = SelectObject(dc, font.into());
        let ink = if dark { 255 } else { 0 };
        crate::drawing::text(dc, &[glyph], rect, true, rgb(ink, ink, ink));
        let _ = GdiFlush();
        let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), (size * size * 4) as usize)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| if dark { pixel[2] } else { 255 - pixel[2] })
            .collect();
        SelectObject(dc, previous_font);
        SelectObject(dc, previous);
        let _ = DeleteObject(font.into());
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Some(pixels)
    }
}

/// The taskbar speaker glyph in the theme's accent color, with the weight of the
/// system icons for the taskbar's light or dark shade.
pub fn tray_icon(size: i32) -> Option<HICON> {
    let palette = PALETTE.get();
    let dark = palette.dark;
    unsafe {
        let full = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        let alpha = glyph_coverage(size, 0xe995, size as f32, full, dark)?;
        let accent = palette.accent;
        let (red, green, blue) = (
            accent.0 as u8,
            (accent.0 >> 8) as u8,
            (accent.0 >> 16) as u8,
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (size * size * 4) as usize);
        for (pixel, alpha) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
            pixel.copy_from_slice(&[blue, green, red, *alpha]);
        }
        let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: color,
            ..Default::default()
        });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok()
    }
}
