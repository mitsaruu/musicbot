# Бот на Rust

Это новая версия бота на Rust. Она растёт здесь, пока основной бот (`bot.py`) продолжает работать на сервере.
Обе версии читают одни и те же файлы: `config/settings.ini` и `levels.db`.

## Что уже перенесено
| Часть | Состояние |
|---|---|
| Основа: запуск, настройки, база | готово, проверено тестами |
| Экономика: `/balance`, `/work`, `/daily`, `/give_money`, `/coinflip`, `/slots`, `/blackjack` | готово, проверено тестами; в Discord не запускалось |
| Питомцы: `/case`, `/cases`, `/pets`, `/pets_collect`, `/pet_feed`, `/pet_sell`, `/pet_battle`, `/pet_trade` | готово, проверено тестами; в Discord не запускалось |
| Магазин ролей: `/shop`, `/shop_buy`, `/shop_add`, `/shop_remove` | готово; база проверена тестами, выдача ролей — нет |
| Музыка: `/play` (YouTube, SoundCloud, Spotify), очередь в базе, `/skip`, `/pause`, `/resume`, `/queue`, `/remove_all`, `/setvolume`, `/leave`, кнопки ⏯️⏭️🔁🔀, `/autoplay`, `/playlist_save`, `/playlist_play`, `/playlists`, `/playlist_delete`, `/lyrics`, `/top_chart_month` | готово; очередь, плейлисты, топ и разбор ссылок проверены тестами, поиск, звук и автоплей через настоящий yt-dlp и страница Spotify — тоже. **В голосовом канале Discord не проверялось** |
| Уровни: опыт за сообщения и голос, `/rank`, `/leaderboard`, `/money_leaderboard` | готово; картинки нарисованы и сверены с Python на глаз; начисление опыта в Discord не проверялось |
| Сервер: `/remind`, `/reminders`, `/poll`, `/patchnotes`, `/patchnotes_channel`, `/games_channel`, `/log_channel`, `/warns`, `/warns_clear`, `/weather`, `/weather_cities` | готово; разбор времени, опросы, патчноуты, погода и база проверены тестами; в Discord не запускалось |
| Слова: `/wort`, `/slovo`, правила, проверка по Викисловарю, картинка и итоги в чате, веб-сервер мини-приложения | готово; правила, база и Викисловарь проверены тестами; **веб-сервер и запуск Activity в Discord не проверялись** |
| Слова: `/wordstats`, `/wordtop`, `/wordduel` | готово; подсчёты и база проверены тестами; в Discord не запускалось |
| Мелкие игры: `/dice`, `/roll`, `/predskazanie`, `/shar`, `/dick_growth`, `/dick_size`, `/dick_leaderboard` | готово, проверено тестами; в Discord не запускалось |
| ИИ: `/ask` (с картинкой), `/forget`, `/ai_limits`, `/summary`, `/quiz`, лимиты, память разговора, тайм-ауты за грубость | готово; всё, кроме самого обращения к Gemini, проверено тестами. **Запрос к Gemini с настоящим ключом не проверялся** |
| ИИ в чате: ответы на упоминания и на ответы боту (с картинками и контекстом чата), автоответы из `config/auto_replies.txt`, ответы другим ботам, случайные реплики | готово; разбор автоответов, приветствия «не боту» и настройки проверены тестами; в Discord не запускалось |
| Модерация: варны и тайм-ауты по просьбе модератора через ИИ, автонаказания за варны (`warn_punishments`), анти-спам | готово; разбор меток, пороги варнов и анти-спам проверены тестами; **выдача тайм-аутов в Discord не проверялась** |
| Сервер: `/welcome_channel` (приветствие с карточкой), `/daily_setup`, `/daily_off`, `/daily_test` | готово; расписание, часовые пояса и карточка проверены тестами; в Discord не запускалось |

## Что нужно для сборки
- Rust (https://rustup.rs).
- **CMake** — для музыки (собирает звуковой кодек Opus): https://cmake.org/download/ или `winget install Kitware.CMake`.
  На Linux: `sudo apt install cmake build-essential`.
- Для музыки при работе: `yt-dlp` и `deno`, как и для `bot.py`.

## Сборка и тесты
```
cargo test                          # тесты
cargo test -- --ignored             # с интернетом: yt-dlp, Spotify, Викисловарь, погода, Gemini (нужен GEMINI_API_KEY)
cargo build --release               # бот целиком (нужен CMake)
cargo build --release --no-default-features   # без музыки (CMake не нужен)
```
Готовый файл — `target/release/musicbot` (на Windows — `musicbot.exe`).

## Запуск
Бот берёт настройки из переменных окружения:
- `DISCORD_TOKEN` — токен бота (обязательно);
- `TEST_GUILD_ID` — id сервера для проверки: команды появятся только на нём и сразу;
- `REGISTER_GLOBAL=1` — зарегистрировать команды глобально;
- `BOT_DIR` — папка с `config/` и `levels.db` (по умолчанию — текущая или папка выше);
- `GEMINI_API_KEY` — ключ Gemini для ИИ (без него `/ask`, `/summary` и `/quiz` отвечают, что ИИ не настроен);
- `DISCORD_CLIENT_SECRET` — секрет приложения (Developer Portal → OAuth2) для мини-приложения игр в слова;
  без него `/wort` и `/slovo` отвечают, что мини-приложение не настроено;
- `SPOTIFY_CLIENT_ID`, `SPOTIFY_CLIENT_SECRET` — ключи Spotify API (необязательно: без них ссылки Spotify читаются со страницы плеера);
- `YTDLP` — как запускать yt-dlp (по умолчанию `yt-dlp`; на сервере — `venv/bin/python -m yt_dlp`).

**Важно: не запускай Rust-бота с тем же токеном, что и `bot.py`, одновременно.** Оба будут отвечать на команды,
а `REGISTER_GLOBAL=1` заменит список команд `bot.py` на список Rust-версии.
Боту нужны разрешения в Developer Portal → Bot: **Message Content Intent** (как и для `bot.py`), а для приветствий —
**Server Members Intent** и `members_intent = on` в `settings.ini`.

Для проверки лучше создать в Developer Portal отдельного тестового бота и запускать Rust-версию с его токеном
и `TEST_GUILD_ID`.
