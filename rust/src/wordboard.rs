//! Слова в чате: картинка с полями всех, кто играет в слово дня (обновляется после каждого хода),
//! кнопка «Играть!», итоги вчерашнего дня и команды /wort и /slovo, которые открывают мини-приложение.

use crate::draw::{self, Anchor, Fonts};
use crate::levels::fetch_avatar;
use crate::words::{self, Lang, Mark, MAX_GUESSES, WORD_LENGTH};
use crate::{Context, Data, Error};
use image::{Rgba, RgbaImage};
use poise::serenity_prelude as serenity;
use std::collections::HashMap;

/// Цвета клеток — как в мини-приложении.
fn mark_color(mark: Mark) -> draw::Rgb {
    [(59, 154, 104), (199, 154, 50), (74, 77, 85)][mark as usize]
}

/// Игрок на картинке: имя, аватарка (None — серый кружок с буквой) и подсветки его догадок.
pub struct Card {
    pub name: String,
    pub avatar: Option<Vec<u8>>,
    pub rows: Vec<Vec<Mark>>,
}

/// Картинка для чата: на каждого игрока узкая карточка — сверху аватарка, под ней поле из цветных квадратиков
/// (без букв). Название игры и кто играет — в тексте сообщения над картинкой.
pub fn board_image(fonts: &Fonts, players: &[Card]) -> RgbaImage {
    let scale = 2; // рисуем вдвое крупнее — в Discord картинка получается чёткой
    let (cell, gap, avatar_size, pad) = (15 * scale, 2 * scale, 80 * scale, 14 * scale);
    let grid_w = WORD_LENGTH as i32 * cell + (WORD_LENGTH as i32 - 1) * gap;
    let grid_h = MAX_GUESSES as i32 * cell + (MAX_GUESSES as i32 - 1) * gap;
    let card_w = grid_w.max(avatar_size) + 2 * pad;
    let card_h = pad + avatar_size + 16 * scale + grid_h + pad;
    let (space, outer, per_row) = (12 * scale, 14 * scale, 6);
    let count = players.len().max(1) as i32;
    let (columns, rows) = (count.min(per_row), (count + per_row - 1) / per_row);
    let width = columns * card_w + (columns - 1) * space + 2 * outer;
    let height = rows * card_h + (rows - 1) * space + 2 * outer;
    let (line, empty) = ((52, 54, 61), (44, 46, 52));
    let mut image = RgbaImage::from_pixel(width as u32, height as u32, Rgba([18, 19, 22, 255]));
    for (index, player) in players.iter().enumerate() {
        let (column, row) = (index as i32 % per_row, index as i32 / per_row);
        let (x, y) = (outer + column * (card_w + space), outer + row * (card_h + space));
        draw::outline_rounded(&mut image, (x, y, card_w as u32, card_h as u32), (16 * scale) as f32, scale as f32, line, 255);
        let (ax, ay) = (x + (card_w - avatar_size) / 2, y + pad);
        match &player.avatar {
            Some(bytes) => {
                let avatar = draw::avatar(Some(bytes), avatar_size as u32, &player.name, fonts);
                image::imageops::overlay(&mut image, &avatar, ax as i64, ay as i64);
            }
            None => {
                // нет аватарки — спокойный серый кружок с первой буквой имени
                draw::fill_rounded(&mut image, (ax, ay, avatar_size as u32, avatar_size as u32), avatar_size as f32 / 2.0, empty, 255);
                let letter: String = player.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
                let center = (ax as f32 + avatar_size as f32 / 2.0, ay as f32 + avatar_size as f32 / 2.0);
                draw::text(&mut image, center, Anchor::Center, &fonts.bold, (32 * scale) as f32, (139, 142, 150), &letter);
            }
        }
        let (gx, gy) = (x + (card_w - grid_w) / 2, y + pad + avatar_size + 16 * scale);
        for r in 0..MAX_GUESSES {
            for c in 0..WORD_LENGTH {
                let area = (gx + c as i32 * (cell + gap), gy + r as i32 * (cell + gap), cell as u32, cell as u32);
                match player.rows.get(r).and_then(|marks| marks.get(c)) {
                    Some(mark) => draw::fill_rounded(&mut image, area, 0.0, mark_color(*mark), 255),
                    None => draw::outline_rounded(&mut image, area, 0.0, scale as f32, empty, 255),
                }
            }
        }
    }
    image
}

/// [A] → «A», [A, B] → «A и B», [A, B, C] → «A, B и C» (упоминания без пинга).
pub fn names_text(users: &[u64]) -> String {
    let mentions: Vec<String> = users.iter().map(|user| format!("<@{user}>")).collect();
    match mentions.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} и {last}", rest.join(", ")),
        _ => mentions.concat(),
    }
}

/// Подпись над картинкой: «A и B играют в Wortjagd — немецкое слово дня».
pub fn caption(lang: Lang, users: &[u64], anyone_playing: bool) -> String {
    let verb = match (anyone_playing, users.len() == 1) {
        (true, true) => "играет",
        (true, false) => "играют",
        (false, true) => "сыграл(а)",
        (false, false) => "сыграли",
    };
    format!("🧩 {} {verb} в {} — {} дня", names_text(users), lang.name(), lang.what())
}

/// Игрок, который сегодня открывал слово дня в этом канале.
#[derive(Clone)]
pub struct Player {
    pub id: u64,
    pub name: String,
    pub face: String, // ссылка на аватарку
}

/// Что бот помнит про картинки в чате (после перезапуска — с нуля, как и в bot.py).
#[derive(Default)]
pub struct BoardState {
    /// (сервер, канал, день, язык) → (сообщение с картинкой, игроки)
    boards: tokio::sync::Mutex<HashMap<(u64, u64, String, Lang), (Option<serenity::MessageId>, Vec<Player>)>>,
    /// участник → байты аватарки (чтобы не качать её при каждом ходе)
    pub avatars: tokio::sync::Mutex<HashMap<u64, Option<Vec<u8>>>>,
    /// (сервер, участник) → в какую игру он последний раз позвал (/wort или /slovo)
    pub lang_choice: std::sync::Mutex<HashMap<(u64, u64), Lang>>,
}

/// Аватарка участника (скачивается один раз за запуск бота).
pub async fn cached_avatar(data: &Data, user: u64, face: &str) -> Option<Vec<u8>> {
    let mut avatars = data.boards.avatars.lock().await;
    if let Some(known) = avatars.get(&user) {
        return known.clone();
    }
    let avatar = fetch_avatar(data, face).await;
    avatars.insert(user, avatar.clone());
    avatar
}

fn play_button(lang: Lang) -> Vec<serenity::CreateActionRow> {
    vec![serenity::CreateActionRow::Buttons(vec![serenity::CreateButton::new(lang.button())
        .label("Играть!")
        .emoji('🧩')
        .style(serenity::ButtonStyle::Primary)])]
}

/// Картинка с полями игроков слова дня в канале: создаёт её или обновляет.
/// player — кто только что открыл игру или сделал ход (первый за день вызывает и итоги вчерашнего дня).
pub async fn update(ctx: &serenity::Context, data: &Data, guild: u64, channel: serenity::ChannelId, player: Player, lang: Lang) {
    if !data.words.settings.get(lang).announce {
        return;
    }
    if let Err(error) = update_inner(ctx, data, guild, channel, player, lang).await {
        println!("{}: не получилось обновить картинку в чате: {error}", lang.name());
    }
}

async fn update_inner(
    ctx: &serenity::Context,
    data: &Data,
    guild: u64,
    channel: serenity::ChannelId,
    player: Player,
    lang: Lang,
) -> Result<(), Error> {
    let day = words::today(data, lang);
    // Замок на всё обновление: двое зашли одновременно — всё равно одно сообщение
    let mut boards = data.boards.boards.lock().await;
    let no_pings = serenity::CreateAllowedMentions::new();
    if data.db.mark_word_summary(lang.summary_table(), guild, &day)? {
        let yesterday = words::day_before(&day);
        let rows = data.db.word_day_rows(lang.table(), guild, &yesterday)?;
        let streak = words::streak(&data.db.word_played_days(lang.table(), guild)?, &yesterday);
        if let Some(summary) = words::summary_text(lang, &rows, streak) {
            channel.send_message(ctx, serenity::CreateMessage::new().content(summary).allowed_mentions(no_pings.clone())).await?;
        }
    }
    boards.retain(|key, _| key.3 != lang || key.2 == day); // вчерашние картинки больше не обновляем
    let key = (guild, channel.get(), day, lang);
    let (message, mut players) = boards.get(&key).cloned().unwrap_or((None, Vec::new()));
    if !players.iter().any(|known| known.id == player.id) {
        players.push(player);
    }
    let (mut cards, mut anyone_playing) = (Vec::new(), false);
    for player in &players {
        let game = words::daily_game(data, guild, player.id, lang)?;
        anyone_playing |= !game.finished();
        cards.push(Card { name: player.name.clone(), avatar: cached_avatar(data, player.id, &player.face).await, rows: game.rows() });
    }
    let dir = data.dir.clone();
    let png = tokio::task::spawn_blocking(move || draw::fonts(&dir).map(|fonts| draw::to_png(&board_image(fonts, &cards))))
        .await?
        .ok_or("нет шрифтов для картинки")?;
    let users: Vec<u64> = players.iter().map(|player| player.id).collect();
    let text = caption(lang, &users, anyone_playing);
    let filename = format!("{}.png", lang.button().split(':').next().unwrap_or("words"));
    let attachment = || serenity::CreateAttachment::bytes(png.clone(), filename.clone());

    let mut message = message;
    if let Some(id) = message {
        let edit = serenity::EditMessage::new()
            .content(&text)
            .attachments(serenity::EditAttachments::new().add(attachment()))
            .components(play_button(lang))
            .allowed_mentions(no_pings.clone());
        if channel.edit_message(ctx, id, edit).await.is_err() {
            message = None; // картинку удалили — пришлём новую
        }
    }
    if message.is_none() {
        let new = serenity::CreateMessage::new()
            .content(&text)
            .add_file(attachment())
            .components(play_button(lang))
            .allowed_mentions(no_pings);
        message = Some(channel.send_message(ctx, new).await?.id);
    }
    boards.insert(key, (message, players));
    Ok(())
}

/// Что ответить, если мини-приложение не открылось. code — код ошибки Discord.
pub fn launch_problem(code: Option<isize>) -> &'static str {
    if code == Some(50231) {
        // в настройках Activity не включена эта платформа (ПК, телефон)
        "🧩 Мини-приложение пока не работает на твоём устройстве. Админу: Developer Portal → Activities → \
         Settings → Supported Platforms — включить Web (это и ПК), iOS и Android."
    } else {
        "🧩 Мини-приложение не открылось. Админу: проверь, что в Developer Portal включены Activities (инструкция в README)."
    }
}

fn error_code(error: &serenity::Error) -> Option<isize> {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => Some(response.error.code),
        _ => None,
    }
}

fn ephemeral(text: impl Into<String>) -> serenity::CreateInteractionResponse {
    serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

/// Открывает мини-приложение сразу на нужной игре. respond — как ответить на команду или нажатие кнопки.
async fn open_miniapp<F, Fut>(data: &Data, guild: Option<serenity::GuildId>, user: serenity::UserId, lang: Lang, respond: F) -> Result<(), Error>
where
    F: Fn(serenity::CreateInteractionResponse) -> Fut,
    Fut: std::future::Future<Output = serenity::Result<()>>,
{
    if !data.miniapp_ready {
        respond(ephemeral(format!(
            "🧩 Мини-приложение {} ещё не настроено. Админу: инструкция в README, раздел «Wortjagd».", lang.name()))).await?;
        return Ok(());
    }
    if let Some(guild) = guild {
        // страница спросит при входе, какую игру открыть
        data.boards.lang_choice.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert((guild.get(), user.get()), lang);
    }
    if let Err(error) = respond(serenity::CreateInteractionResponse::LaunchActivity).await {
        println!("Не получилось открыть мини-приложение: {error}");
        // Discord мог уже перестать ждать ответа на эту команду — тогда подсказка останется только в консоли
        let _ = respond(ephemeral(launch_problem(error_code(&error)))).await;
    }
    Ok(())
}

async fn open_from_command(ctx: Context<'_>, lang: Lang) -> Result<(), Error> {
    let poise::Context::Application(app) = ctx else { return Ok(()) };
    let http = ctx.serenity_context().http.clone();
    open_miniapp(ctx.data(), ctx.guild_id(), ctx.author().id, lang, |response| app.interaction.create_response(&http, response)).await
}

/// Wortjagd: угадай немецкое слово из 5 букв (мини-приложение)
#[poise::command(slash_command, guild_only)]
pub async fn wort(ctx: Context<'_>) -> Result<(), Error> {
    open_from_command(ctx, Lang::De).await
}

/// 5 букв: угадай русское слово из 5 букв (мини-приложение)
#[poise::command(slash_command, guild_only)]
pub async fn slovo(ctx: Context<'_>) -> Result<(), Error> {
    open_from_command(ctx, Lang::Ru).await
}

/// Нажатие кнопки «Играть!» под картинкой (работает и под старыми картинками, после перезапуска бота).
/// Возвращает true, если это была наша кнопка.
pub async fn play_pressed(ctx: &serenity::Context, data: &Data, press: &serenity::ComponentInteraction) -> bool {
    let Some(lang) = Lang::ALL.into_iter().find(|lang| lang.button() == press.data.custom_id) else { return false };
    let result = open_miniapp(data, press.guild_id, press.user.id, lang, |response| press.create_response(ctx, response)).await;
    if let Err(error) = result {
        println!("Кнопка «Играть!»: {error}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions() {
        assert_eq!(names_text(&[1]), "<@1>");
        assert_eq!(names_text(&[1, 2]), "<@1> и <@2>");
        assert_eq!(names_text(&[1, 2, 3]), "<@1>, <@2> и <@3>");
        assert_eq!(caption(Lang::De, &[1], true), "🧩 <@1> играет в Wortjagd — немецкое слово дня");
        assert_eq!(caption(Lang::Ru, &[1, 2], false), "🧩 <@1> и <@2> сыграли в 5 букв — русское слово дня");
        assert!(launch_problem(Some(50231)).contains("Supported Platforms"));
        assert!(launch_problem(None).contains("включены Activities"));
    }

    #[test]
    fn renders_board() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let Some(fonts) = draw::fonts(&dir) else { return };
        use Mark::{Absent as A, Correct as C, Present as P};
        let win = vec![vec![A, P, A, A, A], vec![A, A, P, A, A], vec![P, P, A, C, A], vec![C; 5]];
        let card = |name: &str, rows: Vec<Vec<Mark>>| Card { name: name.to_string(), avatar: None, rows };
        let one = board_image(fonts, &[card("Talia", Vec::new())]);
        assert_eq!(one.dimensions(), (278, 504)); // тот же размер, что рисует bot.py
        let two = board_image(fonts, &[card("Talia", win.clone()), card("Боб", vec![vec![A, A, P, A, A]])]);
        assert_eq!(two.dimensions(), (524, 504));
        let seven: Vec<Card> = (0..7).map(|index| card(&format!("P{index}"), win.clone())).collect();
        assert_eq!(board_image(fonts, &seven).dimensions(), (1508, 976));
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("sample_board.png");
        std::fs::write(out, draw::to_png(&two)).unwrap();
    }
}
