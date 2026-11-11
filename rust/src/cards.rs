//! Готовые картинки: карточка уровня (/rank) и таблицы-рейтинги. Раскладка — как у make_rank_card
//! и make_table_image в bot.py, пиксель в пиксель по координатам.

use crate::draw::{self, Anchor, Fonts};
use crate::text::number;
use image::RgbaImage;
use std::path::Path;

const RANK_DARKNESS: u8 = 140; // насколько затемнять фото на карточке уровня
const LEADERBOARD_DARKNESS: u8 = 100; // и в таблицах (там строки и так на тёмных плашках)

/// Карточка уровня 1000×300. position — место в рейтинге (None — опыта ещё нет).
#[allow(clippy::too_many_arguments)]
pub fn rank_card(
    fonts: &Fonts,
    name: &str,
    avatar: Option<&[u8]>,
    level: i64,
    current_xp: i64,
    need_xp: i64,
    total_xp: i64,
    position: Option<i64>,
    background: Option<&Path>,
) -> RgbaImage {
    let (mut card, _) = draw::background(1000, 300, 36.0, background, (0.5, 0.45), RANK_DARKNESS, 0);

    // Аватарка с цветным кольцом и тёмным зазором
    draw::paste_rounded(&mut card, &draw::gradient(216, 216, draw::ACCENT_1, draw::ACCENT_2), (42, 42), 108.0);
    draw::fill_rounded(&mut card, (48, 48, 204, 204), 102.0, draw::BG_LEFT, 255);
    image::imageops::overlay(&mut card, &draw::avatar(avatar, 192, name, fonts), 54, 54);

    let (left, right) = (290.0, 950.0); // левая и правая граница текстовой части
    let (bold, regular) = (&fonts.bold, &fonts.regular);

    // Справа сверху: РАНГ #N  УРОВЕНЬ N
    let top_line = 98.0;
    let level_text = level.to_string();
    draw::text(&mut card, (right, top_line), Anchor::RightBaseline, bold, 54.0, draw::ACCENT_2, &level_text);
    let mut x = right - draw::text_width(bold, 54.0, &level_text) - 12.0;
    draw::text(&mut card, (x, top_line), Anchor::RightBaseline, bold, 22.0, draw::TEXT_DIM, "УРОВЕНЬ");
    x -= draw::text_width(bold, 22.0, "УРОВЕНЬ") + 36.0;
    let rank_text = position.map_or("—".to_string(), |position| format!("#{position}"));
    draw::text(&mut card, (x, top_line), Anchor::RightBaseline, bold, 54.0, draw::WHITE, &rank_text);
    x -= draw::text_width(bold, 54.0, &rank_text) + 12.0;
    draw::text(&mut card, (x, top_line), Anchor::RightBaseline, bold, 22.0, draw::TEXT_DIM, "РАНГ");

    // Строка над полосой: справа прогресс уровня, слева имя
    let middle_line = 166.0;
    let need_text = format!(" / {} XP", number(need_xp));
    draw::text(&mut card, (right, middle_line), Anchor::RightBaseline, regular, 24.0, draw::TEXT_DIM, &need_text);
    let mut x = right - draw::text_width(regular, 24.0, &need_text);
    draw::text(&mut card, (x, middle_line), Anchor::RightBaseline, bold, 26.0, draw::WHITE, &number(current_xp));
    x -= draw::text_width(bold, 26.0, &number(current_xp)) + 30.0; // левая граница блока с опытом
    let shown_name = draw::fit_text(bold, 44.0, name, x - left); // укорачиваем, если не влезает
    draw::text(&mut card, (left, middle_line), Anchor::LeftBaseline, bold, 44.0, draw::WHITE, &shown_name);

    // Полоса прогресса до следующего уровня и подписи под ней
    draw::progress_bar(&mut card, (290, 184, 660, 36), current_xp as f64 / need_xp.max(1) as f64);
    draw::text(&mut card, (left, 256.0), Anchor::LeftBaseline, regular, 21.0, draw::TEXT_DIM,
               &format!("Всего опыта: {} XP", number(total_xp)));
    draw::text(&mut card, (right, 256.0), Anchor::RightBaseline, regular, 21.0, draw::TEXT_DIM,
               &format!("До {} уровня: {} XP", level + 1, number(need_xp - current_xp)));
    card
}

/// Что показать слева в строке таблицы.
pub enum Picture {
    Avatar(Option<Vec<u8>>), // аватарка (None — кружок с буквой)
    #[cfg_attr(not(feature = "voice"), allow(dead_code))] // топ треков месяца
    Music, // иконка «play»
}

/// Строка таблицы-рейтинга.
pub struct Row {
    pub name: String,     // главный текст строки (имя или название трека)
    pub picture: Picture,
    pub progress: f64,    // заполненность полоски под именем, от 0 до 1
    pub pill: String,     // текст на цветной плашке справа («УР. 12»)
    pub hint: String,     // маленькая подпись под плашкой («15 430 XP»)
}

/// Общая картинка для всех рейтингов. right_hint — подпись под right_title (пустая — не рисуется).
pub fn table(
    fonts: &Fonts,
    title: &str,
    subtitle: &str,
    right_title: &str,
    right_hint: &str,
    rows: &[Row],
    background: Option<&Path>,
) -> RgbaImage {
    let width = 1000u32;
    let (header, row_height, gap) = (170i32, 84i32, 12i32); // высота шапки, строки и промежуток между строками
    let height = (header + rows.len() as i32 * (row_height + gap) + 28) as u32;
    let (mut board, has_photo) =
        draw::background(width, height, 36.0, background, (0.5, 0.25), LEADERBOARD_DARKNESS, (header + 30) as u32);
    let (bold, regular) = (&fonts.bold, &fonts.regular);

    // Шапка
    draw::text(&mut board, (50.0, 92.0), Anchor::LeftBaseline, bold, 50.0, draw::WHITE, title);
    let subtitle = draw::fit_text(regular, 26.0, subtitle, 640.0);
    draw::text(&mut board, (52.0, 132.0), Anchor::LeftBaseline, regular, 26.0, draw::TEXT_DIM, &subtitle);
    draw::text(&mut board, (950.0, 92.0), Anchor::RightBaseline, bold, 34.0, draw::ACCENT_2, right_title);
    if !right_hint.is_empty() {
        draw::text(&mut board, (950.0, 132.0), Anchor::RightBaseline, regular, 22.0, draw::TEXT_DIM, right_hint);
    }
    draw::paste_rounded(&mut board, &draw::gradient(width - 100, 4, draw::ACCENT_1, draw::ACCENT_2), (50, 150), 2.0);

    for (index, row) in rows.iter().enumerate() {
        let top = header + 12 + index as i32 * (row_height + gap); // верхний край строки
        let medal = [draw::GOLD, draw::SILVER, draw::BRONZE].get(index).copied(); // цвета первых трёх мест

        // Фон строки. На фото — тёмное «стекло» (иначе текст теряется на ярких местах), на обычном фоне —
        // полупрозрачная светлая плашка. У первых трёх мест плашка чуть заметнее.
        let (row_color, row_alpha) = match (has_photo, medal.is_some()) {
            (true, true) => ((10, 12, 28), 165),
            (true, false) => ((10, 12, 28), 140),
            (false, true) => (draw::WHITE, 24),
            (false, false) => (draw::WHITE, 13),
        };
        draw::fill_rounded(&mut board, (40, top, width - 80, row_height as u32), 20.0, row_color, row_alpha);
        if let Some(medal) = medal {
            draw::fill_rounded(&mut board, (46, top + 14, 6, (row_height - 28) as u32), 3.0, medal, 255); // полоска слева
        }

        // Кружок с номером места
        match medal {
            Some(medal) => draw::fill_rounded(&mut board, (60, top + 16, 52, 52), 26.0, medal, 255),
            None => draw::fill_rounded(&mut board, (60, top + 16, 52, 52), 26.0, draw::WHITE, 30),
        }
        let place_color = if medal.is_some() { (20, 20, 35) } else { draw::WHITE };
        draw::text(&mut board, (86.0, top as f32 + 42.0), Anchor::Center, bold, 26.0, place_color, &(index + 1).to_string());

        // Аватарка или иконка
        let picture = match &row.picture {
            Picture::Music => draw::music_icon(60),
            Picture::Avatar(bytes) => draw::avatar(bytes.as_deref(), 60, &row.name, fonts),
        };
        image::imageops::overlay(&mut board, &picture, 128, (top + 12) as i64);

        // Справа: цветная плашка и подпись под ней
        let pill_width = draw::text_width(bold, 22.0, &row.pill) as i32 + 32;
        let pill_left = width as i32 - 60 - pill_width;
        draw::paste_rounded(&mut board, &draw::gradient(pill_width as u32, 36, draw::ACCENT_1, draw::ACCENT_2), (pill_left, top + 10), 18.0);
        draw::text(&mut board, (pill_left as f32 + pill_width as f32 / 2.0, top as f32 + 28.0), Anchor::Center, bold, 22.0,
                   draw::WHITE, &row.pill);
        draw::text(&mut board, (940.0, top as f32 + 72.0), Anchor::RightBaseline, regular, 20.0, draw::TEXT_DIM, &row.hint);

        // Имя и полоска под ним (тянутся до плашки справа)
        let right_edge = (pill_left - 30).min(770);
        let name = draw::fit_text(bold, 28.0, &row.name, (right_edge - 210) as f32);
        draw::text(&mut board, (210.0, top as f32 + 42.0), Anchor::LeftBaseline, bold, 28.0, draw::WHITE, &name);
        draw::progress_bar(&mut board, (210, top + 54, (right_edge - 210).max(8) as u32, 8), row.progress);
    }
    board
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rows() -> Vec<Row> {
        let row = |name: &str, progress, pill: &str, hint: &str| Row {
            name: name.to_string(), picture: Picture::Avatar(None), progress, pill: pill.to_string(), hint: hint.to_string(),
        };
        vec![
            row("Alice", 0.76, "УР. 16", "15 430 XP"),
            row("Боб с очень-очень длинным именем, которое не помещается", 0.3, "УР. 9", "4 120 XP"),
            row("Карл", 0.05, "УР. 3", "510 XP"),
            Row { name: "Трек".into(), picture: Picture::Music, progress: 1.0, pill: "12 раз".into(), hint: "прослушано".into() },
        ]
    }

    /// Картинки рисуются нужного размера и не пустые. Заодно сохраняем их в target/ — посмотреть глазами.
    #[test]
    fn renders_cards() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let Some(fonts) = draw::fonts(&dir) else {
            println!("шрифтов нет — тест пропущен");
            return;
        };
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        let card = rank_card(fonts, "Founder of BakaSoft Solution", None, 16, 1630, 2180, 15430, Some(1), None);
        assert_eq!(card.dimensions(), (1000, 300));
        assert_eq!(card.get_pixel(0, 0)[3], 0, "углы карточки скруглены — самый угол прозрачный");
        assert_eq!(card.get_pixel(500, 150)[3], 255);
        std::fs::write(out.join("sample_rank.png"), draw::to_png(&card)).unwrap();

        let photo = draw::pick_background(&dir, "rank_bg");
        let with_photo = rank_card(fonts, "Alice", None, 3, 20, 265, 495, None, photo.as_deref());
        std::fs::write(out.join("sample_rank_photo.png"), draw::to_png(&with_photo)).unwrap();

        let rows = sample_rows();
        let board = table(fonts, "РЕЙТИНГ СЕРВЕРА", "Сервер BakaSoft", "ТОП 4", "по опыту в чате", &rows, None);
        assert_eq!(board.dimensions(), (1000, (170 + 4 * 96 + 28) as u32));
        std::fs::write(out.join("sample_table.png"), draw::to_png(&board)).unwrap();
        let png = draw::to_png(&board);
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']) && png.len() > 10_000);
    }
}
