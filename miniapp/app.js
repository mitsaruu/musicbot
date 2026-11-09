// Wortjagd (немецкие слова) и Словечко (русские слова) — мини-приложение Discord (Activity).
// Связь с Discord — по протоколу Embedded App SDK (сообщения postMessage окну Discord),
// ходы и проверка слов — на сервере бота (/api/...), поэтому загаданное слово страница не знает.
"use strict";

const LENGTH = 5;
const ROWS = 6;
// Всё, что отличается у двух игр: клавиатура и тексты
const GAMES = {
  de: {
    name: "Wortjagd", logo: "W", sub: "Угадай немецкое слово из 5 букв", what: "немецкое",
    keys: [
      ["q", "w", "e", "r", "t", "z", "u", "i", "o", "p", "ü"],
      ["a", "s", "d", "f", "g", "h", "j", "k", "l", "ö", "ä"],
      ["enter", "y", "x", "c", "v", "b", "n", "m", "back"],
    ],
  },
  ru: {
    name: "Словечко", logo: "С", sub: "Угадай русское слово из 5 букв", what: "русское",
    keys: [
      ["й", "ц", "у", "к", "е", "н", "г", "ш", "щ", "з", "х", "ъ"],
      ["ф", "ы", "в", "а", "п", "р", "о", "л", "д", "ж", "э"],
      ["enter", "я", "ч", "с", "м", "и", "т", "ь", "б", "ю", "back"],
    ],
  },
};
let lang = null;            // какая игра открыта: de или ru
let LETTERS = new Set();    // буквы этой игры

const params = new URLSearchParams(location.search);
const boardEl = document.getElementById("board");
const keyboardEl = document.getElementById("keyboard");
const statusEl = document.getElementById("status");

let session = null;   // пропуск от сервера бота
let state = null;     // состояние партии с сервера
let typed = "";       // что игрок сейчас набирает
let busy = false;     // ждём ответа сервера

// ---------- Связь с Discord (минимальный Embedded App SDK) ----------
const OP = { HANDSHAKE: 0, FRAME: 1, CLOSE: 2, HELLO: 3 };
const discordWindow = window.parent.opener || window.parent;
const discordOrigin = document.referrer || "*";
const pending = new Map();
let onReady = null;

window.addEventListener("message", (event) => {
  if (event.source !== discordWindow || !Array.isArray(event.data)) return;
  const [opcode, data] = event.data;
  if (opcode === OP.CLOSE) {
    setStatus("Discord закрыл соединение: " + (data && data.message ? data.message : ""), "error");
    return;
  }
  if (opcode !== OP.FRAME || !data) return;
  if (data.cmd === "DISPATCH") {
    if (data.evt === "READY" && onReady) onReady();
    return;
  }
  const waiter = pending.get(data.nonce);
  if (!waiter) return;
  pending.delete(data.nonce);
  if (data.evt === "ERROR") waiter.reject(new Error((data.data && data.data.message) || "ошибка Discord"));
  else waiter.resolve(data.data);
});

function discordCommand(cmd, args) {
  const nonce = crypto.randomUUID ? crypto.randomUUID() : String(Math.random()).slice(2);
  return new Promise((resolve, reject) => {
    pending.set(nonce, { resolve, reject });
    discordWindow.postMessage([OP.FRAME, { cmd, args, nonce }], discordOrigin);
  });
}

function connectToDiscord(clientId) {
  return new Promise((resolve, reject) => {
    onReady = resolve;
    discordWindow.postMessage([OP.HANDSHAKE, {
      v: 1, encoding: "json", client_id: clientId, frame_id: params.get("frame_id"),
    }], discordOrigin);
    setTimeout(() => reject(new Error("Discord не ответил")), 15000);
  });
}

// ---------- Запросы к серверу бота ----------
async function api(path, body) {
  const options = { headers: { "Content-Type": "application/json" } };
  if (session) options.headers.Authorization = "Bearer " + session;
  if (body !== undefined) {
    options.method = "POST";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(path, options);
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error || "Сервер бота не ответил (" + response.status + ")");
  return data;
}

// ---------- Экран загрузки ----------
function loading(text, percent) {
  document.getElementById("splash-text").textContent = text;
  document.getElementById("progress").style.width = percent + "%";
}

function loadingFailed(text) {
  const el = document.getElementById("splash-text");
  el.textContent = text;
  el.className = "splash-text error";
}

function hideSplash() {
  document.getElementById("progress").style.width = "100%";
  setTimeout(() => document.getElementById("splash").classList.add("hide"), 350);
}

// ---------- Отрисовка ----------
function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = "status" + (kind ? " " + kind : "");
}

function buildBoard() {
  boardEl.innerHTML = "";
  for (let r = 0; r < ROWS; r++) {
    const row = document.createElement("div");
    row.className = "row";
    for (let c = 0; c < LENGTH; c++) {
      const tile = document.createElement("div");
      tile.className = "tile";
      row.appendChild(tile);
    }
    boardEl.appendChild(row);
  }
}

function setLanguage(newLang) {
  if (newLang === lang || !GAMES[newLang]) return;
  lang = newLang;
  const game = GAMES[lang];
  LETTERS = new Set(game.keys.flat().filter((key) => key.length === 1));
  document.title = game.name;
  document.documentElement.lang = lang;
  document.getElementById("game-logo").textContent = game.logo;
  document.getElementById("game-name").textContent = game.name;
  document.getElementById("game-sub").textContent = game.sub;
  document.getElementById("help-what").textContent = game.what;
  for (const button of document.querySelectorAll(".lang")) {
    button.classList.toggle("active", button.dataset.lang === lang);
  }
  buildKeyboard();
}

function buildKeyboard() {
  keyboardEl.innerHTML = "";
  for (const keys of GAMES[lang].keys) {
    const row = document.createElement("div");
    row.className = "krow";
    for (const key of keys) {
      const button = document.createElement("button");
      button.className = "key" + (key.length > 1 ? " wide" : "");
      button.dataset.key = key;
      button.textContent = key === "enter" ? "Ввод" : key === "back" ? "⌫" : key;
      button.addEventListener("click", () => press(key));
      row.appendChild(button);
    }
    keyboardEl.appendChild(row);
  }
}

function render(animateRow) {
  const rows = boardEl.children;
  for (let r = 0; r < ROWS; r++) {
    const tiles = rows[r].children;
    const guess = state && state.rows[r];
    for (let c = 0; c < LENGTH; c++) {
      const tile = tiles[c];
      let letter = "", cls = "tile";
      if (guess) {
        letter = guess.word[c];
        cls += " " + guess.marks[c];
      } else if (state && r === state.rows.length) {
        letter = typed[c] || "";
        if (letter) cls += " filled";
      }
      tile.textContent = letter;
      tile.className = cls;
      tile.style.animationDelay = r === animateRow ? c * 0.12 + "s" : "0s";
      if (guess && r !== animateRow) tile.style.animation = "none";
      else tile.style.animation = "";
    }
  }
  for (const button of keyboardEl.querySelectorAll(".key")) {
    const mark = state && state.letters[button.dataset.key];
    button.className = "key" + (button.dataset.key.length > 1 ? " wide" : "") + (mark ? " " + mark : "");
  }
  document.getElementById("mode-daily").classList.toggle("active", !!(state && state.daily));
  document.getElementById("mode-practice").classList.toggle("active", !!(state && !state.daily));
  if (state) {
    document.getElementById("wallet").hidden = false;
    document.getElementById("balance").textContent = state.balance.toLocaleString("ru-RU");
    document.getElementById("rewards-text").textContent =
      "Слово дня одно на весь сервер, сыграть можно раз в день. Награда за победу с 1-й…" + state.max_guesses +
      "-й попытки: " + state.rewards.join(" / ") + " монет. Тренировка — без награды и без ограничений.";
  }
}

function showGameStatus() {
  if (!state) return;
  if (state.won) {
    const tries = state.rows.length;
    setStatus("🎉 " + state.answer.toUpperCase() + " — угадано с " + tries + "-й попытки" +
      (state.reward ? "! +" + state.reward + " монет" : "!"), "win");
  } else if (state.finished) {
    setStatus("😿 Попытки кончились. Было слово " + state.answer.toUpperCase() +
      (state.daily ? ". Новое слово — завтра, а пока можно потренироваться." : ""), "error");
  } else {
    const left = state.max_guesses - state.rows.length;
    setStatus((state.daily ? "Слово дня. " : "Тренировка. ") + "Осталось попыток: " + left);
  }
}

function shake() {
  const row = boardEl.children[state.rows.length];
  if (!row) return;
  row.classList.remove("shake");
  void row.offsetWidth;
  row.classList.add("shake");
}

// ---------- Ввод ----------
async function press(key) {
  if (!state || busy || state.finished) return;
  if (key === "back") {
    typed = typed.slice(0, -1);
  } else if (key === "enter") {
    if (typed.length < LENGTH) {
      setStatus("Нужно " + LENGTH + " букв", "error");
      shake();
      return;
    }
    busy = true;
    try {
      const data = await api("api/guess", { word: typed });
      const before = state.rows.length;
      state = data.state;
      if (data.error) {
        setStatus(data.error, "error");
        shake();
      } else {
        typed = "";
        render(before);
        showGameStatus();
      }
    } catch (error) {
      setStatus(error.message, "error");
    } finally {
      busy = false;
    }
    return;
  } else if (LETTERS.has(key) && typed.length < LENGTH) {
    typed += key;
  }
  render(-1);
}

window.addEventListener("keydown", (event) => {
  if (event.ctrlKey || event.metaKey || event.altKey) return;
  let key = event.key.toLowerCase();
  if (key === "ё") key = "е";  // в Словечке ё и е — одна буква
  if (key === "enter") press("enter");
  else if (key === "backspace") press("back");
  else if (LETTERS.has(key)) press(key);
  else return;
  event.preventDefault();
});

async function newGame(body) {
  if (busy || !session) return;
  busy = true;
  try {
    state = (await api("api/new", body)).state;
    setLanguage(state.lang);
    typed = "";
    render(-1);
    showGameStatus();
  } catch (error) {
    setStatus(error.message, "error");
  } finally {
    busy = false;
  }
}

for (const button of document.querySelectorAll(".mode")) {
  button.addEventListener("click", () => newGame({ mode: button.dataset.mode }));
}
for (const button of document.querySelectorAll(".lang")) {
  // другая игра — сразу её слово дня
  button.addEventListener("click", () => {
    if (button.dataset.lang !== lang) newGame({ mode: "daily", lang: button.dataset.lang });
  });
}

// ---------- Запуск ----------
async function start() {
  buildBoard();
  setLanguage("de");
  if (!params.get("frame_id")) {
    loadingFailed("Эта страница открывается внутри Discord: команда /wort, /slovo или кнопка запуска приложения.");
    return;
  }
  try {
    loading("Загружаем словарь…", 20);
    const config = await api("api/config");
    loading("Подключаемся к Discord…", 45);
    await connectToDiscord(config.client_id);
    loading("Входим через Discord…", 70);
    const { code } = await discordCommand("AUTHORIZE", {
      client_id: config.client_id, response_type: "code", state: "", prompt: "none", scope: ["identify"],
    });
    const login = await api("api/token", {
      code, guild_id: params.get("guild_id"), channel_id: params.get("channel_id"),
    });
    session = login.session;
    await discordCommand("AUTHENTICATE", { access_token: login.access_token }).catch(() => {});
    state = login.state;
    setLanguage(state.lang);
    render(-1);
    showGameStatus();
    hideSplash();
  } catch (error) {
    loadingFailed("Не получилось войти: " + error.message);
  }
}

start();
