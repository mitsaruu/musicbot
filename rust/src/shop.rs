//! Магазин ролей: /shop, /shop_buy, для админов — /shop_add и /shop_remove.
//! Админ выставляет роль на продажу: цена и на сколько дней (0 — навсегда). Роль на время бот снимает сам.
//! Боту нужно право «Управлять ролями», и его роль должна быть выше продаваемых.

use crate::db;
use crate::reply::{display_name, say, take_money, whisper};
use crate::text::{money_text, plural};
use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use std::sync::Arc;

pub fn days_text(days: i64) -> String {
    if days == 0 {
        "навсегда".to_string()
    } else {
        format!("на {days} {}", plural(days, "день", "дня", "дней"))
    }
}

/// До какого времени действует роль: если она уже куплена и срок не вышел — продлеваем от конца срока.
pub fn new_expiry(current: Option<f64>, now: f64, days: i64) -> f64 {
    current.map_or(now, |expires| expires.max(now)) + days as f64 * 86400.0
}

/// Может ли бот выдавать эту роль: есть право «Управлять ролями», и роль бота выше неё.
fn bot_can_manage(guild: &serenity::Guild, bot: serenity::UserId, role: &serenity::Role) -> bool {
    let Some(member) = guild.members.get(&bot) else { return false };
    let roles: Vec<&serenity::Role> = member.roles.iter().filter_map(|id| guild.roles.get(id)).collect();
    let everyone = guild.roles.get(&serenity::RoleId::new(guild.id.get()));
    let allowed = guild.owner_id == bot
        || roles.iter().chain(everyone.iter()).any(|role| role.permissions.manage_roles() || role.permissions.administrator());
    let top = roles.iter().map(|role| role.position).max().unwrap_or(0);
    allowed && role.position < top
}

/// Роли в магазине сервера: (роль, цена, дней), от дешёвых к дорогим. Удалённые роли пропускаем.
fn shop_items(ctx: Context<'_>) -> Result<Vec<(serenity::Role, i64, i64)>, Error> {
    let guild_id = ctx.guild_id().unwrap();
    let rows = ctx.data().db.shop_roles(guild_id.get())?;
    let Some(guild) = ctx.guild() else { return Ok(Vec::new()) };
    Ok(rows
        .into_iter()
        .filter_map(|(role, price, days)| guild.roles.get(&serenity::RoleId::new(role)).map(|role| (role.clone(), price, days)))
        .collect())
}

async fn shop_autocomplete(ctx: Context<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    let partial = partial.to_lowercase();
    let choices = shop_items(ctx)
        .unwrap_or_default()
        .into_iter()
        .filter(|(role, _, _)| role.name.to_lowercase().contains(&partial))
        .take(25)
        .map(|(role, price, days)| {
            let label: String = format!("{} — {}, {}", role.name, money_text(price), days_text(days)).chars().take(100).collect();
            serenity::AutocompleteChoice::new(label, role.id.to_string())
        })
        .collect();
    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}

/// Магазин ролей: что можно купить за монеты
#[poise::command(slash_command, guild_only)]
pub async fn shop(ctx: Context<'_>) -> Result<(), Error> {
    let items = shop_items(ctx)?;
    if items.is_empty() {
        return whisper(ctx, "🛒 В магазине пока пусто. Админ может выставить роль командой /shop_add.").await;
    }
    let lines: Vec<String> = items
        .iter()
        .map(|(role, price, days)| format!("<@&{}> — **{}**, {}", role.id, money_text(*price), days_text(*days)))
        .collect();
    let embed = serenity::CreateEmbed::new()
        .color(0x7C6CF2)
        .title("🛒 Магазин ролей")
        .description(lines.join("\n"))
        .footer(serenity::CreateEmbedFooter::new("Купить — /shop_buy"));
    ctx.send(poise::CreateReply::default().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Купить роль из магазина за монеты
#[poise::command(slash_command, guild_only)]
pub async fn shop_buy(
    ctx: Context<'_>,
    #[description = "Какую роль купить"]
    #[autocomplete = "shop_autocomplete"]
    role: String,
) -> Result<(), Error> {
    let (data, guild_id, user) = (ctx.data(), ctx.guild_id().unwrap(), ctx.author().id);
    let Some((role, price, days)) = shop_items(ctx)?.into_iter().find(|(item, _, _)| item.id.to_string() == role) else {
        return whisper(ctx, "Такой роли в магазине нет — выбери из списка.").await;
    };
    let bought_until = data.db.shop_purchase(guild_id.get(), user.get(), role.id.get())?;
    let has_role = ctx.author_member().await.is_some_and(|member| member.roles.contains(&role.id));
    if has_role && (days == 0 || bought_until.is_none()) {
        return whisper(ctx, "Эта роль у тебя уже есть.").await;
    }
    let bot = ctx.framework().bot_id();
    if !ctx.guild().is_some_and(|guild| bot_can_manage(&guild, bot, &role)) {
        return whisper(ctx,
            "Не могу выдать эту роль: мне нужно право «Управлять ролями», и моя роль должна быть выше неё. Скажи админу.").await;
    }
    if !take_money(ctx, price, "цена роли").await? {
        return Ok(());
    }
    if ctx.http().add_member_role(guild_id, user, role.id, Some("Покупка в магазине ролей")).await.is_err() {
        // не получилось — возвращаем монеты
        data.db.add_money(guild_id.get(), user.get(), &display_name(ctx).await, data.economy.start_balance, price)?;
        return whisper(ctx, "Не получилось выдать роль — монеты вернул. Скажи админу.").await;
    }
    let mut text = format!("🛒 <@{user}> покупает роль <@&{}> за **{}**", role.id, money_text(price));
    if days > 0 {
        let expires = new_expiry(bought_until, db::now(), days);
        data.db.set_shop_purchase(guild_id.get(), user.get(), role.id.get(), expires)?;
        text += &format!(" — до <t:{}:f>", expires as i64);
    }
    say(ctx, text + ".").await
}

/// Выставить роль на продажу в магазине (или поменять цену)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn shop_add(
    ctx: Context<'_>,
    #[description = "Какую роль продавать"] role: serenity::Role,
    #[description = "Цена в монетах"]
    #[min = 1]
    #[max = 100_000_000]
    price: i64,
    #[description = "На сколько дней даётся роль (0 — навсегда)"]
    #[min = 0]
    #[max = 365]
    days: Option<i64>,
) -> Result<(), Error> {
    let (guild_id, days) = (ctx.guild_id().unwrap(), days.unwrap_or(0));
    if role.id.get() == guild_id.get() || role.managed {
        return whisper(ctx, "Эту роль продавать нельзя (она служебная).").await;
    }
    let permissions = role.permissions;
    if permissions.administrator() || permissions.manage_guild() || permissions.moderate_members() {
        return whisper(ctx, "У этой роли права админа или модератора — такие роли в магазин не ставлю.").await;
    }
    let bot = ctx.framework().bot_id();
    if !ctx.guild().is_some_and(|guild| bot_can_manage(&guild, bot, &role)) {
        return whisper(ctx,
            "Я не смогу выдавать эту роль: мне нужно право «Управлять ролями», и моя роль должна быть выше неё.").await;
    }
    ctx.data().db.set_shop_role(guild_id.get(), role.id.get(), price, days)?;
    whisper(ctx, format!("✅ <@&{}> в магазине: {}, {}.", role.id, money_text(price), days_text(days))).await
}

/// Убрать роль из магазина
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn shop_remove(ctx: Context<'_>, #[description = "Какую роль убрать"] role: serenity::Role) -> Result<(), Error> {
    let removed = ctx.data().db.remove_shop_role(ctx.guild_id().unwrap().get(), role.id.get())?;
    whisper(ctx, if removed {
        format!("✅ <@&{}> убрана из магазина. У кого она уже куплена — остаётся.", role.id)
    } else {
        "Этой роли и так нет в магазине.".to_string()
    }).await
}

/// Снимает роли, срок которых вышел. Вызывается раз в 10 минут.
pub async fn expire_roles(http: &serenity::Http, database: &db::Database) {
    let expired = match database.expired_shop_purchases(db::now()) {
        Ok(expired) => expired,
        Err(error) => return println!("Магазин ролей: база не ответила: {error}"),
    };
    for (guild, user, role) in expired {
        let result = http
            .remove_member_role(
                serenity::GuildId::new(guild), serenity::UserId::new(user), serenity::RoleId::new(role),
                Some("Закончился срок роли из магазина"))
            .await;
        match result {
            Ok(()) => {}
            // 404 — человек ушёл с сервера или роль удалили: снимать нечего, запись убираем
            Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)))
                if response.status_code.as_u16() == 404 => {}
            Err(error) => {
                println!("Магазин ролей: не получилось снять роль: {error}");
                continue; // попробуем в следующий раз
            }
        }
        if let Err(error) = database.remove_shop_purchase(guild, user, role) {
            println!("Магазин ролей: база не ответила: {error}");
        }
    }
}

/// Запускает проверку сроков ролей в фоне.
pub fn start_expiry_loop(http: Arc<serenity::Http>, database: Arc<db::Database>) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(600));
        loop {
            timer.tick().await;
            expire_roles(&http, &database).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_and_expiry() {
        assert_eq!([days_text(0), days_text(1), days_text(3), days_text(7), days_text(21)],
                   ["навсегда", "на 1 день", "на 3 дня", "на 7 дней", "на 21 день"]);
        assert_eq!(new_expiry(None, 1000.0, 1), 1000.0 + 86400.0);
        assert_eq!(new_expiry(Some(5000.0), 1000.0, 1), 5000.0 + 86400.0); // ещё действует — продлеваем от конца
        assert_eq!(new_expiry(Some(500.0), 1000.0, 2), 1000.0 + 2.0 * 86400.0); // срок давно вышел — от сейчас
    }
}
