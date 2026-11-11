//! Рисование картинок: фоны, скруглённые фигуры, текст, аватарки. То же, что делают функции с Pillow в bot.py
//! (gradient, paste_rounded, make_background, make_avatar, fit_text, progress_bar).

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::{imageops, Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub type Rgb = (u8, u8, u8);

pub const BG_LEFT: Rgb = (31, 27, 75); // тёмно-фиолетовый — левый край фона
pub const BG_RIGHT: Rgb = (12, 18, 38); // почти чёрно-синий — правый край фона
pub const ACCENT_1: Rgb = (139, 92, 246); // фиолетовый акцент
pub const ACCENT_2: Rgb = (236, 72, 153); // розовый акцент
pub const WHITE: Rgb = (255, 255, 255);
pub const TEXT_DIM: Rgb = (165, 170, 220); // приглушённый текст (подписи)
pub const GOLD: Rgb = (251, 191, 36);
pub const SILVER: Rgb = (203, 213, 225);
pub const BRONZE: Rgb = (217, 119, 6);

fn rgba(color: Rgb, alpha: u8) -> Rgba<u8> {
    Rgba([color.0, color.1, color.2, alpha])
}

// ---------- Шрифты ----------

/// Обычный и жирный шрифт с русскими буквами. Берём первый, который найдётся на компьютере;
/// можно положить в папку бота свои font.ttf и font-bold.ttf — тогда возьмутся они.
pub struct Fonts {
    pub regular: FontVec,
    pub bold: FontVec,
}

static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();

fn load_font(candidates: &[PathBuf]) -> Option<FontVec> {
    candidates.iter().find_map(|path| FontVec::try_from_vec(std::fs::read(path).ok()?).ok())
}

/// Шрифты бота (загружаются один раз). None — на компьютере не нашлось ни одного подходящего шрифта.
pub fn fonts(bot_dir: &Path) -> Option<&'static Fonts> {
    FONTS
        .get_or_init(|| {
            let paths = |own: &str, system: &[&str]| -> Vec<PathBuf> {
                std::iter::once(bot_dir.join(own)).chain(system.iter().map(PathBuf::from)).collect()
            };
            let bold = load_font(&paths("font-bold.ttf", &[
                "C:/Windows/Fonts/segoeuib.ttf",
                "C:/Windows/Fonts/arialbd.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            ]))?;
            let regular = load_font(&paths("font.ttf", &[
                "C:/Windows/Fonts/segoeui.ttf",
                "C:/Windows/Fonts/arial.ttf",
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            ]))?;
            Some(Fonts { regular, bold })
        })
        .as_ref()
}

/// Где на картинке «прикрепить» текст к точке (как anchor в Pillow).
#[derive(Clone, Copy, PartialEq)]
pub enum Anchor {
    LeftBaseline,  // "ls": точка — левый край строки, на линии, где стоят буквы
    RightBaseline, // "rs": правый край строки
    Center,        // "mm": середина строки по ширине и высоте
}

/// Размер в пикселях, как его понимает Pillow (высота «кегельной площадки»), → масштаб ab_glyph.
fn scale(font: &FontVec, size: f32) -> PxScale {
    PxScale::from(size * font.height_unscaled() / font.units_per_em().unwrap_or(1000.0))
}

/// Ширина строки в пикселях.
pub fn text_width(font: &FontVec, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(scale(font, size));
    let mut width = 0.0;
    let mut previous = None;
    for character in text.chars() {
        let glyph = scaled.glyph_id(character);
        if let Some(previous) = previous {
            width += scaled.kern(previous, glyph);
        }
        width += scaled.h_advance(glyph);
        previous = Some(glyph);
    }
    width
}

/// Укорачивает текст с «…» на конце, если он не помещается в max_width пикселей.
pub fn fit_text(font: &FontVec, size: f32, text: &str, max_width: f32) -> String {
    if text_width(font, size, text) <= max_width {
        return text.to_string();
    }
    let mut characters: Vec<char> = text.chars().collect();
    while !characters.is_empty() {
        let shorter: String = characters.iter().collect::<String>() + "…";
        if text_width(font, size, &shorter) <= max_width {
            return shorter;
        }
        characters.pop();
    }
    "…".to_string()
}

fn blend(image: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>, coverage: f32) {
    if x < 0 || y < 0 || x >= image.width() as i32 || y >= image.height() as i32 {
        return;
    }
    let alpha = (color[3] as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let pixel = image.get_pixel_mut(x as u32, y as u32);
    let under = pixel[3] as f32 / 255.0;
    let out = alpha + under * (1.0 - alpha);
    for channel in 0..3 {
        let mixed = (color[channel] as f32 * alpha + pixel[channel] as f32 * under * (1.0 - alpha)) / out;
        pixel[channel] = mixed.round() as u8;
    }
    pixel[3] = (out * 255.0).round() as u8;
}

/// Пишет текст. (x, y) — точка привязки, anchor — какой частью строки к ней прикрепиться.
pub fn text(image: &mut RgbaImage, at: (f32, f32), anchor: Anchor, font: &FontVec, size: f32, color: Rgb, text: &str) {
    let scaled = font.as_scaled(scale(font, size));
    let width = text_width(font, size, text);
    let (mut x, baseline) = match anchor {
        Anchor::LeftBaseline => (at.0, at.1),
        Anchor::RightBaseline => (at.0 - width, at.1),
        // середина по высоте — между верхом заглавных букв и низом «хвостиков»
        Anchor::Center => (at.0 - width / 2.0, at.1 + (scaled.ascent() + scaled.descent()) / 2.0),
    };
    let color = rgba(color, 255);
    let mut previous = None;
    for character in text.chars() {
        let id = scaled.glyph_id(character);
        if let Some(previous) = previous {
            x += scaled.kern(previous, id);
        }
        let glyph = id.with_scale_and_position(scaled.scale(), ab_glyph::point(x, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|dx, dy, coverage| {
                blend(image, bounds.min.x as i32 + dx as i32, bounds.min.y as i32 + dy as i32, color, coverage);
            });
        }
        x += scaled.h_advance(id);
        previous = Some(id);
    }
}

// ---------- Фигуры ----------

/// Насколько точка (центр пикселя) внутри прямоугольника w×h со скруглением radius: 1 — внутри, 0 — снаружи,
/// между ними — гладкий край.
fn rounded_coverage(px: f32, py: f32, w: f32, h: f32, radius: f32) -> f32 {
    let radius = radius.min(w / 2.0).min(h / 2.0);
    let (dx, dy) = ((px - w / 2.0).abs() - (w / 2.0 - radius), (py - h / 2.0).abs() - (h / 2.0 - radius));
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    let distance = outside + dx.max(dy).min(0.0) - radius;
    (0.5 - distance).clamp(0.0, 1.0)
}

/// Плавный переход цвета слева направо.
pub fn gradient(width: u32, height: u32, left: Rgb, right: Rgb) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, _| {
        let t = if width > 1 { x as f32 / (width - 1) as f32 } else { 0.0 };
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgba([mix(left.0, right.0), mix(left.1, right.1), mix(left.2, right.2), 255])
    })
}

/// Накладывает picture на base в точку (x, y), скругляя углы.
pub fn paste_rounded(base: &mut RgbaImage, picture: &RgbaImage, at: (i32, i32), radius: f32) {
    let (w, h) = (picture.width() as f32, picture.height() as f32);
    for (px, py, pixel) in picture.enumerate_pixels() {
        let coverage = rounded_coverage(px as f32 + 0.5, py as f32 + 0.5, w, h, radius);
        blend(base, at.0 + px as i32, at.1 + py as i32, *pixel, coverage);
    }
}

/// Закрашенный прямоугольник со скруглёнными углами. area — (x, y, ширина, высота).
pub fn fill_rounded(base: &mut RgbaImage, area: (i32, i32, u32, u32), radius: f32, color: Rgb, alpha: u8) {
    let (w, h) = (area.2 as f32, area.3 as f32);
    for py in 0..area.3 {
        for px in 0..area.2 {
            let coverage = rounded_coverage(px as f32 + 0.5, py as f32 + 0.5, w, h, radius);
            blend(base, area.0 + px as i32, area.1 + py as i32, rgba(color, alpha), coverage);
        }
    }
}

/// Рамка (только контур) скруглённого прямоугольника толщиной width.
pub fn outline_rounded(base: &mut RgbaImage, area: (i32, i32, u32, u32), radius: f32, width: f32, color: Rgb, alpha: u8) {
    let (w, h) = (area.2 as f32, area.3 as f32);
    for py in 0..area.3 {
        for px in 0..area.2 {
            let outer = rounded_coverage(px as f32 + 0.5, py as f32 + 0.5, w, h, radius);
            let inner = rounded_coverage(
                px as f32 + 0.5 - width, py as f32 + 0.5 - width, w - 2.0 * width, h - 2.0 * width, (radius - width).max(0.0));
            blend(base, area.0 + px as i32, area.1 + py as i32, rgba(color, alpha), outer - inner);
        }
    }
}

/// Полоса прогресса. area — (x, y, ширина, высота), ratio — заполненность от 0 до 1.
pub fn progress_bar(base: &mut RgbaImage, area: (i32, i32, u32, u32), ratio: f64) {
    let radius = (area.3 / 2) as f32;
    fill_rounded(base, area, radius, WHITE, 35); // подложка — полупрозрачная белая
    let filled = (area.2 as f64 * ratio.clamp(0.0, 1.0)) as u32;
    if filled > 0 {
        let filled = filled.max(area.3); // даже маленький прогресс выглядит как аккуратный кружок
        paste_rounded(base, &gradient(filled, area.3, ACCENT_1, ACCENT_2), (area.0, area.1), radius);
    }
}

// ---------- Картинки ----------

/// Обрезает и подгоняет картинку под размер, сохраняя пропорции (лишнее отрезается по краям).
/// focus — какую часть оставить: (0.5, 0.5) — центр, (0.5, 0.3) — ближе к верху.
pub fn cover(picture: &RgbaImage, width: u32, height: u32, focus: (f32, f32)) -> RgbaImage {
    let (pw, ph) = (picture.width() as f32, picture.height() as f32);
    let ratio = width as f32 / height as f32;
    let (cw, ch) = if pw / ph > ratio { (ph * ratio, ph) } else { (pw, pw / ratio) };
    let (x, y) = ((pw - cw) * focus.0, (ph - ch) * focus.1);
    let cropped = imageops::crop_imm(picture, x as u32, y as u32, cw.max(1.0) as u32, ch.max(1.0) as u32).to_image();
    imageops::resize(&cropped, width, height, imageops::FilterType::Lanczos3)
}

/// Круглая аватарка. Если картинки нет или она не открылась — цветной круг с первой буквой имени.
pub fn avatar(bytes: Option<&[u8]>, diameter: u32, name: &str, fonts: &Fonts) -> RgbaImage {
    let picture = bytes.and_then(|bytes| image::load_from_memory(bytes).ok()).map(|picture| {
        cover(&picture.to_rgba8(), diameter, diameter, (0.5, 0.5))
    });
    let picture = picture.unwrap_or_else(|| {
        let mut circle = gradient(diameter, diameter, ACCENT_1, ACCENT_2);
        let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".to_string());
        let center = (diameter as f32 / 2.0, diameter as f32 / 2.0);
        text(&mut circle, center, Anchor::Center, &fonts.bold, (diameter / 2) as f32, WHITE, &letter);
        circle
    });
    let mut result = RgbaImage::new(diameter, diameter);
    paste_rounded(&mut result, &picture, (0, 0), diameter as f32 / 2.0);
    result
}

/// Круглая иконка с треугольником «play» — вместо аватарки в топе треков.
pub fn music_icon(diameter: u32) -> RgbaImage {
    let size = diameter as f32;
    let mut icon = gradient(diameter, diameter, ACCENT_1, ACCENT_2);
    // Треугольник с вершинами (0.40, 0.30), (0.40, 0.70), (0.72, 0.50) — чуть сдвинут вправо, так он выглядит по центру
    for y in 0..diameter {
        for x in 0..diameter {
            let (px, py) = ((x as f32 + 0.5) / size, (y as f32 + 0.5) / size);
            let half = 0.20 * (0.72 - px) / 0.32; // половина высоты треугольника на этой вертикали
            if (0.40..=0.72).contains(&px) && (py - 0.50).abs() <= half {
                icon.put_pixel(x, y, rgba(WHITE, 255));
            }
        }
    }
    let mut result = RgbaImage::new(diameter, diameter);
    paste_rounded(&mut result, &icon, (0, 0), size / 2.0);
    result
}

/// Путь к файлу фона (rank_bg, leaderboard_bg, money_bg…) в папке бота или None, если такого файла нет.
pub fn pick_background(bot_dir: &Path, name: &str) -> Option<PathBuf> {
    ["jpg", "jpeg", "png", "webp"].iter().map(|extension| bot_dir.join(format!("{name}.{extension}"))).find(|path| path.exists())
}

/// Фон карточки: фото (если передан путь к нему) или градиент со свечением.
/// darkness — насколько затемнить фото (0–255), top_shade — высота тёмной подложки сверху под заголовок.
/// Возвращает (картинка со скруглёнными углами, есть ли фото).
pub fn background(
    width: u32,
    height: u32,
    radius: f32,
    photo: Option<&Path>,
    focus: (f32, f32),
    darkness: u8,
    top_shade: u32,
) -> (RgbaImage, bool) {
    let photo = photo.and_then(|path| match image::open(path) {
        Ok(picture) => Some(picture.to_rgba8()),
        Err(error) => {
            println!("Не получилось открыть фон {}: {error}", path.display());
            None
        }
    });
    let has_photo = photo.is_some();
    let mut canvas = match photo {
        Some(picture) => {
            let mut canvas = cover(&picture, width, height, focus);
            for pixel in canvas.pixels_mut() {
                pixel[3] = 255;
            }
            // Затемняем фото, чтобы на нём читался белый текст
            for y in 0..height {
                // Подложка под заголовок: сверху плотная (190 из 255), книзу плавно исчезает
                let shade = if y < top_shade { 190.0 * (1.0 - y as f32 / top_shade as f32) } else { 0.0 };
                for x in 0..width {
                    blend(&mut canvas, x as i32, y as i32, Rgba([8, 10, 24, darkness]), 1.0);
                    if shade > 0.0 {
                        blend(&mut canvas, x as i32, y as i32, Rgba([8, 10, 24, shade as u8]), 1.0);
                    }
                }
            }
            canvas
        }
        None => {
            let mut canvas = gradient(width, height, BG_LEFT, BG_RIGHT);
            // Два размытых цветных пятна: фиолетовое справа сверху и розовое слева снизу
            let mut glow = RgbaImage::new(width, height);
            let (w, h) = (width as f32, height as f32);
            let spots = [((w - 320.0, -200.0, w + 120.0, 200.0), rgba(ACCENT_1, 110)), ((-150.0, h - 150.0, 300.0, h + 250.0), rgba(ACCENT_2, 80))];
            for (x, y, pixel) in glow.enumerate_pixels_mut() {
                for ((x1, y1, x2, y2), color) in spots {
                    let (cx, cy, rx, ry) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0, (x2 - x1) / 2.0, (y2 - y1) / 2.0);
                    if ((x as f32 - cx) / rx).powi(2) + ((y as f32 - cy) / ry).powi(2) <= 1.0 {
                        *pixel = color;
                    }
                }
            }
            let glow = imageops::fast_blur(&glow, 70.0);
            imageops::overlay(&mut canvas, &glow, 0, 0);
            canvas
        }
    };
    // Тонкая светлая рамка по краю
    outline_rounded(&mut canvas, (1, 1, width - 2, height - 2), radius, 2.0, WHITE, 30);
    let mut result = RgbaImage::new(width, height);
    paste_rounded(&mut result, &canvas, (0, 0), radius);
    (result, has_photo)
}

/// Картинка → байты PNG, которые можно отправить в Discord.
pub fn to_png(picture: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    if let Err(error) = picture.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png) {
        println!("Не получилось сохранить картинку: {error}");
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_edges() {
        // центр — внутри, угол квадрата 100×100 с радиусом 20 — снаружи, середина стороны — внутри
        assert_eq!(rounded_coverage(50.0, 50.0, 100.0, 100.0, 20.0), 1.0);
        assert_eq!(rounded_coverage(0.5, 0.5, 100.0, 100.0, 20.0), 0.0);
        assert_eq!(rounded_coverage(50.0, 1.5, 100.0, 100.0, 20.0), 1.0);
        assert_eq!(rounded_coverage(150.0, 50.0, 100.0, 100.0, 20.0), 0.0);
        // круг: точка на расстоянии радиуса от центра — наполовину закрашена
        let edge = rounded_coverage(50.0 + 50.0, 50.0, 100.0, 100.0, 50.0);
        assert!((edge - 0.5).abs() < 0.01, "{edge}");
    }

    #[test]
    fn gradient_ends() {
        let picture = gradient(10, 2, (0, 0, 0), (255, 100, 50));
        assert_eq!(picture.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(picture.get_pixel(9, 1).0, [255, 100, 50, 255]);
    }

    #[test]
    fn blending() {
        let mut picture = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 255]));
        blend(&mut picture, 0, 0, Rgba([255, 255, 255, 255]), 0.5);
        let mixed = picture.get_pixel(0, 0).0;
        assert!((126..=129).contains(&mixed[0]) && mixed[3] == 255, "{mixed:?}");
        blend(&mut picture, 5, 5, Rgba([255, 255, 255, 255]), 1.0); // за краем — ничего не ломается
        let mut clear = RgbaImage::new(1, 1);
        blend(&mut clear, 0, 0, Rgba([10, 20, 30, 255]), 1.0);
        assert_eq!(clear.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn cover_keeps_size() {
        let wide = RgbaImage::from_pixel(400, 100, Rgba([1, 2, 3, 255]));
        let fitted = cover(&wide, 50, 50, (0.5, 0.5));
        assert_eq!((fitted.width(), fitted.height()), (50, 50));
        let tall = RgbaImage::from_pixel(100, 400, Rgba([1, 2, 3, 255]));
        assert_eq!(cover(&tall, 60, 30, (0.5, 0.3)).dimensions(), (60, 30));
    }
}
