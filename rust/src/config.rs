//! Чтение config/settings.ini — того же файла, что читает bot.py.
//! Формат: разделы `[name]`, строки `ключ = значение`, комментарии начинаются с `;` или `#`.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Settings {
    /// (раздел, ключ в нижнем регистре) → значение
    values: HashMap<(String, String), String>,
}

impl Settings {
    /// Читает файл. Если файла нет или он не читается — пустые настройки (везде значения по умолчанию).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) => {
                println!("settings.ini не прочитан ({error}) — беру значения по умолчанию");
                Self::default()
            }
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut values = HashMap::new();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
                section = name.trim().to_string();
            } else if let Some((key, value)) = line.split_once('=') {
                values.insert((section.clone(), key.trim().to_lowercase()), value.trim().to_string());
            }
        }
        Self { values }
    }

    /// Строка из настроек; если её нет или она пустая — значение по умолчанию.
    pub fn text(&self, section: &str, key: &str, default: &str) -> String {
        match self.values.get(&(section.to_string(), key.to_lowercase())) {
            Some(value) if !value.is_empty() => value.clone(),
            _ => default.to_string(),
        }
    }

    /// Целое число из настроек; если его нет или там не число — значение по умолчанию.
    pub fn int(&self, section: &str, key: &str, default: i64) -> i64 {
        match self.values.get(&(section.to_string(), key.to_lowercase())) {
            None => default,
            Some(value) => value.parse().unwrap_or_else(|_| {
                println!("settings.ini: «{key} = {value}» — нужно целое число, беру {default}");
                default
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_and_defaults() {
        let settings = Settings::parse(
            "; комментарий\n[economy]\nstart_balance = 500\nWork_Min = 150\nbad = abc\nempty =\n\n[ai]\nmodel = gemini\n",
        );
        assert_eq!(settings.int("economy", "start_balance", 1), 500);
        assert_eq!(settings.int("economy", "work_min", 1), 150);
        assert_eq!(settings.int("economy", "bad", 7), 7);
        assert_eq!(settings.int("economy", "missing", 9), 9);
        assert_eq!(settings.text("ai", "model", "x"), "gemini");
        assert_eq!(settings.text("economy", "empty", "запасное"), "запасное");
        assert_eq!(settings.text("nope", "model", "x"), "x");
    }
}
