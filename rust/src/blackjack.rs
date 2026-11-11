//! Блэкджек на монеты (/blackjack). Правила — как в bot.py: дилер добирает до 17, блэкджек платит 3 к 2,
//! можно удвоить ставку (одна карта и стоп) и разделить пару (до 4 рук; разделённые тузы получают по одной карте).
//! Карты сдаются из общего «башмака» сервера — 3 колоды вместе, как в казино.

use crate::economy::parse_bet;
use crate::reply::{display_name, take_money, whisper};
use crate::text::money_text;
use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use rand::seq::SliceRandom;
use std::time::Duration;

const RANKS: [&str; 13] = ["2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K", "A"];
const SUITS: [&str; 4] = ["♠", "♥", "♦", "♣"];
const ACE: u8 = 12;
const DECKS: usize = 3; // сколько колод в башмаке (3 × 52 = 156 карт)
const RESHUFFLE_BELOW: usize = DECKS * 52 / 4; // осталось меньше четверти — перед раздачей мешаем новый
const MAX_HANDS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(120); // столько не нажимают кнопки — бот сам говорит «Хватит»

/// Карта: (достоинство — номер в RANKS, масть — номер в SUITS).
pub type Card = (u8, u8);

/// Новый перемешанный башмак.
pub fn new_shoe() -> Vec<Card> {
    let mut shoe: Vec<Card> = (0..DECKS).flat_map(|_| (0..13).flat_map(|rank| (0..4).map(move |suit| (rank, suit)))).collect();
    shoe.shuffle(&mut rand::rng());
    shoe
}

/// Берёт карту из башмака. Если карты вдруг кончились посреди партии — мешает новый башмак.
fn draw(shoe: &mut Vec<Card>) -> Card {
    if shoe.is_empty() {
        *shoe = new_shoe();
    }
    shoe.pop().unwrap()
}

/// Очки одной карты: картинки и десятка — по 10, туз — 11.
pub fn card_value(card: Card) -> i64 {
    match card.0 {
        ACE => 11,
        9..=11 => 10, // J, Q, K
        rank => rank as i64 + 2,
    }
}

/// Сумма очков на руке. Туз — 11, но если получается больше 21, туз считается за 1.
pub fn hand_value(hand: &[Card]) -> i64 {
    let mut value: i64 = hand.iter().map(|card| card_value(*card)).sum();
    let mut aces = hand.iter().filter(|card| card.0 == ACE).count();
    while value > 21 && aces > 0 {
        value -= 10;
        aces -= 1;
    }
    value
}

/// Карты текстом: `A♠` `10♥`. hide_second — спрятать вторую карту дилера.
pub fn hand_text(hand: &[Card], hide_second: bool) -> String {
    hand.iter()
        .enumerate()
        .map(|(index, (rank, suit))| {
            if hide_second && index == 1 {
                "`🂠`".to_string()
            } else {
                format!("`{}{}`", RANKS[*rank as usize], SUITS[*suit as usize])
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Чем закончилась партия.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub lines: Vec<String>, // итог по каждой руке
    pub payout: i64,        // сколько вернуть игроку (ставки уже списаны)
    pub total_bet: i64,
}

/// Одна партия: только правила, без Discord и базы — поэтому её удобно проверять тестами.
#[derive(Debug, Clone)]
pub struct Game {
    pub hands: Vec<Vec<Card>>, // руки игрока (после разделения их несколько)
    pub bets: Vec<i64>,        // ставка на каждой руке
    pub doubled: Vec<bool>,
    pub current: usize, // какую руку игрок сейчас играет
    pub dealer: Vec<Card>,
    pub outcome: Option<Outcome>, // пока None — партия идёт
}

impl Game {
    pub fn deal(shoe: &mut Vec<Card>, bet: i64) -> Self {
        let hand = vec![draw(shoe), draw(shoe)];
        let dealer = vec![draw(shoe), draw(shoe)];
        Self { hands: vec![hand], bets: vec![bet], doubled: vec![false], current: 0, dealer, outcome: None }
    }

    pub fn hand(&self) -> &Vec<Card> {
        &self.hands[self.current]
    }

    /// У кого-то сразу 21 с раздачи — партия заканчивается без ходов.
    pub fn natural(&self) -> bool {
        hand_value(self.hand()) == 21 || hand_value(&self.dealer) == 21
    }

    /// Удвоить можно на двух первых картах руки, если хватает монет ещё на одну такую же ставку.
    pub fn can_double(&self, balance: i64) -> bool {
        self.outcome.is_none() && self.hand().len() == 2 && balance >= self.bets[self.current]
    }

    /// Разделить можно пару одинаковых по очкам карт, если хватает монет и рук ещё не слишком много.
    pub fn can_split(&self, balance: i64) -> bool {
        let hand = self.hand();
        self.outcome.is_none()
            && hand.len() == 2
            && card_value(hand[0]) == card_value(hand[1])
            && self.hands.len() < MAX_HANDS
            && balance >= self.bets[self.current]
    }

    /// Текущая рука сыграна — переходим к следующей, а если рук больше нет — к дилеру и итогу.
    fn next_hand(&mut self, shoe: &mut Vec<Card>) {
        loop {
            self.current += 1;
            if self.current >= self.hands.len() {
                self.current = self.hands.len() - 1;
                self.finish(shoe);
                return;
            }
            if self.hands[self.current].len() == 1 {
                let card = draw(shoe); // рука после разделения — добираем ей вторую карту
                self.hands[self.current].push(card);
            }
            if self.hand()[0].0 == ACE && self.hands.len() > 1 {
                continue; // разделённые тузы получают только по одной карте
            }
            if hand_value(self.hand()) < 21 {
                return; // на этой руке есть что решать
            }
        }
    }

    /// «Ещё»: карта на текущую руку.
    pub fn hit(&mut self, shoe: &mut Vec<Card>) {
        let card = draw(shoe);
        self.hands[self.current].push(card);
        if hand_value(self.hand()) >= 21 {
            self.next_hand(shoe); // перебор или ровно 21 — брать больше нечего
        }
    }

    /// «Хватит».
    pub fn stand(&mut self, shoe: &mut Vec<Card>) {
        self.next_hand(shoe);
    }

    /// «Удвоить»: ставка ×2, ровно одна карта, и рука останавливается. Вторую ставку списывает тот, кто вызвал.
    pub fn double(&mut self, shoe: &mut Vec<Card>) {
        self.bets[self.current] *= 2;
        self.doubled[self.current] = true;
        let card = draw(shoe);
        self.hands[self.current].push(card);
        self.next_hand(shoe);
    }

    /// «Разделить»: вторая карта уходит в новую руку с такой же ставкой (её списывает тот, кто вызвал).
    pub fn split(&mut self, shoe: &mut Vec<Card>) {
        let bet = self.bets[self.current];
        let second = self.hands[self.current].pop().unwrap();
        self.hands.insert(self.current + 1, vec![second]);
        self.bets.insert(self.current + 1, bet);
        self.doubled.insert(self.current + 1, false);
        let card = draw(shoe);
        self.hands[self.current].push(card);
        if self.hand()[0].0 == ACE || hand_value(self.hand()) == 21 {
            self.next_hand(shoe); // разделённые тузы — по одной карте; 21 — решать нечего
        }
    }

    /// Время вышло: бот сам говорит «Хватит» на всех руках.
    pub fn stand_all(&mut self, shoe: &mut Vec<Card>) {
        if self.outcome.is_some() {
            return;
        }
        for index in 0..self.hands.len() {
            if self.hands[index].len() == 1 {
                let card = draw(shoe);
                self.hands[index].push(card);
            }
        }
        self.finish(shoe);
    }

    /// Дилер добирает карты, считаем итог по каждой руке.
    pub fn finish(&mut self, shoe: &mut Vec<Card>) {
        let split = self.hands.len() > 1;
        let dealer_blackjack = self.dealer.len() == 2 && hand_value(&self.dealer) == 21;
        // Блэкджек — только 21 с двух первых карт без разделения (после разделения 21 — обычные 21)
        let player_blackjack = !split && self.hands[0].len() == 2 && hand_value(&self.hands[0]) == 21;
        let alive = self.hands.iter().any(|hand| hand_value(hand) <= 21);
        if alive && !player_blackjack && !dealer_blackjack {
            while hand_value(&self.dealer) < 17 {
                let card = draw(shoe);
                self.dealer.push(card);
            }
        }
        let dealer = hand_value(&self.dealer);
        let (mut payout, mut lines) = (0, Vec::new());
        for (index, (hand, bet)) in self.hands.iter().zip(self.bets.iter().copied()).enumerate() {
            let player = hand_value(hand);
            let prefix = if split { format!("Рука {}: ", index + 1) } else { String::new() };
            if player > 21 {
                lines.push(format!("{prefix}💥 Перебор, −{}", money_text(bet)));
            } else if player_blackjack && dealer_blackjack {
                payout += bet;
                lines.push("🤝 Блэкджек у обоих — ничья, ставка возвращена".to_string());
            } else if player_blackjack {
                let win = bet * 3 / 2; // блэкджек платит 3 к 2
                payout += bet + win;
                lines.push(format!("🎉 Блэкджек! +{}", money_text(win)));
            } else if dealer_blackjack {
                lines.push(format!("{prefix}😔 У дилера блэкджек, −{}", money_text(bet)));
            } else if dealer > 21 || player > dealer {
                payout += bet * 2;
                let why = if dealer > 21 { "Дилер перебрал" } else { "Победа" };
                lines.push(format!("{prefix}🎉 {why}! +{}", money_text(bet)));
            } else if player == dealer {
                payout += bet;
                lines.push(format!("{prefix}🤝 Ничья, ставка возвращена"));
            } else {
                lines.push(format!("{prefix}😔 У дилера больше, −{}", money_text(bet)));
            }
        }
        self.outcome = Some(Outcome { lines, payout, total_bet: self.bets.iter().sum() });
    }
}

/// Игрок занят партией, пока жив этот объект: при любом выходе из команды (и при ошибке) место освобождается.
struct Seat<'a> {
    data: &'a crate::Data,
    user: u64,
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        self.data.blackjack_players.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.user);
    }
}

/// Карточка с текущим состоянием партии. result — итог с балансом, когда партия закончена.
fn embed(game: &Game, name: &str, avatar: &str, cards_left: usize, balance: Option<i64>) -> serenity::CreateEmbed {
    let outcome = game.outcome.as_ref();
    let color = match outcome {
        None => 0x3B82F6,
        Some(outcome) if outcome.payout > outcome.total_bet => 0x22C55E, // зелёный — выигрыш
        Some(outcome) if outcome.payout == outcome.total_bet => 0x94A3B8, // серый — ничья
        Some(_) => 0xEF4444,                                             // красный — проигрыш
    };
    let mut embed = serenity::CreateEmbed::new().color(color).author(serenity::CreateEmbedAuthor::new(name).icon_url(avatar));
    for (index, hand) in game.hands.iter().enumerate() {
        let title = if game.hands.len() == 1 {
            "Твоя рука".to_string()
        } else {
            let arrow = if index == game.current && outcome.is_none() { "▶ " } else { "" };
            format!("{arrow}Рука {}", index + 1)
        };
        let doubled = if game.doubled[index] { " (×2)" } else { "" };
        embed = embed.field(title, format!(
            "{}\n\nОчки: **{}**\nСтавка: {}{doubled}", hand_text(hand, false), hand_value(hand), money_text(game.bets[index])), true);
    }
    embed = match outcome {
        // в конце показываем все карты дилера, а пока игра идёт — вторая карта скрыта
        Some(_) => embed.field("Рука дилера", format!(
            "{}\n\nОчки: **{}**", hand_text(&game.dealer, false), hand_value(&game.dealer)), true),
        None => embed.field("Рука дилера", format!(
            "{}\n\nОчки: **{}**", hand_text(&game.dealer, true), card_value(game.dealer[0])), true),
    };
    if let (Some(outcome), Some(balance)) = (outcome, balance) {
        embed = embed.field("Итог", format!("{}\nБаланс: {}", outcome.lines.join("\n"), money_text(balance)), false);
    }
    embed.footer(serenity::CreateEmbedFooter::new(format!("Карт в колоде: {cards_left}")))
}

/// Кнопки под партией; серые (выключенные) — то, что сейчас сделать нельзя.
fn buttons(game: &Game, balance: i64) -> Vec<serenity::CreateActionRow> {
    use serenity::ButtonStyle::{Primary, Secondary, Success};
    let button = |id: &str, label: &str, style| serenity::CreateButton::new(id).label(label).style(style);
    vec![serenity::CreateActionRow::Buttons(vec![
        button("bj:hit", "Ещё", Primary),
        button("bj:stand", "Хватит", Success),
        button("bj:double", "Удвоить", Secondary).disabled(!game.can_double(balance)),
        button("bj:split", "Разделить", Secondary).disabled(!game.can_split(balance)),
    ])]
}

/// Блэкджек на монеты: можно удвоить и разделить (победа ×2, блэкджек ×2.5)
#[poise::command(slash_command, guild_only)]
pub async fn blackjack(
    ctx: Context<'_>,
    #[description = "Сколько монет поставить или all — все монеты"] bet: String,
) -> Result<(), Error> {
    let (data, guild, user) = (ctx.data(), ctx.guild_id().unwrap().get(), ctx.author().id.get());
    let (start, name) = (data.economy.start_balance, display_name(ctx).await);
    let avatar = ctx.author().face();
    let wallet = |data: &crate::Data| data.db.wallet(guild, user, &name, start).map(|(balance, _)| balance);

    // Занимаем место сразу, чтобы нельзя было начать две партии
    if !data.blackjack_players.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(user) {
        return whisper(ctx, "Сначала доиграй текущую партию 🙂").await;
    }
    let _seat = Seat { data, user };
    let bet = match parse_bet(&bet, wallet(data)?, data.economy.min_bet) {
        Ok(bet) => bet,
        Err(problem) => return whisper(ctx, problem).await,
    };
    if !take_money(ctx, bet, "ставка").await? {
        return Ok(());
    }
    // Башмак общий на сервер; замок берём коротко, на один ход, и не держим его во время ожидания Discord
    let with_shoe = |action: &mut dyn FnMut(&mut Vec<Card>)| -> usize {
        let mut shoes = data.blackjack_shoes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let shoe = shoes.entry(guild).or_default();
        action(shoe);
        shoe.len()
    };
    let mut game = None;
    let mut cards_left = with_shoe(&mut |shoe| {
        if shoe.len() < RESHUFFLE_BELOW {
            *shoe = new_shoe(); // карт мало — мешаем новый башмак перед раздачей, а не посреди партии
        }
        let mut dealt = Game::deal(shoe, bet);
        if dealt.natural() {
            dealt.finish(shoe); // у кого-то сразу 21 — партия заканчивается
        }
        game = Some(dealt);
    });
    let mut game = game.unwrap();

    // Выплата и карточка с итогом
    let settle = |game: &Game, cards_left: usize| -> Result<serenity::CreateEmbed, Error> {
        let payout = game.outcome.as_ref().map(|outcome| outcome.payout).unwrap_or(0);
        let balance = data.db.add_money(guild, user, &name, start, payout)?;
        Ok(embed(game, &name, &avatar, cards_left, Some(balance)))
    };
    if game.outcome.is_some() {
        ctx.send(poise::CreateReply::default().embed(settle(&game, cards_left)?)).await?;
        return Ok(());
    }
    let reply = ctx
        .send(poise::CreateReply::default()
            .embed(embed(&game, &name, &avatar, cards_left, None))
            .components(buttons(&game, wallet(data)?)))
        .await?;
    let message_id = reply.message().await?.id;

    loop {
        let press = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
            .message_id(message_id)
            .timeout(TIMEOUT)
            .await;
        let Some(press) = press else {
            // Никто не нажимал кнопки — бот сам говорит «Хватит» и убирает кнопки
            cards_left = with_shoe(&mut |shoe| game.stand_all(shoe));
            let finished = poise::CreateReply::default().embed(settle(&game, cards_left)?).components(Vec::new());
            let _ = reply.edit(ctx, finished).await;
            return Ok(());
        };
        if press.user.id != ctx.author().id {
            let note = serenity::CreateInteractionResponseMessage::new()
                .content("Это не твоя партия 🙂 Начни свою: /blackjack")
                .ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(note)).await?;
            continue;
        }
        let balance = wallet(data)?;
        let refused = match press.data.custom_id.as_str() {
            "bj:hit" => {
                cards_left = with_shoe(&mut |shoe| game.hit(shoe));
                None
            }
            "bj:stand" => {
                cards_left = with_shoe(&mut |shoe| game.stand(shoe));
                None
            }
            "bj:double" if game.can_double(balance) => {
                data.db.add_money(guild, user, &name, start, -game.bets[game.current])?; // ещё одна такая же ставка
                cards_left = with_shoe(&mut |shoe| game.double(shoe));
                None
            }
            "bj:split" if game.can_split(balance) => {
                data.db.add_money(guild, user, &name, start, -game.bets[game.current])?; // ставка на вторую руку
                cards_left = with_shoe(&mut |shoe| game.split(shoe));
                None
            }
            "bj:double" => Some("Удвоить сейчас нельзя 🙂"),
            "bj:split" => Some("Разделить сейчас нельзя 🙂"),
            _ => Some("Не понял кнопку 🙂"),
        };
        if let Some(text) = refused {
            let note = serenity::CreateInteractionResponseMessage::new().content(text).ephemeral(true);
            press.create_response(ctx, serenity::CreateInteractionResponse::Message(note)).await?;
            continue;
        }
        let update = if game.outcome.is_some() {
            serenity::CreateInteractionResponseMessage::new().embed(settle(&game, cards_left)?).components(Vec::new())
        } else {
            serenity::CreateInteractionResponseMessage::new()
                .embed(embed(&game, &name, &avatar, cards_left, None))
                .components(buttons(&game, wallet(data)?))
        };
        press.create_response(ctx, serenity::CreateInteractionResponse::UpdateMessage(update)).await?;
        if game.outcome.is_some() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Карта по названию: "A", "10", "K"... (масть — пики).
    fn card(rank: &str) -> Card {
        (RANKS.iter().position(|name| *name == rank).unwrap() as u8, 0)
    }

    /// Башмак, из которого карты выйдут в указанном порядке (draw берёт с конца).
    fn shoe(order: &[&str]) -> Vec<Card> {
        order.iter().rev().map(|rank| card(rank)).collect()
    }

    fn hand(ranks: &[&str]) -> Vec<Card> {
        ranks.iter().map(|rank| card(rank)).collect()
    }

    #[test]
    fn values() {
        assert_eq!(hand_value(&hand(&["A", "K"])), 21);
        assert_eq!(hand_value(&hand(&["A", "A"])), 12);
        assert_eq!(hand_value(&hand(&["A", "9", "5"])), 15);
        assert_eq!(hand_value(&hand(&["10", "9", "5"])), 24);
        assert_eq!(hand_value(&hand(&["A", "A", "A", "8"])), 21);
        assert_eq!([card_value(card("2")), card_value(card("10")), card_value(card("J")), card_value(card("A"))], [2, 10, 10, 11]);
        assert_eq!(hand_text(&hand(&["A", "10"]), false), "`A♠` `10♠`");
        assert_eq!(hand_text(&hand(&["A", "10"]), true), "`A♠` `🂠`");
        assert_eq!(new_shoe().len(), 156);
    }

    #[test]
    fn player_blackjack_pays_3_to_2() {
        let mut shoe = shoe(&["A", "K", "9", "7"]); // игроку A K, дилеру 9 7
        let mut game = Game::deal(&mut shoe, 100);
        assert!(game.natural());
        game.finish(&mut shoe);
        let outcome = game.outcome.unwrap();
        assert_eq!((outcome.payout, outcome.total_bet), (250, 100));
        assert_eq!(outcome.lines, ["🎉 Блэкджек! +150 монет"]);
        assert_eq!(game.dealer.len(), 2); // при блэкджеке дилер карты не добирает
    }

    #[test]
    fn both_blackjack_is_a_tie() {
        let mut shoe = shoe(&["A", "K", "Q", "A"]);
        let mut game = Game::deal(&mut shoe, 100);
        game.finish(&mut shoe);
        assert_eq!(game.outcome.unwrap().payout, 100);
    }

    #[test]
    fn hit_bust_loses_without_dealer_draw() {
        let mut shoe = shoe(&["10", "6", "9", "5", "K"]); // игрок 16, дилер 14, потом K → перебор
        let mut game = Game::deal(&mut shoe, 100);
        game.hit(&mut shoe);
        let outcome = game.outcome.clone().unwrap();
        assert_eq!(outcome.payout, 0);
        assert_eq!(outcome.lines, ["💥 Перебор, −100 монет"]);
        assert_eq!(game.dealer.len(), 2); // все руки сгорели — дилер не добирает
    }

    #[test]
    fn stand_and_dealer_draws_to_17() {
        let mut shoe = shoe(&["10", "9", "6", "5", "4", "3"]); // игрок 19; дилер 11 → +4 = 15 → +3 = 18
        let mut game = Game::deal(&mut shoe, 100);
        game.stand(&mut shoe);
        assert_eq!(hand_value(&game.dealer), 18);
        assert_eq!(game.outcome.unwrap().payout, 200); // 19 против 18 — победа
    }

    #[test]
    fn dealer_bust_and_push() {
        let mut shoe1 = shoe(&["10", "2", "10", "6", "K"]); // игрок 12 стоит; дилер 16 + K = 26
        let mut game = Game::deal(&mut shoe1, 50);
        game.stand(&mut shoe1);
        assert_eq!(game.outcome.unwrap().lines, ["🎉 Дилер перебрал! +50 монет"]);
        let mut shoe2 = shoe(&["10", "8", "10", "8"]); // 18 против 18
        let mut game = Game::deal(&mut shoe2, 50);
        game.stand(&mut shoe2);
        assert_eq!(game.outcome.unwrap().payout, 50);
    }

    #[test]
    fn double_takes_one_card() {
        let mut shoe = shoe(&["5", "6", "10", "7", "10"]); // игрок 11, дилер 17, удвоение: +10 = 21
        let mut game = Game::deal(&mut shoe, 100);
        assert!(game.can_double(100) && !game.can_double(99));
        game.double(&mut shoe);
        let outcome = game.outcome.clone().unwrap();
        assert_eq!((game.bets[0], game.doubled[0], outcome.total_bet, outcome.payout), (200, true, 200, 400));
        assert_eq!(game.hands[0].len(), 3);
    }

    #[test]
    fn split_plays_two_hands() {
        // игрок 8 8, дилер 10 7; после разделения: первой руке 3 (11), второй потом 10 (18)
        let mut shoe = shoe(&["8", "8", "10", "7", "3", "10", "10"]);
        let mut game = Game::deal(&mut shoe, 100);
        assert!(game.can_split(100) && !game.can_split(50));
        game.split(&mut shoe);
        assert_eq!((game.hands.len(), game.current, hand_value(&game.hands[0])), (2, 0, 11));
        game.hit(&mut shoe); // 11 + 10 = 21 → переход ко второй руке, ей добирается 10 → 18
        assert_eq!((game.current, hand_value(&game.hands[1])), (1, 18));
        assert!(game.outcome.is_none());
        game.stand(&mut shoe);
        let outcome = game.outcome.unwrap();
        assert_eq!(outcome.total_bet, 200);
        assert_eq!(outcome.payout, 400); // обе руки бьют 17 дилера
        assert_eq!(outcome.lines, ["Рука 1: 🎉 Победа! +100 монет", "Рука 2: 🎉 Победа! +100 монет"]);
    }

    #[test]
    fn split_aces_get_one_card_each() {
        let mut shoe = shoe(&["A", "A", "10", "7", "K", "9"]); // после разделения: A K и A 9
        let mut game = Game::deal(&mut shoe, 100);
        game.split(&mut shoe);
        let outcome = game.outcome.clone().expect("разделённые тузы — партия сразу идёт к итогу");
        assert_eq!((game.hands[0].len(), game.hands[1].len()), (2, 2));
        // 21 после разделения — не блэкджек: платит 1 к 1
        assert_eq!(outcome.payout, 400);
        assert_eq!(outcome.lines[0], "Рука 1: 🎉 Победа! +100 монет");
    }

    #[test]
    fn cannot_split_past_four_hands_or_unequal() {
        let mut game = Game::deal(&mut shoe(&["10", "K", "9", "7"]), 10);
        assert!(game.can_split(1000)); // 10 и K — одинаковые по очкам
        game.hands = vec![hand(&["8", "8"]); 4];
        game.bets = vec![10; 4];
        game.doubled = vec![false; 4];
        assert!(!game.can_split(1000));
        let game = Game::deal(&mut shoe(&["10", "9", "9", "7"]), 10);
        assert!(!game.can_split(1000));
    }

    #[test]
    fn timeout_stands_everywhere() {
        let mut shoe = shoe(&["8", "8", "10", "7", "3", "2", "5"]);
        let mut game = Game::deal(&mut shoe, 100);
        game.split(&mut shoe); // рука 1: 8 3; рука 2: одна карта
        game.stand_all(&mut shoe);
        assert!(game.hands.iter().all(|hand| hand.len() == 2));
        assert_eq!(game.outcome.unwrap().total_bet, 200);
    }

    #[test]
    fn empty_shoe_is_refilled() {
        let mut shoe = Vec::new();
        let game = Game::deal(&mut shoe, 10);
        assert_eq!(game.hands[0].len() + game.dealer.len(), 4);
        assert_eq!(shoe.len(), 152);
    }
}
