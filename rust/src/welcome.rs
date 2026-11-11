//! Приветствие новичков (/welcome_channel). Когда на сервер заходит новый участник, бот пишет в выбранный канал
//! приветствие (его сочиняет ИИ, а без ИИ — обычный текст) и прикладывает картинку-карточку.
//! Нужно разрешение Server Members Intent и settings.ini → [welcome] members_intent = on.

use crate::admin::{bot_permissions, report_error};
use crate::draw::{self, Anchor, Fonts};
use crate::reply::whisper;
use crate::{ai, Context, Data, Error};
use image::RgbaImage;
use poise::serenity_prelude as serenity;

const WELCOME_TASK: &str = "На Discord-сервер «{server}» только что зашёл новый участник по имени {name}.
Поприветствуй его от себя ОДНОЙ короткой фразой в своём характере. Не используй упоминания, не задавай вопросов
о личном и не пиши ничего, кроме самого приветствия.";

/// Включено ли событие «зашёл новый участник» (settings.ini → [welcome] members_intent).
pub fn members_intent(settings: &crate::config::Settings) -> bool {
    ["on", "да", "yes", "1"].contains(&settings.text("welcome", "members_intent", "off").trim().to_lowercase().as_str())
}

/// Картинка-приветствие 1000×300 в том же стиле, что и карточка /rank.
pub fn welcome_card(fonts: &Fonts, name: &str, avatar: Option<&[u8]>, server: &str, member_number: u64) -> RgbaImage {
    let (mut card, _) = draw::background(1000, 300, 36.0, None, (0.5, 0.5), 140, 0);
    draw::paste_rounded(&mut card, &draw::gradient(216, 216, draw::ACCENT_1, draw::ACCENT_2), (42, 42), 108.0);
    draw::fill_rounded(&mut card, (48, 48, 204, 204), 102.0, draw::BG_LEFT, 255);
    image::imageops::overlay(&mut card, &draw::avatar(avatar, 192, name, fonts), 54, 54);
    let (bold, regular) = (&fonts.bold, &fonts.regular);
    draw::text(&mut card, (300.0, 100.0), Anchor::LeftBaseline, bold, 30.0, draw::TEXT_DIM, "ДОБРО ПОЖАЛОВАТЬ");
    draw::text(&mut card, (298.0, 178.0), Anchor::LeftBaseline, bold, 64.0, draw::WHITE, &draw::fit_text(bold, 64.0, name, 650.0));
    let info = draw::fit_text(regular, 28.0, &format!("{server} · участник №{member_number}"), 650.0);
    draw::text(&mut card, (300.0, 232.0), Anchor::LeftBaseline, regular, 28.0, draw::TEXT_DIM, &info);
    card
}

/// Приветствие от ИИ; если ИИ нет или он не ответил — обычная фраза.
async fn welcome_text(data: &Data, server: &str, name: &str) -> String {
    if data.ai.ready() {
        let task = WELCOME_TASK.replace("{server}", server).replace("{name}", name);
        match ai::generate(data, &ai::system_prompt(data), &[ai::Turn::user(task)]).await {
            Ok(answer) if !ai::has_ignore_tag(&answer.text) => {
                let text: String = ai::to_one_line(&ai::without_tags(&answer.text)).chars().take(500).collect();
                if !text.is_empty() {
                    return text;
                }
            }
            Ok(_) => {}
            Err(error) => println!("Приветствие: ИИ не ответил: {error}"),
        }
    }
    "Добро пожаловать! Располагайся.".to_string()
}

/// Новый участник на сервере (событие приходит, только если включён members_intent).
pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    if member.user.bot {
        return;
    }
    let Ok(Some(channel)) = data.db.welcome_channel(member.guild_id.get()) else { return };
    let channel = serenity::ChannelId::new(channel);
    let (server, count) = ctx.cache.guild(member.guild_id).map(|guild| (guild.name.clone(), guild.member_count)).unwrap_or_default();
    let name = member.display_name().to_string();
    let text = welcome_text(data, &server, &name).await;
    let avatar = crate::levels::fetch_avatar(data, &member.face()).await;
    let dir = data.dir.clone();
    let (card_name, card_server) = (name.clone(), server.clone());
    let png = tokio::task::spawn_blocking(move || {
        draw::fonts(&dir).map(|fonts| draw::to_png(&welcome_card(fonts, &card_name, avatar.as_deref(), &card_server, count)))
    })
    .await
    .ok()
    .flatten();
    let mut message = serenity::CreateMessage::new()
        .content(format!("👋 <@{}> {text}", member.user.id))
        .allowed_mentions(serenity::CreateAllowedMentions::new().users([member.user.id]));
    if let Some(png) = png {
        message = message.add_file(serenity::CreateAttachment::bytes(png, "welcome.png"));
    }
    if let Err(error) = channel.send_message(ctx, message).await {
        report_error(&ctx.http, data, Some(member.guild_id), "Приветствие новичка", &error.to_string()).await;
    }
}

/// Канал, где бот приветствует новых участников (пусто — выключить)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "welcome_channel")]
pub async fn set_welcome_channel(
    ctx: Context<'_>,
    #[description = "Канал для приветствий. Пусто — выключить"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let Some(channel) = channel else {
        data.db.set_welcome_channel(guild, None)?;
        return whisper(ctx, "✅ Больше не приветствую новичков.").await;
    };
    let allowed = bot_permissions(ctx, &channel).is_some_and(|p| p.view_channel() && p.send_messages() && p.attach_files());
    if !allowed {
        return whisper(ctx, format!(
            "Мне не хватает прав в <#{}>: нужны «Просматривать канал», «Отправлять сообщения» и «Прикреплять файлы».",
            channel.id)).await;
    }
    data.db.set_welcome_channel(guild, Some(channel.id.get()))?;
    let mut text = format!("✅ Буду приветствовать новичков в <#{}>.", channel.id);
    if !data.members_intent {
        text.push_str("\n⚠️ Пока это не заработает: боту нужно разрешение. Developer Portal → Bot → включить \
                       **Server Members Intent**, потом в `settings.ini` в разделе `[welcome]` поставить \
                       `members_intent = on` и перезапустить бота.");
    }
    whisper(ctx, text).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_setting() {
        assert!(!members_intent(&crate::config::Settings::default()));
        assert!(members_intent(&crate::config::Settings::parse("[welcome]\nmembers_intent = ДА\n")));
    }

    #[test]
    fn card_draws() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let Some(fonts) = draw::fonts(&dir) else { return }; // без шрифтов рисовать нечем
        let card = welcome_card(fonts, "Очень длинное имя нового участника сервера", None, "Сервер", 42);
        assert_eq!(card.dimensions(), (1000, 300));
    }
}
