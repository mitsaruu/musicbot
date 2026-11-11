//! База данных — тот же файл levels.db, что у bot.py, с теми же таблицами.
//! Пока здесь только то, что нужно уже перенесённым командам: кошельки и ежедневный бонус.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

/// Питомец из таблицы pets.
#[derive(Debug, Clone, PartialEq)]
pub struct Pet {
    pub id: i64,
    pub species: String, // вид — ключ из списка видов
    pub level: i64,
    pub last_collect: f64, // с какого момента копится доход
    pub spent: i64,        // сколько монет в него вложено (кейс + корм) — для продажи
}

fn pet_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Pet> {
    Ok(Pet {
        id: row.get(0)?,
        species: row.get(1)?,
        level: row.get::<_, Option<i64>>(2)?.unwrap_or(1),
        last_collect: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
        spent: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
    })
}

/// Настройки утренних и вечерних сообщений сервера из таблицы daily_settings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DailySettings {
    pub guild: u64,
    pub channel: u64,
    pub timezone: String,     // например "Asia/Jerusalem"
    pub morning: String,      // "08:00"
    pub evening: String,      // "22:00"
    pub last_morning: String, // дата, когда последний раз отправили утреннее
    pub last_evening: String,
}

/// Дуэль на слово дня из таблицы word_duels.
#[derive(Debug, Clone, PartialEq)]
pub struct WordDuel {
    pub id: i64,
    pub guild: u64,
    pub channel: u64, // где объявить итог
    pub lang: String,
    pub day: String, // слово какого дня
    pub user1: u64,
    pub user2: u64,
    pub bet: i64,
}

pub struct Database {
    /// SQLite-соединение нельзя использовать из двух потоков сразу, поэтому оно под замком.
    connection: Mutex<Connection>,
}

/// Текущее время в секундах (как time.time() в Python).
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs_f64())
        .unwrap_or(0.0)
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::prepare(Connection::open(path)?)
    }

    /// База в памяти — для тестов.
    #[cfg(test)]
    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> rusqlite::Result<Self> {
        // Таблицы создаются теми же командами, что в bot.py: если базу уже создал Python — ничего не меняется.
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS economy (
                 guild_id  INTEGER,
                 user_id   INTEGER,
                 name      TEXT,
                 balance   INTEGER,
                 last_work REAL,
                 PRIMARY KEY (guild_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS daily_bonus (
                 guild_id INTEGER,
                 user_id  INTEGER,
                 last_day TEXT,
                 streak   INTEGER,
                 PRIMARY KEY (guild_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS pets (
                 id           INTEGER PRIMARY KEY AUTOINCREMENT,
                 guild_id     INTEGER,
                 user_id      INTEGER,
                 species      TEXT,
                 level        INTEGER DEFAULT 1,
                 last_collect REAL,
                 spent        INTEGER DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS levels (
                 guild_id     INTEGER,
                 user_id      INTEGER,
                 name         TEXT,
                 xp           INTEGER DEFAULT 0,
                 last_xp_time REAL    DEFAULT 0,
                 PRIMARY KEY (guild_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS games_channel (
                 guild_id   INTEGER PRIMARY KEY,
                 channel_id INTEGER
             );
             CREATE TABLE IF NOT EXISTS word_daily (
                 guild_id INTEGER, user_id INTEGER, day TEXT, answer TEXT, guesses TEXT,
                 PRIMARY KEY (guild_id, user_id, day)
             );
             CREATE TABLE IF NOT EXISTS word_daily_ru (
                 guild_id INTEGER, user_id INTEGER, day TEXT, answer TEXT, guesses TEXT,
                 PRIMARY KEY (guild_id, user_id, day)
             );
             CREATE TABLE IF NOT EXISTS word_summary (guild_id INTEGER, day TEXT, PRIMARY KEY (guild_id, day));
             CREATE TABLE IF NOT EXISTS word_summary_ru (guild_id INTEGER, day TEXT, PRIMARY KEY (guild_id, day));
             CREATE TABLE IF NOT EXISTS word_check (word TEXT PRIMARY KEY, valid INTEGER);
             CREATE TABLE IF NOT EXISTS word_duels (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 guild_id INTEGER, channel_id INTEGER, lang TEXT, day TEXT, user1 INTEGER, user2 INTEGER, bet INTEGER
             );
             CREATE TABLE IF NOT EXISTS dick (
                 guild_id    INTEGER,
                 user_id     INTEGER,
                 name        TEXT,
                 size        INTEGER DEFAULT 0,
                 last_growth REAL    DEFAULT 0,
                 PRIMARY KEY (guild_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS reminders (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 guild_id INTEGER, channel_id INTEGER, user_id INTEGER, due REAL, text TEXT
             );
             CREATE TABLE IF NOT EXISTS polls (
                 message_id INTEGER PRIMARY KEY,
                 channel_id INTEGER, author_id INTEGER, question TEXT, options TEXT, ends REAL
             );
             CREATE TABLE IF NOT EXISTS poll_votes (
                 message_id INTEGER, user_id INTEGER, option INTEGER,
                 PRIMARY KEY (message_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS log_channel (guild_id INTEGER PRIMARY KEY, channel_id INTEGER);
             CREATE TABLE IF NOT EXISTS patchnotes_channel (guild_id INTEGER PRIMARY KEY, channel_id INTEGER, last_note TEXT);
             CREATE TABLE IF NOT EXISTS weather_cities (
                 guild_id INTEGER, name TEXT, latitude REAL, longitude REAL,
                 PRIMARY KEY (guild_id, name)
             );
             CREATE TABLE IF NOT EXISTS warns (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 guild_id INTEGER, user_id INTEGER, moderator_id INTEGER, reason TEXT, created REAL
             );
             CREATE TABLE IF NOT EXISTS ai_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 channel_id INTEGER, role TEXT, text TEXT, created REAL
             );
             CREATE TABLE IF NOT EXISTS ai_stats (day TEXT PRIMARY KEY, requests INTEGER);
             CREATE TABLE IF NOT EXISTS rude_strikes (
                 guild_id INTEGER, user_id INTEGER, count INTEGER, last_time REAL,
                 PRIMARY KEY (guild_id, user_id)
             );
             CREATE TABLE IF NOT EXISTS plays (
                 guild_id INTEGER, month TEXT, title TEXT, count INTEGER DEFAULT 0,
                 PRIMARY KEY (guild_id, month, title)
             );
             CREATE TABLE IF NOT EXISTS queue (id INTEGER PRIMARY KEY, guild_id INTEGER, url TEXT, title TEXT);
             CREATE TABLE IF NOT EXISTS music_settings (guild_id INTEGER PRIMARY KEY, autoplay INTEGER);
             CREATE TABLE IF NOT EXISTS playlists (
                 guild_id INTEGER, name TEXT, position INTEGER, url TEXT, title TEXT,
                 PRIMARY KEY (guild_id, name, position)
             );
             CREATE TABLE IF NOT EXISTS welcome_channel (guild_id INTEGER PRIMARY KEY, channel_id INTEGER);
             CREATE TABLE IF NOT EXISTS daily_settings (
                 guild_id INTEGER PRIMARY KEY, channel_id INTEGER, timezone TEXT, morning TEXT, evening TEXT,
                 last_morning TEXT DEFAULT '', last_evening TEXT DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS shop_roles (
                 guild_id INTEGER,
                 role_id  INTEGER,
                 price    INTEGER,
                 days     INTEGER,
                 PRIMARY KEY (guild_id, role_id)
             );
             CREATE TABLE IF NOT EXISTS shop_purchases (
                 guild_id INTEGER,
                 user_id  INTEGER,
                 role_id  INTEGER,
                 expires  REAL,
                 PRIMARY KEY (guild_id, user_id, role_id)
             );",
        )?;
        Ok(Self { connection: Mutex::new(connection) })
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // Если другой поток упал, держа замок, — база всё равно цела, продолжаем работать.
        self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// (баланс, время последней работы). Если кошелька ещё нет — создаёт его со стартовыми монетами,
    /// а у старого игрока только обновляет имя.
    pub fn wallet(&self, guild: u64, user: u64, name: &str, start_balance: i64) -> rusqlite::Result<(i64, f64)> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO economy (guild_id, user_id, name, balance, last_work) VALUES (?1, ?2, ?3, ?4, 0)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET name = excluded.name",
            params![guild as i64, user as i64, name, start_balance],
        )?;
        connection.query_row(
            "SELECT balance, last_work FROM economy WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |row| Ok((row.get(0)?, row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
        )
    }

    /// Прибавляет монеты (отрицательное число — отнимает). Возвращает новый баланс.
    pub fn add_money(&self, guild: u64, user: u64, name: &str, start_balance: i64, amount: i64) -> rusqlite::Result<i64> {
        self.wallet(guild, user, name, start_balance)?;
        self.lock().execute(
            "UPDATE economy SET balance = balance + ?1 WHERE guild_id = ?2 AND user_id = ?3",
            params![amount, guild as i64, user as i64],
        )?;
        Ok(self.wallet(guild, user, name, start_balance)?.0)
    }

    /// Записывает заработок за /work и время смены.
    pub fn record_work(&self, guild: u64, user: u64, earned: i64, time: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE economy SET balance = balance + ?1, last_work = ?2 WHERE guild_id = ?3 AND user_id = ?4",
            params![earned, time, guild as i64, user as i64],
        )?;
        Ok(())
    }

    /// Место в рейтинге богатства = сколько участников богаче, плюс один.
    pub fn money_position(&self, guild: u64, balance: i64) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM economy WHERE guild_id = ?1 AND balance > ?2",
            params![guild as i64, balance],
            |row| Ok(row.get::<_, i64>(0)? + 1),
        )
    }

    /// Ежедневный бонус: (день последнего бонуса, дней подряд) или None, если ещё не забирал.
    pub fn daily_bonus(&self, guild: u64, user: u64) -> rusqlite::Result<Option<(String, i64)>> {
        self.lock()
            .query_row(
                "SELECT last_day, streak FROM daily_bonus WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Питомцы человека в том порядке, в каком они появились.
    pub fn user_pets(&self, guild: u64, user: u64) -> rusqlite::Result<Vec<Pet>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, species, level, last_collect, spent FROM pets WHERE guild_id = ?1 AND user_id = ?2 ORDER BY id",
        )?;
        let pets = statement.query_map(params![guild as i64, user as i64], pet_from_row)?.collect();
        pets
    }

    /// Питомец по номеру, только если он принадлежит этому человеку.
    pub fn own_pet(&self, id: i64, guild: u64, user: u64) -> rusqlite::Result<Option<Pet>> {
        self.lock()
            .query_row(
                "SELECT id, species, level, last_collect, spent FROM pets WHERE id = ?1 AND guild_id = ?2 AND user_id = ?3",
                params![id, guild as i64, user as i64],
                pet_from_row,
            )
            .optional()
    }

    /// Новый питомец 1 уровня. Возвращает его номер.
    pub fn add_pet(&self, guild: u64, user: u64, species: &str, time: f64, spent: i64) -> rusqlite::Result<i64> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO pets (guild_id, user_id, species, level, last_collect, spent) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![guild as i64, user as i64, species, time, spent],
        )?;
        Ok(connection.last_insert_rowid())
    }

    /// Питомец поел: +1 уровень, корм добавляется к вложенному, отсчёт дохода сдвигается.
    pub fn feed_pet(&self, id: i64, cost: i64, last_collect: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE pets SET level = level + 1, spent = spent + ?1, last_collect = ?2 WHERE id = ?3",
            params![cost, last_collect, id],
        )?;
        Ok(())
    }

    pub fn delete_pet(&self, id: i64) -> rusqlite::Result<()> {
        self.lock().execute("DELETE FROM pets WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Доход забрали — у всех питомцев человека отсчёт начинается заново.
    pub fn mark_pets_collected(&self, guild: u64, user: u64, time: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE pets SET last_collect = ?1 WHERE guild_id = ?2 AND user_id = ?3",
            params![time, guild as i64, user as i64],
        )?;
        Ok(())
    }

    /// Обмен питомцами: каждый питомец переходит к другому хозяину. Если, пока ждали согласия, кого-то из
    /// питомцев продали или отдали — ничего не меняется и возвращается false.
    pub fn swap_pets(&self, guild: u64, first: (i64, u64), second: (i64, u64)) -> rusqlite::Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let owns = |pet: i64, user: u64| -> rusqlite::Result<bool> {
            transaction
                .query_row(
                    "SELECT 1 FROM pets WHERE id = ?1 AND guild_id = ?2 AND user_id = ?3",
                    params![pet, guild as i64, user as i64],
                    |_| Ok(()),
                )
                .optional()
                .map(|found| found.is_some())
        };
        if !(owns(first.0, first.1)? && owns(second.0, second.1)?) {
            return Ok(false);
        }
        transaction.execute("UPDATE pets SET user_id = ?1 WHERE id = ?2", params![second.1 as i64, first.0])?;
        transaction.execute("UPDATE pets SET user_id = ?1 WHERE id = ?2", params![first.1 as i64, second.0])?;
        transaction.commit()?;
        Ok(true)
    }

    /// Роли в магазине сервера: (роль, цена, дней), от дешёвых к дорогим.
    pub fn shop_roles(&self, guild: u64) -> rusqlite::Result<Vec<(u64, i64, i64)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT role_id, price, days FROM shop_roles WHERE guild_id = ?1 ORDER BY price")?;
        let roles = statement
            .query_map(params![guild as i64], |row| Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?)))?
            .collect();
        roles
    }

    pub fn set_shop_role(&self, guild: u64, role: u64, price: i64, days: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO shop_roles (guild_id, role_id, price, days) VALUES (?1, ?2, ?3, ?4)",
            params![guild as i64, role as i64, price, days],
        )?;
        Ok(())
    }

    /// Убирает роль из магазина. true — она там была.
    pub fn remove_shop_role(&self, guild: u64, role: u64) -> rusqlite::Result<bool> {
        let removed = self.lock().execute(
            "DELETE FROM shop_roles WHERE guild_id = ?1 AND role_id = ?2",
            params![guild as i64, role as i64],
        )?;
        Ok(removed > 0)
    }

    /// До какого времени у человека куплена роль на время (None — не покупал).
    pub fn shop_purchase(&self, guild: u64, user: u64, role: u64) -> rusqlite::Result<Option<f64>> {
        self.lock()
            .query_row(
                "SELECT expires FROM shop_purchases WHERE guild_id = ?1 AND user_id = ?2 AND role_id = ?3",
                params![guild as i64, user as i64, role as i64],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_shop_purchase(&self, guild: u64, user: u64, role: u64, expires: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO shop_purchases (guild_id, user_id, role_id, expires) VALUES (?1, ?2, ?3, ?4)",
            params![guild as i64, user as i64, role as i64, expires],
        )?;
        Ok(())
    }

    /// Покупки, срок которых вышел: (сервер, участник, роль).
    pub fn expired_shop_purchases(&self, now: f64) -> rusqlite::Result<Vec<(u64, u64, u64)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT guild_id, user_id, role_id FROM shop_purchases WHERE expires <= ?1")?;
        let rows = statement
            .query_map(params![now], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64))
            })?
            .collect();
        rows
    }

    pub fn remove_shop_purchase(&self, guild: u64, user: u64, role: u64) -> rusqlite::Result<()> {
        self.lock().execute(
            "DELETE FROM shop_purchases WHERE guild_id = ?1 AND user_id = ?2 AND role_id = ?3",
            params![guild as i64, user as i64, role as i64],
        )?;
        Ok(())
    }

    /// Общий опыт участника (0, если его в базе ещё нет).
    pub fn xp(&self, guild: u64, user: u64) -> rusqlite::Result<i64> {
        let xp = self
            .lock()
            .query_row(
                "SELECT xp FROM levels WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(xp.flatten().unwrap_or(0))
    }

    /// Записывает опыт и имя участника (если строки нет — создаёт).
    pub fn set_xp(&self, guild: u64, user: u64, name: &str, xp: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO levels (guild_id, user_id, name, xp) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET name = excluded.name, xp = excluded.xp",
            params![guild as i64, user as i64, name, xp],
        )?;
        Ok(())
    }

    /// Место в рейтинге = сколько людей на сервере набрали больше опыта, плюс один.
    pub fn xp_position(&self, guild: u64, xp: i64) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND xp > ?2",
            params![guild as i64, xp],
            |row| Ok(row.get::<_, i64>(0)? + 1),
        )
    }

    /// Когда участник последний раз получал опыт за сообщение (0 — никогда).
    pub fn last_xp_time(&self, guild: u64, user: u64) -> rusqlite::Result<f64> {
        let time = self
            .lock()
            .query_row(
                "SELECT last_xp_time FROM levels WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| row.get::<_, Option<f64>>(0),
            )
            .optional()?;
        Ok(time.flatten().unwrap_or(0.0))
    }

    pub fn set_last_xp_time(&self, guild: u64, user: u64, time: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE levels SET last_xp_time = ?1 WHERE guild_id = ?2 AND user_id = ?3",
            params![time, guild as i64, user as i64],
        )?;
        Ok(())
    }

    /// 10 участников сервера с наибольшим опытом: (участник, имя, опыт).
    pub fn top_levels(&self, guild: u64) -> rusqlite::Result<Vec<(u64, String, i64)>> {
        self.top("SELECT user_id, name, xp FROM levels WHERE guild_id = ?1 ORDER BY xp DESC LIMIT 10", guild)
    }

    /// 10 самых богатых участников сервера: (участник, имя, баланс).
    pub fn top_money(&self, guild: u64) -> rusqlite::Result<Vec<(u64, String, i64)>> {
        self.top("SELECT user_id, name, balance FROM economy WHERE guild_id = ?1 ORDER BY balance DESC, name LIMIT 10", guild)
    }

    fn top(&self, query: &str, guild: u64) -> rusqlite::Result<Vec<(u64, String, i64)>> {
        let connection = self.lock();
        let mut statement = connection.prepare(query)?;
        let rows = statement
            .query_map(params![guild as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            })?
            .collect();
        rows
    }

    /// Канал для игр на сервере (его выбирают командой /games_channel) или None.
    pub fn games_channel(&self, guild: u64) -> rusqlite::Result<Option<u64>> {
        let channel = self
            .lock()
            .query_row("SELECT channel_id FROM games_channel WHERE guild_id = ?1", params![guild as i64], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .optional()?;
        Ok(channel.flatten().map(|id| id as u64))
    }

    // ---------- Слова. table — "word_daily" или "word_daily_ru" (имена таблиц заданы в коде, не пользователем) ----------

    /// Партия слова дня: (загаданное слово, догадки через запятую) или None, если человек её ещё не открывал.
    pub fn word_row(&self, table: &str, guild: u64, user: u64, day: &str) -> rusqlite::Result<Option<(String, String)>> {
        self.lock()
            .query_row(
                &format!("SELECT answer, guesses FROM {table} WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3"),
                params![guild as i64, user as i64, day],
                |row| Ok((row.get(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default())),
            )
            .optional()
    }

    /// Слово, которое в этот день уже загадано кому-то на сервере.
    pub fn word_any_answer(&self, table: &str, guild: u64, day: &str) -> rusqlite::Result<Option<String>> {
        self.lock()
            .query_row(
                &format!("SELECT answer FROM {table} WHERE guild_id = ?1 AND day = ?2 LIMIT 1"),
                params![guild as i64, day],
                |row| row.get(0),
            )
            .optional()
    }

    /// Начинает партию (если её начали одновременно дважды — остаётся первая).
    pub fn word_start(&self, table: &str, guild: u64, user: u64, day: &str, answer: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            &format!("INSERT OR IGNORE INTO {table} (guild_id, user_id, day, answer, guesses) VALUES (?1, ?2, ?3, ?4, '')"),
            params![guild as i64, user as i64, day, answer],
        )?;
        Ok(())
    }

    pub fn set_word_guesses(&self, table: &str, guild: u64, user: u64, day: &str, guesses: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            &format!("UPDATE {table} SET guesses = ?1 WHERE guild_id = ?2 AND user_id = ?3 AND day = ?4"),
            params![guesses, guild as i64, user as i64, day],
        )?;
        Ok(())
    }

    /// Все партии дня на сервере: (участник, загаданное слово, догадки через запятую).
    pub fn word_day_rows(&self, table: &str, guild: u64, day: &str) -> rusqlite::Result<Vec<(u64, String, String)>> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(&format!("SELECT user_id, answer, guesses FROM {table} WHERE guild_id = ?1 AND day = ?2 ORDER BY rowid"))?;
        let rows = statement
            .query_map(params![guild as i64, day], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get::<_, Option<String>>(2)?.unwrap_or_default()))
            })?
            .collect();
        rows
    }

    /// Дни, в которые на сервере кто-то сделал хотя бы один ход.
    pub fn word_played_days(&self, table: &str, guild: u64) -> rusqlite::Result<Vec<String>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare(&format!("SELECT DISTINCT day FROM {table} WHERE guild_id = ?1 AND guesses != ''"))?;
        let days = statement.query_map(params![guild as i64], |row| row.get(0))?.collect();
        days
    }

    /// Отмечает, что итоги вчерашнего дня на сервере уже написаны. true — отметили только что (значит, пора писать).
    pub fn mark_word_summary(&self, table: &str, guild: u64, day: &str) -> rusqlite::Result<bool> {
        let added = self.lock().execute(
            &format!("INSERT OR IGNORE INTO {table} (guild_id, day) VALUES (?1, ?2)"),
            params![guild as i64, day],
        )?;
        Ok(added > 0)
    }

    /// Что Викисловарь уже ответил про это слово: Some(true) — настоящее, Some(false) — такого нет, None — не спрашивали.
    pub fn word_check(&self, word: &str) -> rusqlite::Result<Option<bool>> {
        let valid = self
            .lock()
            .query_row("SELECT valid FROM word_check WHERE word = ?1", params![word], |row| row.get::<_, Option<i64>>(0))
            .optional()?;
        Ok(valid.flatten().map(|valid| valid != 0))
    }

    pub fn set_word_check(&self, word: &str, valid: bool) -> rusqlite::Result<()> {
        self.lock().execute("INSERT OR REPLACE INTO word_check (word, valid) VALUES (?1, ?2)", params![word, valid as i64])?;
        Ok(())
    }

    /// Прибавляет монеты по id участника, когда его имени под рукой нет (кошелёк уже должен быть).
    pub fn add_money_by_id(&self, guild: u64, user: u64, amount: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE economy SET balance = balance + ?1 WHERE guild_id = ?2 AND user_id = ?3",
            params![amount, guild as i64, user as i64],
        )?;
        Ok(())
    }

    /// Партии слова дня с ходами: (участник, день, загаданное слово, догадки), по дням.
    /// user — только этого участника, month — только за месяц вида "2026-11".
    pub fn word_results(
        &self,
        table: &str,
        guild: u64,
        user: Option<u64>,
        month: Option<&str>,
    ) -> rusqlite::Result<Vec<(u64, String, String, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT user_id, day, answer, guesses FROM {table}
             WHERE guild_id = ?1 AND guesses != '' AND (?2 IS NULL OR user_id = ?2) AND (?3 IS NULL OR day LIKE ?3)
             ORDER BY day"
        ))?;
        let rows = statement
            .query_map(params![guild as i64, user.map(|user| user as i64), month.map(|month| format!("{month}-%"))], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect();
        rows
    }

    pub fn add_word_duel(&self, guild: u64, channel: u64, lang: &str, day: &str, user1: u64, user2: u64, bet: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO word_duels (guild_id, channel_id, lang, day, user1, user2, bet) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![guild as i64, channel as i64, lang, day, user1 as i64, user2 as i64, bet],
        )?;
        Ok(())
    }

    /// Все дуэли, по которым ещё не подведён итог.
    pub fn word_duels(&self) -> rusqlite::Result<Vec<WordDuel>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT id, guild_id, channel_id, lang, day, user1, user2, bet FROM word_duels ORDER BY id")?;
        let duels = statement
            .query_map([], |row| {
                Ok(WordDuel {
                    id: row.get(0)?,
                    guild: row.get::<_, i64>(1)? as u64,
                    channel: row.get::<_, i64>(2)? as u64,
                    lang: row.get(3)?,
                    day: row.get(4)?,
                    user1: row.get::<_, i64>(5)? as u64,
                    user2: row.get::<_, i64>(6)? as u64,
                    bet: row.get(7)?,
                })
            })?
            .collect();
        duels
    }

    /// Убирает дуэль. true — убрали сейчас, false — её уже посчитал кто-то другой.
    pub fn delete_word_duel(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self.lock().execute("DELETE FROM word_duels WHERE id = ?1", params![id])? > 0)
    }

    /// Есть ли у участника дуэль на слово этого дня в этой игре.
    pub fn in_word_duel(&self, guild: u64, lang: &str, day: &str, user: u64) -> rusqlite::Result<bool> {
        let found = self
            .lock()
            .query_row(
                "SELECT 1 FROM word_duels WHERE guild_id = ?1 AND lang = ?2 AND day = ?3 AND ?4 IN (user1, user2)",
                params![guild as i64, lang, day, user as i64],
                |_| Ok(()),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Шуточная игра: (размер, время последней попытки). Новичок начинает с 0 см.
    pub fn dick(&self, guild: u64, user: u64) -> rusqlite::Result<(i64, f64)> {
        let row = self
            .lock()
            .query_row(
                "SELECT size, last_growth FROM dick WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| Ok((row.get::<_, Option<i64>>(0)?.unwrap_or(0), row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
            )
            .optional()?;
        Ok(row.unwrap_or((0, 0.0)))
    }

    pub fn set_dick(&self, guild: u64, user: u64, name: &str, size: i64, time: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO dick (guild_id, user_id, name, size, last_growth) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET
                 name = excluded.name, size = excluded.size, last_growth = excluded.last_growth",
            params![guild as i64, user as i64, name, size, time],
        )?;
        Ok(())
    }

    /// Место в рейтинге игры = сколько участников с размером больше, плюс один.
    pub fn dick_position(&self, guild: u64, size: i64) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM dick WHERE guild_id = ?1 AND size > ?2",
            params![guild as i64, size],
            |row| Ok(row.get::<_, i64>(0)? + 1),
        )
    }

    pub fn top_dick(&self, guild: u64) -> rusqlite::Result<Vec<(u64, String, i64)>> {
        self.top("SELECT user_id, name, size FROM dick WHERE guild_id = ?1 ORDER BY size DESC, name LIMIT 10", guild)
    }

    // ---------- Напоминания ----------

    pub fn add_reminder(&self, guild: u64, channel: u64, user: u64, due: f64, text: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO reminders (guild_id, channel_id, user_id, due, text) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, channel as i64, user as i64, due, text],
        )?;
        Ok(())
    }

    pub fn count_reminders(&self, guild: u64, user: u64) -> rusqlite::Result<i64> {
        self.lock().query_row(
            "SELECT COUNT(*) FROM reminders WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |row| row.get(0),
        )
    }

    /// Напоминания человека, ближайшее — первым: (номер в базе, когда, текст).
    pub fn user_reminders(&self, guild: u64, user: u64) -> rusqlite::Result<Vec<(i64, f64, String)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT id, due, text FROM reminders WHERE guild_id = ?1 AND user_id = ?2 ORDER BY due")?;
        let rows = statement.query_map(params![guild as i64, user as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect();
        rows
    }

    pub fn delete_reminder(&self, id: i64) -> rusqlite::Result<()> {
        self.lock().execute("DELETE FROM reminders WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Напоминания, у которых пришло время: (номер в базе, канал, участник, текст).
    pub fn due_reminders(&self, now: f64) -> rusqlite::Result<Vec<(i64, u64, u64, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT id, channel_id, user_id, text FROM reminders WHERE due <= ?1 ORDER BY due")?;
        let rows = statement
            .query_map(params![now], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64, row.get(3)?)))?
            .collect();
        rows
    }

    // ---------- Опросы ----------

    pub fn add_poll(&self, message: u64, channel: u64, author: u64, question: &str, options: &str, ends: f64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO polls (message_id, channel_id, author_id, question, options, ends) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![message as i64, channel as i64, author as i64, question, options, ends],
        )?;
        Ok(())
    }

    /// Опрос по сообщению: (вопрос, варианты в виде JSON, когда закроется, автор).
    pub fn poll(&self, message: u64) -> rusqlite::Result<Option<(String, String, f64, u64)>> {
        self.lock()
            .query_row(
                "SELECT question, options, ends, author_id FROM polls WHERE message_id = ?1",
                params![message as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get::<_, i64>(3)? as u64)),
            )
            .optional()
    }

    /// Голос участника (новый голос заменяет прежний).
    pub fn set_poll_vote(&self, message: u64, user: u64, option: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO poll_votes (message_id, user_id, option) VALUES (?1, ?2, ?3)",
            params![message as i64, user as i64, option],
        )?;
        Ok(())
    }

    /// Сколько голосов у каждого варианта: номер варианта → голосов.
    pub fn poll_votes(&self, message: u64) -> rusqlite::Result<std::collections::HashMap<i64, i64>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT option, COUNT(*) FROM poll_votes WHERE message_id = ?1 GROUP BY option")?;
        let votes = statement.query_map(params![message as i64], |row| Ok((row.get(0)?, row.get(1)?)))?.collect();
        votes
    }

    /// Опросы, у которых вышло время: (сообщение, канал, автор, вопрос, варианты в виде JSON, когда закрылся).
    #[allow(clippy::type_complexity)]
    pub fn finished_polls(&self, now: f64) -> rusqlite::Result<Vec<(u64, u64, u64, String, String, f64)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT message_id, channel_id, author_id, question, options, ends FROM polls WHERE ends <= ?1")?;
        let rows = statement
            .query_map(params![now], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64, row.get::<_, i64>(2)? as u64,
                    row.get(3)?, row.get(4)?, row.get(5)?))
            })?
            .collect();
        rows
    }

    pub fn delete_poll(&self, message: u64) -> rusqlite::Result<()> {
        let connection = self.lock();
        connection.execute("DELETE FROM polls WHERE message_id = ?1", params![message as i64])?;
        connection.execute("DELETE FROM poll_votes WHERE message_id = ?1", params![message as i64])?;
        Ok(())
    }

    // ---------- Настройки каналов ----------

    /// Выбирает канал для игр (None — снять ограничение).
    pub fn set_games_channel(&self, guild: u64, channel: Option<u64>) -> rusqlite::Result<()> {
        self.set_channel("games_channel", guild, channel)
    }

    /// Канал для логов ошибок на сервере или None.
    pub fn log_channel(&self, guild: u64) -> rusqlite::Result<Option<u64>> {
        let channel = self
            .lock()
            .query_row("SELECT channel_id FROM log_channel WHERE guild_id = ?1", params![guild as i64], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .optional()?;
        Ok(channel.flatten().map(|id| id as u64))
    }

    pub fn set_log_channel(&self, guild: u64, channel: Option<u64>) -> rusqlite::Result<()> {
        self.set_channel("log_channel", guild, channel)
    }

    fn set_channel(&self, table: &str, guild: u64, channel: Option<u64>) -> rusqlite::Result<()> {
        let connection = self.lock();
        match channel {
            Some(channel) => connection.execute(
                &format!("INSERT INTO {table} (guild_id, channel_id) VALUES (?1, ?2)
                          ON CONFLICT (guild_id) DO UPDATE SET channel_id = excluded.channel_id"),
                params![guild as i64, channel as i64],
            )?,
            None => connection.execute(&format!("DELETE FROM {table} WHERE guild_id = ?1"), params![guild as i64])?,
        };
        Ok(())
    }

    // ---------- Патчноуты ----------

    /// Каналы для патчноутов на всех серверах: (сервер, канал, номер последнего опубликованного патчноута).
    pub fn patchnotes_channels(&self) -> rusqlite::Result<Vec<(u64, u64, Option<String>)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT guild_id, channel_id, last_note FROM patchnotes_channel")?;
        let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64, row.get(2)?)))?.collect();
        rows
    }

    /// Выбирает канал для патчноутов: (канал, номер последнего патчноута). None — больше не писать.
    pub fn set_patchnotes_channel(&self, guild: u64, channel: Option<(u64, Option<&str>)>) -> rusqlite::Result<()> {
        let connection = self.lock();
        match channel {
            Some((channel, last_note)) => connection.execute(
                "INSERT INTO patchnotes_channel (guild_id, channel_id, last_note) VALUES (?1, ?2, ?3)
                 ON CONFLICT (guild_id) DO UPDATE SET channel_id = excluded.channel_id, last_note = excluded.last_note",
                params![guild as i64, channel as i64, last_note],
            )?,
            None => connection.execute("DELETE FROM patchnotes_channel WHERE guild_id = ?1", params![guild as i64])?,
        };
        Ok(())
    }

    pub fn set_last_patchnote(&self, guild: u64, note: &str) -> rusqlite::Result<()> {
        self.lock().execute("UPDATE patchnotes_channel SET last_note = ?1 WHERE guild_id = ?2", params![note, guild as i64])?;
        Ok(())
    }

    // ---------- Погода ----------

    /// Города сервера в том порядке, в каком их добавляли: (название, широта, долгота).
    pub fn weather_cities(&self, guild: u64) -> rusqlite::Result<Vec<(String, f64, f64)>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT name, latitude, longitude FROM weather_cities WHERE guild_id = ?1 ORDER BY rowid")?;
        let rows = statement.query_map(params![guild as i64], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect();
        rows
    }

    /// Перезаписывает список городов сервера.
    pub fn save_weather_cities(&self, guild: u64, cities: &[(String, f64, f64)]) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM weather_cities WHERE guild_id = ?1", params![guild as i64])?;
        for (name, latitude, longitude) in cities {
            transaction.execute(
                "INSERT INTO weather_cities (guild_id, name, latitude, longitude) VALUES (?1, ?2, ?3, ?4)",
                params![guild as i64, name, latitude, longitude],
            )?;
        }
        transaction.commit()
    }

    // ---------- Варны ----------

    /// Записывает варн. Возвращает, сколько всего варнов у человека.
    #[allow(dead_code)] // понадобится модерации через ИИ и анти-спаму
    pub fn add_warn(&self, guild: u64, user: u64, moderator: u64, reason: &str, time: f64) -> rusqlite::Result<i64> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO warns (guild_id, user_id, moderator_id, reason, created) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, user as i64, moderator as i64, reason, time],
        )?;
        connection.query_row(
            "SELECT COUNT(*) FROM warns WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |row| row.get(0),
        )
    }

    /// Последние 10 варнов человека (кто выдал, причина, когда) и сколько их всего.
    #[allow(clippy::type_complexity)]
    pub fn warns(&self, guild: u64, user: u64) -> rusqlite::Result<(Vec<(u64, String, f64)>, i64)> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT moderator_id, reason, created FROM warns WHERE guild_id = ?1 AND user_id = ?2 ORDER BY id DESC LIMIT 10",
        )?;
        let rows: rusqlite::Result<Vec<(u64, String, f64)>> = statement
            .query_map(params![guild as i64, user as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<f64>>(2)?.unwrap_or(0.0)))
            })?
            .collect();
        let total = connection.query_row(
            "SELECT COUNT(*) FROM warns WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |row| row.get(0),
        )?;
        Ok((rows?, total))
    }

    /// Снимает все варны. Возвращает, сколько их было.
    pub fn clear_warns(&self, guild: u64, user: u64) -> rusqlite::Result<usize> {
        self.lock().execute("DELETE FROM warns WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user as i64])
    }

    // ---------- ИИ ----------

    /// Последние сообщения разговора с ботом в канале: (роль, текст), от старых к новым.
    /// Роль "user" — сообщение человека, "model" — ответ бота. Разговор всегда начинается с вопроса человека.
    pub fn ai_history(&self, channel: u64, limit: i64) -> rusqlite::Result<Vec<(String, String)>> {
        let connection = self.lock();
        let mut statement = connection.prepare("SELECT role, text FROM ai_history WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2")?;
        let rows: rusqlite::Result<Vec<(String, String)>> =
            statement.query_map(params![channel as i64, limit], |row| Ok((row.get(0)?, row.get(1)?)))?.collect();
        let mut rows = rows?;
        rows.reverse(); // из базы достали от новых к старым
        if rows.first().is_some_and(|(role, _)| role == "model") {
            rows.remove(0);
        }
        Ok(rows)
    }

    /// Запоминает пару «вопрос — ответ» и удаляет из памяти канала всё, что старше limit сообщений.
    pub fn save_ai_history(&self, channel: u64, question: &str, answer: &str, time: f64, limit: i64) -> rusqlite::Result<()> {
        let connection = self.lock();
        for (role, text) in [("user", question), ("model", answer)] {
            connection.execute(
                "INSERT INTO ai_history (channel_id, role, text, created) VALUES (?1, ?2, ?3, ?4)",
                params![channel as i64, role, text, time],
            )?;
        }
        connection.execute(
            "DELETE FROM ai_history WHERE channel_id = ?1 AND id NOT IN (
                 SELECT id FROM ai_history WHERE channel_id = ?1 ORDER BY id DESC LIMIT ?2
             )",
            params![channel as i64, limit],
        )?;
        Ok(())
    }

    /// Стирает память разговора в канале. Возвращает, сколько сообщений удалено.
    pub fn clear_ai_history(&self, channel: u64) -> rusqlite::Result<usize> {
        self.lock().execute("DELETE FROM ai_history WHERE channel_id = ?1", params![channel as i64])
    }

    /// Засчитывает один запрос к Google за день day (прошлые дни из счётчика убираются).
    pub fn count_ai_request(&self, day: &str) -> rusqlite::Result<()> {
        let connection = self.lock();
        connection.execute("DELETE FROM ai_stats WHERE day != ?1", params![day])?;
        connection.execute(
            "INSERT INTO ai_stats (day, requests) VALUES (?1, 1) ON CONFLICT (day) DO UPDATE SET requests = requests + 1",
            params![day],
        )?;
        Ok(())
    }

    /// Сколько запросов к Google бот отправил за день day.
    pub fn ai_requests(&self, day: &str) -> rusqlite::Result<i64> {
        let requests = self
            .lock()
            .query_row("SELECT requests FROM ai_stats WHERE day = ?1", params![day], |row| row.get::<_, Option<i64>>(0))
            .optional()?;
        Ok(requests.flatten().unwrap_or(0))
    }

    /// +1 к счётчику грубостей подряд (если человек давно не грубил — счёт с нуля). Возвращает новое значение.
    pub fn add_rude_strike(&self, guild: u64, user: u64, now: f64, reset_after: f64) -> rusqlite::Result<i64> {
        let connection = self.lock();
        let row = connection
            .query_row(
                "SELECT count, last_time FROM rude_strikes WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| Ok((row.get::<_, Option<i64>>(0)?.unwrap_or(0), row.get::<_, Option<f64>>(1)?.unwrap_or(0.0))),
            )
            .optional()?;
        let count = row.filter(|(_, last)| now - last < reset_after).map_or(0, |(count, _)| count) + 1;
        connection.execute(
            "INSERT INTO rude_strikes (guild_id, user_id, count, last_time) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET count = excluded.count, last_time = excluded.last_time",
            params![guild as i64, user as i64, count, now],
        )?;
        Ok(count)
    }

    /// Человек нормально обратился к боту — грубости больше не «подряд» (время последней грубости остаётся).
    pub fn reset_rude_strikes(&self, guild: u64, user: u64) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE rude_strikes SET count = 0 WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
        )?;
        Ok(())
    }

    /// Когда человек последний раз грубил боту (None — не грубил).
    pub fn last_rude_time(&self, guild: u64, user: u64) -> rusqlite::Result<Option<f64>> {
        let time = self
            .lock()
            .query_row(
                "SELECT last_time FROM rude_strikes WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |row| row.get::<_, Option<f64>>(0),
            )
            .optional()?;
        Ok(time.flatten())
    }

    // ---------- Приветствие новичков ----------

    pub fn welcome_channel(&self, guild: u64) -> rusqlite::Result<Option<u64>> {
        let channel = self
            .lock()
            .query_row("SELECT channel_id FROM welcome_channel WHERE guild_id = ?1", params![guild as i64], |row| row.get::<_, Option<i64>>(0))
            .optional()?;
        Ok(channel.flatten().map(|id| id as u64))
    }

    pub fn set_welcome_channel(&self, guild: u64, channel: Option<u64>) -> rusqlite::Result<()> {
        let connection = self.lock();
        match channel {
            Some(channel) => connection.execute(
                "INSERT OR REPLACE INTO welcome_channel (guild_id, channel_id) VALUES (?1, ?2)",
                params![guild as i64, channel as i64],
            )?,
            None => connection.execute("DELETE FROM welcome_channel WHERE guild_id = ?1", params![guild as i64])?,
        };
        Ok(())
    }

    // ---------- Утренние и вечерние сообщения ----------

    /// Все серверы с включёнными сообщениями.
    pub fn daily_settings(&self) -> rusqlite::Result<Vec<DailySettings>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT guild_id, channel_id, timezone, morning, evening, last_morning, last_evening FROM daily_settings",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DailySettings {
                guild: row.get::<_, i64>(0)? as u64,
                channel: row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                timezone: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                morning: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                evening: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                last_morning: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                last_evening: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            })
        })?;
        rows.collect()
    }

    pub fn set_daily_settings(&self, settings: &DailySettings) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO daily_settings (guild_id, channel_id, timezone, morning, evening, last_morning, last_evening)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (guild_id) DO UPDATE SET
                 channel_id = excluded.channel_id, timezone = excluded.timezone,
                 morning = excluded.morning, evening = excluded.evening,
                 last_morning = excluded.last_morning, last_evening = excluded.last_evening",
            params![settings.guild as i64, settings.channel as i64, settings.timezone, settings.morning, settings.evening,
                    settings.last_morning, settings.last_evening],
        )?;
        Ok(())
    }

    /// Выключает сообщения. Возвращает, были ли они включены.
    pub fn delete_daily_settings(&self, guild: u64) -> rusqlite::Result<bool> {
        Ok(self.lock().execute("DELETE FROM daily_settings WHERE guild_id = ?1", params![guild as i64])? > 0)
    }

    /// Отмечает, что сообщение (morning — утреннее, иначе вечернее) за день day отправлено.
    pub fn mark_daily_sent(&self, guild: u64, morning: bool, day: &str) -> rusqlite::Result<()> {
        let sql = if morning {
            "UPDATE daily_settings SET last_morning = ?1 WHERE guild_id = ?2"
        } else {
            "UPDATE daily_settings SET last_evening = ?1 WHERE guild_id = ?2"
        };
        self.lock().execute(sql, params![day, guild as i64])?;
        Ok(())
    }

    pub fn set_daily_bonus(&self, guild: u64, user: u64, day: &str, streak: i64) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO daily_bonus (guild_id, user_id, last_day, streak) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET last_day = excluded.last_day, streak = excluded.streak",
            params![guild as i64, user as i64, day, streak],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_and_money() {
        let db = Database::in_memory().unwrap();
        assert_eq!(db.wallet(7, 1, "Alice", 500).unwrap(), (500, 0.0));
        assert_eq!(db.add_money(7, 1, "Alice", 500, 250).unwrap(), 750);
        assert_eq!(db.add_money(7, 1, "Алиса", 500, -50).unwrap(), 700);
        assert_eq!(db.wallet(7, 2, "Bob", 500).unwrap().0, 500);
        assert_eq!(db.money_position(7, 500).unwrap(), 2); // Алиса богаче Боба
        assert_eq!(db.money_position(7, 700).unwrap(), 1);
        db.record_work(7, 2, 300, 1234.5).unwrap();
        assert_eq!(db.wallet(7, 2, "Bob", 500).unwrap(), (800, 1234.5));
        // другой сервер — другой кошелёк
        assert_eq!(db.wallet(8, 1, "Alice", 500).unwrap().0, 500);
    }

    #[test]
    fn pets_rows() {
        let db = Database::in_memory().unwrap();
        let cat = db.add_pet(7, 1, "valera", 100.0, 500).unwrap();
        let dog = db.add_pet(7, 1, "sergey", 200.0, 7500).unwrap();
        db.add_pet(7, 2, "igor", 300.0, 2000).unwrap();
        assert_eq!(db.user_pets(7, 1).unwrap().iter().map(|pet| pet.id).collect::<Vec<_>>(), [cat, dog]);
        assert_eq!(db.own_pet(dog, 7, 2).unwrap(), None); // чужой питомец
        db.feed_pet(cat, 200, 150.0).unwrap();
        assert_eq!(db.own_pet(cat, 7, 1).unwrap(),
                   Some(Pet { id: cat, species: "valera".into(), level: 2, last_collect: 150.0, spent: 700 }));
        db.mark_pets_collected(7, 1, 999.0).unwrap();
        assert!(db.user_pets(7, 1).unwrap().iter().all(|pet| pet.last_collect == 999.0));
        assert_eq!(db.user_pets(7, 2).unwrap()[0].last_collect, 300.0);
        db.delete_pet(cat).unwrap();
        assert_eq!(db.user_pets(7, 1).unwrap().len(), 1);
    }

    #[test]
    fn pet_swap() {
        let db = Database::in_memory().unwrap();
        let cat = db.add_pet(7, 1, "valera", 0.0, 0).unwrap();
        let wolf = db.add_pet(7, 2, "igor", 0.0, 0).unwrap();
        assert!(db.swap_pets(7, (cat, 1), (wolf, 2)).unwrap());
        assert_eq!(db.user_pets(7, 1).unwrap()[0].species, "igor");
        assert_eq!(db.user_pets(7, 2).unwrap()[0].species, "valera");
        // питомцы уже у других хозяев — повторный обмен в ту же сторону не проходит
        assert!(!db.swap_pets(7, (cat, 1), (wolf, 2)).unwrap());
        db.delete_pet(cat).unwrap();
        assert!(!db.swap_pets(7, (wolf, 1), (cat, 2)).unwrap());
        assert_eq!(db.user_pets(7, 1).unwrap().len(), 1);
    }

    #[test]
    fn shop_rows() {
        let db = Database::in_memory().unwrap();
        db.set_shop_role(7, 20, 900, 0).unwrap();
        db.set_shop_role(7, 10, 300, 7).unwrap();
        db.set_shop_role(7, 10, 250, 7).unwrap(); // новая цена
        assert_eq!(db.shop_roles(7).unwrap(), [(10, 250, 7), (20, 900, 0)]);
        assert!(db.remove_shop_role(7, 20).unwrap());
        assert!(!db.remove_shop_role(7, 20).unwrap());
        assert_eq!(db.shop_purchase(7, 1, 10).unwrap(), None);
        db.set_shop_purchase(7, 1, 10, 500.0).unwrap();
        db.set_shop_purchase(7, 2, 10, 5000.0).unwrap();
        assert_eq!(db.shop_purchase(7, 1, 10).unwrap(), Some(500.0));
        assert_eq!(db.expired_shop_purchases(1000.0).unwrap(), [(7, 1, 10)]);
        db.remove_shop_purchase(7, 1, 10).unwrap();
        assert!(db.expired_shop_purchases(1000.0).unwrap().is_empty());
    }

    #[test]
    fn levels_rows() {
        let db = Database::in_memory().unwrap();
        assert_eq!((db.xp(7, 1).unwrap(), db.last_xp_time(7, 1).unwrap()), (0, 0.0));
        db.set_xp(7, 1, "Alice", 120).unwrap();
        db.set_xp(7, 2, "Bob", 500).unwrap();
        db.set_xp(7, 1, "Алиса", 300).unwrap();
        db.set_last_xp_time(7, 1, 42.5).unwrap();
        assert_eq!((db.xp(7, 1).unwrap(), db.last_xp_time(7, 1).unwrap()), (300, 42.5));
        assert_eq!((db.xp_position(7, 300).unwrap(), db.xp_position(7, 500).unwrap()), (2, 1));
        assert_eq!(db.top_levels(7).unwrap(), [(2, "Bob".to_string(), 500), (1, "Алиса".to_string(), 300)]);
        db.wallet(7, 1, "Алиса", 500).unwrap();
        db.add_money(7, 2, "Bob", 500, 100).unwrap();
        assert_eq!(db.top_money(7).unwrap()[0], (2, "Bob".to_string(), 600));
        assert_eq!(db.games_channel(7).unwrap(), None);
    }

    #[test]
    fn word_rows() {
        let db = Database::in_memory().unwrap();
        for table in ["word_daily", "word_daily_ru"] {
            assert_eq!(db.word_row(table, 7, 1, "2026-11-11").unwrap(), None);
            assert_eq!(db.word_any_answer(table, 7, "2026-11-11").unwrap(), None);
            db.word_start(table, 7, 1, "2026-11-11", "apfel").unwrap();
            db.word_start(table, 7, 1, "2026-11-11", "blume").unwrap(); // вторая попытка начать — остаётся первая
            assert_eq!(db.word_row(table, 7, 1, "2026-11-11").unwrap(), Some(("apfel".into(), String::new())));
            assert_eq!(db.word_any_answer(table, 7, "2026-11-11").unwrap(), Some("apfel".into()));
            assert!(db.word_played_days(table, 7).unwrap().is_empty()); // ходов ещё не было
            db.set_word_guesses(table, 7, 1, "2026-11-11", "kerze,apfel").unwrap();
            db.word_start(table, 7, 2, "2026-11-11", "apfel").unwrap();
            assert_eq!(db.word_day_rows(table, 7, "2026-11-11").unwrap(),
                       [(1, "apfel".to_string(), "kerze,apfel".to_string()), (2, "apfel".to_string(), String::new())]);
            assert_eq!(db.word_played_days(table, 7).unwrap(), ["2026-11-11"]);
        }
        assert!(db.mark_word_summary("word_summary", 7, "2026-11-11").unwrap());
        assert!(!db.mark_word_summary("word_summary", 7, "2026-11-11").unwrap());
        assert!(db.mark_word_summary("word_summary_ru", 7, "2026-11-11").unwrap());
        assert_eq!(db.word_check("kzrze").unwrap(), None);
        db.set_word_check("kzrze", false).unwrap();
        db.set_word_check("kerze", true).unwrap();
        assert_eq!((db.word_check("kzrze").unwrap(), db.word_check("kerze").unwrap()), (Some(false), Some(true)));
    }

    #[test]
    fn duel_and_growth_rows() {
        let db = Database::in_memory().unwrap();
        db.word_start("word_daily", 7, 1, "2026-11-10", "apfel").unwrap();
        db.set_word_guesses("word_daily", 7, 1, "2026-11-10", "kerze,apfel").unwrap();
        db.word_start("word_daily", 7, 1, "2026-12-01", "blume").unwrap();
        db.set_word_guesses("word_daily", 7, 1, "2026-12-01", "blume").unwrap();
        db.word_start("word_daily", 7, 2, "2026-11-10", "apfel").unwrap(); // без ходов — не считается
        assert_eq!(db.word_results("word_daily", 7, None, None).unwrap().len(), 2);
        assert_eq!(db.word_results("word_daily", 7, Some(1), Some("2026-11")).unwrap(),
                   [(1, "2026-11-10".to_string(), "apfel".to_string(), "kerze,apfel".to_string())]);
        assert!(db.word_results("word_daily", 7, Some(2), None).unwrap().is_empty());

        assert!(!db.in_word_duel(7, "de", "2026-11-10", 1).unwrap());
        db.add_word_duel(7, 55, "de", "2026-11-10", 1, 2, 200).unwrap();
        assert!(db.in_word_duel(7, "de", "2026-11-10", 2).unwrap());
        assert!(!db.in_word_duel(7, "ru", "2026-11-10", 2).unwrap());
        let duel = db.word_duels().unwrap().remove(0);
        assert_eq!((duel.guild, duel.channel, duel.user1, duel.user2, duel.bet), (7, 55, 1, 2, 200));
        assert!(db.delete_word_duel(duel.id).unwrap());
        assert!(!db.delete_word_duel(duel.id).unwrap());

        db.wallet(7, 1, "Alice", 500).unwrap();
        db.add_money_by_id(7, 1, 400).unwrap();
        assert_eq!(db.wallet(7, 1, "Alice", 500).unwrap().0, 900);

        assert_eq!(db.dick(7, 1).unwrap(), (0, 0.0));
        db.set_dick(7, 1, "Alice", 12, 100.0).unwrap();
        db.set_dick(7, 2, "Bob", 30, 100.0).unwrap();
        db.set_dick(7, 1, "Alice", 15, 200.0).unwrap();
        assert_eq!(db.dick(7, 1).unwrap(), (15, 200.0));
        assert_eq!((db.dick_position(7, 15).unwrap(), db.dick_position(7, 30).unwrap()), (2, 1));
        assert_eq!(db.top_dick(7).unwrap()[0], (2, "Bob".to_string(), 30));
    }

    #[test]
    fn reminders_and_polls() {
        let db = Database::in_memory().unwrap();
        db.add_reminder(7, 55, 1, 2000.0, "позже").unwrap();
        db.add_reminder(7, 55, 1, 1000.0, "раньше").unwrap();
        db.add_reminder(7, 55, 2, 500.0, "чужое").unwrap();
        assert_eq!(db.count_reminders(7, 1).unwrap(), 2);
        let mine = db.user_reminders(7, 1).unwrap();
        assert_eq!(mine.iter().map(|(_, _, text)| text.as_str()).collect::<Vec<_>>(), ["раньше", "позже"]);
        let due = db.due_reminders(1500.0).unwrap();
        assert_eq!(due.iter().map(|(_, channel, user, text)| (*channel, *user, text.as_str())).collect::<Vec<_>>(),
                   [(55, 2, "чужое"), (55, 1, "раньше")]);
        db.delete_reminder(mine[0].0).unwrap();
        assert_eq!(db.count_reminders(7, 1).unwrap(), 1);

        db.add_poll(900, 55, 1, "Что едим?", r#"["а","б"]"#, 5000.0).unwrap();
        assert_eq!(db.poll(900).unwrap().unwrap().0, "Что едим?");
        assert_eq!(db.poll(901).unwrap(), None);
        db.set_poll_vote(900, 1, 0).unwrap();
        db.set_poll_vote(900, 2, 0).unwrap();
        db.set_poll_vote(900, 2, 1).unwrap(); // передумал
        let votes = db.poll_votes(900).unwrap();
        assert_eq!((votes.get(&0), votes.get(&1)), (Some(&1), Some(&1)));
        assert!(db.finished_polls(4000.0).unwrap().is_empty());
        assert_eq!(db.finished_polls(6000.0).unwrap()[0].0, 900);
        db.delete_poll(900).unwrap();
        assert!(db.poll(900).unwrap().is_none() && db.poll_votes(900).unwrap().is_empty());
    }

    #[test]
    fn channels_notes_cities_warns() {
        let db = Database::in_memory().unwrap();
        db.set_games_channel(7, Some(55)).unwrap();
        db.set_games_channel(7, Some(66)).unwrap();
        assert_eq!(db.games_channel(7).unwrap(), Some(66));
        db.set_games_channel(7, None).unwrap();
        assert_eq!(db.games_channel(7).unwrap(), None);
        db.set_log_channel(7, Some(77)).unwrap();
        assert_eq!((db.log_channel(7).unwrap(), db.log_channel(8).unwrap()), (Some(77), None));

        db.set_patchnotes_channel(7, Some((55, Some("v9")))).unwrap();
        db.set_patchnotes_channel(8, Some((56, None))).unwrap();
        db.set_last_patchnote(7, "v10").unwrap();
        let mut channels = db.patchnotes_channels().unwrap();
        channels.sort();
        assert_eq!(channels, [(7, 55, Some("v10".to_string())), (8, 56, None)]);
        db.set_patchnotes_channel(8, None).unwrap();
        assert_eq!(db.patchnotes_channels().unwrap().len(), 1);

        assert!(db.weather_cities(7).unwrap().is_empty());
        db.save_weather_cities(7, &[("📍 Берлин".into(), 52.5, 13.4), ("📍 Рим".into(), 41.9, 12.5)]).unwrap();
        db.save_weather_cities(7, &[("📍 Рим".into(), 41.9, 12.5)]).unwrap();
        assert_eq!(db.weather_cities(7).unwrap(), [("📍 Рим".to_string(), 41.9, 12.5)]);

        assert_eq!(db.add_warn(7, 1, 99, "спам", 100.0).unwrap(), 1);
        assert_eq!(db.add_warn(7, 1, 99, "флуд", 200.0).unwrap(), 2);
        let (rows, total) = db.warns(7, 1).unwrap();
        assert_eq!((total, rows[0].clone()), (2, (99, "флуд".to_string(), 200.0))); // последний — первым
        assert_eq!(db.clear_warns(7, 1).unwrap(), 2);
        assert_eq!(db.warns(7, 1).unwrap().1, 0);
    }

    #[test]
    fn ai_rows() {
        let db = Database::in_memory().unwrap();
        assert!(db.ai_history(55, 4).unwrap().is_empty());
        db.save_ai_history(55, "Alice: привет", "бака", 1.0, 4).unwrap();
        db.save_ai_history(55, "Alice: как дела", "нормально", 2.0, 4).unwrap();
        db.save_ai_history(56, "Bob: эй", "что", 2.0, 4).unwrap();
        assert_eq!(db.ai_history(55, 4).unwrap().len(), 4);
        db.save_ai_history(55, "Alice: ещё", "ладно", 3.0, 4).unwrap(); // старше 4 сообщений — забывается
        let history = db.ai_history(55, 4).unwrap();
        assert_eq!(history[0], ("user".to_string(), "Alice: как дела".to_string()));
        assert_eq!(history.len(), 4);
        // если в память влезло нечётное число — разговор всё равно начинается с вопроса человека
        assert_eq!(db.ai_history(55, 3).unwrap()[0].0, "user");
        assert_eq!(db.ai_history(55, 3).unwrap().len(), 2);
        assert_eq!(db.clear_ai_history(55).unwrap(), 4);
        assert_eq!(db.ai_history(56, 4).unwrap().len(), 2);

        assert_eq!(db.ai_requests("2026-11-10").unwrap(), 0);
        db.count_ai_request("2026-11-10").unwrap();
        db.count_ai_request("2026-11-10").unwrap();
        db.count_ai_request("2026-11-11").unwrap(); // новый день — прошлый убирается
        assert_eq!((db.ai_requests("2026-11-10").unwrap(), db.ai_requests("2026-11-11").unwrap()), (0, 1));

        assert_eq!(db.add_rude_strike(7, 1, 100.0, 3600.0).unwrap(), 1);
        assert_eq!(db.add_rude_strike(7, 1, 200.0, 3600.0).unwrap(), 2);
        db.reset_rude_strikes(7, 1).unwrap();
        assert_eq!(db.add_rude_strike(7, 1, 300.0, 3600.0).unwrap(), 1);
        assert_eq!(db.add_rude_strike(7, 1, 400.0, 3600.0).unwrap(), 2);
        assert_eq!(db.add_rude_strike(7, 1, 9000.0, 3600.0).unwrap(), 1); // давно не грубил — с нуля
    }

    #[test]
    fn daily_bonus_rows() {
        let db = Database::in_memory().unwrap();
        assert_eq!(db.daily_bonus(7, 1).unwrap(), None);
        db.set_daily_bonus(7, 1, "2026-11-10", 1).unwrap();
        db.set_daily_bonus(7, 1, "2026-11-11", 2).unwrap();
        assert_eq!(db.daily_bonus(7, 1).unwrap(), Some(("2026-11-11".to_string(), 2)));
    }
}
