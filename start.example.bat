@echo off
chcp 65001 >nul
rem Шаблон файла запуска. Скопируйте его, назовите копию start.bat и впишите свои ключи после "=".
rem Без пробелов вокруг "=" и без кавычек. Строки с ключами, которых нет, можно удалить.
set DISCORD_TOKEN=сюда_токен_бота
set SPOTIFY_CLIENT_ID=сюда_client_id
set SPOTIFY_CLIENT_SECRET=сюда_client_secret
set GEMINI_API_KEY=сюда_ключ_gemini
cd /d "%~dp0"
py bot.py
pause
