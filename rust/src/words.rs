//! Игры в слова из 5 букв: Wortjagd — немецкие слова (/wort), «5 букв» — русские (/slovo).
//! Здесь правила, словари, проверка слов по Викисловарю и работа с базой. Мини-приложение — в miniapp.rs,
//! картинка и сообщения в чат — в wordboard.rs.
//!
//! У каждого 6 попыток. Слово дня — одно на весь сервер, за победу дают монеты (чем быстрее, тем больше).
//! Немецкое и русское слово дня — отдельные. Принимаются только настоящие слова: из словаря игры или из Викисловаря.

use crate::Data;
use chrono::NaiveDate;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const WORD_LENGTH: usize = 5;
pub const MAX_GUESSES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    De,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::De, Lang::Ru];

    pub fn from_key(key: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|lang| lang.key() == key)
    }

    /// Короткое имя языка — так он называется в мини-приложении.
    pub fn key(self) -> &'static str {
        ["de", "ru"][self as usize]
    }

    pub fn name(self) -> &'static str {
        ["Wortjagd", "5 букв"][self as usize]
    }

    pub fn command(self) -> &'static str {
        ["/wort", "/slovo"][self as usize]
    }

    /// «немецкое слово» / «русское слово»
    pub fn what(self) -> &'static str {
        ["немецкое слово", "русское слово"][self as usize]
    }

    fn language(self) -> &'static str {
        ["немецком", "русском"][self as usize]
    }

    fn letters(self) -> &'static str {
        ["abcdefghijklmnopqrstuvwxyzäöü", "абвгдежзийклмнопрстуфхцчшщъыьэюя"][self as usize]
    }

    /// Раздел в settings.ini
    fn section(self) -> &'static str {
        ["wortspiel", "slovo"][self as usize]
    }

    fn words_file(self) -> &'static str {
        ["german_words.txt", "russian_words.txt"][self as usize]
    }

    fn fallback(self) -> &'static [&'static str] {
        match self {
            Lang::De => &["apfel", "blume", "brief", "feuer", "fisch", "katze", "kraft", "licht", "nacht", "sonne",
                          "stein", "stern", "stuhl", "tisch", "traum", "vogel", "wagen", "wolke", "zunge", "zwerg"],
            Lang::Ru => &["арбуз", "берег", "ветер", "война", "дождь", "замок", "зебра", "игрок", "книга", "кошка",
                          "лампа", "лодка", "мечта", "огонь", "песня", "птица", "сосна", "сумка", "улица", "школа"],
        }
    }

    /// Таблица с партиями слова дня (те же таблицы, что у bot.py)
    pub fn table(self) -> &'static str {
        ["word_daily", "word_daily_ru"][self as usize]
    }

    pub fn summary_table(self) -> &'static str {
        ["word_summary", "word_summary_ru"][self as usize]
    }

    /// custom_id кнопки «Играть!» под картинкой в чате
    pub fn button(self) -> &'static str {
        ["wortjagd:play", "slovo:play"][self as usize]
    }

    /// День №1 игры — для номера на картинке
    pub fn epoch(self) -> NaiveDate {
        [NaiveDate::from_ymd_opt(2026, 11, 8), NaiveDate::from_ymd_opt(2026, 11, 9)][self as usize].unwrap()
    }

    fn wiki(self) -> (&'static str, &'static str) {
        [("https://de.wiktionary.org/w/api.php", "{{Sprache|Deutsch}}"), ("https://ru.wiktionary.org/w/api.php", "{{-ru-")]
            [self as usize]
    }
}

/// Как проверять, что слово существует.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Check {
    Wiktionary, // словарь игры + Викисловарь
    List,       // только словарь игры
    Off,        // любое сочетание из 5 букв
}

/// Настройки одной игры из settings.ini ([wortspiel] или [slovo]).
pub struct LangSettings {
    pub rewards: Vec<i64>, // монеты за победу с 1-й, 2-й … 6-й попытки
    pub check: Check,
    pub timezone: chrono_tz::Tz, // по какому времени наступает новый день
    pub announce: bool,          // показывать ли картинку и итоги в чате
}

pub struct WordSettings {
    langs: [LangSettings; 2],
}

impl WordSettings {
    pub fn load(settings: &crate::config::Settings) -> Self {
        let load = |lang: Lang, default_zone: chrono_tz::Tz| {
            let section = lang.section();
            let rewards = parse_numbers(&settings.text(section, "rewards", "500, 350, 250, 150, 100, 50"));
            LangSettings {
                rewards: if rewards.is_empty() { vec![0] } else { rewards },
                check: match settings.text(section, "check_words", "wiktionary").to_lowercase().as_str() {
                    "off" => Check::Off,
                    "list" => Check::List,
                    _ => Check::Wiktionary,
                },
                timezone: settings.text(section, "timezone", default_zone.name()).parse().unwrap_or(default_zone),
                announce: settings.text(section, "announce", "on").to_lowercase() == "on",
            }
        };
        Self { langs: [load(Lang::De, chrono_tz::Europe::Berlin), load(Lang::Ru, chrono_tz::Europe::Moscow)] }
    }

    pub fn get(&self, lang: Lang) -> &LangSettings {
        &self.langs[lang as usize]
    }
}

/// Все целые числа из строки: "500, 350 и 250" → [500, 350, 250].
fn parse_numbers(text: &str) -> Vec<i64> {
    text.split(|c: char| !c.is_ascii_digit()).filter(|part| !part.is_empty()).filter_map(|part| part.parse().ok()).collect()
}

/// Сегодняшний день игры, например "2026-11-08".
pub fn today(data: &Data, lang: Lang) -> String {
    chrono::Utc::now().with_timezone(&data.words.settings.get(lang).timezone).date_naive().to_string()
}

/// Как игрок написал слово → как его хранит игра: строчными, в русском ё = е.
pub fn normalize(text: &str, lang: Lang) -> String {
    let word = text.trim().to_lowercase();
    if lang == Lang::Ru { word.replace('ё', "е") } else { word }
}

fn fits(word: &str, lang: Lang) -> bool {
    word.chars().count() == WORD_LENGTH && word.chars().all(|c| lang.letters().contains(c))
}

/// Текст файла со словами → список слов из 5 букв нужного языка без повторов.
pub fn parse_words(text: &str, lang: Lang) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for line in text.lines().filter(|line| !line.trim_start().starts_with('#')) {
        for word in normalize(line, lang).split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
            if fits(word, lang) && !words.iter().any(|known| known == word) {
                words.push(word.to_string());
            }
        }
    }
    if words.is_empty() {
        words = lang.fallback().iter().map(|word| word.to_string()).collect();
    }
    words
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Correct, // буква на своём месте
    Present, // буква есть в слове, но в другом месте
    Absent,  // такой буквы нет
}

impl Mark {
    pub fn as_str(self) -> &'static str {
        ["correct", "present", "absent"][self as usize]
    }
}

/// Подсветка букв догадки. Повторы считаются честно: если в слове одна «e», а в догадке две — подсветится одна.
pub fn score_guess(guess: &str, answer: &str) -> Vec<Mark> {
    let (guess, answer): (Vec<char>, Vec<char>) = (guess.chars().collect(), answer.chars().collect());
    let mut marks = vec![Mark::Absent; guess.len()];
    let mut left: HashMap<char, usize> = HashMap::new(); // буквы ответа, ещё не «занятые» точными попаданиями
    for (index, (g, a)) in guess.iter().zip(&answer).enumerate() {
        if g == a {
            marks[index] = Mark::Correct;
        } else {
            *left.entry(*a).or_default() += 1;
        }
    }
    for (index, g) in guess.iter().enumerate() {
        if marks[index] != Mark::Correct {
            if let Some(count) = left.get_mut(g).filter(|count| **count > 0) {
                marks[index] = Mark::Present;
                *count -= 1;
            }
        }
    }
    marks
}

/// Партия слова дня: загаданное слово и догадки игрока.
#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub lang: Lang,
    pub day: String,
    pub answer: String,
    pub guesses: Vec<String>,
}

impl Game {
    pub fn from_row(lang: Lang, day: &str, answer: String, guesses: &str) -> Self {
        let guesses = guesses.split(',').filter(|guess| !guess.is_empty()).map(str::to_string).collect();
        Self { lang, day: day.to_string(), answer, guesses }
    }

    pub fn won(&self) -> bool {
        self.guesses.last() == Some(&self.answer)
    }

    pub fn finished(&self) -> bool {
        self.won() || self.guesses.len() >= MAX_GUESSES
    }

    pub fn rows(&self) -> Vec<Vec<Mark>> {
        self.guesses.iter().map(|guess| score_guess(guess, &self.answer)).collect()
    }

    /// Что известно о каждой букве: лучшая подсветка из всех догадок — для клавиатуры.
    pub fn letters(&self) -> HashMap<char, Mark> {
        let rank = |mark: Mark| [2, 1, 0][mark as usize];
        let mut known: HashMap<char, Mark> = HashMap::new();
        for guess in &self.guesses {
            for (letter, mark) in guess.chars().zip(score_guess(guess, &self.answer)) {
                if known.get(&letter).is_none_or(|old| rank(mark) > rank(*old)) {
                    known.insert(letter, mark);
                }
            }
        }
        known
    }

    /// Счёт: «3/6», «X/6» или «играет…».
    #[cfg(test)]
    pub fn score_text(&self) -> String {
        if self.won() {
            format!("{}/{MAX_GUESSES}", self.guesses.len())
        } else if self.finished() {
            format!("X/{MAX_GUESSES}")
        } else {
            "играет…".to_string()
        }
    }
}

/// Сколько монет положено за победу с такой-то попытки.
pub fn reward(rewards: &[i64], tries: usize) -> i64 {
    rewards[tries.clamp(1, rewards.len()) - 1]
}

/// Какое слово загадать: одно и то же для всех на сервере в этот день (номер считается из названия дня и сервера).
pub fn pick_answer(words: &[String], lang: Lang, guild: u64, day: &str) -> String {
    let seed = format!("{}:{guild}:{day}", lang.key());
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a: простой способ превратить строку в число
    for byte in seed.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    words[(hash % words.len() as u64) as usize].clone()
}

/// Словари игр (перечитываются сами, когда файл изменили) и настройки.
pub struct WordsState {
    pub settings: WordSettings,
    lists: Mutex<HashMap<Lang, (Option<std::time::SystemTime>, Arc<Vec<String>>)>>,
}

impl WordsState {
    pub fn new(settings: WordSettings) -> Self {
        Self { settings, lists: Default::default() }
    }

    /// Слова игры из config/german_words.txt или russian_words.txt. Нет файла — запасной короткий список.
    pub fn list(&self, dir: &Path, lang: Lang) -> Arc<Vec<String>> {
        let path = dir.join("config").join(lang.words_file());
        let changed = std::fs::metadata(&path).and_then(|file| file.modified()).ok();
        let mut lists = self.lists.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((seen, words)) = lists.get(&lang) {
            if *seen == changed {
                return words.clone();
            }
        }
        let words = Arc::new(parse_words(&std::fs::read_to_string(&path).unwrap_or_default(), lang));
        lists.insert(lang, (changed, words.clone()));
        words
    }
}

/// Слово дня для участника: продолжаем сегодняшнюю партию из базы или начинаем новую.
pub fn daily_game(data: &Data, guild: u64, user: u64, lang: Lang) -> rusqlite::Result<Game> {
    let day = today(data, lang);
    if let Some((answer, guesses)) = data.db.word_row(lang.table(), guild, user, &day)? {
        return Ok(Game::from_row(lang, &day, answer, &guesses));
    }
    // Если сегодня на сервере уже кто-то играл — берём его слово (так оно точно одно на всех, даже если
    // словарь успели поправить или партию начал bot.py), иначе выбираем сами.
    let answer = match data.db.word_any_answer(lang.table(), guild, &day)? {
        Some(answer) => answer,
        None => pick_answer(&data.words.list(&data.dir, lang), lang, guild, &day),
    };
    data.db.word_start(lang.table(), guild, user, &day, &answer)?;
    Ok(Game { lang, day, answer, guesses: Vec::new() })
}

/// Какие страницы искать в Викисловаре. В русском ё пишут как е, поэтому для «щетка» ищем и «щётка».
pub fn wiki_titles(word: &str, lang: Lang) -> Vec<String> {
    match lang {
        Lang::De => {
            // существительные там с большой буквы: «Kerze»
            let mut letters = word.chars();
            let capital: String = letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default();
            vec![capital, word.to_string()]
        }
        Lang::Ru => {
            let letters: Vec<char> = word.chars().collect();
            let mut titles = vec![word.to_string()];
            for (index, letter) in letters.iter().enumerate() {
                if *letter == 'е' {
                    let mut variant = letters.clone();
                    variant[index] = 'ё';
                    titles.push(variant.into_iter().collect());
                }
            }
            titles
        }
    }
}

/// Есть ли в ответе Викисловаря страница со статьёй на нужном языке.
pub fn wiki_has_word(response: &serde_json::Value, mark: &str) -> Option<bool> {
    let pages = response.get("query")?.get("pages")?.as_array()?;
    Some(pages.iter().any(|page| {
        page.pointer("/revisions/0/slots/main/content").and_then(|content| content.as_str()).is_some_and(|text| text.contains(mark))
    }))
}

/// Существует ли такое слово. Some(true/false), или None — если проверить не получилось.
pub async fn is_real_word(data: &Data, word: &str, lang: Lang) -> Option<bool> {
    let check = data.words.settings.get(lang).check;
    if check == Check::Off || data.words.list(&data.dir, lang).iter().any(|known| known == word) {
        return Some(true);
    }
    if check == Check::List {
        return Some(false);
    }
    if let Ok(Some(valid)) = data.db.word_check(word) {
        return Some(valid);
    }
    // Спрашиваем Викисловарь: есть ли страница с этим словом и статьёй на нужном языке. Там есть и формы
    // слов («gehst», «столы»), а выдуманных сочетаний вроде «kzrze» — нет.
    let (url, mark) = lang.wiki();
    let titles = wiki_titles(word, lang).join("|");
    let request = data
        .web
        .get(url)
        .query(&[("action", "query"), ("titles", titles.as_str()), ("prop", "revisions"), ("rvprop", "content"),
                 ("rvslots", "main"), ("redirects", "1"), ("format", "json"), ("formatversion", "2")])
        .header("User-Agent", "musicbot-wortjagd/1.0")
        .timeout(std::time::Duration::from_secs(8));
    let answer = async { request.send().await?.error_for_status()?.text().await }.await;
    let valid = match answer.ok().and_then(|text| serde_json::from_str(&text).ok()).and_then(|json| wiki_has_word(&json, mark)) {
        Some(valid) => valid,
        None => {
            println!("{}: Викисловарь не ответил", lang.name());
            return None;
        }
    };
    let _ = data.db.set_word_check(word, valid);
    Some(valid)
}

const FINISHED: &str = "Слово дня уже отгадано (или попытки кончились) — приходи за новым словом завтра.";

/// Догадка участника. Возвращает (партия, текст ошибки или None).
pub async fn make_guess(data: &Data, guild: u64, user: u64, name: &str, text: &str, lang: Lang) -> rusqlite::Result<(Game, Option<String>)> {
    let word = normalize(text, lang);
    let game = daily_game(data, guild, user, lang)?;
    if game.finished() {
        return Ok((game, Some(FINISHED.to_string())));
    }
    if lang == Lang::De && word.contains('ß') {
        return Ok((game, Some("Слова с ß в игре не бывает (ß пишется как ss, а это уже лишняя буква).".to_string())));
    }
    if !fits(&word, lang) {
        let hint = if lang == Lang::De { " (ä, ö, ü — можно)" } else { "" };
        return Ok((game, Some(format!("Нужно {} ровно из {WORD_LENGTH} букв{hint}.", lang.what()))));
    }
    let real = is_real_word(data, &word, lang).await;
    let mut game = daily_game(data, guild, user, lang)?; // пока проверяли слово, партия могла измениться
    let shown = word.to_uppercase();
    let problem = match real {
        None => Some("Не получилось проверить слово (Викисловарь не отвечает) — попробуй ещё раз.".to_string()),
        // в русском Викисловаре есть не все формы слов («клёны»), поэтому подсказываем
        Some(false) if lang == Lang::Ru => Some(format!(
            "Не знаю слова «{shown}». Лучше пиши существительные в начальной форме: «кошка», а не «кошки».")),
        Some(false) => Some(format!("Слова «{shown}» нет в {} языке.", lang.language())),
        Some(true) if game.finished() => Some(FINISHED.to_string()),
        Some(true) if game.guesses.contains(&word) => Some(format!("«{shown}» уже было — попробуй другое слово.")),
        Some(true) => None,
    };
    if problem.is_some() {
        return Ok((game, problem));
    }
    game.guesses.push(word);
    data.db.set_word_guesses(lang.table(), guild, user, &game.day, &game.guesses.join(","))?;
    if game.won() {
        let coins = reward(&data.words.settings.get(lang).rewards, game.guesses.len());
        if coins > 0 {
            data.db.add_money(guild, user, name, data.economy.start_balance, coins)?;
        }
    }
    Ok((game, None))
}

/// Состояние партии для мини-приложения (загаданное слово — только когда партия закончена).
pub fn game_state(data: &Data, game: &Game, balance: i64) -> serde_json::Value {
    let rewards = &data.words.settings.get(game.lang).rewards;
    let rows: Vec<serde_json::Value> = game
        .guesses
        .iter()
        .zip(game.rows())
        .map(|(word, marks)| serde_json::json!({ "word": word, "marks": marks.iter().map(|mark| mark.as_str()).collect::<Vec<_>>() }))
        .collect();
    let letters: HashMap<String, &str> = game.letters().into_iter().map(|(letter, mark)| (letter.to_string(), mark.as_str())).collect();
    serde_json::json!({
        "lang": game.lang.key(),
        "name": game.lang.name(),
        "rows": rows,
        "letters": letters,
        "daily": true,
        "day": game.day,
        "finished": game.finished(),
        "won": game.won(),
        "answer": if game.finished() { Some(&game.answer) } else { None },
        "reward": if game.won() { reward(rewards, game.guesses.len()) } else { 0 },
        "rewards": rewards,
        "balance": balance,
        "length": WORD_LENGTH,
        "max_guesses": MAX_GUESSES,
    })
}

/// Сколько дней подряд (по last_day включительно) на сервере кто-то играл в слово дня.
pub fn streak(played_days: &[String], last_day: &str) -> i64 {
    let Ok(mut day) = last_day.parse::<NaiveDate>() else { return 0 };
    let mut streak = 0;
    while played_days.iter().any(|played| *played == day.to_string()) {
        streak += 1;
        day -= chrono::Duration::days(1);
    }
    streak
}

/// Итоги вчерашнего слова дня: серия дней и кто с какой попытки отгадал. None — вчера никто не доиграл.
/// rows — вчерашние партии (участник, ответ, догадки через запятую).
pub fn summary_text(lang: Lang, rows: &[(u64, String, String)], streak: i64) -> Option<String> {
    let mut results: Vec<(usize, u64)> = Vec::new();
    for (user, answer, guesses) in rows {
        let game = Game::from_row(lang, "", answer.clone(), guesses);
        if game.finished() && !game.guesses.is_empty() {
            results.push((if game.won() { game.guesses.len() } else { MAX_GUESSES + 1 }, *user)); // недоигранные не считаем
        }
    }
    results.sort();
    let best = results.first()?.0;
    let mut lines: Vec<String> = Vec::new();
    let mut index = 0;
    while index < results.len() {
        let tries = results[index].0;
        let people: Vec<String> = results[index..].iter().take_while(|(same, _)| *same == tries).map(|(_, user)| format!("<@{user}>")).collect();
        index += people.len();
        let score = if tries <= MAX_GUESSES { format!("{tries}/{MAX_GUESSES}") } else { format!("X/{MAX_GUESSES}") };
        let crown = if tries == best && tries <= MAX_GUESSES { "👑 " } else { "" };
        lines.push(format!("{crown}{score}: {}", people.join(" ")));
    }
    let name = lang.name();
    let head = if streak >= 2 {
        format!("🔥 **Сервер играет в {name} {streak} {} подряд!** ", crate::text::plural(streak, "день", "дня", "дней"))
    } else {
        "🧩 ".to_string()
    };
    let answer = rows.first().map(|(_, answer, _)| answer.to_uppercase()).unwrap_or_default();
    Some(format!("{head}Вчерашние результаты ({name}):\n{}\nВчерашнее слово: ||{answer}||", lines.join("\n")))
}

/// Вчерашний день для дня вида "2026-11-08".
pub fn day_before(day: &str) -> String {
    day.parse::<NaiveDate>().map(|date| (date - chrono::Duration::days(1)).to_string()).unwrap_or_default()
}

/// Номер дня игры для картинки («Wortjagd №3»).
#[allow(dead_code)]
pub fn day_number(lang: Lang, day: &str) -> i64 {
    day.parse::<NaiveDate>().map(|date| (date - lang.epoch()).num_days() + 1).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(guess: &str, answer: &str) -> String {
        score_guess(guess, answer).iter().map(|mark| ["c", "p", "x"][*mark as usize]).collect()
    }

    #[test]
    fn scoring_like_python() {
        assert_eq!(marks("apfel", "apfel"), "ccccc");
        assert_eq!(marks("eeeee", "feder"), "xcxcx");
        assert_eq!(marks("ebene", "feder"), "pxpxx"); // в feder две «e»: одна на месте не стоит, вторая тоже
        assert_eq!(marks("hallo", "allee"), "xpcpx");
        assert_eq!(marks("сосна", "кошка"), "xcxxc");
        assert_eq!(marks("кошка", "кошка"), "ccccc");
    }

    #[test]
    fn normalizing_and_parsing() {
        assert_eq!(normalize(" Щётка ", Lang::Ru), "щетка");
        assert_eq!(normalize("Kerze", Lang::De), "kerze");
        let words = parse_words("# комментарий apfel\nApfel, blume;Kerze  zu\nstraße КОШКА apfel\n", Lang::De);
        assert_eq!(words, ["apfel", "blume", "kerze"]);
        assert_eq!(parse_words("ёлочка ЩЁТКА кошка kerze", Lang::Ru), ["щетка", "кошка"]);
        assert_eq!(parse_words("", Lang::Ru).len(), 20); // пустой файл — запасной список
    }

    #[test]
    fn game_progress() {
        let mut game = Game::from_row(Lang::Ru, "2026-11-11", "кошка".into(), "сосна,лампа");
        assert_eq!((game.guesses.len(), game.won(), game.finished(), game.score_text().as_str()), (2, false, false, "играет…"));
        assert_eq!(game.letters().get(&'а'), Some(&Mark::Correct));
        assert_eq!(game.letters().get(&'с'), Some(&Mark::Absent));
        game.guesses.push("кошка".into());
        assert_eq!((game.won(), game.finished(), game.score_text().as_str()), (true, true, "3/6"));
        let lost = Game::from_row(Lang::Ru, "", "кошка".into(), "a,b,c,d,e,f");
        assert_eq!((lost.won(), lost.finished(), lost.score_text().as_str()), (false, true, "X/6"));
        assert_eq!(Game::from_row(Lang::De, "", "apfel".into(), "").guesses.len(), 0);
    }

    #[test]
    fn rewards_and_answers() {
        let rewards = [500, 350, 250, 150, 100, 50];
        assert_eq!([reward(&rewards, 1), reward(&rewards, 3), reward(&rewards, 6), reward(&[100, 50], 5)], [500, 250, 50, 50]);
        let words: Vec<String> = ["apfel", "blume", "kerze", "stern"].iter().map(|word| word.to_string()).collect();
        let first = pick_answer(&words, Lang::De, 7, "2026-11-11");
        assert_eq!(first, pick_answer(&words, Lang::De, 7, "2026-11-11")); // одно слово на весь сервер
        let different = (1..40).any(|day| pick_answer(&words, Lang::De, 7, &format!("2026-12-{day:02}")) != first);
        assert!(different, "в разные дни слова разные");
        assert_eq!(parse_numbers("500, 350 и 250"), [500, 350, 250]);
    }

    #[test]
    fn wiktionary_helpers() {
        assert_eq!(wiki_titles("kerze", Lang::De), ["Kerze", "kerze"]);
        assert_eq!(wiki_titles("щетка", Lang::Ru), ["щетка", "щётка"]);
        assert_eq!(wiki_titles("береза", Lang::Ru), ["береза", "бёреза", "берёза"]);
        let found: serde_json::Value = serde_json::from_str(
            r#"{"query":{"pages":[{"title":"Kerze","revisions":[{"slots":{"main":{"content":"== Kerze ({{Sprache|Deutsch}}) =="}}}]}]}}"#).unwrap();
        let missing: serde_json::Value = serde_json::from_str(r#"{"query":{"pages":[{"title":"Kzrze","missing":true}]}}"#).unwrap();
        assert_eq!(wiki_has_word(&found, "{{Sprache|Deutsch}}"), Some(true));
        assert_eq!(wiki_has_word(&found, "{{-ru-"), Some(false));
        assert_eq!(wiki_has_word(&missing, "{{Sprache|Deutsch}}"), Some(false));
        assert_eq!(wiki_has_word(&serde_json::json!({"error": "x"}), "x"), None);
    }

    /// Бот с базой в памяти и настоящими словарями из папки config. check — как проверять слова.
    fn test_data(check: &str) -> Data {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let settings = crate::config::Settings::parse(&format!("[wortspiel]
check_words = {check}
[slovo]
check_words = {check}
"));
        Data::new(crate::db::Database::in_memory().unwrap(), &settings, dir, false)
    }

    #[tokio::test]
    async fn playing_a_daily_game() {
        let data = test_data("list");
        assert!(data.words.list(&data.dir, Lang::De).len() > 400 && data.words.list(&data.dir, Lang::Ru).len() > 400);
        let game = daily_game(&data, 7, 1, Lang::Ru).unwrap();
        // у всех на сервере одно слово, а у немецкой игры — своё
        assert_eq!(daily_game(&data, 7, 2, Lang::Ru).unwrap().answer, game.answer);
        assert!(data.words.list(&data.dir, Lang::Ru).contains(&game.answer));
        assert!(data.words.list(&data.dir, Lang::De).contains(&daily_game(&data, 7, 1, Lang::De).unwrap().answer));

        let problem = |text: &'static str| {
            let data = &data;
            async move { make_guess(data, 7, 1, "Alice", text, Lang::Ru).await.unwrap().1.unwrap_or_default() }
        };
        assert!(problem("кот").await.starts_with("Нужно русское слово ровно из 5 букв"));
        assert!(problem("abcde").await.starts_with("Нужно русское слово"));
        assert!(problem("кзрзе").await.starts_with("Не знаю слова «КЗРЗЕ»")); // нет в словаре игры
        let other = data.words.list(&data.dir, Lang::Ru).iter().find(|word| **word != game.answer).unwrap().clone();
        let (after, error) = make_guess(&data, 7, 1, "Alice", &other.to_uppercase(), Lang::Ru).await.unwrap();
        assert_eq!((error, after.guesses.len(), after.finished()), (None, 1, false));
        let (_, error) = make_guess(&data, 7, 1, "Alice", &other, Lang::Ru).await.unwrap();
        assert!(error.unwrap().contains("уже было"));

        // победа со второй попытки: 350 монет поверх стартовых 500
        let (won, error) = make_guess(&data, 7, 1, "Alice", &game.answer, Lang::Ru).await.unwrap();
        assert_eq!((error, won.won(), won.guesses.len()), (None, true, 2));
        assert_eq!(data.db.wallet(7, 1, "Alice", 500).unwrap().0, 850);
        let (_, error) = make_guess(&data, 7, 1, "Alice", &other, Lang::Ru).await.unwrap();
        assert!(error.unwrap().starts_with("Слово дня уже отгадано"));
        // партия лежит в базе — «после перезапуска» она та же
        assert_eq!(daily_game(&data, 7, 1, Lang::Ru).unwrap(), won);
        // немецкая партия при этом не тронута
        assert!(daily_game(&data, 7, 1, Lang::De).unwrap().guesses.is_empty());

        let state = game_state(&data, &won, 850);
        assert_eq!(state["lang"], "ru");
        assert_eq!(state["answer"], game.answer.as_str());
        assert_eq!((state["won"].as_bool(), state["reward"].as_i64(), state["rows"].as_array().map(Vec::len)), (Some(true), Some(350), Some(2)));
        let hidden = game_state(&data, &daily_game(&data, 7, 2, Lang::Ru).unwrap(), 500);
        assert!(hidden["answer"].is_null(), "пока партия идёт, слово странице не сообщаем");
    }

    #[tokio::test]
    async fn six_misses_lose() {
        let data = test_data("off"); // любое сочетание из 5 букв
        let answer = daily_game(&data, 7, 1, Lang::De).unwrap().answer;
        let mut last = None;
        for word in ["aaaaa", "bbbbb", "ccccc", "ddddd", "eeeee", "fffff"].into_iter().filter(|word| *word != answer) {
            last = Some(make_guess(&data, 7, 1, "Bob", word, Lang::De).await.unwrap());
        }
        let (game, error) = last.unwrap();
        assert_eq!((error, game.finished(), game.won()), (None, true, false));
        assert_eq!(data.db.wallet(7, 1, "Bob", 500).unwrap().0, 500); // награды нет
        assert!(make_guess(&data, 7, 1, "Bob", "straß", Lang::De).await.unwrap().1.unwrap().starts_with("Слово дня уже"));
    }

    /// Настоящий запрос к Викисловарю (нужен интернет): cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn wiktionary_live() {
        let data = test_data("wiktionary");
        assert_eq!(is_real_word(&data, "gehst", Lang::De).await, Some(true)); // форма слова, которой нет в словаре игры
        assert_eq!(is_real_word(&data, "kzrze", Lang::De).await, Some(false));
        assert_eq!(is_real_word(&data, "столы", Lang::Ru).await, Some(true));
        assert_eq!(is_real_word(&data, "кзрзе", Lang::Ru).await, Some(false));
        assert_eq!(data.db.word_check("kzrze").unwrap(), Some(false)); // ответ запомнен
    }

    #[test]
    fn streaks_and_summary() {
        let days: Vec<String> = ["2026-11-08", "2026-11-09", "2026-11-10", "2026-11-06"].iter().map(|day| day.to_string()).collect();
        assert_eq!([streak(&days, "2026-11-10"), streak(&days, "2026-11-06"), streak(&days, "2026-11-07")], [3, 1, 0]);
        assert_eq!(day_before("2026-11-01"), "2026-10-31");
        assert_eq!(day_number(Lang::De, "2026-11-10"), 3);
        let rows = vec![
            (1, "кошка".to_string(), "сосна,кошка".to_string()),
            (2, "кошка".to_string(), "a,b,c,d,e,f".to_string()),
            (3, "кошка".to_string(), "лампа,кошка".to_string()),
            (4, "кошка".to_string(), "лампа".to_string()), // не доиграл — не считается
        ];
        assert_eq!(summary_text(Lang::Ru, &rows, 3).unwrap(),
                   "🔥 **Сервер играет в 5 букв 3 дня подряд!** Вчерашние результаты (5 букв):\n👑 2/6: <@1> <@3>\nX/6: <@2>\nВчерашнее слово: ||КОШКА||");
        assert!(summary_text(Lang::Ru, &rows[..2], 1).unwrap().starts_with("🧩 Вчерашние результаты (5 букв):"));
        assert_eq!(summary_text(Lang::Ru, &rows[3..], 1), None);
    }
}
