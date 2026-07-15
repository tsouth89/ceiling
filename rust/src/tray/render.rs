//! Pixel-level tray icon renderer, decoupled from any platform icon API.
//!
//! Returns raw RGBA bytes so callers (egui tray manager, Tauri shell, tests)
//! can adapt the result to their own icon type without pulling in extra deps.

use image::{ImageBuffer, Rgba, RgbaImage};

use super::icon::UsageLevel;

/// Side length of the generated tray icon in pixels.
pub const TRAY_ICON_SIZE: u32 = 32;

/// Ceiling brand colour for a usage level: cyan while healthy, warming toward
/// red as capacity fills toward the ceiling.
fn brand_usage_color(percent: f64) -> (u8, u8, u8) {
    match UsageLevel::from_percent(percent) {
        UsageLevel::Low => (53, 194, 218),     // Ceiling cyan
        UsageLevel::Medium => (224, 163, 60),  // amber
        UsageLevel::High => (240, 150, 70),    // orange
        UsageLevel::Critical => (240, 98, 90), // red
        UsageLevel::Unknown => (140, 146, 156),
    }
}

/// Dark app-icon tile colour (matches `rust/icons/icon.*` / the `<CeilingMark>`).
const TILE: (u8, u8, u8) = (15, 18, 22);
/// Brand green for the ceiling line.
const CEILING: (u8, u8, u8) = (166, 227, 92);
/// Track colour behind an unfilled usage bar.
const TRACK: (u8, u8, u8) = (44, 50, 58);

/// Render the compact Ceiling mark used by the Windows notification area.
///
/// The full app icon is reduced to its two defining shapes instead of being
/// scaled down wholesale. A transparent background avoids the bolted-on black
/// tile, while a subtle dark keyline keeps the lime ceiling and pale chamber
/// legible on both light and dark taskbars. Usage belongs in the tooltip and
/// taskbar widget; the tray mark stays calm and recognizable at 16px.
pub fn render_ceiling_tray_icon_rgba(_percent: f64, has_error: bool) -> (Vec<u8>, u32, u32) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let mut img: RgbaImage = ImageBuffer::new(SZ, SZ);
    for pixel in img.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }

    let keyline = Rgba([18, 23, 29, if has_error { 170 } else { 225 }]);
    let (cr, cg, cb) = desat(CEILING, has_error);
    let ceiling = Rgba([cr, cg, cb, 255]);
    let (fr, fg, fb) = desat((225, 232, 238), has_error);
    let chamber = Rgba([fr, fg, fb, 255]);

    fill_rounded_rect(&mut img, 5, 3, 27, 12, 5, keyline);
    fill_rounded_rect(&mut img, 7, 5, 25, 10, 3, ceiling);
    fill_rounded_rect(&mut img, 5, 13, 27, 30, 7, keyline);
    fill_rounded_rect(&mut img, 7, 15, 25, 28, 5, chamber);

    (img.into_raw(), SZ, SZ)
}

fn fill_rounded_rect(
    img: &mut RgbaImage,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    radius: u32,
    color: Rgba<u8>,
) {
    let radius = (radius as f64)
        .min((right.saturating_sub(left)) as f64 / 2.0)
        .min((bottom.saturating_sub(top)) as f64 / 2.0);
    for y in top..bottom {
        for x in left..right {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let nearest_x = px.clamp(left as f64 + radius, right as f64 - radius);
            let nearest_y = py.clamp(top as f64 + radius, bottom as f64 - radius);
            let dx = px - nearest_x;
            let dy = py - nearest_y;
            if dx * dx + dy * dy <= radius * radius {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn desat(rgb: (u8, u8, u8), has_error: bool) -> (u8, u8, u8) {
    if has_error {
        let g = ((rgb.0 as u16 + rgb.1 as u16 + rgb.2 as u16) / 3) as u8;
        (g, g, g)
    } else {
        rgb
    }
}

fn fill_tile(img: &mut RgbaImage, has_error: bool) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let a: u8 = if has_error { 190 } else { 255 };
    let tile = Rgba([TILE.0, TILE.1, TILE.2, a]);
    for y in 1..SZ - 1 {
        for x in 1..SZ - 1 {
            img.put_pixel(x, y, tile);
        }
    }
    // The signature ceiling line near the top.
    let (r, g, b) = desat(CEILING, has_error);
    for y in 6..9 {
        for x in 6..SZ - 6 {
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }
}

/// Render a Ceiling-branded usage tray icon as raw RGBA bytes.
///
/// A dark app-icon tile with the green ceiling line on top and one or two usage
/// bars below, filled in the brand cyan→amber→red ramp.
///
/// - `session_percent`: primary bar fill (0–100)
/// - `weekly_percent`: optional secondary bar fill; two bars when `Some`, one when `None`
/// - `has_error`: desaturate to grey to signal an error/unknown state.
pub fn render_bar_icon_rgba(
    session_percent: f64,
    weekly_percent: Option<f64>,
    has_error: bool,
) -> (Vec<u8>, u32, u32) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let mut img: RgbaImage = ImageBuffer::new(SZ, SZ);
    for pixel in img.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }
    fill_tile(&mut img, has_error);

    let bar_left = 6u32;
    let bar_right = SZ - 6;
    let bar_width = bar_right - bar_left;
    let fill_px = |pct: f64| ((pct.clamp(0.0, 100.0) / 100.0) * bar_width as f64) as u32;
    let track = Rgba([TRACK.0, TRACK.1, TRACK.2, 255]);

    let mut draw_bar = |y_start: u32, y_end: u32, pct: f64| {
        let (r, g, b) = desat(brand_usage_color(pct), has_error);
        let fill_end = (bar_left + fill_px(pct)).min(bar_right);
        for y in y_start..y_end {
            for x in bar_left..bar_right {
                img.put_pixel(x, y, track);
            }
        }
        for y in y_start..y_end {
            for x in bar_left..fill_end {
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            }
        }
    };

    match weekly_percent {
        Some(weekly) => {
            draw_bar(13, 18, session_percent); // session bar (top)
            draw_bar(21, 25, weekly); // weekly bar (bottom)
        }
        None => {
            draw_bar(15, 23, session_percent); // single thick bar
        }
    }

    (img.into_raw(), SZ, SZ)
}

/// Render a Ceiling-branded numeric-percent tray icon as raw RGBA bytes.
pub fn render_percent_icon_rgba(percent: f64, has_error: bool) -> (Vec<u8>, u32, u32) {
    const SZ: u32 = TRAY_ICON_SIZE;
    let mut img: RgbaImage = ImageBuffer::new(SZ, SZ);
    for pixel in img.pixels_mut() {
        *pixel = Rgba([0, 0, 0, 0]);
    }
    fill_tile(&mut img, has_error);

    let pct = percent.clamp(0.0, 100.0).round() as u32;
    let text = if pct >= 100 {
        "100".to_string()
    } else {
        format!("{pct}%")
    };
    let glyph_width = 3u32;
    let glyph_gap = 1u32;
    let scale = if text.len() >= 3 { 2u32 } else { 3u32 };
    let text_width = text.len() as u32 * glyph_width * scale + (text.len() as u32 - 1) * glyph_gap;
    let text_height = 5 * scale;
    let start_x = (SZ.saturating_sub(text_width)) / 2;
    // Sit the number below the ceiling line rather than dead-centre.
    let start_y = ((SZ.saturating_sub(text_height)) / 2 + 3).min(SZ - text_height - 1);

    let (r, g, b) = desat(brand_usage_color(percent), has_error);
    let color = Rgba([r, g, b, 255]);

    let mut x = start_x;
    for ch in text.chars() {
        draw_glyph(&mut img, ch, x, start_y, scale, color);
        x += glyph_width * scale + glyph_gap;
    }

    (img.into_raw(), SZ, SZ)
}

fn draw_glyph(img: &mut RgbaImage, ch: char, x: u32, y: u32, scale: u32, color: Rgba<u8>) {
    let Some(rows) = glyph_rows(ch) else {
        return;
    };
    for (row_idx, row) in rows.iter().enumerate() {
        for col in 0..3 {
            let bit = 1 << (2 - col);
            if row & bit == 0 {
                continue;
            }
            for yy in 0..scale {
                for xx in 0..scale {
                    let px = x + col * scale + xx;
                    let py = y + row_idx as u32 * scale + yy;
                    if px < TRAY_ICON_SIZE && py < TRAY_ICON_SIZE {
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }
}

fn glyph_rows(ch: char) -> Option<[u8; 5]> {
    Some(match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_correct_dimensions() {
        let (rgba, w, h) = render_bar_icon_rgba(50.0, None, false);
        assert_eq!(w, TRAY_ICON_SIZE);
        assert_eq!(h, TRAY_ICON_SIZE);
        assert_eq!(rgba.len() as u32, w * h * 4);
    }

    #[test]
    fn compact_ceiling_icon_is_transparent_and_legible() {
        let (rgba, w, h) = render_ceiling_tray_icon_rgba(72.0, false);
        assert_eq!((w, h), (TRAY_ICON_SIZE, TRAY_ICON_SIZE));
        assert_eq!(rgba.len() as u32, w * h * 4);
        assert_eq!(rgba[3], 0, "top-left corner should remain transparent");
        assert!(
            rgba.chunks_exact(4)
                .any(|pixel| { pixel[3] == 255 && (pixel[0], pixel[1], pixel[2]) == CEILING })
        );
        assert!(
            rgba.chunks_exact(4).any(|pixel| {
                pixel[3] == 255 && (pixel[0], pixel[1], pixel[2]) == (225, 232, 238)
            })
        );
    }

    #[test]
    fn compact_ceiling_icon_stays_brand_first_across_usage_levels() {
        let (low, _, _) = render_ceiling_tray_icon_rgba(12.0, false);
        let (high, _, _) = render_ceiling_tray_icon_rgba(91.0, false);
        assert_eq!(low, high);
    }

    #[test]
    fn render_two_bar_has_correct_size() {
        let (rgba, w, h) = render_bar_icon_rgba(30.0, Some(60.0), false);
        assert_eq!(rgba.len() as u32, w * h * 4);
    }

    #[test]
    fn zero_fill_gives_track_only_bar() {
        let (rgba, w, _h) = render_bar_icon_rgba(0.0, None, false);
        // Sample a pixel in the single bar's track area (y=16, x=8).
        let idx = ((16 * w + 8) * 4) as usize;
        assert_eq!((rgba[idx], rgba[idx + 1], rgba[idx + 2]), TRACK);
    }

    #[test]
    fn full_fill_gives_colored_bar() {
        let (rgba, w, _h) = render_bar_icon_rgba(100.0, None, false);
        // At 100% used the bar fills with the brand critical colour.
        let idx = ((16 * w + 8) * 4) as usize;
        let (er, eg, eb) = brand_usage_color(100.0);
        assert_eq!(rgba[idx], er);
        assert_eq!(rgba[idx + 1], eg);
        assert_eq!(rgba[idx + 2], eb);
    }

    #[test]
    fn error_state_desaturates_colors() {
        let (normal, _, _) = render_bar_icon_rgba(100.0, None, false);
        let (error, _, _) = render_bar_icon_rgba(100.0, None, true);
        // In error mode all three channels at the filled bar pixel should be equal (grey)
        let idx = ((16 * 32 + 8) * 4) as usize;
        assert_ne!(normal[idx], normal[idx + 1]); // colour has distinct channels
        assert_eq!(error[idx], error[idx + 1]); // grey: R == G
        assert_eq!(error[idx + 1], error[idx + 2]); // grey: G == B
    }

    #[test]
    fn percent_icon_produces_correct_dimensions() {
        let (rgba, w, h) = render_percent_icon_rgba(72.0, false);
        assert_eq!(w, TRAY_ICON_SIZE);
        assert_eq!(h, TRAY_ICON_SIZE);
        assert_eq!(rgba.len() as u32, w * h * 4);
    }

    #[test]
    fn percent_icon_draws_visible_text() {
        let (rgba, _, _) = render_percent_icon_rgba(72.0, false);
        // A visible glyph pixel: opaque, and neither the tile nor the ceiling line.
        assert!(
            rgba.chunks_exact(4)
                .any(|px| px[3] == 255 && px[0] != TILE.0 && px[0] != CEILING.0)
        );
    }

    #[test]
    fn percent_icon_clamps_to_hundred() {
        let (rgba, w, h) = render_percent_icon_rgba(125.0, false);
        assert_eq!(rgba.len() as u32, w * h * 4);
    }
}
