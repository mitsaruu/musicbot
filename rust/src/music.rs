//! Музыка: /play (YouTube, SoundCloud, Spotify), очередь в базе (переживает /leave и перезапуск), /skip, /pause,
//! /resume, /queue, /remove_all, /setvolume, /leave, кнопки под «Играет» (пауза, дальше, повтор, перемешать),
//! /autoplay, плейлисты сервера, /lyrics и /top_chart_month — как в bot.py.
//!
//! Звук, как и в bot.py, качает yt-dlp и отдаёт его боту через «трубу» (pipe): по прямой ссылке YouTube
//! на серверах отвечает ошибкой 403.

use crate::admin::report_error;
use crate::cards::{Picture, Row};
use crate::reply::whisper;
use crate::text::plural;
use crate::{db, Context, Data, Error, Shared};
use poise::serenity_prelude as serenity;
use rand::seq::{IndexedRandom, SliceRandom};
use rusqlite::{params, OptionalExtension};
use songbird::events::{Event, EventContext, TrackEvent};
use songbird::input::{ChildContainer, Input};
use songbird::tracks::{PlayMode, TrackHandle};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TRACK_TOO_SHORT: Duration = Duration::from_secs(3); // трек «закончился» быстрее — значит, звук не загрузился
const MAX_PLAYLISTS: usize = 25; // сколько плейлистов может сохранить один сервер
const MAX_PLAYLIST_SAVE: usize = 100; // сколько треков помещается в один плейлист
const NOT_PLAYING: &str = "Сейчас ничего не играет.";
const MONTH_NAMES: [&str; 12] = ["Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь"];

/// Что передать yt-dlp: ссылку — как есть, обычный текст — как поиск первого результата на YouTube.
pub fn ytdlp_target(query: &str) -> String {
    let query = query.trim();
    if query.starts_with("http://") || query.starts_with("https://") || query.starts_with("ytsearch") {
        query.to_string()
    } else {
        format!("ytsearch1:{query}")
    }
}

fn tracks_text(count: usize) -> String {
    format!("{count} {}", plural(count as i64, "трек", "трека", "треков"))
}

// ---------- Что играет на каждом сервере (после перезапуска — с нуля, очередь — в базе) ----------

#[derive(Default)]
struct GuildMusic {
    now_playing: Option<(String, String)>, // (ссылка, название) — при /leave вернётся в начало очереди
    playing_url: Option<String>,           // страница трека, который играет (для автоплея)
    track: Option<TrackHandle>,
    skipped: bool,                         // трек остановили сами (/skip, /leave) — это не ошибка звука
    repeat: bool,                          // кнопка 🔁
    volume: Option<f32>,                   // 1.0 = 100%
}

#[derive(Default)]
pub struct MusicState {
    guilds: Mutex<HashMap<u64, GuildMusic>>,
    /// «Замки»: не дают включить два трека одновременно, если play_next вызвали сразу из двух мест
    locks: Mutex<HashMap<u64, Arc<tokio::sync::Mutex<()>>>>,
    /// Сервер → ссылки, которые недавно играли (чтобы автоплей не ходил по кругу)
    autoplay_recent: Mutex<HashMap<u64, Vec<String>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MusicState {
    fn with<T>(&self, guild: u64, action: impl FnOnce(&mut GuildMusic) -> T) -> T {
        action(locked(&self.guilds).entry(guild).or_default())
    }

    fn lock_for(&self, guild: u64) -> Arc<tokio::sync::Mutex<()>> {
        locked(&self.locks).entry(guild).or_default().clone()
    }
}

/// Играет ли что-то (или стоит на паузе). Трек, который songbird уже выбросил (бота выкинули из канала), не в счёт.
async fn current_mode(data: &Data, guild: u64) -> Option<PlayMode> {
    let track = data.music.with(guild, |state| state.track.clone())?;
    match track.get_info().await {
        Ok(info) if matches!(info.playing, PlayMode::Play | PlayMode::Pause) => Some(info.playing),
        _ => {
            data.music.with(guild, |state| state.track = None);
            None
        }
    }
}

// ---------- Очередь в базе (таблица queue, та же, что у bot.py) ----------

impl db::Database {
    pub fn queue_add(&self, guild: u64, url: &str, title: &str) -> rusqlite::Result<()> {
        self.lock().execute("INSERT INTO queue (guild_id, url, title) VALUES (?1, ?2, ?3)", params![guild as i64, url, title])?;
        Ok(())
    }

    /// Трек в самое начало очереди (номер меньше, чем у всех остальных).
    pub fn queue_push_front(&self, guild: u64, url: &str, title: &str) -> rusqlite::Result<()> {
        let connection = self.lock();
        let smallest: Option<i64> = connection.query_row("SELECT MIN(id) FROM queue", [], |row| row.get(0))?;
        connection.execute(
            "INSERT INTO queue (id, guild_id, url, title) VALUES (?1, ?2, ?3, ?4)",
            params![smallest.unwrap_or(1) - 1, guild as i64, url, title],
        )?;
        Ok(())
    }

    /// Достаёт первый трек очереди и удаляет его оттуда.
    pub fn queue_pop(&self, guild: u64) -> rusqlite::Result<Option<(String, String)>> {
        let connection = self.lock();
        let row = connection
            .query_row("SELECT id, url, title FROM queue WHERE guild_id = ?1 ORDER BY id LIMIT 1", params![guild as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?.unwrap_or_default()))
            })
            .optional()?;
        let Some((id, url, title)) = row else { return Ok(None) };
        connection.execute("DELETE FROM queue WHERE id = ?1", params![id])?;
        Ok(Some((url, title)))
    }

    pub fn queue_tracks(&self, guild: u64, limit: i64) -> rusqlite::Result<Vec<(String, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT url, title FROM queue WHERE guild_id = ?1 ORDER BY id LIMIT ?2")?;
        let rows = statement.query_map(params![guild as i64, limit], |row| Ok((row.get(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
        rows.collect()
    }

    pub fn queue_count(&self, guild: u64) -> rusqlite::Result<usize> {
        self.lock().query_row("SELECT COUNT(*) FROM queue WHERE guild_id = ?1", params![guild as i64], |row| row.get::<_, i64>(0)).map(|n| n as usize)
    }

    pub fn queue_clear(&self, guild: u64) -> rusqlite::Result<usize> {
        self.lock().execute("DELETE FROM queue WHERE guild_id = ?1", params![guild as i64])
    }

    /// Перемешивает очередь: номера остаются, меняются местами сами треки. Возвращает, сколько в ней треков.
    pub fn queue_shuffle(&self, guild: u64) -> rusqlite::Result<usize> {
        let connection = self.lock();
        let rows: Vec<(i64, String, Option<String>)> = {
            let mut statement = connection.prepare("SELECT id, url, title FROM queue WHERE guild_id = ?1 ORDER BY id")?;
            let rows = statement.query_map(params![guild as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut tracks: Vec<(String, Option<String>)> = rows.iter().map(|(_, url, title)| (url.clone(), title.clone())).collect();
        tracks.shuffle(&mut rand::rng());
        for ((id, _, _), (url, title)) in rows.iter().zip(tracks) {
            connection.execute("UPDATE queue SET url = ?1, title = ?2 WHERE id = ?3", params![url, title, id])?;
        }
        Ok(rows.len())
    }

    /// Засчитывает прослушивание трека в этом месяце (прошлые месяцы стираются — топ сам обнуляется 1-го числа).
    pub fn record_play(&self, guild: u64, month: &str, title: &str) -> rusqlite::Result<()> {
        let connection = self.lock();
        connection.execute("DELETE FROM plays WHERE month != ?1", params![month])?;
        connection.execute(
            "INSERT INTO plays (guild_id, month, title, count) VALUES (?1, ?2, ?3, 1)
             ON CONFLICT (guild_id, month, title) DO UPDATE SET count = count + 1",
            params![guild as i64, month, title],
        )?;
        Ok(())
    }

    pub fn top_tracks(&self, guild: u64, month: &str) -> rusqlite::Result<Vec<(String, i64)>> {
        let connection = self.lock();
        connection.execute("DELETE FROM plays WHERE month != ?1", params![month])?;
        let mut statement = connection.prepare("SELECT title, count FROM plays WHERE guild_id = ?1 AND month = ?2 ORDER BY count DESC, title LIMIT 10")?;
        let rows = statement.query_map(params![guild as i64, month], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn autoplay(&self, guild: u64) -> rusqlite::Result<bool> {
        let value = self
            .lock()
            .query_row("SELECT autoplay FROM music_settings WHERE guild_id = ?1", params![guild as i64], |row| row.get::<_, Option<i64>>(0))
            .optional()?;
        Ok(value.flatten().unwrap_or(0) != 0)
    }

    pub fn set_autoplay(&self, guild: u64, enabled: bool) -> rusqlite::Result<()> {
        self.lock().execute("INSERT OR REPLACE INTO music_settings (guild_id, autoplay) VALUES (?1, ?2)", params![guild as i64, enabled as i64])?;
        Ok(())
    }

    /// Плейлисты сервера: (название, сколько треков), по алфавиту.
    pub fn playlists(&self, guild: u64) -> rusqlite::Result<Vec<(String, i64)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT name, COUNT(*) FROM playlists WHERE guild_id = ?1 GROUP BY name ORDER BY name")?;
        let rows = statement.query_map(params![guild as i64], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn playlist(&self, guild: u64, name: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT url, title FROM playlists WHERE guild_id = ?1 AND name = ?2 ORDER BY position")?;
        let rows = statement.query_map(params![guild as i64, name], |row| Ok((row.get(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
        rows.collect()
    }

    /// Сохраняет плейлист (то же имя — перезаписывает).
    pub fn save_playlist(&self, guild: u64, name: &str, tracks: &[(String, String)]) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM playlists WHERE guild_id = ?1 AND name = ?2", params![guild as i64, name])?;
        for (position, (url, title)) in tracks.iter().enumerate() {
            transaction.execute(
                "INSERT INTO playlists (guild_id, name, position, url, title) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, name, position as i64, url, title],
            )?;
        }
        transaction.commit()
    }

    pub fn delete_playlist(&self, guild: u64, name: &str) -> rusqlite::Result<bool> {
        Ok(self.lock().execute("DELETE FROM playlists WHERE guild_id = ?1 AND name = ?2", params![guild as i64, name])? > 0)
    }
}

// ---------- yt-dlp ----------

/// Команда yt-dlp («yt-dlp» или «python -m yt_dlp» из переменной YTDLP).
fn ytdlp_command(data: &Data) -> tokio::process::Command {
    let mut parts = data.ytdlp.split_whitespace();
    let mut command = tokio::process::Command::new(parts.next().unwrap_or("yt-dlp"));
    command.args(parts).stdin(Stdio::null()).kill_on_drop(true);
    command
}

/// Ищет трек. Возвращает (название, ссылка на страницу трека) — звук по ней качается заново перед включением.
async fn search(data: &Data, query: &str) -> Result<(String, String), Error> {
    let mut command = ytdlp_command(data);
    command.args(["--no-playlist", "--no-warnings", "--format", "bestaudio/best", "--print", "title", "--print", "webpage_url"]).arg(ytdlp_target(query));
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .map_err(|_| "yt-dlp думал дольше минуты")?
        .map_err(|error| format!("не получилось запустить yt-dlp (`{}`): {error}. Задай переменную YTDLP", data.ytdlp))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    match (lines.next(), lines.next()) {
        (Some(title), Some(url)) if output.status.success() => Ok((title.to_string(), url.to_string())),
        _ => Err(format!("yt-dlp ничего не нашёл: {}", String::from_utf8_lossy(&output.stderr).trim()).into()),
    }
}

/// id видео YouTube из ссылки (v=…, youtu.be/…, /shorts/…).
pub fn youtube_id(url: &str) -> Option<String> {
    ["v=", "youtu.be/", "/shorts/"].iter().find_map(|marker| {
        let start = url.find(marker)? + marker.len();
        let id: String = url[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(11).collect();
        (id.chars().count() == 11).then_some(id)
    })
}

/// Строки yt-dlp «id, название, id, название…» → треки без самого видео.
pub fn parse_mix(output: &str, video: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = output.lines().map(str::trim).collect();
    lines
        .chunks(2)
        .filter(|pair| pair.len() == 2 && !pair[0].is_empty() && pair[0] != video)
        .map(|pair| (format!("https://www.youtube.com/watch?v={}", pair[0]), if pair[1].is_empty() || pair[1] == "NA" { "Без названия".into() } else { pair[1].to_string() }))
        .collect()
}

/// Похожие треки с YouTube: «микс» (автоматический плейлист YouTube) для этого видео.
async fn similar_tracks(data: &Data, url: &str) -> Vec<(String, String)> {
    let Some(video) = youtube_id(url) else { return Vec::new() };
    let mut command = ytdlp_command(data);
    command
        .args(["--flat-playlist", "--yes-playlist", "--playlist-end", "15", "--no-warnings", "--print", "id", "--print", "title"])
        .arg(format!("https://www.youtube.com/watch?v={video}&list=RD{video}"));
    match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(Ok(output)) => parse_mix(&String::from_utf8_lossy(&output.stdout), &video),
        other => {
            println!("Автоплей: не получилось найти похожие треки: {other:?}");
            Vec::new()
        }
    }
}

/// Убирает длинные ссылки из сообщений yt-dlp.
pub fn hide_links(text: &str) -> String {
    text.split(' ')
        .map(|word| if word.starts_with("http://") || word.starts_with("https://") { "<ссылка>" } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Главная причина ошибки: строка yt-dlp «ERROR: …», иначе последняя строка.
pub fn failure_reason(details: &str, fallback: &str) -> String {
    let lines: Vec<&str> = details.lines().filter(|line| !line.trim().is_empty()).collect();
    let reason = lines.iter().rev().find(|line| line.starts_with("ERROR:")).or(lines.last()).copied().unwrap_or(fallback);
    reason.chars().take(300).collect()
}

// ---------- Включение треков ----------

fn music_buttons() -> Vec<serenity::CreateActionRow> {
    let button = |id: &str, emoji: char| serenity::CreateButton::new(id).emoji(emoji).style(serenity::ButtonStyle::Secondary);
    vec![serenity::CreateActionRow::Buttons(vec![
        button("music:pause", '⏯'),
        button("music:skip", '⏭'),
        button("music:repeat", '🔁'),
        button("music:shuffle", '🔀'),
    ])]
}

async fn say_in(ctx: &serenity::Context, channel: serenity::ChannelId, text: &str) {
    let message = serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new());
    let _ = channel.send_message(ctx, message).await;
}

/// В каком голосовом канале бот на этом сервере (None — не в канале).
async fn bot_channel(ctx: &serenity::Context, guild: serenity::GuildId) -> Option<songbird::id::ChannelId> {
    let call = songbird::get(ctx).await?.get(guild)?;
    let channel = call.lock().await.current_channel();
    channel
}

/// Голосовой канал участника.
fn user_channel(ctx: &serenity::Context, guild: serenity::GuildId, user: serenity::UserId) -> Option<serenity::ChannelId> {
    ctx.cache.guild(guild)?.voice_states.get(&user)?.channel_id
}

/// Заходит в голосовой канал (если бот ещё не там). Текст ошибки — если не получилось.
async fn join(ctx: &serenity::Context, guild: serenity::GuildId, channel: serenity::ChannelId) -> Result<(), String> {
    if bot_channel(ctx, guild).await.is_some() {
        return Ok(());
    }
    let manager = songbird::get(ctx).await.ok_or("голосовой модуль не запущен")?;
    manager.join(guild, channel).await.map(|_| ()).map_err(|error| error.to_string())
}

/// Обработчик «трек закончился» (или сломался).
struct TrackEnd {
    ctx: serenity::Context,
    data: Shared,
    guild: serenity::GuildId,
    channel: serenity::ChannelId,
    title: String,
    log: Arc<Mutex<String>>,                    // что написал yt-dlp
    done: Arc<std::sync::atomic::AtomicBool>,   // End и Error могут прийти оба — обрабатываем один раз
}

#[async_trait::async_trait]
impl songbird::EventHandler for TrackEnd {
    async fn act(&self, event: &EventContext<'_>) -> Option<Event> {
        if self.done.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let (played, error) = match event {
            EventContext::Track(tracks) => tracks.first().map_or((Duration::ZERO, None), |(state, _)| {
                let error = match &state.playing {
                    PlayMode::Errored(error) => Some(error.to_string()),
                    _ => None,
                };
                (state.play_time, error)
            }),
            _ => (Duration::ZERO, None),
        };
        let (ctx, data, guild, channel, title, log) = (self.ctx.clone(), self.data.clone(), self.guild, self.channel, self.title.clone(), self.log.clone());
        tokio::spawn(async move {
            // yt-dlp мог ещё дописывать сообщение об ошибке — даём ему мгновение
            tokio::time::sleep(Duration::from_millis(300)).await;
            let details = locked(&log).clone();
            on_track_end(&ctx, &data, guild, channel, &title, played, error, &details).await;
        });
        None
    }
}

/// Трек закончился (или его пропустили). Если он оборвался в первые секунды не из-за /skip — пишем причину.
#[allow(clippy::too_many_arguments)]
async fn on_track_end(ctx: &serenity::Context, data: &Shared, guild: serenity::GuildId, channel: serenity::ChannelId, title: &str,
                      played: Duration, error: Option<String>, details: &str) {
    let (now_playing, playing_url, skipped, repeat) = data.music.with(guild.get(), |state| {
        state.track = None;
        (state.now_playing.take(), state.playing_url.take(), std::mem::take(&mut state.skipped), state.repeat)
    });
    let mut failed = false;
    if !skipped && (error.is_some() || played < TRACK_TOO_SHORT) {
        failed = true;
        let details = hide_links(details);
        let reason = failure_reason(&details, error.as_deref().unwrap_or("yt-dlp ничего не написал"));
        println!("Трек «{title}» не проигрался (звука: {:.1} с). yt-dlp:\n{details}", played.as_secs_f32());
        let report = format!("{title}: {}\n{}", error.as_deref().unwrap_or(""), details.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect::<String>());
        report_error(&ctx.http, data, Some(guild), "Музыка", &report).await;
        say_in(ctx, channel, &format!("⚠️ **{title}** не проигрался: `{reason}`\nОбычно помогает обновить yt-dlp на сервере: `pip install -U yt-dlp`.")).await;
    }
    if let (Some((url, saved_title)), false, false, true) = (&now_playing, skipped, failed, repeat) {
        let _ = data.db.queue_push_front(guild.get(), url, saved_title); // повтор — тот же трек снова первым
    } else if let (Some(url), false) = (&playing_url, failed) {
        if data.db.queue_count(guild.get()).unwrap_or(1) == 0 {
            autoplay_add(ctx, data, guild, channel, url).await; // очередь кончилась — похожий трек (если включено)
        }
    }
    play_next(ctx, data, guild, channel).await;
}

/// Включает следующий трек из очереди, если бот в канале и сейчас ничего не играет.
pub async fn play_next(ctx: &serenity::Context, data: &Shared, guild: serenity::GuildId, channel: serenity::ChannelId) {
    let lock = data.music.lock_for(guild.get());
    let _guard = lock.lock().await; // пока один вызов здесь, второй ждёт
    let Some(manager) = songbird::get(ctx).await else { return };
    let Some(call) = manager.get(guild) else { return }; // бот не в канале — очередь сохранится
    if call.lock().await.current_channel().is_none() || current_mode(data, guild.get()).await.is_some() {
        return; // не в канале или что-то уже играет
    }
    // Пробуем треки по очереди, пока какой-нибудь не найдётся
    let (item, title, url) = loop {
        let Ok(Some(item)) = data.db.queue_pop(guild.get()) else { return }; // очередь закончилась
        match search(data, &item.0).await {
            Ok((title, url)) => break (item, title, url),
            Err(error) => {
                println!("Не нашла «{}»: {error}", item.1);
                say_in(ctx, channel, &format!("⚠️ Не получилось загрузить **{}**, пропускаю.", item.1)).await;
            }
        }
    };
    if call.lock().await.current_channel().is_none() {
        let _ = data.db.queue_push_front(guild.get(), &item.0, &item.1); // пока искали, бот вышел из канала
        return;
    }
    // Звук качает yt-dlp и отдаёт его через «трубу»; его сообщения собираем — покажем, если трек оборвётся
    let mut command = std::process::Command::new(data.ytdlp.split_whitespace().next().unwrap_or("yt-dlp"));
    command
        .args(data.ytdlp.split_whitespace().skip(1))
        .args(["--format", "bestaudio/best", "--no-playlist", "--quiet", "--no-warnings", "--no-part", "--output", "-"])
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = data.db.queue_push_front(guild.get(), &item.0, &item.1);
            say_in(ctx, channel, &format!("⚠️ Не получилось запустить yt-dlp: `{error}`")).await;
            return;
        }
    };
    let log = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let log = log.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut log = locked(&log);
                log.push_str(&line);
                log.push('\n');
                if log.len() > 20_000 {
                    log.drain(..10_000); // держим только конец
                }
            }
        });
    }
    let input: Input = ChildContainer::from(child).into();
    let volume = data.music.with(guild.get(), |state| state.volume.unwrap_or(1.0));
    let handle = call.lock().await.play_input(input);
    let _ = handle.set_volume(volume);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let end = || TrackEnd {
        ctx: ctx.clone(),
        data: data.clone(),
        guild,
        channel,
        title: title.clone(),
        log: log.clone(),
        done: done.clone(),
    };
    let _ = handle.add_event(Event::Track(TrackEvent::End), end());
    let _ = handle.add_event(Event::Track(TrackEvent::Error), end());
    data.music.with(guild.get(), |state| {
        state.track = Some(handle);
        state.now_playing = Some(item);
        state.playing_url = Some(url);
        state.skipped = false;
    });
    drop(_guard);
    let message = serenity::CreateMessage::new()
        .content(format!("▶️ Играет: **{title}**"))
        .components(music_buttons())
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    let _ = channel.send_message(ctx, message).await;
    let _ = data.db.record_play(guild.get(), &chrono::Local::now().format("%Y-%m").to_string(), &title); // для топа месяца
}

/// Очередь кончилась: если автоплей включён и бота кто-то слушает — добавляет в очередь похожий трек.
async fn autoplay_add(ctx: &serenity::Context, data: &Data, guild: serenity::GuildId, channel: serenity::ChannelId, url: &str) {
    if !data.db.autoplay(guild.get()).unwrap_or(false) {
        return;
    }
    let Some(voice) = bot_channel(ctx, guild).await else { return };
    let listeners = ctx.cache.guild(guild).is_some_and(|cached| {
        cached.voice_states.values().any(|state| {
            state.channel_id.map(|id| id.get()) == Some(voice.0.get())
                && !cached.members.get(&state.user_id).is_some_and(|member| member.user.bot)
                && state.user_id != ctx.cache.current_user().id
        })
    });
    if !listeners {
        return; // в канале никого нет — не играем в пустоту
    }
    let recent = {
        let mut all = locked(&data.music.autoplay_recent);
        let recent = all.entry(guild.get()).or_default();
        recent.push(url.to_string());
        if recent.len() > 40 {
            recent.drain(..recent.len() - 40); // помним последние 40 треков
        }
        recent.clone()
    };
    let fresh: Vec<(String, String)> = similar_tracks(data, url).await.into_iter().filter(|(link, _)| !recent.contains(link)).collect();
    let Some((link, title)) = fresh[..fresh.len().min(6)].choose(&mut rand::rng()).cloned() else { return };
    if data.db.queue_add(guild.get(), &link, &title).is_ok() {
        say_in(ctx, channel, &format!("📻 Автоплей: дальше **{title}**. Выключить — /autoplay.")).await;
    }
}

// ---------- Команды ----------

/// Включить песню по названию или ссылке
#[poise::command(slash_command, guild_only)]
pub async fn play(
    ctx: Context<'_>,
    #[description = "Название песни или ссылка (YouTube, SoundCloud, Spotify)"] query: String,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let Some(voice) = user_channel(ctx.serenity_context(), guild, ctx.author().id) else {
        return whisper(ctx, "Сначала зайди в голосовой канал.").await;
    };
    ctx.defer().await?; // поиск дольше 3 секунд, которые даёт Discord
    let was_playing = current_mode(data, guild.get()).await.is_some();
    if let Err(error) = join(ctx.serenity_context(), guild, voice).await {
        ctx.say(format!("⚠️ Не получилось зайти в голосовой канал: `{error}`")).await?;
        return Ok(());
    }
    let mut query = query;
    if let Some((kind, id)) = crate::spotify::parse_link(&query) {
        let mut found = None;
        let keys = (std::env::var("SPOTIFY_CLIENT_ID").unwrap_or_default(), std::env::var("SPOTIFY_CLIENT_SECRET").unwrap_or_default());
        // Треки и альбомы — через Spotify API (если ключи заданы), плейлисты и всё остальное — со страницы плеера
        if !keys.0.is_empty() && !keys.1.is_empty() && kind != crate::spotify::Kind::Playlist {
            match crate::spotify::api_lookup(&data.web, &keys.0, &keys.1, kind, &id).await {
                Ok(result) => found = Some(result),
                Err(error) => report_error(ctx.http(), data, Some(guild), "Spotify API /play", &error.to_string()).await,
            }
        }
        if found.is_none() {
            match crate::spotify::embed_lookup(&data.web, kind, &id).await {
                Ok(result) => found = Some(result),
                Err(error) => {
                    report_error(ctx.http(), data, Some(guild), "Spotify (страница плеера) /play", &error.to_string()).await;
                    let what = if kind == crate::spotify::Kind::Playlist { "плейлист приватный или удалён" } else { "ссылка неправильная или трек удалён" };
                    ctx.say(format!("Не получилось получить данные из Spotify: возможно, {what}.")).await?;
                    return Ok(());
                }
            }
        }
        let (queries, name) = found.unwrap_or_default();
        if kind != crate::spotify::Kind::Track {
            // Искать треки на YouTube бот будет по одному, перед включением
            for query in &queries {
                data.db.queue_add(guild.get(), &format!("ytsearch1:{query}"), query)?;
            }
            let label = if kind == crate::spotify::Kind::Playlist { "📜 Плейлист" } else { "💿 Альбом" };
            ctx.say(format!("{label} **{name}**: добавил в очередь треков — {}.", queries.len())).await?;
            play_next(ctx.serenity_context(), data, guild, ctx.channel_id()).await;
            return Ok(());
        }
        query = queries.into_iter().next().unwrap_or(query); // один трек ищем как «Исполнитель - Название»
    }
    let (title, url) = match search(data, &query).await {
        Ok(found) => found,
        Err(error) => {
            report_error(ctx.http(), data, Some(guild), "Музыка: поиск /play", &error.to_string()).await;
            let reason: String = error.to_string().chars().take(300).collect();
            ctx.say(format!("Не получилось найти или загрузить этот трек: `{reason}`")).await?;
            return Ok(());
        }
    };
    let waiting = data.db.queue_count(guild.get())?;
    data.db.queue_add(guild.get(), &url, &title)?;
    let text = match (was_playing, waiting) {
        (false, 0) => format!("🔎 Нашёл: **{title}**"),
        (false, _) => format!("➕ Добавил **{title}** в конец очереди. Сначала сыграю сохранённые треки: {waiting}."),
        (true, _) => format!("➕ Добавлено в очередь: **{title}**"),
    };
    ctx.send(poise::CreateReply::default().content(text).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    play_next(ctx.serenity_context(), data, guild, ctx.channel_id()).await;
    Ok(())
}

/// Пропустить текущую песню
#[poise::command(slash_command, guild_only)]
pub async fn skip(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    if current_mode(data, guild).await != Some(PlayMode::Play) {
        return whisper(ctx, NOT_PLAYING).await;
    }
    stop_current(data, guild);
    ctx.say("⏭️ Пропущено.").await?;
    Ok(())
}

/// Останавливает текущий трек как «пропущенный» (это не ошибка звука). Следующий включит обработчик конца трека.
fn stop_current(data: &Data, guild: u64) {
    if let Some(track) = data.music.with(guild, |state| {
        state.skipped = true;
        state.track.clone()
    }) {
        let _ = track.stop();
    }
}

/// Поставить на паузу
#[poise::command(slash_command, guild_only)]
pub async fn pause(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    if current_mode(data, guild).await != Some(PlayMode::Play) {
        return whisper(ctx, NOT_PLAYING).await;
    }
    if let Some(track) = data.music.with(guild, |state| state.track.clone()) {
        track.pause()?;
    }
    ctx.say("⏸️ Пауза.").await?;
    Ok(())
}

/// Продолжить после паузы или запустить сохранённую очередь
#[poise::command(slash_command, guild_only)]
pub async fn resume(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let mode = current_mode(data, guild.get()).await;
    if mode == Some(PlayMode::Pause) {
        if let Some(track) = data.music.with(guild.get(), |state| state.track.clone()) {
            track.play()?;
        }
        ctx.say("▶️ Продолжаю.").await?;
        return Ok(());
    }
    let waiting = data.db.queue_count(guild.get())?;
    if waiting == 0 {
        return whisper(ctx, "Очередь пуста — включи что-нибудь через /play.").await;
    }
    if mode.is_some() {
        return whisper(ctx, "Музыка и так играет 🙂").await;
    }
    if bot_channel(ctx.serenity_context(), guild).await.is_none() {
        // бот не в канале — заходим к тому, кто вызвал команду
        let Some(voice) = user_channel(ctx.serenity_context(), guild, ctx.author().id) else {
            return whisper(ctx, "Сначала зайди в голосовой канал.").await;
        };
        ctx.defer().await?;
        if let Err(error) = join(ctx.serenity_context(), guild, voice).await {
            ctx.say(format!("⚠️ Не получилось зайти в голосовой канал: `{error}`")).await?;
            return Ok(());
        }
        ctx.say(format!("▶️ Продолжаю сохранённую очередь: {}.", tracks_text(waiting))).await?;
    } else {
        ctx.say("▶️ Продолжаю очередь.").await?;
    }
    play_next(ctx.serenity_context(), data, guild, ctx.channel_id()).await;
    Ok(())
}

/// Показать очередь песен
#[poise::command(slash_command, guild_only, rename = "queue")]
pub async fn show_queue(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let total = data.db.queue_count(guild)?;
    if total == 0 {
        ctx.say("Очередь пуста.").await?;
        return Ok(());
    }
    let tracks = data.db.queue_tracks(guild, 15)?; // показываем первые 15
    let lines: Vec<String> = tracks.iter().enumerate().map(|(index, (_, title))| format!("{}. {title}", index + 1)).collect();
    let mut text = format!("📜 Очередь ({}):\n{}", tracks_text(total), lines.join("\n"));
    if total > tracks.len() {
        text.push_str(&format!("\n…и ещё {}", total - tracks.len()));
    }
    ctx.send(poise::CreateReply::default().content(text).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Удалить все треки из очереди
#[poise::command(slash_command, guild_only)]
pub async fn remove_all(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let removed = data.db.queue_clear(guild)?;
    let mut text = format!("🗑️ Очередь очищена, удалено треков: {removed}.");
    if current_mode(data, guild).await.is_some() {
        text.push_str("\nТекущий трек доиграет — чтобы выключить его, используй /skip.");
    }
    ctx.say(text).await?;
    Ok(())
}

/// Изменить громкость (от 0 до 200%)
#[poise::command(slash_command, guild_only)]
pub async fn setvolume(
    ctx: Context<'_>,
    #[description = "Громкость в процентах: 100 — обычная, 50 — тише, 150 — громче"]
    #[min = 0]
    #[max = 200]
    level: u32,
) -> Result<(), Error> {
    let volume = level.min(200) as f32 / 100.0;
    let track = ctx.data().music.with(ctx.guild_id().unwrap().get(), |state| {
        state.volume = Some(volume); // для следующих песен
        state.track.clone()
    });
    if let Some(track) = track {
        let _ = track.set_volume(volume); // и для той, что играет
    }
    ctx.say(format!("🔊 Громкость: **{}%**", level.min(200))).await?;
    Ok(())
}

/// Выйти из канала (очередь сохранится)
#[poise::command(slash_command, guild_only)]
pub async fn leave(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let manager = songbird::get(ctx.serenity_context()).await.ok_or("голосовой модуль не запущен")?;
    if manager.get(guild).is_none() {
        return whisper(ctx, "Я и так не в канале.").await;
    }
    ctx.defer().await?;
    let (current, track) = data.music.with(guild.get(), |state| {
        state.skipped = true;
        (state.now_playing.take(), state.track.take())
    });
    if let Some((url, title)) = current {
        data.db.queue_push_front(guild.get(), &url, &title)?; // что играло — сыграет первым
    }
    if let Some(track) = track {
        let _ = track.stop();
    }
    manager.remove(guild).await?;
    let saved = data.db.queue_count(guild.get())?;
    let mut text = "👋 Вышел из канала.".to_string();
    if saved > 0 {
        text.push_str(&format!(" Очередь сохранена ({}) — продолжить: /resume", tracks_text(saved)));
    }
    ctx.say(text).await?;
    Ok(())
}

/// Автоплей: когда очередь кончилась, бот сам включает похожие треки
#[poise::command(slash_command, guild_only)]
pub async fn autoplay(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let enabled = !data.db.autoplay(guild)?;
    data.db.set_autoplay(guild, enabled)?;
    ctx.say(if enabled {
        "📻 Автоплей включён: когда очередь закончится, я сам подберу похожий трек (работает для треков с YouTube)."
    } else {
        "📻 Автоплей выключен."
    })
    .await?;
    Ok(())
}

async fn playlist_autocomplete(ctx: Context<'_>, current: &str) -> serenity::CreateAutocompleteResponse {
    let names = ctx.guild_id().and_then(|guild| ctx.data().db.playlists(guild.get()).ok()).unwrap_or_default();
    let current = current.to_lowercase();
    let choices: Vec<serenity::AutocompleteChoice> = names
        .into_iter()
        .filter(|(name, _)| name.to_lowercase().contains(&current))
        .take(25)
        .map(|(name, _)| serenity::AutocompleteChoice::new(name.chars().take(100).collect::<String>(), name))
        .collect();
    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}

/// Сохранить текущий трек и очередь как плейлист сервера
#[poise::command(slash_command, guild_only)]
pub async fn playlist_save(
    ctx: Context<'_>,
    #[description = "Как назвать плейлист"]
    #[max_length = 40]
    name: String,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let name = name.trim().to_string();
    let mut tracks: Vec<(String, String)> = data.music.with(guild, |state| state.now_playing.clone()).into_iter().collect();
    tracks.extend(data.db.queue_tracks(guild, MAX_PLAYLIST_SAVE as i64)?);
    tracks.truncate(MAX_PLAYLIST_SAVE);
    if name.is_empty() || tracks.is_empty() {
        return whisper(ctx, "Сохранять нечего: сейчас ничего не играет и очередь пуста.").await;
    }
    let names = data.db.playlists(guild)?;
    if !names.iter().any(|(existing, _)| *existing == name) && names.len() >= MAX_PLAYLISTS {
        return whisper(ctx, format!("На сервере уже {MAX_PLAYLISTS} плейлистов — удали ненужный: /playlist_delete.")).await;
    }
    data.db.save_playlist(guild, &name, &tracks)?;
    ctx.say(format!("💾 Плейлист **{name}** сохранён: {}. Включить — /playlist_play.", tracks_text(tracks.len()))).await?;
    Ok(())
}

/// Добавить в очередь сохранённый плейлист сервера
#[poise::command(slash_command, guild_only)]
pub async fn playlist_play(
    ctx: Context<'_>,
    #[description = "Какой плейлист включить"]
    #[autocomplete = "playlist_autocomplete"]
    name: String,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let tracks = data.db.playlist(guild.get(), &name)?;
    if tracks.is_empty() {
        return whisper(ctx, "Такого плейлиста нет — выбери из списка (/playlists).").await;
    }
    let Some(voice) = user_channel(ctx.serenity_context(), guild, ctx.author().id) else {
        return whisper(ctx, "Сначала зайди в голосовой канал.").await;
    };
    ctx.defer().await?;
    if let Err(error) = join(ctx.serenity_context(), guild, voice).await {
        ctx.say(format!("⚠️ Не получилось зайти в голосовой канал: `{error}`")).await?;
        return Ok(());
    }
    for (url, title) in &tracks {
        data.db.queue_add(guild.get(), url, title)?;
    }
    ctx.say(format!("📜 Плейлист **{name}**: добавил в очередь {}.", tracks_text(tracks.len()))).await?;
    play_next(ctx.serenity_context(), data, guild, ctx.channel_id()).await;
    Ok(())
}

/// Сохранённые плейлисты сервера
#[poise::command(slash_command, guild_only, rename = "playlists")]
pub async fn show_playlists(ctx: Context<'_>) -> Result<(), Error> {
    let rows = ctx.data().db.playlists(ctx.guild_id().unwrap().get())?;
    if rows.is_empty() {
        return whisper(ctx, "Плейлистов пока нет. Собери очередь и сохрани её: /playlist_save.").await;
    }
    let lines: Vec<String> = rows.iter().map(|(name, count)| format!("• **{name}** — {}", tracks_text(*count as usize))).collect();
    ctx.say(format!("💾 Плейлисты сервера:\n{}", lines.join("\n"))).await?;
    Ok(())
}

/// Удалить сохранённый плейлист сервера
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
pub async fn playlist_delete(
    ctx: Context<'_>,
    #[description = "Какой плейлист удалить"]
    #[autocomplete = "playlist_autocomplete"]
    name: String,
) -> Result<(), Error> {
    let removed = ctx.data().db.delete_playlist(ctx.guild_id().unwrap().get(), &name)?;
    whisper(ctx, if removed { format!("🗑️ Плейлист **{name}** удалён.") } else { "Такого плейлиста нет.".to_string() }).await
}

const LYRICS_NOISE: [&str; 10] = ["official", "video", "audio", "lyric", "клип", "премьера", "hd", "4k", "remaster", "visualizer"];

/// Название трека без мусора вроде «(Official Video)» — чтобы поиск текста нашёл песню.
pub fn lyrics_query(title: &str) -> String {
    let chars: Vec<char> = title.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < chars.len() {
        if matches!(chars[index], '(' | '[') {
            if let Some(length) = chars[index + 1..].iter().position(|c| matches!(c, ')' | ']')) {
                let inside: String = chars[index + 1..index + 1 + length].iter().collect::<String>().to_lowercase();
                if LYRICS_NOISE.iter().any(|noise| inside.contains(noise)) {
                    result.push(' ');
                    index += length + 2;
                    continue;
                }
            }
        }
        result.push(chars[index]);
        index += 1;
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches([' ', '-', '|']).to_string()
}

/// Найти текст песни (той, что играет, или по названию)
#[poise::command(slash_command, guild_only)]
pub async fn lyrics(
    ctx: Context<'_>,
    #[description = "Название песни (если не указать — та, что сейчас играет)"]
    #[max_length = 150]
    song: Option<String>,
) -> Result<(), Error> {
    let playing = ctx.data().music.with(ctx.guild_id().unwrap().get(), |state| state.now_playing.clone().map(|(_, title)| title));
    let Some(title) = song.or(playing) else {
        return whisper(ctx, "Сейчас ничего не играет — напиши название песни в поле song.").await;
    };
    let cleaned = lyrics_query(&title);
    let query = if cleaned.is_empty() { title } else { cleaned };
    // Сами тексты бот в чат не пишет (они защищены авторским правом) — даёт кнопку на страницу с текстом
    let link = reqwest::Url::parse_with_params("https://genius.com/search", [("q", query.as_str())])?;
    let button = serenity::CreateButton::new_link(link.as_str()).label("Открыть текст на Genius");
    let reply = poise::CreateReply::default()
        .content(format!("🎤 Текст песни **{query}**:"))
        .components(vec![serenity::CreateActionRow::Buttons(vec![button])])
        .ephemeral(true);
    ctx.send(reply).await?;
    Ok(())
}

/// Топ-10 самых прослушиваемых треков за этот месяц
#[poise::command(slash_command, guild_only)]
pub async fn top_chart_month(ctx: Context<'_>) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap());
    let today = chrono::Local::now().date_naive();
    let rows = data.db.top_tracks(guild.get(), &today.format("%Y-%m").to_string())?;
    if rows.is_empty() {
        ctx.say("В этом месяце ещё ничего не слушали — включай /play!").await?;
        return Ok(());
    }
    ctx.defer().await?;
    use chrono::Datelike;
    let month_title = format!("{} {}", MONTH_NAMES[today.month0() as usize], today.year());
    let most = rows[0].1.max(1); // больше всего прослушиваний у первого — от него длина полосок
    let rows: Vec<Row> = rows
        .into_iter()
        .map(|(title, plays)| Row {
            name: title,
            picture: Picture::Music,
            progress: plays as f64 / most as f64,
            pill: format!("{plays} {}", plural(plays, "раз", "раза", "раз")),
            hint: "прослушано".to_string(),
        })
        .collect();
    let server = guild.name(ctx).unwrap_or_default();
    let png = crate::levels::render_table(data, "ТОП ТРЕКОВ МЕСЯЦА", format!("{month_title} · {server}"), "по прослушиваниям", rows, "chart_bg").await;
    crate::levels::send_png(ctx, png, "top_chart.png").await
}

// ---------- Кнопки под «Играет» ----------

/// Нажали кнопку плеера. Возвращает false, если это не кнопка музыки.
pub async fn button_pressed(ctx: &serenity::Context, data: &Shared, press: &serenity::ComponentInteraction) -> bool {
    let Some(action) = press.data.custom_id.strip_prefix("music:") else { return false };
    let Some(guild) = press.guild_id else { return true };
    let answer = |text: String, private: bool| {
        let message = serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(private).allowed_mentions(serenity::CreateAllowedMentions::new());
        press.create_response(ctx, serenity::CreateInteractionResponse::Message(message))
    };
    // Нажимать может тот, кто сидит с ботом в одном голосовом канале
    let same_voice = match (bot_channel(ctx, guild).await, user_channel(ctx, guild, press.user.id)) {
        (Some(bot), Some(user)) => bot.0.get() == user.get(),
        _ => false,
    };
    if !same_voice {
        let _ = answer("Зайди в голосовой канал, где играет бот.".into(), true).await;
        return true;
    }
    let who = format!("<@{}>", press.user.id);
    let mode = current_mode(data, guild.get()).await;
    let track = data.music.with(guild.get(), |state| state.track.clone());
    let (text, private) = match (action, mode, track) {
        ("pause", Some(PlayMode::Pause), Some(track)) => {
            let _ = track.play();
            (format!("▶️ {who} продолжает музыку."), false)
        }
        ("pause", Some(PlayMode::Play), Some(track)) => {
            let _ = track.pause();
            (format!("⏸️ {who} ставит на паузу."), false)
        }
        ("skip", Some(_), Some(_)) => {
            stop_current(data, guild.get());
            (format!("⏭️ {who} пропускает трек."), false)
        }
        ("repeat", _, _) => {
            let repeat = data.music.with(guild.get(), |state| {
                state.repeat = !state.repeat;
                state.repeat
            });
            if repeat {
                (format!("🔁 {who} включает повтор: трек будет играть снова, пока не нажмут ⏭️ или 🔁."), false)
            } else {
                (format!("➡️ {who} выключает повтор."), false)
            }
        }
        ("shuffle", _, _) => match data.db.queue_shuffle(guild.get()) {
            Ok(count) if count >= 2 => (format!("🔀 {who} перемешивает очередь ({}).", tracks_text(count)), false),
            _ => ("В очереди нечего перемешивать.".to_string(), true),
        },
        _ => (NOT_PLAYING.to_string(), true),
    };
    let _ = answer(text, private).await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проверка без Discord: yt-dlp качает настоящий трек, а бот распознаёт формат звука.
    /// Нужны интернет и yt-dlp, поэтому тест запускается только вручную: cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn decodes_ytdlp_stream() {
        let program = std::env::var("YTDLP").unwrap_or_else(|_| "yt-dlp".to_string());
        let mut parts = program.split_whitespace();
        let child = std::process::Command::new(parts.next().unwrap())
            .args(parts)
            .args(["--format", "bestaudio/best", "--no-playlist", "--quiet", "--no-warnings", "--no-part", "--output", "-"])
            .arg("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("yt-dlp не запустился");
        let input: Input = ChildContainer::from(child).into();
        let playable = input
            .make_playable_async(songbird::input::codecs::get_codec_registry(), songbird::input::codecs::get_probe())
            .await;
        assert!(playable.is_ok(), "звук не распознан: {:?}", playable.err());
        assert!(playable.unwrap().is_playable());
    }

    /// Поиск и автоплей через настоящий yt-dlp (нужен интернет): cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn real_search_and_mix() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let data = Data::new(db::Database::in_memory().unwrap(), &crate::config::Settings::default(), dir, false);
        let (title, url) = search(&data, "rick astley never gonna give you up").await.unwrap();
        println!("{title} — {url}");
        assert!(youtube_id(&url).is_some(), "{url}");
        let mix = similar_tracks(&data, &url).await;
        println!("микс: {}", mix.len());
        assert!(!mix.is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(ytdlp_target("  dark days "), "ytsearch1:dark days");
        assert_eq!(ytdlp_target("https://youtu.be/abc"), "https://youtu.be/abc");
        assert_eq!(ytdlp_target("ytsearch1:A - B"), "ytsearch1:A - B");
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=5").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://youtube.com/shorts/abcdefghijk").as_deref(), Some("abcdefghijk"));
        assert_eq!(youtube_id("https://soundcloud.com/x/y"), None);
        let mix = parse_mix("dQw4w9WgXcQ\nСама песня\nAAAAAAAAAAA\nДругая\nBBBBBBBBBBB\nNA\n", "dQw4w9WgXcQ");
        assert_eq!(mix, [("https://www.youtube.com/watch?v=AAAAAAAAAAA".to_string(), "Другая".to_string()),
                         ("https://www.youtube.com/watch?v=BBBBBBBBBBB".to_string(), "Без названия".to_string())]);
        assert_eq!(lyrics_query("Rick Astley - Never Gonna Give You Up (Official Music Video) [4K Remaster]"), "Rick Astley - Never Gonna Give You Up");
        assert_eq!(lyrics_query("Песня (feat. Друг) | Премьера клипа"), "Песня (feat. Друг) | Премьера клипа");
        assert_eq!(lyrics_query("Кино - Группа крови [Клип]"), "Кино - Группа крови");
        assert_eq!(failure_reason("[youtube] x\nERROR: Video unavailable\nпотом что-то", "?"), "ERROR: Video unavailable");
        assert_eq!(failure_reason("", "ничего"), "ничего");
        assert_eq!(hide_links("ERROR: https://googlevideo.com/abc?x=1 403"), "ERROR: <ссылка> 403");
    }

    #[test]
    fn queue_rows() {
        let db = db::Database::in_memory().unwrap();
        db.queue_add(1, "u1", "Первый").unwrap();
        db.queue_add(1, "u2", "Второй").unwrap();
        db.queue_add(2, "x", "Чужой").unwrap();
        db.queue_push_front(1, "u0", "Нулевой").unwrap();
        assert_eq!(db.queue_count(1).unwrap(), 3);
        assert_eq!(db.queue_pop(1).unwrap(), Some(("u0".into(), "Нулевой".into())));
        assert_eq!(db.queue_tracks(1, 15).unwrap().iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), ["Первый", "Второй"]);
        assert_eq!(db.queue_shuffle(1).unwrap(), 2);
        assert_eq!(db.queue_clear(1).unwrap(), 2);
        assert_eq!(db.queue_pop(1).unwrap(), None);
        assert_eq!(db.queue_count(2).unwrap(), 1);

        db.record_play(1, "2026-10", "Старое").unwrap();
        db.record_play(1, "2026-11", "Хит").unwrap();
        db.record_play(1, "2026-11", "Хит").unwrap();
        db.record_play(1, "2026-11", "Другое").unwrap();
        assert_eq!(db.top_tracks(1, "2026-11").unwrap(), [("Хит".to_string(), 2), ("Другое".to_string(), 1)]);
        assert!(db.top_tracks(1, "2026-10").unwrap().is_empty()); // прошлый месяц стёрт

        assert!(!db.autoplay(1).unwrap());
        db.set_autoplay(1, true).unwrap();
        assert!(db.autoplay(1).unwrap());

        let tracks = vec![("a".to_string(), "A".to_string()), ("b".to_string(), "B".to_string())];
        db.save_playlist(1, "вечер", &tracks).unwrap();
        db.save_playlist(1, "вечер", &tracks[..1]).unwrap(); // то же имя — перезаписан
        db.save_playlist(1, "утро", &tracks).unwrap();
        assert_eq!(db.playlists(1).unwrap(), [("вечер".to_string(), 1), ("утро".to_string(), 2)]);
        assert_eq!(db.playlist(1, "утро").unwrap(), tracks);
        assert!(db.delete_playlist(1, "утро").unwrap() && !db.delete_playlist(1, "утро").unwrap());
    }
}
