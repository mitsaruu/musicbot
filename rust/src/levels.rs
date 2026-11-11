//! Уровни: опыт за сообщения и за голосовые каналы, /rank, /leaderboard, /money_leaderboard.
//! Правила — как в bot.py: опыт за сообщение не чаще раза в минуту, в голосовом — каждую минуту тем,
//! кто сидит не один и не выключил звук.

use crate::cards::{self, Picture, Row};
use crate::draw;
use crate::reply::user_name;
use crate::text::{number, plural};
use crate::{db, Context, Data, Error};
use poise::serenity_prelude as serenity;
use rand::Rng;

/// Настройки из settings.ini, раздел [levels].
pub struct LevelSettings {
    pub xp_min: i64,
    pub xp_max: i64,
    pub cooldown: i64,          // опыт за сообщения — не чаще раза в столько секунд
    pub voice_xp: i64,          // опыт за минуту в голосовом канале
    pub voice_min_people: usize, // сколько людей (не ботов) должно быть в канале, чтобы шёл опыт
    pub level_up: LevelUp,
}

/// Что писать при новом уровне.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LevelUp {
    Off,
    Text,
    Card,
}

impl LevelSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        let (a, b) = (settings.int("levels", "xp_per_message_min", 15), settings.int("levels", "xp_per_message_max", 25));
        Self {
            xp_min: a.min(b),
            xp_max: a.max(b),
            cooldown: settings.int("levels", "xp_cooldown_seconds", 60),
            voice_xp: settings.int("levels", "voice_xp_per_minute", 10),
            voice_min_people: settings.int("levels", "voice_min_people", 2).max(1) as usize,
            level_up: match settings.text("levels", "level_up_message", "off").to_lowercase().as_str() {
                "text" => LevelUp::Text,
                "card" => LevelUp::Card,
                _ => LevelUp::Off,
            },
        }
    }
}

/// Сколько опыта нужно, чтобы перейти с уровня level на следующий: 0→1 — 100, 1→2 — 155, 2→3 — 220…
pub fn xp_for_level(level: i64) -> i64 {
    5 * level * level + 50 * level + 100
}

/// По общему опыту: (уровень, опыт внутри текущего уровня, сколько нужно до следующего).
pub fn level_info(total_xp: i64) -> (i64, i64, i64) {
    let (mut level, mut xp) = (0, total_xp.max(0));
    while xp >= xp_for_level(level) {
        xp -= xp_for_level(level);
        level += 1;
    }
    (level, xp, xp_for_level(level))
}

/// Ссылка на аватарку → такая же, но PNG 256×256 (анимированные и webp рисовать незачем).
pub fn avatar_png_url(face: &str) -> String {
    let base = face.split('?').next().unwrap_or(face);
    let base = base.strip_suffix(".webp").or_else(|| base.strip_suffix(".gif")).map_or(base.to_string(), |stem| format!("{stem}.png"));
    format!("{base}?size=256")
}

/// Скачивает аватарку. None — не получилось (тогда нарисуется кружок с буквой).
pub async fn fetch_avatar(data: &Data, face: &str) -> Option<Vec<u8>> {
    let response = data.web.get(avatar_png_url(face)).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.bytes().await.ok().map(|bytes| bytes.to_vec())
}

/// Аватарка по id участника (для таблиц рейтинга).
pub async fn avatar_by_id(ctx: &serenity::Context, data: &Data, user: u64) -> Option<Vec<u8>> {
    let user = serenity::UserId::new(user).to_user(ctx).await.ok()?;
    fetch_avatar(data, &user.face()).await
}

/// Рисует карточку уровня в отдельном потоке (рисование — долгая работа). None — нет шрифтов.
async fn render_rank(data: &Data, name: String, avatar: Option<Vec<u8>>, total_xp: i64, position: Option<i64>) -> Option<Vec<u8>> {
    let dir = data.dir.clone();
    tokio::task::spawn_blocking(move || {
        let fonts = draw::fonts(&dir)?;
        let (level, current, need) = level_info(total_xp);
        let background = draw::pick_background(&dir, "rank_bg");
        let card = cards::rank_card(fonts, &name, avatar.as_deref(), level, current, need, total_xp, position, background.as_deref());
        Some(draw::to_png(&card))
    })
    .await
    .ok()
    .flatten()
}

pub async fn render_table(
    data: &Data,
    title: &'static str,
    subtitle: String,
    right_hint: &'static str,
    rows: Vec<Row>,
    background: &'static str,
) -> Option<Vec<u8>> {
    let dir = data.dir.clone();
    tokio::task::spawn_blocking(move || {
        let fonts = draw::fonts(&dir)?;
        let photo = draw::pick_background(&dir, background);
        let board = cards::table(fonts, title, &subtitle, &format!("ТОП {}", rows.len()), right_hint, &rows, photo.as_deref());
        Some(draw::to_png(&board))
    })
    .await
    .ok()
    .flatten()
}

const NO_FONTS: &str = "😵 Не могу нарисовать картинку: на сервере не нашлось шрифта. Положи font.ttf и font-bold.ttf в папку бота.";

pub async fn send_png(ctx: Context<'_>, png: Option<Vec<u8>>, filename: &str) -> Result<(), Error> {
    match png {
        Some(png) => {
            ctx.send(poise::CreateReply::default().attachment(serenity::CreateAttachment::bytes(png, filename))).await?;
        }
        None => {
            ctx.say(NO_FONTS).await?;
        }
    }
    Ok(())
}

/// Показать карточку уровня
#[poise::command(slash_command, guild_only)]
pub async fn rank(
    ctx: Context<'_>,
    #[description = "Чей уровень показать (если не выбрать — твой)"] member: Option<serenity::Member>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    ctx.defer().await?; // рисование занимает пару секунд — показываем «бот думает…»
    let (user, name, face) = match &member {
        Some(member) => (member.user.id.get(), member.display_name().to_string(), member.face()),
        None => match ctx.author_member().await {
            Some(member) => (ctx.author().id.get(), member.display_name().to_string(), member.face()),
            None => (ctx.author().id.get(), user_name(ctx.author()), ctx.author().face()),
        },
    };
    let total_xp = data.db.xp(guild, user)?;
    let position = if total_xp > 0 { Some(data.db.xp_position(guild, total_xp)?) } else { None }; // без опыта места нет
    let avatar = fetch_avatar(data, &face).await;
    send_png(ctx, render_rank(data, name, avatar, total_xp, position).await, "rank.png").await
}

/// Таблица рейтинга сервера (топ-10)
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let top = data.db.top_levels(guild.get())?;
    if top.is_empty() {
        ctx.say("Пока никто не набрал опыта — пишите в чат!").await?;
        return Ok(());
    }
    ctx.defer().await?;
    let mut rows = Vec::new();
    for (user, name, xp) in top {
        let (level, current, need) = level_info(xp);
        rows.push(Row {
            name,
            picture: Picture::Avatar(avatar_by_id(ctx.serenity_context(), data, user).await),
            progress: current as f64 / need as f64, // прогресс до следующего уровня
            pill: format!("УР. {level}"),
            hint: format!("{} XP", number(xp)),
        });
    }
    let server = guild.name(ctx).unwrap_or_default();
    let png = render_table(data, "РЕЙТИНГ СЕРВЕРА", server, "по опыту в чате", rows, "leaderboard_bg").await;
    send_png(ctx, png, "leaderboard.png").await
}

/// Самые богатые участники (топ-10)
#[poise::command(slash_command, guild_only)]
pub async fn money_leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let top = data.db.top_money(guild.get())?;
    if top.is_empty() {
        ctx.say("Пока ни у кого нет монет — начни с /work!").await?;
        return Ok(());
    }
    ctx.defer().await?;
    let richest = top.iter().map(|(_, _, balance)| *balance).max().unwrap_or(1).max(1);
    let mut rows = Vec::new();
    for (user, name, balance) in top {
        rows.push(Row {
            name,
            picture: Picture::Avatar(avatar_by_id(ctx.serenity_context(), data, user).await),
            progress: balance.max(0) as f64 / richest as f64,
            pill: number(balance),
            hint: plural(balance, "монета", "монеты", "монет").to_string(),
        });
    }
    let server = guild.name(ctx).unwrap_or_default();
    let png = render_table(data, "РЕЙТИНГ БОГАТСТВА", server, "по монетам", rows, "money_bg").await;
    send_png(ctx, png, "money_leaderboard.png").await
}

/// Добавляет участнику опыт. Если вырос уровень — поздравляет (если это включено в settings.ini)
/// в канале для игр, а если его нет — в канале channel.
pub async fn give_xp(
    ctx: &serenity::Context,
    data: &Data,
    guild: serenity::GuildId,
    user: &serenity::User,
    name: &str,
    amount: i64,
    channel: serenity::ChannelId,
) -> Result<(), Error> {
    let old_xp = data.db.xp(guild.get(), user.id.get())?;
    let total_xp = old_xp + amount;
    data.db.set_xp(guild.get(), user.id.get(), name, total_xp)?;
    let (old_level, new_level) = (level_info(old_xp).0, level_info(total_xp).0);
    if new_level <= old_level || data.levels.level_up == LevelUp::Off {
        return Ok(());
    }
    let text = format!("🎉 <@{}> получает **{new_level} уровень**!", user.id);
    let channel = data.db.games_channel(guild.get())?.map(serenity::ChannelId::new).unwrap_or(channel);
    let mut message = serenity::CreateMessage::new().content(text);
    if data.levels.level_up == LevelUp::Card {
        let avatar = fetch_avatar(data, &user.face()).await;
        let position = data.db.xp_position(guild.get(), total_xp)?;
        if let Some(png) = render_rank(data, name.to_string(), avatar, total_xp, Some(position)).await {
            message = message.add_file(serenity::CreateAttachment::bytes(png, "rank.png"));
        }
    }
    let _ = channel.send_message(ctx, message).await; // нет прав писать в этот канал — просто молчим
    Ok(())
}

/// Опыт за сообщение (не чаще раза в cooldown секунд).
pub async fn message_xp(ctx: &serenity::Context, data: &Data, message: &serenity::Message) -> Result<(), Error> {
    let Some(guild) = message.guild_id else { return Ok(()) };
    if message.author.bot {
        return Ok(());
    }
    let (now, user) = (db::now(), message.author.id.get());
    if now - data.db.last_xp_time(guild.get(), user)? < data.levels.cooldown as f64 {
        return Ok(());
    }
    let amount = rand::rng().random_range(data.levels.xp_min..=data.levels.xp_max);
    // Имя на сервере (ник), а если ника нет — обычное имя
    let name = message.member.as_ref().and_then(|member| member.nick.clone()).unwrap_or_else(|| user_name(&message.author));
    give_xp(ctx, data, guild, &message.author, &name, amount, message.channel_id).await?;
    data.db.set_last_xp_time(guild.get(), user, now)?;
    Ok(())
}

/// Кто сейчас получает опыт за голосовой канал на сервере: (участник, имя, канал).
/// Не считаются: AFK-канал, каналы, где людей меньше нужного, и те, у кого выключен звук.
fn voice_listeners(guild: &serenity::Guild, min_people: usize) -> Vec<(serenity::User, String, serenity::ChannelId)> {
    let mut channels: std::collections::HashMap<serenity::ChannelId, Vec<&serenity::VoiceState>> = Default::default();
    for state in guild.voice_states.values() {
        let Some(channel) = state.channel_id else { continue };
        if Some(channel) == guild.afk_metadata.as_ref().map(|afk| afk.afk_channel_id) {
            continue;
        }
        channels.entry(channel).or_default().push(state);
    }
    let mut listeners = Vec::new();
    for (channel, states) in channels {
        let people: Vec<(&serenity::VoiceState, &serenity::Member)> = states
            .iter()
            .filter_map(|state| state.member.as_ref().or_else(|| guild.members.get(&state.user_id)).map(|member| (*state, member)))
            .filter(|(_, member)| !member.user.bot) // боты, в том числе музыкальный, не считаются
            .collect();
        if people.len() < min_people {
            continue; // человек сидит один — опыт не идёт никому
        }
        for (state, member) in people {
            if !state.self_deaf && !state.deaf {
                listeners.push((member.user.clone(), member.display_name().to_string(), channel));
            }
        }
    }
    listeners
}

/// Раз в минуту даёт опыт всем, кто сидит в голосовых каналах.
pub fn start_voice_xp_loop(ctx: serenity::Context, data: std::sync::Arc<Data>) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        timer.tick().await; // первый «тик» срабатывает сразу — пропускаем его
        loop {
            timer.tick().await;
            for guild_id in ctx.cache.guilds() {
                // Список собираем отдельно: кэш нельзя держать открытым, пока ждём ответов Discord
                let listeners = match ctx.cache.guild(guild_id) {
                    Some(guild) => voice_listeners(&guild, data.levels.voice_min_people),
                    None => continue,
                };
                for (user, name, channel) in listeners {
                    if let Err(error) = give_xp(&ctx, &data, guild_id, &user, &name, data.levels.voice_xp, channel).await {
                        println!("Опыт за голосовой канал: {error}"); // ошибка с одним не останавливает остальных
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_match_python() {
        assert_eq!([xp_for_level(0), xp_for_level(1), xp_for_level(2), xp_for_level(10)], [100, 155, 220, 1100]);
        assert_eq!(level_info(0), (0, 0, 100));
        assert_eq!(level_info(99), (0, 99, 100));
        assert_eq!(level_info(100), (1, 0, 155));
        assert_eq!(level_info(255), (2, 0, 220));
        assert_eq!(level_info(15430), (16, 1630, 2180)); // то же, что level_info(15430) в bot.py
        assert_eq!(level_info(-5), (0, 0, 100));
    }

    #[test]
    fn avatar_urls() {
        assert_eq!(avatar_png_url("https://cdn.discordapp.com/avatars/1/abc.webp?size=1024"),
                   "https://cdn.discordapp.com/avatars/1/abc.png?size=256");
        assert_eq!(avatar_png_url("https://cdn.discordapp.com/avatars/1/a_abc.gif?size=1024"),
                   "https://cdn.discordapp.com/avatars/1/a_abc.png?size=256");
        assert_eq!(avatar_png_url("https://cdn.discordapp.com/embed/avatars/3.png"),
                   "https://cdn.discordapp.com/embed/avatars/3.png?size=256");
    }

    #[test]
    fn settings_defaults() {
        let levels = LevelSettings::load(&crate::config::Settings::default());
        assert_eq!((levels.xp_min, levels.xp_max, levels.cooldown, levels.voice_xp, levels.voice_min_people, levels.level_up),
                   (15, 25, 60, 10, 2, LevelUp::Off));
        let levels = LevelSettings::load(&crate::config::Settings::parse("[levels]\nlevel_up_message = Card\nxp_per_message_min = 30\n"));
        assert_eq!((levels.xp_min, levels.xp_max, levels.level_up), (25, 30, LevelUp::Card));
    }
}
