//! Утренние и вечерние сообщения (/daily_setup, /daily_off, /daily_test): пожелание и интересный факт.
//! Текст пишет ИИ, а без него — запасной факт. Раз в минуту бот проверяет, не пора ли отправить.

use crate::admin::{bot_permissions, report_error};
use crate::db::DailySettings;
use crate::reply::whisper;
use crate::{ai, Context, Data, Error, Shared};
use poise::serenity_prelude as serenity;
use rand::seq::IndexedRandom;

const WINDOW_MINUTES: i64 = 30; // если бот был выключен в нужную минуту, сообщение ещё отправится в течение 30 минут

/// О чём рассказывать факты (тема выбирается случайно).
const TOPICS: [&str; 8] = ["математика", "физика", "инженерия", "электроника и радиотехника", "астрономия",
                           "информатика и программирование", "химия", "история изобретений"];

/// Запасные факты — если ИИ не настроен или недоступен (те же, что в bot.py).
const FALLBACK_FACTS: [&str; 14] = [
    "число π иррационально: его десятичная запись бесконечна и никогда не повторяется. Это доказал Иоганн Ламберт в XVIII веке.",
    "свет от Солнца летит до Земли около 8 минут 20 секунд — мы всегда видим Солнце немного «в прошлом».",
    "сумма углов треугольника, нарисованного на сфере, всегда больше 180°.",
    "первый транзистор создали в 1947 году в Bell Labs Джон Бардин, Уолтер Браттейн и Уильям Шокли.",
    "запись 0,999… с бесконечными девятками — это ровно 1, а не «чуть меньше единицы».",
    "звук в стали распространяется более чем в 15 раз быстрее, чем в воздухе.",
    "Эйлер в 1736 году решил задачу о семи мостах Кёнигсберга — с неё началась теория графов.",
    "спутники GPS учитывают эффекты теории относительности: без поправок ошибка накапливалась бы примерно на 10 км в сутки.",
    "в группе всего из 23 человек вероятность того, что у двоих совпадут дни рождения, больше 50%.",
    "в оптоволокне свет идёт примерно на треть медленнее, чем в вакууме.",
    "канал молнии нагревается примерно до 30 000 °C — это в несколько раз горячее поверхности Солнца.",
    "простых чисел бесконечно много — Евклид доказал это больше 2000 лет назад.",
    "вода имеет наибольшую плотность при температуре около 4 °C, поэтому зимой у дна водоёмов вода теплее, а замерзают они сверху.",
    "Томас Хейлс в 1999 году доказал, что шестиугольные соты — способ разбить плоскость на равные ячейки с наименьшей общей длиной границ.",
];

/// Часовые пояса для /daily_setup (код пояса — как в bot.py, чтобы старые настройки из базы подходили).
#[derive(poise::ChoiceParameter, Clone, Copy)]
pub enum Zone {
    #[name = "Израиль"]
    Israel,
    #[name = "Москва"]
    Moscow,
    #[name = "Киев"]
    Kyiv,
    #[name = "Минск"]
    Minsk,
    #[name = "Казахстан (Алматы)"]
    Almaty,
    #[name = "Ташкент"]
    Tashkent,
    #[name = "Центральная Европа (Берлин)"]
    Berlin,
    #[name = "Лондон"]
    London,
    #[name = "Нью-Йорк"]
    NewYork,
}

impl Zone {
    /// Код пояса.
    fn code(self) -> &'static str {
        match self {
            Zone::Israel => "Asia/Jerusalem",
            Zone::Moscow => "Europe/Moscow",
            Zone::Kyiv => "Europe/Kyiv",
            Zone::Minsk => "Europe/Minsk",
            Zone::Almaty => "Asia/Almaty",
            Zone::Tashkent => "Asia/Tashkent",
            Zone::Berlin => "Europe/Berlin",
            Zone::London => "Europe/London",
            Zone::NewYork => "America/New_York",
        }
    }
}

#[derive(poise::ChoiceParameter, Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    #[name = "☀️ Утреннее"]
    Morning,
    #[name = "🌙 Вечернее"]
    Evening,
}

/// Текущее время (без пояса) в часовом поясе сервера. Неизвестный пояс — UTC.
pub fn local_now(zone: &str) -> chrono::NaiveDateTime {
    match zone.parse::<chrono_tz::Tz>() {
        Ok(zone) => chrono::Utc::now().with_timezone(&zone).naive_local(),
        Err(_) => chrono::Utc::now().naive_utc(),
    }
}

/// «8:00», «08.00» → «08:00». None — не время.
pub fn parse_time(text: &str) -> Option<String> {
    let (hours, minutes) = text.trim().split_once([':', '.'])?;
    if hours.is_empty() || hours.len() > 2 || minutes.len() != 2 || !(hours.chars().chain(minutes.chars())).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours <= 23 && minutes <= 59).then(|| format!("{hours:02}:{minutes:02}"))
}

/// Наступило ли уже время at («08:00») в момент now.
fn passed(now: chrono::NaiveDateTime, at: &str) -> bool {
    scheduled(now, at).is_some_and(|scheduled| now >= scheduled)
}

fn scheduled(now: chrono::NaiveDateTime, at: &str) -> Option<chrono::NaiveDateTime> {
    let (hours, minutes) = at.split_once(':')?;
    now.date().and_hms_opt(hours.parse().ok()?, minutes.parse().ok()?, 0)
}

/// Пора ли отправить: нужное время уже наступило, но прошло не больше WINDOW_MINUTES минут, и сегодня ещё не отправляли.
pub fn is_due(now: chrono::NaiveDateTime, at: &str, last: &str) -> bool {
    if last == now.date().to_string() {
        return false;
    }
    scheduled(now, at).is_some_and(|scheduled| (0..WINDOW_MINUTES * 60).contains(&(now - scheduled).num_seconds()))
}

/// Текст утреннего или вечернего сообщения. Пишет ИИ, а без него — запасной вариант.
async fn make_text(http: &serenity::Http, data: &Data, kind: Kind, guild: Option<serenity::GuildId>) -> String {
    let (topic, fact) = {
        let mut random = rand::rng();
        (*TOPICS.choose(&mut random).unwrap(), *FALLBACK_FACTS.choose(&mut random).unwrap())
    };
    let today = chrono::Local::now().date_naive();
    let task = match kind {
        Kind::Morning => format!(
            "Напиши утреннее сообщение для Discord-чата друзей. Пожелай всем доброго утра и удачи на сегодня, \
             затем расскажи один интересный факт из области «{topic}». Факт должен быть широко известным \
             и достоверным — без выдуманных чисел, дат и имён. Всего 3–6 предложений."),
        Kind::Evening => format!(
            "Напиши вечернее сообщение для Discord-чата друзей. Расскажи интересный «факт дня» из области «{topic}» \
             (если точно знаешь событие из этой области, связанное с датой {}, — можно о нём, \
             иначе просто интересный факт). Факт должен быть достоверным — без выдуманных чисел, дат и имён. \
             Потом пожелай всем удачного завтрашнего дня и спокойной ночи. Всего 3–6 предложений.", today.format("%d.%m")),
    };
    if data.ai.ready() {
        let system = format!("{}\nСегодня {}.", ai::system_prompt(data), today.format("%d.%m.%Y"));
        match ai::generate(data, &system, &[ai::Turn::user(task)]).await {
            Ok(answer) => {
                let text = ai::without_tags(&answer.text);
                if !text.is_empty() {
                    return text;
                }
            }
            Err(error) => report_error(http, data, guild, "ИИ (ежедневное сообщение)", &error.to_string()).await,
        }
    }
    match kind {
        Kind::Morning => format!("Доброе утро! Удачи всем сегодня — пусть всё получится 💪\n\n🧠 **Интересный факт:** {fact}"),
        Kind::Evening => format!("🧠 **Факт дня:** {fact}\n\nПусть завтрашний день будет удачным. Спокойной ночи! 🌙"),
    }
}

/// Отправляет утреннее или вечернее сообщение в канал. Возвращает true, если получилось.
async fn send(http: &serenity::Http, data: &Data, channel: serenity::ChannelId, kind: Kind, guild: Option<serenity::GuildId>) -> bool {
    let text: String = make_text(http, data, kind, guild).await.chars().take(4000).collect();
    let (title, color) = match kind {
        Kind::Morning => ("☀️ Доброе утро!", 0xF59E0B),
        Kind::Evening => ("🌙 Спокойной ночи!", 0x6366F1),
    };
    let embed = serenity::CreateEmbed::new().title(title).description(text).color(color);
    match channel.send_message(http, serenity::CreateMessage::new().embed(embed)).await {
        Ok(_) => true,
        Err(error) => {
            report_error(http, data, guild, "Отправка ежедневного сообщения", &error.to_string()).await;
            false
        }
    }
}

/// Раз в минуту проверяет, не пора ли отправить сообщения.
pub fn start_loop(http: std::sync::Arc<serenity::Http>, data: Shared) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            timer.tick().await;
            let rows = match data.db.daily_settings() {
                Ok(rows) => rows,
                Err(error) => {
                    println!("Ежедневные сообщения: {error}");
                    continue;
                }
            };
            for row in rows {
                let now = local_now(&row.timezone); // текущее время в часовом поясе сервера
                for (kind, at, last) in [(Kind::Morning, &row.morning, &row.last_morning), (Kind::Evening, &row.evening, &row.last_evening)] {
                    if !is_due(now, at, last) {
                        continue;
                    }
                    // Сначала отмечаем, что отправили, — чтобы не отправить дважды, если ИИ будет долго думать
                    if let Err(error) = data.db.mark_daily_sent(row.guild, kind == Kind::Morning, &now.date().to_string()) {
                        println!("Ежедневные сообщения: {error}");
                        continue;
                    }
                    let guild = Some(serenity::GuildId::new(row.guild.max(1)));
                    send(&http, &data, serenity::ChannelId::new(row.channel.max(1)), kind, guild).await;
                }
            }
        }
    });
}

/// Включить утренние и вечерние сообщения
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn daily_setup(
    ctx: Context<'_>,
    #[description = "В какой канал писать"]
    #[channel_types("Text")]
    channel: serenity::GuildChannel,
    #[description = "Часовой пояс сервера"] timezone: Zone,
    #[description = "Время утреннего сообщения, например 08:00"] morning: Option<String>,
    #[description = "Время вечернего сообщения, например 22:00"] evening: Option<String>,
) -> Result<(), Error> {
    let mut times = Vec::new();
    for value in [morning.unwrap_or_else(|| "08:00".into()), evening.unwrap_or_else(|| "22:00".into())] {
        match parse_time(&value) {
            Some(time) => times.push(time),
            None => return whisper(ctx, format!("Не понял время «{value}». Пиши так: 08:00")).await,
        }
    }
    let allowed = bot_permissions(ctx, &channel).is_some_and(|p| p.send_messages() && p.embed_links());
    if !allowed {
        return whisper(ctx, format!("Мне нельзя писать в <#{}>: нужны права «Отправлять сообщения» и «Встраивать ссылки».", channel.id)).await;
    }
    // Если сегодня это время уже прошло — первое сообщение будет завтра (отмечаем как будто уже отправили)
    let code = timezone.code();
    let now = local_now(code);
    let today = now.date().to_string();
    let mark = |at: &str| if passed(now, at) { today.clone() } else { String::new() };
    let settings = DailySettings {
        guild: ctx.guild_id().unwrap().get(),
        channel: channel.id.get(),
        timezone: code.to_string(),
        last_morning: mark(&times[0]),
        last_evening: mark(&times[1]),
        morning: times[0].clone(),
        evening: times[1].clone(),
    };
    ctx.data().db.set_daily_settings(&settings)?;
    ctx.say(format!(
        "✅ Готово! Буду писать в <#{}>: утром в **{}** и вечером в **{}** ({}).\nПроверить прямо сейчас: /daily_test",
        channel.id, settings.morning, settings.evening, poise::ChoiceParameter::name(&timezone))).await?;
    Ok(())
}

/// Выключить утренние и вечерние сообщения
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn daily_off(ctx: Context<'_>) -> Result<(), Error> {
    if ctx.data().db.delete_daily_settings(ctx.guild_id().unwrap().get())? {
        ctx.say("🔕 Утренние и вечерние сообщения выключены.").await?;
        Ok(())
    } else {
        whisper(ctx, "Они и так выключены.").await
    }
}

/// Прислать утреннее или вечернее сообщение прямо сейчас
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn daily_test(ctx: Context<'_>, #[description = "Какое сообщение прислать"] kind: Kind) -> Result<(), Error> {
    let data = ctx.data();
    let guild = ctx.guild_id().unwrap();
    let configured = data.db.daily_settings()?.into_iter().find(|row| row.guild == guild.get()).map(|row| row.channel);
    let channel = configured.map_or(ctx.channel_id(), serenity::ChannelId::new); // не настроено — пишем в текущий канал
    ctx.defer_ephemeral().await?; // ИИ пишет текст несколько секунд
    let text = if send(ctx.http(), data, channel, kind, Some(guild)).await {
        "✅ Отправила."
    } else {
        "Не получилось отправить — подробности в канале логов."
    };
    whisper(ctx, text).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(hours: u32, minutes: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 12).unwrap().and_hms_opt(hours, minutes, 0).unwrap()
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("8:00").as_deref(), Some("08:00"));
        assert_eq!(parse_time(" 22.30 ").as_deref(), Some("22:30"));
        assert_eq!([parse_time("24:00"), parse_time("8:60"), parse_time("8"), parse_time("8:0"), parse_time("ab:cd"), parse_time("123:00")],
                   [None, None, None, None, None, None]);
        assert!(passed(at(8, 0), "08:00") && !passed(at(7, 59), "08:00"));
    }

    #[test]
    fn schedule() {
        assert!(is_due(at(8, 0), "08:00", ""));
        assert!(is_due(at(8, 29), "08:00", "2026-11-11")); // бот был выключен — ещё успеваем
        assert!(!is_due(at(8, 30), "08:00", "")); // окно прошло
        assert!(!is_due(at(7, 59), "08:00", ""));
        assert!(!is_due(at(8, 5), "08:00", "2026-11-12")); // сегодня уже отправили
        assert!(!is_due(at(8, 5), "чушь", ""));
    }

    #[test]
    fn zones_known() {
        for zone in [Zone::Israel, Zone::Moscow, Zone::Kyiv, Zone::Minsk, Zone::Almaty, Zone::Tashkent, Zone::Berlin, Zone::London, Zone::NewYork] {
            assert!(zone.code().parse::<chrono_tz::Tz>().is_ok(), "{}", zone.code());
        }
    }
}
