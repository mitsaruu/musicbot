//! Discord-бот на Rust — переписан с bot.py: те же команды, те же config/ и levels.db.
//! Работает с теми же файлами, что и bot.py: config/settings.ini и levels.db в папке бота.
//!
//! Переменные окружения:
//!   DISCORD_TOKEN   — токен бота (обязательно);
//!   TEST_GUILD_ID   — id сервера: команды регистрируются только на нём (сразу и не трогая команды bot.py);
//!   REGISTER_GLOBAL — 1: зарегистрировать команды глобально. ВНИМАНИЕ: это заменит список команд bot.py,
//!                     если бот запущен с тем же токеном;
//!   BOT_DIR         — папка с config/ и levels.db (по умолчанию — текущая или папка выше);
//!   YTDLP           — как запускать yt-dlp (по умолчанию «yt-dlp»; на сервере — «venv/bin/python -m yt_dlp»).

mod admin;
mod ai;
mod ai_commands;
mod blackjack;
mod cards;
mod chat;
mod config;
mod daily;
mod db;
mod draw;
mod economy;
mod fun;
mod levels;
mod miniapp;
mod moderation;
mod patchnotes;
mod pets;
mod polls;
mod reminders;
mod reply;
mod shop;
#[cfg(feature = "voice")]
mod music;
#[cfg(feature = "voice")]
mod spotify;
mod text;
mod weather;
mod welcome;
mod wordboard;
mod wordstats;
mod words;

use poise::serenity_prelude as serenity;
#[cfg(feature = "voice")]
use songbird::SerenityInit;
use std::path::PathBuf;

/// Общие данные бота — доступны из каждой команды через ctx.data().
pub struct Data {
    pub db: std::sync::Arc<db::Database>,
    pub economy: economy::EconomySettings,
    pub pets: pets::PetSettings,
    pub levels: levels::LevelSettings,
    pub growth: fun::GrowthSettings,
    /// ИИ: настройки, ключ Gemini и счётчики лимитов
    pub ai: ai::AiState,
    pub quiz: ai_commands::QuizSettings,
    /// Бот в чате: автоответы, анти-спам, ответы ботам, случайные реплики
    pub chat: chat::ChatState,
    /// Варн номер N → тайм-аут (settings.ini → [moderation] warn_punishments)
    pub warn_punishments: Vec<(i64, i64)>,
    /// Приходит ли событие «зашёл новый участник» (settings.ini → [welcome] members_intent)
    pub members_intent: bool,
    /// Каналы, где викторина идёт прямо сейчас
    pub quiz_running: std::sync::Mutex<std::collections::HashSet<u64>>,
    /// Сервер → последние вопросы викторины (чтобы ИИ не повторялся)
    pub quiz_recent: std::sync::Mutex<std::collections::HashMap<u64, Vec<String>>>,
    /// Команды, которые работают только в канале для игр (если его выбрали)
    pub games_commands: std::collections::BTreeSet<String>,
    /// Игры в слова: настройки и словари
    pub words: words::WordsState,
    /// Картинки игр в слова в чате
    pub boards: wordboard::BoardState,
    /// Настроено ли мини-приложение (есть DISCORD_CLIENT_SECRET и порт не 0)
    pub miniapp_ready: bool,
    /// Папка бота: config/, levels.db, фоны картинок и шрифты
    pub dir: PathBuf,
    /// Для скачивания аватарок
    pub web: reqwest::Client,
    /// Кто сейчас играет в блэкджек: нельзя начать вторую партию, не закончив первую
    pub blackjack_players: std::sync::Mutex<std::collections::HashSet<u64>>,
    /// Башмак блэкджека на каждом сервере: id сервера → оставшиеся карты
    pub blackjack_shoes: std::sync::Mutex<std::collections::HashMap<u64, Vec<blackjack::Card>>>,
    #[cfg_attr(not(feature = "voice"), allow(dead_code))]
    pub ytdlp: String,
    /// Музыка: что играет на каждом сервере (очередь — в базе)
    #[cfg(feature = "voice")]
    pub music: music::MusicState,
}

impl Data {
    /// Собирает данные бота из базы и настроек. dir — папка бота.
    pub fn new(database: db::Database, settings: &config::Settings, dir: PathBuf, miniapp_ready: bool) -> Self {
        Self {
            db: std::sync::Arc::new(database),
            economy: economy::EconomySettings::load(settings),
            pets: pets::PetSettings::load(settings),
            levels: levels::LevelSettings::load(settings),
            growth: fun::GrowthSettings::load(settings),
            ai: ai::AiState::new(settings, std::env::var("GEMINI_API_KEY").ok()),
            quiz: ai_commands::QuizSettings::load(settings),
            chat: chat::ChatState::new(settings),
            warn_punishments: moderation::parse_warn_punishments(&settings.text("moderation", "warn_punishments", "3: 60, 5: 1440, 7: 10080")),
            members_intent: welcome::members_intent(settings),
            quiz_running: Default::default(),
            quiz_recent: Default::default(),
            games_commands: admin::parse_commands(&settings.text("channels", "games_commands", admin::DEFAULT_GAMES_COMMANDS)),
            words: words::WordsState::new(words::WordSettings::load(settings)),
            boards: Default::default(),
            miniapp_ready,
            dir,
            web: reqwest::Client::new(),
            blackjack_players: Default::default(),
            blackjack_shoes: Default::default(),
            ytdlp: std::env::var("YTDLP").unwrap_or_else(|_| "yt-dlp".to_string()),
            #[cfg(feature = "voice")]
            music: Default::default(),
        }
    }
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
/// Данные бота общие для команд и фоновых задач, поэтому лежат в Arc
pub type Shared = std::sync::Arc<Data>;
pub type Context<'a> = poise::Context<'a, Shared, Error>;

/// Папка бота: BOT_DIR, иначе текущая (если в ней есть config/), иначе папка выше (запуск из rust/).
fn bot_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BOT_DIR") {
        return PathBuf::from(dir);
    }
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if here.join("config").is_dir() {
        here
    } else {
        here.parent().map(PathBuf::from).unwrap_or(here)
    }
}

/// Ошибка внутри команды: пишем в консоль и коротко отвечаем человеку.
async fn on_error(error: poise::FrameworkError<'_, Shared, Error>) {
    match error {
        poise::FrameworkError::Command { error, ctx, .. } => {
            println!("Ошибка в команде /{}: {error:?}", ctx.command().name);
            let place = format!("команда /{}", ctx.command().name);
            admin::report_error(ctx.http(), ctx.data(), ctx.guild_id(), &place, &format!("{error:?}")).await;
            let reply = poise::CreateReply::default().content("😵 Что-то пошло не так. Ошибка записана в консоль.").ephemeral(true);
            let _ = ctx.send(reply).await;
        }
        other => {
            if let Err(error) = poise::builtins::on_error(other).await {
                println!("Ошибка при обработке ошибки: {error}");
            }
        }
    }
}

/// Раз в 20 секунд: напоминания, у которых пришло время, и опросы, которые пора закрыть.
fn start_fast_loop(http: std::sync::Arc<serenity::Http>, data: Shared) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(20));
        loop {
            timer.tick().await;
            if let Err(error) = reminders::send_due(&http, &data).await {
                println!("Напоминания: {error}");
            }
            if let Err(error) = polls::close_finished(&http, &data).await {
                println!("Опросы: {error}");
            }
        }
    });
}

#[tokio::main]
async fn main() {
    let Ok(token) = std::env::var("DISCORD_TOKEN") else {
        println!("Нет токена: задай переменную окружения DISCORD_TOKEN.");
        std::process::exit(1);
    };
    let dir = bot_dir();
    let settings = config::Settings::load(&dir.join("config").join("settings.ini"));
    let database = match db::Database::open(&dir.join("levels.db")) {
        Ok(database) => database,
        Err(error) => {
            println!("Не получилось открыть базу {}: {error}", dir.join("levels.db").display());
            std::process::exit(1);
        }
    };
    // Мини-приложение: секрет приложения из Developer Portal → OAuth2 и адрес веб-сервера из settings.ini
    let client_secret = std::env::var("DISCORD_CLIENT_SECRET").ok().filter(|secret| !secret.is_empty());
    let miniapp_host = settings.text("miniapp", "host", "127.0.0.1");
    let miniapp_port = settings.int("miniapp", "port", 8080).clamp(0, 65535) as u16;
    let data: Shared = std::sync::Arc::new(Data::new(database, &settings, dir.clone(), client_secret.is_some() && miniapp_port > 0));
    let test_guild = std::env::var("TEST_GUILD_ID").ok().and_then(|id| id.parse::<u64>().ok());
    let register_global = std::env::var("REGISTER_GLOBAL").is_ok_and(|value| value == "1");

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![
                economy::balance(),
                economy::work(),
                economy::daily(),
                economy::give_money(),
                economy::coinflip(),
                economy::slots(),
                blackjack::blackjack(),
                pets::open_case(),
                pets::cases(),
                pets::show_pets(),
                pets::pets_collect(),
                pets::pet_feed(),
                pets::pet_sell(),
                pets::pet_battle(),
                pets::pet_trade(),
                shop::shop(),
                shop::shop_buy(),
                shop::shop_add(),
                shop::shop_remove(),
                levels::rank(),
                levels::leaderboard(),
                levels::money_leaderboard(),
                wordboard::wort(),
                wordboard::slovo(),
                wordstats::wordstats(),
                wordstats::wordtop(),
                wordstats::wordduel(),
                fun::dice(),
                fun::roll(),
                fun::predskazanie(),
                fun::shar(),
                fun::dick_growth(),
                fun::dick_size(),
                fun::dick_leaderboard(),
                reminders::remind(),
                reminders::reminders(),
                polls::poll(),
                patchnotes::show(),
                patchnotes::set_channel(),
                admin::set_games_channel(),
                admin::set_log_channel(),
                admin::show_warns(),
                admin::warns_clear(),
                weather::weather(),
                weather::weather_cities(),
                ai_commands::ask(),
                ai_commands::forget(),
                ai_commands::ai_limits(),
                ai_commands::summary(),
                ai_commands::quiz(),
                welcome::set_welcome_channel(),
                daily::daily_setup(),
                daily::daily_off(),
                daily::daily_test(),
                #[cfg(feature = "voice")]
                music::play(),
                #[cfg(feature = "voice")]
                music::skip(),
                #[cfg(feature = "voice")]
                music::pause(),
                #[cfg(feature = "voice")]
                music::resume(),
                #[cfg(feature = "voice")]
                music::show_queue(),
                #[cfg(feature = "voice")]
                music::remove_all(),
                #[cfg(feature = "voice")]
                music::setvolume(),
                #[cfg(feature = "voice")]
                music::leave(),
                #[cfg(feature = "voice")]
                music::autoplay(),
                #[cfg(feature = "voice")]
                music::playlist_save(),
                #[cfg(feature = "voice")]
                music::playlist_play(),
                #[cfg(feature = "voice")]
                music::show_playlists(),
                #[cfg(feature = "voice")]
                music::playlist_delete(),
                #[cfg(feature = "voice")]
                music::lyrics(),
                #[cfg(feature = "voice")]
                music::top_chart_month(),
            ],
            on_error: |error| Box::pin(on_error(error)),
            // Перед каждой командой: игры и рейтинги — только в канале для игр, если его выбрали
            command_check: Some(|ctx| Box::pin(admin::games_channel_check(ctx))),
            event_handler: |framework, event| {
                Box::pin(async move {
                    let (ctx, data) = (framework.serenity_context, framework.user_data);
                    // Кнопка «Играть!» под картинкой игры в слова
                    if let serenity::FullEvent::InteractionCreate { interaction: serenity::Interaction::Component(press) } = event {
                        // …и кнопки вариантов в опросах
                        #[cfg(feature = "voice")]
                        if music::button_pressed(ctx, data, press).await {
                            return Ok(()); // кнопки плеера: пауза, дальше, повтор, перемешать
                        }
                        if !wordboard::play_pressed(ctx, data, press).await {
                            polls::vote_pressed(ctx, data, press).await;
                        }
                    }
                    // Сообщения: анти-спам, ответы ИИ, автоответы и опыт
                    if let serenity::FullEvent::Message { new_message } = event {
                        chat::on_message(ctx, data, new_message).await;
                    }
                    // Новый участник на сервере — приветствие
                    if let serenity::FullEvent::GuildMemberAddition { new_member } = event {
                        welcome::on_member_join(ctx, data, new_member).await;
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                let commands = &framework.options().commands;
                if let Some(guild) = test_guild {
                    poise::builtins::register_in_guild(ctx, commands, serenity::GuildId::new(guild)).await?;
                    println!("Зарегистрировано слэш-команд на сервере {guild}: {}", commands.len());
                } else if register_global {
                    poise::builtins::register_globally(ctx, commands).await?;
                    println!("Зарегистрировано слэш-команд глобально: {}", commands.len());
                } else {
                    println!("Команды не регистрировал: задай TEST_GUILD_ID (сервер для проверки) или REGISTER_GLOBAL=1.");
                }
                shop::start_expiry_loop(ctx.http.clone(), data.db.clone()); // снимать роли, срок которых вышел
                levels::start_voice_xp_loop(ctx.clone(), data.clone()); // опыт за голосовые каналы, раз в минуту
                wordstats::start_duel_loop(ctx.http.clone(), data.clone()); // итоги дуэлей в словах
                start_fast_loop(ctx.http.clone(), data.clone()); // напоминания и опросы
                daily::start_loop(ctx.http.clone(), data.clone()); // утренние и вечерние сообщения
                chat::start_chatter_loop(ctx.clone(), data.clone()); // случайные реплики по теме чата
                if let Err(error) = patchnotes::post_new(&ctx.http, &data).await {
                    println!("Патчноуты: {error}");
                }
                match client_secret {
                    Some(secret) if miniapp_port > 0 => miniapp::start(ctx.clone(), data.clone(), secret, miniapp_host, miniapp_port),
                    _ => println!("Мини-приложение выключено: нет DISCORD_CLIENT_SECRET (или port = 0). /wort и /slovo не будут работать."),
                }
                println!("Бот запущен как {}", ready.user.name);
                Ok(data)
            })
        })
        .build();

    // Какие события бот получает от Discord: серверы, голосовые каналы (для музыки) и сообщения.
    // MESSAGE_CONTENT — текст сообщений (для ИИ, автоответов и анти-спама): включается в Developer Portal → Bot.
    let mut intents = serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::GUILD_VOICE_STATES
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT;
    if welcome::members_intent(&settings) {
        intents |= serenity::GatewayIntents::GUILD_MEMBERS; // «зашёл новый участник» — для приветствий
    }
    let builder = serenity::ClientBuilder::new(token, intents).framework(framework);
    #[cfg(feature = "voice")]
    let builder = builder.register_songbird();
    let client = builder.await;
    match client {
        Ok(mut client) => {
            if let Err(error) = client.start().await {
                println!("Бот остановился с ошибкой: {error}");
            }
        }
        Err(error) => println!("Не получилось создать клиент Discord: {error}"),
    }
}
