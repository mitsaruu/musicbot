# ============================================================
#  Музыкальный бот для Discord со слэш-командами (discord.py 2.7+ и yt-dlp)
#  Музыка:  /play, /skip, /pause, /resume, /queue, /remove_all, /setvolume, /leave, /top_chart_month
#           (очередь хранится в базе и не пропадает после /leave и перезапуска)
#  Уровни:  /rank, /leaderboard  (опыт за сообщения в чате и за время в голосовом канале)
#  Разное:  /weather, /weather_cities, /dice, /roll, /divination, /pogonyalo
#  Игра:    /dick_growth, /dick_size, /dick_leaderboard
#  Деньги:  /work, /balance, /give_money, /money_leaderboard, /coinflip, /slots, /blackjack
#  Каждый день: /daily_setup, /daily_off, /daily_test — утренние и вечерние сообщения с фактами
#  Каналы:  /games_channel — ранги, рейтинги и игры работают только в выбранном канале
#           /log_channel — куда бот присылает свои ошибки (а ещё они всегда пишутся в bot.log)
#  ИИ:      характер тсундере; /ask, /ai_limits, а также ответ, если упомянуть бота (@бот) или ответить ему
#           память: /remember, /memory, /forget (бот помнит разговор и факты о людях)
#           картинки: бот видит изображения в сообщении и в том, на которое ответили
#  Автоответы на слова «привет» и «даня»
#  Грубость: на мат и непотребства ИИ не отвечает (слова — в config/bad_words.txt, остальное решает сам ИИ),
#           со второго раза подряд — тайм-аут: 1, 5, 10, 30, 60 минут (settings.ini, раздел [moderation])
#  Настройки: папка config — prompt.txt (характер ИИ), auto_replies.txt (автоответы),
#             bad_words.txt (слова, на которые бот молчит), settings.ini (числа)
# ============================================================

import asyncio   # стандартная библиотека Python для асинхронного кода (бот делает много дел "одновременно")
import datetime  # даты: какой сегодня день и месяц (для погоняла на день и топа треков за месяц)
import io        # "файлы в памяти": картинку не сохраняем на диск, а сразу отправляем в Discord
import json      # разбор данных в формате JSON (например, списка треков со страницы плеера Spotify)
import os        # нужна, чтобы прочитать токен бота из переменной окружения
import random    # случайные числа (сколько опыта дать за сообщение)
import re        # "регулярные выражения" — поиск шаблонов в тексте (ссылки Spotify, слова для автоответов)
import sqlite3   # встроенная в Python база данных — в ней храним опыт участников
import time      # текущее время (для защиты от накрутки опыта)
import logging   # запись ошибок в файл bot.log
import sys       # sys.exc_info() — узнать, какая ошибка сейчас случилась
import traceback # подробности ошибки: в каком файле и на какой строке она произошла
from logging.handlers import RotatingFileHandler  # файл логов, который не разрастается бесконечно

from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageOps  # Pillow — библиотека для рисования картинок

import aiohttp                        # запросы к сайтам из асинхронного кода (ставится вместе с discord.py)
import discord                        # сама библиотека для работы с Discord
from discord import app_commands      # модуль для слэш-команд (тех, что начинаются с "/")
from discord.ext import commands, tasks  # commands — класс Bot, tasks — задачи, которые повторяются по таймеру
import yt_dlp                         # библиотека, которая находит видео на YouTube и достаёт ссылку на звук
from google import genai                                # библиотека Google для работы с ИИ Gemini (пакет google-genai)
from google.genai import types                          # настройки запроса к Gemini
from google.genai import errors as genai_errors         # виды ошибок Gemini (например, ServerError — 503 "перегружен")
import spotipy                                          # библиотека для работы с Spotify API
from spotipy.oauth2 import SpotifyClientCredentials     # вход в Spotify API по ключам приложения


# ============================================================
#  НАСТРОЙКИ ИЗ ПАПКИ config — их можно менять, не трогая код
#  config/prompt.txt       — характер ИИ (подхватывается сразу после сохранения)
#  config/auto_replies.txt — автоответы «слово = ответ» (подхватываются сразу)
#  config/settings.ini     — числа и параметры (после изменения нужен перезапуск бота)
#  Если файлов нет, бот при запуске сам создаст их с настройками по умолчанию.
# ============================================================

import configparser  # стандартная библиотека Python для чтения файлов вида settings.ini

BASE_DIR = os.path.dirname(os.path.abspath(__file__))   # папка, где лежит bot.py (база и фоны тоже здесь)
CONFIG_DIR = os.path.join(BASE_DIR, "config")           # папка с настройками
PROMPT_PATH = os.path.join(CONFIG_DIR, "prompt.txt")
AUTO_REPLIES_PATH = os.path.join(CONFIG_DIR, "auto_replies.txt")
BAD_WORDS_PATH = os.path.join(CONFIG_DIR, "bad_words.txt")
SETTINGS_PATH = os.path.join(CONFIG_DIR, "settings.ini")

# Характер ИИ по умолчанию (если prompt.txt пустой или его нет)
DEFAULT_CHARACTER = """Ты — бот в Discord-чате с характером тсундере. Говоришь о себе в женском роде.
Снаружи ты ворчливая и колючая, делаешь вид, что помогаешь нехотя и тебе всё равно
("Н-не то чтобы я хотела тебе помогать!", "Хмф, бака!", "Только в этот раз, понял?"),
но на самом деле всегда отвечаешь правильно и полезно и немного переживаешь за собеседника.
Не переигрывай: максимум одна короткая тсундере-фраза, а сама суть ответа — чёткая и верная.
Не оскорбляй всерьёз и не груби по-настоящему — только в шутку.
Отвечай на языке собеседника. Отвечай ОЧЕНЬ КОРОТКО: одной строкой, 1–2 коротких предложения, без абзацев, переносов строк,
вступлений и повторов. Подробнее — только если прямо просят или нужно решить задачу/написать код."""

DEFAULT_PROMPT_FILE = """# Характер ИИ-бота. Пиши обычным текстом — так, как объяснил бы человеку.
# Например, чтобы бот отвечал только на украинском, допиши строку: Всегда отвечай только на украинском языке.
# Строки, которые начинаются с #, — пояснения, ИИ их не видит.
# Правила оформления формул и памяти бот добавляет сам — их сюда писать не нужно.
# После сохранения файла перезапускать бота не нужно: новый характер подхватится со следующего вопроса.

""" + DEFAULT_CHARACTER + "\n"

DEFAULT_AUTO_REPLIES_FILE = """# Автоответы бота. Каждая строка: слово = ответ
# Несколько слов через запятую:          привет, здарова = привет милый
# Несколько ответов через | (бот выберет случайный):  даня = это который сидит на красном стуле? | опять он
# Строки, которые начинаются с #, — пояснения, бот их пропускает.
# Большие и маленькие буквы не важны. После сохранения файла перезапускать бота не нужно.

привет = привет милый
даня = это который сидит на красном стуле?
"""

DEFAULT_BAD_WORDS_FILE = """# Слова, на которые бот не отвечает: если к боту обратились с одним из них — ИИ промолчит.
# Одно слово на строке (можно и через запятую). Звёздочка * — «любые буквы»:
#   бля     — только само слово «бля»
#   ебал*   — слова, которые НАЧИНАЮТСЯ с «ебал»: ебал, ебала, ебалом...
#   *пизд*  — слова, в которых ВНУТРИ есть «пизд»
# Осторожно со звёздочками с двух сторон: «*бля*» поймает и «употребляю».
# Ё и Е считаются одной буквой, большие и маленькие буквы не важны.
# Строки, которые начинаются с #, — пояснения. После сохранения файла перезапускать бота не нужно.
# Замаскированный мат (х*й, х у й) и непотребства без матных слов ловит сам ИИ.

хуй, хуя, хуи, хую, хуем, хуе*, *хуйн*, нахуй, похуй, дохуя, охуе*, охуи*
*пизд*
бля, блять, бляд*
ебать, ебал*, ебан*, ебну*, ебуч*, *ебись, заеб*, наеб*, выеб*, уеб*, отъеб*, съеб*, доеб*, долбоеб*
сука, суки, суку, сукой, сучка*, сучар*
мудак*, мудил*, пидор*, пидар*, педик, педики, педика, гандон*, залуп*, шлюх*, шалав*, дроч*
"""

DEFAULT_SETTINGS_FILE = """; Основные настройки бота. Меняй значения после знака «=».
; Строки, которые начинаются с «;», — пояснения, бот их пропускает.
; После изменения этого файла бота нужно ПЕРЕЗАПУСТИТЬ.
; Если удалить строку или ошибиться — бот возьмёт значение по умолчанию и напишет об этом в консоль.

[ai]
; Модель Gemini: gemini-3.5-flash-lite — быстрая и с большими лимитами, gemini-3.8-flash — умнее, но лимиты меньше
model = gemini-3.5-flash-lite
; Сколько ИИ «обдумывает» ответ: minimal (только у Flash-Lite), low, medium, high или none
thinking_level = high
; Запасная модель: если основная перегружена (ошибка 503) или кончился её лимит — бот спросит эту.
; none — без запасной модели
fallback_model = gemini-3.8-flash
; Сколько раз повторить запрос, если Google ответил «перегружен» (503), прежде чем перейти на запасную модель
retries = 2
; Склеивать ответ ИИ в одну строку, без абзацев: on или off (ответы с кодом не склеиваются)
one_line_answers = on
; Сколько вопросов к ИИ может задать один человек за день
daily_limit_per_user = 50
; Через сколько секунд можно задать следующий вопрос
cooldown_seconds = 5
; Сколько последних сообщений разговора ИИ помнит в каждом канале
history_limit = 20
; Примерные лимиты Google для этой модели, в день и в минуту (только для показа в /ai_limits)
google_daily_limit = 500
google_minute_limit = 15

[levels]
; Опыт за сообщение (случайно от min до max) и как часто он даётся (в секундах)
xp_per_message_min = 15
xp_per_message_max = 25
xp_cooldown_seconds = 60
; Опыт за каждую минуту в голосовом канале и сколько людей должно быть в канале
voice_xp_per_minute = 10
voice_min_people = 2
; Что писать, когда участник получает новый уровень: off — ничего, text — только текст, card — текст и карточка
level_up_message = off

[economy]
; Монеты новичку
start_balance = 500
; Заработок в /work (случайно от min до max) и как часто можно работать (в часах)
work_min = 150
work_max = 400
work_cooldown_hours = 8
; Минимальная ставка в играх
min_bet = 10

[dick_game]
; На сколько сантиметров может измениться размер за раз и как часто можно играть (в часах)
growth_min = -10
growth_max = 10
growth_cooldown_hours = 24

[channels]
; Команды, которые работают только в канале для игр (его выбирают в Discord командой /games_channel).
; Пиши названия через запятую. Пока канал не выбран, все команды работают везде.
games_commands = rank, leaderboard, top_chart_month, money_leaderboard, dick_growth, dick_size, dick_leaderboard, work, balance, give_money, coinflip, slots, blackjack, dice, roll, divination, pogonyalo

[moderation]
; Тайм-ауты за грубость к боту (мат, оскорбления, непотребства). Первый раз бот просто молчит,
; со второго раза подряд — тайм-аут. Длительности в минутах по порядку: 2-й раз, 3-й, 4-й...
; Дальше каждый раз — последняя из списка. off — не давать тайм-аутов (бот будет только молчать).
timeouts = 1, 5, 10, 30, 60
; Через сколько минут без грубостей счётчик обнуляется (вежливое сообщение боту тоже обнуляет его)
strike_reset_minutes = 60
; Писать ли в чат, что человек получил тайм-аут: on или off
announce_timeouts = on
"""


def create_default_configs():
    """Если папки config или файлов в ней нет — создаёт их с настройками по умолчанию."""
    os.makedirs(CONFIG_DIR, exist_ok=True)  # exist_ok=True — не ругаться, если папка уже есть
    for path, text in ((PROMPT_PATH, DEFAULT_PROMPT_FILE),
                       (AUTO_REPLIES_PATH, DEFAULT_AUTO_REPLIES_FILE),
                       (BAD_WORDS_PATH, DEFAULT_BAD_WORDS_FILE),
                       (SETTINGS_PATH, DEFAULT_SETTINGS_FILE)):
        if not os.path.exists(path):
            with open(path, "w", encoding="utf-8") as file:
                file.write(text)
            print("Создан файл настроек:", path)


def read_text(path):
    """Читает текстовый файл. Сначала как UTF-8, а если не вышло — как старую русскую кодировку Windows."""
    with open(path, "rb") as file:  # "rb" — читаем как байты, а кодировку выбираем сами
        data = file.read()
    try:
        return data.decode("utf-8-sig")  # utf-8-sig — Блокнот иногда добавляет в начало невидимый символ
    except UnicodeDecodeError:
        return data.decode("cp1251", errors="replace")


create_default_configs()

# --- settings.ini: числа и параметры (читаются один раз при запуске) ---
settings = configparser.ConfigParser()
try:
    settings.read_string(read_text(SETTINGS_PATH))
except (OSError, configparser.Error) as error:
    print("Не получилось прочитать settings.ini, беру настройки по умолчанию:", error)


def setting(section, key, default):
    """Значение из settings.ini. Если его нет или оно записано с ошибкой — значение по умолчанию."""
    try:
        value = settings.get(section, key).strip()
    except configparser.Error:
        return default  # такой строки (или раздела) в файле нет
    if isinstance(default, int):  # здесь ждём целое число
        try:
            return int(value)
        except ValueError:
            print(f"settings.ini: «{key} = {value}» — нужно целое число, беру {default}")
            return default
    return value or default


# --- prompt.txt и auto_replies.txt: перечитываются сами, как только файл изменили ---
_file_cache = {}  # путь к файлу → (время последнего изменения файла, уже разобранное содержимое)


def cached_file(path, parse, default):
    """Читает файл и разбирает его функцией parse. Пока файл не меняли — отдаёт сохранённый результат.
    Если файла нет или он не читается — возвращает default."""
    try:
        changed = os.path.getmtime(path)  # время последнего изменения файла
    except OSError:
        return default                    # файла нет
    cached = _file_cache.get(path)
    if cached and cached[0] == changed:   # файл не менялся — берём готовое
        return cached[1]
    try:
        result = parse(read_text(path))
    except Exception as error:
        print("Не получилось прочитать", path, error)
        return cached[1] if cached else default
    _file_cache[path] = (changed, result)
    return result


def parse_prompt(text):
    """Убирает из prompt.txt строки-пояснения (начинаются с #)."""
    lines = [line for line in text.splitlines() if not line.lstrip().startswith("#")]
    return "\n".join(lines).strip()


def ai_character():
    """Характер ИИ из config/prompt.txt."""
    return cached_file(PROMPT_PATH, parse_prompt, "") or DEFAULT_CHARACTER


def parse_auto_replies(text):
    """Строки «слово = ответ» → список пар (шаблон для поиска слова, варианты ответа)."""
    rules = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:  # пустые строки и пояснения пропускаем
            continue
        words, answers = line.split("=", 1)  # делим по первому «=»: слева слова, справа ответы
        options = [answer.strip() for answer in answers.split("|") if answer.strip()]
        for word in words.split(","):
            word = word.strip().lower()
            if word and options:
                # \b — граница слова: "привет" сработает в "привет всем", но не в "приветствую"
                rules.append((re.compile(rf"\b{re.escape(word)}\b"), options))
    return rules


def auto_replies():
    """Автоответы из config/auto_replies.txt."""
    return cached_file(AUTO_REPLIES_PATH, parse_auto_replies, [])


LETTER = "[0-9a-zа-я]"  # "буква" для поиска плохих слов (ё заранее заменяем на е)


def parse_bad_words(text):
    """Строки из bad_words.txt → один шаблон для поиска (или None, если слов нет).
    Звёздочка * превращается в "любые буквы", а края слова проверяются, чтобы «сука» не ловилась в «сукно»."""
    patterns = []
    for line in text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        for word in line.split(","):
            word = word.strip().lower().replace("ё", "е")
            if word.strip("*"):
                body = f"{LETTER}*".join(re.escape(part) for part in word.split("*"))
                # (?<!буква) и (?!буква) — до и после слова не должно быть других букв
                patterns.append(f"(?<!{LETTER}){body}(?!{LETTER})")
    return re.compile("|".join(patterns)) if patterns else None


def is_rude(text):
    """Есть ли в тексте слова из config/bad_words.txt."""
    pattern = cached_file(BAD_WORDS_PATH, parse_bad_words, None)
    return bool(pattern and pattern.search(text.lower().replace("ё", "е")))


# Какие команды работают только в канале для игр (если его выбрали через /games_channel).
# Список берётся из settings.ini, раздел [channels], строка games_commands — названия через запятую.
DEFAULT_GAMES_COMMANDS = ("rank, leaderboard, top_chart_month, money_leaderboard, "
                          "dick_growth, dick_size, dick_leaderboard, work, balance, give_money, "
                          "coinflip, slots, blackjack, dice, roll, divination, pogonyalo")
GAMES_COMMANDS = {name.strip().lstrip("/").lower()
                  for name in setting("channels", "games_commands", DEFAULT_GAMES_COMMANDS).split(",")
                  if name.strip()}


def games_channel_for(guild):
    """Канал для игр на сервере (объект канала) или None, если его не выбирали или его удалили."""
    row = db.execute("SELECT channel_id FROM games_channel WHERE guild_id = ?", (guild.id,)).fetchone()
    return guild.get_channel(row[0]) if row else None


async def games_channel_check(interaction):
    """Можно ли выполнить эту команду здесь. Если нельзя — тихо подсказывает, где можно, и возвращает False."""
    name = (interaction.data or {}).get("name", "")  # название вызванной команды, например "rank"
    if interaction.guild is None or name not in GAMES_COMMANDS:
        return True                      # личные сообщения или команда не из списка — можно везде
    games = games_channel_for(interaction.guild)
    if games is None:
        return True                      # канал для игр не выбран — можно везде
    channel = interaction.channel
    if interaction.channel_id == games.id or getattr(channel, "parent_id", None) == games.id:
        return True                      # это сам канал для игр или ветка внутри него
    if interaction.type == discord.InteractionType.application_command:
        # ephemeral=True — сообщение увидит только тот, кто вызвал команду
        await interaction.response.send_message(f"🎮 Эта команда работает только в {games.mention}", ephemeral=True)
    return False


def hours_text(seconds):
    """Секунды → текст вроде «8 часов», «24 часа», «1 час»."""
    hours = int(seconds // 3600)
    return f"{hours} {plural(hours, 'час', 'часа', 'часов')}"


# Токен бота берём из переменной окружения DISCORD_TOKEN (так он не хранится в коде).
TOKEN = os.getenv("DISCORD_TOKEN")

# Ключи приложения Spotify — тоже из переменных окружения.
SPOTIFY_CLIENT_ID = os.getenv("SPOTIFY_CLIENT_ID")
SPOTIFY_CLIENT_SECRET = os.getenv("SPOTIFY_CLIENT_SECRET")

# Ключ для ИИ Gemini — тоже из переменной окружения. Без него бот работает, просто без ИИ.
GEMINI_API_KEY = os.getenv("GEMINI_API_KEY")

# Подключаемся к Spotify, только если оба ключа заданы. Иначе spotify = None и ссылки Spotify не работают.
spotify = None
if SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET:
    spotify = spotipy.Spotify(
        auth_manager=SpotifyClientCredentials(        # spotipy сам получает и обновляет временный пропуск в API
            client_id=SPOTIFY_CLIENT_ID,
            client_secret=SPOTIFY_CLIENT_SECRET,
        )
    )

# Шаблон ссылки Spotify. Находит тип (track, album или playlist) и id, например:
# https://open.spotify.com/intl-ru/track/4cOdK2wGLETKBW3PvgPWqT?si=... → ("track", "4cOdK2wGLETKBW3PvgPWqT")
SPOTIFY_LINK = re.compile(r"open\.spotify\.com/(?:intl-[a-z]+/)?(track|album|playlist)/([A-Za-z0-9]+)")

# Сколько треков максимум брать из одного альбома (каждый ищется на YouTube, это небыстро).
MAX_ALBUM_TRACKS = 20
# И из одного плейлиста. Треки ищутся на YouTube по одному, перед включением, так что длинный плейлист не тормозит.
MAX_PLAYLIST_TRACKS = 100

# Настройки для yt-dlp:
YDL_OPTIONS = {
    "format": "bestaudio/best",     # брать лучший доступный аудиопоток
    "noplaylist": True,             # если дали ссылку на плейлист — брать только одно видео
    "default_search": "ytsearch",   # если дали не ссылку, а текст — искать его на YouTube
    "quiet": True,                  # не засорять консоль лишними сообщениями
}

# Настройки для FFmpeg (программа, которая перекодирует звук в формат Discord):
FFMPEG_OPTIONS = {
    # если интернет-поток оборвётся, FFmpeg попробует переподключиться
    "before_options": "-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 5",
    "options": "-vn",  # -vn = "no video": нам нужен только звук
}

# "Intents" — разрешения на то, какие события бот получает от Discord.
intents = discord.Intents.default()  # стандартный набор (в нём есть и голосовые каналы)
intents.message_content = True       # разрешаем читать текст сообщений — нужно для автоответов
                                     # (в Developer Portal должен быть включён Message Content Intent)

class BotTree(app_commands.CommandTree):
    """"Дерево" слэш-команд бота. interaction_check вызывается перед КАЖДОЙ командой —
    здесь проверяем, можно ли использовать команду в этом канале."""

    async def interaction_check(self, interaction):
        return await games_channel_check(interaction)

    async def on_error(self, interaction, error):
        """Вызывается, если внутри слэш-команды случилась ошибка, которую никто не поймал."""
        if isinstance(error, app_commands.CheckFailure):  # например, команду вызвали не там — это не поломка
            return
        error = getattr(error, "original", error)  # discord.py заворачивает ошибку — достаём настоящую
        name = interaction.command.qualified_name if interaction.command else "?"
        await report_error(f"Команда /{name}", error, interaction.guild, interaction.user, interaction.channel)
        text = "😵 Что-то пошло не так. Ошибка уже записана в логи."
        try:
            if interaction.response.is_done():  # на команду уже начали отвечать — пишем новым сообщением
                await interaction.followup.send(text, ephemeral=True)
            else:
                await interaction.response.send_message(text, ephemeral=True)
        except Exception:
            pass  # ответить не получилось (например, команда устарела) — в логах ошибка всё равно есть


# Создаём бота. command_prefix здесь не используется, но его обязательно нужно указать.
# tree_cls=BotTree — используем наше "дерево" команд с проверкой канала.
bot = commands.Bot(command_prefix="!", intents=intents, tree_cls=BotTree)


# ============================================================
#  Логи ошибок
#  Каждая ошибка пишется в консоль и в файл bot.log (рядом с bot.py; старые записи
#  уходят в bot.log.1, bot.log.2...). Если на сервере выбрали канал командой /log_channel,
#  бот ещё и присылает туда карточку: где случилась ошибка, у кого и подробности.
# ============================================================
error_logger = logging.getLogger("musicbot")
error_logger.setLevel(logging.INFO)
_log_file = RotatingFileHandler(os.path.join(BASE_DIR, "bot.log"),
                                maxBytes=2_000_000, backupCount=3, encoding="utf-8")  # до 2 МБ на файл
_log_file.setFormatter(logging.Formatter("%(asctime)s [%(levelname)s] %(message)s"))
error_logger.addHandler(_log_file)


def log_channel_for(guild_id):
    """Канал для логов на сервере (объект канала) или None, если его не выбирали или удалили."""
    if guild_id is None:
        return None
    row = db.execute("SELECT channel_id FROM log_channel WHERE guild_id = ?", (guild_id,)).fetchone()
    return bot.get_channel(row[0]) if row else None


async def report_error(where, error, guild=None, user=None, channel=None):
    """Записывает ошибку в консоль и bot.log и присылает её в канал логов сервера (если он выбран).
    where — где случилась ошибка (например, "ИИ /ask"); guild — сервер (объект или id); user и channel — по желанию."""
    details = "".join(traceback.format_exception(type(error), error, error.__traceback__)).strip()
    guild_id = getattr(guild, "id", guild)  # можно передать и сам сервер, и просто его id
    print(f"{where}: {error!r}")
    error_logger.error("%s | сервер=%s | пользователь=%s | канал=%s\n%s",
                       where, guild_id, getattr(user, "id", user), getattr(channel, "id", channel), details)

    log_channel = log_channel_for(guild_id)
    if log_channel is None:
        return
    embed = discord.Embed(title=f"⚠️ Ошибка: {where}"[:256], color=0xEF4444,
                          timestamp=datetime.datetime.now(datetime.timezone.utc))
    embed.add_field(name="Что случилось", value=f"`{type(error).__name__}`: {str(error)[:900] or '—'}", inline=False)
    if user is not None:
        embed.add_field(name="Кто", value=getattr(user, "mention", str(user)))
    if channel is not None:
        embed.add_field(name="Где", value=getattr(channel, "mention", str(channel)))
    embed.description = f"```py\n{details[-3800:]}\n```"  # конец трейсбэка — самое важное место
    try:
        await log_channel.send(embed=embed, allowed_mentions=discord.AllowedMentions.none())  # без пингов
    except Exception as send_error:
        print("Не получилось отправить лог в канал:", send_error)

# Громкость для каждого сервера в процентах. Ключ — id сервера, значение — число (100 = обычная громкость).
volumes = {}

# Что сейчас играет на каждом сервере: id сервера → (ссылка на трек, название).
# Нужно, чтобы при /leave вернуть текущий трек в начало сохранённой очереди.
now_playing = {}

# "Замки" для каждого сервера. Не дают включить два трека одновременно,
# если play_next вызвали сразу из двух мест (например, /play и окончание песни).
play_locks = {}


def get_volume(guild_id):
    """Возвращает громкость сервера в процентах; если её ещё не задавали — 100."""
    return volumes.get(guild_id, 100)


def get_lock(guild_id):
    """Возвращает "замок" сервера (создаёт, если его ещё нет)."""
    return play_locks.setdefault(guild_id, asyncio.Lock())


# --- Очередь песен хранится в базе данных (таблица queue), поэтому она не пропадает
#     ни после /leave, ни после перезапуска бота. ---

def queue_add(guild_id, url, title):
    """Добавляет трек в конец очереди. url — ссылка на страницу трека (или поисковый запрос)."""
    db.execute("INSERT INTO queue (guild_id, url, title) VALUES (?, ?, ?)", (guild_id, url, title))
    db.commit()


def queue_push_front(guild_id, url, title):
    """Добавляет трек в самое начало очереди (номер меньше, чем у всех остальных)."""
    smallest = db.execute("SELECT MIN(id) FROM queue").fetchone()[0]  # самый маленький номер в таблице
    new_id = (smallest if smallest is not None else 1) - 1             # ставим номер ещё меньше
    db.execute("INSERT INTO queue (id, guild_id, url, title) VALUES (?, ?, ?, ?)", (new_id, guild_id, url, title))
    db.commit()


def queue_pop(guild_id):
    """Достаёт первый трек очереди и удаляет его оттуда. Возвращает (ссылка, название) или None."""
    row = db.execute(
        "SELECT id, url, title FROM queue WHERE guild_id = ? ORDER BY id LIMIT 1", (guild_id,)
    ).fetchone()
    if row is None:  # очередь пуста
        return None
    db.execute("DELETE FROM queue WHERE id = ?", (row[0],))
    db.commit()
    return row[1], row[2]


def queue_titles(guild_id, limit=15):
    """Названия первых limit треков очереди."""
    rows = db.execute(
        "SELECT title FROM queue WHERE guild_id = ? ORDER BY id LIMIT ?", (guild_id, limit)
    ).fetchall()
    return [row[0] for row in rows]


def queue_count(guild_id):
    """Сколько треков в очереди."""
    return db.execute("SELECT COUNT(*) FROM queue WHERE guild_id = ?", (guild_id,)).fetchone()[0]


def queue_clear(guild_id):
    """Удаляет всю очередь сервера. Возвращает, сколько треков удалено."""
    cursor = db.execute("DELETE FROM queue WHERE guild_id = ?", (guild_id,))
    db.commit()
    return cursor.rowcount  # rowcount — сколько строк затронула команда


async def search(query):
    """Ищет песню через yt-dlp. Возвращает (ссылка_на_звук, название, ссылка_на_страницу_трека)."""
    loop = asyncio.get_running_loop()  # получаем текущий "цикл событий" бота

    def extract():
        # Эта функция работает медленно (ходит в интернет),
        # поэтому запускаем её в отдельном потоке, чтобы бот не "зависал".
        with yt_dlp.YoutubeDL(YDL_OPTIONS) as ydl:
            info = ydl.extract_info(query, download=False)  # download=False — не скачивать, только получить данные
            if "entries" in info:          # если был поиск по тексту, результат — список
                info = info["entries"][0]  # берём первый найденный результат
            # Ссылка на звук (info["url"]) через несколько часов перестаёт работать,
            # поэтому в очереди храним ссылку на страницу трека и получаем звук заново перед включением.
            page_url = info.get("webpage_url") or query
            return info["url"], info.get("title", "Без названия"), page_url

    # run_in_executor запускает extract() в отдельном потоке и ждёт результат
    return await loop.run_in_executor(None, extract)


async def spotify_lookup(kind, spotify_id):
    """Узнаёт в Spotify названия песен. Возвращает (список строк "Исполнитель - Название", название трека/альбома)."""
    loop = asyncio.get_running_loop()

    def fetch():
        # spotipy работает медленно и "синхронно", поэтому тоже запускаем его в отдельном потоке.
        if kind == "track":
            track = spotify.track(spotify_id)            # данные одного трека
            artist = track["artists"][0]["name"]        # первый исполнитель
            return [f"{artist} - {track['name']}"], track["name"]

        album = spotify.album(spotify_id)                # данные альбома вместе со списком треков
        tracks = album["tracks"]["items"][:MAX_ALBUM_TRACKS]  # берём не больше MAX_ALBUM_TRACKS треков
        queries = [f"{t['artists'][0]['name']} - {t['name']}" for t in tracks]  # строка для поиска по каждому
        return queries, album["name"]

    return await loop.run_in_executor(None, fetch)


SPOTIFY_EMBED_DATA = re.compile(r'<script id="__NEXT_DATA__" type="application/json">(.+?)</script>', re.S)


async def spotify_embed_lookup(kind, spotify_id):
    """То же, что spotify_lookup, но без ключей API — через публичную страницу плеера Spotify (embed).
    Так работают и плейлисты: Spotify API чужие плейлисты больше не отдаёт, а плеер на сайтах — отдаёт.
    Возвращает (список строк "Исполнитель - Название", название) или выбрасывает ошибку."""
    timeout = aiohttp.ClientTimeout(total=15)
    headers = {"User-Agent": "Mozilla/5.0"}  # без этого Spotify может отдать пустую страницу
    async with aiohttp.ClientSession(timeout=timeout, headers=headers) as session:
        async with session.get(f"https://open.spotify.com/embed/{kind}/{spotify_id}") as response:
            response.raise_for_status()
            html = await response.text()
    match = SPOTIFY_EMBED_DATA.search(html)  # данные плеера лежат на странице в виде JSON
    if not match:
        raise ValueError("на странице Spotify нет данных плеера")
    entity =json.loads(match.group(1))["props"]["pageProps"]["state"]["data"]["entity"]
    name = entity.get("name") or entity.get("title") or "Spotify"
    if kind == "track":
        artist = entity.get("subtitle") or ", ".join(a.get("name", "") for a in entity.get("artists", []))
        return [f"{artist} - {name}".strip(" -")], name
    limit = MAX_PLAYLIST_TRACKS if kind == "playlist" else MAX_ALBUM_TRACKS
    tracks = [t for t in entity.get("trackList", []) if t.get("isPlayable", True)][:limit]
    # \xa0 — "неразрывный пробел", которым Spotify разделяет исполнителей; меняем на обычный
    queries = [f"{t.get('subtitle', '')} - {t.get('title', '')}".replace("\xa0", " ").strip(" -")
               for t in tracks if t.get("title")]
    if not queries:
        raise ValueError("в плейлисте/альбоме нет доступных треков")
    return queries, name


async def add_album(interaction, queries, album_name, kind="album"):
    """Добавляет все треки альбома или плейлиста в очередь. Искать их на YouTube бот будет по одному, перед включением."""
    for query in queries:
        # "ytsearch1:" — явно говорим yt-dlp "найди на YouTube первый результат по этому тексту"
        queue_add(interaction.guild.id, f"ytsearch1:{query}", query)
    label = "📜 Плейлист" if kind == "playlist" else "💿 Альбом"
    await interaction.followup.send(f"{label} **{album_name}**: добавил в очередь треков — {len(queries)}.")
    await play_next(interaction.guild, interaction.channel)  # если ничего не играет — начнём


async def play_next(guild, text_channel):
    """Включает следующий трек из очереди, если бот в канале и сейчас ничего не играет."""
    async with get_lock(guild.id):  # пока один вызов внутри этого блока, второй ждёт снаружи
        vc = guild.voice_client     # подключение бота к голосовому каналу (или None)
        if vc is None or not vc.is_connected():  # бот не в канале — очередь не трогаем, она сохранится
            return
        if vc.is_playing() or vc.is_paused():    # что-то уже играет — ничего не делаем
            return

        while True:  # пробуем треки по очереди, пока какой-нибудь не загрузится
            item = queue_pop(guild.id)
            if item is None:  # очередь закончилась
                return
            now_playing[guild.id] = item  # запоминаем, что сейчас будет играть
            page_url, saved_title = item
            try:
                stream_url, title, _ = await search(page_url)  # получаем свежую ссылку на звук
                break                                          # получилось — выходим из цикла
            except Exception:
                now_playing.pop(guild.id, None)
                await text_channel.send(f"⚠️ Не получилось загрузить **{saved_title}**, пропускаю.")

        if not vc.is_connected():  # пока искали звук, бот успел выйти из канала
            if now_playing.get(guild.id) == item:  # трек ещё не вернули в очередь — возвращаем сами
                now_playing.pop(guild.id)
                queue_push_front(guild.id, *item)  # * раскладывает пару (ссылка, название) на два аргумента
            return

        # Создаём источник звука: FFmpeg читает поток по ссылке и превращает его в "сырой" звук (PCM).
        audio = discord.FFmpegPCMAudio(stream_url, **FFMPEG_OPTIONS)
        # PCMVolumeTransformer — "регулятор громкости" поверх звука.
        # volume — множитель: 1.0 = 100%, 0.5 = 50%, 2.0 = 200%. Поэтому делим проценты на 100.
        source = discord.PCMVolumeTransformer(audio, volume=get_volume(guild.id) / 100)

        # Включаем песню. after вызывается, когда она закончится, — из другого потока, поэтому
        # run_coroutine_threadsafe передаёт работу в основной поток бота.
        vc.play(source, after=lambda error: asyncio.run_coroutine_threadsafe(
            on_track_end(guild, text_channel), bot.loop))

    await text_channel.send(f"▶️ Играет: **{title}**")
    record_play(guild.id, title)  # засчитываем прослушивание в топ месяца


async def on_track_end(guild, text_channel):
    """Вызывается, когда трек закончился (или его пропустили). Включает следующий."""
    now_playing.pop(guild.id, None)  # текущий трек больше не играет
    await play_next(guild, text_channel)


# ============================================================
#  АВТООТВЕТЫ
# ============================================================

# Сами пары «слово = ответ» лежат в файле config/auto_replies.txt (см. начало файла).

async def auto_reply(message):
    """Если в сообщении есть слово из config/auto_replies.txt — отвечает на него."""
    text = message.content.lower()  # переводим в нижний регистр, чтобы "Привет" и "ПРИВЕТ" тоже срабатывали
    for pattern, options in auto_replies():
        if pattern.search(text):
            # отвечаем цитатой, без пинга автора; NO_PINGS — даже если в ответе написать @everyone, никто не получит пинг
            await message.reply(random.choice(options), mention_author=False, allowed_mentions=NO_PINGS)
            return  # отвечаем только один раз, даже если в сообщении несколько слов


# ============================================================
#  СИСТЕМА УРОВНЕЙ
# ============================================================

# Подключаемся к файлу базы данных levels.db (если его нет — он создастся сам).
db = sqlite3.connect(os.path.join(BASE_DIR, "levels.db"))

# Создаём таблицу, если её ещё нет. Одна строка = один участник на одном сервере.
db.execute("""
    CREATE TABLE IF NOT EXISTS levels (
        guild_id     INTEGER,             -- id сервера
        user_id      INTEGER,             -- id участника
        name         TEXT,                -- его имя (для таблицы рейтинга)
        xp           INTEGER DEFAULT 0,   -- весь набранный опыт
        last_xp_time REAL    DEFAULT 0,   -- когда последний раз получал опыт (в секундах)
        PRIMARY KEY (guild_id, user_id)   -- у пары "сервер + участник" может быть только одна строка
    )
""")
# Вторая таблица — сколько раз какой трек сыграли за месяц (для /top_chart_month).
db.execute("""
    CREATE TABLE IF NOT EXISTS plays (
        guild_id INTEGER,                    -- id сервера
        month    TEXT,                       -- месяц в виде "2026-10"
        title    TEXT,                       -- название трека
        count    INTEGER DEFAULT 0,          -- сколько раз его включали
        PRIMARY KEY (guild_id, month, title)
    )
""")

# Очередь песен. Номер id задаёт порядок: чем меньше, тем раньше трек сыграет.
db.execute("""
    CREATE TABLE IF NOT EXISTS queue (
        id       INTEGER PRIMARY KEY,   -- номер (база сама даёт следующий по порядку)
        guild_id INTEGER,               -- id сервера
        url      TEXT,                  -- ссылка на страницу трека или поисковый запрос
        title    TEXT                   -- название (чтобы показывать в /queue)
    )
""")

# Шуточная игра с размером: у каждого участника на сервере свой размер.
db.execute("""
    CREATE TABLE IF NOT EXISTS dick (
        guild_id    INTEGER,
        user_id     INTEGER,
        name        TEXT,                -- имя (для таблицы рейтинга)
        size        INTEGER DEFAULT 0,   -- размер в сантиметрах
        last_growth REAL    DEFAULT 0,   -- когда последний раз использовали /dick_growth (в секундах)
        PRIMARY KEY (guild_id, user_id)
    )
""")

# Память разговора с ИИ: последние сообщения диалога с ботом в каждом канале.
db.execute("""
    CREATE TABLE IF NOT EXISTS ai_history (
        id         INTEGER PRIMARY KEY,  -- номер по порядку
        channel_id INTEGER,              -- в каком канале был разговор
        role       TEXT,                 -- "user" (человек) или "model" (бот)
        text       TEXT,                 -- текст сообщения
        created    REAL                  -- когда (в секундах)
    )
""")

# Экономика: кошелёк каждого участника на сервере.
db.execute("""
    CREATE TABLE IF NOT EXISTS economy (
        guild_id  INTEGER,
        user_id   INTEGER,
        name      TEXT,                 -- имя (для таблицы рейтинга)
        balance   INTEGER DEFAULT 0,    -- сколько монет
        last_work REAL    DEFAULT 0,    -- когда последний раз использовал /work (в секундах)
        PRIMARY KEY (guild_id, user_id)
    )
""")

# Настройки утренних и вечерних сообщений для каждого сервера (/daily_setup).
db.execute("""
    CREATE TABLE IF NOT EXISTS daily_settings (
        guild_id     INTEGER PRIMARY KEY,
        channel_id   INTEGER,           -- в какой канал писать
        timezone     TEXT,              -- часовой пояс, например "Asia/Jerusalem"
        morning      TEXT,              -- время утреннего сообщения, например "08:00"
        evening      TEXT,              -- время вечернего сообщения, например "22:00"
        last_morning TEXT DEFAULT '',   -- дата, когда последний раз отправили утреннее
        last_evening TEXT DEFAULT ''    -- дата, когда последний раз отправили вечернее
    )
""")

# Канал для рангов, рейтингов и игр на каждом сервере (/games_channel).
db.execute("""
    CREATE TABLE IF NOT EXISTS games_channel (
        guild_id   INTEGER PRIMARY KEY,
        channel_id INTEGER
    )
""")

# Счётчик грубостей к боту подряд — для тайм-аутов (раздел [moderation] в settings.ini).
db.execute("""
    CREATE TABLE IF NOT EXISTS rude_strikes (
        guild_id  INTEGER,
        user_id   INTEGER,
        count     INTEGER,   -- сколько раз подряд человек грубил боту
        last_time REAL,      -- когда последний раз (чтобы со временем обнулять)
        PRIMARY KEY (guild_id, user_id)
    )
""")

# Канал, куда бот присылает ошибки, на каждом сервере (/log_channel).
db.execute("""
    CREATE TABLE IF NOT EXISTS log_channel (
        guild_id   INTEGER PRIMARY KEY,
        channel_id INTEGER
    )
""")

# Сколько запросов к ИИ (Google) бот отправил за день — для команды /ai_limits.
db.execute("""
    CREATE TABLE IF NOT EXISTS ai_stats (
        day      TEXT PRIMARY KEY,    -- день по тихоокеанскому времени, например "2026-11-01"
        requests INTEGER DEFAULT 0    -- сколько запросов отправлено
    )
""")

# Долгая память ИИ: факты о людях ("любит рок", "учится на радиотехника"...).
db.execute("""
    CREATE TABLE IF NOT EXISTS ai_memory (
        id       INTEGER PRIMARY KEY,
        guild_id INTEGER,                -- сервер (на разных серверах — разная память)
        user_id  INTEGER,                -- о ком факт
        fact     TEXT,                   -- сам факт
        created  REAL
    )
""")

# Города для /weather, которые добавили на сервере через /weather_cities.
db.execute("""
    CREATE TABLE IF NOT EXISTS weather_cities (
        guild_id  INTEGER,
        name      TEXT,     -- название для показа, например "📍 Берлин, Германия"
        latitude  REAL,     -- широта
        longitude REAL,     -- долгота
        PRIMARY KEY (guild_id, name)
    )
""")
db.commit()  # сохраняем изменения в файл

# Значения берутся из config/settings.ini (раздел [levels]); числа справа — на случай, если там их нет.
# sorted(...) — если в файле перепутать min и max местами, бот сам поставит их по порядку.
XP_MIN, XP_MAX = sorted((setting("levels", "xp_per_message_min", 15),   # опыт за сообщение: от...
                         setting("levels", "xp_per_message_max", 25)))  # ...и до
XP_COOLDOWN = setting("levels", "xp_cooldown_seconds", 60)  # опыт за сообщения — не чаще раза в N секунд

VOICE_XP_PER_MINUTE = setting("levels", "voice_xp_per_minute", 10)  # опыт за каждую минуту в голосовом канале
LEVEL_UP_MESSAGE = setting("levels", "level_up_message", "off").lower()  # что писать при новом уровне
if LEVEL_UP_MESSAGE not in ("off", "text", "card"):
    print(f"settings.ini: level_up_message = {LEVEL_UP_MESSAGE} — можно off, text или card, беру off")
    LEVEL_UP_MESSAGE = "off"
VOICE_MIN_PEOPLE = setting("levels", "voice_min_people", 2)  # сколько людей (не ботов) должно быть в канале, чтобы шёл опыт.
                          # 2 — кто сидит в канале один (или только с ботом), опыт не получает.


def xp_for_level(level):
    """Сколько опыта нужно, чтобы перейти с уровня level на следующий. Каждый уровень сложнее предыдущего."""
    return 5 * level ** 2 + 50 * level + 100   # 0→1: 100 XP, 1→2: 155 XP, 2→3: 220 XP, ...


def level_info(total_xp):
    """По общему опыту считает: (уровень, опыт внутри текущего уровня, сколько нужно до следующего)."""
    level = 0
    xp = total_xp
    while xp >= xp_for_level(level):  # пока опыта хватает на следующий уровень —
        xp -= xp_for_level(level)     # "тратим" его
        level += 1                    # и поднимаем уровень
    return level, xp, xp_for_level(level)


def get_xp(guild_id, user_id):
    """Достаёт из базы общий опыт участника (0, если его там ещё нет)."""
    row = db.execute(
        "SELECT xp FROM levels WHERE guild_id = ? AND user_id = ?", (guild_id, user_id)
    ).fetchone()  # fetchone() — берём одну найденную строку (или None)
    return row[0] if row else 0


def get_position(guild_id, total_xp):
    """Место в рейтинге = сколько людей на сервере набрали больше опыта, плюс один."""
    row = db.execute(
        "SELECT COUNT(*) FROM levels WHERE guild_id = ? AND xp > ?", (guild_id, total_xp)
    ).fetchone()
    return row[0] + 1


async def give_xp(member, amount, channel):
    """Добавляет участнику amount опыта. Если вырос уровень — поздравляет (если это включено в settings.ini)
    в канале для игр, а если его нет — в канале channel."""
    guild_id = member.guild.id
    total_xp = get_xp(guild_id, member.id)  # сколько опыта было
    old_level = level_info(total_xp)[0]     # уровень до начисления
    total_xp += amount
    new_level, current, need = level_info(total_xp)  # уровень после начисления

    # Записываем в базу: если строки нет — создаём, если есть — обновляем (это и есть "ON CONFLICT ... DO UPDATE").
    db.execute("""
        INSERT INTO levels (guild_id, user_id, name, xp) VALUES (?, ?, ?, ?)
        ON CONFLICT (guild_id, user_id) DO UPDATE SET name = excluded.name, xp = excluded.xp
    """, (guild_id, member.id, member.display_name, total_xp))
    db.commit()

    if new_level <= old_level or LEVEL_UP_MESSAGE == "off":  # уровень не вырос или поздравления выключены
        return

    text = f"🎉 {member.mention} получает **{new_level} уровень**!"
    games = games_channel_for(member.guild)  # если выбран канал для игр — поздравляем там
    if games:
        channel = games
    if LEVEL_UP_MESSAGE == "text":  # только текст, без карточки
        try:
            await channel.send(text)
        except Exception:
            pass                    # нет прав писать в этот канал — просто молчим
        return

    # LEVEL_UP_MESSAGE == "card" — текст и карточка уровня
    try:
        avatar = await get_avatar(member)
        position = get_position(guild_id, total_xp)
        loop = asyncio.get_running_loop()
        # Рисование картинки — долгая работа, поэтому выполняем её в отдельном потоке.
        image = await loop.run_in_executor(
            None, make_rank_card, member.display_name, avatar,
            new_level, current, need, total_xp, position, pick_background("rank"),
        )
        await channel.send(text, file=discord.File(image, filename="rank.png"))
    except Exception:
        try:
            await channel.send(text)  # если картинку отправить не вышло — хотя бы текст
        except Exception:
            pass                      # нет прав писать в этот канал — просто молчим


async def add_xp(message):
    """Начисляет опыт за сообщение (не чаще раза в минуту)."""
    now = time.time()  # текущее время в секундах
    row = db.execute(
        "SELECT last_xp_time FROM levels WHERE guild_id = ? AND user_id = ?",
        (message.guild.id, message.author.id),
    ).fetchone()
    last_time = row[0] if row else 0  # когда участник последний раз получал опыт за сообщение

    if now - last_time < XP_COOLDOWN:  # с прошлого начисления прошло меньше минуты — опыт не даём
        return

    await give_xp(message.author, random.randint(XP_MIN, XP_MAX), message.channel)  # от 15 до 25 опыта

    # Запоминаем время, чтобы следующую минуту опыт за сообщения не начислялся
    db.execute(
        "UPDATE levels SET last_xp_time = ? WHERE guild_id = ? AND user_id = ?",
        (now, message.guild.id, message.author.id),
    )
    db.commit()


@tasks.loop(minutes=1)  # эта функция будет автоматически запускаться каждую минуту
async def voice_xp_loop():
    """Раз в минуту даёт опыт всем, кто сидит в голосовых каналах."""
    for guild in bot.guilds:                   # перебираем все серверы, где есть бот
        for channel in guild.voice_channels:   # и все голосовые каналы на сервере
            if channel == guild.afk_channel:   # в AFK-канале опыт не даём
                continue

            # Люди в канале (боты, в том числе наш музыкальный, не считаются)
            people = [member for member in channel.members if not member.bot]
            if len(people) < VOICE_MIN_PEOPLE:  # человек сидит один — опыт не идёт никому
                continue

            # Опыт получают только те, у кого не выключен звук (иначе они ничего не слышат)
            listeners = [
                member for member in people
                if member.voice and not member.voice.self_deaf and not member.voice.deaf
            ]

            for member in listeners:
                try:
                    # Поздравление с уровнем отправится во встроенный чат голосового канала
                    await give_xp(member, VOICE_XP_PER_MINUTE, channel)
                except Exception as error:
                    # Ошибка с одним участником не должна останавливать начисление остальным
                    await report_error("Опыт за голосовой канал", error, member.guild, member, channel)


@voice_xp_loop.before_loop
async def before_voice_xp_loop():
    """Перед первым запуском ждём, пока бот полностью подключится к Discord."""
    await bot.wait_until_ready()


async def get_avatar(user):
    """Скачивает аватарку участника. Возвращает байты картинки или None, если не получилось."""
    try:
        # replace(size=256, static_format="png") — просим аватарку 256×256 в формате PNG
        return await user.display_avatar.replace(size=256, static_format="png").read()
    except Exception:
        return None


async def get_avatar_by_id(guild, user_id):
    """Аватарка по id участника (для таблиц рейтинга)."""
    user = guild.get_member(user_id)  # пробуем найти участника в памяти бота
    if user is None:
        try:
            user = await bot.fetch_user(user_id)  # не нашли — спрашиваем у Discord
        except Exception:
            return None
    return await get_avatar(user)


# ============================================================
#  ТОП ТРЕКОВ МЕСЯЦА
# ============================================================

MONTH_NAMES = ["Январь", "Февраль", "Март", "Апрель", "Май", "Июнь",
               "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь"]


def current_month():
    """Текущий месяц строкой, например "2026-10"."""
    return datetime.date.today().strftime("%Y-%m")  # %Y — год, %m — номер месяца


def clear_old_months():
    """Удаляет статистику прошлых месяцев — так топ сам обнуляется с 1-го числа."""
    db.execute("DELETE FROM plays WHERE month != ?", (current_month(),))  # != значит "не равно"
    db.commit()


def record_play(guild_id, title):
    """Засчитывает одно прослушивание трека в этом месяце."""
    clear_old_months()
    # Если трека в этом месяце ещё не было — добавляем со счётчиком 1, иначе увеличиваем счётчик на 1.
    db.execute("""
        INSERT INTO plays (guild_id, month, title, count) VALUES (?, ?, ?, 1)
        ON CONFLICT (guild_id, month, title) DO UPDATE SET count = count + 1
    """, (guild_id, current_month(), title))
    db.commit()


def plural(n, one, few, many):
    """Правильное окончание слова после числа: 1 раз, 2 раза, 5 раз."""
    if n % 10 == 1 and n % 100 != 11:                    # 1, 21, 31... (но не 11)
        return one
    if 2 <= n % 10 <= 4 and not 12 <= n % 100 <= 14:     # 2–4, 22–24... (но не 12–14)
        return few
    return many                                          # всё остальное: 5–20, 25–30...


# ============================================================
#  КАРТИНКИ: карточка уровня и таблица рейтинга (рисуем с помощью Pillow)
# ============================================================

# Цвета в формате (красный, зелёный, синий), каждое число от 0 до 255.
BG_LEFT = (31, 27, 75)       # тёмно-фиолетовый — левый край фона
BG_RIGHT = (12, 18, 38)      # почти чёрно-синий — правый край фона
ACCENT_1 = (139, 92, 246)    # фиолетовый акцент
ACCENT_2 = (236, 72, 153)    # розовый акцент
WHITE = (255, 255, 255)
TEXT_DIM = (165, 170, 220)   # приглушённый текст (подписи)
GOLD = (251, 191, 36)        # 1 место
SILVER = (203, 213, 225)     # 2 место
BRONZE = (217, 119, 6)       # 3 место

# Фоновые картинки. Их кладут рядом с bot.py (подойдёт .jpg, .jpeg, .png или .webp).
# Если файла нет — будет обычный градиентный фон.
# Для какой картинки → имя файла (без расширения)
BACKGROUNDS = {
    "rank": "rank_bg",                # карточка /rank
    "leaderboard": "leaderboard_bg",  # таблица /leaderboard
    "chart": "chart_bg",              # топ треков /top_chart_month
    "dick": "dick_bg",                # рейтинг /dick_leaderboard
    "money": "money_bg",              # рейтинг богатства /money_leaderboard
}

# Насколько затемнять фото, чтобы на нём читался текст: 0 — совсем не затемнять, 255 — сделать чёрным.
RANK_DARKNESS = 140         # у карточки уровня
LEADERBOARD_DARKNESS = 100  # у таблицы (там строки и так на тёмных плашках, поэтому можно светлее)

# Шрифты с поддержкой русских букв. Берём первый, который найдётся на компьютере.
# Можно положить рядом с bot.py свои файлы font.ttf и font-bold.ttf — тогда возьмутся они.
FONT_PATHS = {
    True: [   # жирные
        os.path.join(BASE_DIR, "font-bold.ttf"),
        "C:/Windows/Fonts/segoeuib.ttf",                              # Windows
        "C:/Windows/Fonts/arialbd.ttf",                               # Windows
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",       # Linux
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    ],
    False: [  # обычные
        os.path.join(BASE_DIR, "font.ttf"),
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ],
}


def load_font(size, bold=False):
    """Загружает шрифт нужного размера."""
    for path in FONT_PATHS[bold]:
        try:
            return ImageFont.truetype(path, size)  # пробуем открыть файл шрифта
        except OSError:
            continue                               # нет такого файла — пробуем следующий
    return ImageFont.load_default(size=size)       # запасной встроенный шрифт


def gradient(size, left_color, right_color):
    """Плавный переход цвета слева направо. Возвращает картинку размера size = (ширина, высота)."""
    mask = Image.linear_gradient("L").rotate(90).resize(size)  # маска: слева чёрная, справа белая
    left = Image.new("RGBA", size, left_color + (255,))        # сплошной левый цвет
    right = Image.new("RGBA", size, right_color + (255,))      # сплошной правый цвет
    return Image.composite(right, left, mask)                  # смешиваем их по маске


def rounded_mask(size, radius):
    """Маска прямоугольника со скруглёнными углами (белое — видно, чёрное — прозрачно)."""
    scale = 4  # рисуем в 4 раза крупнее и уменьшаем — так края получаются гладкими
    big = Image.new("L", (size[0] * scale, size[1] * scale), 0)
    ImageDraw.Draw(big).rounded_rectangle(
        (0, 0, size[0] * scale - 1, size[1] * scale - 1), radius=radius * scale, fill=255
    )
    return big.resize(size, Image.LANCZOS)


def circle_mask(diameter):
    """Маска круга с гладкими краями."""
    return rounded_mask((diameter, diameter), diameter // 2)


def paste_rounded(base, image, position, radius):
    """Накладывает image на base в точку position, скругляя углы."""
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))     # пустой прозрачный слой размером с base
    mask = rounded_mask(image.size, radius)
    layer.paste(image, position, mask)                      # кладём картинку на слой через маску
    return Image.alpha_composite(base, layer)               # смешиваем слой с основой


def draw_layer(base, draw_function):
    """Рисует полупрозрачные фигуры на отдельном слое и накладывает его на base."""
    layer = Image.new("RGBA", base.size, (0, 0, 0, 0))
    draw_function(ImageDraw.Draw(layer))  # вызываем переданную функцию рисования
    return Image.alpha_composite(base, layer)


def pick_background(kind):
    """Путь к файлу фона для картинки kind ("rank", "leaderboard"...) или None, если файла нет."""
    for extension in (".jpg", ".jpeg", ".png", ".webp"):
        path = os.path.join(BASE_DIR, BACKGROUNDS[kind] + extension)
        if os.path.exists(path):  # такой файл есть
            return path
    return None


def make_background(size, radius, photo_path=None, focus=(0.5, 0.5), darkness=140, top_shade=0):
    """Фон карточки: фото (если передан путь к нему) или градиент со свечением.
    focus — какую часть фото оставить при обрезке: (0.5, 0.5) — центр, (0.5, 0.3) — ближе к верху.
    darkness — насколько затемнить фото (от 0 до 255).
    top_shade — высота тёмной подложки сверху (для заголовка), 0 — без неё.
    Возвращает (картинка, есть_ли_фото)."""
    width, height = size
    background = None

    if photo_path:
        try:
            photo = Image.open(photo_path).convert("RGBA")
            # ImageOps.fit обрезает фото под нужные пропорции и подгоняет размер (лишнее отрезается по краям)
            background = ImageOps.fit(photo, size, Image.LANCZOS, centering=focus)
            # Затемняем фото полупрозрачным тёмным слоем, чтобы на нём читался белый текст
            shade = Image.new("RGBA", size, (8, 10, 24, darkness))
            background = Image.alpha_composite(background, shade)

            if top_shade:
                # Подложка: сверху плотная, книзу плавно исчезает.
                # linear_gradient — маска от чёрного (сверху) к белому (снизу); переворачиваем её вверх ногами.
                fade = Image.linear_gradient("L").transpose(Image.FLIP_TOP_BOTTOM).resize((width, top_shade))
                fade = fade.point(lambda value: value * 190 // 255)  # самое тёмное место — 190 из 255
                top = Image.new("RGBA", (width, top_shade), (8, 10, 24, 255))
                top.putalpha(fade)                   # прозрачность подложки берём из маски
                background.alpha_composite(top, (0, 0))  # накладываем на верх фона
        except Exception as error:
            print("Не получилось открыть фон", photo_path, error)
            background = None  # что-то не так с файлом — используем обычный фон

    has_photo = background is not None
    if not has_photo:
        background = gradient(size, BG_LEFT, BG_RIGHT)
        glow = Image.new("RGBA", size, (0, 0, 0, 0))
        glow_draw = ImageDraw.Draw(glow)
        glow_draw.ellipse((width - 320, -200, width + 120, 200), fill=ACCENT_1 + (110,))   # фиолетовое пятно справа сверху
        glow_draw.ellipse((-150, height - 150, 300, height + 250), fill=ACCENT_2 + (80,))  # розовое пятно слева снизу
        glow = glow.filter(ImageFilter.GaussianBlur(70))  # сильно размываем — получается мягкое свечение
        background = Image.alpha_composite(background, glow)

    # Тонкая светлая рамка по краю
    background = draw_layer(background, lambda d: d.rounded_rectangle(
        (1, 1, width - 2, height - 2), radius=radius, outline=(255, 255, 255, 30), width=2))

    result = Image.new("RGBA", size, (0, 0, 0, 0))
    result.paste(background, (0, 0), rounded_mask(size, radius))  # скругляем углы всей карточки
    return result, has_photo


def make_avatar(avatar_bytes, diameter, name):
    """Делает круглую аватарку. Если аватарки нет — рисует круг с первой буквой имени."""
    if avatar_bytes:
        try:
            avatar = Image.open(io.BytesIO(avatar_bytes)).convert("RGBA")  # открываем картинку из байтов
            avatar = ImageOps.fit(avatar, (diameter, diameter), Image.LANCZOS)  # обрезаем до квадрата
        except Exception:
            avatar_bytes = None
    if not avatar_bytes:
        avatar = gradient((diameter, diameter), ACCENT_1, ACCENT_2)  # запасной вариант — цветной круг
        letter = (name[:1] or "?").upper()
        ImageDraw.Draw(avatar).text(
            (diameter / 2, diameter / 2), letter, font=load_font(diameter // 2, True), fill=WHITE, anchor="mm"
        )
    result = Image.new("RGBA", (diameter, diameter), (0, 0, 0, 0))
    result.paste(avatar, (0, 0), circle_mask(diameter))  # обрезаем по кругу
    return result


def fit_text(draw, text, font, max_width):
    """Укорачивает текст с "…" на конце, если он не помещается в max_width пикселей."""
    if draw.textlength(text, font=font) <= max_width:
        return text
    while text and draw.textlength(text + "…", font=font) > max_width:
        text = text[:-1]  # отрезаем по одной букве с конца
    return text + "…"


def progress_bar(base, box, ratio):
    """Рисует полосу прогресса в прямоугольнике box = (x1, y1, x2, y2). ratio — заполненность от 0 до 1."""
    x1, y1, x2, y2 = box
    height = y2 - y1
    # Подложка полосы — полупрозрачная белая
    base = draw_layer(base, lambda d: d.rounded_rectangle(box, radius=height // 2, fill=(255, 255, 255, 35)))
    filled = int((x2 - x1) * max(0.0, min(1.0, ratio)))  # ширина заполненной части
    if filled > 0:
        filled = max(filled, height)  # чтобы даже маленький прогресс выглядел как аккуратный кружок
        bar = gradient((filled, height), ACCENT_1, ACCENT_2)
        base = paste_rounded(base, bar, (x1, y1), height // 2)
    return base


def number(value):
    """Красиво записывает число с пробелами: 15430 → "15 430"."""
    return f"{value:,}".replace(",", " ")


def to_png(image):
    """Сохраняет картинку в "файл в памяти", который можно сразу отправить в Discord."""
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    buffer.seek(0)  # "перематываем" файл в начало, чтобы Discord прочитал его целиком
    return buffer


def make_rank_card(name, avatar_bytes, level, current_xp, need_xp, total_xp, position, background_path=None):
    """Рисует карточку уровня (баннер) 1000×300 и возвращает её как PNG.
    background_path — путь к фото для фона (None — градиент)."""
    width, height = 1000, 300
    # Фон — выбранное фото (или градиент). focus (0.5, 0.45) — обрезаем чуть выше центра, где обычно лицо.
    card, has_photo = make_background((width, height), 36, background_path, focus=(0.5, 0.45), darkness=RANK_DARKNESS)

    # --- Аватарка с цветным кольцом ---
    ring = gradient((216, 216), ACCENT_1, ACCENT_2)                      # кольцо — градиентный круг
    card = paste_rounded(card, ring, (42, 42), 108)
    card = paste_rounded(card, Image.new("RGBA", (204, 204), BG_LEFT + (255,)), (48, 48), 102)  # тёмный зазор
    avatar = make_avatar(avatar_bytes, 192, name)
    card.alpha_composite(avatar, (54, 54))                               # сама аватарка

    draw = ImageDraw.Draw(card)
    left, right = 290, 950  # левая и правая граница текстовой части

    # --- Справа сверху: РАНГ #N  УРОВЕНЬ N ---
    big = load_font(54, True)
    small = load_font(22, True)
    top_line = 98  # линия, на которой "стоят" буквы верхней строки

    level_text = str(level)
    draw.text((right, top_line), level_text, font=big, fill=ACCENT_2, anchor="rs")  # anchor="rs" — выравнивание по правому краю
    x = right - draw.textlength(level_text, font=big) - 12
    draw.text((x, top_line), "УРОВЕНЬ", font=small, fill=TEXT_DIM, anchor="rs")
    x -= draw.textlength("УРОВЕНЬ", font=small) + 36

    rank_text = f"#{position}" if position else "—"
    draw.text((x, top_line), rank_text, font=big, fill=WHITE, anchor="rs")
    x -= draw.textlength(rank_text, font=big) + 12
    draw.text((x, top_line), "РАНГ", font=small, fill=TEXT_DIM, anchor="rs")

    # --- Строка над полосой: справа прогресс уровня, слева имя ---
    middle_line = 166
    info_font = load_font(24)
    info_bold = load_font(26, True)
    need_text = f" / {number(need_xp)} XP"
    draw.text((right, middle_line), need_text, font=info_font, fill=TEXT_DIM, anchor="rs")
    x = right - draw.textlength(need_text, font=info_font)
    draw.text((x, middle_line), number(current_xp), font=info_bold, fill=WHITE, anchor="rs")
    x -= draw.textlength(number(current_xp), font=info_bold) + 30  # левая граница блока с опытом

    name_font = load_font(44, True)
    draw.text((left, middle_line), fit_text(draw, name, name_font, x - left),  # укорачиваем, если не влезает
              font=name_font, fill=WHITE, anchor="ls")

    # --- Полоса прогресса до следующего уровня ---
    card = progress_bar(card, (left, 184, right, 220), current_xp / need_xp)

    # --- Подписи под полосой ---
    draw = ImageDraw.Draw(card)
    hint_font = load_font(21)
    draw.text((left, 256), f"Всего опыта: {number(total_xp)} XP", font=hint_font, fill=TEXT_DIM, anchor="ls")
    draw.text((right, 256), f"До {level + 1} уровня: {number(need_xp - current_xp)} XP",
              font=hint_font, fill=TEXT_DIM, anchor="rs")
    return to_png(card)


def make_music_icon(diameter):
    """Круглая иконка с треугольником "play" — вместо аватарки в топе треков."""
    scale = 4  # рисуем крупнее и уменьшаем, чтобы края были гладкими
    size = diameter * scale
    icon = gradient((size, size), ACCENT_1, ACCENT_2)
    # Треугольник: три точки (x, y). Чуть сдвинут вправо — так он визуально выглядит по центру.
    ImageDraw.Draw(icon).polygon(
        [(size * 0.40, size * 0.30), (size * 0.40, size * 0.70), (size * 0.72, size * 0.50)], fill=WHITE
    )
    icon = icon.resize((diameter, diameter), Image.LANCZOS)
    result = Image.new("RGBA", (diameter, diameter), (0, 0, 0, 0))
    result.paste(icon, (0, 0), circle_mask(diameter))  # обрезаем по кругу
    return result


def make_table_image(title, subtitle, right_title, right_hint, rows, background_path):
    """Общая функция для всех картинок-рейтингов (уровни, топ треков, игра).
    rows — список словарей, по одному на строку:
        name     — главный текст строки (имя или название трека)
        avatar   — байты аватарки, None (тогда кружок с буквой) или "music" (иконка "play")
        progress — заполненность полоски под именем, от 0 до 1
        pill     — текст на цветной плашке справа (например "УР. 12")
        hint     — маленькая подпись под плашкой (например "15 430 XP")
    right_hint — подпись под right_title (можно пустую строку "" — тогда её не будет).
    background_path — путь к фото для фона (None — градиент)."""
    width = 1000
    header, row_height, gap = 170, 84, 12  # высота шапки, высота строки, промежуток между строками
    height = header + len(rows) * (row_height + gap) + 28
    # Фон: фото (если есть) или градиент. top_shade — тёмная подложка под заголовок.
    board, has_photo = make_background((width, height), 36, background_path, focus=(0.5, 0.25),
                                       darkness=LEADERBOARD_DARKNESS, top_shade=header + 30)
    draw = ImageDraw.Draw(board)

    # --- Шапка ---
    title_font = load_font(50, True)
    subtitle_font = load_font(26)
    draw.text((50, 92), title, font=title_font, fill=WHITE, anchor="ls")
    draw.text((52, 132), fit_text(draw, subtitle, subtitle_font, 640), font=subtitle_font, fill=TEXT_DIM, anchor="ls")
    draw.text((width - 50, 92), right_title, font=load_font(34, True), fill=ACCENT_2, anchor="rs")
    if right_hint:  # подпись справа рисуем, только если она есть
        draw.text((width - 50, 132), right_hint, font=load_font(22), fill=TEXT_DIM, anchor="rs")
    board = paste_rounded(board, gradient((width - 100, 4), ACCENT_1, ACCENT_2), (50, 150), 2)  # линия под шапкой

    medal_colors = {1: GOLD, 2: SILVER, 3: BRONZE}  # цвета первых трёх мест
    name_font = load_font(28, True)
    number_font = load_font(26, True)
    pill_font = load_font(22, True)
    hint_font = load_font(20)

    for index, row in enumerate(rows):
        place = index + 1
        top = header + 12 + index * (row_height + gap)  # верхний край строки
        medal = medal_colors.get(place)

        # Фон строки. На фото — тёмное "стекло" (иначе текст теряется на ярких местах картинки),
        # на обычном фоне — полупрозрачная светлая плашка. У первых трёх мест плашка чуть заметнее.
        if has_photo:
            row_color = (10, 12, 28, 165 if medal else 140)
        else:
            row_color = (255, 255, 255, 24 if medal else 13)
        board = paste_rounded(board, Image.new("RGBA", (width - 80, row_height), row_color), (40, top), 20)
        if medal:  # у первых трёх мест — цветная полоска слева
            board = paste_rounded(board, Image.new("RGBA", (6, row_height - 28), medal + (255,)), (46, top + 14), 3)

        # Кружок с номером места
        badge = Image.new("RGBA", (52, 52), medal + (255,) if medal else (255, 255, 255, 30))
        board = paste_rounded(board, badge, (60, top + 16), 26)
        draw = ImageDraw.Draw(board)
        draw.text((86, top + 42), str(place), font=number_font,
                  fill=(20, 20, 35) if medal else WHITE, anchor="mm")  # anchor="mm" — по центру

        # Аватарка или иконка
        if row["avatar"] == "music":
            picture = make_music_icon(60)
        else:
            picture = make_avatar(row["avatar"], 60, row["name"])
        board.alpha_composite(picture, (128, top + 12))

        # Справа: цветная плашка и подпись под ней
        draw = ImageDraw.Draw(board)
        pill_width = int(draw.textlength(row["pill"], font=pill_font)) + 32
        pill_left = width - 60 - pill_width
        board = paste_rounded(board, gradient((pill_width, 36), ACCENT_1, ACCENT_2), (pill_left, top + 10), 18)
        draw = ImageDraw.Draw(board)
        draw.text((pill_left + pill_width / 2, top + 28), row["pill"], font=pill_font, fill=WHITE, anchor="mm")
        draw.text((width - 60, top + 72), row["hint"], font=hint_font, fill=TEXT_DIM, anchor="rs")

        # Имя и полоска под ним (тянутся до плашки справа)
        right_edge = min(pill_left - 30, 770)
        draw.text((210, top + 42), fit_text(draw, row["name"], name_font, right_edge - 210),
                  font=name_font, fill=WHITE, anchor="ls")
        board = progress_bar(board, (210, top + 54, right_edge, top + 62), row["progress"])

    return to_png(board)


def make_leaderboard(server_name, players, background_path=None):
    """Таблица рейтинга по уровням. players — список словарей с name, avatar, level, xp, progress."""
    rows = [{
        "name": player["name"],
        "avatar": player["avatar"],
        "progress": player["progress"],           # прогресс до следующего уровня
        "pill": f"УР. {player['level']}",
        "hint": f"{number(player['xp'])} XP",
    } for player in players]
    return make_table_image("РЕЙТИНГ СЕРВЕРА", server_name, f"ТОП {len(rows)}", "по опыту в чате",
                            rows, background_path)


def make_top_chart(server_name, month_title, tracks, background_path=None):
    """Топ треков месяца. tracks — список пар (название, сколько раз включали)."""
    most_plays = tracks[0][1]  # больше всего прослушиваний у первого — от него считаем длину полосок
    rows = [{
        "name": title,
        "avatar": "music",
        "progress": plays / most_plays,
        "pill": f"{plays} {plural(plays, 'раз', 'раза', 'раз')}",
        "hint": "прослушано",
    } for title, plays in tracks]
    return make_table_image("ТОП ТРЕКОВ МЕСЯЦА", f"{month_title} · {server_name}", f"ТОП {len(rows)}",
                            "по прослушиваниям", rows, background_path)


def make_dick_leaderboard(server_name, players, background_path=None):
    """Рейтинг шуточной игры. players — список словарей с name, avatar, size."""
    biggest = max(player["size"] for player in players) or 1  # "or 1" — чтобы не делить на ноль
    rows = [{
        "name": player["name"],
        "avatar": player["avatar"],
        "progress": player["size"] / biggest,
        "pill": f"{player['size']} см",
        "hint": "размер",
    } for player in players]
    return make_table_image("РЕЙТИНГ ЧЛЕНОВ", server_name, f"ТОП {len(rows)}", "",
                            rows, background_path)


def make_money_leaderboard(server_name, players, background_path=None):
    """Рейтинг богатства. players — список словарей с name, avatar, balance."""
    richest = max(player["balance"] for player in players) or 1  # "or 1" — чтобы не делить на ноль
    rows = [{
        "name": player["name"],
        "avatar": player["avatar"],
        "progress": max(0, player["balance"]) / richest,
        "pill": number(player["balance"]),
        "hint": plural(player["balance"], "монета", "монеты", "монет"),
    } for player in players]
    return make_table_image("РЕЙТИНГ БОГАТСТВА", server_name, f"ТОП {len(rows)}", "по монетам",
                            rows, background_path)


# ============================================================
#  ПОГОДА (сервис Open-Meteo — бесплатный, ключ не нужен)
# ============================================================

# Города по умолчанию: (название, широта, долгота). Показываются, пока на сервере не добавили свои
# через /weather_cities (тогда эти два города тоже попадают в список и их можно удалить).
DEFAULT_WEATHER_CITIES = [
    ("🇷🇺 Москва", 55.7558, 37.6173),
    ("🇮🇱 Израиль, Тель-Авив", 32.0853, 34.7818),
]
MAX_WEATHER_CITIES = 8  # больше городов в одном сообщении будет неудобно читать

# Коды погоды (международный стандарт WMO) → (описание, значок)
WEATHER_CODES = {
    0: ("ясно", "☀️"),
    1: ("преимущественно ясно", "🌤️"),
    2: ("переменная облачность", "⛅"),
    3: ("пасмурно", "☁️"),
    45: ("туман", "🌫️"), 48: ("туман с изморозью", "🌫️"),
    51: ("слабая морось", "🌦️"), 53: ("морось", "🌦️"), 55: ("сильная морось", "🌧️"),
    56: ("ледяная морось", "🌧️"), 57: ("сильная ледяная морось", "🌧️"),
    61: ("небольшой дождь", "🌦️"), 63: ("дождь", "🌧️"), 65: ("сильный дождь", "🌧️"),
    66: ("ледяной дождь", "🌧️"), 67: ("сильный ледяной дождь", "🌧️"),
    71: ("небольшой снег", "🌨️"), 73: ("снег", "🌨️"), 75: ("сильный снег", "❄️"),
    77: ("снежная крупа", "🌨️"),
    80: ("небольшой ливень", "🌦️"), 81: ("ливень", "🌧️"), 82: ("сильный ливень", "⛈️"),
    85: ("снегопад", "🌨️"), 86: ("сильный снегопад", "❄️"),
    95: ("гроза", "⛈️"), 96: ("гроза с градом", "⛈️"), 99: ("гроза с сильным градом", "⛈️"),
}

WEEKDAYS = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"]  # дни недели: weekday() даёт 0 для понедельника


async def get_json(url, params):
    """Делает запрос к сайту и возвращает ответ в виде словаря Python."""
    timeout = aiohttp.ClientTimeout(total=10)  # ждём ответа не больше 10 секунд
    async with aiohttp.ClientSession(timeout=timeout) as session:
        async with session.get(url, params=params) as response:
            response.raise_for_status()   # если сайт ответил ошибкой — сразу выдаём исключение
            return await response.json()  # превращаем ответ сайта (JSON) в словарь


async def find_city(name):
    """Ищет город по названию. Возвращает (название для показа, широта, долгота) или None."""
    data = await get_json(
        "https://geocoding-api.open-meteo.com/v1/search",
        {"name": name, "count": 1, "language": "ru", "format": "json"},  # language="ru" — названия по-русски
    )
    results = data.get("results") or []  # если ничего не нашлось, списка results может не быть
    if not results:
        return None
    place = results[0]  # берём самый подходящий вариант
    title = f"📍 {place['name']}, {place['country']}" if place.get("country") else f"📍 {place['name']}"
    return title, place["latitude"], place["longitude"]


def get_weather_cities(guild_id):
    """Список городов сервера. Если своих ещё не добавляли — города по умолчанию."""
    rows = db.execute(
        "SELECT name, latitude, longitude FROM weather_cities WHERE guild_id = ? ORDER BY rowid", (guild_id,)
    ).fetchall()  # rowid — порядок, в котором города добавляли
    return rows or DEFAULT_WEATHER_CITIES


def save_weather_cities(guild_id, cities):
    """Перезаписывает список городов сервера."""
    db.execute("DELETE FROM weather_cities WHERE guild_id = ?", (guild_id,))
    db.executemany(  # executemany — выполнить одну команду для каждого элемента списка
        "INSERT INTO weather_cities (guild_id, name, latitude, longitude) VALUES (?, ?, ?, ?)",
        [(guild_id, name, lat, lon) for name, lat, lon in cities],
    )
    db.commit()


async def fetch_weather(latitude, longitude):
    """Запрашивает у Open-Meteo погоду сейчас и прогноз на 3 дня. Возвращает словарь с данными."""
    params = {
        "latitude": latitude,
        "longitude": longitude,
        "current": "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m",
        "daily": "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max",
        "wind_speed_unit": "ms",   # ветер в метрах в секунду
        "timezone": "auto",        # время и дни — по местному времени города
        "forecast_days": 3,        # прогноз на 3 дня
    }
    return await get_json("https://api.open-meteo.com/v1/forecast", params)


def temperature(value):
    """Красиво пишет температуру: 12.4 → "+12°", -3.6 → "-4°", 0.2 → "0°"."""
    value = round(value)  # округляем до целого
    return f"{value:+d}°" if value != 0 else "0°"  # :+d — всегда показывать знак


def format_weather(data):
    """Превращает ответ Open-Meteo в несколько строк текста для Discord."""
    now = data["current"]
    description, icon = WEATHER_CODES.get(now["weather_code"], ("неизвестно", "🌡️"))
    local_time = now["time"][-5:]  # из "2026-10-30T21:15" берём последние 5 символов: "21:15"

    lines = [
        f"{icon} **{temperature(now['temperature_2m'])}**, {description} · 🕐 {local_time}",
        f"Ощущается как {temperature(now['apparent_temperature'])} · "
        f"ветер {round(now['wind_speed_10m'])} м/с · влажность {now['relative_humidity_2m']}%",
        "",  # пустая строка — отступ перед прогнозом
    ]

    days = data["daily"]
    for i, day in enumerate(days["time"]):              # i — номер дня (0, 1, 2), day — дата строкой
        date = datetime.date.fromisoformat(day)          # "2026-10-31" → дата
        if i == 0:
            label = "Сегодня"
        elif i == 1:
            label = "Завтра"
        else:
            label = f"{WEEKDAYS[date.weekday()]} {date:%d.%m}"  # например "Вс 01.11"
        _, day_icon = WEATHER_CODES.get(days["weather_code"][i], ("", "🌡️"))
        low = temperature(days["temperature_2m_min"][i])
        high = temperature(days["temperature_2m_max"][i])
        rain = days["precipitation_probability_max"][i]  # вероятность осадков в % (может не быть)
        rain_text = f" · 💧 {rain}%" if rain else ""
        lines.append(f"{day_icon} {label}: {low}…{high}{rain_text}")

    return "\n".join(lines)  # склеиваем строки через перенос строки


# ============================================================
#  РАЗВЛЕКАТЕЛЬНЫЕ КОМАНДЫ: кости, предсказания, погоняло, шуточная игра с размером
# ============================================================

DICE_FACES = ["⚀", "⚁", "⚂", "⚃", "⚄", "⚅"]  # грани кубика от 1 до 6

# --- Шуточная игра /dick_growth ---
# Значения из config/settings.ini, раздел [dick_game]
GROWTH_MIN, GROWTH_MAX = sorted((setting("dick_game", "growth_min", -10),   # самое большое уменьшение за раз (см)
                                 setting("dick_game", "growth_max", 10)))   # самое большое увеличение за раз (см)
GROWTH_COOLDOWN = setting("dick_game", "growth_cooldown_hours", 24) * 60 * 60  # как часто можно играть (в секундах)


def get_dick(guild_id, user_id):
    """Возвращает (размер, время последнего роста) участника. Новичок начинает с 0 см."""
    row = db.execute(
        "SELECT size, last_growth FROM dick WHERE guild_id = ? AND user_id = ?", (guild_id, user_id)
    ).fetchone()
    return row if row else (0, 0)


def get_dick_position(guild_id, size):
    """Место в рейтинге игры = сколько участников с размером больше, плюс один."""
    return db.execute(
        "SELECT COUNT(*) FROM dick WHERE guild_id = ? AND size > ?", (guild_id, size)
    ).fetchone()[0] + 1


def time_left_text(seconds):
    """Превращает секунды в текст вроде "5 ч 12 мин"."""
    hours = int(seconds // 3600)          # // — деление без остатка
    minutes = int(seconds % 3600 // 60)   # % — остаток от деления
    if hours:
        return f"{hours} ч {minutes} мин"
    return f"{max(minutes, 1)} мин"       # меньше минуты всё равно пишем "1 мин"

# Виды предсказаний для /divination: ключ → (заголовок, список предсказаний)
PREDICTIONS = {
    "day": ("🔮 Предсказание на день", [
        "Сегодня ты найдёшь то, что давно искал. Скорее всего, это второй носок.",
        "Звёзды советуют не спорить с микроволновкой. Она знает больше, чем говорит.",
        "День будет продуктивным. По крайней мере, первые пятнадцать минут.",
        "Удача сегодня на твоей стороне, но только до первого дедлайна.",
        "Тебя ждёт неожиданная встреча. Возможно, с отражением в выключенном экране.",
        "Вселенная шепчет: «ляг поспи». Не спорь со Вселенной.",
        "Идеальный день, чтобы начать новую жизнь. Или хотя бы доесть вчерашнюю пиццу.",
        "Кто-то думает о тебе прямо сейчас. Например, этот бот.",
        "Сегодня ты скажешь что-то очень умное. Главное — вовремя замолчать после этого.",
        "Карты говорят: всё будет хорошо. Правда, карты в этом не очень разбираются.",
    ]),
    "love": ("💘 Любовное предсказание", [
        "Твоя вторая половинка уже рядом. Проверь холодильник.",
        "Скоро ты встретишь человека, который полюбит тебя так же сильно, как ты любишь спать.",
        "Звёзды видят в твоём будущем свечи, романтику и… отключение электричества.",
        "Кто-то тайно перечитывает твои сообщения. Пиши грамотно.",
        "Сердце подскажет верный путь. Но лучше всё-таки свериться с навигатором.",
        "В этом месяце тебя ждёт бурный роман. С сериалом на восемь сезонов.",
        "Тот самый человек напишет тебе первым. Примерно через 3–5 рабочих дней.",
        "Любовь найдёт тебя сама. Главное — иногда выходить из голосового канала.",
    ]),
    "study": ("📚 Предсказание по учёбе", [
        "Сессия сдастся сама. Шучу, придётся немного поботать.",
        "Преподаватель сегодня в хорошем настроении. Используй это с умом.",
        "Твоя лаба зачтётся с первого раза. Звёзды не уверены, но очень в тебя верят.",
        "Дедлайн ближе, чем кажется. Обернись.",
        "Сегодня ты наконец поймёшь прошлую пару. А эту — нет.",
        "Знания придут к тебе ночью. Около трёх часов, за четыре часа до экзамена.",
        "На зачёте попадётся тот самый билет, который ты выучил. Единственный.",
        "Удача улыбнётся тебе на экзамене. Главное — улыбнись ей в ответ.",
    ]),
    "money": ("💰 Денежное предсказание", [
        "Скоро ты найдёшь деньги в старой куртке. Хватит ровно на одну шаурму.",
        "Финансовый поток уже в пути. Курьер немного задерживается.",
        "Звёзды советуют сегодня не открывать приложение банка. Ради твоего же спокойствия.",
        "Тебя ждёт крупная покупка: шаурма с двойным мясом.",
        "Деньги любят счёт. Начни с мелочи в кармане.",
        "Скоро тебе вернут долг. Тот, о котором ты уже забыл.",
        "Богатство уже близко. Оно просто стесняется подойти.",
        "В этом месяце ты разбогатеешь. На опыт в чате.",
    ]),
    "ball": ("🎱 Магический шар", [
        "Бесспорно ✅",
        "Даже не сомневайся ✅",
        "Скорее да, чем нет 🙂",
        "Знаки говорят — да ✨",
        "50 на 50 🪙",
        "Спроси позже, я занят 😴",
        "Лучше тебе пока не знать 🤐",
        "Сконцентрируйся и спроси снова 🔁",
        "Весьма сомнительно 🤨",
        "Мой ответ — нет ❌",
        "Даже не думай 🚫",
    ]),
}

# Части для погоняла: прилагательное + титул + "чего?" (всё в мужском роде, чтобы слова сочетались)
NICK_ADJECTIVES = ["Великий", "Сонный", "Легендарный", "Хитрый", "Голодный", "Таинственный",
                   "Ленивый", "Бешеный", "Ночной", "Бронированный", "Неуловимый", "Грозный"]
NICK_TITLES = ["повелитель", "король", "барон", "хранитель", "магистр", "пожиратель",
               "князь", "император", "рыцарь", "шаман", "лорд", "генерал"]
NICK_OBJECTS = ["пельменей", "подъезда", "дивана", "микроволновки", "голосовых", "мемов",
                "холодильника", "носков", "дедлайнов", "шаурмы", "тапок", "вайфая",
                "кринжа", "сессии", "пар", "доширака", "будильников", "сквозняков"]


def daily_nickname(user_id):
    """Погоняло на сегодня. У одного человека в течение дня оно не меняется, а завтра будет новое."""
    today = datetime.date.today().isoformat()  # сегодняшняя дата, например "2026-10-30"
    # random.Random(зерно) — генератор случайностей, который при одинаковом "зерне" выдаёт одно и то же.
    # Зерно = id человека + дата, поэтому весь день результат одинаковый.
    generator = random.Random(f"{user_id}-{today}")
    return f"{generator.choice(NICK_ADJECTIVES)} {generator.choice(NICK_TITLES)} {generator.choice(NICK_OBJECTS)}"


# ============================================================
#  ИСКУССТВЕННЫЙ ИНТЕЛЛЕКТ (Google Gemini API)
# ============================================================

# Основные настройки ИИ берутся из config/settings.ini, раздел [ai]
AI_MODEL = setting("ai", "model", "gemini-3.5-flash-lite")  # модель Gemini
AI_MAX_TOKENS = 8192           # предел длины ответа ("токен" — кусочек слова). С большим запасом: модель сначала
                               # "обдумывает" ответ, и это тоже входит в предел (на "high" обдумывание длинное).
AI_THINKING_LEVEL = setting("ai", "thinking_level", "high").lower()  # сколько ИИ "обдумывает" ответ:
                               # "minimal" — быстрее всего (есть только у Flash-Lite), "low" — быстро,
                               # "medium" — обычно, "high" — умнее, но дольше, "none" — не передавать вовсе.
if AI_THINKING_LEVEL in ("none", "нет", "off"):
    AI_THINKING_LEVEL = None
elif AI_THINKING_LEVEL not in ("minimal", "low", "medium", "high"):
    print(f"settings.ini: thinking_level = {AI_THINKING_LEVEL} — такого уровня нет, беру high")
    AI_THINKING_LEVEL = "high"
AI_FALLBACK_MODEL = setting("ai", "fallback_model", "gemini-3.8-flash")  # запасная модель, если основная не отвечает
if AI_FALLBACK_MODEL.lower() in ("none", "нет", "off") or AI_FALLBACK_MODEL == AI_MODEL:
    AI_FALLBACK_MODEL = None
AI_RETRIES = max(0, setting("ai", "retries", 2))  # сколько раз повторить запрос при ошибке 503 "перегружен"
AI_CONTEXT_MESSAGES = 10       # сколько последних сообщений чата показывать ИИ, чтобы он понимал разговор
AI_COOLDOWN = setting("ai", "cooldown_seconds", 5)         # один человек может спрашивать не чаще раза в N секунд
AI_DAILY_LIMIT = setting("ai", "daily_limit_per_user", 50)  # и не больше N вопросов в день
AI_HISTORY_LIMIT = setting("ai", "history_limit", 20)       # память разговора: сколько последних сообщений помнить
                               # (вопросы и ответы вместе; 20 = 10 пар "вопрос — ответ")
AI_MAX_FACTS = 20              # долгая память: сколько фактов о каждом человеке можно запомнить
AI_FACT_LENGTH = 200           # максимальная длина одного факта (в символах)
AI_MAX_IMAGES = 3              # сколько картинок из одного сообщения отправлять ИИ
AI_MAX_IMAGE_MB = 5            # картинки тяжелее 5 МБ не отправляем (весь запрос к Gemini — не больше 20 МБ)
AI_IMAGE_TYPES = ("image/png", "image/jpeg", "image/webp", "image/heic", "image/heif")  # форматы, которые понимает Gemini

# Примерные лимиты бесплатного тарифа Google для модели AI_MODEL (для команды /ai_limits).
# Google их не публикует, точные — на aistudio.google.com/rate-limit. Для gemini-3.5-flash-lite — около 500 и 15,
# для gemini-3.8-flash — около 20 и 5. Если сменишь модель, поменяй и эти числа в settings.ini.
AI_GOOGLE_DAILY_LIMIT = setting("ai", "google_daily_limit", 500)    # запросов в день на весь ключ
AI_GOOGLE_MINUTE_LIMIT = setting("ai", "google_minute_limit", 15)   # запросов в минуту на весь ключ

# Дневной лимит Google обнуляется в полночь по тихоокеанскому времени (Калифорния).
try:
    from zoneinfo import ZoneInfo                    # часовые пояса (входит в Python)
    PACIFIC = ZoneInfo("America/Los_Angeles")        # на Windows для этого нужен пакет tzdata
except Exception:
    PACIFIC = datetime.timezone(datetime.timedelta(hours=-8))  # запасной вариант: UTC-8 (летом ошибка на час)

# Метка, которой ИИ сообщает боту "запомни этот факт". Бот находит её в ответе, сохраняет факт и убирает метку.
REMEMBER_TAG = re.compile(r"\[ЗАПОМНИТЬ:\s*(.+?)\]", re.IGNORECASE)

# Технические правила для ИИ: оформление для Discord, формулы и память. Они всегда добавляются к характеру
# из config/prompt.txt — так правка характера не сломает формулы и команду «запомни».
AI_TECH_RULES = """Ответ показывается в Discord, поэтому:
- не используй заголовки (#) и таблицы — Discord их плохо показывает; списки и **жирный** можно;
- формулы пиши без LaTeX, обычными символами, и всегда оформляй как код в обратных кавычках:
  одну формулу в строке — `Δ = 2d·cos(θ) = m·λ`, несколько формул подряд — блоком между строками ```.
  Не пиши "$\\Delta = ...$".
Перед вопросом тебе показывают несколько последних сообщений чата — это только контекст.
Память: тебе передают ваш прошлый разговор в этом канале и факты, которые ты помнишь о собеседнике.
Используй их естественно, когда это к месту, но не пересказывай без повода.
Если собеседник ПРЯМО просит что-то запомнить о нём (например, «запомни, что я люблю рок»),
допиши в самом конце ответа отдельной строкой метку [ЗАПОМНИТЬ: короткий факт о нём в третьем лице],
например [ЗАПОМНИТЬ: любит рок]. Без прямой просьбы метку не ставь.
Грубость: если собеседник обращается к тебе с матом (в том числе замаскированным, например «х*й»),
всерьёз оскорбляет тебя или других, или пишет пошлости и непотребства — не отвечай на это сообщение
и не объясняй почему, а напиши ТОЛЬКО метку [ИГНОР]. Лёгкие шутки, «бака» и обычные вопросы о
неприятных темах грубостью не считаются. Мат в контексте чата (не в самом вопросе к тебе) тоже не повод."""

# Метка, которой ИИ сообщает "на это не отвечаю" (см. правило о грубости выше).
IGNORE_TAG = re.compile(r"\[ИГНОР\]", re.IGNORECASE)


def ai_system_prompt():
    """Полная инструкция для ИИ: характер из config/prompt.txt + технические правила."""
    return ai_character() + "\n\n" + AI_TECH_RULES

# Клиент для запросов к Gemini. Если ключа нет — None, и ИИ-функции просто сообщат, что не настроены.
ai_client = genai.Client(api_key=GEMINI_API_KEY) if GEMINI_API_KEY else None

# Сколько раз кто спрашивал: id человека → (дата, сколько вопросов за эту дату, время последнего вопроса).
# Хранится в памяти, после перезапуска бота обнуляется.
ai_usage = {}

ai_recent_requests = []  # время (в секундах) запросов к Google за последнюю минуту
ai_last_quota_error = 0  # когда Google последний раз отказал из-за лимита (0 — ни разу)


def google_day():
    """Сегодняшний день по тихоокеанскому времени, например "2026-11-01" — по нему Google считает дневной лимит."""
    return datetime.datetime.now(PACIFIC).date().isoformat()


def google_reset_time():
    """Когда обнулится дневной лимит Google (ближайшая полночь по тихоокеанскому времени), в секундах."""
    now = datetime.datetime.now(PACIFIC)
    midnight = (now + datetime.timedelta(days=1)).replace(hour=0, minute=0, second=0, microsecond=0)
    return int(midnight.timestamp())


def count_ai_request():
    """Засчитывает один запрос к Google: в дневной счётчик (в базе) и в список за последнюю минуту."""
    today = google_day()
    db.execute("DELETE FROM ai_stats WHERE day != ?", (today,))  # прошлые дни больше не нужны
    db.execute("""
        INSERT INTO ai_stats (day, requests) VALUES (?, 1)
        ON CONFLICT (day) DO UPDATE SET requests = requests + 1
    """, (today,))
    db.commit()
    ai_recent_requests.append(time.time())


def ai_requests_today():
    """Сколько запросов к Google бот отправил сегодня (по тихоокеанскому времени)."""
    row = db.execute("SELECT requests FROM ai_stats WHERE day = ?", (google_day(),)).fetchone()
    return row[0] if row else 0


def ai_requests_last_minute():
    """Сколько запросов к Google бот отправил за последние 60 секунд."""
    minute_ago = time.time() - 60
    ai_recent_requests[:] = [moment for moment in ai_recent_requests if moment > minute_ago]  # убираем старые
    return len(ai_recent_requests)

# Запрещаем ИИ-ответам кого-либо пинговать (иначе ИИ мог бы случайно написать @everyone)
NO_PINGS = discord.AllowedMentions.none()


def ai_limit_check(user_id):
    """Проверяет лимиты. Возвращает текст отказа или None, если спрашивать можно (и засчитывает вопрос)."""
    today = datetime.date.today().isoformat()
    day, count, last_time = ai_usage.get(user_id, (today, 0, 0))
    if day != today:  # наступил новый день — счётчик заново
        count = 0
    now = time.time()
    if now - last_time < AI_COOLDOWN:
        return f"Не так быстро 🙂 Подожди {int(AI_COOLDOWN - (now - last_time)) + 1} сек."
    if count >= AI_DAILY_LIMIT:
        return f"На сегодня лимит вопросов ({AI_DAILY_LIMIT}) закончился. Завтра продолжим!"
    ai_usage[user_id] = (today, count + 1, now)  # засчитываем вопрос
    return None


# --- Память разговора (таблица ai_history) ---

def history_load(channel_id):
    """Последние сообщения разговора с ботом в канале: список пар (роль, текст), от старых к новым.
    Роль "user" — сообщение человека, "model" — ответ бота (так их называет Gemini)."""
    rows = db.execute(
        "SELECT role, text FROM ai_history WHERE channel_id = ? ORDER BY id DESC LIMIT ?",
        (channel_id, AI_HISTORY_LIMIT),
    ).fetchall()
    rows.reverse()  # из базы достали от новых к старым — переворачиваем
    if rows and rows[0][0] == "model":  # разговор должен начинаться с вопроса человека
        rows = rows[1:]
    return rows


def history_save(channel_id, user_text, model_text):
    """Запоминает пару "вопрос — ответ" и удаляет из памяти канала всё, что старше AI_HISTORY_LIMIT сообщений."""
    now = time.time()
    db.execute("INSERT INTO ai_history (channel_id, role, text, created) VALUES (?, 'user', ?, ?)",
               (channel_id, user_text, now))
    db.execute("INSERT INTO ai_history (channel_id, role, text, created) VALUES (?, 'model', ?, ?)",
               (channel_id, model_text, now))
    # Оставляем только последние AI_HISTORY_LIMIT сообщений этого канала
    db.execute("""
        DELETE FROM ai_history WHERE channel_id = ? AND id NOT IN (
            SELECT id FROM ai_history WHERE channel_id = ? ORDER BY id DESC LIMIT ?
        )
    """, (channel_id, channel_id, AI_HISTORY_LIMIT))
    db.commit()


def history_clear(channel_id):
    """Стирает память разговора в канале. Возвращает, сколько сообщений удалено."""
    cursor = db.execute("DELETE FROM ai_history WHERE channel_id = ?", (channel_id,))
    db.commit()
    return cursor.rowcount


# --- Долгая память о людях (таблица ai_memory) ---

def facts_get(guild_id, user_id):
    """Все факты о человеке на этом сервере: список пар (номер в базе, текст), от старых к новым."""
    return db.execute(
        "SELECT id, fact FROM ai_memory WHERE guild_id = ? AND user_id = ? ORDER BY id",
        (guild_id, user_id),
    ).fetchall()


def fact_add(guild_id, user_id, fact):
    """Запоминает факт. Возвращает "ok", "full" (память заполнена) или "duplicate" (уже помним)."""
    fact = fact.strip()[:AI_FACT_LENGTH]
    facts = facts_get(guild_id, user_id)
    if any(old.lower() == fact.lower() for _, old in facts):  # такой факт уже есть
        return "duplicate"
    if len(facts) >= AI_MAX_FACTS:
        return "full"
    db.execute("INSERT INTO ai_memory (guild_id, user_id, fact, created) VALUES (?, ?, ?, ?)",
               (guild_id, user_id, fact, time.time()))
    db.commit()
    return "ok"


# --- Картинки ---

def image_type(attachment):
    """Формат вложения, например "image/png" (без лишнего вроде "; charset=...")."""
    return (attachment.content_type or "").split(";")[0].strip()


def is_supported_image(attachment):
    """Можно ли отправить это вложение в ИИ: картинка нужного формата и не слишком тяжёлая."""
    return image_type(attachment) in AI_IMAGE_TYPES and attachment.size <= AI_MAX_IMAGE_MB * 1024 * 1024


async def download_images(attachments, guild=None):
    """Скачивает картинки из Discord. Возвращает список пар (байты картинки, формат)."""
    images = []
    for attachment in attachments[:AI_MAX_IMAGES]:  # не больше AI_MAX_IMAGES штук
        try:
            images.append((await attachment.read(), image_type(attachment)))  # read() — скачать вложение
        except Exception as error:
            await report_error("Скачивание картинки для ИИ", error, guild)
    return images


def ai_config(system, model=None):
    """Настройки запроса к Gemini: инструкция, предел длины и уровень "обдумывания"."""
    level = AI_THINKING_LEVEL
    if level == "minimal" and "lite" not in (model or AI_MODEL):
        level = "low"  # "minimal" есть только у Flash-Lite — для других моделей берём ближайший
    return types.GenerateContentConfig(
        system_instruction=system,
        max_output_tokens=AI_MAX_TOKENS,
        # Уровень "обдумывания": чем выше, тем умнее, но дольше
        thinking_config=types.ThinkingConfig(thinking_level=level) if level else None,
    )


AI_ONE_LINE = setting("ai", "one_line_answers", "on").lower() not in ("off", "нет", "no", "0")
BULLET = re.compile(r"^[-*•]\s+")  # значок пункта списка в начале строки: "- ", "* ", "• "


def to_one_line(text):
    """Склеивает ответ в одну строку: абзацы и переносы → пробелы, значки списков убираются.
    Ответы с кодом (```) не трогаем — код в одну строку сломается."""
    if "```" in text:
        return text
    result, previous_bullet = "", False
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        bullet = bool(BULLET.match(line))
        line = BULLET.sub("", line)
        # пункты списка подряд разделяем запятой ("молоко, хлеб, сыр"), всё остальное — пробелом
        separator = ", " if bullet and previous_bullet else " "
        result = f"{result}{separator}{line}" if result else line
        previous_bullet = bullet
    return result


def is_overloaded(error):
    """Google ответил "перегружен / временная ошибка сервера" (коды 5xx) — такое стоит просто повторить."""
    return isinstance(error, genai_errors.ServerError) or "503" in str(error) or "UNAVAILABLE" in str(error)


def is_quota_error(error):
    """Google ответил "лимит запросов исчерпан" (код 429)."""
    return "429" in str(error) or "RESOURCE_EXHAUSTED" in str(error)


async def ai_generate(contents, system):
    """Запрос к Gemini с повторами и запасной моделью.
    1) Основная модель: если Google ответил 503 "перегружен" — ждём и пробуем ещё (до AI_RETRIES раз).
    2) Не помогло (или кончился лимит основной модели) — тот же вопрос запасной модели AI_FALLBACK_MODEL.
    Если не ответила ни одна — выбрасывает последнюю ошибку (её поймает тот, кто вызвал)."""
    models = [AI_MODEL] + ([AI_FALLBACK_MODEL] if AI_FALLBACK_MODEL else [])
    last_error = None
    for model in models:
        for attempt in range(AI_RETRIES + 1):  # первая попытка + AI_RETRIES повторов
            try:
                response = await ai_client.aio.models.generate_content(
                    model=model, contents=contents, config=ai_config(system, model),
                )
                count_ai_request()  # Google ответил — засчитываем запрос (для /ai_limits)
                if model != AI_MODEL:
                    error_logger.warning("ИИ: основная модель %s не ответила (%r), ответила запасная %s",
                                         AI_MODEL, last_error, model)
                return response
            except Exception as error:
                last_error = error
                if is_quota_error(error):
                    ai_error_text(error)  # отмечаем для /ai_limits, что упёрлись в лимит
                    break                 # лимит у этой модели — повторять бесполезно, сразу к запасной
                if not is_overloaded(error):
                    raise                 # другая ошибка (например, неверный ключ) — повтор не поможет
                if attempt < AI_RETRIES:
                    await asyncio.sleep(2 * (attempt + 1))  # пауза 2, потом 4 секунды — и пробуем снова
    raise last_error


async def ask_ai(question, user, channel_id, guild_id, context="", images=None):
    """Отправляет вопрос в Gemini вместе с памятью и возвращает готовый текст ответа.
    question — текст вопроса, user — кто спрашивает, context — последние сообщения чата (необязательно),
    images — список пар (байты картинки, формат) из download_images (необязательно)."""
    guild_key = guild_id or 0  # в личных сообщениях сервера нет — используем 0

    # Инструкция + дата + то, что бот помнит о собеседнике
    system = ai_system_prompt() + f"\nСегодня {datetime.date.today():%d.%m.%Y}."
    facts = facts_get(guild_key, user.id)
    if facts:
        system += f"\nЧто ты помнишь о собеседнике ({user.display_name}):\n" + "\n".join(f"- {fact}" for _, fact in facts)

    # Разговор: сначала прошлые сообщения из памяти, потом новый вопрос.
    # Формат Gemini: {"role": кто говорит, "parts": [{"text": текст}]}
    current = f"{user.display_name}: {question}"
    prompt = current
    if context:
        prompt = f"Последние сообщения в чате (для контекста):\n{context}\n\nВопрос — {current}"
    contents = [{"role": role, "parts": [{"text": text}]} for role, text in history_load(channel_id)]
    parts = [{"text": prompt}]
    for data, mime_type in images or []:  # картинки идут в том же сообщении, что и вопрос
        parts.append(types.Part.from_bytes(data=data, mime_type=mime_type))
    contents.append({"role": "user", "parts": parts})

    # client.aio — асинхронная версия клиента: бот не "зависает", пока ИИ думает
    response = await ai_generate(contents, system)  # с повторами и запасной моделью
    text =(response.text or "").strip()  # response.text может быть пустым, если модель отказалась отвечать
    if not text:
        return "🤔 ИИ не дал ответа. Попробуй переформулировать вопрос."
    if IGNORE_TAG.search(text):  # ИИ решил, что к нему обратились грубо, — не отвечаем и не запоминаем
        error_logger.info("ИИ промолчала на грубость: пользователь=%s канал=%s", user.id, channel_id)
        return None

    # Ищем метки [ЗАПОМНИТЬ: ...], сохраняем факты и убираем метки из текста
    notes = []
    for fact in REMEMBER_TAG.findall(text):
        result = fact_add(guild_key, user.id, fact)
        if result == "ok":
            notes.append(f"📝 *Запомнила: {fact.strip()[:AI_FACT_LENGTH]}*")
        elif result == "full":
            notes.append(f"📝 *Память о тебе заполнена ({AI_MAX_FACTS} фактов) — удали лишнее через /forget.*")
    text = REMEMBER_TAG.sub("", text).strip()
    if AI_ONE_LINE:
        text = to_one_line(text)  # склеиваем абзацы в одну строку (в памяти тоже — чтобы ИИ не копировал абзацы)

    # Запоминаем этот обмен репликами. Сами картинки в память не сохраняются — только отметка о них.
    remembered_question = current + (f" [прислал картинок: {len(images)}]" if images else "")
    history_save(channel_id, remembered_question, text)

    # finish_reason — почему модель остановилась. MAX_TOKENS значит "упёрлась в предел длины".
    if response.candidates and "MAX_TOKENS" in str(response.candidates[0].finish_reason):
        text += "\n\n✂️ *Ответ обрезан — слишком длинный. Попроси объяснить короче или по частям.*"
    if notes:
        text += "\n" + "\n".join(notes)
    return text


def ai_error_text(error):
    """Понятный текст ошибки ИИ для чата."""
    global ai_last_quota_error  # global — меняем переменную, объявленную вне функции
    if is_quota_error(error):  # так Google сообщает о превышении лимитов
        ai_last_quota_error = time.time()  # запоминаем, когда это случилось (покажем в /ai_limits)
        return "😵 Лимит бесплатных запросов к ИИ на сейчас исчерпан. Попробуй позже (подробнее — /ai_limits)."
    if is_overloaded(error):  # 503 — у Google сейчас слишком много запросов, это временно
        return "😵 ИИ у Google сейчас перегружен, попробуй через пару минут."
    return "😵 ИИ сейчас недоступен, попробуй чуть позже."


def split_message(text, limit=1900):
    """Режет длинный текст на части (в одном сообщении Discord не больше 2000 символов)."""
    parts = []
    while len(text) > limit:
        cut = text.rfind("\n", 0, limit)  # стараемся резать по переносу строки
        if cut <= 0:
            cut = limit                   # переносов нет — режем ровно по лимиту
        part = text[:cut]
        text = text[cut:].lstrip("\n")
        if part.count("```") % 2 == 1:    # нечётное число ``` — значит, разрезали внутри блока кода
            part += "\n```"               # закрываем блок в этом сообщении...
            text = "```\n" + text         # ...и открываем его заново в следующем
        parts.append(part)
    parts.append(text)
    return parts


def is_addressed_to_bot(message):
    """Обращаются ли к боту: упомянули его или ответили на его сообщение."""
    if bot.user in message.mentions:
        return True
    reference = message.reference  # на какое сообщение это ответ (если это ответ)
    if reference and isinstance(reference.resolved, discord.Message):
        return reference.resolved.author == bot.user
    return False


async def chat_context(channel, before_message):
    """Несколько последних сообщений чата одной строкой — чтобы ИИ понимал, о чём разговор."""
    lines = []
    # history — история канала; before — только сообщения раньше текущего
    async for old in channel.history(limit=AI_CONTEXT_MESSAGES, before=before_message):
        if old.author == bot.user:  # свои сообщения пропускаем — свои ответы бот и так помнит
            continue
        text = old.clean_content[:300]  # [:300] — длинные сообщения обрезаем
        if old.attachments:             # было вложение (картинка, файл) — отмечаем это
            text = (text + " [вложение]").strip()
        if text:                        # совсем пустые сообщения пропускаем
            lines.append(f"{old.author.display_name}: {text}")
    lines.reverse()  # history идёт от новых к старым — переворачиваем в обычный порядок
    return "\n".join(lines)


# ============================================================
#  ТАЙМ-АУТЫ ЗА ГРУБОСТЬ
#  Первый раз бот просто молчит. Если человек грубит боту второй раз подряд — тайм-аут на 1 минуту,
#  третий — на 5, четвёртый — на 10 и так далее (список — в settings.ini, раздел [moderation]).
#  Счётчик обнуляется, если человек нормально обратился к боту или долго (strike_reset_minutes) не грубил.
#  Боту нужно право «Отправлять участников подумать» (Moderate Members), а его роль должна быть
#  выше роли нарушителя. Владельцу сервера и администраторам тайм-аут дать нельзя — это ограничение Discord.
# ============================================================

def parse_timeouts(text):
    """«1, 5, 10, 30, 60» → [1, 5, 10, 30, 60] (минуты). off / пусто → [] (тайм-аутов нет)."""
    if text.strip().lower() in ("off", "none", "нет", "0", ""):
        return []
    try:
        minutes = [int(part) for part in text.split(",") if part.strip()]
    except ValueError:
        print(f"settings.ini: timeouts = {text} — нужны числа через запятую, беру 1, 5, 10, 30, 60")
        return [1, 5, 10, 30, 60]
    return [min(max(m, 1), 28 * 24 * 60) for m in minutes]  # Discord разрешает тайм-аут от 1 минуты до 28 дней


TIMEOUT_STEPS = parse_timeouts(setting("moderation", "timeouts", "1, 5, 10, 30, 60"))
STRIKE_RESET_MINUTES = setting("moderation", "strike_reset_minutes", 60)
ANNOUNCE_TIMEOUTS = setting("moderation", "announce_timeouts", "on").lower() not in ("off", "нет", "no", "0")


def minutes_text(minutes):
    """Минуты → текст вроде «1 минуту», «5 минут», «2 часа»."""
    if minutes % (24 * 60) == 0:
        days = minutes // (24 * 60)
        return f"{days} {plural(days, 'день', 'дня', 'дней')}"
    if minutes % 60 == 0:
        hours = minutes // 60
        return f"{hours} {plural(hours, 'час', 'часа', 'часов')}"
    return f"{minutes} {plural(minutes, 'минуту', 'минуты', 'минут')}"


def add_strike(guild_id, user_id):
    """+1 к счётчику грубостей подряд. Возвращает новое значение."""
    now = time.time()
    row = db.execute("SELECT count, last_time FROM rude_strikes WHERE guild_id = ? AND user_id = ?",
                     (guild_id, user_id)).fetchone()
    count = row[0] if row and now - row[1] < STRIKE_RESET_MINUTES * 60 else 0  # давно не грубил — с нуля
    count += 1
    db.execute("""
        INSERT INTO rude_strikes (guild_id, user_id, count, last_time) VALUES (?, ?, ?, ?)
        ON CONFLICT (guild_id, user_id) DO UPDATE SET count = excluded.count, last_time = excluded.last_time
    """, (guild_id, user_id, count, now))
    db.commit()
    return count


def reset_strikes(guild_id, user_id):
    """Человек нормально обратился к боту — грубости больше не "подряд"."""
    if guild_id:
        db.execute("DELETE FROM rude_strikes WHERE guild_id = ? AND user_id = ?", (guild_id, user_id))
        db.commit()


async def punish_rudeness(member, channel):
    """Засчитывает грубость и, начиная со второй подряд, даёт тайм-аут."""
    guild = getattr(member, "guild", None)
    if guild is None or not isinstance(member, discord.Member):  # в личных сообщениях тайм-аутов нет
        return
    count = add_strike(guild.id, member.id)
    error_logger.info("Грубость к боту: пользователь=%s сервер=%s подряд=%s", member.id, guild.id, count)
    if count < 2 or not TIMEOUT_STEPS:  # первый раз — только молчим
        return

    minutes = TIMEOUT_STEPS[min(count - 2, len(TIMEOUT_STEPS) - 1)]  # 2-й раз — первая длительность и т.д.
    try:
        await member.timeout(datetime.timedelta(minutes=minutes), reason=f"Грубость к боту ({count} раз подряд)")
    except discord.Forbidden:
        # Нет права Moderate Members, роль бота ниже или это администратор/владелец — пишем только в логи
        error_logger.warning("Не получилось дать тайм-аут %s: не хватает прав", member.id)
        log_channel = log_channel_for(guild.id)
        if log_channel:
            try:
                await log_channel.send(
                    f"⚠️ Хотела дать тайм-аут {member.mention} за грубость ({count} раз подряд), но не хватает прав: "
                    f"нужно право «Отправлять участников подумать» (Moderate Members), и моя роль должна быть выше его роли.",
                    allowed_mentions=NO_PINGS)
            except Exception:
                pass
        return
    except Exception as error:
        await report_error("Тайм-аут за грубость", error, guild, member, channel)
        return

    text = f"🔇 {member.mention} отдохнёт {minutes_text(minutes)}. Хватит грубить."
    if ANNOUNCE_TIMEOUTS and channel is not None:
        try:
            await channel.send(text, allowed_mentions=NO_PINGS)  # без пинга
        except Exception:
            pass
    log_channel = log_channel_for(guild.id)
    if log_channel and log_channel != channel:
        try:
            await log_channel.send(f"{text} ({count} раз подряд, в {channel.mention if channel else '?'})",
                                   allowed_mentions=NO_PINGS)
        except Exception:
            pass


async def ai_reply(message):
    """Отвечает с помощью ИИ на сообщение, в котором обратились к боту."""
    if is_rude(message.content):  # к боту обратились с матом — молчим (и не тратим запрос к Google)
        error_logger.info("Бот промолчала на мат: пользователь=%s канал=%s", message.author.id, message.channel.id)
        await punish_rudeness(message.author, message.channel)
        return
    refusal = ai_limit_check(message.author.id)
    if refusal:
        await message.reply(refusal, mention_author=False)
        return

    # Убираем из текста само упоминание бота (<@id> или <@!id>)
    question = message.content.replace(f"<@{bot.user.id}>", "").replace(f"<@!{bot.user.id}>", "").strip()
    # Картинки: из самого сообщения и из сообщения, на которое ответили (например, «@бот что тут?» на чужое фото)
    attachments = [a for a in message.attachments if is_supported_image(a)]
    replied = message.reference.resolved if message.reference else None
    if isinstance(replied, discord.Message):
        attachments += [a for a in replied.attachments if is_supported_image(a)]

    if not question:
        question = "(прислал картинку без текста)" if attachments else "(просто позвал тебя, ничего не написав)"

    async with message.channel.typing():  # показываем "бот печатает...", пока ИИ думает
        images = await download_images(attachments, message.guild)
        try:
            context = await chat_context(message.channel, message)
        except Exception:
            context = ""  # нет права читать историю канала — обойдёмся без контекста
        try:
            answer = await ask_ai(question, message.author, message.channel.id, message.guild.id, context, images)
        except Exception as error:
            await report_error("ИИ (ответ на сообщение)", error, message.guild, message.author, message.channel)
            answer = ai_error_text(error)

    if answer is None:  # ИИ сама решила, что обращение грубое, — не отвечаем
        await punish_rudeness(message.author, message.channel)
        return
    reset_strikes(message.guild.id, message.author.id)  # нормальное обращение — счётчик грубостей с нуля
    parts = split_message(answer)
    await message.reply(parts[0], mention_author=False, allowed_mentions=NO_PINGS)
    for part in parts[1:]:  # если ответ длинный — остальные части отдельными сообщениями
        await message.channel.send(part, allowed_mentions=NO_PINGS)


# ============================================================
#  ЭКОНОМИКА: монеты, работа, переводы и игры
# ============================================================

# Значения из config/settings.ini, раздел [economy]
START_BALANCE = setting("economy", "start_balance", 500)                  # монеты новичку при первом использовании
WORK_COOLDOWN = setting("economy", "work_cooldown_hours", 8) * 60 * 60    # как часто можно /work (в секундах)
WORK_MIN, WORK_MAX = sorted((setting("economy", "work_min", 150),         # заработок за /work: от...
                             setting("economy", "work_max", 400)))        # ...и до
MIN_BET = max(1, setting("economy", "min_bet", 10))                       # минимальная ставка в играх (не меньше 1)
MAX_BET = 1_000_000             # максимальная ставка (1_000_000 — то же, что 1000000, подчёркивания для удобства)

# Чем занимался участник в /work — выбирается случайно, только для красоты
WORK_TEXTS = [
    "Ремонт осциллографа в лаборатории",
    "Пайка усилителя для соседа",
    "Репетиторство по физике",
    "Настройка роутера для бабушки",
    "Ночная отладка чужого кода",
    "Доставка пиццы по общаге",
    "Сборка компьютера на заказ",
    "Перемотка трансформатора",
    "Смена в серверной",
    "Лабораторная работа за двоих",
]

# Слоты: символ → "вес" (чем больше, тем чаще выпадает) и выплата за три одинаковых (во сколько раз умножается ставка).
# Два одинаковых символа — ставка возвращается. В среднем игрок получает обратно около 93% ставок.
SLOT_WEIGHTS = {"🍒": 30, "🍋": 25, "🍇": 20, "🔔": 12, "💎": 8, "7️⃣": 5}
SLOT_PAYOUTS = {"🍒": 5, "🍋": 8, "🍇": 12, "🔔": 25, "💎": 50, "7️⃣": 100}


def money_text(amount):
    """Сумма с правильным словом: 1 монета, 3 монеты, 15 430 монет."""
    return f"{number(amount)} {plural(amount, 'монета', 'монеты', 'монет')}"


def get_wallet(guild_id, user):
    """Возвращает (баланс, время последней работы). Если кошелька ещё нет — создаёт его со стартовыми монетами."""
    # INSERT ... ON CONFLICT DO UPDATE: новичку создаём кошелёк, а у старого игрока только обновляем имя
    db.execute("""
        INSERT INTO economy (guild_id, user_id, name, balance, last_work) VALUES (?, ?, ?, ?, 0)
        ON CONFLICT (guild_id, user_id) DO UPDATE SET name = excluded.name
    """, (guild_id, user.id, user.display_name, START_BALANCE))
    db.commit()
    return db.execute(
        "SELECT balance, last_work FROM economy WHERE guild_id = ? AND user_id = ?", (guild_id, user.id)
    ).fetchone()


def add_money(guild_id, user, amount):
    """Прибавляет amount монет (отрицательное число — отнимает). Возвращает новый баланс."""
    get_wallet(guild_id, user)  # убеждаемся, что кошелёк есть
    db.execute("UPDATE economy SET balance = balance + ? WHERE guild_id = ? AND user_id = ?",
               (amount, guild_id, user.id))
    db.commit()
    return get_wallet(guild_id, user)[0]


def money_position(guild_id, balance):
    """Место в рейтинге богатства = сколько участников богаче, плюс один."""
    return db.execute(
        "SELECT COUNT(*) FROM economy WHERE guild_id = ? AND balance > ?", (guild_id, balance)
    ).fetchone()[0] + 1


async def take_bet(interaction, bet):
    """Проверяет, хватает ли монет, и списывает ставку. Возвращает True, если ставка принята."""
    balance, _ = get_wallet(interaction.guild.id, interaction.user)
    if bet > balance:
        await interaction.response.send_message(
            f"Не хватает монет: у тебя {money_text(balance)}, а ставка — {money_text(bet)}. "
            f"Заработать можно через /work.", ephemeral=True)
        return False
    add_money(interaction.guild.id, interaction.user, -bet)
    return True


def spin_slots():
    """Крутит три барабана. random.choices выбирает символы с учётом их "веса"."""
    return random.choices(list(SLOT_WEIGHTS), weights=list(SLOT_WEIGHTS.values()), k=3)


def slots_multiplier(reels):
    """Во сколько раз умножается ставка: три одинаковых — по таблице, два одинаковых — 1 (возврат), иначе 0."""
    if reels[0] == reels[1] == reels[2]:
        return SLOT_PAYOUTS[reels[0]]
    if len(set(reels)) == 2:  # set убирает повторы: осталось 2 разных символа — значит, два одинаковых
        return 1
    return 0


# --- Блэкджек ---

CARD_RANKS = ["2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K", "A"]
CARD_SUITS = ["♠", "♥", "♦", "♣"]
blackjack_players = set()  # id тех, кто сейчас играет: нельзя начать вторую партию, не закончив первую


def new_deck():
    """Новая перемешанная колода из 52 карт. Карта — пара (достоинство, масть)."""
    deck = [(rank, suit) for rank in CARD_RANKS for suit in CARD_SUITS]
    random.shuffle(deck)
    return deck


def hand_value(hand):
    """Сумма очков на руке. Туз — 11, но если получается больше 21, туз считается за 1."""
    value = 0
    aces = 0
    for rank, _ in hand:
        if rank == "A":
            value += 11
            aces += 1
        elif rank in ("J", "Q", "K"):  # картинки — по 10
            value += 10
        else:
            value += int(rank)
    while value > 21 and aces:  # перебор — превращаем тузы из 11 в 1, пока можно
        value -= 10
        aces -= 1
    return value


def hand_text(hand, hide_second=False):
    """Карты текстом, например `A♠` `10♥`. hide_second — спрятать вторую карту дилера."""
    cards = [f"`{rank}{suit}`" for rank, suit in hand]
    if hide_second:
        cards[1] = "`??`"
    return " ".join(cards)


class BlackjackGame(discord.ui.View):
    """Одна партия в блэкджек. View — набор кнопок под сообщением («Ещё карту», «Хватит»)."""

    def __init__(self, guild_id, player, bet):
        super().__init__(timeout=120)  # если 2 минуты не нажимать кнопки — бот сам нажмёт «Хватит»
        self.guild_id = guild_id
        self.player = player
        self.bet = bet
        self.deck = new_deck()
        self.hand = [self.deck.pop(), self.deck.pop()]    # карты игрока (pop — взять карту из колоды)
        self.dealer = [self.deck.pop(), self.deck.pop()]  # карты дилера
        self.message = None   # сообщение с игрой — нужно, чтобы обновить его, если время выйдет
        self.result = None    # итог партии текстом; пока None — партия идёт
        self.color = 0x8B5CF6 # цвет полоски слева от карточки

    def embed(self):
        """Карточка с текущим состоянием партии."""
        finished = self.result is not None
        embed = discord.Embed(title="🃏 Блэкджек", color=self.color)
        embed.add_field(name=f"Твои карты — {hand_value(self.hand)}", value=hand_text(self.hand), inline=False)
        if finished:  # в конце показываем все карты дилера
            embed.add_field(name=f"Карты дилера — {hand_value(self.dealer)}", value=hand_text(self.dealer),
                            inline=False)
        else:         # пока игра идёт, вторая карта дилера скрыта
            embed.add_field(name="Карты дилера — ?", value=hand_text(self.dealer, hide_second=True), inline=False)
        embed.add_field(name="Ставка", value=money_text(self.bet), inline=False)
        if finished:
            embed.add_field(name="Итог", value=self.result, inline=False)
        embed.set_footer(text=f"Игрок: {self.player.display_name}")
        return embed

    async def interaction_check(self, interaction):
        """Discord вызывает это перед каждым нажатием кнопки: нажимать может только сам игрок."""
        if interaction.user.id != self.player.id:
            await interaction.response.send_message("Это не твоя партия 🙂 Начни свою: /blackjack", ephemeral=True)
            return False
        return True

    @discord.ui.button(label="Ещё карту", emoji="🃏", style=discord.ButtonStyle.primary)
    async def hit(self, interaction, button):
        self.hand.append(self.deck.pop())
        if hand_value(self.hand) >= 21:  # перебор или ровно 21 — дальше брать нечего
            self.finish()
        # view=None убирает кнопки, когда партия закончилась
        await interaction.response.edit_message(embed=self.embed(), view=self if self.result is None else None)

    @discord.ui.button(label="Хватит", emoji="✋", style=discord.ButtonStyle.secondary)
    async def stand(self, interaction, button):
        self.finish()
        await interaction.response.edit_message(embed=self.embed(), view=None)

    def finish(self):
        """Дилер добирает карты, считаем итог и выплачиваем выигрыш."""
        player = hand_value(self.hand)
        player_blackjack = len(self.hand) == 2 and player == 21                   # 21 с двух первых карт
        dealer_blackjack = len(self.dealer) == 2 and hand_value(self.dealer) == 21
        # Дилер берёт карты, пока у него меньше 17 (если у игрока нет перебора и ни у кого нет блэкджека)
        if player <= 21 and not player_blackjack and not dealer_blackjack:
            while hand_value(self.dealer) < 17:
                self.dealer.append(self.deck.pop())
        dealer = hand_value(self.dealer)

        # payout — сколько вернуть игроку (ставка уже списана в начале)
        if player > 21:
            payout, self.result = 0, f"💥 Перебор! Проигрыш: {money_text(self.bet)}."
        elif player_blackjack and dealer_blackjack:
            payout, self.result = self.bet, "🤝 Блэкджек у обоих — ничья, ставка возвращена."
        elif player_blackjack:
            payout = self.bet + self.bet * 3 // 2  # блэкджек платит 3 к 2
            self.result = f"🎉 Блэкджек! Выигрыш: +{money_text(payout - self.bet)}."
        elif dealer_blackjack:
            payout, self.result = 0, f"😔 У дилера блэкджек. Проигрыш: {money_text(self.bet)}."
        elif dealer > 21:
            payout, self.result = self.bet * 2, f"🎉 Дилер перебрал! Выигрыш: +{money_text(self.bet)}."
        elif player > dealer:
            payout, self.result = self.bet * 2, f"🎉 Победа! Выигрыш: +{money_text(self.bet)}."
        elif player == dealer:
            payout, self.result = self.bet, "🤝 Ничья — ставка возвращена."
        else:
            payout, self.result = 0, f"😔 У дилера больше. Проигрыш: {money_text(self.bet)}."

        if payout > self.bet:
            self.color = 0x22C55E  # зелёный — выигрыш
        elif payout == self.bet:
            self.color = 0x94A3B8  # серый — ничья
        else:
            self.color = 0xEF4444  # красный — проигрыш

        new_balance = add_money(self.guild_id, self.player, payout) if payout else get_wallet(self.guild_id, self.player)[0]
        self.result += f"\nБаланс: {money_text(new_balance)}"
        blackjack_players.discard(self.player.id)  # игрок свободен — можно начинать новую партию
        self.stop()  # останавливаем View: кнопки больше не реагируют

    async def on_timeout(self):
        """Вызывается, если 2 минуты никто не нажимал кнопки: бот сам говорит «Хватит»."""
        if self.result is None:
            self.finish()
            if self.message:
                try:
                    await self.message.edit(embed=self.embed(), view=None)
                except Exception:
                    pass


# ============================================================
#  УТРЕННИЕ И ВЕЧЕРНИЕ СООБЩЕНИЯ
# ============================================================

from zoneinfo import ZoneInfo  # часовые пояса (входит в Python; на Windows нужен ещё пакет tzdata)

DAILY_WINDOW = 30  # если бот был выключен в нужную минуту, сообщение ещё отправится в течение 30 минут

# О чём рассказывать факты (тема выбирается случайно)
DAILY_TOPICS = ["математика", "физика", "инженерия", "электроника и радиотехника", "астрономия",
                "информатика и программирование", "химия", "история изобретений"]

# Часовые пояса для /daily_setup: код пояса → (название, запасной сдвиг от UTC в часах — если нет пакета tzdata)
DAILY_TIMEZONES = {
    "Asia/Jerusalem": ("Израиль", 2),
    "Europe/Moscow": ("Москва", 3),
    "Europe/Kyiv": ("Киев", 2),
    "Europe/Minsk": ("Минск", 3),
    "Asia/Almaty": ("Казахстан (Алматы)", 5),
    "Asia/Tashkent": ("Ташкент", 5),
    "Europe/Berlin": ("Центральная Европа (Берлин)", 1),
    "Europe/London": ("Лондон", 0),
    "America/New_York": ("Нью-Йорк", -5),
}

# Запасные факты — если ИИ не настроен или недоступен
FALLBACK_FACTS = [
    "число π иррационально: его десятичная запись бесконечна и никогда не повторяется. Это доказал Иоганн Ламберт в XVIII веке.",
    "свет от Солнца летит до Земли около 8 минут 20 секунд — мы всегда видим Солнце немного «в прошлом».",
    "сумма углов треугольника, нарисованного на сфере, всегда больше 180°.",
    "первый транзистор создали в 1947 году в Bell Labs Джон Бардин, Уолтер Браттейн и Уильям Шокли.",
    "запись 0,999… с бесконечными девятками — это ровно 1, а не «чуть меньше единицы».",
    "звук в стали распространяется более чем в 15 раз быстрее, чем в воздухе.",
    "Эйлер в 1736 году решил задачу о семи мостах Кёнигсберга — с неё началась теория графов.",
    "спутники GPS учитывают эффекты теории относительности: без поправок ошибка накапливалась бы примерно на 10 км в сутки.",
    "в группе всего из 23 человек вероятность того, что у двоих совпадут дни рождения, больше 50%.",
    "в оптоволокне свет идёт примерно на треть медленнее, чем в вакууме.",
    "канал молнии нагревается примерно до 30 000 °C — это в несколько раз горячее поверхности Солнца.",
    "простых чисел бесконечно много — Евклид доказал это больше 2000 лет назад.",
    "вода имеет наибольшую плотность при температуре около 4 °C, поэтому зимой у дна водоёмов вода теплее, а замерзают они сверху.",
    "Томас Хейлс в 1999 году доказал, что шестиугольные соты — способ разбить плоскость на равные ячейки с наименьшей общей длиной границ.",
]


def zone(name):
    """Часовой пояс по коду. Если на компьютере нет базы поясов (tzdata) — берём запасной сдвиг."""
    try:
        return ZoneInfo(name)
    except Exception:
        return datetime.timezone(datetime.timedelta(hours=DAILY_TIMEZONES.get(name, ("", 0))[1]))


async def make_daily_text(kind, guild=None):
    """Текст утреннего (kind="morning") или вечернего ("evening") сообщения. Пишет ИИ, а без него — запасной вариант."""
    topic = random.choice(DAILY_TOPICS)
    today = datetime.date.today()
    if kind == "morning":
        task = (f"Напиши утреннее сообщение для Discord-чата друзей. Пожелай всем доброго утра и удачи на сегодня, "
                f"затем расскажи один интересный факт из области «{topic}». Факт должен быть широко известным "
                f"и достоверным — без выдуманных чисел, дат и имён. Всего 3–6 предложений.")
    else:
        task = (f"Напиши вечернее сообщение для Discord-чата друзей. Расскажи интересный «факт дня» из области «{topic}» "
                f"(если точно знаешь событие из этой области, связанное с датой {today:%d.%m}, — можно о нём, "
                f"иначе просто интересный факт). Факт должен быть достоверным — без выдуманных чисел, дат и имён. "
                f"Потом пожелай всем удачного завтрашнего дня и спокойной ночи. Всего 3–6 предложений.")
    if ai_client:
        try:
            response = await ai_generate(task, ai_system_prompt() + f"\nСегодня {today:%d.%m.%Y}.")
            text = REMEMBER_TAG.sub("", response.text or "")  # на всякий случай убираем метки памяти...
            text = IGNORE_TAG.sub("", text).strip()            # ...и метку "не отвечаю"
            if text:
                return text
        except Exception as error:
            await report_error("ИИ (ежедневное сообщение)", error, guild)
            ai_error_text(error)  # чтобы /ai_limits знала, если Google отказал из-за лимита
    fact = random.choice(FALLBACK_FACTS)  # ИИ не ответил — берём запасной факт
    if kind == "morning":
        return f"Доброе утро! Удачи всем сегодня — пусть всё получится 💪\n\n🧠 **Интересный факт:** {fact}"
    return f"🧠 **Факт дня:** {fact}\n\nПусть завтрашний день будет удачным. Спокойной ночи! 🌙"


async def send_daily(channel_id, kind):
    """Отправляет утреннее или вечернее сообщение в канал. Возвращает True, если получилось."""
    channel = bot.get_channel(channel_id)
    if channel is None:
        print("Ежедневные сообщения: не нашла канал", channel_id)
        return False
    text = await make_daily_text(kind, channel.guild)
    if kind == "morning":
        embed = discord.Embed(title="☀️ Доброе утро!", description=text[:4000], color=0xF59E0B)
    else:
        embed = discord.Embed(title="🌙 Спокойной ночи!", description=text[:4000], color=0x6366F1)
    try:
        await channel.send(embed=embed)
        return True
    except Exception as error:
        await report_error("Отправка ежедневного сообщения", error, channel.guild, channel=channel)
        return False


@tasks.loop(minutes=1)  # раз в минуту проверяем, не пора ли отправить сообщение
async def daily_loop():
    rows = db.execute(
        "SELECT guild_id, channel_id, timezone, morning, evening, last_morning, last_evening FROM daily_settings"
    ).fetchall()
    for guild_id, channel_id, tz_name, morning, evening, last_morning, last_evening in rows:
        try:
            now = datetime.datetime.now(zone(tz_name))  # текущее время в часовом поясе сервера
            today = now.date().isoformat()
            for kind, at, last in (("morning", morning, last_morning), ("evening", evening, last_evening)):
                if last == today:  # сегодня уже отправляли
                    continue
                hour, minute = map(int, at.split(":"))  # "08:00" → 8 и 0
                scheduled = now.replace(hour=hour, minute=minute, second=0, microsecond=0)
                # Пора, если нужное время уже наступило, но прошло не больше DAILY_WINDOW минут
                if 0 <= (now - scheduled).total_seconds() < DAILY_WINDOW * 60:
                    # Сначала отмечаем, что отправили, — чтобы не отправить дважды, если ИИ будет долго думать.
                    # kind здесь только "morning" или "evening", поэтому подставлять его в запрос безопасно.
                    db.execute(f"UPDATE daily_settings SET last_{kind} = ? WHERE guild_id = ?", (today, guild_id))
                    db.commit()
                    await send_daily(channel_id, kind)
        except Exception as error:  # ошибка на одном сервере не должна останавливать проверку остальных
            await report_error("Ежедневные сообщения", error, guild_id)


@daily_loop.before_loop
async def before_daily_loop():
    """Перед первым запуском ждём, пока бот полностью подключится к Discord."""
    await bot.wait_until_ready()


@bot.event
async def on_message(message):
    """Вызывается на каждое новое сообщение в любом канале, который видит бот."""
    if message.author.bot or message.guild is None:  # не реагируем на ботов (и на себя) и на личные сообщения
        return
    if ai_client and is_addressed_to_bot(message):  # к боту обратились — отвечает ИИ
        await ai_reply(message)
    else:
        await auto_reply(message)  # иначе — обычные автоответы
    await add_xp(message)          # опыт за сообщение


@bot.event
async def setup_hook():
    """Вызывается при запуске бота, до подключения. Здесь регистрируем слэш-команды."""
    synced = await bot.tree.sync()  # отправляем список наших команд в Discord
    print(f"Зарегистрировано слэш-команд: {len(synced)}")
    voice_xp_loop.start()           # запускаем начисление опыта за голосовые каналы (раз в минуту)
    daily_loop.start()              # и проверку утренних/вечерних сообщений (раз в минуту)


@bot.event
async def on_ready():
    """Вызывается, когда бот успешно подключился к Discord."""
    print(f"Бот запущен как {bot.user}")


@bot.event
async def on_error(event, *args, **kwargs):
    """Вызывается, если ошибка случилась в событии (например, при обработке сообщения в on_message)."""
    error = sys.exc_info()[1]  # та ошибка, из-за которой нас вызвали
    message = args[0] if args and isinstance(args[0], discord.Message) else None
    if message:
        await report_error(f"Событие {event}", error, message.guild, message.author, message.channel)
    else:
        await report_error(f"Событие {event}", error)


# @bot.tree.command создаёт слэш-команду. name — как она называется, description — подсказка в Discord.
@bot.tree.command(name="play", description="Включить песню по названию или ссылке")
@app_commands.describe(query="Название песни или ссылка (YouTube, SoundCloud, Spotify)")  # подсказка к полю ввода
@app_commands.guild_only()  # команда работает только на серверах, не в личных сообщениях
async def play(interaction: discord.Interaction, query: str):
    # interaction — это "вызов команды": кто вызвал, на каком сервере, в каком канале.
    # query — текст, который человек ввёл в поле команды.

    if interaction.user.voice is None:  # человек не в голосовом канале
        # ephemeral=True — это сообщение увидит только тот, кто вызвал команду
        await interaction.response.send_message("Сначала зайди в голосовой канал.", ephemeral=True)
        return

    # Discord требует ответить на команду за 3 секунды. Поиск дольше,
    # поэтому сразу говорим "бот думает..." — после этого есть 15 минут на ответ.
    await interaction.response.defer()

    vc = interaction.guild.voice_client               # бот уже в канале?
    if vc is None:
        vc = await interaction.user.voice.channel.connect()  # подключаемся к каналу автора команды

    match = SPOTIFY_LINK.search(query)  # проверяем, не ссылка ли это на Spotify
    if match:
        kind, spotify_id = match.groups()  # kind = "track" / "album" / "playlist", spotify_id = id из ссылки

        queries = None
        # Треки и альбомы — через Spotify API, если ключи заданы. Плейлисты API чужим приложениям не отдаёт,
        # поэтому их (и всё остальное, если ключей нет или API не ответил) берём со страницы плеера Spotify.
        if spotify is not None and kind != "playlist":
            try:
                queries, name = await spotify_lookup(kind, spotify_id)  # узнаём названия песен
            except Exception as error:
                await report_error("Spotify API /play", error, interaction.guild, interaction.user,
                                   interaction.channel)
        if queries is None:
            try:
                queries, name = await spotify_embed_lookup(kind, spotify_id)
            except Exception as error:
                await report_error("Spotify (страница плеера) /play", error, interaction.guild, interaction.user,
                                   interaction.channel)
                what = "плейлист приватный или удалён" if kind == "playlist" else "ссылка неправильная или трек удалён"
                await interaction.followup.send(f"Не получилось получить данные из Spotify: возможно, {what}.")
                return

        if kind in ("album", "playlist"):
            await add_album(interaction, queries, name, kind)  # у альбома и плейлиста отдельная обработка
            return

        # Для одного трека дальше ищем "Исполнитель - Название" на YouTube, как обычный текст.
        query = queries[0]

    try:
        _, title, page_url = await search(query)  # ищем песню (ссылка на звук сейчас не нужна — "_")
    except Exception:
        await interaction.followup.send("Не получилось найти или загрузить этот трек.")
        return

    idle = not vc.is_playing() and not vc.is_paused()  # сейчас ничего не играет?
    waiting = queue_count(interaction.guild.id)         # сколько треков уже ждут в очереди
    queue_add(interaction.guild.id, page_url, title)    # добавляем песню в конец очереди

    if idle and waiting == 0:
        await interaction.followup.send(f"🔎 Нашёл: **{title}**")
    elif idle:
        await interaction.followup.send(
            f"➕ Добавил **{title}** в конец очереди. Сначала сыграю сохранённые треки: {waiting}."
        )
    else:
        await interaction.followup.send(f"➕ Добавлено в очередь: **{title}**")

    await play_next(interaction.guild, interaction.channel)  # если ничего не играет — начнёт играть


@bot.tree.command(name="skip", description="Пропустить текущую песню")
@app_commands.guild_only()
async def skip(interaction: discord.Interaction):
    vc = interaction.guild.voice_client
    if vc and vc.is_playing():
        vc.stop()  # остановка песни вызывает after → play_next включит следующую
        await interaction.response.send_message("⏭️ Пропущено.")
    else:
        await interaction.response.send_message("Сейчас ничего не играет.", ephemeral=True)


@bot.tree.command(name="pause", description="Поставить на паузу")
@app_commands.guild_only()
async def pause(interaction: discord.Interaction):
    vc = interaction.guild.voice_client
    if vc and vc.is_playing():
        vc.pause()
        await interaction.response.send_message("⏸️ Пауза.")
    else:
        await interaction.response.send_message("Сейчас ничего не играет.", ephemeral=True)


@bot.tree.command(name="resume", description="Продолжить после паузы или запустить сохранённую очередь")
@app_commands.guild_only()
async def resume(interaction: discord.Interaction):
    vc = interaction.guild.voice_client

    if vc and vc.is_paused():  # обычная пауза — просто продолжаем
        vc.resume()
        await interaction.response.send_message("▶️ Продолжаю.")
        return

    waiting = queue_count(interaction.guild.id)
    if waiting == 0:
        await interaction.response.send_message("Очередь пуста — включи что-нибудь через /play.", ephemeral=True)
        return
    if vc and vc.is_playing():
        await interaction.response.send_message("Музыка и так играет 🙂", ephemeral=True)
        return

    if vc is None:  # бот не в канале — заходим к тому, кто вызвал команду
        if interaction.user.voice is None:
            await interaction.response.send_message("Сначала зайди в голосовой канал.", ephemeral=True)
            return
        await interaction.response.defer()
        await interaction.user.voice.channel.connect()
        await interaction.followup.send(f"▶️ Продолжаю сохранённую очередь: {waiting} {plural(waiting, 'трек', 'трека', 'треков')}.")
    else:
        await interaction.response.send_message("▶️ Продолжаю очередь.")

    await play_next(interaction.guild, interaction.channel)


@bot.tree.command(name="queue", description="Показать очередь песен")
@app_commands.guild_only()
async def show_queue(interaction: discord.Interaction):
    total = queue_count(interaction.guild.id)
    if total == 0:
        await interaction.response.send_message("Очередь пуста.")
        return
    titles = queue_titles(interaction.guild.id, 15)  # показываем первые 15
    # enumerate(titles, 1) нумерует песни с единицы: 1. ..., 2. ...
    lines = [f"{i}. {title}" for i, title in enumerate(titles, 1)]
    text = f"📜 Очередь ({total} {plural(total, 'трек', 'трека', 'треков')}):\n" + "\n".join(lines)
    if total > len(titles):
        text += f"\n…и ещё {total - len(titles)}"
    await interaction.response.send_message(text)


@bot.tree.command(name="remove_all", description="Удалить все треки из очереди")
@app_commands.guild_only()
async def remove_all(interaction: discord.Interaction):
    removed = queue_clear(interaction.guild.id)  # удаляем и узнаём, сколько было
    text = f"🗑️ Очередь очищена, удалено треков: {removed}."
    vc = interaction.guild.voice_client
    if vc and (vc.is_playing() or vc.is_paused()):
        text += "\nТекущий трек доиграет — чтобы выключить его, используй /skip."
    await interaction.response.send_message(text)


@bot.tree.command(name="setvolume", description="Изменить громкость (от 0 до 200%)")
@app_commands.describe(level="Громкость в процентах: 100 — обычная, 50 — тише, 150 — громче")
@app_commands.guild_only()
async def setvolume(interaction: discord.Interaction, level: app_commands.Range[int, 0, 200]):
    # app_commands.Range[int, 0, 200] — Discord сам не даст ввести что-то кроме целого числа от 0 до 200.

    volumes[interaction.guild.id] = level  # запоминаем громкость для этого сервера (для следующих песен)

    vc = interaction.guild.voice_client
    # Если сейчас что-то играет и у источника есть регулятор громкости — меняем её сразу.
    if vc and isinstance(vc.source, discord.PCMVolumeTransformer):
        vc.source.volume = level / 100  # переводим проценты в множитель

    await interaction.response.send_message(f"🔊 Громкость: **{level}%**")


@bot.tree.command(name="rank", description="Показать карточку уровня")
@app_commands.describe(member="Чей уровень показать (если не выбрать — твой)")
@app_commands.guild_only()
async def rank(interaction: discord.Interaction, member: discord.Member | None = None):
    # discord.Member | None = None — участника можно выбрать, а можно оставить поле пустым
    member = member or interaction.user  # если никого не выбрали — показываем того, кто вызвал команду
    await interaction.response.defer()   # рисование занимает пару секунд — показываем "бот думает..."

    total_xp = get_xp(interaction.guild.id, member.id)
    level, current, need = level_info(total_xp)
    position = get_position(interaction.guild.id, total_xp) if total_xp > 0 else None  # без опыта места нет
    avatar = await get_avatar(member)

    loop = asyncio.get_running_loop()
    image = await loop.run_in_executor(
        None, make_rank_card, member.display_name, avatar, level, current, need, total_xp, position,
        pick_background("rank"),  # фон карточки (если есть файл rank_bg)
    )
    await interaction.followup.send(file=discord.File(image, filename="rank.png"))


@bot.tree.command(name="leaderboard", description="Таблица рейтинга сервера (топ-10)")
@app_commands.guild_only()
async def leaderboard(interaction: discord.Interaction):
    # Берём из базы 10 участников этого сервера с наибольшим опытом (ORDER BY xp DESC — по убыванию)
    rows = db.execute(
        "SELECT user_id, name, xp FROM levels WHERE guild_id = ? ORDER BY xp DESC LIMIT 10",
        (interaction.guild.id,),
    ).fetchall()  # fetchall() — все найденные строки списком

    if not rows:
        await interaction.response.send_message("Пока никто не набрал опыта — пишите в чат!")
        return

    await interaction.response.defer()

    players = []
    for user_id, name, xp in rows:
        avatar = await get_avatar_by_id(interaction.guild, user_id)
        level, current, need = level_info(xp)
        players.append({"name": name, "avatar": avatar, "level": level, "xp": xp, "progress": current / need})

    loop = asyncio.get_running_loop()
    image = await loop.run_in_executor(None, make_leaderboard, interaction.guild.name, players,
                                       pick_background("leaderboard"))
    await interaction.followup.send(file=discord.File(image, filename="leaderboard.png"))


@bot.tree.command(name="top_chart_month", description="Топ-10 самых прослушиваемых треков за этот месяц")
@app_commands.guild_only()
async def top_chart_month(interaction: discord.Interaction):
    clear_old_months()  # если начался новый месяц — сначала обнуляем старую статистику
    rows = db.execute(
        "SELECT title, count FROM plays WHERE guild_id = ? AND month = ? ORDER BY count DESC, title LIMIT 10",
        (interaction.guild.id, current_month()),
    ).fetchall()  # список пар (название, сколько раз включали), от самых популярных

    if not rows:
        await interaction.response.send_message("В этом месяце ещё ничего не слушали — включай /play!")
        return

    await interaction.response.defer()
    today = datetime.date.today()
    month_title = f"{MONTH_NAMES[today.month - 1]} {today.year}"  # например "Октябрь 2026"
    loop = asyncio.get_running_loop()
    image = await loop.run_in_executor(None, make_top_chart, interaction.guild.name, month_title, rows,
                                       pick_background("chart"))
    await interaction.followup.send(file=discord.File(image, filename="top_chart.png"))


@bot.tree.command(name="ask", description="Задать вопрос ИИ (можно с картинкой)")
@app_commands.describe(question="Твой вопрос", image="Картинка к вопросу (необязательно): PNG, JPEG, WEBP")
async def ask(interaction: discord.Interaction, question: str, image: discord.Attachment | None = None):
    # discord.Attachment | None = None — к команде можно прикрепить файл, а можно и не прикреплять
    if ai_client is None:  # ключ не задан
        await interaction.response.send_message("ИИ не настроен: нет ключа GEMINI_API_KEY.", ephemeral=True)
        return
    if image and not is_supported_image(image):
        await interaction.response.send_message(
            f"Это не подходит: нужна картинка PNG, JPEG или WEBP не больше {AI_MAX_IMAGE_MB} МБ.", ephemeral=True)
        return
    if is_rude(question):  # вопрос с матом — молчим
        error_logger.info("Бот промолчала на мат в /ask: пользователь=%s канал=%s",
                          interaction.user.id, interaction.channel_id)
        # На слэш-команду Discord требует ответ, иначе покажет «приложение не отвечает». Поэтому отвечаем
        # невидимо: "думаю..." видно только автору, и мы сразу его удаляем — получается просто тишина.
        await interaction.response.defer(ephemeral=True)
        await interaction.delete_original_response()
        await punish_rudeness(interaction.user, interaction.channel)
        return
    refusal = ai_limit_check(interaction.user.id)
    if refusal:
        await interaction.response.send_message(refusal, ephemeral=True)
        return

    await interaction.response.defer()  # ИИ отвечает несколько секунд — показываем "бот думает..."
    images = await download_images([image], interaction.guild) if image else []
    try:
        answer = await ask_ai(question, interaction.user, interaction.channel_id, interaction.guild_id,
                              images=images)
    except Exception as error:
        await report_error("ИИ /ask", error, interaction.guild, interaction.user, interaction.channel)
        answer = ai_error_text(error)

    if answer is None:  # ИИ решила, что вопрос грубый, — убираем "бот думает..." и молчим
        await interaction.delete_original_response()
        await punish_rudeness(interaction.user, interaction.channel)
        return
    reset_strikes(interaction.guild_id, interaction.user.id)  # нормальный вопрос — счётчик грубостей с нуля
    parts =split_message(f"❓ {question}\n\n{answer}")  # сначала вопрос, потом ответ
    if image:  # показываем картинку, о которой спрашивали, в "карточке" под первым сообщением
        embed = discord.Embed(color=0x8B5CF6).set_image(url=image.url)
        await interaction.followup.send(parts[0], embed=embed, allowed_mentions=NO_PINGS)
    else:
        await interaction.followup.send(parts[0], allowed_mentions=NO_PINGS)
    for part in parts[1:]:
        await interaction.followup.send(part, allowed_mentions=NO_PINGS)


@bot.tree.command(name="ai_limits", description="Сколько запросов к ИИ осталось")
async def ai_limits(interaction: discord.Interaction):
    if ai_client is None:
        await interaction.response.send_message("ИИ не настроен: нет ключа GEMINI_API_KEY.", ephemeral=True)
        return

    # --- Личный лимит (его ведёт сам бот) ---
    today = datetime.date.today().isoformat()
    day, count, last_time = ai_usage.get(interaction.user.id, (today, 0, 0))
    if day != today:  # вопросы были вчера — сегодня счётчик с нуля
        count = 0
    left = max(0, AI_DAILY_LIMIT - count)
    # Личный счётчик обнуляется в полночь по времени компьютера, на котором запущен бот
    tomorrow = datetime.datetime.combine(datetime.date.today() + datetime.timedelta(days=1), datetime.time())
    personal_reset = int(tomorrow.timestamp())
    wait = AI_COOLDOWN - (time.time() - last_time)  # сколько секунд ещё ждать до следующего вопроса

    # --- Общий лимит ключа Google ---
    used = ai_requests_today()
    google_reset = google_reset_time()

    # <t:ЧИСЛО:t> и <t:ЧИСЛО:R> — особый формат Discord: каждый увидит время в своём часовом поясе
    # (":t" — время, например 10:00; ":R" — "через 5 часов")
    lines = [
        "📊 **Лимиты ИИ**",
        "",
        "**Твой личный лимит** (ограничение бота):",
        f"• Осталось вопросов сегодня: **{left}** из {AI_DAILY_LIMIT}",
        f"• Обновится: <t:{personal_reset}:t> (<t:{personal_reset}:R>)",
    ]
    if wait > 0:
        lines.append(f"• Следующий вопрос можно задать через {int(wait) + 1} сек")
    lines += [
        "",
        f"**Общий лимит Google** (на весь сервер, модель `{AI_MODEL}`):",
        f"• Запросов сегодня: **{used}** из ~{AI_GOOGLE_DAILY_LIMIT} (осталось ~{max(0, AI_GOOGLE_DAILY_LIMIT - used)})",
        f"• За последнюю минуту: {ai_requests_last_minute()} из ~{AI_GOOGLE_MINUTE_LIMIT}",
        f"• Обновится: <t:{google_reset}:t> (<t:{google_reset}:R>)",
    ]
    if time.time() - ai_last_quota_error < 24 * 60 * 60:  # Google отказывал за последние сутки
        lines.append(f"⚠️ Google последний раз отказал из-за лимита <t:{int(ai_last_quota_error)}:R>")
    lines += [
        "",
        "-# Лимиты Google примерные, бот считает только свои запросы. Точно: aistudio.google.com/rate-limit",
    ]  # "-# " — в Discord это мелкий серый текст
    await interaction.response.send_message("\n".join(lines), ephemeral=True)  # видит только сам человек


@bot.tree.command(name="remember", description="Попросить бота запомнить факт о тебе")
@app_commands.describe(fact="Что запомнить, например: люблю рок, учусь на радиотехника")
async def remember(interaction: discord.Interaction, fact: app_commands.Range[str, 1, AI_FACT_LENGTH]):
    # Range[str, 1, AI_FACT_LENGTH] — Discord не даст написать пустой факт или длиннее 200 символов
    result = fact_add(interaction.guild_id or 0, interaction.user.id, fact)
    if result == "ok":
        text = f"📝 Запомнила: *{fact.strip()}*"
    elif result == "duplicate":
        text = "Я это и так помню!"
    else:  # "full"
        text = f"Память о тебе заполнена ({AI_MAX_FACTS} фактов). Удали лишнее через /forget."
    await interaction.response.send_message(text, ephemeral=True)  # видит только сам человек


@bot.tree.command(name="memory", description="Что бот помнит о тебе")
async def memory(interaction: discord.Interaction):
    facts = facts_get(interaction.guild_id or 0, interaction.user.id)
    if facts:
        # Нумеруем с 1 — по этим номерам факты можно удалять через /forget
        lines = [f"{number_}. {fact}" for number_, (_, fact) in enumerate(facts, 1)]
        text = f"🧠 Что я помню о тебе ({len(facts)}/{AI_MAX_FACTS}):\n" + "\n".join(lines)
    else:
        text = "🧠 Я пока ничего о тебе не помню. Скажи мне «запомни, что …» или используй /remember."
    history_count = len(history_load(interaction.channel_id))
    text += f"\n\n💬 В памяти разговора в этом канале сообщений: {history_count}/{AI_HISTORY_LIMIT}."
    await interaction.response.send_message(text, ephemeral=True)


@bot.tree.command(name="forget", description="Стереть память бота")
@app_commands.describe(what="Что стереть", number="Номер факта из /memory (только для «Один факт»)")
@app_commands.choices(what=[
    app_commands.Choice(name="🗑️ Один факт обо мне (по номеру)", value="fact"),
    app_commands.Choice(name="🧹 Все факты обо мне", value="facts"),
    app_commands.Choice(name="💬 Память разговора в этом канале", value="chat"),
])
async def forget(interaction: discord.Interaction, what: app_commands.Choice[str], number: int | None = None):
    guild_key = interaction.guild_id or 0

    if what.value == "chat":
        removed = history_clear(interaction.channel_id)
        await interaction.response.send_message(f"💬 Память разговора в этом канале стёрта (сообщений: {removed}).")
        return

    facts = facts_get(guild_key, interaction.user.id)
    if what.value == "facts":
        db.execute("DELETE FROM ai_memory WHERE guild_id = ? AND user_id = ?", (guild_key, interaction.user.id))
        db.commit()
        await interaction.response.send_message(f"🧹 Стёрла всё, что помнила о тебе (фактов: {len(facts)}).",
                                                ephemeral=True)
        return

    # what.value == "fact" — удаляем один факт по номеру из /memory
    if number is None or not 1 <= number <= len(facts):
        await interaction.response.send_message(
            "Укажи номер факта в поле `number` — номера видно в /memory.", ephemeral=True)
        return
    fact_id, fact_text = facts[number - 1]  # номера в /memory начинаются с 1, а в списке — с 0
    db.execute("DELETE FROM ai_memory WHERE id = ?", (fact_id,))
    db.commit()
    await interaction.response.send_message(f"🗑️ Забыла: *{fact_text}*", ephemeral=True)


@bot.tree.command(name="weather", description="Погода сейчас и на 3 дня")
@app_commands.describe(city="Любой город (если не указать — города из /weather_cities)")
async def weather(interaction: discord.Interaction, city: str | None = None):
    await interaction.response.defer()  # запрос к сайту погоды может занять пару секунд

    if city:  # попросили конкретный город — ищем его
        try:
            found = await find_city(city)
        except Exception as error:
            await report_error("Поиск города /weather", error, interaction.guild, interaction.user, interaction.channel)
            found = None
        if found is None:
            await interaction.followup.send(f"Не нашёл город «{city}» 😕 Попробуй написать по-другому.")
            return
        cities = [found]
    elif interaction.guild:  # на сервере — список городов этого сервера
        cities = get_weather_cities(interaction.guild.id)
    else:                    # в личных сообщениях — города по умолчанию
        cities = DEFAULT_WEATHER_CITIES

    embed = discord.Embed(title="Прогноз погоды", color=0x8B5CF6)  # embed — красивая "карточка" с полями
    for name, latitude, longitude in cities:
        try:
            text = format_weather(await fetch_weather(latitude, longitude))
        except Exception as error:
            await report_error(f"Погода /weather ({name})", error, interaction.guild, interaction.user,
                               interaction.channel)
            text = "Не получилось узнать погоду 😕"
        embed.add_field(name=name, value=text, inline=False)  # inline=False — каждый город с новой строки
    embed.set_footer(text="Данные: Open-Meteo")
    await interaction.followup.send(embed=embed)


@bot.tree.command(name="weather_cities", description="Добавить, удалить или посмотреть города для /weather")
@app_commands.describe(action="Что сделать", city="Название города (для добавления и удаления)")
@app_commands.choices(action=[
    app_commands.Choice(name="➕ Добавить город", value="add"),
    app_commands.Choice(name="➖ Удалить город", value="remove"),
    app_commands.Choice(name="📋 Показать список", value="list"),
])
@app_commands.guild_only()
async def weather_cities(interaction: discord.Interaction, action: app_commands.Choice[str], city: str | None = None):
    guild_id = interaction.guild.id
    cities = list(get_weather_cities(guild_id))  # текущий список (копия, чтобы можно было менять)

    if action.value == "list":
        lines = [f"{i}. {name}" for i, (name, _, _) in enumerate(cities, 1)]
        await interaction.response.send_message("🗺️ Города для /weather:\n" + "\n".join(lines))
        return

    if not city:  # для добавления и удаления нужно название
        await interaction.response.send_message("Напиши название города в поле `city`.", ephemeral=True)
        return

    if action.value == "add":
        if len(cities) >= MAX_WEATHER_CITIES:
            await interaction.response.send_message(
                f"Уже {MAX_WEATHER_CITIES} городов — сначала удали какой-нибудь.", ephemeral=True)
            return
        await interaction.response.defer()  # поиск города занимает пару секунд
        try:
            found = await find_city(city)
        except Exception as error:
            await report_error("Поиск города /weather_cities", error, interaction.guild, interaction.user,
                               interaction.channel)
            found = None
        if found is None:
            await interaction.followup.send(f"Не нашёл город «{city}» 😕 Попробуй написать по-другому.")
            return
        if any(name == found[0] for name, _, _ in cities):  # any — "есть ли хоть один такой"
            await interaction.followup.send(f"{found[0]} уже есть в списке.")
            return
        cities.append(found)
        save_weather_cities(guild_id, cities)
        await interaction.followup.send(f"✅ Добавил {found[0]}. Теперь в /weather городов: {len(cities)}.")
        return

    # action.value == "remove": ищем город, в названии которого есть введённый текст (без учёта регистра)
    matches = [item for item in cities if city.lower() in item[0].lower()]
    if not matches:
        await interaction.response.send_message(f"Города «{city}» нет в списке. Посмотри: /weather_cities → Показать список.",
                                                ephemeral=True)
        return
    removed = matches[0]
    cities.remove(removed)
    save_weather_cities(guild_id, cities)
    text = f"🗑️ Удалил {removed[0]}."
    if not cities:
        text += " Список пуст, поэтому /weather снова покажет Москву и Тель-Авив."
    await interaction.response.send_message(text)


@bot.tree.command(name="dice", description="Кинуть игральные кости")
@app_commands.describe(count="Сколько костей кинуть (от 1 до 10)")
async def dice(interaction: discord.Interaction, count: app_commands.Range[int, 1, 10] = 2):
    # count: Range[int, 1, 10] = 2 — число от 1 до 10, если не указать — 2 кости
    rolls = [random.randint(1, 6) for _ in range(count)]  # бросаем count раз число от 1 до 6
    faces = " ".join(DICE_FACES[roll - 1] for roll in rolls)  # превращаем числа в значки граней

    text = f"🎲 {interaction.user.mention} кидает {count} {plural(count, 'кость', 'кости', 'костей')}:\n# {faces}\n"
    if count == 1:
        text += f"Выпало **{rolls[0]}**"
    else:
        text += f"{' + '.join(str(roll) for roll in rolls)} = **{sum(rolls)}**"  # например "3 + 5 = 8"
        if len(set(rolls)) == 1:  # set убирает повторы: если остался один элемент — все кости одинаковые
            text += "\n🔥 Дубль!"
    await interaction.response.send_message(text)


@bot.tree.command(name="roll", description="Случайное число от 1 до 100")
async def roll(interaction: discord.Interaction):
    number_rolled = random.randint(1, 100)  # случайное целое число от 1 до 100 включительно
    await interaction.response.send_message(f"🎲 {interaction.user.mention} выбрасывает **{number_rolled}** (1–100)")


@bot.tree.command(name="divination", description="Шуточное предсказание")
@app_commands.describe(kind="Какое предсказание нужно", question="Вопрос для магического шара (необязательно)")
@app_commands.choices(kind=[  # варианты, из которых выбирают в Discord
    app_commands.Choice(name="🔮 На день", value="day"),
    app_commands.Choice(name="💘 Любовь", value="love"),
    app_commands.Choice(name="📚 Учёба", value="study"),
    app_commands.Choice(name="💰 Деньги", value="money"),
    app_commands.Choice(name="🎱 Магический шар (да/нет)", value="ball"),
])
async def divination(interaction: discord.Interaction, kind: app_commands.Choice[str], question: str | None = None):
    title, variants = PREDICTIONS[kind.value]  # kind.value — "day", "love" и т. д.
    answer = random.choice(variants)           # случайное предсказание из списка

    text = f"{title} для {interaction.user.mention}\n"
    if question:  # если задали вопрос — показываем его
        text += f"❓ *{question}*\n"
    text += f"> {answer}"  # "> " — в Discord это оформление цитатой
    await interaction.response.send_message(text)


@bot.tree.command(name="pogonyalo", description="Погоняло на сегодня")
@app_commands.describe(member="Кому выдать погоняло (если не выбрать — тебе)")
async def pogonyalo(interaction: discord.Interaction, member: discord.Member | None = None):
    member = member or interaction.user
    await interaction.response.send_message(
        f"🏷️ Погоняло {member.mention} на сегодня: **{daily_nickname(member.id)}**"
    )


@bot.tree.command(name="dick_growth",
                  description=f"Изменить размер (раз в {hours_text(GROWTH_COOLDOWN)}, от {GROWTH_MIN:+d} до {GROWTH_MAX:+d} см)")
@app_commands.guild_only()
async def dick_growth(interaction: discord.Interaction):
    guild_id = interaction.guild.id
    user = interaction.user
    size, last_growth = get_dick(guild_id, user.id)
    now = time.time()

    passed = now - last_growth  # сколько секунд прошло с прошлой попытки
    if passed < GROWTH_COOLDOWN:  # сутки ещё не прошли
        await interaction.response.send_message(
            f"⏳ Расти можно раз в {hours_text(GROWTH_COOLDOWN)}. Следующая попытка через **{time_left_text(GROWTH_COOLDOWN - passed)}**.\n"
            f"Сейчас у тебя **{size} см**.",
            ephemeral=True,  # это сообщение увидит только сам игрок
        )
        return

    change = random.randint(GROWTH_MIN, GROWTH_MAX)  # случайно от GROWTH_MIN до GROWTH_MAX (по умолчанию от -10 до +10)
    new_size = max(0, size + change)                 # max(0, ...) — меньше нуля не бывает

    db.execute("""
        INSERT INTO dick (guild_id, user_id, name, size, last_growth) VALUES (?, ?, ?, ?, ?)
        ON CONFLICT (guild_id, user_id) DO UPDATE SET
            name = excluded.name, size = excluded.size, last_growth = excluded.last_growth
    """, (guild_id, user.id, user.display_name, new_size, now))
    db.commit()

    if change > 0:
        text = f"📈 {user.mention}, твой член вырос на **{change} см**! Теперь он **{new_size} см**."
    elif change < 0 and new_size < size:
        text = f"📉 {user.mention}, твой член уменьшился на **{size - new_size} см**… Теперь он **{new_size} см**."
    elif change < 0:  # хотел уменьшиться, но уже 0
        text = f"📉 {user.mention}, твой член должен был уменьшиться на {-change} см, но меньше уже некуда. Сейчас **0 см**."
    else:
        text = f"😐 {user.mention}, твой член без изменений: всё те же **{new_size} см**."
    text += f"\nМесто в рейтинге: **#{get_dick_position(guild_id, new_size)}**. Следующая попытка — через {hours_text(GROWTH_COOLDOWN)}."
    await interaction.response.send_message(text)


@bot.tree.command(name="dick_size", description="Посмотреть размер")
@app_commands.describe(member="Чей размер посмотреть (если не выбрать — твой)")
@app_commands.guild_only()
async def dick_size(interaction: discord.Interaction, member: discord.Member | None = None):
    member = member or interaction.user
    size, last_growth = get_dick(interaction.guild.id, member.id)
    position = get_dick_position(interaction.guild.id, size)

    text = f"📏 Размер у {member.mention}: **{size} см** · место в рейтинге **#{position}**"
    if member == interaction.user:  # про время до следующей попытки пишем только самому игроку
        passed = time.time() - last_growth
        if passed >= GROWTH_COOLDOWN:
            text += "\n✅ Можно расти: /dick_growth"
        else:
            text += f"\n⏳ Следующая попытка через {time_left_text(GROWTH_COOLDOWN - passed)}"
    await interaction.response.send_message(text)


@bot.tree.command(name="dick_leaderboard", description="Таблица рейтинга (топ-10)")
@app_commands.guild_only()
async def dick_leaderboard(interaction: discord.Interaction):
    rows = db.execute(
        "SELECT user_id, name, size FROM dick WHERE guild_id = ? ORDER BY size DESC, name LIMIT 10",
        (interaction.guild.id,),
    ).fetchall()
    if not rows:
        await interaction.response.send_message("Пока никто не играл — начни с /dick_growth!")
        return

    await interaction.response.defer()
    players = []
    for user_id, name, size in rows:
        avatar = await get_avatar_by_id(interaction.guild, user_id)
        players.append({"name": name, "avatar": avatar, "size": size})

    loop = asyncio.get_running_loop()
    image = await loop.run_in_executor(None, make_dick_leaderboard, interaction.guild.name, players,
                                       pick_background("dick"))
    await interaction.followup.send(file=discord.File(image, filename="dick_leaderboard.png"))


# ---------- Экономика ----------

@bot.tree.command(name="work", description=f"Поработать и заработать монеты (раз в {hours_text(WORK_COOLDOWN)})")
@app_commands.guild_only()
async def work(interaction: discord.Interaction):
    balance, last_work = get_wallet(interaction.guild.id, interaction.user)
    passed = time.time() - last_work  # сколько секунд прошло с прошлой работы
    if passed < WORK_COOLDOWN:
        await interaction.response.send_message(
            f"⏳ Рано! Следующая смена через **{time_left_text(WORK_COOLDOWN - passed)}**. "
            f"Баланс: {money_text(balance)}.", ephemeral=True)
        return

    earned = random.randint(WORK_MIN, WORK_MAX)
    db.execute("UPDATE economy SET balance = balance + ?, last_work = ? WHERE guild_id = ? AND user_id = ?",
               (earned, time.time(), interaction.guild.id, interaction.user.id))
    db.commit()
    await interaction.response.send_message(
        f"💼 {random.choice(WORK_TEXTS)} — {interaction.user.mention} получает **+{money_text(earned)}**.\n"
        f"Баланс: **{money_text(balance + earned)}**. Следующая смена — через {hours_text(WORK_COOLDOWN)}.")


@bot.tree.command(name="balance", description="Посмотреть баланс монет")
@app_commands.describe(member="Чей баланс посмотреть (если не выбрать — твой)")
@app_commands.guild_only()
async def show_balance(interaction: discord.Interaction, member: discord.Member | None = None):
    member = member or interaction.user
    if member.bot:
        await interaction.response.send_message("У ботов кошельков нет 🙂", ephemeral=True)
        return
    balance, last_work = get_wallet(interaction.guild.id, member)
    text = (f"💰 Баланс {member.mention}: **{money_text(balance)}** · "
            f"место в рейтинге **#{money_position(interaction.guild.id, balance)}**")
    if member == interaction.user:  # про работу пишем только самому человеку
        passed = time.time() - last_work
        if passed >= WORK_COOLDOWN:
            text += "\n✅ Можно поработать: /work"
        else:
            text += f"\n⏳ Следующая смена через {time_left_text(WORK_COOLDOWN - passed)}"
    await interaction.response.send_message(text, allowed_mentions=NO_PINGS)


@bot.tree.command(name="give_money", description="Перевести монеты другому участнику")
@app_commands.describe(member="Кому перевести", amount="Сколько монет")
@app_commands.guild_only()
async def give_money(interaction: discord.Interaction, member: discord.Member,
                     amount: app_commands.Range[int, 1, 1_000_000_000]):
    if member.bot:
        await interaction.response.send_message("Ботам монеты ни к чему 🙂", ephemeral=True)
        return
    if member == interaction.user:
        await interaction.response.send_message("Самому себе перевести нельзя.", ephemeral=True)
        return
    balance, _ = get_wallet(interaction.guild.id, interaction.user)
    if amount > balance:
        await interaction.response.send_message(
            f"Не хватает монет: у тебя {money_text(balance)}.", ephemeral=True)
        return

    new_balance = add_money(interaction.guild.id, interaction.user, -amount)  # списываем у отправителя
    add_money(interaction.guild.id, member, amount)                            # и начисляем получателю
    # Получателя упоминаем с пингом — пусть узнает о переводе
    await interaction.response.send_message(
        f"💸 {interaction.user.mention} переводит {member.mention} **{money_text(amount)}**.\n"
        f"Остаток у отправителя: {money_text(new_balance)}.")


@bot.tree.command(name="money_leaderboard", description="Самые богатые участники (топ-10)")
@app_commands.guild_only()
async def money_leaderboard(interaction: discord.Interaction):
    rows = db.execute(
        "SELECT user_id, name, balance FROM economy WHERE guild_id = ? ORDER BY balance DESC, name LIMIT 10",
        (interaction.guild.id,),
    ).fetchall()
    if not rows:
        await interaction.response.send_message("Пока ни у кого нет монет — начни с /work!")
        return

    await interaction.response.defer()
    players = []
    for user_id, name, balance in rows:
        avatar = await get_avatar_by_id(interaction.guild, user_id)
        players.append({"name": name, "avatar": avatar, "balance": balance})

    loop = asyncio.get_running_loop()
    image = await loop.run_in_executor(None, make_money_leaderboard, interaction.guild.name, players,
                                       pick_background("money"))
    await interaction.followup.send(file=discord.File(image, filename="money_leaderboard.png"))


# ---------- Игры на монеты ----------

@bot.tree.command(name="coinflip", description="Подбросить монету на ставку (угадал — ставка удваивается)")
@app_commands.describe(bet="Сколько монет поставить", side="На что ставишь")
@app_commands.choices(side=[
    app_commands.Choice(name="🦅 Орёл", value="орёл"),
    app_commands.Choice(name="🪙 Решка", value="решка"),
])
@app_commands.guild_only()
async def coinflip(interaction: discord.Interaction, bet: app_commands.Range[int, MIN_BET, MAX_BET],
                   side: app_commands.Choice[str]):
    if not await take_bet(interaction, bet):
        return
    result = random.choice(["орёл", "решка"])
    shown = "Выпал **орёл** 🦅" if result == "орёл" else "Выпала **решка** 🪙"
    if result == side.value:
        new_balance = add_money(interaction.guild.id, interaction.user, bet * 2)  # возвращаем ставку и столько же сверху
        text = f"{shown}\n🎉 {interaction.user.mention} угадывает и выигрывает **+{money_text(bet)}**!"
    else:
        new_balance = get_wallet(interaction.guild.id, interaction.user)[0]
        text = f"{shown}\n😔 {interaction.user.mention} ставил(а) на «{side.value}» и проигрывает {money_text(bet)}."
    await interaction.response.send_message(f"{text}\nБаланс: {money_text(new_balance)}", allowed_mentions=NO_PINGS)


@bot.tree.command(name="slots", description="Сыграть в слоты на монеты")
@app_commands.describe(bet="Сколько монет поставить")
@app_commands.guild_only()
async def slots(interaction: discord.Interaction, bet: app_commands.Range[int, MIN_BET, MAX_BET]):
    if not await take_bet(interaction, bet):
        return
    reels = spin_slots()
    multiplier = slots_multiplier(reels)
    payout = bet * multiplier
    new_balance = add_money(interaction.guild.id, interaction.user, payout) if payout else \
        get_wallet(interaction.guild.id, interaction.user)[0]

    if multiplier > 1:
        outcome = f"🎉 Три {reels[0]}! Выигрыш ×{multiplier}: **+{money_text(payout - bet)}**"
    elif multiplier == 1:
        outcome = "😌 Два одинаковых — ставка возвращается."
    else:
        outcome = f"😔 Мимо. Проигрыш: {money_text(bet)}."
    paytable = " ".join(f"{symbol}×{SLOT_PAYOUTS[symbol]}" for symbol in SLOT_PAYOUTS)
    await interaction.response.send_message(
        f"🎰 {interaction.user.mention}\n# {' '.join(reels)}\n{outcome}\nБаланс: {money_text(new_balance)}\n"
        f"-# Три одинаковых: {paytable} · два одинаковых — возврат ставки",  # "-# " — мелкий серый текст
        allowed_mentions=NO_PINGS)


@bot.tree.command(name="blackjack", description="Сыграть в блэкджек на монеты (победа ×2, блэкджек ×2.5)")
@app_commands.describe(bet="Сколько монет поставить")
@app_commands.guild_only()
async def blackjack(interaction: discord.Interaction, bet: app_commands.Range[int, MIN_BET, MAX_BET]):
    if interaction.user.id in blackjack_players:
        await interaction.response.send_message("Сначала доиграй текущую партию 🙂", ephemeral=True)
        return
    blackjack_players.add(interaction.user.id)  # занимаем место сразу, чтобы нельзя было начать две партии
    if not await take_bet(interaction, bet):
        blackjack_players.discard(interaction.user.id)
        return

    game = BlackjackGame(interaction.guild.id, interaction.user, bet)
    # Если у кого-то сразу 21 (блэкджек), партия заканчивается сразу
    if hand_value(game.hand) == 21 or hand_value(game.dealer) == 21:
        game.finish()
        await interaction.response.send_message(embed=game.embed())
        return
    await interaction.response.send_message(embed=game.embed(), view=game)
    game.message = await interaction.original_response()  # запоминаем сообщение (для таймаута)


# ---------- Утренние и вечерние сообщения ----------

@bot.tree.command(name="daily_setup", description="Включить утренние и вечерние сообщения")
@app_commands.describe(
    channel="В какой канал писать",
    timezone="Часовой пояс сервера",
    morning="Время утреннего сообщения, например 08:00",
    evening="Время вечернего сообщения, например 22:00",
)
@app_commands.choices(timezone=[
    app_commands.Choice(name=title, value=code) for code, (title, _) in DAILY_TIMEZONES.items()
])
@app_commands.default_permissions(manage_guild=True)  # команду видят только те, кто может управлять сервером
@app_commands.guild_only()
async def daily_setup(interaction: discord.Interaction, channel: discord.TextChannel,
                      timezone: app_commands.Choice[str], morning: str = "08:00", evening: str = "22:00"):
    # Проверяем и приводим время к виду "08:00"
    times = []
    for value in (morning, evening):
        match = re.fullmatch(r"(\d{1,2})[:.](\d{2})", value.strip())  # часы:минуты (можно через точку)
        if not match or int(match.group(1)) > 23 or int(match.group(2)) > 59:
            await interaction.response.send_message(f"Не понял время «{value}». Пиши так: 08:00", ephemeral=True)
            return
        times.append(f"{int(match.group(1)):02d}:{match.group(2)}")
    morning, evening = times

    # Может ли бот писать в этот канал
    permissions = channel.permissions_for(interaction.guild.me)
    if not (permissions.send_messages and permissions.embed_links):
        await interaction.response.send_message(
            f"Мне нельзя писать в {channel.mention}: нужны права «Отправлять сообщения» и «Встраивать ссылки».",
            ephemeral=True)
        return

    # Если сегодня это время уже прошло — первое сообщение будет завтра (отмечаем как будто уже отправили)
    now = datetime.datetime.now(zone(timezone.value))
    today = now.date().isoformat()
    def passed(at):
        hour, minute = map(int, at.split(":"))
        return now >= now.replace(hour=hour, minute=minute, second=0, microsecond=0)

    db.execute("""
        INSERT INTO daily_settings (guild_id, channel_id, timezone, morning, evening, last_morning, last_evening)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT (guild_id) DO UPDATE SET
            channel_id = excluded.channel_id, timezone = excluded.timezone,
            morning = excluded.morning, evening = excluded.evening,
            last_morning = excluded.last_morning, last_evening = excluded.last_evening
    """, (interaction.guild.id, channel.id, timezone.value, morning, evening,
          today if passed(morning) else "", today if passed(evening) else ""))
    db.commit()
    await interaction.response.send_message(
        f"✅ Готово! Буду писать в {channel.mention}: утром в **{morning}** и вечером в **{evening}** "
        f"({timezone.name}).\nПроверить прямо сейчас: /daily_test")


@bot.tree.command(name="daily_off", description="Выключить утренние и вечерние сообщения")
@app_commands.default_permissions(manage_guild=True)
@app_commands.guild_only()
async def daily_off(interaction: discord.Interaction):
    cursor = db.execute("DELETE FROM daily_settings WHERE guild_id = ?", (interaction.guild.id,))
    db.commit()
    if cursor.rowcount:
        await interaction.response.send_message("🔕 Утренние и вечерние сообщения выключены.")
    else:
        await interaction.response.send_message("Они и так выключены.", ephemeral=True)


@bot.tree.command(name="daily_test", description="Прислать утреннее или вечернее сообщение прямо сейчас")
@app_commands.describe(kind="Какое сообщение прислать")
@app_commands.choices(kind=[
    app_commands.Choice(name="☀️ Утреннее", value="morning"),
    app_commands.Choice(name="🌙 Вечернее", value="evening"),
])
@app_commands.default_permissions(manage_guild=True)
@app_commands.guild_only()
async def daily_test(interaction: discord.Interaction, kind: app_commands.Choice[str]):
    row = db.execute("SELECT channel_id FROM daily_settings WHERE guild_id = ?", (interaction.guild.id,)).fetchone()
    channel_id = row[0] if row else interaction.channel_id  # не настроено — пишем в текущий канал
    await interaction.response.defer(ephemeral=True)       # ИИ пишет текст несколько секунд
    if await send_daily(channel_id, kind.value):
        await interaction.followup.send("✅ Отправила.", ephemeral=True)
    else:
        await interaction.followup.send("Не получилось отправить — подробности в консоли бота.", ephemeral=True)


@bot.tree.command(name="games_channel", description="Канал для рангов, рейтингов и игр (пусто — работают везде)")
@app_commands.describe(channel="Канал для игр (если не выбрать — команды снова будут работать в любом канале)")
@app_commands.default_permissions(manage_guild=True)  # команду видят только те, кто может управлять сервером
@app_commands.guild_only()
async def set_games_channel(interaction: discord.Interaction, channel: discord.TextChannel | None = None):
    if channel is None:  # канал не выбрали — снимаем ограничение
        db.execute("DELETE FROM games_channel WHERE guild_id = ?", (interaction.guild.id,))
        db.commit()
        await interaction.response.send_message("✅ Ранги, рейтинги и игры снова работают в любом канале.")
        return

    permissions = channel.permissions_for(interaction.guild.me)  # что боту можно в этом канале
    if not (permissions.send_messages and permissions.attach_files and permissions.embed_links):
        await interaction.response.send_message(
            f"Мне не хватает прав в {channel.mention}: нужны «Отправлять сообщения», «Прикреплять файлы» "
            f"и «Встраивать ссылки».", ephemeral=True)
        return

    db.execute("""
        INSERT INTO games_channel (guild_id, channel_id) VALUES (?, ?)
        ON CONFLICT (guild_id) DO UPDATE SET channel_id = excluded.channel_id
    """, (interaction.guild.id, channel.id))
    db.commit()
    command_list = ", ".join(f"/{name}" for name in sorted(GAMES_COMMANDS))
    await interaction.response.send_message(
        f"✅ Теперь эти команды работают только в {channel.mention}:\n{command_list}\n"
        f"Чтобы снова разрешить их везде — вызови /games_channel без канала.")


@bot.tree.command(name="log_channel", description="Канал, куда бот присылает свои ошибки (пусто — не присылать)")
@app_commands.describe(channel="Канал для логов (лучше закрытый, только для админов). Пусто — выключить")
@app_commands.default_permissions(manage_guild=True)  # команду видят только те, кто может управлять сервером
@app_commands.guild_only()
async def set_log_channel(interaction: discord.Interaction, channel: discord.TextChannel | None = None):
    if channel is None:  # канал не выбрали — выключаем логи в Discord (в bot.log они пишутся всегда)
        db.execute("DELETE FROM log_channel WHERE guild_id = ?", (interaction.guild.id,))
        db.commit()
        await interaction.response.send_message("✅ Больше не присылаю ошибки в Discord (в bot.log пишу как раньше).",
                                                ephemeral=True)
        return

    permissions = channel.permissions_for(interaction.guild.me)  # что боту можно в этом канале
    if not (permissions.view_channel and permissions.send_messages and permissions.embed_links):
        await interaction.response.send_message(
            f"Мне не хватает прав в {channel.mention}: нужны «Просматривать канал», «Отправлять сообщения» "
            f"и «Встраивать ссылки».", ephemeral=True)
        return

    db.execute("""
        INSERT INTO log_channel (guild_id, channel_id) VALUES (?, ?)
        ON CONFLICT (guild_id) DO UPDATE SET channel_id = excluded.channel_id
    """, (interaction.guild.id, channel.id))
    db.commit()
    await interaction.response.send_message(
        f"✅ Теперь присылаю ошибки в {channel.mention}. Выключить — /log_channel без канала.", ephemeral=True)


@bot.tree.command(name="leave", description="Выйти из канала (очередь сохранится)")
@app_commands.guild_only()
async def leave(interaction: discord.Interaction):
    vc = interaction.guild.voice_client
    if vc is None:
        await interaction.response.send_message("Я и так не в канале.", ephemeral=True)
        return

    await interaction.response.defer()  # выход из канала может занять пару секунд
    current = now_playing.pop(interaction.guild.id, None)  # что играло в момент выхода
    if current:
        queue_push_front(interaction.guild.id, *current)   # возвращаем его в начало очереди — сыграет первым
    await vc.disconnect()

    saved = queue_count(interaction.guild.id)
    text = "👋 Вышел из канала."
    if saved:
        text += f" Очередь сохранена ({saved} {plural(saved, 'трек', 'трека', 'треков')}) — продолжить: /resume"
    await interaction.followup.send(text)


# Запуск бота. Эта строка "держит" программу работающей, пока её не остановят (Ctrl+C).
bot.run(TOKEN)
