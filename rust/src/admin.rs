//! Настройки сервера и модерация: канал для игр (/games_channel), канал логов (/log_channel),
//! просмотр и снятие варнов (/warns, /warns_clear). Сами варны выдаёт ИИ по просьбе модератора (moderation.rs)
//! и анти-спам (chat.rs).

use crate::reply::{say, whisper};
use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use std::collections::BTreeSet;

/// Какие команды работают только в канале для игр, если его выбрали (settings.ini → [channels] games_commands).
pub const DEFAULT_GAMES_COMMANDS: &str = "rank, leaderboard, top_chart_month, money_leaderboard, dick_growth, dick_size, \
    dick_leaderboard, work, balance, give_money, coinflip, slots, blackjack, case, pets, pets_collect, pet_feed, pet_sell, \
    dice, roll, predskazanie, shar, wort, slovo, daily, shop, shop_buy, pet_battle, pet_trade, wordstats, wordtop, wordduel, quiz";

/// Названия через запятую → набор названий без «/» и в нижнем регистре.
pub fn parse_commands(text: &str) -> BTreeSet<String> {
    text.split(',').map(|name| name.trim().trim_start_matches('/').to_lowercase()).filter(|name| !name.is_empty()).collect()
}

/// Что боту можно в этом канале. None — не получилось узнать (нет данных о сервере).
pub fn bot_permissions(ctx: Context<'_>, channel: &serenity::GuildChannel) -> Option<serenity::Permissions> {
    let guild = ctx.guild()?;
    let member = guild.members.get(&ctx.framework().bot_id())?;
    Some(guild.user_permissions_in(channel, member))
}

/// Можно ли выполнить команду в этом канале. channel_parent — канал, внутри которого ветка (если это ветка).
pub fn allowed_here(games: &BTreeSet<String>, command: &str, games_channel: Option<u64>, channel: u64, channel_parent: Option<u64>) -> bool {
    match games_channel {
        Some(games_channel) if games.contains(command) => channel == games_channel || channel_parent == Some(games_channel),
        _ => true, // команда не из списка или канал для игр не выбран — можно везде
    }
}

/// Проверка перед каждой командой: если нельзя — тихо подсказывает, где можно.
pub async fn games_channel_check(ctx: Context<'_>) -> Result<bool, Error> {
    let Some(guild) = ctx.guild_id() else { return Ok(true) }; // в личных сообщениях можно всё
    let data = ctx.data();
    let games_channel = data.db.games_channel(guild.get())?;
    let parent = ctx.guild_channel().await.and_then(|channel| channel.parent_id).map(|id| id.get());
    if allowed_here(&data.games_commands, &ctx.command().name, games_channel, ctx.channel_id().get(), parent) {
        return Ok(true);
    }
    whisper(ctx, format!("🎮 Эта команда работает только в <#{}>", games_channel.unwrap_or_default())).await?;
    Ok(false)
}

/// Канал для рангов, рейтингов и игр (пусто — работают везде)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "games_channel")]
pub async fn set_games_channel(
    ctx: Context<'_>,
    #[description = "Канал для игр (если не выбрать — команды снова будут работать в любом канале)"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let Some(channel) = channel else {
        data.db.set_games_channel(guild, None)?;
        return say(ctx, "✅ Ранги, рейтинги и игры снова работают в любом канале.").await;
    };
    let allowed = bot_permissions(ctx, &channel).is_some_and(|p| p.send_messages() && p.attach_files() && p.embed_links());
    if !allowed {
        return whisper(ctx, format!(
            "Мне не хватает прав в <#{}>: нужны «Отправлять сообщения», «Прикреплять файлы» и «Встраивать ссылки».",
            channel.id)).await;
    }
    data.db.set_games_channel(guild, Some(channel.id.get()))?;
    let commands: Vec<String> = data.games_commands.iter().map(|name| format!("/{name}")).collect();
    say(ctx, format!(
        "✅ Теперь эти команды работают только в <#{}>:\n{}\nЧтобы снова разрешить их везде — вызови /games_channel без канала.",
        channel.id, commands.join(", "))).await
}

/// Канал, куда бот присылает свои ошибки (пусто — не присылать)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "log_channel")]
pub async fn set_log_channel(
    ctx: Context<'_>,
    #[description = "Канал для логов (лучше закрытый, только для админов). Пусто — выключить"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let Some(channel) = channel else {
        data.db.set_log_channel(guild, None)?;
        return whisper(ctx, "✅ Больше не присылаю ошибки в Discord (в консоль пишу как раньше).").await;
    };
    let allowed = bot_permissions(ctx, &channel).is_some_and(|p| p.view_channel() && p.send_messages() && p.embed_links());
    if !allowed {
        return whisper(ctx, format!(
            "Мне не хватает прав в <#{}>: нужны «Просматривать канал», «Отправлять сообщения» и «Встраивать ссылки».",
            channel.id)).await;
    }
    data.db.set_log_channel(guild, Some(channel.id.get()))?;
    whisper(ctx, format!("✅ Теперь присылаю ошибки в <#{}>. Выключить — /log_channel без канала.", channel.id)).await
}

/// Присылает ошибку в канал логов сервера (если он выбран). Без пингов.
pub async fn report_error(http: &serenity::Http, data: &Data, guild: Option<serenity::GuildId>, place: &str, details: &str) {
    let Some(guild) = guild else { return };
    let Ok(Some(channel)) = data.db.log_channel(guild.get()) else { return };
    let tail: String = details.chars().rev().take(3800).collect::<Vec<_>>().into_iter().rev().collect(); // конец — самое важное
    let embed = serenity::CreateEmbed::new().color(0xEF4444).title(format!("⚠️ Ошибка: {place}")).description(format!("```\n{tail}\n```"));
    let message = serenity::CreateMessage::new().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Err(error) = serenity::ChannelId::new(channel).send_message(http, message).await {
        println!("Не получилось отправить лог в канал: {error}");
    }
}

/// Варны участника (их выдаёт бот по просьбе модератора)
#[poise::command(slash_command, guild_only, rename = "warns")]
pub async fn show_warns(
    ctx: Context<'_>,
    #[description = "Чьи варны посмотреть (если не выбрать — твои)"] member: Option<serenity::Member>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let (user, name) = match &member {
        Some(member) => (member.user.id.get(), member.display_name().to_string()),
        None => (ctx.author().id.get(), crate::reply::display_name(ctx).await),
    };
    let (rows, total) = data.db.warns(guild, user)?;
    if rows.is_empty() {
        return say(ctx, format!("✅ У <@{user}> нет варнов.")).await;
    }
    let lines: Vec<String> = rows
        .iter()
        .map(|(moderator, reason, created)| format!("• <t:{}:d> — {reason} (выдал <@{moderator}>)", *created as i64))
        .collect();
    let mut embed = serenity::CreateEmbed::new().color(0xF59E0B).title(format!("⚠️ Варны {name}: {total}")).description(lines.join("\n"));
    if total > rows.len() as i64 {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!("Показаны последние {}", rows.len())));
    }
    ctx.send(poise::CreateReply::default().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Снять все варны с участника
#[poise::command(slash_command, guild_only, default_member_permissions = "MODERATE_MEMBERS")]
pub async fn warns_clear(ctx: Context<'_>, #[description = "С кого снять варны"] member: serenity::User) -> Result<(), Error> {
    let deleted = ctx.data().db.clear_warns(ctx.guild_id().unwrap().get(), member.id.get())?;
    say(ctx, if deleted > 0 {
        format!("🧹 С <@{}> сняты варны: {deleted}.", member.id)
    } else {
        format!("У <@{}> и так нет варнов.", member.id)
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_channel_rules() {
        let games = parse_commands(" rank, /Work ,slots,, ");
        assert_eq!(games.iter().cloned().collect::<Vec<_>>(), ["rank", "slots", "work"]);
        assert!(allowed_here(&games, "rank", None, 5, None)); // канал для игр не выбран
        assert!(allowed_here(&games, "play", Some(9), 5, None)); // команда не из списка
        assert!(!allowed_here(&games, "rank", Some(9), 5, None));
        assert!(allowed_here(&games, "rank", Some(9), 9, None));
        assert!(allowed_here(&games, "rank", Some(9), 5, Some(9))); // ветка внутри канала для игр
        assert!(parse_commands(DEFAULT_GAMES_COMMANDS).contains("wordduel"));
    }
}
