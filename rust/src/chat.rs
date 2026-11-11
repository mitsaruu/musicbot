//! Бот в чате: ответы ИИ на упоминания и ответы на его сообщения, автоответы (config/auto_replies.txt),
//! анти-спам, ответы другим ботам и случайные реплики по теме разговора.

use crate::admin::report_error;
use crate::ai::{self, is_letter};
use crate::moderation::{self, Requester};
use crate::reply::user_name;
use crate::{db, Data, Shared};
use poise::serenity_prelude as serenity;
use rand::Rng;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const CONTEXT_MESSAGES: u8 = 10; // сколько последних сообщений чата показывать ИИ, чтобы он понимал разговор
const CHATTER_HISTORY: u8 = 15; // сколько сообщений показывать ИИ для случайной реплики
const RUDE_MEMORY_HOURS: f64 = 24.0; // сколько часов автоответы помнят, что человек грубил боту

const CHATTER_TASK: &str = "Ниже последние сообщения из чата Discord. Ты — участник этого чата, тебя никто не звал.
Вставь от себя ОДНУ короткую реплику в тему разговора: шутку, мнение, вопрос или интересный факт по теме.
Не здоровайся, не говори, что тебя не звали, не пересказывай чат и не упоминай, что ты бот.
Если тема неприличная, грубая или сказать нечего — напиши только [ИГНОР].

";

fn on(text: &str) -> bool {
    !["off", "нет", "no", "0"].contains(&text.trim().to_lowercase().as_str())
}

/// Настройки из settings.ini: разделы [names], [ai], [chatter], [antispam].
pub struct ChatSettings {
    pub bot_names: Vec<String>,  // как ещё зовут бота (на эти имена он отвечает в приветствиях)
    pub reply_to_bots: bool,     // отвечать ли другим ботам, когда они обращаются к нашему
    pub bot_reply_limit: i64,    // и не больше стольких раз подряд в канале
    pub chatter_chance: i64,     // шанс случайной реплики, в процентах
    pub chatter_interval: u64,   // как часто бросать «кубик», минуты
    pub antispam: bool,
    pub spam_repeats: usize,     // столько одинаковых сообщений подряд — спам
    pub spam_links: usize,       // столько сообщений со ссылками подряд — спам
    pub spam_window: f64,        // за сколько секунд их считать
}

impl ChatSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        Self {
            bot_names: settings
                .text("names", "bot_names", "малфоид, malfoid, maldoid, hallaan, халлаан")
                .split(',')
                .map(|name| name.trim().to_lowercase().replace('ё', "е"))
                .filter(|name| !name.is_empty())
                .collect(),
            reply_to_bots: on(&settings.text("ai", "reply_to_bots", "on")),
            bot_reply_limit: settings.int("ai", "bot_reply_limit", 3).max(0),
            chatter_chance: settings.int("chatter", "chance_percent", 5).clamp(0, 100),
            chatter_interval: settings.int("chatter", "interval_minutes", 20).max(1) as u64,
            antispam: on(&settings.text("antispam", "enabled", "on")),
            spam_repeats: settings.int("antispam", "same_messages", 4).max(2) as usize,
            spam_links: settings.int("antispam", "link_messages", 4).max(2) as usize,
            spam_window: settings.int("antispam", "window_seconds", 20).max(5) as f64,
        }
    }
}

/// Всё, что бот помнит о чате между сообщениями (после перезапуска — с нуля).
pub struct ChatState {
    pub settings: ChatSettings,
    /// Сервер → имена и ники тех, кто писал в чат (чтобы узнавать их в приветствиях)
    known_names: Mutex<HashMap<u64, HashSet<String>>>,
    /// Сервер → (канал, время): где последний раз писали люди (для случайных реплик)
    last_activity: Mutex<HashMap<u64, (u64, f64)>>,
    /// Канал → сколько раз подряд бот ответил другим ботам
    bot_reply_streak: Mutex<HashMap<u64, i64>>,
    spam: Mutex<SpamTracker>,
}

impl ChatState {
    pub fn new(settings: &crate::config::Settings) -> Self {
        Self {
            settings: ChatSettings::load(settings),
            known_names: Default::default(),
            last_activity: Default::default(),
            bot_reply_streak: Default::default(),
            spam: Default::default(),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Нижний регистр и ё → е, чтобы «Привет», «ПРИВЕТ» и «Санёк»/«Санек» срабатывали одинаково.
pub fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

// ---------- Анти-спам ----------

struct SpamEntry {
    time: f64,
    text: String,
    link: bool,
    message: (u64, u64), // (канал, сообщение) — чтобы удалить
}

/// Последние сообщения каждого участника и когда его последний раз наказали.
#[derive(Default)]
pub struct SpamTracker {
    recent: HashMap<(u64, u64), Vec<SpamEntry>>,
    punished: HashMap<(u64, u64), f64>,
}

/// Итог проверки: какие сообщения удалить, за что и выдавать ли варн (за один спам — один варн).
#[derive(Debug, PartialEq)]
pub struct Spam {
    pub messages: Vec<(u64, u64)>,
    pub reason: &'static str,
    pub punish: bool,
}

pub fn has_link(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("http://") || text.contains("https://") || text.contains("discord.gg/")
}

impl SpamTracker {
    /// key — (сервер, участник). Возвращает Some, если это спам.
    pub fn check(&mut self, settings: &ChatSettings, key: (u64, u64), now: f64, content: &str, message: (u64, u64)) -> Option<Spam> {
        let text = content.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
        let recent = self.recent.entry(key).or_default();
        recent.retain(|entry| now - entry.time <= settings.spam_window);
        recent.push(SpamEntry { time: now, link: has_link(&text), text: text.clone(), message });
        if recent.len() > 12 {
            recent.drain(..recent.len() - 12);
        }
        let same: Vec<(u64, u64)> = recent.iter().filter(|entry| !text.is_empty() && entry.text == text).map(|entry| entry.message).collect();
        let links: Vec<(u64, u64)> = recent.iter().filter(|entry| entry.link).map(|entry| entry.message).collect();
        let (messages, reason) = if same.len() >= settings.spam_repeats {
            (same, "спам одинаковыми сообщениями")
        } else if links.len() >= settings.spam_links {
            (links, "спам ссылками")
        } else {
            return None;
        };
        recent.clear();
        // за этот спам уже наказали — просто молча чистим
        let punish = self.punished.get(&key).is_none_or(|last| now - last >= 60.0);
        if punish {
            self.punished.insert(key, now);
        }
        Some(Spam { messages, reason, punish })
    }
}

/// Проверяет сообщение на спам. true — это спам: сообщения удалены, участнику выдан варн.
async fn check_spam(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, message: &serenity::Message) -> bool {
    if !data.chat.settings.antispam {
        return false;
    }
    let Ok(member) = guild.member(ctx, message.author.id).await else { return false };
    if moderation::is_moderator(ctx, guild, &member) {
        return false; // модераторов не трогаем
    }
    let key = (guild.get(), message.author.id.get());
    let ids = (message.channel_id.get(), message.id.get());
    let Some(spam) = locked(&data.chat.spam).check(&data.chat.settings, key, db::now(), &message.content, ids) else { return false };
    for (channel, id) in &spam.messages {
        // убираем спам из чата (если у бота есть право «Управлять сообщениями»)
        let _ = serenity::ChannelId::new(*channel).delete_message(ctx, serenity::MessageId::new(*id)).await;
    }
    if !spam.punish {
        return true;
    }
    let bot = ctx.cache.current_user().id.get();
    let Ok((count, minutes)) = moderation::give_warn(ctx, data, guild, message.author.id, bot, spam.reason, "").await else { return true };
    let mut result = format!("🚫 <@{}> получает варн №{count}: {}.", message.author.id, spam.reason);
    if let Some(minutes) = minutes {
        result.push_str(&format!(" И тайм-аут на {}.", ai::minutes_text(minutes)));
    }
    let _ = message.channel_id.send_message(ctx, no_pings(&result)).await;
    moderation::send_log(ctx, data, guild, &format!("{result} Канал: <#{}>", message.channel_id)).await;
    true
}

fn no_pings(text: &str) -> serenity::CreateMessage {
    serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new())
}

// ---------- Автоответы ----------

/// Строка из config/auto_replies.txt.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    words: Vec<(Vec<char>, bool)>, // слово и есть ли * на конце («любые буквы»)
    pub options: Vec<String>,
    pub rude_options: Option<Vec<String>>, // ответы тем, кто недавно грубил боту (после ||)
    pub greeting: bool,                    // строка с ~: не отвечаем, если приветствие адресовано другому
}

/// Строки «слова = ответы || ответы грубиянам» → правила.
pub fn parse_auto_replies(text: &str) -> Vec<Rule> {
    let split = |text: &str| -> Vec<String> { text.split('|').map(str::trim).filter(|answer| !answer.is_empty()).map(String::from).collect() };
    let mut rules = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let greeting = line.starts_with('~');
        let (words, answers) = line.trim_start_matches('~').split_once('=').unwrap_or_default();
        let (nice, rude) = answers.split_once("||").unwrap_or((answers, ""));
        let options = split(nice);
        let rude_options = split(rude);
        let words: Vec<(Vec<char>, bool)> = words
            .split(',')
            .map(normalize)
            .map(|word| word.trim().to_string())
            .filter(|word| !word.is_empty())
            .map(|word| (word.trim_end_matches('*').chars().collect(), word.ends_with('*')))
            .collect();
        if !words.is_empty() && !options.is_empty() {
            rules.push(Rule { words, options, rude_options: (!rude_options.is_empty()).then_some(rude_options), greeting });
        }
    }
    rules
}

/// Есть ли слово в тексте: граница слова с обеих сторон — «привет» сработает в «привет всем», но не в «приветствую».
fn has_word(text: &[char], word: &[char], any_ending: bool) -> bool {
    (0..text.len()).any(|start| {
        if start > 0 && is_letter(text[start - 1]) || !text[start..].starts_with(word) {
            return false;
        }
        let mut end = start + word.len();
        if any_ending {
            while end < text.len() && is_letter(text[end]) {
                end += 1;
            }
        }
        end >= text.len() || !is_letter(text[end])
    })
}

impl Rule {
    pub fn matches(&self, text: &[char]) -> bool {
        self.words.iter().any(|(word, any_ending)| has_word(text, word, *any_ending))
    }
}

/// «Kirill_228 (Кирюха)» → {"kirill", "кирюха"}: слова из имени длиной от 3 букв.
pub fn name_words(name: &str) -> HashSet<String> {
    words(&normalize(name)).into_iter().filter(|word| word.chars().count() >= 3).collect()
}

/// Слова текста (только буквы и цифры).
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !is_letter(c)).filter(|word| !word.is_empty()).map(String::from).collect()
}

/// Похоже ли слово на одно из имён, с учётом окончаний: «саню», «кириллу», «малфоиду».
pub fn word_is_name(word: &str, names: &HashSet<String>) -> bool {
    let length = word.chars().count();
    names.iter().any(|name| {
        let name: Vec<char> = name.chars().collect();
        let stem: String = if name.len() < 4 { name.iter().collect() } else { name[..name.len() - 1].iter().collect() }; // «кирилл» → «кирил»
        word.starts_with(&stem) && length.saturating_sub(stem.chars().count()) <= 3
    })
}

/// Что известно о сообщении с приветствием.
pub struct Greeting<'a> {
    pub text: &'a str,                  // текст в нижнем регистре, ё → е
    pub bot_names: &'a [String],        // имена бота из settings.ini
    pub bot_words: HashSet<String>,     // имена бота: из настроек и из Discord
    pub mentions_others: bool,          // упомянули кого-то, кроме бота (или роль)
    pub replies_to_other: bool,         // это ответ на чужое сообщение
    pub other_names: HashSet<String>,   // имена других участников
}

/// Приветствие адресовано кому-то другому, а не боту? «привет» — боту; «привет саня» — не боту; «привет малфоид» — боту.
pub fn greeting_for_someone_else(greeting: &Greeting, rules: &[Rule]) -> bool {
    let words = words(greeting.text);
    if words.iter().any(|word| word_is_name(word, &greeting.bot_words))
        || greeting.bot_names.iter().any(|name| name.contains(' ') && greeting.text.contains(name.as_str()))
    {
        return false; // позвали бота по имени — отвечаем
    }
    if greeting.mentions_others || greeting.replies_to_other {
        return true; // «привет @Саня» или «привет» в ответ на чужое сообщение
    }
    // Чьё-то имя: слова из других автоответов (саня, даня…) или имя/ник кого-то с сервера
    let chars: Vec<char> = greeting.text.chars().collect();
    if rules.iter().any(|rule| !rule.greeting && rule.matches(&chars)) {
        return true;
    }
    let others: HashSet<String> = greeting.other_names.difference(&greeting.bot_words).cloned().collect();
    words.iter().any(|word| word_is_name(word, &others))
}

/// Имя на сервере (ник), а если ника нет — обычное имя.
fn message_author_name(message: &serenity::Message) -> String {
    message.member.as_ref().and_then(|member| member.nick.clone()).unwrap_or_else(|| user_name(&message.author))
}

/// Имя автора старого сообщения: ник из памяти бота или обычное имя.
fn author_name(ctx: &serenity::Context, guild: serenity::GuildId, user: &serenity::User) -> String {
    cached_name(ctx, guild, user.id).unwrap_or_else(|| user_name(user))
}

/// Имя участника на сервере из памяти бота (None — бот его ещё не видел).
fn cached_name(ctx: &serenity::Context, guild: serenity::GuildId, user: serenity::UserId) -> Option<String> {
    ctx.cache.guild(guild)?.members.get(&user).map(|member| member.display_name().to_string())
}

/// Запоминаем имя, ник и глобальное имя автора сообщения — чтобы узнавать их в приветствиях.
fn remember_names(data: &Data, guild: u64, message: &serenity::Message) {
    let mut names = locked(&data.chat.known_names);
    let words = names.entry(guild).or_default();
    for name in [Some(message.author.name.clone()), message.author.global_name.clone(), Some(message_author_name(message))].into_iter().flatten() {
        words.extend(name_words(&name));
    }
}

/// Если в сообщении есть слово из config/auto_replies.txt — отвечает на него.
async fn auto_reply(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, message: &serenity::Message) {
    let file = std::fs::read_to_string(data.dir.join("config").join("auto_replies.txt")).unwrap_or_default();
    let rules = parse_auto_replies(&file);
    let text = normalize(&message.content);
    let chars: Vec<char> = text.chars().collect();
    let bot = ctx.cache.current_user().clone();
    for rule in &rules {
        if !rule.matches(&chars) {
            continue;
        }
        if rule.greeting {
            let mut bot_words: HashSet<String> = data.chat.settings.bot_names.iter().cloned().collect();
            bot_words.extend(name_words(&bot.name));
            bot_words.extend(bot.global_name.as_deref().map(name_words).unwrap_or_default());
            if let Some(name) = cached_name(ctx, guild, bot.id) {
                bot_words.extend(name_words(&name));
            }
            let mut other_names = locked(&data.chat.known_names).get(&guild.get()).cloned().unwrap_or_default();
            if let Some(cached) = ctx.cache.guild(guild) {
                for member in cached.members.values().filter(|member| !member.user.bot) {
                    other_names.extend(name_words(&member.user.name));
                    other_names.extend(name_words(member.display_name()));
                }
            }
            // своё имя не в счёт
            for own in name_words(&message.author.name).union(&name_words(&message_author_name(message))) {
                other_names.remove(own);
            }
            let replied = message.referenced_message.as_ref();
            let greeting = Greeting {
                text: &text,
                bot_names: &data.chat.settings.bot_names,
                bot_words,
                mentions_others: message.mentions.iter().any(|user| user.id != bot.id) || !message.mention_roles.is_empty(),
                replies_to_other: replied.is_some_and(|replied| replied.author.id != bot.id),
                other_names,
            };
            if greeting_for_someone_else(&greeting, &rules) {
                continue; // «привет саня» — здороваются не с ботом, приветствие пропускаем
            }
        }
        let mut options = &rule.options;
        if let Some(rude) = &rule.rude_options {
            let last = data.db.last_rude_time(guild.get(), message.author.id.get()).ok().flatten();
            if last.is_some_and(|last| db::now() - last < RUDE_MEMORY_HOURS * 3600.0) {
                options = rude; // человек недавно грубил боту — отвечаем ему по-другому
            }
        }
        let answer = options[rand::rng().random_range(0..options.len())].clone();
        // отвечаем цитатой, без пинга автора и без пингов вообще (даже если в ответе написать @everyone)
        let reply = no_pings(&answer).reference_message(message);
        let _ = message.channel_id.send_message(ctx, reply).await;
        return; // отвечаем только один раз, даже если в сообщении несколько слов
    }
}

// ---------- Ответы ИИ на сообщения ----------

/// Обращаются ли к боту: упомянули его или ответили на его сообщение.
fn is_addressed_to_bot(bot: serenity::UserId, message: &serenity::Message) -> bool {
    message.mentions.iter().any(|user| user.id == bot) || message.referenced_message.as_ref().is_some_and(|replied| replied.author.id == bot)
}

/// Несколько последних сообщений чата одной строкой — чтобы ИИ понимал, о чём разговор.
async fn chat_context(ctx: &serenity::Context, guild: serenity::GuildId, message: &serenity::Message) -> String {
    let request = serenity::GetMessages::new().before(message.id).limit(CONTEXT_MESSAGES);
    let Ok(history) = message.channel_id.messages(ctx, request).await else { return String::new() }; // нет права читать историю
    let bot = ctx.cache.current_user().id;
    let mut lines: Vec<String> = history
        .iter()
        .filter(|old| old.author.id != bot) // свои ответы бот и так помнит
        .filter_map(|old| {
            let mut text: String = old.content_safe(&ctx.cache).chars().take(300).collect();
            if !old.attachments.is_empty() {
                text = format!("{text} [вложение]").trim().to_string();
            }
            (!text.is_empty()).then(|| format!("{}: {text}", author_name(ctx, guild, &old.author)))
        })
        .collect();
    lines.reverse(); // история идёт от новых к старым
    lines.join("\n")
}

/// Отвечает с помощью ИИ на сообщение, в котором обратились к боту.
async fn ai_reply(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, message: &serenity::Message) {
    if ai::is_rude(data, &message.content) {
        // к боту обратились с матом — молчим (и не тратим запрос к Google)
        println!("Бот промолчал на мат: пользователь={} канал={}", message.author.id, message.channel_id);
        ai::punish_rudeness(ctx, data, Some(guild), &message.author, message.channel_id).await;
        return;
    }
    if let Some(refusal) = data.ai.limit_check(message.author.id.get()) {
        let _ = message.channel_id.send_message(ctx, no_pings(&refusal).reference_message(message)).await;
        return;
    }
    let bot = ctx.cache.current_user().id;
    let question = message.content.replace(&format!("<@{bot}>"), "").replace(&format!("<@!{bot}>"), "").trim().to_string();
    // Картинки: из самого сообщения и из сообщения, на которое ответили (например, «@бот что тут?» на чужое фото)
    let replied = message.referenced_message.as_deref();
    let attachments: Vec<&serenity::Attachment> = message
        .attachments
        .iter()
        .chain(replied.map(|replied| replied.attachments.iter()).into_iter().flatten())
        .filter(|attachment| ai::is_supported_image(attachment.content_type.as_deref(), attachment.size as u64))
        .take(ai::MAX_IMAGES)
        .collect();
    let question = match (question.is_empty(), attachments.is_empty()) {
        (false, _) => question,
        (true, false) => "(прислал картинку без текста)".to_string(),
        (true, true) => "(просто позвал тебя, ничего не написав)".to_string(),
    };

    let typing = message.channel_id.start_typing(&ctx.http); // «бот печатает…», пока ИИ думает
    let mut images = Vec::new();
    for attachment in attachments {
        match attachment.download().await {
            Ok(bytes) => images.push((bytes, ai::image_type(attachment.content_type.as_deref()))),
            Err(error) => report_error(&ctx.http, data, Some(guild), "Скачивание картинки для ИИ", &error.to_string()).await,
        }
    }
    let chat = chat_context(ctx, guild, message).await;
    // Кого могут попросить наказать: упомянутые и автор сообщения, на которое ответили («дай ему варн»)
    let mut targets = Vec::new();
    for user in message.mentions.iter().chain(replied.map(|replied| &replied.author)) {
        if let Ok(member) = guild.member(ctx, user.id).await {
            targets.push(member);
        }
    }
    let author = guild.member(ctx, message.author.id).await.ok();
    let mut extras = ai::Extras { chat, moderation: None };
    if let Some(author) = &author {
        let note = moderation::context_note(ctx, guild, author, &question, &targets).await;
        extras.moderation = Some((Requester { ctx, guild, member: author }, note));
    }
    let name = message_author_name(message);
    let answer = match ai::ask(data, &question, &name, message.channel_id.get(), images, extras).await {
        Ok(answer) => answer,
        Err(error) => {
            report_error(&ctx.http, data, Some(guild), "ИИ (ответ на сообщение)", &error.to_string()).await;
            Some(ai::error_text(&error).to_string())
        }
    };
    typing.stop();
    let Some(answer) = answer else {
        // ИИ сама решила, что обращение грубое, — не отвечаем
        ai::punish_rudeness(ctx, data, Some(guild), &message.author, message.channel_id).await;
        return;
    };
    let _ = data.db.reset_rude_strikes(guild.get(), message.author.id.get()); // нормальное обращение — счётчик с нуля
    for (index, part) in ai::split_message(&answer, 1900).into_iter().enumerate() {
        let reply = if index == 0 { no_pings(&part).reference_message(message) } else { no_pings(&part) };
        if message.channel_id.send_message(ctx, reply).await.is_err() {
            break;
        }
    }
}

/// Сообщение от другого бота: опыт и автоответы ботам не положены, а на прямое обращение — ответ ИИ.
/// Не больше bot_reply_limit раз подряд в канале — иначе два бота с ИИ переписывались бы бесконечно.
async fn bot_message(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, message: &serenity::Message) {
    let bot = ctx.cache.current_user().id;
    if !(data.chat.settings.reply_to_bots && data.ai.ready() && is_addressed_to_bot(bot, message)) {
        return;
    }
    {
        let mut streaks = locked(&data.chat.bot_reply_streak);
        let streak = streaks.entry(message.channel_id.get()).or_default();
        if *streak >= data.chat.settings.bot_reply_limit {
            println!("Не отвечаю боту {}: лимит ответов ботам подряд в канале {}", message.author.id, message.channel_id);
            return;
        }
        *streak += 1;
    }
    ai_reply(ctx, data, guild, message).await;
}

/// Каждое новое сообщение в канале, который видит бот.
pub async fn on_message(ctx: &serenity::Context, data: &Data, message: &serenity::Message) {
    let Some(guild) = message.guild_id else { return }; // личные сообщения пропускаем
    let bot = ctx.cache.current_user().id;
    if message.author.id == bot {
        return;
    }
    if message.author.bot {
        bot_message(ctx, data, guild, message).await;
        return;
    }
    locked(&data.chat.bot_reply_streak).remove(&message.channel_id.get()); // написал живой человек — счётчик с нуля
    remember_names(data, guild.get(), message);
    locked(&data.chat.last_activity).insert(guild.get(), (message.channel_id.get(), db::now())); // здесь общаются
    if check_spam(ctx, data, guild, message).await {
        return; // спам — сообщения удалены, человеку выдан варн
    }
    if data.ai.ready() && is_addressed_to_bot(bot, message) {
        ai_reply(ctx, data, guild, message).await;
    } else {
        auto_reply(ctx, data, guild, message).await;
    }
    if let Err(error) = crate::levels::message_xp(ctx, data, message).await {
        println!("Опыт за сообщение: {error}");
    }
}

// ---------- Случайные реплики ----------

/// Придумывает реплику по теме последних сообщений канала. None — если писать не стоит.
async fn chatter_text(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, channel: serenity::ChannelId) -> Result<Option<String>, crate::Error> {
    let history = channel.messages(ctx, serenity::GetMessages::new().limit(CHATTER_HISTORY)).await?;
    let bot = ctx.cache.current_user().id;
    let mut lines = Vec::new();
    for old in &history {
        if lines.is_empty() && old.author.id == bot {
            return Ok(None); // последнее сообщение в канале — наше: не разговариваем сами с собой
        }
        let text: String = old.content_safe(&ctx.cache).chars().take(300).collect();
        if !text.is_empty() {
            let name = if old.author.id == bot { "ты (бот)".to_string() } else { author_name(ctx, guild, &old.author) };
            lines.push(format!("{name}: {text}"));
        }
    }
    if lines.len() < 3 {
        return Ok(None); // слишком мало сообщений — тему не понять
    }
    lines.reverse();
    let task = format!("{CHATTER_TASK}{}", lines.join("\n"));
    let answer = ai::generate(data, &ai::system_prompt(data), &[ai::Turn::user(task)]).await?;
    if answer.text.is_empty() || ai::has_ignore_tag(&answer.text) {
        return Ok(None);
    }
    let text = ai::without_tags(&answer.text); // сама по себе никого не наказывает
    Ok(Some(if data.ai.settings.one_line { ai::to_one_line(&text) } else { text }).filter(|text| !text.is_empty()))
}

/// Раз в interval_minutes бот с шансом chance_percent сам пишет что-нибудь по теме разговора —
/// в тот канал сервера, где недавно общались люди.
pub fn start_chatter_loop(ctx: serenity::Context, data: Shared) {
    tokio::spawn(async move {
        let interval = data.chat.settings.chatter_interval * 60;
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(interval));
        timer.tick().await; // первый «тик» срабатывает сразу — пропускаем его
        loop {
            timer.tick().await;
            if !data.ai.ready() || data.chat.settings.chatter_chance <= 0 {
                continue;
            }
            let now = db::now();
            let active: Vec<(u64, u64)> = locked(&data.chat.last_activity)
                .iter()
                .filter(|(_, (_, at))| now - at <= interval as f64) // за последний интервал никто не писал — чат «спит»
                .map(|(guild, (channel, _))| (*guild, *channel))
                .collect();
            for (guild, channel) in active {
                if rand::rng().random_range(0..100) >= data.chat.settings.chatter_chance {
                    continue; // бросаем «кубик»
                }
                let (guild, channel) = (serenity::GuildId::new(guild), serenity::ChannelId::new(channel));
                match chatter_text(&ctx, &data, guild, channel).await {
                    Ok(Some(text)) => {
                        let text: String = text.chars().take(1900).collect();
                        if channel.send_message(&ctx, no_pings(&text)).await.is_ok() {
                            println!("Случайная реплика: сервер={guild} канал={channel}");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => report_error(&ctx.http, &data, Some(guild), "Случайная реплика в чат", &error.to_string()).await,
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ChatSettings {
        ChatSettings::load(&crate::config::Settings::default())
    }

    #[test]
    fn settings_like_python() {
        let chat = settings();
        assert_eq!(chat.bot_names, ["малфоид", "malfoid", "maldoid", "hallaan", "халлаан"]);
        assert!(chat.reply_to_bots && chat.antispam);
        assert_eq!((chat.bot_reply_limit, chat.chatter_chance, chat.chatter_interval), (3, 5, 20));
        assert_eq!((chat.spam_repeats, chat.spam_links, chat.spam_window), (4, 4, 20.0));
        let chat = ChatSettings::load(&crate::config::Settings::parse("[antispam]\nsame_messages = 1\nwindow_seconds = 1\n[chatter]\nchance_percent = 500\n"));
        assert_eq!((chat.spam_repeats, chat.spam_window, chat.chatter_chance), (2, 5.0, 100));
    }

    #[test]
    fn antispam() {
        let chat = settings();
        let mut tracker = SpamTracker::default();
        let key = (1, 2);
        for i in 0..3 {
            assert_eq!(tracker.check(&chat, key, i as f64, "Купи  СЛОНА", (9, i)), None);
        }
        let spam = tracker.check(&chat, key, 3.0, "купи слона", (9, 3)).unwrap();
        assert_eq!((spam.messages.len(), spam.reason, spam.punish), (4, "спам одинаковыми сообщениями", true));
        // ещё раз в ту же минуту — чистим, но второй варн не даём
        for i in 4..7 {
            tracker.check(&chat, key, i as f64, "купи слона", (9, i));
        }
        assert!(!tracker.check(&chat, key, 7.0, "купи слона", (9, 7)).unwrap().punish);
        // старые сообщения за окном не считаются
        let mut tracker = SpamTracker::default();
        for i in 0..3 {
            tracker.check(&chat, key, i as f64 * 30.0, "одно и то же", (9, i));
        }
        assert_eq!(tracker.check(&chat, key, 90.0, "одно и то же", (9, 3)), None);
        // ссылки — разные сообщения, но со ссылками
        let mut tracker = SpamTracker::default();
        let links = ["https://a.ru", "заходи discord.gg/xyz", "HTTP://b.com", "смотри https://c.org"];
        let results: Vec<bool> = links.iter().enumerate().map(|(i, text)| tracker.check(&chat, key, i as f64, text, (9, i as u64)).is_some()).collect();
        assert_eq!(results, [false, false, false, true]);
        assert!(!has_link("просто текст"));
    }

    #[test]
    fn auto_replies() {
        let rules = parse_auto_replies("# пояснение\n~привет, ку = привет милый | хай || иди отсюда\nдот* = иди со своей дотой\nсаня = 200\nбез ответа =\nмусор\n");
        assert_eq!(rules.len(), 3);
        assert!(rules[0].greeting && !rules[1].greeting);
        assert_eq!(rules[0].options, ["привет милый", "хай"]);
        assert_eq!(rules[0].rude_options.as_deref(), Some(&["иди отсюда".to_string()][..]));
        assert_eq!(rules[2].rude_options, None);
        let check = |rule: &Rule, text: &str| rule.matches(&normalize(text).chars().collect::<Vec<_>>());
        assert!(check(&rules[0], "Привет всем!"));
        assert!(!check(&rules[0], "приветствую")); // граница слова
        assert!(check(&rules[0], "ну ку"));
        assert!(check(&rules[1], "опять дотой занят"));
        assert!(check(&rules[1], "дот"));
        assert!(!check(&rules[1], "анекдот"));
        assert!(check(&rules[2], "где САНЯ?"));
    }

    #[test]
    fn greetings() {
        let rules = parse_auto_replies("~привет = привет милый\nсаня, саню = 200\n");
        let names: Vec<String> = vec!["малфоид".into(), "мой бот".into()];
        let bot_words: HashSet<String> = ["малфоид".to_string(), "hallaan".to_string()].into();
        let others: HashSet<String> = ["кирилл".to_string(), "вова".to_string()].into();
        let check = |text: &str, mentions: bool, reply: bool| {
            let text = normalize(text);
            greeting_for_someone_else(&Greeting { text: &text, bot_names: &names, bot_words: bot_words.clone(),
                                                  mentions_others: mentions, replies_to_other: reply, other_names: others.clone() }, &rules)
        };
        assert!(!check("привет", false, false)); // просто привет — боту
        assert!(!check("привет малфоиду", true, false)); // по имени бота — боту, даже с упоминанием
        assert!(!check("привет, мой бот", false, false)); // имя из нескольких слов
        assert!(check("привет саня", false, false)); // имя из другого автоответа
        assert!(check("привет кириллу", false, false)); // имя участника с окончанием
        assert!(check("привет вове", false, false));
        assert!(check("привет", true, false)); // «привет @кто-то»
        assert!(check("привет", false, true)); // в ответ на чужое сообщение
        assert_eq!(name_words("Kirill_228 (Кирюха) Ёж"), ["kirill".to_string(), "228".to_string(), "кирюха".to_string()].into());
        assert!(!word_is_name("кириллович", &others)); // слишком длинное окончание
    }
}
