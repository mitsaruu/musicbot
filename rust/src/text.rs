//! Мелкие функции для текста: числа, склонения, время. Ведут себя так же, как одноимённые в bot.py.

/// Выбирает форму слова по числу: plural(1, "монета", "монеты", "монет") → "монета".
pub fn plural<'a>(n: i64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let n = n.abs();
    if n % 10 == 1 && n % 100 != 11 {
        one
    } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
        few
    } else {
        many
    }
}

/// Число с пробелами между тысячами: 15430 → "15 430".
pub fn number(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut result = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            result.push(' ');
        }
        result.push(digit);
    }
    if value < 0 {
        result.insert(0, '-');
    }
    result
}

/// Сумма с правильным словом: "1 монета", "3 монеты", "15 430 монет".
pub fn money_text(amount: i64) -> String {
    format!("{} {}", number(amount), plural(amount, "монета", "монеты", "монет"))
}

/// Секунды → "8 часов", "24 часа", "1 час".
pub fn hours_text(seconds: i64) -> String {
    let hours = seconds / 3600;
    format!("{hours} {}", plural(hours, "час", "часа", "часов"))
}

/// Сколько осталось ждать: "5 ч 12 мин" или "3 мин" (меньше минуты — всё равно "1 мин").
pub fn time_left_text(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as i64;
    let (hours, minutes) = (seconds / 3600, seconds % 3600 / 60);
    if hours > 0 {
        format!("{hours} ч {minutes} мин")
    } else {
        format!("{} мин", minutes.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_forms() {
        let word = |n| plural(n, "монета", "монеты", "монет");
        assert_eq!([word(1), word(2), word(5), word(11), word(12), word(21), word(22), word(25), word(111), word(0)],
                   ["монета", "монеты", "монет", "монет", "монет", "монета", "монеты", "монет", "монет", "монет"]);
    }

    #[test]
    fn numbers_and_money() {
        assert_eq!(number(0), "0");
        assert_eq!(number(999), "999");
        assert_eq!(number(1000), "1 000");
        assert_eq!(number(15430), "15 430");
        assert_eq!(number(1234567), "1 234 567");
        assert_eq!(number(-2500), "-2 500");
        assert_eq!(money_text(1), "1 монета");
        assert_eq!(money_text(15430), "15 430 монет");
    }

    #[test]
    fn time_texts() {
        assert_eq!(hours_text(8 * 3600), "8 часов");
        assert_eq!(hours_text(24 * 3600), "24 часа");
        assert_eq!(hours_text(3600), "1 час");
        assert_eq!(time_left_text(5.0 * 3600.0 + 12.0 * 60.0), "5 ч 12 мин");
        assert_eq!(time_left_text(200.0), "3 мин");
        assert_eq!(time_left_text(20.0), "1 мин");
    }
}
