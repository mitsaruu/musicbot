//! Патчноуты. Что нового — в файле patchnotes.txt (в папке бота), новая запись сверху.
//! После перезапуска бот сам публикует записи, которых ещё не было, в канал из /patchnotes_channel.

use crate::admin::bot_permissions;
use crate::reply::whisper;
use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;

const MAX_AT_ONCE: usize = 3; // если бот долго не перезапускали — публикуем не больше стольких записей разом

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: String, // номер, например "v5" — по нему бот понимает, что запись ещё не публиковал
    pub date: String,
    pub title: String,
    pub items: Vec<String>,
}

/// Текст patchnotes.txt → список записей, новая — первая. Заголовок: «## номер | дата | название».
pub fn parse(text: &str) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix("## ") {
            let mut parts = header.split('|').map(|part| part.trim().to_string());
            let mut next = || parts.next().unwrap_or_default();
            notes.push(Note { id: next(), date: next(), title: next(), items: Vec::new() });
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(note) = notes.last_mut() {
                let item = line.strip_prefix("- ").or_else(|| line.strip_prefix("• ")).unwrap_or(line);
                note.items.push(item.to_string());
            }
        }
    }
    notes.retain(|note| !note.id.is_empty());
    notes
}

fn load(data: &Data) -> Vec<Note> {
    parse(&std::fs::read_to_string(data.dir.join("patchnotes.txt")).unwrap_or_default())
}

/// Записи новее last_note — от старой к новой. Если last_note в файле не нашёлся — только самая новая.
pub fn unpublished<'a>(notes: &'a [Note], last_note: Option<&str>) -> Vec<&'a Note> {
    let known = last_note.and_then(|last| notes.iter().position(|note| note.id == last));
    let fresh = match known {
        Some(position) => &notes[..position],
        None => &notes[..notes.len().min(1)],
    };
    fresh.iter().take(MAX_AT_ONCE).rev().collect()
}

fn embed(note: &Note) -> serenity::CreateEmbed {
    let title = if note.title.is_empty() { format!("📝 Обновление {}", note.id) } else { format!("📝 Обновление {} — {}", note.id, note.title) };
    let mut text: String = note.items.iter().map(|item| format!("• {item}")).collect::<Vec<_>>().join("\n").chars().take(4000).collect();
    if text.is_empty() {
        text = "Мелкие исправления".to_string();
    }
    let embed = serenity::CreateEmbed::new().color(0x7C6CF2).title(title).description(text);
    if note.date.is_empty() { embed } else { embed.footer(serenity::CreateEmbedFooter::new(&note.date)) }
}

/// При запуске: на каждом сервере публикуем патчноуты, которых там ещё не было.
pub async fn post_new(http: &serenity::Http, data: &Data) -> Result<(), Error> {
    let notes = load(data);
    for (guild, channel, last_note) in data.db.patchnotes_channels()? {
        for note in unpublished(&notes, last_note.as_deref()) {
            let message = serenity::CreateMessage::new().embed(embed(note));
            if let Err(error) = serenity::ChannelId::new(channel).send_message(http, message).await {
                println!("Патчноуты: не получилось написать в канал {channel}: {error}");
                break;
            }
            data.db.set_last_patchnote(guild, &note.id)?;
        }
    }
    Ok(())
}

async fn version_autocomplete(ctx: Context<'_>, partial: &str) -> serenity::CreateAutocompleteResponse {
    let partial = partial.to_lowercase();
    let choices = load(ctx.data())
        .into_iter()
        .filter(|note| format!("{} {}", note.id, note.title).to_lowercase().contains(&partial))
        .take(25)
        .map(|note| {
            let label: String = format!("{} — {}", note.id, note.title).chars().take(100).collect();
            serenity::AutocompleteChoice::new(label, note.id)
        })
        .collect();
    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}

/// Что нового в боте: последний патчноут или любой прошлый
#[poise::command(slash_command, rename = "patchnotes")]
pub async fn show(
    ctx: Context<'_>,
    #[description = "Какой патчноут показать (если не выбрать — последний)"]
    #[autocomplete = "version_autocomplete"]
    version: Option<String>,
) -> Result<(), Error> {
    let notes = load(ctx.data());
    let note = match &version {
        Some(version) => notes.iter().find(|note| note.id == *version),
        None => notes.first(),
    };
    match note {
        Some(note) => {
            ctx.send(poise::CreateReply::default().embed(embed(note)).ephemeral(true)).await?;
            Ok(())
        }
        None => whisper(ctx, "Такого патчноута нет.").await,
    }
}

/// Канал, куда бот пишет патчноуты после обновлений (пусто — не писать)
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "patchnotes_channel")]
pub async fn set_channel(
    ctx: Context<'_>,
    #[description = "Канал для патчноутов. Пусто — выключить"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let Some(channel) = channel else {
        data.db.set_patchnotes_channel(guild, None)?;
        return whisper(ctx, "✅ Больше не пишу патчноуты.").await;
    };
    let allowed = bot_permissions(ctx, &channel).is_some_and(|p| p.view_channel() && p.send_messages() && p.embed_links());
    if !allowed {
        return whisper(ctx, format!(
            "Мне не хватает прав в <#{}>: нужны «Просматривать канал», «Отправлять сообщения» и «Встраивать ссылки».",
            channel.id)).await;
    }
    let notes = load(data);
    let latest = notes.first();
    data.db.set_patchnotes_channel(guild, Some((channel.id.get(), latest.map(|note| note.id.as_str()))))?;
    whisper(ctx, format!(
        "✅ Теперь после каждого обновления пишу патчноут в <#{}>. Выключить — /patchnotes_channel без канала.", channel.id)).await?;
    if let Some(latest) = latest {
        // сразу показываем последний — чтобы было видно, как выглядит
        channel.id.send_message(ctx, serenity::CreateMessage::new().embed(embed(latest))).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Патчноуты бота\n\n## v3 | 10.11.2026 | Третье\n- раз\n• два\nтри\n\n## v2 | 09.11.2026 | Второе\n- а\n\n## v1 | 08.11.2026\n";

    #[test]
    fn parsing() {
        let notes = parse(FILE);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0], Note { id: "v3".into(), date: "10.11.2026".into(), title: "Третье".into(),
                                    items: vec!["раз".into(), "два".into(), "три".into()] });
        assert_eq!((notes[2].id.as_str(), notes[2].title.as_str(), notes[2].items.len()), ("v1", "", 0));
        assert!(parse("строка без заголовка\n## | пусто").is_empty());
    }

    #[test]
    fn what_to_publish() {
        let notes = parse(FILE);
        let ids = |last: Option<&str>| unpublished(&notes, last).iter().map(|note| note.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Some("v1")), ["v2", "v3"]); // от старой к новой
        assert!(ids(Some("v3")).is_empty()); // всё уже опубликовано
        assert_eq!(ids(Some("v0")), ["v3"]); // такой записи нет — только самая новая
        assert_eq!(ids(None), ["v3"]);
        assert!(unpublished(&[], None).is_empty());
    }

    #[test]
    fn real_file_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("patchnotes.txt");
        let notes = parse(&std::fs::read_to_string(path).unwrap());
        assert!(notes.len() >= 9 && notes.iter().all(|note| note.id.starts_with('v') && !note.items.is_empty()));
    }
}
