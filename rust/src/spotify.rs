//! Ссылки Spotify: бот узнаёт названия песен, а сами песни ищет на YouTube.
//! Треки и альбомы — через Spotify API (если заданы SPOTIFY_CLIENT_ID и SPOTIFY_CLIENT_SECRET),
//! плейлисты и всё остальное — через публичную страницу плеера Spotify (embed): API чужие плейлисты не отдаёт.

use serde_json::Value;

pub const MAX_ALBUM_TRACKS: usize = 20; // сколько треков максимум брать из одного альбома
pub const MAX_PLAYLIST_TRACKS: usize = 100; // и из одного плейлиста

/// Что за ссылка Spotify.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Track,
    Album,
    Playlist,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
        }
    }
}

/// «https://open.spotify.com/intl-ru/track/4cOd…?si=…» → (Track, "4cOd…"). Не ссылка Spotify — None.
pub fn parse_link(text: &str) -> Option<(Kind, String)> {
    let rest = &text[text.find("open.spotify.com/")? + "open.spotify.com/".len()..];
    let rest = match rest.strip_prefix("intl-") {
        Some(after) => after.split_once('/')?.1, // «intl-ru/» — язык страницы, пропускаем
        None => rest,
    };
    let (kind, rest) = rest.split_once('/')?;
    let kind = match kind {
        "track" => Kind::Track,
        "album" => Kind::Album,
        "playlist" => Kind::Playlist,
        _ => return None,
    };
    let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    (!id.is_empty()).then_some((kind, id))
}

/// Ответ: строки «Исполнитель - Название» для поиска на YouTube и название трека/альбома/плейлиста.
pub type Found = (Vec<String>, String);

/// Через Spotify API (только треки и альбомы).
pub async fn api_lookup(web: &reqwest::Client, client_id: &str, secret: &str, kind: Kind, id: &str) -> Result<Found, crate::Error> {
    let token: Value = web
        .post("https://accounts.spotify.com/api/token")
        .basic_auth(client_id, Some(secret))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await?
        .error_for_status()?
        .json_value()
        .await?;
    let token = token["access_token"].as_str().ok_or("Spotify не дал токен")?;
    let info: Value = web
        .get(format!("https://api.spotify.com/v1/{}s/{id}", kind.path()))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json_value()
        .await?;
    api_tracks(kind, &info).ok_or_else(|| "непонятный ответ Spotify API".into())
}

/// Разбирает ответ Spotify API.
pub fn api_tracks(kind: Kind, info: &Value) -> Option<Found> {
    let name = info["name"].as_str()?.to_string();
    let line = |track: &Value| Some(format!("{} - {}", track["artists"][0]["name"].as_str()?, track["name"].as_str()?));
    match kind {
        Kind::Track => Some((vec![line(info)?], name)),
        _ => {
            let tracks = info["tracks"]["items"].as_array()?;
            Some((tracks.iter().take(MAX_ALBUM_TRACKS).filter_map(line).collect(), name))
        }
    }
}

/// Через страницу плеера Spotify — без ключей. Так работают и плейлисты.
pub async fn embed_lookup(web: &reqwest::Client, kind: Kind, id: &str) -> Result<Found, crate::Error> {
    let html = web
        .get(format!("https://open.spotify.com/embed/{}/{id}", kind.path()))
        .header("User-Agent", "Mozilla/5.0") // без этого Spotify может отдать пустую страницу
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    embed_tracks(kind, &html)
}

/// Достаёт треки из страницы плеера: данные лежат в <script id="__NEXT_DATA__"> в виде JSON.
pub fn embed_tracks(kind: Kind, html: &str) -> Result<Found, crate::Error> {
    let marker = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let start = html.find(marker).ok_or("на странице Spotify нет данных плеера")? + marker.len();
    let end = start + html[start..].find("</script>").ok_or("на странице Spotify нет данных плеера")?;
    let data: Value = serde_json::from_str(&html[start..end])?;
    let entity = &data["props"]["pageProps"]["state"]["data"]["entity"];
    let name = entity["name"].as_str().or(entity["title"].as_str()).unwrap_or("Spotify").to_string();
    let tidy = |text: String| text.replace('\u{a0}', " ").trim_matches([' ', '-']).to_string(); // Spotify разделяет исполнителей неразрывным пробелом
    if kind == Kind::Track {
        let artist = match entity["subtitle"].as_str() {
            Some(artist) => artist.to_string(),
            None => entity["artists"].as_array().map_or(vec![], |artists| artists.iter().filter_map(|a| a["name"].as_str()).collect()).join(", "),
        };
        return Ok((vec![tidy(format!("{artist} - {name}"))], name));
    }
    let limit = if kind == Kind::Playlist { MAX_PLAYLIST_TRACKS } else { MAX_ALBUM_TRACKS };
    let queries: Vec<String> = entity["trackList"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|track| track["isPlayable"].as_bool() != Some(false))
        .take(limit)
        .filter_map(|track| {
            let title = track["title"].as_str().filter(|title| !title.is_empty())?;
            Some(tidy(format!("{} - {title}", track["subtitle"].as_str().unwrap_or(""))))
        })
        .collect();
    if queries.is_empty() {
        return Err("в плейлисте/альбоме нет доступных треков".into());
    }
    Ok((queries, name))
}

/// Небольшое удобство: ответ сервера → JSON.
trait JsonValue {
    async fn json_value(self) -> Result<Value, crate::Error>;
}

impl JsonValue for reqwest::Response {
    async fn json_value(self) -> Result<Value, crate::Error> {
        Ok(serde_json::from_str(&self.text().await?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Настоящая страница плеера Spotify (нужен интернет): cargo test -- --ignored
    #[tokio::test]
    #[ignore]
    async fn real_embed_page() {
        let (queries, name) = embed_lookup(&reqwest::Client::new(), Kind::Track, "4cOdK2wGLETKBW3PvgPWqT").await.unwrap();
        println!("{name}: {queries:?}");
        assert!(queries[0].to_lowercase().contains("never gonna"), "{queries:?}");
    }

    #[test]
    fn links() {
        assert_eq!(parse_link("https://open.spotify.com/intl-ru/track/4cOdK2wGLETKBW3PvgPWqT?si=abc"),
                   Some((Kind::Track, "4cOdK2wGLETKBW3PvgPWqT".into())));
        assert_eq!(parse_link("open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M"), Some((Kind::Playlist, "37i9dQZF1DXcBWIGoYBM5M".into())));
        assert_eq!(parse_link("https://open.spotify.com/album/1A2b"), Some((Kind::Album, "1A2b".into())));
        assert_eq!(parse_link("https://open.spotify.com/artist/1A2b"), None);
        assert_eq!(parse_link("dark days"), None);
    }

    #[test]
    fn api_answer() {
        let track = json!({"name": "Song", "artists": [{"name": "Band"}, {"name": "Other"}]});
        assert_eq!(api_tracks(Kind::Track, &track), Some((vec!["Band - Song".into()], "Song".into())));
        let items: Vec<Value> = (0..30).map(|i| json!({"name": format!("T{i}"), "artists": [{"name": "A"}]})).collect();
        let album = json!({"name": "Album", "tracks": {"items": items}});
        let (queries, name) = api_tracks(Kind::Album, &album).unwrap();
        assert_eq!((queries.len(), queries[0].as_str(), name.as_str()), (MAX_ALBUM_TRACKS, "A - T0", "Album"));
    }

    #[test]
    fn embed_page() {
        let entity = json!({"name": "Мой плейлист", "trackList": [
            {"title": "Песня", "subtitle": "Исполнитель\u{a0}Один, Два"},
            {"title": "Скрытая", "subtitle": "X", "isPlayable": false},
            {"title": "", "subtitle": "Без названия"},
            {"title": "Вторая"}
        ]});
        let page = format!(r#"<html><script id="__NEXT_DATA__" type="application/json">{}</script></html>"#,
                           json!({"props": {"pageProps": {"state": {"data": {"entity": entity}}}}}));
        let (queries, name) = embed_tracks(Kind::Playlist, &page).unwrap();
        assert_eq!(name, "Мой плейлист");
        assert_eq!(queries, ["Исполнитель Один, Два - Песня", "Вторая"]);
        let track = json!({"props": {"pageProps": {"state": {"data": {"entity": {"name": "Hit", "subtitle": "Star"}}}}}});
        let page = format!(r#"<script id="__NEXT_DATA__" type="application/json">{track}</script>"#);
        assert_eq!(embed_tracks(Kind::Track, &page).unwrap(), (vec!["Star - Hit".to_string()], "Hit".to_string()));
        assert!(embed_tracks(Kind::Album, "<html></html>").is_err());
    }
}
