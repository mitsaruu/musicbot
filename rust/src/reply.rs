//! Общие мелочи для команд: ответы в чат и имя участника.

use crate::text::money_text;
use crate::{Context, Error};
use poise::serenity_prelude as serenity;

/// Ответ, который видит только вызвавший команду.
pub async fn whisper(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).ephemeral(true)).await?;
    Ok(())
}

/// Обычный ответ в чат без пингов (упоминания видны, но уведомление не приходит).
pub async fn say(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(text).allowed_mentions(serenity::CreateAllowedMentions::new()))
        .await?;
    Ok(())
}

/// Имя участника на сервере (ник), а если его не узнать — обычное имя.
pub async fn display_name(ctx: Context<'_>) -> String {
    match ctx.author_member().await {
        Some(member) => member.display_name().to_string(),
        None => user_name(ctx.author()),
    }
}

/// Имя пользователя без сервера: отображаемое, а если его нет — логин.
pub fn user_name(user: &serenity::User) -> String {
    user.global_name.clone().unwrap_or_else(|| user.name.clone())
}

/// Проверяет, хватает ли монет, и списывает их (ставка, цена кейса, корм — what).
/// Ok(true) — списали. Ok(false) — не хватило, человеку уже ответили.
pub async fn take_money(ctx: Context<'_>, amount: i64, what: &str) -> Result<bool, Error> {
    let (data, guild, user) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get());
    let name = display_name(ctx).await;
    let (balance, _) = data.db.wallet(guild, user, &name, data.economy.start_balance)?;
    if amount > balance {
        whisper(ctx, format!(
            "Не хватает монет: у тебя {}, а {what} — {}. Заработать можно через /work.",
            money_text(balance), money_text(amount))).await?;
        return Ok(false);
    }
    data.db.add_money(guild, user, &name, data.economy.start_balance, -amount)?;
    Ok(true)
}

/// Предложение другому участнику с кнопками «Принять» / «Отказаться» (бой питомцев, обмен).
/// Нажать может только тот, кого позвали. Возвращает нажатие «Принять» — на него нужно ответить
/// (обычно правкой сообщения). None — отказался или не ответил за 2 минуты; сообщение уже поправлено.
pub async fn challenge(
    ctx: Context<'_>,
    target: serenity::UserId,
    text: String,
) -> Result<Option<serenity::ComponentInteraction>, Error> {
    let buttons = vec![serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("challenge:accept").label("Принять").style(serenity::ButtonStyle::Success),
        serenity::CreateButton::new("challenge:decline").label("Отказаться").style(serenity::ButtonStyle::Secondary),
    ])];
    let reply = ctx
        .send(poise::CreateReply::default()
            .content(text)
            .components(buttons)
            .allowed_mentions(serenity::CreateAllowedMentions::new().users(vec![target])))
        .await?;
    let message_id = reply.message().await?.id;
    let no_pings = serenity::CreateAllowedMentions::new();
    loop {
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message_id)
            .timeout(std::time::Duration::from_secs(120))
            .await;
        let Some(press) = press else {
            let expired = poise::CreateReply::default()
                .content(format!("⌛ <@{target}> не ответил — предложение отменено."))
                .components(Vec::new())
                .allowed_mentions(no_pings);
            let _ = reply.edit(ctx, expired).await;
            return Ok(None);
        };
        if press.user.id != target {
            let note = serenity::CreateInteractionResponseMessage::new().content("Это предложение не тебе 🙂").ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(note)).await?;
            continue;
        }
        if press.data.custom_id == "challenge:accept" {
            return Ok(Some(press));
        }
        let declined = serenity::CreateInteractionResponseMessage::new()
            .content(format!("🙅 <@{target}> отказывается."))
            .components(Vec::new())
            .allowed_mentions(no_pings);
        press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(declined)).await?;
        return Ok(None);
    }
}

/// Ответ на принятое предложение: заменяет текст сообщения и убирает кнопки.
pub async fn finish_challenge(ctx: Context<'_>, press: &serenity::ComponentInteraction, text: String) -> Result<(), Error> {
    let update = serenity::CreateInteractionResponseMessage::new()
        .content(text)
        .components(Vec::new())
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
    Ok(())
}
