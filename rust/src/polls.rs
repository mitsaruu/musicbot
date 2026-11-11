//! Опросы (/poll): вопрос и до 5 вариантов. Голосуют кнопками, голос можно поменять.
//! Когда время выйдет, бот подводит итог. Кнопки работают и после перезапуска.

use crate::reply::whisper;
use crate::{db, Context, Data, Error};
use poise::serenity_prelude as serenity;

const MAX_OPTIONS: usize = 5;

/// Варианты из строки «Пицца | Суши | Бургеры». None — их меньше 2, больше 5 или есть одинаковые.
pub fn parse_options(text: &str) -> Option<Vec<String>> {
    let options: Vec<String> = text
        .split(['|', ';', '\n'])
        .map(|option| option.trim().chars().take(80).collect::<String>())
        .filter(|option| !option.is_empty())
        .collect();
    let unique = options.iter().collect::<std::collections::HashSet<_>>().len();
    ((2..=MAX_OPTIONS).contains(&options.len()) && unique == options.len()).then_some(options)
}

/// Текст карточки опроса: варианты с полосками. votes — сколько голосов у каждого варианта.
pub fn poll_lines(options: &[String], votes: &[i64], closed: bool) -> String {
    let total: i64 = votes.iter().sum();
    let best = votes.iter().copied().max().unwrap_or(0);
    options
        .iter()
        .zip(votes)
        .map(|(option, count)| {
            let share = if total > 0 { *count as f64 / total as f64 } else { 0.0 };
            let mark = if closed && *count == best && *count > 0 { " 🏆" } else { "" };
            let bar = "█".repeat((share * 12.0).round_ties_even() as usize);
            format!("**{option}**{mark}\n`{bar:<12}` {}% · {count}", (share * 100.0).round_ties_even())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn embed(question: &str, options: &[String], votes: &[i64], ends: f64, author: u64, closed: bool) -> serenity::CreateEmbed {
    let total: i64 = votes.iter().sum();
    let status = if closed { "завершён".to_string() } else { format!("закроется <t:{}:R>", ends as i64) };
    serenity::CreateEmbed::new()
        .color(0x7C6CF2)
        .title(format!("{}{question}", if closed { "📊 Итоги: " } else { "📊 " }))
        .description(poll_lines(options, votes, closed))
        .field("\u{200b}", format!("Проголосовало: {total} · опрос от <@{author}> · {status}"), false)
}

fn buttons(options: &[String]) -> Vec<serenity::CreateActionRow> {
    let buttons = options
        .iter()
        .enumerate()
        .map(|(index, option)| serenity::CreateButton::new(format!("poll:{index}")).label(option).style(serenity::ButtonStyle::Secondary))
        .collect();
    vec![serenity::CreateActionRow::Buttons(buttons)]
}

fn counts(data: &Data, message: u64, options: usize) -> rusqlite::Result<Vec<i64>> {
    let votes = data.db.poll_votes(message)?;
    Ok((0..options as i64).map(|option| votes.get(&option).copied().unwrap_or(0)).collect())
}

/// Опрос с кнопками: вопрос и до 5 вариантов ответа
#[poise::command(slash_command, guild_only)]
pub async fn poll(
    ctx: Context<'_>,
    #[description = "Вопрос"]
    #[max_length = 200]
    question: String,
    #[description = "Варианты через | — например: Пицца | Суши | Бургеры"]
    #[max_length = 500]
    options: String,
    #[description = "Сколько минут идёт опрос (по умолчанию 60)"]
    #[min = 1]
    #[max = 10080]
    minutes: Option<i64>,
) -> Result<(), Error> {
    let Some(choices) = parse_options(&options) else {
        return whisper(ctx, format!(
            "Нужно от 2 до {MAX_OPTIONS} разных вариантов через `|`. Пример: `Пицца | Суши | Бургеры`")).await;
    };
    let (ends, author) = (db::now() + minutes.unwrap_or(60) as f64 * 60.0, ctx.author().id.get());
    let reply = ctx
        .send(poise::CreateReply::default()
            .embed(embed(&question, &choices, &vec![0; choices.len()], ends, author, false))
            .components(buttons(&choices))
            .allowed_mentions(serenity::CreateAllowedMentions::new()))
        .await?;
    let message = reply.message().await?.id.get();
    ctx.data().db.add_poll(message, ctx.channel_id().get(), author, &question, &serde_json::to_string(&choices)?, ends)?;
    Ok(())
}

/// Нажатие кнопки варианта. Возвращает true, если это была кнопка опроса.
pub async fn vote_pressed(ctx: &serenity::Context, data: &Data, press: &serenity::ComponentInteraction) -> bool {
    let Some(option) = press.data.custom_id.strip_prefix("poll:").and_then(|index| index.parse::<i64>().ok()) else {
        return false;
    };
    if let Err(error) = vote(ctx, data, press, option).await {
        println!("Опрос: {error}");
    }
    true
}

async fn vote(ctx: &serenity::Context, data: &Data, press: &serenity::ComponentInteraction, option: i64) -> Result<(), Error> {
    let message = press.message.id.get();
    let open = data.db.poll(message)?.filter(|poll| poll.2 > db::now());
    let Some((question, options, ends, author)) = open else {
        let note = serenity::CreateInteractionResponseMessage::new().content("Этот опрос уже закончился.").ephemeral(true);
        press.create_response(ctx, serenity::CreateInteractionResponse::Message(note)).await?;
        return Ok(());
    };
    let options: Vec<String> = serde_json::from_str(&options)?;
    data.db.set_poll_vote(message, press.user.id.get(), option)?;
    let update = serenity::CreateInteractionResponseMessage::new()
        .embed(embed(&question, &options, &counts(data, message, options.len())?, ends, author, false));
    press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
    Ok(())
}

/// Закрывает опросы, у которых вышло время: показывает итоги и убирает кнопки.
pub async fn close_finished(http: &serenity::Http, data: &Data) -> Result<(), Error> {
    for (message, channel, author, question, options, ends) in data.db.finished_polls(db::now())? {
        let options: Vec<String> = serde_json::from_str(&options).unwrap_or_default();
        let votes = counts(data, message, options.len())?;
        data.db.delete_poll(message)?;
        let edit = serenity::EditMessage::new().embed(embed(&question, &options, &votes, ends, author, true)).components(Vec::new());
        // если сообщение с опросом удалили — ничего страшного
        let _ = serenity::ChannelId::new(channel).edit_message(http, serenity::MessageId::new(message), edit).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        assert_eq!(parse_options("Пицца | Суши|Бургеры ").unwrap(), ["Пицца", "Суши", "Бургеры"]);
        assert_eq!(parse_options("да;нет").unwrap(), ["да", "нет"]);
        assert_eq!(parse_options("один"), None);
        assert_eq!(parse_options("a|a"), None);
        assert_eq!(parse_options("1|2|3|4|5|6"), None);
        assert_eq!(parse_options("a| |b").unwrap().len(), 2);
    }

    #[test]
    fn lines() {
        let options: Vec<String> = ["Пицца", "Суши", "Бургеры"].iter().map(|option| option.to_string()).collect();
        assert_eq!(poll_lines(&options, &[0, 0, 0], false),
                   "**Пицца**\n`            ` 0% · 0\n**Суши**\n`            ` 0% · 0\n**Бургеры**\n`            ` 0% · 0");
        assert_eq!(poll_lines(&options, &[3, 1, 0], true),
                   "**Пицца** 🏆\n`█████████   ` 75% · 3\n**Суши**\n`███         ` 25% · 1\n**Бургеры**\n`            ` 0% · 0");
        assert!(!poll_lines(&options, &[3, 1, 0], false).contains("🏆")); // пока опрос идёт, победителя не отмечаем
    }
}
