//! One-off live test of the write endpoints. **Changes the account**:
//! creates a playlist "Figaro Test" (the API can't delete it; remove it in
//! Music.app), adds two library songs to it, and adds one catalog song to
//! the library.
//!
//! ```sh
//! FIGARO_USER_TOKEN=… cargo run -p applemusic --example writetest
//! ```

use applemusic::{Client, Source};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let user = std::env::var("FIGARO_USER_TOKEN").map_err(|_| "FIGARO_USER_TOKEN not set")?;
    let web = applemusic::fetch_web_token().await?;
    let mut client = Client::web(&web.token, &user, "us");
    let sf = client.fetch_storefront().await?;
    client.set_storefront(&sf);

    let songs = client.library_songs(|_| {}).await?;
    let [first, second, ..] = songs.as_slice() else { return Err("need two library songs".into()) };

    // Create with one track, then append one: both write paths.
    let playlist = client
        .create_playlist("Figaro Test", Some("Created by Figaro's write test"), &[first.track_ref()])
        .await?;
    println!("created playlist {:?} ({}), editable: {}", playlist.name, playlist.id, playlist.can_edit);
    client.add_to_playlist(&playlist.id, &[second.track_ref()]).await?;
    println!("added {:?} to it", second.name);

    // Writes may take a moment to show up.
    for attempt in 1..=5 {
        let tracks = client.playlist_tracks(&playlist.id).await?;
        println!(
            "read back (try {attempt}): {} tracks: {:?}",
            tracks.len(),
            tracks.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        if tracks.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }

    // A catalog song that isn't in the library yet.
    let owned: std::collections::HashSet<_> = songs.iter().filter_map(|s| s.catalog_id.clone()).collect();
    let found = client.search_catalog("mozart requiem lacrimosa", 25).await?;
    let pick = found
        .iter()
        .find(|s| s.source == Source::Catalog && !owned.contains(&s.id))
        .ok_or("nothing new to add")?;
    client.add_to_library(&[pick.id.as_str()]).await?;
    println!("added to library: {:?} — {} ({})", pick.name, pick.artist, pick.id);

    for attempt in 1..=5 {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let now = client.library_songs(|_| {}).await?;
        let there = now.iter().any(|s| s.catalog_id.as_deref() == Some(pick.id.as_str()));
        println!("library check (try {attempt}): {} songs, added song present: {there}", now.len());
        if there {
            break;
        }
    }
    Ok(())
}
