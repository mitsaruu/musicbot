//! Команды ИИ: /ask, /forget, /ai_limits, /summary, /quiz.

use crate::admin::report_error;
use crate::ai::{self, Turn};
use crate::reply::{display_name, whisper};
use crate::text::money_text;
use crate::{db, Context, Error};
use poise::serenity_prelude as serenity;
use std::collections::HashSet;
use std::time::Duration;

const NOT_READY: &str = "ИИ не настроен: нет ключа GEMINI_API_KEY.";

/// Убирает «бот думает…», как будто команды и не было (когда бот решил промолчать).
async fn vanish(ctx: Context<'_>) {
    if let poise::Context::Application(app) = ctx {
        let _ = app.interaction.delete_response(ctx.http()).await;
    }
}

/// Задать вопрос ИИ (можно с картинкой)
#[poise::command(slash_command)]
pub async fn ask(
    ctx: Context<'_>,
    #[description = "Твой вопрос"] question: String,
    #[description = "Картинка к вопросу (необязательно): PNG, JPEG, WEBP"] image: Option<serenity::Attachment>,
) -> Result<(), Error> {
    let data = ctx.data();
    if !data.ai.ready() {
        return whisper(ctx, NOT_READY).await;
    }
    if let Some(image) = &image {
        if !ai::is_supported_image(image.content_type.as_deref(), image.size as u64) {
            return whisper(ctx, format!(
                "Это не подходит: нужна картинка PNG, JPEG или WEBP не больше {} МБ.", ai::MAX_IMAGE_MB)).await;
        }
    }
    if ai::is_rude(data, &question) {
        // Вопрос с матом — молчим. Discord требует ответа на команду, поэтому отвечаем невидимо и сразу удаляем.
        ctx.defer_ephemeral().await?;
        vanish(ctx).await;
        ai::punish_rudeness(ctx.serenity_context(), data, ctx.guild_id(), ctx.author(), ctx.channel_id()).await;
        return Ok(());
    }
    if let Some(refusal) = data.ai.limit_check(ctx.author().id.get()) {
        return whisper(ctx, refusal).await;
    }
    ctx.defer().await?; // ИИ отвечает несколько секунд — показываем «бот думает…»
    let mut images = Vec::new();
    if let Some(image) = &image {
        match image.download().await {
            Ok(bytes) => images.push((bytes, ai::image_type(image.content_type.as_deref()))),
            Err(error) => report_error(ctx.http(), data, ctx.guild_id(), "Скачивание картинки для ИИ", &error.to_string()).await,
        }
    }
    let name = display_name(ctx).await;
    // На сервере модератор может попросить через /ask выдать варн или тайм-аут
    let member = ctx.author_member().await.map(|member| member.into_owned());
    let mut extras = ai::Extras::default();
    if let (Some(guild), Some(member)) = (ctx.guild_id(), &member) {
        let note = crate::moderation::context_note(ctx.serenity_context(), guild, member, &question, &[]).await;
        extras.moderation = Some((crate::moderation::Requester { ctx: ctx.serenity_context(), guild, member }, note));
    }
    let answer = match ai::ask(data, &question, &name, ctx.channel_id().get(), images, extras).await {
        Ok(answer) => answer,
        Err(error) => {
            report_error(ctx.http(), data, ctx.guild_id(), "ИИ /ask", &error.to_string()).await;
            Some(ai::error_text(&error).to_string())
        }
    };
    let Some(answer) = answer else {
        // ИИ решил, что вопрос грубый, — убираем «бот думает…» и молчим
        vanish(ctx).await;
        ai::punish_rudeness(ctx.serenity_context(), data, ctx.guild_id(), ctx.author(), ctx.channel_id()).await;
        return Ok(());
    };
    if let Some(guild) = ctx.guild_id() {
        data.db.reset_rude_strikes(guild.get(), ctx.author().id.get())?; // нормальный вопрос — счётчик грубостей с нуля
    }
    let no_pings = serenity::CreateAllowedMentions::new(); // иначе ИИ мог бы случайно написать @everyone
    for (index, part) in ai::split_message(&format!("❓ {question}\n\n{answer}"), 1900).into_iter().enumerate() {
        let mut reply = poise::CreateReply::default().content(part).allowed_mentions(no_pings.clone());
        if let (0, Some(image)) = (index, &image) {
            // показываем картинку, о которой спрашивали, в «карточке» под первым сообщением
            reply = reply.embed(serenity::CreateEmbed::new().color(0x8B5CF6).image(&image.url));
        }
        ctx.send(reply).await?;
    }
    Ok(())
}

/// Стереть память разговора с ботом в этом канале
#[poise::command(slash_command)]
pub async fn forget(ctx: Context<'_>) -> Result<(), Error> {
    let removed = ctx.data().db.clear_ai_history(ctx.channel_id().get())?;
    ctx.say(format!("💬 Память разговора в этом канале стёрта (сообщений: {removed}).")).await?;
    Ok(())
}

/// Сколько запросов к ИИ осталось
#[poise::command(slash_command)]
pub async fn ai_limits(ctx: Context<'_>) -> Result<(), Error> {
    let data = ctx.data();
    if !data.ai.ready() {
        return whisper(ctx, NOT_READY).await;
    }
    let settings = &data.ai.settings;
    // Личный лимит (его ведёт сам бот); обнуляется в полночь по времени компьютера, на котором запущен бот
    let (count, last) = data.ai.usage_today(ctx.author().id.get());
    let left = (settings.daily_limit - count).max(0);
    let tomorrow = (chrono::Local::now().date_naive() + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap();
    let personal_reset = tomorrow.and_local_timezone(chrono::Local).earliest().map_or(0, |time| time.timestamp());
    let wait = settings.cooldown as f64 - (db::now() - last);
    // Общий лимит ключа Google
    let used = data.db.ai_requests(&ai::google_day())?;
    let google_reset = ai::google_reset_time();
    // <t:ЧИСЛО:t> и <t:ЧИСЛО:R> — формат Discord: каждый увидит время в своём часовом поясе
    let mut lines = vec![
        "📊 **Лимиты ИИ**".to_string(),
        String::new(),
        "**Твой личный лимит** (ограничение бота):".to_string(),
        format!("• Осталось вопросов сегодня: **{left}** из {}", settings.daily_limit),
        format!("• Обновится: <t:{personal_reset}:t> (<t:{personal_reset}:R>)"),
    ];
    if wait > 0.0 {
        lines.push(format!("• Следующий вопрос можно задать через {} сек", wait as i64 + 1));
    }
    lines.extend([
        String::new(),
        format!("**Общий лимит Google** (на весь сервер, модель `{}`):", settings.model),
        format!("• Запросов сегодня: **{used}** из ~{} (осталось ~{})", settings.google_daily_limit, (settings.google_daily_limit - used).max(0)),
        format!("• За последнюю минуту: {} из ~{}", data.ai.requests_last_minute(), settings.google_minute_limit),
        format!("• Обновится: <t:{google_reset}:t> (<t:{google_reset}:R>)"),
    ]);
    let quota_error = data.ai.last_quota_error();
    if db::now() - quota_error < 24.0 * 3600.0 {
        lines.push(format!("⚠️ Google последний раз отказал из-за лимита <t:{}:R>", quota_error as i64));
    }
    // "-# " — в Discord это мелкий серый текст
    lines.extend([String::new(), "-# Лимиты Google примерные, бот считает только свои запросы. Точно: aistudio.google.com/rate-limit".to_string()]);
    whisper(ctx, lines.join("\n")).await
}

const SUMMARY_MAX_MESSAGES: usize = 300;

/// Задание для пересказа чата.
pub fn summary_task(hours: i64, chat: &str) -> String {
    format!(
        "Ниже сообщения из чата Discord за последние {hours} ч. Перескажи кратко, о чём говорили:\n\
         3–7 пунктов списком, самое важное — первым. Называй участников по именам, без упоминаний через @.\n\
         Не выдумывай того, чего в сообщениях нет. Если сообщений почти нет — так и скажи одной фразой.\n\n{chat}")
}

/// ИИ кратко перескажет, о чём говорили в этом канале
#[poise::command(slash_command, guild_only)]
pub async fn summary(
    ctx: Context<'_>,
    #[description = "За сколько последних часов (по умолчанию 1)"]
    #[min = 1]
    #[max = 24]
    hours: Option<i64>,
) -> Result<(), Error> {
    let (data, hours) = (ctx.data(), hours.unwrap_or(1));
    if !data.ai.ready() {
        return whisper(ctx, "ИИ не настроен: нет ключа Gemini.").await;
    }
    if let Some(refusal) = data.ai.limit_check(ctx.author().id.get()) {
        return whisper(ctx, refusal).await;
    }
    ctx.defer_ephemeral().await?;
    let after = db::now() as i64 - hours * 3600;
    // Discord отдаёт историю порциями по 100 сообщений, от новых к старым
    let (mut lines, mut before, mut done) = (Vec::new(), None::<serenity::MessageId>, false);
    while !done && lines.len() < SUMMARY_MAX_MESSAGES {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(before) = before {
            request = request.before(before);
        }
        let Ok(messages) = ctx.channel_id().messages(ctx, request).await else {
            ctx.say("Не могу прочитать историю этого канала — не хватает прав.").await?;
            return Ok(());
        };
        done = messages.len() < 100;
        for message in &messages {
            before = Some(message.id);
            if message.timestamp.unix_timestamp() < after {
                done = true;
                break;
            }
            let text: String = message.content_safe(ctx).chars().take(300).collect();
            if !text.is_empty() && !message.author.bot {
                let name = message.member.as_ref().and_then(|member| member.nick.clone()).unwrap_or_else(|| crate::reply::user_name(&message.author));
                lines.push(format!("{name}: {text}"));
            }
        }
    }
    if lines.len() < 3 {
        ctx.say(format!("За последние {hours} ч здесь почти ничего не писали.")).await?;
        return Ok(());
    }
    lines.reverse(); // история шла от новых к старым
    let task = summary_task(hours, &lines.join("\n"));
    let system = "Ты аккуратно и нейтрально пересказываешь переписку. Отвечай по-русски.";
    let answer = match ai::generate(data, system, &[Turn::user(task)]).await {
        Ok(answer) => answer,
        Err(error) => {
            ctx.say(ai::error_text(&error)).await?;
            return Ok(());
        }
    };
    let text = ai::strip_tags(&answer.text, &ai::MOD_TAGS);
    let text = if text.is_empty() { "🤔 ИИ не дал ответа.".to_string() } else { text };
    for part in ai::split_message(&format!("📝 **О чём говорили за {hours} ч** ({} сообщений):\n{text}", lines.len()), 1900) {
        ctx.send(poise::CreateReply::default().content(part).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    }
    Ok(())
}

/// Настройки викторины из settings.ini, раздел [quiz].
pub struct QuizSettings {
    pub reward: i64,  // монет за правильный ответ
    pub seconds: u64, // сколько секунд на ответ
}

impl QuizSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        Self { reward: settings.int("quiz", "reward", 100).max(0), seconds: settings.int("quiz", "seconds", 30).max(10) as u64 }
    }
}

/// Задание для ИИ: придумать вопрос. used — недавние вопросы, чтобы не повторяться.
pub fn quiz_task(topic: Option<&str>, used: &[String]) -> String {
    let topic = topic.map_or(" на любую тему".to_string(), |topic| format!(" на тему «{topic}»"));
    let used = if used.is_empty() { "пока нет".to_string() } else { used.join("; ") };
    format!(
        "Придумай один вопрос для викторины{topic}. Вопрос должен быть интересным, с одним точно правильным\n\
         ответом, не слишком простым и не слишком сложным. Ответь ТОЛЬКО в формате JSON, без пояснений:\n\
         {{\"question\": \"текст вопроса\", \"options\": [\"вариант 1\", \"вариант 2\", \"вариант 3\", \"вариант 4\"], \"answer\": 0}}\n\
         answer — номер правильного варианта, считая с нуля. Все варианты разные, каждый не длиннее 70 символов.\n\
         Не повторяй эти вопросы: {used}")
}

/// Ответ ИИ → (вопрос, 4 варианта, номер правильного) или None, если ИИ ответил не по формату.
pub fn parse_quiz(text: &str) -> Option<(String, Vec<String>, usize)> {
    let json: serde_json::Value = serde_json::from_str(text.get(text.find('{')?..=text.rfind('}')?)?).ok()?;
    let question: String = json.get("question")?.as_str()?.trim().chars().take(300).collect();
    let options: Vec<String> =
        json.get("options")?.as_array()?.iter().map(|option| option.as_str().map(|text| text.trim().chars().take(80).collect())).collect::<Option<_>>()?;
    let answer = json.get("answer")?.as_u64()? as usize;
    let different = options.iter().collect::<HashSet<_>>().len() == 4;
    (!question.is_empty() && options.len() == 4 && different && answer < 4 && options.iter().all(|option| !option.is_empty()))
        .then_some((question, options, answer))
}

fn quiz_buttons(options: &[String], reveal: Option<usize>) -> Vec<serenity::CreateActionRow> {
    let buttons = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            // когда ответ раскрыт — кнопки выключены, а правильная зелёная
            let style = if reveal == Some(index) { serenity::ButtonStyle::Success } else { serenity::ButtonStyle::Secondary };
            serenity::CreateButton::new(format!("quiz:{index}")).label(option).style(style).disabled(reveal.is_some())
        })
        .collect();
    vec![serenity::CreateActionRow::Buttons(buttons)]
}

/// Канал свободен для новой викторины, пока жив этот объект.
struct Running<'a> {
    data: &'a crate::Data,
    channel: u64,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.data.quiz_running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.channel);
    }
}

/// Викторина от ИИ: кто первым ответит правильно — получит монеты
#[poise::command(slash_command, guild_only)]
pub async fn quiz(
    ctx: Context<'_>,
    #[description = "Тема вопроса (если не указать — любая)"]
    #[max_length = 60]
    topic: Option<String>,
) -> Result<(), Error> {
    let (data, guild, channel) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.channel_id().get());
    if !data.ai.ready() {
        return whisper(ctx, "ИИ не настроен: нет ключа Gemini.").await;
    }
    if !data.quiz_running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(channel) {
        return whisper(ctx, "В этом канале уже идёт викторина — сначала ответьте на неё.").await;
    }
    let _running = Running { data, channel };
    if let Some(refusal) = data.ai.limit_check(ctx.author().id.get()) {
        return whisper(ctx, refusal).await;
    }
    ctx.defer().await?;
    let used: Vec<String> = data.quiz_recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&guild).cloned().unwrap_or_default();
    let task = quiz_task(topic.as_deref(), &used[used.len().saturating_sub(15)..]);
    let system = "Ты ведущий викторины. Отвечай по-русски и строго в формате JSON.";
    let answer = match ai::generate(data, system, &[Turn::user(task)]).await {
        Ok(answer) => answer,
        Err(error) => {
            ctx.say(ai::error_text(&error)).await?;
            return Ok(());
        }
    };
    let Some((question, options, correct)) = parse_quiz(&answer.text) else {
        ctx.say("🤔 ИИ придумал вопрос не по правилам — попробуй ещё раз.").await?;
        return Ok(());
    };
    {
        let mut recent = data.quiz_recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let list = recent.entry(guild).or_default();
        list.push(question.clone());
        let extra = list.len().saturating_sub(30);
        list.drain(..extra);
    }
    let (reward, seconds) = (data.quiz.reward, data.quiz.seconds);
    let reply = ctx
        .send(poise::CreateReply::default()
            .content(format!(
                "🧠 **Викторина:** {question}\nНа ответ {seconds} сек., у каждого одна попытка. Приз — {}.", money_text(reward)))
            .components(quiz_buttons(&options, None)))
        .await?;
    let message_id = reply.message().await?.id;
    let result = |ending: String| format!("🧠 **Викторина:** {question}\n✅ Правильный ответ: **{}**\n{ending}", options[correct]);
    let (deadline, mut tried) = (tokio::time::Instant::now() + Duration::from_secs(seconds), HashSet::new());
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context()).message_id(message_id).timeout(left).await;
        let Some(press) = press else {
            let ended = poise::CreateReply::default()
                .content(result("⌛ Время вышло — никто не угадал.".to_string()))
                .components(quiz_buttons(&options, Some(correct)));
            let _ = reply.edit(ctx, ended).await;
            return Ok(());
        };
        let note = |text: &str| {
            serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true))
        };
        if !tried.insert(press.user.id) {
            press.create_response(ctx, note("У тебя была одна попытка 🙂")).await?;
            continue;
        }
        if press.data.custom_id != format!("quiz:{correct}") {
            press.create_response(ctx, note("❌ Неверно.")).await?;
            continue;
        }
        let name = guild_name(&press);
        let balance = data.db.add_money(guild, press.user.id.get(), &name, data.economy.start_balance, reward)?;
        let won = serenity::CreateInteractionResponseMessage::new()
            .content(result(format!(
                "🏆 Первым правильно ответил <@{}>: **+{}** (баланс {}).", press.user.id, money_text(reward), money_text(balance))))
            .components(quiz_buttons(&options, Some(correct)))
            .allowed_mentions(serenity::CreateAllowedMentions::new());
        press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(won)).await?;
        return Ok(());
    }
}

/// Имя нажавшего кнопку на сервере (ник), а если ника нет — обычное имя.
fn guild_name(press: &serenity::ComponentInteraction) -> String {
    press.member.as_ref().map(|member| member.display_name().to_string()).unwrap_or_else(|| crate::reply::user_name(&press.user))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiz_parsing() {
        let good = r#"Вот: {"question": " Столица Австралии? ", "options": ["Сидней", "Канберра", "Мельбурн", "Перт"], "answer": 1} готово"#;
        let (question, options, answer) = parse_quiz(good).unwrap();
        assert_eq!((question.as_str(), options.len(), answer), ("Столица Австралии?", 4, 1));
        assert_eq!(parse_quiz("чушь"), None);
        assert_eq!(parse_quiz(r#"{"question":"q","options":["a","a","b","c"],"answer":0}"#), None); // одинаковые варианты
        assert_eq!(parse_quiz(r#"{"question":"q","options":["a","b","c"],"answer":0}"#), None);
        assert_eq!(parse_quiz(r#"{"question":"q","options":["a","b","c","d"],"answer":4}"#), None);
        assert_eq!(parse_quiz(r#"{"question":"","options":["a","b","c","d"],"answer":0}"#), None);
        assert_eq!(parse_quiz(r#"{"question":"q","options":["a","b","c",5],"answer":0}"#), None);
    }

    #[test]
    fn tasks() {
        let task = quiz_task(Some("космос"), &["Первый вопрос".to_string(), "Второй".to_string()]);
        assert!(task.contains("на тему «космос»") && task.contains("Не повторяй эти вопросы: Первый вопрос; Второй"));
        assert!(task.contains(r#"{"question": "текст вопроса""#));
        assert!(quiz_task(None, &[]).contains("на любую тему") && quiz_task(None, &[]).ends_with("пока нет"));
        assert!(summary_task(3, "A: привет").starts_with("Ниже сообщения из чата Discord за последние 3 ч."));
        let quiz = QuizSettings::load(&crate::config::Settings::default());
        assert_eq!((quiz.reward, quiz.seconds), (100, 30));
    }
}
