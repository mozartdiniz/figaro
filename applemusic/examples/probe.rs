//! Live check of the API against a real account, without the GTK app.
//!
//! ```sh
//! FIGARO_TEAM_ID=… FIGARO_KEY_ID=… FIGARO_KEY_PATH=~/AuthKey_….p8 \
//! FIGARO_USER_TOKEN=… cargo run -p applemusic --example probe -- "search term"
//! ```
//!
//! Without `FIGARO_USER_TOKEN` it only runs a catalog search (storefront `us`),
//! which checks the developer token on its own. Without `FIGARO_KEY_PATH`
//! it uses the web player's token from music.apple.com.

use applemusic::{Client, DeveloperKey};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env = |k: &str| std::env::var(k).map_err(|_| format!("{k} not set"));
    let term = std::env::args().nth(1).unwrap_or_else(|| "mozart".into());
    let user = std::env::var("FIGARO_USER_TOKEN").unwrap_or_default();

    let mut client = match env("FIGARO_KEY_PATH") {
        Ok(path) => {
            let pem = std::fs::read_to_string(path)?;
            let key = DeveloperKey::from_pem(&env("FIGARO_TEAM_ID")?, &env("FIGARO_KEY_ID")?, &pem)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
            Client::new(&key.sign(now, 3600), &user, "us")
        }
        Err(_) => {
            let web = applemusic::fetch_web_token().await?;
            println!("web player token found, expires at {}", web.expires_at);
            Client::web(&web.token, &user, "us")
        }
    };
    if !user.is_empty() {
        let sf = client.fetch_storefront().await?;
        println!("storefront: {sf}");
        client.set_storefront(&sf);
    }

    let found = client.search_catalog(&term, 10).await?;
    println!("catalog search {term:?}: {} songs", found.len());
    for s in &found {
        println!("  {} — {} ({})", s.name, s.artist, s.id);
    }
    if user.is_empty() {
        return Ok(());
    }

    let songs = client.library_songs(|n| eprint!("\rlibrary songs: {n}")).await?;
    eprintln!();
    let matched = songs.iter().filter(|s| s.catalog_id.is_some()).count();
    let dated = songs.iter().filter(|s| s.date_added.is_some()).count();
    println!("library: {} songs, {matched} with catalog IDs, {dated} with dateAdded", songs.len());

    let playlists = client.library_playlists().await?;
    println!("playlists: {}", playlists.len());
    for p in playlists.iter().take(10) {
        let tracks = client.playlist_tracks(&p.id).await?;
        println!("  {} ({} tracks, editable: {})", p.name, tracks.len(), p.can_edit);
    }
    Ok(())
}
