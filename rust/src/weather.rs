//! Погода: /weather и /weather_cities. Сервис Open-Meteo — бесплатный, ключ не нужен.

use crate::admin::report_error;
use crate::reply::whisper;
use crate::{Context, Data, Error};
use chrono::{Datelike, NaiveDate};
use poise::serenity_prelude as serenity;
use serde_json::Value;

/// Город: (название для показа, широта, долгота).
pub type City = (String, f64, f64);

const MAX_CITIES: usize = 8; // больше городов в одном сообщении будет неудобно читать
const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

/// Города по умолчанию: показываются, пока на сервере не добавили свои через /weather_cities.
fn default_cities() -> Vec<City> {
    vec![("🇷🇺 Москва".to_string(), 55.7558, 37.6173), ("🇮🇱 Израиль, Тель-Авив".to_string(), 32.0853, 34.7818)]
}

/// Код погоды (международный стандарт WMO) → (описание, значок).
pub fn weather_code(code: i64) -> (&'static str, &'static str) {
    match code {
        0 => ("ясно", "☀️"),
        1 => ("преимущественно ясно", "🌤️"),
        2 => ("переменная облачность", "⛅"),
        3 => ("пасмурно", "☁️"),
        45 => ("туман", "🌫️"),
        48 => ("туман с изморозью", "🌫️"),
        51 => ("слабая морось", "🌦️"),
        53 => ("морось", "🌦️"),
        55 => ("сильная морось", "🌧️"),
        56 => ("ледяная морось", "🌧️"),
        57 => ("сильная ледяная морось", "🌧️"),
        61 => ("небольшой дождь", "🌦️"),
        63 => ("дождь", "🌧️"),
        65 => ("сильный дождь", "🌧️"),
        66 => ("ледяной дождь", "🌧️"),
        67 => ("сильный ледяной дождь", "🌧️"),
        71 => ("небольшой снег", "🌨️"),
        73 => ("снег", "🌨️"),
        75 => ("сильный снег", "❄️"),
        77 => ("снежная крупа", "🌨️"),
        80 => ("небольшой ливень", "🌦️"),
        81 => ("ливень", "🌧️"),
        82 => ("сильный ливень", "⛈️"),
        85 => ("снегопад", "🌨️"),
        86 => ("сильный снегопад", "❄️"),
        95 => ("гроза", "⛈️"),
        96 => ("гроза с градом", "⛈️"),
        99 => ("гроза с сильным градом", "⛈️"),
        _ => ("неизвестно", "🌡️"),
    }
}

/// Красиво пишет температуру: 12.4 → "+12°", -3.6 → "-4°", 0.2 → "0°".
pub fn temperature(value: f64) -> String {
    let value = value.round_ties_even() as i64;
    if value == 0 { "0°".to_string() } else { format!("{value:+}°") }
}

/// Превращает ответ Open-Meteo в несколько строк текста для Discord. None — ответ не того вида.
pub fn format_weather(data: &Value) -> Option<String> {
    let now = data.get("current")?;
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64);
    let (description, icon) = weather_code(now.get("weather_code")?.as_i64()?);
    let time = now.get("time")?.as_str()?;
    let local_time = time.get(time.len().saturating_sub(5)..)?; // из "2026-10-30T21:15" берём "21:15"
    let mut lines = vec![
        format!("{icon} **{}**, {description} · 🕐 {local_time}", temperature(number(now, "temperature_2m")?)),
        format!(
            "Ощущается как {} · ветер {} м/с · влажность {}%",
            temperature(number(now, "apparent_temperature")?),
            number(now, "wind_speed_10m")?.round_ties_even(),
            number(now, "relative_humidity_2m")?),
        String::new(), // пустая строка — отступ перед прогнозом
    ];
    let days = data.get("daily")?;
    let list = |key: &str| days.get(key).and_then(Value::as_array);
    let (codes, lows, highs, rains) =
        (list("weather_code")?, list("temperature_2m_min")?, list("temperature_2m_max")?, list("precipitation_probability_max"));
    for (index, day) in list("time")?.iter().enumerate() {
        let date: NaiveDate = day.as_str()?.parse().ok()?;
        let label = match index {
            0 => "Сегодня".to_string(),
            1 => "Завтра".to_string(),
            _ => format!("{} {}", WEEKDAYS[date.weekday().num_days_from_monday() as usize], date.format("%d.%m")),
        };
        let (_, day_icon) = weather_code(codes.get(index)?.as_i64().unwrap_or(-1));
        // вероятность осадков в % (её может не быть)
        let rain = rains.and_then(|rains| rains.get(index)).and_then(Value::as_f64).filter(|rain| *rain > 0.0);
        let rain_text = rain.map_or(String::new(), |rain| format!(" · 💧 {rain}%"));
        lines.push(format!(
            "{day_icon} {label}: {}…{}{rain_text}", temperature(lows.get(index)?.as_f64()?), temperature(highs.get(index)?.as_f64()?)));
    }
    Some(lines.join("\n"))
}

async fn get_json(data: &Data, url: &str, query: &[(&str, String)]) -> Result<Value, Error> {
    let text = data.web.get(url).query(query).timeout(std::time::Duration::from_secs(10)).send().await?.error_for_status()?.text().await?;
    Ok(serde_json::from_str(&text)?)
}

/// Название города из ответа поиска Open-Meteo: (название для показа, широта, долгота).
pub fn city_from_search(found: &Value) -> Option<City> {
    let place = found.get("results")?.as_array()?.first()?; // самый подходящий вариант
    let name = place.get("name")?.as_str()?;
    let title = match place.get("country").and_then(Value::as_str) {
        Some(country) => format!("📍 {name}, {country}"),
        None => format!("📍 {name}"),
    };
    Some((title, place.get("latitude")?.as_f64()?, place.get("longitude")?.as_f64()?))
}

/// Ищет город по названию. Ok(None) — не нашёлся.
async fn find_city(data: &Data, name: &str) -> Result<Option<City>, Error> {
    let query = [("name", name.to_string()), ("count", "1".into()), ("language", "ru".into()), ("format", "json".into())];
    Ok(city_from_search(&get_json(data, "https://geocoding-api.open-meteo.com/v1/search", &query).await?))
}

/// Погода сейчас и прогноз на 3 дня.
async fn fetch_weather(data: &Data, latitude: f64, longitude: f64) -> Result<Value, Error> {
    let query = [
        ("latitude", latitude.to_string()),
        ("longitude", longitude.to_string()),
        ("current", "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m".into()),
        ("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max".into()),
        ("wind_speed_unit", "ms".into()), // ветер в метрах в секунду
        ("timezone", "auto".into()),      // время и дни — по местному времени города
        ("forecast_days", "3".into()),
    ];
    get_json(data, "https://api.open-meteo.com/v1/forecast", &query).await
}

/// Города сервера; если своих ещё не добавляли — города по умолчанию.
fn server_cities(data: &Data, guild: u64) -> rusqlite::Result<Vec<City>> {
    let cities = data.db.weather_cities(guild)?;
    Ok(if cities.is_empty() { default_cities() } else { cities })
}

/// Погода сейчас и на 3 дня
#[poise::command(slash_command)]
pub async fn weather(
    ctx: Context<'_>,
    #[description = "Любой город (если не указать — города из /weather_cities)"] city: Option<String>,
) -> Result<(), Error> {
    let data = ctx.data();
    ctx.defer().await?; // запрос к сайту погоды может занять пару секунд
    let cities = match (&city, ctx.guild_id()) {
        (Some(city), guild) => {
            let found = match find_city(data, city).await {
                Ok(found) => found,
                Err(error) => {
                    report_error(ctx.http(), data, guild, "Поиск города /weather", &error.to_string()).await;
                    None
                }
            };
            let Some(found) = found else {
                ctx.say(format!("Не нашёл город «{city}» 😕 Попробуй написать по-другому.")).await?;
                return Ok(());
            };
            vec![found]
        }
        (None, Some(guild)) => server_cities(data, guild.get())?,
        (None, None) => default_cities(), // в личных сообщениях — города по умолчанию
    };
    let mut embed = serenity::CreateEmbed::new().title("Прогноз погоды").color(0x8B5CF6);
    for (name, latitude, longitude) in cities {
        let text = match fetch_weather(data, latitude, longitude).await.map(|answer| format_weather(&answer)) {
            Ok(Some(text)) => text,
            failed => {
                let details = failed.err().map_or("ответ не того вида".to_string(), |error| error.to_string());
                report_error(ctx.http(), data, ctx.guild_id(), &format!("Погода /weather ({name})"), &details).await;
                "Не получилось узнать погоду 😕".to_string()
            }
        };
        embed = embed.field(name, text, false); // каждый город с новой строки
    }
    ctx.send(poise::CreateReply::default().embed(embed.footer(serenity::CreateEmbedFooter::new("Данные: Open-Meteo")))).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, poise::ChoiceParameter)]
pub enum CityAction {
    #[name = "➕ Добавить город"]
    Add,
    #[name = "➖ Удалить город"]
    Remove,
    #[name = "📋 Показать список"]
    List,
}

/// Добавить, удалить или посмотреть города для /weather
#[poise::command(slash_command, guild_only)]
pub async fn weather_cities(
    ctx: Context<'_>,
    #[description = "Что сделать"] action: CityAction,
    #[description = "Название города (для добавления и удаления)"] city: Option<String>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let mut cities = server_cities(data, guild.get())?;
    if action == CityAction::List {
        let lines: Vec<String> = cities.iter().enumerate().map(|(index, (name, _, _))| format!("{}. {name}", index + 1)).collect();
        ctx.say(format!("🗺️ Города для /weather:\n{}", lines.join("\n"))).await?;
        return Ok(());
    }
    let Some(city) = city.filter(|city| !city.trim().is_empty()) else {
        return whisper(ctx, "Напиши название города в поле `city`.").await; // для добавления и удаления нужно название
    };
    if action == CityAction::Add {
        if cities.len() >= MAX_CITIES {
            return whisper(ctx, format!("Уже {MAX_CITIES} городов — сначала удали какой-нибудь.")).await;
        }
        ctx.defer().await?; // поиск города занимает пару секунд
        let found = match find_city(data, &city).await {
            Ok(found) => found,
            Err(error) => {
                report_error(ctx.http(), data, Some(guild), "Поиск города /weather_cities", &error.to_string()).await;
                None
            }
        };
        let Some(found) = found else {
            ctx.say(format!("Не нашёл город «{city}» 😕 Попробуй написать по-другому.")).await?;
            return Ok(());
        };
        if cities.iter().any(|(name, _, _)| *name == found.0) {
            ctx.say(format!("{} уже есть в списке.", found.0)).await?;
            return Ok(());
        }
        let title = found.0.clone();
        cities.push(found);
        data.db.save_weather_cities(guild.get(), &cities)?;
        ctx.say(format!("✅ Добавил {title}. Теперь в /weather городов: {}.", cities.len())).await?;
        return Ok(());
    }
    // Удаление: ищем город, в названии которого есть введённый текст (без учёта регистра)
    let Some(position) = cities.iter().position(|(name, _, _)| name.to_lowercase().contains(&city.to_lowercase())) else {
        return whisper(ctx, format!("Города «{city}» нет в списке. Посмотри: /weather_cities → Показать список.")).await;
    };
    let removed = cities.remove(position);
    data.db.save_weather_cities(guild.get(), &cities)?;
    let mut text = format!("🗑️ Удалил {}.", removed.0);
    if cities.is_empty() {
        text += " Список пуст, поэтому /weather снова покажет Москву и Тель-Авив.";
    }
    ctx.say(text).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperatures() {
        assert_eq!([temperature(12.4), temperature(-3.6), temperature(0.2), temperature(-0.4), temperature(25.5)],
                   ["+12°", "-4°", "0°", "0°", "+26°"]);
        assert_eq!(weather_code(0), ("ясно", "☀️"));
        assert_eq!(weather_code(95).0, "гроза");
        assert_eq!(weather_code(1234), ("неизвестно", "🌡️"));
    }

    #[test]
    fn formats_forecast() {
        let answer: Value = serde_json::from_str(r#"{
            "current": {"time": "2026-10-30T21:15", "temperature_2m": 12.4, "apparent_temperature": 10.6,
                        "relative_humidity_2m": 81, "weather_code": 3, "wind_speed_10m": 4.6},
            "daily": {"time": ["2026-10-30", "2026-10-31", "2026-11-01"], "weather_code": [3, 61, 0],
                      "temperature_2m_max": [13.2, 9.8, 4.5], "temperature_2m_min": [7.1, 3.4, -0.2],
                      "precipitation_probability_max": [0, 85, null]}
        }"#).unwrap();
        assert_eq!(format_weather(&answer).unwrap(),
                   "☁️ **+12°**, пасмурно · 🕐 21:15\nОщущается как +11° · ветер 5 м/с · влажность 81%\n\n\
                    ☁️ Сегодня: +7°…+13°\n🌦️ Завтра: +3°…+10° · 💧 85%\n☀️ Вс 01.11: 0°…+4°");
        assert_eq!(format_weather(&serde_json::json!({"error": true})), None);
    }

    #[test]
    fn city_search() {
        let found: Value = serde_json::from_str(
            r#"{"results": [{"name": "Берлин", "country": "Германия", "latitude": 52.52, "longitude": 13.41}]}"#).unwrap();
        assert_eq!(city_from_search(&found), Some(("📍 Берлин, Германия".to_string(), 52.52, 13.41)));
        let no_country: Value = serde_json::from_str(r#"{"results": [{"name": "X", "latitude": 1.0, "longitude": 2.0}]}"#).unwrap();
        assert_eq!(city_from_search(&no_country).unwrap().0, "📍 X");
        assert_eq!(city_from_search(&serde_json::json!({})), None);
    }

    /// Настоящий запрос к Open-Meteo (нужен интернет): cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn weather_live() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let data = Data::new(crate::db::Database::in_memory().unwrap(), &crate::config::Settings::default(), dir, false);
        let city = find_city(&data, "Берлин").await.unwrap().unwrap();
        assert!(city.0.contains("Берлин"), "{}", city.0);
        let text = format_weather(&fetch_weather(&data, city.1, city.2).await.unwrap()).unwrap();
        assert!(text.contains("Сегодня") && text.contains("Завтра"), "{text}");
    }
}
