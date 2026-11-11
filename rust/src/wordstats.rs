//! Слова: статистика игрока (/wordstats), лучшие игроки месяца (/wordtop) и дуэли на монеты (/wordduel).

use crate::cards::{Picture, Row};
use crate::levels::{avatar_by_id, render_table, send_png};
use crate::reply::{challenge, display_name, finish_challenge, user_name, whisper};
use crate::text::{money_text, number};
use crate::words::{self, Game, Lang, MAX_GUESSES};
use crate::{Context, Data, Error};
use chrono::NaiveDate;
use poise::serenity_prelude as serenity;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, poise::ChoiceParameter)]
pub enum GameChoice {
    #[name = "Wortjagd (немецкое слово)"]
    Wortjagd,
    #[name = "5 букв (русское слово)"]
    FiveLetters,
}

impl GameChoice {
    fn lang(self) -> Lang {
        [Lang::De, Lang::Ru][self as usize]
    }
}

/// Законченные партии из строк базы: (участник, день, попыток или None, если не угадал). Недоигранные пропускаем.
pub fn results(lang: Lang, rows: &[(u64, String, String, String)]) -> Vec<(u64, String, Option<usize>)> {
    rows.iter()
        .filter_map(|(user, day, answer, guesses)| {
            let game = Game::from_row(lang, day, answer.clone(), guesses);
            if game.won() {
                Some((*user, day.clone(), Some(game.guesses.len())))
            } else if game.finished() {
                Some((*user, day.clone(), None))
            } else {
                None
            }
        })
        .collect()
}

/// По дням с победой: (текущая серия дней подряд, лучшая серия). Текущая считается, если последняя победа
/// была сегодня или вчера.
pub fn win_streaks(days_won: &[String], today: &str) -> (i64, i64) {
    let mut dates: Vec<NaiveDate> = days_won.iter().filter_map(|day| day.parse().ok()).collect();
    dates.sort();
    dates.dedup();
    let (mut best, mut run, mut previous) = (0, 0, None::<NaiveDate>);
    for date in &dates {
        run = if previous.is_some_and(|previous| (*date - previous).num_days() == 1) { run + 1 } else { 1 };
        best = best.max(run);
        previous = Some(*date);
    }
    let fresh = today.parse::<NaiveDate>().ok().zip(previous).is_some_and(|(today, last)| (today - last).num_days() <= 1);
    (if fresh { run } else { 0 }, best)
}

/// Очки за слово дня: с 1-й попытки — 6, со 2-й — 5 … с 6-й — 1, не угадал — 0.
pub fn points(tries: Option<usize>) -> i64 {
    tries.map_or(0, |tries| (MAX_GUESSES + 1).saturating_sub(tries) as i64)
}

/// Текст статистики одной игры для /wordstats. None — человек в неё ещё не играл.
pub fn stats_text(results: &[(u64, String, Option<usize>)], today: &str) -> Option<String> {
    if results.is_empty() {
        return None;
    }
    let wins: Vec<usize> = results.iter().filter_map(|(_, _, tries)| *tries).collect();
    let won_days: Vec<String> = results.iter().filter(|(_, _, tries)| tries.is_some()).map(|(_, day, _)| day.clone()).collect();
    let (current, best) = win_streaks(&won_days, today);
    let count = |tries: usize| wins.iter().filter(|won| **won == tries).count();
    let biggest = (1..=MAX_GUESSES).map(count).max().unwrap_or(0).max(1);
    let bars: Vec<String> = (1..=MAX_GUESSES)
        .map(|tries| {
            let length = (8.0 * count(tries) as f64 / biggest as f64).round_ties_even() as usize;
            let bar = if length > 0 { "█".repeat(length) } else { "▏".to_string() };
            format!("`{tries}` {bar} {}", count(tries))
        })
        .collect();
    let average = if wins.is_empty() {
        String::new()
    } else {
        format!(", в среднем с {:.1}-й попытки", wins.iter().sum::<usize>() as f64 / wins.len() as f64).replace('.', ",")
    };
    let percent = (100.0 * wins.len() as f64 / results.len() as f64).round_ties_even();
    Some(format!(
        "Сыграно: **{}** · побед: **{}** ({percent}%){average}\n🔥 Серия побед: **{current}** · лучшая: **{best}**\n{}",
        results.len(), wins.len(), bars.join("\n")))
}

/// Твоя статистика в Wortjagd и «5 букв»
#[poise::command(slash_command, guild_only)]
pub async fn wordstats(
    ctx: Context<'_>,
    #[description = "Чью статистику показать (если не выбрать — твою)"] member: Option<serenity::Member>,
) -> Result<(), Error> {
    let (data, guild) = (ctx.data(), ctx.guild_id().unwrap().get());
    let (user, name) = match &member {
        Some(member) => (member.user.id.get(), member.display_name().to_string()),
        None => (ctx.author().id.get(), display_name(ctx).await),
    };
    let mut embed = serenity::CreateEmbed::new().color(0x3B9A68).title(format!("🧩 Статистика слов — {name}"));
    for lang in Lang::ALL {
        let rows = data.db.word_results(lang.table(), guild, Some(user), None)?;
        let text = stats_text(&results(lang, &rows), &words::today(data, lang))
            .unwrap_or_else(|| format!("Ещё не играл(а). Начать — {}", lang.command()));
        embed = embed.field(lang.name(), text, false);
    }
    ctx.send(poise::CreateReply::default().embed(embed).allowed_mentions(serenity::CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Таблица месяца: участник → (очки, побед, игр), от лучших к худшим, не больше 10.
pub fn month_top(results: &[(u64, String, Option<usize>)]) -> Vec<(u64, i64, i64, i64)> {
    let mut totals: HashMap<u64, (i64, i64, i64)> = HashMap::new();
    for (user, _, tries) in results {
        let total = totals.entry(*user).or_default();
        total.0 += points(*tries);
        total.1 += tries.is_some() as i64;
        total.2 += 1;
    }
    let mut top: Vec<(u64, i64, i64, i64)> = totals.into_iter().map(|(user, (points, wins, games))| (user, points, wins, games)).collect();
    top.sort_by_key(|(user, points, wins, _)| (-points, -wins, *user));
    top.truncate(10);
    top
}

/// Лучшие игроки месяца в Wortjagd или «5 букв» (топ-10)
#[poise::command(slash_command, guild_only)]
pub async fn wordtop(ctx: Context<'_>, #[description = "Какая игра"] game: GameChoice) -> Result<(), Error> {
    let (data, guild, lang) = (ctx.data(), ctx.guild_id().unwrap(), game.lang());
    let month = words::today(data, lang)[..7].to_string();
    let top = month_top(&results(lang, &data.db.word_results(lang.table(), guild.get(), None, Some(&month))?));
    if top.is_empty() {
        ctx.say(format!("В этом месяце в {} ещё никто не играл — {}!", lang.name(), lang.command())).await?;
        return Ok(());
    }
    ctx.defer().await?;
    let best = top[0].1.max(1);
    let mut rows = Vec::new();
    for (user, points, wins, games) in top {
        let name = match guild.member(ctx, serenity::UserId::new(user)).await {
            Ok(member) => member.display_name().to_string(),
            Err(_) => format!("Участник {user}"),
        };
        rows.push(Row {
            name,
            picture: Picture::Avatar(avatar_by_id(ctx.serenity_context(), data, user).await),
            progress: points as f64 / best as f64,
            pill: number(points),
            hint: format!("побед {wins} из {games}"),
        });
    }
    let title: &'static str = if lang == Lang::De { "WORTJAGD" } else { "5 БУКВ" };
    let server = guild.name(ctx).unwrap_or_default();
    let png = render_table(data, title, server, "очки за месяц", rows, "leaderboard_bg").await;
    send_png(ctx, png, "wordtop.png").await
}

// ---------- Дуэль: двое ставят монеты на сегодняшнее слово дня; кто отгадает за меньше попыток — забирает всё ----------

/// Результат участника в слове дня: число попыток, MAX_GUESSES + 1 — не угадал, None — ещё играет.
/// day_over — день уже кончился: кто не доиграл, тот не угадал.
pub fn duel_tries(game: Option<&Game>, day_over: bool) -> Option<usize> {
    match game {
        Some(game) if game.won() => Some(game.guesses.len()),
        Some(game) if game.finished() => Some(MAX_GUESSES + 1),
        _ if day_over => Some(MAX_GUESSES + 1),
        _ => None,
    }
}

fn score(tries: usize) -> String {
    if tries <= MAX_GUESSES { format!("{tries}/{MAX_GUESSES}") } else { format!("X/{MAX_GUESSES}") }
}

/// Итог дуэли: (текст для чата, кому сколько вернуть). Ничья — ставки возвращаются.
pub fn duel_outcome(name: &str, users: (u64, u64), tries: (usize, usize), bet: i64) -> (String, Vec<(u64, i64)>) {
    if tries.0 == tries.1 {
        let text = format!(
            "🤝 Дуэль в {name}: <@{}> и <@{}> сыграли одинаково ({}) — ставки по {} возвращены.",
            users.0, users.1, score(tries.0), money_text(bet));
        return (text, vec![(users.0, bet), (users.1, bet)]);
    }
    let (winner, loser) = if tries.0 < tries.1 { (users.0, users.1) } else { (users.1, users.0) };
    let text = format!(
        "🏆 Дуэль в {name}: <@{winner}> ({}) побеждает <@{loser}> ({}) и забирает **{}**!",
        score(tries.0.min(tries.1)), score(tries.0.max(tries.1)), money_text(bet * 2));
    (text, vec![(winner, bet * 2)])
}

/// Подводит итоги дуэлей, где оба доиграли слово дня (или день уже кончился).
pub async fn resolve_duels(http: &serenity::Http, data: &Data) -> Result<(), Error> {
    for duel in data.db.word_duels()? {
        let Some(lang) = Lang::from_key(&duel.lang) else { continue };
        let day_over = duel.day.as_str() < words::today(data, lang).as_str();
        let tries = |user: u64| -> rusqlite::Result<Option<usize>> {
            let game = data.db.word_row(lang.table(), duel.guild, user, &duel.day)?.map(|(answer, guesses)| Game::from_row(lang, &duel.day, answer, &guesses));
            Ok(duel_tries(game.as_ref(), day_over))
        };
        let (Some(first), Some(second)) = (tries(duel.user1)?, tries(duel.user2)?) else { continue }; // кто-то ещё играет
        if !data.db.delete_word_duel(duel.id)? {
            continue; // эту дуэль уже посчитали
        }
        let (text, payouts) = duel_outcome(lang.name(), (duel.user1, duel.user2), (first, second), duel.bet);
        for (user, amount) in payouts {
            data.db.add_money_by_id(duel.guild, user, amount)?;
        }
        let message = serenity::CreateMessage::new().content(text).allowed_mentions(serenity::CreateAllowedMentions::new());
        let _ = serenity::ChannelId::new(duel.channel).send_message(http, message).await;
    }
    Ok(())
}

/// Раз в 10 минут проверяет дуэли: вдруг день кончился, а кто-то так и не доиграл.
pub fn start_duel_loop(http: std::sync::Arc<serenity::Http>, data: crate::Shared) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(600));
        loop {
            timer.tick().await;
            if let Err(error) = resolve_duels(&http, &data).await {
                println!("Дуэли в словах: {error}");
            }
        }
    });
}

/// Дуэль на монеты: кто отгадает сегодняшнее слово дня за меньше попыток
#[poise::command(slash_command, guild_only)]
pub async fn wordduel(
    ctx: Context<'_>,
    #[description = "Кого вызвать"] member: serenity::User,
    #[description = "В какой игре"] game: GameChoice,
    #[description = "Ставка в монетах"]
    #[min = 1]
    #[max = 1_000_000]
    bet: i64,
) -> Result<(), Error> {
    let (data, guild, user, other, lang) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get(), member.id.get(), game.lang());
    let (start, name, other_name) = (data.economy.start_balance, display_name(ctx).await, user_name(&member));
    let day = words::today(data, lang);
    // уже делал ходы в сегодняшнем слове — дуэль была бы нечестной
    let started = |who: u64| data.db.word_row(lang.table(), guild, who, &day).map(|row| row.is_some_and(|(_, guesses)| !guesses.is_empty()));
    let busy = |who: u64| data.db.in_word_duel(guild, lang.key(), &day, who);

    if member.bot || other == user {
        return whisper(ctx, "Вызови на дуэль другого участника (не бота и не себя).").await;
    }
    if started(user)? || started(other)? {
        return whisper(ctx, format!(
            "Дуэль честная, только если оба ещё не начинали сегодняшнее слово в {}. Попробуйте завтра.", lang.name())).await;
    }
    if busy(user)? || busy(other)? {
        return whisper(ctx, "У кого-то из вас уже есть дуэль на сегодняшнее слово.").await;
    }
    if bet < data.economy.min_bet || data.db.wallet(guild, user, &name, start)?.0 < bet {
        return whisper(ctx, format!("Ставка — от {}, и у тебя должно хватать монет.", money_text(data.economy.min_bet))).await;
    }
    let offer = format!(
        "⚔️ <@{other}>, <@{user}> вызывает тебя на дуэль в {}: кто отгадает сегодняшнее слово дня за меньше попыток. \
         Ставка — **{}** с каждого.", lang.name(), money_text(bet));
    let Some(press) = challenge(ctx, member.id, offer).await? else { return Ok(()) };

    // Пока ждали ответа, кто-то мог начать слово, потратить монеты, или наступил новый день
    if started(user)? || started(other)? || busy(user)? || busy(other)? || words::today(data, lang) != day {
        return finish_challenge(ctx, &press, "Дуэль отменена: кто-то уже начал слово дня.".into()).await;
    }
    if data.db.wallet(guild, user, &name, start)?.0 < bet || data.db.wallet(guild, other, &other_name, start)?.0 < bet {
        return finish_challenge(ctx, &press, format!("Дуэль отменена: кому-то не хватает {}.", money_text(bet))).await;
    }
    data.db.add_money(guild, user, &name, start, -bet)?;
    data.db.add_money(guild, other, &other_name, start, -bet)?;
    data.db.add_word_duel(guild, ctx.channel_id().get(), lang.key(), &day, user, other, bet)?;
    finish_challenge(ctx, &press, format!(
        "⚔️ Дуэль в {}: <@{user}> против <@{other}>, на кону **{}**. Открывайте {} и отгадывайте сегодняшнее слово — \
         победит тот, кому хватит меньше попыток. Кто не доиграет до конца дня — проиграл.",
        lang.name(), money_text(bet * 2), lang.command())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(user: u64, day: &str, guesses: &str) -> (u64, String, String, String) {
        (user, day.to_string(), "кошка".to_string(), guesses.to_string())
    }

    #[test]
    fn results_and_points() {
        let rows = [row(1, "2026-11-10", "сосна,кошка"), row(1, "2026-11-11", "a,b,c,d,e,f"), row(2, "2026-11-11", "лампа")];
        assert_eq!(results(Lang::Ru, &rows), [(1, "2026-11-10".to_string(), Some(2)), (1, "2026-11-11".to_string(), None)]);
        assert_eq!([points(Some(1)), points(Some(2)), points(Some(6)), points(None)], [6, 5, 1, 0]);
    }

    #[test]
    fn streaks() {
        let days = |list: &[&str]| list.iter().map(|day| day.to_string()).collect::<Vec<_>>();
        assert_eq!(win_streaks(&days(&["2026-11-11", "2026-11-10", "2026-11-08", "2026-11-07", "2026-11-06"]), "2026-11-11"), (2, 3));
        assert_eq!(win_streaks(&days(&["2026-11-08"]), "2026-11-11"), (0, 1)); // серия прервалась
        assert_eq!(win_streaks(&days(&["2026-11-10"]), "2026-11-11"), (1, 1)); // вчера — ещё считается
        assert_eq!(win_streaks(&[], "2026-11-11"), (0, 0));
    }

    #[test]
    fn stats() {
        let rows = [row(1, "2026-11-11", "сосна,кошка"), row(1, "2026-11-10", "кошка"), row(1, "2026-11-09", "a,b,c,d,e,f"),
                    row(1, "2026-11-08", "x,y,кошка")];
        let text = stats_text(&results(Lang::Ru, &rows), "2026-11-11").unwrap();
        assert_eq!(text, "Сыграно: **4** · побед: **3** (75%), в среднем с 2,0-й попытки\n🔥 Серия побед: **2** · лучшая: **2**\n\
                          `1` ████████ 1\n`2` ████████ 1\n`3` ████████ 1\n`4` ▏ 0\n`5` ▏ 0\n`6` ▏ 0");
        assert_eq!(stats_text(&[], "2026-11-11"), None);
    }

    #[test]
    fn top_of_month() {
        let rows = [row(1, "2026-11-10", "сосна,кошка"), row(1, "2026-11-11", "кошка"), row(2, "2026-11-11", "a,b,c,кошка"),
                    row(3, "2026-11-11", "a,b,c,d,e,f")];
        assert_eq!(month_top(&results(Lang::Ru, &rows)), [(1, 11, 2, 2), (2, 3, 1, 1), (3, 0, 0, 1)]);
    }

    #[test]
    fn duels() {
        let game = |guesses: &str| Game::from_row(Lang::Ru, "", "кошка".into(), guesses);
        assert_eq!(duel_tries(Some(&game("сосна,кошка")), false), Some(2));
        assert_eq!(duel_tries(Some(&game("a,b,c,d,e,f")), false), Some(7));
        assert_eq!(duel_tries(Some(&game("сосна")), false), None); // ещё играет
        assert_eq!(duel_tries(Some(&game("сосна")), true), Some(7)); // день кончился — не доиграл
        assert_eq!(duel_tries(None, false), None);
        assert_eq!(duel_tries(None, true), Some(7));

        let (text, payouts) = duel_outcome("Wortjagd", (1, 2), (2, 4), 200);
        assert_eq!(text, "🏆 Дуэль в Wortjagd: <@1> (2/6) побеждает <@2> (4/6) и забирает **400 монет**!");
        assert_eq!(payouts, [(1, 400)]);
        let (text, payouts) = duel_outcome("5 букв", (1, 2), (7, 3), 50);
        assert!(text.contains("<@2> (3/6) побеждает <@1> (X/6)"));
        assert_eq!(payouts, [(2, 100)]);
        let (text, payouts) = duel_outcome("5 букв", (1, 2), (7, 7), 50);
        assert!(text.starts_with("🤝") && text.contains("(X/6)"));
        assert_eq!(payouts, [(1, 50), (2, 50)]);
    }
}
