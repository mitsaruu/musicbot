//! Ядро ИИ: запросы к Gemini, характер и правила, лимиты, память разговора, грубость.
//! Команды (/ask, /summary, /quiz…) — в ai_commands.rs.

use crate::admin::report_error;
use crate::moderation::Requester;
use crate::text::plural;
use crate::{db, Data};
use base64::Engine;
use poise::serenity_prelude as serenity;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

const MAX_TOKENS: i64 = 8192; // предел длины ответа; с запасом — «обдумывание» модели тоже входит в предел
pub const MAX_IMAGES: usize = 3; // сколько картинок из одного сообщения отправлять ИИ
pub const MAX_IMAGE_MB: u64 = 5; // картинки тяжелее не отправляем (весь запрос к Gemini — не больше 20 МБ)
const IMAGE_TYPES: [&str; 5] = ["image/png", "image/jpeg", "image/webp", "image/heic", "image/heif"];

/// Характер по умолчанию (если config/prompt.txt пуст) и технические правила — те же тексты, что в bot.py.
const DEFAULT_CHARACTER: &str = include_str!("ai_character.txt");
const TECH_RULES: &str = include_str!("ai_rules.txt");

/// Настройки из settings.ini, раздел [ai].
pub struct AiSettings {
    pub model: String,
    pub fallback_model: Option<String>, // запасная модель, если основная не отвечает
    pub thinking_level: Option<String>, // сколько ИИ «обдумывает» ответ: minimal, low, medium, high
    pub retries: i64,                   // сколько раз повторить запрос при ошибке «перегружен»
    pub cooldown: i64,                  // один человек спрашивает не чаще раза в столько секунд
    pub daily_limit: i64,               // и не больше стольких вопросов в день
    pub history_limit: i64,             // память разговора: сколько последних сообщений помнить
    pub google_daily_limit: i64,        // примерные лимиты бесплатного тарифа Google (для /ai_limits)
    pub google_minute_limit: i64,
    pub one_line: bool,                 // склеивать ответ в одну строку
}

impl AiSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        let off = |text: &str| ["none", "нет", "off"].contains(&text.to_lowercase().as_str());
        let model = settings.text("ai", "model", "gemini-3.5-flash-lite");
        let level = settings.text("ai", "thinking_level", "high").to_lowercase();
        let fallback = settings.text("ai", "fallback_model", "gemini-3.8-flash");
        Self {
            thinking_level: if off(&level) {
                None
            } else if ["minimal", "low", "medium", "high"].contains(&level.as_str()) {
                Some(level)
            } else {
                println!("settings.ini: thinking_level = {level} — такого уровня нет, беру high");
                Some("high".to_string())
            },
            fallback_model: (!off(&fallback) && fallback != model).then_some(fallback),
            model,
            retries: settings.int("ai", "retries", 2).max(0),
            cooldown: settings.int("ai", "cooldown_seconds", 5),
            daily_limit: settings.int("ai", "daily_limit_per_user", 50),
            history_limit: settings.int("ai", "history_limit", 20),
            google_daily_limit: settings.int("ai", "google_daily_limit", 500),
            google_minute_limit: settings.int("ai", "google_minute_limit", 15),
            one_line: !["off", "нет", "no", "0"].contains(&settings.text("ai", "one_line_answers", "on").to_lowercase().as_str()),
        }
    }
}

/// Настройки тайм-аутов за грубость из settings.ini, раздел [moderation].
pub struct RudeSettings {
    pub timeouts: Vec<i64>,      // минуты: за 2-ю грубость подряд, 3-ю, 4-ю…
    pub strike_reset: i64,       // через сколько минут без грубостей счётчик обнуляется
    pub announce: bool,          // писать ли в чат, что человек получил тайм-аут
}

/// «1, 5, 10, 30, 60» → [1, 5, 10, 30, 60] (минуты). off / пусто → [] (тайм-аутов нет).
pub fn parse_timeouts(text: &str) -> Vec<i64> {
    if ["off", "none", "нет", "0", ""].contains(&text.trim().to_lowercase().as_str()) {
        return Vec::new();
    }
    let minutes: Option<Vec<i64>> = text.split(',').filter(|part| !part.trim().is_empty()).map(|part| part.trim().parse().ok()).collect();
    match minutes {
        Some(minutes) => minutes.into_iter().map(|m| m.clamp(1, 28 * 24 * 60)).collect(), // Discord: от 1 минуты до 28 дней
        None => {
            println!("settings.ini: timeouts = {text} — нужны числа через запятую, беру 1, 5, 10, 30, 60");
            vec![1, 5, 10, 30, 60]
        }
    }
}

impl RudeSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        Self {
            timeouts: parse_timeouts(&settings.text("moderation", "timeouts", "1, 5, 10, 30, 60")),
            strike_reset: settings.int("moderation", "strike_reset_minutes", 60),
            announce: !["off", "нет", "no", "0"].contains(&settings.text("moderation", "announce_timeouts", "on").to_lowercase().as_str()),
        }
    }
}

/// Всё, что ИИ хранит между запросами.
pub struct AiState {
    pub settings: AiSettings,
    pub rude: RudeSettings,
    pub key: Option<String>, // ключ Gemini; None — ИИ не настроен
    /// Сколько раз кто спрашивал: участник → (дата, вопросов за эту дату, время последнего вопроса). После перезапуска — с нуля.
    usage: Mutex<HashMap<u64, (String, i64, f64)>>,
    recent_requests: Mutex<Vec<f64>>, // время запросов к Google за последнюю минуту
    last_quota_error: Mutex<f64>,     // когда Google последний раз отказал из-за лимита (0 — ни разу)
}

impl AiState {
    pub fn new(settings: &crate::config::Settings, key: Option<String>) -> Self {
        Self {
            settings: AiSettings::load(settings),
            rude: RudeSettings::load(settings),
            key: key.filter(|key| !key.is_empty()),
            usage: Default::default(),
            recent_requests: Default::default(),
            last_quota_error: Default::default(),
        }
    }

    pub fn ready(&self) -> bool {
        self.key.is_some()
    }

    /// Личный счётчик человека на сегодня: (сколько вопросов задал, время последнего).
    pub fn usage_today(&self, user: u64) -> (i64, f64) {
        match locked(&self.usage).get(&user) {
            Some((day, count, last)) if *day == local_day() => (*count, *last),
            Some((_, _, last)) => (0, *last), // вопросы были вчера — сегодня счётчик с нуля
            None => (0, 0.0),
        }
    }

    /// Проверяет лимиты. Возвращает текст отказа или None, если спрашивать можно (и засчитывает вопрос).
    pub fn limit_check(&self, user: u64) -> Option<String> {
        let (count, last) = self.usage_today(user);
        let now = db::now();
        if now - last < self.settings.cooldown as f64 {
            return Some(format!("Не так быстро 🙂 Подожди {} сек.", (self.settings.cooldown as f64 - (now - last)) as i64 + 1));
        }
        if count >= self.settings.daily_limit {
            return Some(format!("На сегодня лимит вопросов ({}) закончился. Завтра продолжим!", self.settings.daily_limit));
        }
        locked(&self.usage).insert(user, (local_day(), count + 1, now));
        None
    }

    /// Сколько запросов к Google бот отправил за последние 60 секунд.
    pub fn requests_last_minute(&self) -> usize {
        let minute_ago = db::now() - 60.0;
        let mut recent = locked(&self.recent_requests);
        recent.retain(|moment| *moment > minute_ago);
        recent.len()
    }

    pub fn last_quota_error(&self) -> f64 {
        *locked(&self.last_quota_error)
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Сегодняшний день по времени компьютера, на котором запущен бот.
fn local_day() -> String {
    chrono::Local::now().date_naive().to_string()
}

/// Сегодняшний день по тихоокеанскому времени — по нему Google считает дневной лимит.
pub fn google_day() -> String {
    chrono::Utc::now().with_timezone(&chrono_tz::America::Los_Angeles).date_naive().to_string()
}

/// Когда обнулится дневной лимит Google (ближайшая полночь по тихоокеанскому времени), в секундах.
pub fn google_reset_time() -> i64 {
    use chrono::TimeZone;
    let zone = chrono_tz::America::Los_Angeles;
    let tomorrow = chrono::Utc::now().with_timezone(&zone).date_naive() + chrono::Duration::days(1);
    zone.from_local_datetime(&tomorrow.and_hms_opt(0, 0, 0).unwrap()).earliest().map_or(0, |time| time.timestamp())
}

/// Что пошло не так при запросе к ИИ.
#[derive(Debug, Clone, PartialEq)]
pub enum AiError {
    Quota,         // Google: лимит запросов исчерпан (429)
    Overloaded,    // Google перегружен или временная ошибка сервера (5xx)
    Other(String), // всё остальное (например, неверный ключ)
}

impl std::fmt::Display for AiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::Quota => write!(formatter, "лимит запросов к Gemini исчерпан (429)"),
            AiError::Overloaded => write!(formatter, "Gemini перегружен (5xx)"),
            AiError::Other(details) => write!(formatter, "{details}"),
        }
    }
}

impl std::error::Error for AiError {}

/// Понятный текст ошибки ИИ для чата.
pub fn error_text(error: &AiError) -> &'static str {
    match error {
        AiError::Quota => "😵 Лимит бесплатных запросов к ИИ на сейчас исчерпан. Попробуй позже (подробнее — /ai_limits).",
        AiError::Overloaded => "😵 ИИ у Google сейчас перегружен, попробуй через пару минут.",
        AiError::Other(_) => "😵 ИИ сейчас недоступен, попробуй чуть позже.",
    }
}

/// Одна реплика разговора для Gemini: роль ("user" или "model"), текст и картинки (байты, формат).
pub struct Turn {
    pub role: String,
    pub text: String,
    pub images: Vec<(Vec<u8>, String)>,
}

impl Turn {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), text: text.into(), images: Vec::new() }
    }
}

/// Ответ ИИ: текст и признак «упёрся в предел длины».
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub cut: bool,
}

/// Тело запроса к Gemini.
pub fn request_body(settings: &AiSettings, model: &str, system: &str, turns: &[Turn]) -> Value {
    let contents: Vec<Value> = turns
        .iter()
        .map(|turn| {
            let mut parts = vec![json!({ "text": turn.text })];
            for (bytes, mime) in &turn.images {
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                parts.push(json!({ "inlineData": { "mimeType": mime, "data": data } }));
            }
            json!({ "role": turn.role, "parts": parts })
        })
        .collect();
    let mut config = json!({ "maxOutputTokens": MAX_TOKENS });
    // «minimal» есть только у Flash-Lite — для других моделей берём ближайший уровень
    let level = settings.thinking_level.as_deref().map(|level| if level == "minimal" && !model.contains("lite") { "low" } else { level });
    if let Some(level) = level {
        config["thinkingConfig"] = json!({ "thinkingLevel": level });
    }
    json!({ "systemInstruction": { "parts": [{ "text": system }] }, "contents": contents, "generationConfig": config })
}

/// Достаёт текст из ответа Gemini (без «мыслей» модели).
pub fn parse_answer(response: &Value) -> Answer {
    let candidate = response.pointer("/candidates/0");
    let text = candidate
        .and_then(|candidate| candidate.pointer("/content/parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default();
    let cut = candidate.and_then(|candidate| candidate.get("finishReason")).and_then(Value::as_str) == Some("MAX_TOKENS");
    Answer { text: text.trim().to_string(), cut }
}

async fn call_gemini(data: &Data, model: &str, system: &str, turns: &[Turn]) -> Result<Answer, AiError> {
    let key = data.ai.key.as_deref().ok_or_else(|| AiError::Other("нет ключа GEMINI_API_KEY".into()))?;
    let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let body = request_body(&data.ai.settings, model, system, turns).to_string();
    let response = data
        .web
        .post(url)
        .header("x-goog-api-key", key)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|error| AiError::Other(error.to_string()))?;
    let status = response.status();
    let text = response.text().await.map_err(|error| AiError::Other(error.to_string()))?;
    if status.as_u16() == 429 || text.contains("RESOURCE_EXHAUSTED") && !status.is_success() {
        return Err(AiError::Quota);
    }
    if status.is_server_error() {
        return Err(AiError::Overloaded);
    }
    if !status.is_success() {
        let short: String = text.chars().take(500).collect();
        return Err(AiError::Other(format!("Gemini ответил {status}: {short}")));
    }
    let json: Value = serde_json::from_str(&text).map_err(|error| AiError::Other(error.to_string()))?;
    Ok(parse_answer(&json))
}

/// Запрос к Gemini с повторами и запасной моделью.
/// 1) Основная модель: если Google ответил «перегружен» — ждём и пробуем ещё (до retries раз).
/// 2) Не помогло (или кончился лимит основной модели) — тот же вопрос запасной модели.
pub async fn generate(data: &Data, system: &str, turns: &[Turn]) -> Result<Answer, AiError> {
    let settings = &data.ai.settings;
    let mut last_error = AiError::Other("нет моделей".into());
    for model in std::iter::once(&settings.model).chain(settings.fallback_model.iter()) {
        for attempt in 0..=settings.retries {
            match call_gemini(data, model, system, turns).await {
                Ok(answer) => {
                    // Google ответил — засчитываем запрос (для /ai_limits)
                    let _ = data.db.count_ai_request(&google_day());
                    locked(&data.ai.recent_requests).push(db::now());
                    if *model != settings.model {
                        println!("ИИ: основная модель {} не ответила ({last_error}), ответила запасная {model}", settings.model);
                    }
                    return Ok(answer);
                }
                Err(AiError::Quota) => {
                    *locked(&data.ai.last_quota_error) = db::now(); // покажем в /ai_limits
                    last_error = AiError::Quota;
                    break; // лимит у этой модели — повторять бесполезно, сразу к запасной
                }
                Err(AiError::Overloaded) => {
                    last_error = AiError::Overloaded;
                    if attempt < settings.retries {
                        tokio::time::sleep(std::time::Duration::from_secs(2 * (attempt as u64 + 1))).await; // 2, потом 4 секунды
                    }
                }
                Err(other) => return Err(other), // другая ошибка — повтор не поможет
            }
        }
    }
    Err(last_error)
}

/// Убирает из prompt.txt строки-пояснения (начинаются с #).
pub fn parse_prompt(text: &str) -> String {
    text.lines().filter(|line| !line.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Полная инструкция для ИИ: характер из config/prompt.txt + технические правила.
pub fn system_prompt(data: &Data) -> String {
    let own = parse_prompt(&std::fs::read_to_string(data.dir.join("config").join("prompt.txt")).unwrap_or_default());
    let character = if own.is_empty() { DEFAULT_CHARACTER.trim().to_string() } else { own };
    format!("{character}\n\n{}", TECH_RULES.trim())
}

/// Убирает из ответа метки вида [ИМЯ: …] или [ИМЯ] (без учёта регистра). names — какие метки убрать.
pub fn strip_tags(text: &str, names: &[&str]) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(length) = rest[start..].find(']') else { break };
        let inside = rest[start + 1..start + length].trim().to_uppercase().replace('-', "").replace(' ', "_");
        let name = inside.split(':').next().unwrap_or("").trim_end_matches('_');
        result.push_str(&rest[..start]);
        if !names.iter().any(|known| *known == name) {
            result.push_str(&rest[start..=start + length]);
        }
        rest = &rest[start + length + 1..];
    }
    result.push_str(rest);
    result.trim().to_string()
}

/// Есть ли в ответе метка [ИГНОР] — ИИ решил не отвечать на грубость.
pub fn has_ignore_tag(text: &str) -> bool {
    text.to_uppercase().contains("[ИГНОР]")
}

/// Метки модерации, которые ИИ ставит по просьбе модератора (выполняет их moderation.rs).
pub const MOD_TAGS: [&str; 3] = ["ВАРН", "ТАЙМАУТ", "СНЯТЬ_ТАЙМАУТ"];

/// Текст, который ИИ пишет сам (приветствие, утреннее сообщение, реплика): без меток памяти, наказаний и [ИГНОР].
pub fn without_tags(text: &str) -> String {
    strip_tags(&strip_tags(text, &["ЗАПОМНИТЬ", "ИГНОР"]), &MOD_TAGS)
}

/// Склеивает ответ в одну строку: абзацы и переносы → пробелы, значки списков убираются.
/// Ответы с кодом (```) не трогаем — код в одну строку сломается.
pub fn to_one_line(text: &str) -> String {
    if text.contains("```") {
        return text.to_string();
    }
    let (mut result, mut previous_bullet) = (String::new(), false);
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let item = ["- ", "* ", "• "].iter().find_map(|mark| line.strip_prefix(mark)).map(str::trim_start);
        let (bullet, line) = (item.is_some(), item.unwrap_or(line));
        if !result.is_empty() {
            // пункты списка подряд разделяем запятой («молоко, хлеб, сыр»), всё остальное — пробелом
            result.push_str(if bullet && previous_bullet { ", " } else { " " });
        }
        result.push_str(line);
        previous_bullet = bullet;
    }
    result
}

/// Режет длинный текст на части (в одном сообщении Discord не больше 2000 символов).
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > limit {
        // стараемся резать по переносу строки; переносов нет — режем ровно по лимиту
        let cut = rest[..limit].iter().rposition(|c| *c == '\n').filter(|cut| *cut > 0).unwrap_or(limit);
        let mut part: String = rest[..cut].iter().collect();
        let mut tail: String = rest[cut..].iter().collect::<String>().trim_start_matches('\n').to_string();
        if part.matches("```").count() % 2 == 1 {
            part.push_str("\n```"); // разрезали внутри блока кода: закрываем его здесь…
            tail = format!("```\n{tail}"); // …и открываем заново в следующем сообщении
        }
        parts.push(part);
        rest = tail.chars().collect();
    }
    if !rest.is_empty() {
        parts.push(rest.into_iter().collect());
    }
    parts
}

/// Формат вложения, например "image/png" (без лишнего вроде "; charset=…").
pub fn image_type(content_type: Option<&str>) -> String {
    content_type.unwrap_or("").split(';').next().unwrap_or("").trim().to_string()
}

/// Можно ли отправить это вложение в ИИ: картинка нужного формата и не слишком тяжёлая.
pub fn is_supported_image(content_type: Option<&str>, size: u64) -> bool {
    IMAGE_TYPES.contains(&image_type(content_type).as_str()) && size <= MAX_IMAGE_MB * 1024 * 1024
}

/// Что ещё передать ИИ вместе с вопросом.
#[derive(Default)]
pub struct Extras<'a> {
    /// Последние сообщения чата (для контекста), пусто — без них
    pub chat: String,
    /// Кто спрашивает на сервере и подсказка о его правах — чтобы выполнять варны и тайм-ауты по просьбе модератора
    pub moderation: Option<(Requester<'a>, String)>,
}

/// Отправляет вопрос в Gemini вместе с памятью разговора. Ok(None) — ИИ решил, что к нему обратились грубо.
pub async fn ask(data: &Data, question: &str, name: &str, channel: u64, images: Vec<(Vec<u8>, String)>, extras: Extras<'_>) -> Result<Option<String>, AiError> {
    let system = format!("{}\nСегодня {}.", system_prompt(data), chrono::Local::now().format("%d.%m.%Y"));
    let current = format!("{name}: {question}");
    let image_count = images.len();
    let mut prompt = current.clone();
    if !extras.chat.is_empty() {
        prompt = format!("Последние сообщения в чате (для контекста):\n{}\n\nВопрос — {current}", extras.chat);
    }
    if let Some((_, note)) = &extras.moderation {
        if !note.is_empty() {
            prompt = format!("{note}\n\n{prompt}"); // права собеседника и id упомянутых — для варнов
        }
    }
    // Разговор: сначала прошлые сообщения из памяти, потом новый вопрос
    let history = data.db.ai_history(channel, data.ai.settings.history_limit).map_err(|error| AiError::Other(error.to_string()))?;
    let mut turns: Vec<Turn> = history.into_iter().map(|(role, text)| Turn { role, text, images: Vec::new() }).collect();
    turns.push(Turn { role: "user".into(), text: prompt, images });
    let answer = generate(data, &system, &turns).await?;
    if answer.text.is_empty() {
        return Ok(Some("🤔 ИИ не дал ответа. Попробуй переформулировать вопрос.".to_string()));
    }
    if has_ignore_tag(&answer.text) {
        return Ok(None); // не отвечаем и не запоминаем
    }
    let text = strip_tags(&answer.text, &["ЗАПОМНИТЬ"]);
    // Варны и тайм-ауты по просьбе модератора: метки выполняем, а неполные (без id) просто вырезаем
    let (text, actions) = crate::moderation::take_actions(&text);
    let mut text = strip_tags(&text, &MOD_TAGS);
    let punishments = match &extras.moderation {
        Some((requester, _)) => crate::moderation::run_actions(data, requester, actions).await,
        None => Vec::new(),
    };
    if data.ai.settings.one_line {
        text = to_one_line(&text); // в памяти тоже — чтобы ИИ не копировал абзацы
    }
    if !punishments.is_empty() {
        text = format!("{text}\n{}", punishments.join("\n")).trim().to_string();
    }
    if text.is_empty() {
        text = "🤔 ИИ не дал ответа. Попробуй переформулировать вопрос.".to_string();
    }
    // Запоминаем этот обмен репликами. Сами картинки в память не сохраняются — только отметка о них.
    let remembered = if image_count > 0 { format!("{current} [прислал картинок: {image_count}]") } else { current };
    let _ = data.db.save_ai_history(channel, &remembered, &text, db::now(), data.ai.settings.history_limit);
    if answer.cut {
        text.push_str("\n\n✂️ *Ответ обрезан — слишком длинный. Попроси объяснить короче или по частям.*");
    }
    Ok(Some(text))
}

// ---------- Грубость: слова из config/bad_words.txt и тайм-ауты ----------

/// «Буква» для поиска слов: цифры, латиница и кириллица в нижнем регистре (ё заранее заменяется на е).
pub fn is_letter(c: char) -> bool {
    c.is_ascii_digit() || c.is_ascii_lowercase() || ('а'..='я').contains(&c)
}

/// Совпадает ли шаблон (буквы и * — «любые буквы») с текстом, начиная с позиции start. Возвращает конец совпадения.
fn match_at(pattern: &[char], text: &[char], start: usize) -> Option<usize> {
    match pattern.split_first() {
        None => Some(start),
        Some(('*', rest)) => {
            let mut end = start;
            while end < text.len() && is_letter(text[end]) {
                end += 1;
            }
            // пробуем «съесть» как можно больше букв, потом меньше
            (start..=end).rev().find_map(|position| match_at(rest, text, position))
        }
        Some((first, rest)) => (text.get(start) == Some(first)).then(|| match_at(rest, text, start + 1)).flatten(),
    }
}

/// Слова из bad_words.txt: по одному или несколько через запятую, строки с # — пояснения.
pub fn parse_bad_words(text: &str) -> Vec<Vec<char>> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(|line| line.split(','))
        .map(|word| word.trim().to_lowercase().replace('ё', "е"))
        .filter(|word| !word.trim_matches('*').is_empty())
        .map(|word| word.chars().collect())
        .collect()
}

/// Есть ли в тексте слово из списка. Края слова проверяются, чтобы «сука» не ловилась в «сукно».
pub fn contains_bad_word(words: &[Vec<char>], text: &str) -> bool {
    let text: Vec<char> = text.to_lowercase().replace('ё', "е").chars().collect();
    (0..text.len()).any(|start| {
        (start == 0 || !is_letter(text[start - 1]))
            && words.iter().any(|word| match_at(word, &text, start).is_some_and(|end| end > start && text.get(end).is_none_or(|next| !is_letter(*next))))
    })
}

/// Есть ли в тексте слова из config/bad_words.txt.
pub fn is_rude(data: &Data, text: &str) -> bool {
    let words = parse_bad_words(&std::fs::read_to_string(data.dir.join("config").join("bad_words.txt")).unwrap_or_default());
    contains_bad_word(&words, text)
}

/// Минуты → текст вроде «1 минуту», «5 минут», «2 часа».
pub fn minutes_text(minutes: i64) -> String {
    if minutes % (24 * 60) == 0 {
        let days = minutes / (24 * 60);
        format!("{days} {}", plural(days, "день", "дня", "дней"))
    } else if minutes % 60 == 0 {
        let hours = minutes / 60;
        format!("{hours} {}", plural(hours, "час", "часа", "часов"))
    } else {
        format!("{minutes} {}", plural(minutes, "минуту", "минуты", "минут"))
    }
}

/// Какой тайм-аут (в минутах) положен за count-ю грубость подряд. None — пока только молчим.
pub fn rude_timeout(steps: &[i64], count: i64) -> Option<i64> {
    if count < 2 || steps.is_empty() {
        return None; // первый раз — только молчим
    }
    Some(steps[((count - 2) as usize).min(steps.len() - 1)]) // 2-й раз — первая длительность и т.д.
}

/// Засчитывает грубость и, начиная со второй подряд, даёт тайм-аут.
pub async fn punish_rudeness(ctx: &serenity::Context, data: &Data, guild: Option<serenity::GuildId>, user: &serenity::User, channel: serenity::ChannelId) {
    let Some(guild) = guild else { return }; // в личных сообщениях тайм-аутов нет
    if user.bot {
        return; // другим ботам тайм-ауты не даём — им достаточно того, что бот промолчал
    }
    let Ok(count) = data.db.add_rude_strike(guild.get(), user.id.get(), db::now(), data.ai.rude.strike_reset as f64 * 60.0) else { return };
    println!("Грубость к боту: пользователь={} сервер={guild} подряд={count}", user.id);
    let Some(minutes) = rude_timeout(&data.ai.rude.timeouts, count) else { return };
    let until = serenity::Timestamp::from_unix_timestamp(db::now() as i64 + minutes * 60).unwrap_or_else(|_| serenity::Timestamp::now());
    let reason = format!("Грубость к боту ({count} раз подряд)");
    let edit = serenity::EditMember::new().disable_communication_until_datetime(until).audit_log_reason(&reason);
    if let Err(error) = guild.edit_member(ctx, user.id, edit).await {
        // Нет права Moderate Members, роль бота ниже или это администратор/владелец — пишем только в логи
        println!("Не получилось дать тайм-аут {}: {error}", user.id);
        let details = format!(
            "Хотела дать тайм-аут <@{}> за грубость ({count} раз подряд), но не получилось: {error}. Нужно право \
             «Отправлять участников подумать» (Moderate Members), и моя роль должна быть выше его роли.", user.id);
        report_error(&ctx.http, data, Some(guild), "Тайм-аут за грубость", &details).await;
        return;
    }
    let text = format!("🔇 <@{}> отдохнёт {}. Хватит грубить.", user.id, minutes_text(minutes));
    if data.ai.rude.announce {
        let message = serenity::CreateMessage::new().content(&text).allowed_mentions(serenity::CreateAllowedMentions::new());
        let _ = channel.send_message(ctx, message).await;
    }
    if data.db.log_channel(guild.get()).ok().flatten() != Some(channel.get()) {
        crate::moderation::send_log(ctx, data, guild, &format!("{text} ({count} раз подряд, в <#{channel}>)")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str) -> AiSettings {
        AiSettings::load(&crate::config::Settings::parse(text))
    }

    #[test]
    fn settings_like_python() {
        let ai = settings("");
        assert_eq!((ai.model.as_str(), ai.fallback_model.as_deref(), ai.thinking_level.as_deref()),
                   ("gemini-3.5-flash-lite", Some("gemini-3.8-flash"), Some("high")));
        assert_eq!((ai.retries, ai.cooldown, ai.daily_limit, ai.history_limit, ai.one_line), (2, 5, 50, 20, true));
        let ai = settings("[ai]\nthinking_level = чушь\nfallback_model = none\none_line_answers = off\n");
        assert_eq!((ai.thinking_level.as_deref(), ai.fallback_model, ai.one_line), (Some("high"), None, false));
        let ai = settings("[ai]\nthinking_level = off\nmodel = x\nfallback_model = x\n");
        assert_eq!((ai.thinking_level, ai.fallback_model), (None, None)); // запасная модель та же, что основная
        assert_eq!(parse_timeouts("1, 5, 10"), [1, 5, 10]);
        assert!(parse_timeouts("off").is_empty() && parse_timeouts("").is_empty());
        assert_eq!(parse_timeouts("1, абв"), [1, 5, 10, 30, 60]);
        assert_eq!(parse_timeouts("0, 99999999"), [1, 40320]);
    }

    #[test]
    fn request_and_answer() {
        let ai = settings("[ai]\nthinking_level = minimal\n");
        let turns = [Turn::user("привет"), Turn { role: "model".into(), text: "бака".into(), images: Vec::new() },
                     Turn { role: "user".into(), text: "что тут?".into(), images: vec![(vec![1, 2, 3], "image/png".into())] }];
        let body = request_body(&ai, "gemini-3.5-flash-lite", "характер", &turns);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "характер");
        assert_eq!(body["contents"][1]["role"], "model");
        assert_eq!(body["contents"][2]["parts"][1]["inlineData"]["data"], "AQID");
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingLevel"], "minimal");
        // «minimal» есть только у Flash-Lite
        assert_eq!(request_body(&ai, "gemini-3.8-flash", "", &turns)["generationConfig"]["thinkingConfig"]["thinkingLevel"], "low");
        assert!(request_body(&settings("[ai]\nthinking_level = none\n"), "m", "", &turns)["generationConfig"].get("thinkingConfig").is_none());

        let response: Value = serde_json::from_str(r#"{"candidates":[{"content":{"parts":[
            {"text":"думаю…","thought":true},{"text":" Привет, "},{"text":"бака! "}]},"finishReason":"STOP"}]}"#).unwrap();
        assert_eq!(parse_answer(&response), Answer { text: "Привет, бака!".into(), cut: false });
        let cut: Value = serde_json::from_str(r#"{"candidates":[{"content":{"parts":[{"text":"длинно"}]},"finishReason":"MAX_TOKENS"}]}"#).unwrap();
        assert!(parse_answer(&cut).cut);
        assert_eq!(parse_answer(&json!({})).text, "");
    }

    #[test]
    fn tags() {
        assert_eq!(strip_tags("Ладно. [ВАРН: 123 за спам] Готово", &MOD_TAGS), "Ладно.  Готово");
        assert_eq!(strip_tags("[тайм-аут: 5 60 флуд] ок [снять таймаут: 5]", &MOD_TAGS), "ок");
        assert_eq!(strip_tags("[ЗАПОМНИТЬ: любит чай] Запомнила", &["ЗАПОМНИТЬ"]), "Запомнила");
        assert_eq!(strip_tags("массив a[0] и [ссылка](url)", &MOD_TAGS), "массив a[0] и [ссылка](url)");
        assert!(has_ignore_tag("  [игнор] ") && !has_ignore_tag("игнор"));
    }

    #[test]
    fn one_line_and_split() {
        assert_eq!(to_one_line("Купи:\n- молоко\n- хлеб\n\n* сыр\nИ всё."), "Купи: молоко, хлеб, сыр И всё.");
        assert_eq!(to_one_line("код:\n```\nx = 1\n```"), "код:\n```\nx = 1\n```");
        assert_eq!(split_message("коротко", 10), ["коротко"]);
        assert_eq!(split_message("строка1\nстрока2\nстрока3", 16), ["строка1\nстрока2", "строка3"]);
        assert_eq!(split_message("абвгдежз", 3), ["абв", "где", "жз"]);
        let parts = split_message("```\nодин\nдва\nтри\n```", 12);
        assert!(parts.iter().all(|part| part.matches("```").count() % 2 == 0), "{parts:?}");
    }

    #[test]
    fn bad_words() {
        let words = parse_bad_words("# список\nсука, бля*\nиди* лесом\n ,*,\n");
        assert_eq!(words.len(), 3);
        assert!(contains_bad_word(&words, "Ну ты и СУКА!"));
        assert!(!contains_bad_word(&words, "купил сукно")); // край слова
        assert!(contains_bad_word(&words, "вот блядство")); // * — любые буквы
        assert!(!contains_bad_word(&words, "табля")); // начало слова
        assert!(contains_bad_word(&words, "идите лесом"));
        assert!(!contains_bad_word(&words, "иди домой"));
        assert!(!contains_bad_word(&[], "что угодно"));
    }

    #[test]
    fn rudeness_steps() {
        let steps = [1, 5, 10, 30, 60];
        assert_eq!([rude_timeout(&steps, 1), rude_timeout(&steps, 2), rude_timeout(&steps, 3), rude_timeout(&steps, 99)],
                   [None, Some(1), Some(5), Some(60)]);
        assert_eq!(rude_timeout(&[], 5), None);
        assert_eq!([minutes_text(1), minutes_text(5), minutes_text(60), minutes_text(120), minutes_text(1440), minutes_text(90)],
                   ["1 минуту", "5 минут", "1 час", "2 часа", "1 день", "90 минут"]);
    }

    #[test]
    fn limits() {
        let ai = AiState::new(&crate::config::Settings::parse("[ai]\ncooldown_seconds = 0\ndaily_limit_per_user = 2\n"), Some("ключ".into()));
        assert!(ai.ready() && !AiState::new(&crate::config::Settings::default(), Some(String::new())).ready());
        assert_eq!([ai.limit_check(1), ai.limit_check(1)], [None, None]);
        assert!(ai.limit_check(1).unwrap().starts_with("На сегодня лимит вопросов (2)"));
        assert_eq!(ai.usage_today(1).0, 2);
        assert_eq!(ai.limit_check(2), None); // у другого человека свой счётчик
        let slow = AiState::new(&crate::config::Settings::default(), None);
        assert_eq!(slow.limit_check(1), None);
        assert!(slow.limit_check(1).unwrap().starts_with("Не так быстро"));
    }

    /// Настоящий запрос к Gemini с ключом из GEMINI_API_KEY и моделями из config/settings.ini.
    /// Запуск: cargo test real_gemini -- --ignored (тратит 1–2 запроса из лимита ключа).
    #[tokio::test]
    #[ignore]
    async fn real_gemini() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let settings = crate::config::Settings::load(&dir.join("config").join("settings.ini"));
        let data = Data::new(crate::db::Database::in_memory().unwrap(), &settings, dir, false);
        assert!(data.ai.ready(), "нет GEMINI_API_KEY");
        let models = (data.ai.settings.model.clone(), data.ai.settings.fallback_model.clone());
        // Сначала основная модель отдельно — чтобы увидеть её ошибку, а не ответ запасной
        let system = system_prompt(&data);
        let main = call_gemini(&data, &models.0, &system, &[Turn::user("Тест: ответь одним словом «работает».")]).await;
        println!("основная модель {}: {main:?}", models.0);
        // Потом полный путь, как у /ask: память разговора, повторы, запасная модель
        let answer = ask(&data, "Сколько будет 2+2? Ответь коротко.", "Тестер", 1, Vec::new(), Extras::default()).await;
        println!("ask: {answer:?}");
        assert!(matches!(&answer, Ok(Some(text)) if !text.is_empty()), "{answer:?}");
        assert!(main.is_ok(), "основная модель не ответила: {main:?}");
    }

    #[test]
    fn prompt_files() {
        assert_eq!(parse_prompt("# пояснение\nТы тсундере.\n  # ещё\nОтвечай кратко.\n"), "Ты тсундере.\nОтвечай кратко.");
        assert!(TECH_RULES.contains("[ИГНОР]") && TECH_RULES.contains("[ВАРН: id причина]"));
        assert!(DEFAULT_CHARACTER.trim().chars().count() > 50);
        assert_eq!(image_type(Some("image/png; charset=binary")), "image/png");
        assert!(is_supported_image(Some("image/jpeg"), 1024) && !is_supported_image(Some("image/gif"), 10));
        assert!(!is_supported_image(Some("image/png"), 6 * 1024 * 1024) && !is_supported_image(None, 1));
    }
}
