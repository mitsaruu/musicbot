//! Мелкие игры: /dice, /roll, /predskazanie, /shar и шуточная игра /dick_growth, /dick_size, /dick_leaderboard.

use crate::cards::{Picture, Row};
use crate::levels::{avatar_by_id, render_table, send_png};
use crate::reply::{display_name, whisper};
use crate::text::{hours_text, plural, time_left_text};
use crate::{db, Context, Error};
use poise::serenity_prelude as serenity;
use rand::seq::IndexedRandom;
use rand::Rng;

const DICE_FACES: [&str; 6] = ["⚀", "⚁", "⚂", "⚃", "⚄", "⚅"]; // грани кубика от 1 до 6

/// Тексты предсказаний лежат в predictions.txt рядом с этим файлом: строки после «[day]» — для /predskazanie,
/// после «[ball]» — ответы магического шара. {name} в тексте заменяется на упоминание того, кто вызвал команду.
const PREDICTIONS: &str = include_str!("predictions.txt");

/// Строки нужного раздела из predictions.txt.
pub fn predictions(section: &str) -> Vec<&'static str> {
    let mut current = "";
    let mut lines = Vec::new();
    for line in PREDICTIONS.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            current = name;
        } else if current == section {
            lines.push(line);
        }
    }
    lines
}

/// Текст броска костей (без упоминания игрока).
pub fn dice_text(rolls: &[u32]) -> String {
    let faces: Vec<&str> = rolls.iter().map(|roll| DICE_FACES[(*roll as usize - 1).min(5)]).collect();
    let mut text = format!("# {}\n", faces.join(" "));
    if let [single] = rolls {
        text += &format!("Выпало **{single}**");
    } else {
        let sum: u32 = rolls.iter().sum();
        text += &format!("{} = **{sum}**", rolls.iter().map(u32::to_string).collect::<Vec<_>>().join(" + "));
        if rolls.windows(2).all(|pair| pair[0] == pair[1]) {
            text += "\n🔥 Дубль!"; // все кости одинаковые
        }
    }
    text
}

/// Кинуть игральные кости
#[poise::command(slash_command)]
pub async fn dice(
    ctx: Context<'_>,
    #[description = "Сколько костей кинуть (от 1 до 10)"]
    #[min = 1]
    #[max = 10]
    count: Option<u32>,
) -> Result<(), Error> {
    let count = count.unwrap_or(2);
    let rolls: Vec<u32> = {
        let mut rng = rand::rng();
        (0..count).map(|_| rng.random_range(1..=6)).collect()
    };
    ctx.say(format!(
        "🎲 <@{}> кидает {count} {}:\n{}", ctx.author().id, plural(count as i64, "кость", "кости", "костей"), dice_text(&rolls)))
        .await?;
    Ok(())
}

/// Случайное число от 1 до 100
#[poise::command(slash_command)]
pub async fn roll(ctx: Context<'_>) -> Result<(), Error> {
    let rolled = rand::rng().random_range(1..=100);
    ctx.say(format!("🎲 <@{}> выбрасывает **{rolled}** (1–100)", ctx.author().id)).await?;
    Ok(())
}

/// Шуточное предсказание
#[poise::command(slash_command)]
pub async fn predskazanie(ctx: Context<'_>) -> Result<(), Error> {
    let mention = format!("<@{}>", ctx.author().id);
    let answer = predictions("day").choose(&mut rand::rng()).copied().unwrap_or("Сегодня без предсказаний.").replace("{name}", &mention);
    ctx.say(format!("🔮 Предсказание для {mention}\n> {answer}")).await?; // "> " — цитата
    Ok(())
}

/// Магический шар: задай вопрос — ответит да или нет
#[poise::command(slash_command)]
pub async fn shar(
    ctx: Context<'_>,
    #[description = "Вопрос, на который можно ответить «да» или «нет»"]
    #[min_length = 1]
    #[max_length = 300]
    question: String,
) -> Result<(), Error> {
    let answer = predictions("ball").choose(&mut rand::rng()).copied().unwrap_or("Спроси позже 😴");
    let text = format!("🎱 Магический шар для <@{}>\n❓ *{question}*\n> {answer}", ctx.author().id);
    // пинги из вопроса не срабатывают — упомянуть можно только самого спрашивающего
    let mentions = serenity::CreateAllowedMentions::new().users(vec![ctx.author().id]);
    ctx.send(poise::CreateReply::default().content(text).allowed_mentions(mentions)).await?;
    Ok(())
}

/// Настройки шуточной игры из settings.ini, раздел [dick_game].
pub struct GrowthSettings {
    pub min: i64,      // самое большое уменьшение за раз (см)
    pub max: i64,      // самое большое увеличение за раз (см)
    pub cooldown: i64, // как часто можно играть (в секундах)
}

impl GrowthSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        let (a, b) = (settings.int("dick_game", "growth_min", -10), settings.int("dick_game", "growth_max", 10));
        Self { min: a.min(b), max: a.max(b), cooldown: settings.int("dick_game", "growth_cooldown_hours", 24) * 3600 }
    }
}

/// Что написать после попытки: (новый размер, текст без упоминания игрока в начале).
pub fn growth_result(size: i64, change: i64) -> (i64, String) {
    let new_size = (size + change).max(0); // меньше нуля не бывает
    let text = if change > 0 {
        format!("твой член вырос на **{change} см**! Теперь он **{new_size} см**.")
    } else if change < 0 && new_size < size {
        format!("твой член уменьшился на **{} см**… Теперь он **{new_size} см**.", size - new_size)
    } else if change < 0 {
        // хотел уменьшиться, но уже 0
        format!("твой член должен был уменьшиться на {} см, но меньше уже некуда. Сейчас **0 см**.", -change)
    } else {
        format!("твой член без изменений: всё те же **{new_size} см**.")
    };
    (new_size, text)
}

/// Изменить размер (раз в сутки, случайно в плюс или в минус)
#[poise::command(slash_command, guild_only)]
pub async fn dick_growth(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild, user) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get());
    let growth = &data.growth;
    let (size, last_growth) = data.db.dick(guild, user)?;
    let (now, cooldown) = (db::now(), growth.cooldown as f64);
    if now - last_growth < cooldown {
        return whisper(ctx, format!(
            "⏳ Расти можно раз в {}. Следующая попытка через **{}**.\nСейчас у тебя **{size} см**.",
            hours_text(growth.cooldown), time_left_text(cooldown - (now - last_growth)))).await;
    }
    let change = rand::rng().random_range(growth.min..=growth.max);
    let (new_size, text) = growth_result(size, change);
    data.db.set_dick(guild, user, &display_name(ctx).await, new_size, now)?;
    let icon = if change > 0 { "📈" } else if change < 0 { "📉" } else { "😐" };
    ctx.say(format!(
        "{icon} <@{user}>, {text}\nМесто в рейтинге: **#{}**. Следующая попытка — через {}.",
        data.db.dick_position(guild, new_size)?, hours_text(growth.cooldown))).await?;
    Ok(())
}

/// Посмотреть размер
#[poise::command(slash_command, guild_only)]
pub async fn dick_size(
    ctx: Context<'_>,
    #[description = "Чей размер посмотреть (если не выбрать — твой)"] member: Option<serenity::User>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let own = member.as_ref().is_none_or(|member| member.id == ctx.author().id);
    let user = member.map_or(ctx.author().id, |member| member.id);
    let (size, last_growth) = data.db.dick(guild, user.get())?;
    let mut text = format!("📏 Размер у <@{user}>: **{size} см** · место в рейтинге **#{}**", data.db.dick_position(guild, size)?);
    if own {
        // про время до следующей попытки пишем только самому игроку
        let passed = db::now() - last_growth;
        if passed >= data.growth.cooldown as f64 {
            text += "\n✅ Можно расти: /dick_growth";
        } else {
            text += &format!("\n⏳ Следующая попытка через {}", time_left_text(data.growth.cooldown as f64 - passed));
        }
    }
    ctx.say(text).await?;
    Ok(())
}

/// Таблица рейтинга (топ-10)
#[poise::command(slash_command, guild_only)]
pub async fn dick_leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let top = data.db.top_dick(guild.get())?;
    if top.is_empty() {
        ctx.say("Пока никто не играл — начни с /dick_growth!").await?;
        return Ok(());
    }
    ctx.defer().await?;
    let biggest = top.iter().map(|(_, _, size)| *size).max().unwrap_or(1).max(1);
    let mut rows = Vec::new();
    for (user, name, size) in top {
        rows.push(Row {
            name,
            picture: Picture::Avatar(avatar_by_id(ctx.serenity_context(), data, user).await),
            progress: size as f64 / biggest as f64,
            pill: format!("{size} см"),
            hint: "размер".to_string(),
        });
    }
    let server = guild.name(ctx).unwrap_or_default();
    send_png(ctx, render_table(data, "РЕЙТИНГ ЧЛЕНОВ", server, "", rows, "dick_bg").await, "dick_leaderboard.png").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dice_texts() {
        assert_eq!(dice_text(&[4]), "# ⚃\nВыпало **4**");
        assert_eq!(dice_text(&[3, 5]), "# ⚂ ⚄\n3 + 5 = **8**");
        assert_eq!(dice_text(&[6, 6]), "# ⚅ ⚅\n6 + 6 = **12**\n🔥 Дубль!");
        assert_eq!(dice_text(&[1, 1, 2]), "# ⚀ ⚀ ⚁\n1 + 1 + 2 = **4**");
    }

    #[test]
    fn prediction_lists() {
        let (day, ball) = (predictions("day"), predictions("ball"));
        assert_eq!((day.len(), ball.len()), (36, 11)); // столько же, сколько в bot.py
        assert!(day.iter().any(|line| line.contains("{name}")));
        assert_eq!(ball[0], "Бесспорно ✅");
        assert!(predictions("нет такого").is_empty());
    }

    #[test]
    fn growth_texts() {
        assert_eq!(growth_result(10, 5), (15, "твой член вырос на **5 см**! Теперь он **15 см**.".to_string()));
        assert_eq!(growth_result(10, -4), (6, "твой член уменьшился на **4 см**… Теперь он **6 см**.".to_string()));
        assert_eq!(growth_result(3, -8).0, 0);
        assert!(growth_result(3, -8).1.starts_with("твой член уменьшился на **3 см**"));
        assert!(growth_result(0, -8).1.contains("меньше уже некуда"));
        assert!(growth_result(7, 0).1.contains("без изменений"));
        let growth = GrowthSettings::load(&crate::config::Settings::default());
        assert_eq!((growth.min, growth.max, growth.cooldown), (-10, 10, 86400));
    }
}
