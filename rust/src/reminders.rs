//! Напоминания: /remind и /reminders. Раз в 20 секунд бот проверяет, не пришло ли время.

use crate::reply::whisper;
use crate::{db, Context, Data, Error};
use poise::serenity_prelude as serenity;

const MAX_REMINDERS: i64 = 10; // сколько напоминаний может ждать у одного человека

/// «2ч 30м», «45m», «1d», «90» (минуты) → секунды. Не получилось разобрать — None.
pub fn parse_duration(text: &str) -> Option<i64> {
    let characters: Vec<char> = text.trim().to_lowercase().chars().collect();
    let (mut total, mut index) = (0i64, 0);
    while index < characters.len() {
        if characters[index].is_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len() && characters[index].is_ascii_digit() {
            index += 1;
        }
        if start == index {
            return None; // ждали число, а там что-то другое
        }
        let amount: i64 = characters[start..index].iter().collect::<String>().parse().ok()?;
        while index < characters.len() && characters[index].is_whitespace() {
            index += 1;
        }
        let unit_start = index;
        while index < characters.len() && characters[index].is_alphabetic() {
            index += 1;
        }
        // Единица — по первой букве: с/s — секунды, м/m — минуты, ч/h — часы, д/d — дни. Без единицы — минуты.
        let unit = match characters[unit_start..index].first() {
            None | Some('м') | Some('m') => 60,
            Some('с') | Some('s') => 1,
            Some('ч') | Some('h') => 3600,
            Some('д') | Some('d') => 86400,
            Some(_) => return None,
        };
        total = total.checked_add(amount.checked_mul(unit)?)?;
    }
    (total > 0).then_some(total)
}

/// Напомнить о чём-нибудь через указанное время
#[poise::command(slash_command, guild_only)]
pub async fn remind(
    ctx: Context<'_>,
    #[description = "Через сколько: 30м, 2ч, 1д, 1ч 30м (просто число — минуты)"]
    #[max_length = 30]
    when: String,
    #[description = "О чём напомнить"]
    #[max_length = 500]
    text: String,
) -> Result<(), Error> {
    let (data, guild, user) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get());
    let Some(seconds) = parse_duration(&when).filter(|seconds| (30..=365 * 86400).contains(seconds)) else {
        return whisper(ctx, "Не понял время. Примеры: `30м`, `2ч`, `1д`, `1ч 30м` (от 30 секунд до года).").await;
    };
    if data.db.count_reminders(guild, user)? >= MAX_REMINDERS {
        return whisper(ctx, format!("У тебя уже {MAX_REMINDERS} напоминаний — отмени лишние: /reminders.")).await;
    }
    let due = db::now() + seconds as f64;
    data.db.add_reminder(guild, ctx.channel_id().get(), user, due, &text)?;
    whisper(ctx, format!("⏰ Напомню <t:{0}:R> (<t:{0}:f>): {text}", due as i64)).await
}

/// Твои напоминания (можно отменить одно по номеру)
#[poise::command(slash_command, guild_only)]
pub async fn reminders(
    ctx: Context<'_>,
    #[description = "Номер напоминания, которое нужно отменить"]
    #[min = 1]
    #[max = 10]
    cancel: Option<usize>,
) -> Result<(), Error> {
    let (data, guild, user) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get());
    let rows = data.db.user_reminders(guild, user)?;
    if rows.is_empty() {
        return whisper(ctx, "Напоминаний нет. Поставить — /remind.").await;
    }
    if let Some(number) = cancel {
        let Some((id, _, text)) = rows.get(number - 1) else {
            return whisper(ctx, "Напоминания с таким номером нет.").await;
        };
        data.db.delete_reminder(*id)?;
        return whisper(ctx, format!("🗑️ Отменил напоминание: {text}")).await;
    }
    let lines: Vec<String> =
        rows.iter().enumerate().map(|(index, (_, due, text))| format!("`{}.` <t:{}:R> — {text}", index + 1, *due as i64)).collect();
    whisper(ctx, format!("⏰ Твои напоминания:\n{}", lines.join("\n"))).await
}

/// Отправляет напоминания, у которых пришло время.
pub async fn send_due(http: &serenity::Http, data: &Data) -> Result<(), Error> {
    for (id, channel, user, text) in data.db.due_reminders(db::now())? {
        data.db.delete_reminder(id)?;
        let message = serenity::CreateMessage::new()
            .content(format!("⏰ <@{user}>, напоминаю: {text}"))
            .allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![serenity::UserId::new(user)]));
        if let Err(error) = serenity::ChannelId::new(channel).send_message(http, message).await {
            println!("Напоминание не отправилось: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let cases = [("30м", Some(1800)), ("2ч", Some(7200)), ("1д", Some(86400)), ("1ч 30м", Some(5400)), ("90", Some(5400)),
                     ("45s", Some(45)), ("2h15m", Some(8100)), ("10 мин", Some(600)), ("1 час 5 минут", Some(3900)),
                     ("завтра", None), ("5 лет", None), ("", None), ("0", None), ("м30", None), ("3 дня", Some(259200))];
        for (text, expected) in cases {
            assert_eq!(parse_duration(text), expected, "{text:?}");
        }
    }
}
