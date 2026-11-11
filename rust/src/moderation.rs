//! Варны и тайм-ауты по просьбе модератора. Модератор пишет боту «дай варн @Вася за спам» или «кинь @Петю
//! в тайм-аут на час» — ИИ ставит в ответ метку, а бот её выполняет. Слушается бот только тех, у кого есть право
//! «Отправлять участников подумать» (Moderate Members) или права администратора, и только против тех,
//! чья роль ниже, чем у просящего. Здесь же — автонаказания за набранные варны (settings.ini → warn_punishments).

use crate::admin::report_error;
use crate::ai::minutes_text;
use crate::{db, Data};
use poise::serenity_prelude as serenity;

pub const DEFAULT_TIMEOUT: i64 = 10; // минут, если модератор не назвал срок
const MAX_ACTIONS: usize = 3; // больше наказаний за один ответ ИИ не выполняем
pub const MAX_TIMEOUT: i64 = 28 * 24 * 60; // Discord: тайм-аут не дольше 28 дней

/// «3: 60, 5: 1440» → [(3, 60), (5, 1440)] — сколько варнов и сколько минут тайм-аута. off → [].
pub fn parse_warn_punishments(text: &str) -> Vec<(i64, i64)> {
    if ["off", "нет", "no", "0"].contains(&text.trim().to_lowercase().as_str()) {
        return Vec::new();
    }
    let mut pairs: Vec<(i64, i64)> = text
        .split(',')
        .filter_map(|pair| {
            let (count, minutes) = pair.split_once(':')?;
            Some((count.trim().parse().ok()?, minutes.trim().parse().ok()?))
        })
        .collect();
    pairs.sort();
    pairs
}

/// Сколько минут тайм-аута положено за варн номер count (0 — ничего).
pub fn warn_timeout_minutes(punishments: &[(i64, i64)], count: i64) -> i64 {
    match punishments.last() {
        None => 0,
        // дальше последнего порога — каждый варн снова самый длинный тайм-аут
        Some(&(last_count, last_minutes)) if count > last_count => last_minutes,
        _ => punishments.iter().find(|(needed, _)| *needed == count).map_or(0, |(_, minutes)| *minutes),
    }
}

/// Что попросил сделать ИИ.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Warn,
    Timeout,
    RemoveTimeout,
}

/// Метка из ответа ИИ: что сделать, с кем и остаток текста (причина, у тайм-аута — сначала минуты).
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: Kind,
    pub target: u64,
    pub rest: String,
}

/// Разбирает содержимое скобок: «ВАРН: 123 спам», «тайм-аут: 5 60 флуд», «СНЯТЬ ТАЙМАУТ: 5».
fn parse_inside(inside: &str) -> Option<Action> {
    let upper = inside.trim_start().to_uppercase();
    let (name, after) = upper.split_once(':')?;
    let kind = match name.trim_end().replace('-', "").replace(' ', "_").as_str() {
        "ВАРН" => Kind::Warn,
        "ТАЙМАУТ" => Kind::Timeout,
        "СНЯТЬ_ТАЙМАУТ" => Kind::RemoveTimeout,
        _ => return None,
    };
    // Причину берём из исходного текста (без перевода в верхний регистр): та же длина после «:»
    let original_after = &inside[inside.find(':')? + 1..];
    let after_trimmed = original_after.trim_start();
    let digits: String = after_trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || after.trim_start().is_empty() {
        return None;
    }
    let target = digits.parse().ok()?;
    Some(Action { kind, target, rest: after_trimmed[digits.len()..].trim().to_string() })
}

/// Достаёт метки наказаний из ответа ИИ. Возвращает (текст без меток, метки).
pub fn take_actions(text: &str) -> (String, Vec<Action>) {
    let (mut result, mut actions, mut rest) = (String::new(), Vec::new(), text);
    while let Some(start) = rest.find('[') {
        let Some(length) = rest[start..].find(']') else { break };
        result.push_str(&rest[..start]);
        match parse_inside(&rest[start + 1..start + length]) {
            Some(action) => actions.push(action),
            None => result.push_str(&rest[start..=start + length]),
        }
        rest = &rest[start + length + 1..];
    }
    result.push_str(rest);
    (result.trim().to_string(), actions)
}

/// Тайм-аут: «60 флуд» → (60, «флуд»); без числа — DEFAULT_TIMEOUT минут.
pub fn timeout_details(rest: &str) -> (i64, String) {
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (minutes, reason) = match digits.parse::<i64>() {
        Ok(minutes) => (minutes, rest[digits.len()..].trim()),
        Err(_) => (DEFAULT_TIMEOUT, rest.trim()),
    };
    let reason = if reason.is_empty() { "по просьбе модератора" } else { reason };
    (minutes.clamp(1, MAX_TIMEOUT), reason.to_string())
}

/// Id упомянутых участников в тексте: <@123> или <@!123>.
pub fn mentioned_ids(text: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    for part in text.split("<@").skip(1) {
        let part = part.strip_prefix('!').unwrap_or(part);
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && part[digits.len()..].starts_with('>') {
            if let Ok(id) = digits.parse() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// Права участника на сервере (по данным, которые бот держит в памяти).
pub fn permissions(ctx: &serenity::Context, guild: serenity::GuildId, member: &serenity::Member) -> serenity::Permissions {
    #[allow(deprecated)]
    ctx.cache.guild(guild).map(|guild| guild.member_permissions(member)).unwrap_or_else(serenity::Permissions::empty)
}

/// Может ли человек просить бота о варнах и тайм-аутах.
pub fn is_moderator(ctx: &serenity::Context, guild: serenity::GuildId, member: &serenity::Member) -> bool {
    let permissions = permissions(ctx, guild, member);
    !member.user.bot && (permissions.moderate_members() || permissions.administrator())
}

/// Самая высокая роль участника: (позиция, id). Без ролей — (0, 0), как у @everyone.
fn top_role(guild: &serenity::Guild, member: &serenity::Member) -> (u16, u64) {
    member
        .roles
        .iter()
        .filter_map(|id| guild.roles.get(id))
        .map(|role| (role.position, role.id.get()))
        .max()
        .unwrap_or((0, 0))
}

/// Выше ли модератор по роли, чем тот, кого он хочет наказать (владельца сервера наказать нельзя).
pub fn outranks(ctx: &serenity::Context, guild: serenity::GuildId, moderator: &serenity::Member, target: &serenity::Member) -> bool {
    let Some(guild) = ctx.cache.guild(guild) else { return false };
    if target.user.id == guild.owner_id {
        return false;
    }
    moderator.user.id == guild.owner_id || top_role(&guild, moderator) > top_role(&guild, target)
}

/// Discord отказал из-за прав (403)?
pub fn is_forbidden(error: &serenity::Error) -> bool {
    matches!(error, serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) if response.status_code.as_u16() == 403)
}

/// Тайм-аут на minutes минут (None — снять тайм-аут).
pub async fn set_timeout(ctx: &serenity::Context, guild: serenity::GuildId, user: serenity::UserId, minutes: Option<i64>, reason: &str) -> Result<(), serenity::Error> {
    let edit = match minutes {
        Some(minutes) => {
            let until = serenity::Timestamp::from_unix_timestamp(db::now() as i64 + minutes * 60).unwrap_or_else(|_| serenity::Timestamp::now());
            serenity::EditMember::new().disable_communication_until_datetime(until)
        }
        None => serenity::EditMember::new().enable_communication(),
    };
    guild.edit_member(ctx, user, edit.audit_log_reason(reason)).await.map(|_| ())
}

/// Записывает варн и, если набралось варнов на автонаказание, даёт тайм-аут.
/// Возвращает (номер варна, на сколько минут дали тайм-аут — None, если не положено или не получилось).
pub async fn give_warn(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, user: serenity::UserId, moderator: u64, reason: &str, giver: &str) -> Result<(i64, Option<i64>), crate::Error> {
    let count = data.db.add_warn(guild.get(), user.get(), moderator, reason, db::now())?;
    let minutes = warn_timeout_minutes(&data.warn_punishments, count).min(MAX_TIMEOUT);
    if minutes == 0 {
        return Ok((count, None));
    }
    let full_reason = format!("Варн №{count}: {reason}{giver}");
    match set_timeout(ctx, guild, user, Some(minutes), &full_reason).await {
        Ok(()) => Ok((count, Some(minutes))),
        Err(error) => {
            println!("Не получилось дать тайм-аут за варн {user}: {error}"); // не хватает прав — останется только варн
            Ok((count, None))
        }
    }
}

/// Подсказка для ИИ: есть ли у собеседника права модератора и кого упомянули (с id).
pub async fn context_note(ctx: &serenity::Context, guild: serenity::GuildId, user: &serenity::Member, question: &str, targets: &[serenity::Member]) -> String {
    let bot = ctx.cache.current_user().id;
    let mut members: Vec<serenity::Member> = Vec::new();
    for member in targets {
        if member.user.id != bot && !members.iter().any(|known| known.user.id == member.user.id) {
            members.push(member.clone());
        }
    }
    for id in mentioned_ids(question) {
        if id == bot.get() || members.iter().any(|known| known.user.id.get() == id) {
            continue;
        }
        if let Ok(member) = guild.member(ctx, id).await {
            members.push(member);
        }
    }
    let mut text = if is_moderator(ctx, guild, user) {
        format!("{} — модератор: может просить варны и тайм-ауты.", user.display_name())
    } else {
        format!("Собеседник ({}) без прав модератора: варны и тайм-ауты по его просьбе не выдавай.", user.display_name())
    };
    if !members.is_empty() {
        let list: Vec<String> = members.iter().map(|member| format!("{} (id {})", member.display_name(), member.user.id)).collect();
        text.push_str(&format!("\nУпомянутые участники: {}", list.join(", ")));
    }
    text
}

/// Кто просит наказать: сервер и сам участник.
pub struct Requester<'a> {
    pub ctx: &'a serenity::Context,
    pub guild: serenity::GuildId,
    pub member: &'a serenity::Member,
}

/// Выполняет метки модерации из ответа ИИ. Возвращает строки с итогом.
pub async fn run_actions(data: &Data, requester: &Requester<'_>, actions: Vec<Action>) -> Vec<String> {
    let Requester { ctx, guild, member: user } = *requester;
    if actions.is_empty() {
        return Vec::new();
    }
    if !is_moderator(ctx, guild, user) {
        // ИИ не должна была ставить метку — но на всякий случай проверяем сами
        println!("ИИ поставила метку наказания по просьбе не-модератора {} — не выполняю", user.user.id);
        return Vec::new();
    }
    let giver = user.display_name().to_string();
    let mut results = Vec::new();
    for action in actions.into_iter().take(MAX_ACTIONS) {
        let Ok(target) = guild.member(ctx, action.target).await else {
            results.push("❓ Не нашла такого участника на сервере.".to_string());
            continue;
        };
        let mention = format!("<@{}>", target.user.id);
        if target.user.bot {
            results.push(format!("🤖 {mention} — бот, ботов не наказываю."));
            continue;
        }
        if !outranks(ctx, guild, user, &target) {
            results.push(format!("⛔ {mention} по роли не ниже тебя — наказать не могу."));
            continue;
        }
        let outcome: Result<(), serenity::Error> = match action.kind {
            Kind::Warn => {
                let reason = if action.rest.is_empty() { "без причины".to_string() } else { action.rest.clone() };
                match give_warn(ctx, data, guild, target.user.id, user.user.id.get(), &reason, &format!(" (выдал {giver})")).await {
                    Ok((count, minutes)) => {
                        results.push(format!("⚠️ {mention} получает варн №{count}: {reason}"));
                        if let Some(minutes) = minutes {
                            results.push(format!("🔇 За {count}-й варн {mention} в тайм-ауте на {}.", minutes_text(minutes)));
                        }
                        Ok(())
                    }
                    Err(error) => {
                        report_error(&ctx.http, data, Some(guild), "Варн по просьбе модератора", &error.to_string()).await;
                        results.push(format!("😵 Не получилось наказать {mention}."));
                        continue;
                    }
                }
            }
            Kind::Timeout => {
                let (minutes, reason) = timeout_details(&action.rest);
                let result = set_timeout(ctx, guild, target.user.id, Some(minutes), &format!("{reason} (попросил {giver})")).await;
                if result.is_ok() {
                    results.push(format!("🔇 {mention} в тайм-ауте на {}: {reason}", minutes_text(minutes)));
                }
                result
            }
            Kind::RemoveTimeout => {
                let result = set_timeout(ctx, guild, target.user.id, None, &format!("Снять тайм-аут попросил {giver}")).await;
                if result.is_ok() {
                    results.push(format!("🔊 С {mention} снят тайм-аут."));
                }
                result
            }
        };
        match outcome {
            Ok(()) => {}
            Err(error) if is_forbidden(&error) => results.push(format!(
                "⚠️ Не хватает прав, чтобы наказать {mention}: нужно право «Отправлять участников подумать», и моя роль должна быть выше его роли.")),
            Err(error) => {
                report_error(&ctx.http, data, Some(guild), "Наказание по просьбе модератора", &error.to_string()).await;
                results.push(format!("😵 Не получилось наказать {mention}."));
            }
        }
    }
    if !results.is_empty() {
        send_log(ctx, data, guild, &format!("👮 По просьбе <@{}>:\n{}", user.user.id, results.join("\n"))).await;
    }
    results
}

/// Пишет в канал логов сервера (если он выбран). Без пингов.
pub async fn send_log(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, text: &str) {
    let Ok(Some(channel)) = data.db.log_channel(guild.get()) else { return };
    let message = serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new());
    let _ = serenity::ChannelId::new(channel).send_message(ctx, message).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warn_punishments() {
        let steps = parse_warn_punishments("5: 1440, 3: 60, 7: 10080, мусор");
        assert_eq!(steps, [(3, 60), (5, 1440), (7, 10080)]);
        let minutes: Vec<i64> = (1..=9).map(|count| warn_timeout_minutes(&steps, count)).collect();
        assert_eq!(minutes, [0, 0, 60, 0, 1440, 0, 10080, 10080, 10080]);
        assert!(parse_warn_punishments("off").is_empty());
        assert_eq!(warn_timeout_minutes(&[], 5), 0);
    }

    #[test]
    fn mod_tags() {
        let (text, actions) = take_actions("Ладно, бака. [ВАРН: 123 спам в чате] [тайм-аут:45 60 флуд] [Снять таймаут: 7]");
        assert_eq!(text, "Ладно, бака.");
        assert_eq!(actions, [
            Action { kind: Kind::Warn, target: 123, rest: "спам в чате".into() },
            Action { kind: Kind::Timeout, target: 45, rest: "60 флуд".into() },
            Action { kind: Kind::RemoveTimeout, target: 7, rest: String::new() },
        ]);
        // без id и чужие скобки не трогаем
        let (text, actions) = take_actions("массив a[0], [ВАРН: Вася] и [ссылка](url)");
        assert_eq!((text.as_str(), actions.len()), ("массив a[0], [ВАРН: Вася] и [ссылка](url)", 0));
        assert_eq!(timeout_details("60 флуд"), (60, "флуд".into()));
        assert_eq!(timeout_details(""), (10, "по просьбе модератора".into()));
        assert_eq!(timeout_details("99999999 x").0, MAX_TIMEOUT);
        assert_eq!(mentioned_ids("дай варн <@12> и <@!34>, <@12> ещё раз, <@abc>"), [12, 34]);
    }
}
