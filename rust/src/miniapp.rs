//! Мини-приложение (Discord Activity) для игр в слова. Discord показывает страницу miniapp/index.html внутри
//! себя; страницу и ответы на ходы отдаёт этот веб-сервер. Снаружи до него Discord достучится через туннель
//! (например, cloudflared) — см. README. Адреса и ответы те же, что у bot.py, поэтому страница не менялась.

use crate::wordboard::{self, Player};
use crate::words::{self, Lang, MAX_GUESSES};
use crate::{db, Shared};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use poise::serenity_prelude as serenity;
use rand::distr::{Alphanumeric, SampleString};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SESSION_HOURS: f64 = 12.0;

/// Кто играет: выдаётся после входа через Discord, страница присылает «пропуск» с каждым запросом.
#[derive(Clone)]
struct Session {
    guild: u64,
    user: u64,
    name: String,
    face: String, // ссылка на аватарку
    channel: Option<u64>,
    time: f64,
    lang: Lang,
}

pub struct Miniapp {
    data: Shared,
    ctx: serenity::Context,
    client_secret: String,
    sessions: Mutex<HashMap<String, Session>>,
    /// (сервер, участник) → (имя, ссылка на аватарку) — чтобы не спрашивать Discord при каждом обновлении
    members: Mutex<HashMap<(u64, u64), (String, String)>>,
}

type App = Arc<Miniapp>;
type Reply = Result<Response, Response>;

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn json_error(text: &str, status: StatusCode) -> Response {
    (status, Json(json!({ "error": text }))).into_response()
}

/// Ошибка в мини-приложении не роняет бота: пишем её в консоль и отвечаем странице.
fn failed(error: impl std::fmt::Display) -> Response {
    println!("Мини-приложение: {error}");
    json_error("Что-то пошло не так. Ошибка записана в логи.", StatusCode::INTERNAL_SERVER_ERROR)
}

impl Miniapp {
    /// Кто делает запрос: ищем пропуск из заголовка Authorization: Bearer <пропуск>.
    fn session(&self, headers: &HeaderMap) -> Result<(String, Session), Response> {
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.strip_prefix("Bearer ").unwrap_or(value).trim().to_string())
            .unwrap_or_default();
        match locked(&self.sessions).get(&token) {
            Some(session) if db::now() - session.time <= SESSION_HOURS * 3600.0 => Ok((token, session.clone())),
            _ => Err(json_error("Сессия устарела — открой игру заново.", StatusCode::UNAUTHORIZED)),
        }
    }

    fn state_of(&self, session: &Session, game: &words::Game) -> Result<Value, Response> {
        let start = self.data.economy.start_balance;
        let (balance, _) = self.data.db.wallet(session.guild, session.user, &session.name, start).map_err(failed)?;
        Ok(words::game_state(&self.data, game, balance))
    }

    /// Обновляет картинку в чате, не задерживая ответ странице.
    fn refresh_board(self: &Arc<Self>, session: &Session, lang: Lang) {
        let Some(channel) = session.channel else { return };
        let (app, player) = (self.clone(), Player { id: session.user, name: session.name.clone(), face: session.face.clone() });
        let guild = session.guild;
        tokio::spawn(async move {
            wordboard::update(&app.ctx, &app.data, guild, serenity::ChannelId::new(channel), player, lang).await;
        });
    }

    /// Имя и аватарка участника сервера: из запаса или (один раз) у Discord. None — не нашли.
    async fn member(&self, guild: u64, user: u64) -> Option<(String, String)> {
        if let Some(known) = locked(&self.members).get(&(guild, user)) {
            return Some(known.clone());
        }
        let member = serenity::GuildId::new(guild).member(&self.ctx, serenity::UserId::new(user)).await.ok()?;
        let found = (member.display_name().to_string(), member.face());
        locked(&self.members).insert((guild, user), found.clone());
        Some(found)
    }
}

async fn index(State(app): State<App>) -> Response {
    file(&app, "index.html", "text/html; charset=utf-8").await
}

/// style.css и app.js. Другие файлы не отдаём.
async fn static_file(State(app): State<App>, Path(name): Path<String>) -> Response {
    match name.as_str() {
        "style.css" => file(&app, "style.css", "text/css; charset=utf-8").await,
        "app.js" => file(&app, "app.js", "text/javascript; charset=utf-8").await,
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn file(app: &Miniapp, name: &str, content_type: &'static str) -> Response {
    match tokio::fs::read(app.data.dir.join("miniapp").join(name)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "no-cache")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn application_id(app: &Miniapp) -> String {
    app.ctx.http.application_id().map(|id| id.to_string()).unwrap_or_default()
}

async fn config(State(app): State<App>) -> Response {
    Json(json!({ "client_id": application_id(&app) })).into_response()
}

fn text_field(body: &Value, key: &str) -> String {
    match body.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// Вход: страница получила от Discord одноразовый код — меняем его на токен и узнаём, кто это.
async fn token(State(app): State<App>, Json(body): Json<Value>) -> Reply {
    let (code, guild_id) = (text_field(&body, "code"), text_field(&body, "guild_id"));
    let client_id = application_id(&app);
    let exchange = app
        .data
        .web
        .post("https://discord.com/api/v10/oauth2/token")
        .form(&[("client_id", client_id.as_str()), ("client_secret", app.client_secret.as_str()),
                ("grant_type", "authorization_code"), ("code", code.as_str())])
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(failed)?;
    if !exchange.status().is_success() {
        println!("Мини-приложение: Discord не принял код входа: {} {}", exchange.status(), exchange.text().await.unwrap_or_default());
        return Err(json_error("Discord не подтвердил вход. Проверь DISCORD_CLIENT_SECRET.", StatusCode::UNAUTHORIZED));
    }
    let tokens: Value = serde_json::from_str(&exchange.text().await.map_err(failed)?).map_err(failed)?;
    let access_token = text_field(&tokens, "access_token");
    let who = app
        .data
        .web
        .get("https://discord.com/api/v10/users/@me")
        .bearer_auth(&access_token)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(failed)?;
    if !who.status().is_success() {
        return Err(json_error("Не получилось узнать, кто ты.", StatusCode::UNAUTHORIZED));
    }
    let who: Value = serde_json::from_str(&who.text().await.map_err(failed)?).map_err(failed)?;
    let user: u64 = text_field(&who, "id").parse().map_err(failed)?;

    let guild = guild_id.parse::<u64>().ok().filter(|id| *id != 0 && app.ctx.cache.guild(serenity::GuildId::new(*id)).is_some());
    let Some(guild) = guild else {
        return Err(json_error("Открой игру на сервере, где есть бот (не в личке).", StatusCode::FORBIDDEN));
    };
    let Some((name, face)) = app.member(guild, user).await else {
        return Err(json_error("Ты не участник этого сервера.", StatusCode::FORBIDDEN));
    };
    let now = db::now();
    let lang = locked(&app.data.boards.lang_choice).get(&(guild, user)).copied().unwrap_or(Lang::De); // какую игру звал
    let session = Session {
        guild, user, name: name.clone(), face, time: now, lang,
        channel: text_field(&body, "channel_id").parse().ok().filter(|id| *id != 0),
    };
    let pass = Alphanumeric.sample_string(&mut rand::rng(), 32);
    {
        let mut sessions = locked(&app.sessions);
        sessions.retain(|_, old| now - old.time <= SESSION_HOURS * 3600.0); // старые пропуска выбрасываем
        sessions.insert(pass.clone(), session.clone());
    }
    let game = words::daily_game(&app.data, guild, user, lang).map_err(failed)?;
    if !game.finished() {
        app.refresh_board(&session, lang);
    }
    let state = app.state_of(&session, &game)?;
    Ok(Json(json!({ "access_token": access_token, "session": pass, "name": name, "state": state })).into_response())
}

async fn state(State(app): State<App>, headers: HeaderMap) -> Reply {
    let (_, session) = app.session(&headers)?;
    let game = words::daily_game(&app.data, session.guild, session.user, session.lang).map_err(failed)?;
    Ok(Json(json!({ "state": app.state_of(&session, &game)? })).into_response())
}

async fn guess(State(app): State<App>, headers: HeaderMap, Json(body): Json<Value>) -> Reply {
    let (_, session) = app.session(&headers)?;
    let word = text_field(&body, "word");
    let (game, problem) = words::make_guess(&app.data, session.guild, session.user, &session.name, &word, session.lang)
        .await
        .map_err(failed)?;
    if problem.is_none() {
        app.refresh_board(&session, game.lang); // обновляем картинку в чате
        if game.finished() {
            // слово дня доиграно — вдруг у него дуэль на это слово
            let app = app.clone();
            tokio::spawn(async move {
                if let Err(error) = crate::wordstats::resolve_duels(&app.ctx.http, &app.data).await {
                    println!("Дуэли в словах: {error}");
                }
            });
        }
    }
    Ok(Json(json!({ "state": app.state_of(&session, &game)?, "error": problem })).into_response())
}

/// Открыть слово дня; с полем lang — переключиться на другую игру прямо в мини-приложении.
async fn new_game(State(app): State<App>, headers: HeaderMap, Json(body): Json<Value>) -> Reply {
    let (pass, mut session) = app.session(&headers)?;
    let switched = Lang::from_key(&text_field(&body, "lang"));
    if let Some(lang) = switched {
        session.lang = lang;
        if let Some(stored) = locked(&app.sessions).get_mut(&pass) {
            stored.lang = lang;
        }
        locked(&app.data.boards.lang_choice).insert((session.guild, session.user), lang);
    }
    let game = words::daily_game(&app.data, session.guild, session.user, session.lang).map_err(failed)?;
    if switched.is_some() && !game.finished() {
        app.refresh_board(&session, game.lang);
    }
    Ok(Json(json!({ "state": app.state_of(&session, &game)? })).into_response())
}

/// Кто сегодня играет в слово дня этой игры на сервере: имя и цвета догадок (без букв) — для панели слева
/// сверху. Страница спрашивает раз в пару секунд, так что видно, как другие подбирают слова.
async fn players(State(app): State<App>, headers: HeaderMap) -> Reply {
    let (_, session) = app.session(&headers)?;
    let lang = session.lang;
    locked(&app.members).insert((session.guild, session.user), (session.name.clone(), session.face.clone()));
    let day = words::today(&app.data, lang);
    let rows = app.data.db.word_day_rows(lang.table(), session.guild, &day).map_err(failed)?;
    let mut players = Vec::new();
    for (user, answer, guesses) in rows {
        let Some((name, _)) = app.member(session.guild, user).await else { continue };
        let game = words::Game::from_row(lang, &day, answer, &guesses);
        let marks: Vec<Vec<&str>> = game.rows().iter().map(|row| row.iter().map(|mark| mark.as_str()).collect()).collect();
        players.push((user != session.user, json!({
            "id": user.to_string(), "name": name, "me": user == session.user, "marks": marks,
            "won": game.won(), "finished": game.won() || game.guesses.len() >= MAX_GUESSES,
        })));
    }
    players.sort_by_key(|(other, _)| *other); // сам игрок — первым
    let players: Vec<Value> = players.into_iter().take(12).map(|(_, player)| player).collect();
    Ok(Json(json!({ "players": players, "guild": session.guild.to_string() })).into_response())
}

/// Аватарка участника из Discord (картинки с чужих сайтов внутри Discord-приложения не грузятся,
/// поэтому отдаём их со своего сервера). Адрес: /api/avatar/<сервер>/<участник>.png
async fn avatar(State(app): State<App>, Path((guild, file)): Path<(u64, String)>) -> Response {
    let Some(user) = file.strip_suffix(".png").and_then(|id| id.parse::<u64>().ok()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((_, face)) = app.member(guild, user).await else { return StatusCode::NOT_FOUND.into_response() };
    match wordboard::cached_avatar(&app.data, user, &face).await {
        Some(bytes) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "max-age=3600")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn router(app: App) -> axum::Router {
    axum::Router::new()
        .route("/", get(index))
        .route("/static/{name}", get(static_file))
        .route("/api/config", get(config))
        .route("/api/token", post(token))
        .route("/api/state", get(state))
        .route("/api/guess", post(guess))
        .route("/api/new", post(new_game))
        .route("/api/players", get(players))
        .route("/api/avatar/{guild}/{file}", get(avatar))
        .with_state(app)
}

/// Запускает веб-сервер мини-приложения в фоне.
pub fn start(ctx: serenity::Context, data: Shared, client_secret: String, host: String, port: u16) {
    let app = Arc::new(Miniapp { data, ctx, client_secret, sessions: Default::default(), members: Default::default() });
    tokio::spawn(async move {
        match tokio::net::TcpListener::bind((host.as_str(), port)).await {
            Ok(listener) => {
                println!("Мини-приложение работает на http://{host}:{port} — пробрось его туннелем (см. README)");
                if let Err(error) = axum::serve(listener, router(app)).await {
                    println!("Мини-приложение остановилось: {error}");
                }
            }
            Err(error) => println!("Мини-приложение не запустилось: порт {port} занят или недоступен ({error})"),
        }
    });
}
