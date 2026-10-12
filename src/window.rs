//! Main window and controller.
//!
//! Layout: sidebar (Search, Songs, Playlists) | content stack, with the
//! player bar across the bottom.
//!
//! Ownership follows scrawl: the window's `destroy` handler holds the only
//! strong `Rc<Inner>`; every closure holds a `Weak`. Network calls run on
//! tokio through [`crate::bg`] and are awaited from GTK's main context, so
//! all UI updates stay on the main thread.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use applemusic::{Client, Playlist, Song};
use gtk::{gio, glib};

use crate::config::{Config, DevToken, LibraryCache};
use crate::playback::{Engine, Event};
use crate::player_bar::{Controls, PlayerBar, ARTWORK_SIZE};
use crate::secrets;
use crate::song_table::{Kind, Request, SongTable};

/// MusicKit fetches metadata for the whole queue up front; keep it bounded.
const QUEUE_MAX: usize = 300;
const SEARCH_RESULTS: usize = 50;

#[derive(Clone, PartialEq)]
enum Page {
    Search,
    Songs,
    Playlist(String),
}

struct Inner {
    window: glib::WeakRef<adw::ApplicationWindow>,
    toasts: adw::ToastOverlay,
    split: adw::NavigationSplitView,
    sidebar: gtk::ListBox,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    spinner: gtk::Spinner,
    signin_button: gtk::Button,
    signin_page: adw::StatusPage,
    setup_page: adw::StatusPage,
    search_entry: gtk::SearchEntry,
    search_table: SongTable,
    songs_table: SongTable,
    playlist_table: SongTable,
    /// "Add to Playlist" entries, shared by every table's context menu.
    playlists_menu: gio::Menu,
    player: PlayerBar,

    config: RefCell<Config>,
    developer_token: RefCell<Option<DevToken>>,
    /// Why there's no developer token (shown on the setup page).
    token_error: RefCell<Option<String>>,
    /// A 401 already triggered one token refresh this session.
    token_refreshed: Cell<bool>,
    user_token: RefCell<Option<String>>,
    /// Signed-in client; `None` until we have a Music User Token.
    client: RefCell<Option<Client>>,
    engine: RefCell<Option<Engine>>,
    engine_error: RefCell<Option<String>>,
    cache: RefCell<LibraryCache>,
    page: RefCell<Page>,
    loading: Cell<u32>,
    search_generation: Cell<u64>,
    artwork_generation: Cell<u64>,
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Figaro")
        .default_width(1180)
        .default_height(760)
        .width_request(720)
        .height_request(480)
        .build();

    let inner = Rc::new_cyclic(|weak: &Weak<Inner>| Inner::new(&window, weak.clone()));
    inner.install_actions(&window);

    // The destroy handler owns the controller (see module docs).
    let keep = RefCell::new(Some(inner.clone()));
    window.connect_destroy(move |_| {
        if let Some(inner) = keep.borrow_mut().take() {
            if let Some(engine) = inner.engine.borrow().as_ref() {
                engine.quit();
            }
        }
    });

    inner.start();
    window
}

impl Inner {
    fn new(window: &adw::ApplicationWindow, weak: Weak<Inner>) -> Inner {
        let playlists_menu = gio::Menu::new();
        let on_request: Rc<dyn Fn(Request)> = {
            let weak = weak.clone();
            Rc::new(move |req| {
                if let Some(inner) = weak.upgrade() {
                    inner.handle_request(req);
                }
            })
        };
        let search_table = SongTable::new(Kind::Search, &playlists_menu, on_request.clone());
        search_table.set_empty_text("Search Apple Music", "Find songs in the Apple Music catalog.");
        let songs_table = SongTable::new(Kind::Library, &playlists_menu, on_request.clone());
        songs_table.set_empty_text("No Songs", "Songs you add to your library appear here.");
        let playlist_table = SongTable::new(Kind::Playlist, &playlists_menu, on_request);
        playlist_table.set_empty_text("Empty Playlist", "Add songs from Songs or Search.");

        let player = PlayerBar::new(Controls {
            toggle: engine_action(&weak, Engine::toggle),
            next: engine_action(&weak, Engine::next),
            previous: engine_action(&weak, Engine::previous),
            seek: {
                let weak = weak.clone();
                Box::new(move |s| with_engine(&weak, |e| e.seek(s)))
            },
            volume: {
                let weak = weak.clone();
                Box::new(move |v| with_engine(&weak, |e| e.set_volume(v)))
            },
        });

        // ── Sidebar ─────────────────────────────────────────────────────
        let sidebar = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        sidebar.append(&sidebar_row("system-search-symbolic", "Search", "search"));
        sidebar.append(&sidebar_row("audio-x-generic-symbolic", "Songs", "songs"));
        let header = gtk::ListBoxRow::builder().selectable(false).activatable(false).build();
        header.set_child(Some(
            &gtk::Label::builder()
                .label("Playlists")
                .xalign(0.0)
                .margin_top(12)
                .css_classes(["heading", "dim-label"])
                .build(),
        ));
        sidebar.append(&header);

        let new_playlist = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New Playlist")
            .action_name("win.new-playlist")
            .build();
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Refresh Library"), Some("win.refresh"));
        section.append(Some("MusicKit Key…"), Some("win.key"));
        section.append(Some("Sign Out"), Some("win.sign-out"));
        menu.append_section(None, &section);
        let about = gio::Menu::new();
        about.append(Some("About Figaro"), Some("win.about"));
        menu.append_section(None, &about);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();
        let sidebar_header = adw::HeaderBar::new();
        sidebar_header.pack_start(&new_playlist);
        sidebar_header.pack_end(&menu_button);
        let sidebar_view = adw::ToolbarView::new();
        sidebar_view.add_top_bar(&sidebar_header);
        sidebar_view.set_content(Some(&gtk::ScrolledWindow::builder().child(&sidebar).vexpand(true).build()));
        let sidebar_page = adw::NavigationPage::new(&sidebar_view, "Figaro");

        // ── Content ─────────────────────────────────────────────────────
        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();

        let setup = adw::StatusPage::builder()
            .icon_name("network-offline-symbolic")
            .title("Can’t Reach Apple Music")
            .build();
        let retry = gtk::Button::builder()
            .label("Try Again")
            .action_name("win.retry")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let own_key = gtk::Button::builder()
            .label("Use My Own MusicKit Key…")
            .action_name("win.key")
            .halign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let setup_buttons = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        setup_buttons.append(&retry);
        setup_buttons.append(&own_key);
        setup.set_child(Some(&setup_buttons));
        stack.add_named(&setup, Some("setup"));

        let signin_page = adw::StatusPage::builder()
            .icon_name("avatar-default-symbolic")
            .title("Sign In to Apple Music")
            .description("Apple's sign-in window opens to connect your library and subscription.")
            .build();
        let signin_big = gtk::Button::builder()
            .label("Sign In")
            .action_name("win.sign-in")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        signin_page.set_child(Some(&signin_big));
        stack.add_named(&signin_page, Some("signin"));

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Artists, songs, albums")
            .search_delay(400)
            .hexpand(true)
            .build();
        let search_bar = adw::Clamp::builder()
            .maximum_size(520)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .child(&search_entry)
            .build();
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        search_box.append(&search_bar);
        search_box.append(search_table.widget());
        search_table.widget().set_vexpand(true);
        stack.add_named(&search_box, Some("search"));
        stack.add_named(songs_table.widget(), Some("songs"));
        stack.add_named(playlist_table.widget(), Some("playlist"));

        let title = adw::WindowTitle::new("Songs", "");
        let spinner = gtk::Spinner::new();
        let signin_button = gtk::Button::builder()
            .label("Sign In")
            .action_name("win.sign-in")
            .css_classes(["suggested-action"])
            .visible(false)
            .build();
        let content_header = adw::HeaderBar::builder().title_widget(&title).build();
        content_header.pack_end(&signin_button);
        content_header.pack_end(&spinner);
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&content_header);
        content_view.set_content(Some(&stack));
        let content_page = adw::NavigationPage::new(&content_view, "Songs");

        let split = adw::NavigationSplitView::builder()
            .sidebar(&sidebar_page)
            .content(&content_page)
            .vexpand(true)
            .build();

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.append(&split);
        main.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        main.append(player.widget());
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&main));
        window.set_content(Some(&toasts));

        // ── Signals ─────────────────────────────────────────────────────
        let w = weak.clone();
        sidebar.connect_row_selected(move |_, row| {
            let (Some(inner), Some(row)) = (w.upgrade(), row) else { return };
            let name = row.widget_name();
            let page = match name.as_str() {
                "search" => Page::Search,
                "songs" => Page::Songs,
                other => match other.strip_prefix("playlist:") {
                    Some(id) => Page::Playlist(id.to_owned()),
                    None => return,
                },
            };
            inner.show_page(page);
            inner.split.set_show_content(true);
        });

        let w = weak.clone();
        search_entry.connect_search_changed(move |entry| {
            if let Some(inner) = w.upgrade() {
                inner.search(entry.text().trim().to_owned());
            }
        });

        Inner {
            window: window.downgrade(),
            toasts,
            split,
            sidebar,
            stack,
            title,
            spinner,
            signin_button,
            signin_page,
            setup_page: setup,
            search_entry,
            search_table,
            songs_table,
            playlist_table,
            playlists_menu,
            player,
            config: RefCell::new(Config::load()),
            developer_token: RefCell::new(None),
            token_error: RefCell::new(None),
            token_refreshed: Cell::new(false),
            user_token: RefCell::new(None),
            client: RefCell::new(None),
            engine: RefCell::new(None),
            engine_error: RefCell::new(None),
            cache: RefCell::new(LibraryCache::default()),
            page: RefCell::new(Page::Songs),
            loading: Cell::new(0),
            search_generation: Cell::new(0),
            artwork_generation: Cell::new(0),
        }
    }

    fn install_actions(self: &Rc<Self>, window: &adw::ApplicationWindow) {
        let add = |name: &str, f: fn(&Rc<Inner>)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    f(&inner);
                }
            });
            window.add_action(&action);
        };
        add("toggle", |i| with_engine(&Rc::downgrade(i), Engine::toggle));
        add("next", |i| with_engine(&Rc::downgrade(i), Engine::next));
        add("previous", |i| with_engine(&Rc::downgrade(i), Engine::previous));
        add("refresh", |i| i.refresh_library());
        add("key", |i| i.show_key_dialog());
        add("retry", |i| i.start_session(true));
        add("sign-in", |i| i.sign_in());
        add("sign-out", |i| i.sign_out(false));
        add("new-playlist", |i| i.new_playlist_dialog(Vec::new()));
        add("about", |i| i.show_about());
        add("filter", |i| match &*i.page.borrow() {
            Page::Search => {
                i.search_entry.grab_focus();
            }
            Page::Songs => i.songs_table.focus_filter(),
            Page::Playlist(_) => i.playlist_table.focus_filter(),
        });
    }

    // ── Session ───────────────────────────────────────────────────────────

    fn start(self: &Rc<Self>) {
        let cache = LibraryCache::load();
        self.songs_table.set_songs(&cache.songs);
        *self.cache.borrow_mut() = cache;
        self.rebuild_playlists();
        self.select_sidebar("songs");
        self.start_session(false);
    }

    /// (Re)starts with the current config: get a developer token (own key
    /// or the web player's; `refresh` refetches the latter), start the
    /// engine, restore the saved sign-in.
    fn start_session(self: &Rc<Self>, refresh: bool) {
        let config = self.config.borrow().clone();
        self.begin_loading();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let token = config.developer_token(refresh).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.end_loading();
            let token = match token {
                Ok(t) => t,
                Err(e) => {
                    log::warn!("{e}");
                    *inner.developer_token.borrow_mut() = None;
                    *inner.token_error.borrow_mut() = Some(e.to_string());
                    inner.refresh_page();
                    return;
                }
            };
            *inner.token_error.borrow_mut() = None;
            *inner.developer_token.borrow_mut() = Some(token.clone());
            inner.ensure_engine();
            if let Some(engine) = inner.engine.borrow().as_ref() {
                engine.configure(token.as_str());
            }

            // FIGARO_USER_TOKEN: dev aid for running without the engine
            // (e.g. on macOS), with a token copied from music.apple.com.
            let env_token = std::env::var("FIGARO_USER_TOKEN").ok().filter(|t| !t.is_empty());
            let saved = match inner.user_token.borrow().clone().or(env_token) {
                Some(t) => Some(t),
                None => secrets::load_user_token().await,
            };
            let Some(inner) = weak.upgrade() else { return };
            match saved {
                Some(user_token) => inner.connect(user_token),
                // Wait for the engine to report whether MusicKit is signed in.
                None => inner.refresh_page(),
            }
        });
    }

    fn ensure_engine(self: &Rc<Self>) {
        if self.engine.borrow().is_some() {
            return;
        }
        let (tx, rx) = async_channel::unbounded();
        let spawned = {
            let _tokio = crate::runtime().enter();
            Engine::spawn(tx)
        };
        match spawned {
            Ok(engine) => {
                *self.engine.borrow_mut() = Some(engine);
                *self.engine_error.borrow_mut() = None;
            }
            Err(e) => {
                log::warn!("{e}");
                *self.engine_error.borrow_mut() = Some(e.to_string());
                self.toast(&format!("Playback unavailable: {e}"));
                return;
            }
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                inner.on_engine_event(event);
            }
        });
    }

    fn on_engine_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::Loaded => log::info!("MusicKit JS loaded"),
            Event::Authorized { user_token } => {
                self.signin_button.set_visible(false);
                if self.user_token.borrow().as_deref() != Some(user_token.as_str()) {
                    let t = user_token.clone();
                    glib::spawn_future_local(secrets::save_user_token(t));
                    self.connect(user_token);
                }
            }
            Event::NeedsAuth => {
                // Library browsing may still work from a saved token, but
                // MusicKit plays only previews until signed in.
                self.signin_button.set_visible(true);
                self.refresh_page();
            }
            Event::State { playing } => self.player.set_playing(playing),
            Event::NowPlaying { item } => {
                self.player.set_now_playing(item.as_ref());
                if let Some(url) = item.and_then(|i| i.artwork) {
                    self.load_artwork(url);
                }
            }
            Event::Time { position, duration } => self.player.set_time(position, duration),
            Event::Error { message } => self.toast(&message),
            Event::Exited => {
                *self.engine.borrow_mut() = None;
                self.player.set_playing(false);
                self.toast("The playback engine stopped");
            }
        }
    }

    /// Becomes signed in with `user_token` and refreshes the library.
    fn connect(self: &Rc<Self>, user_token: String) {
        let Some(dev) = self.developer_token.borrow().clone() else { return };
        let storefront = self.config.borrow().storefront.clone();
        *self.user_token.borrow_mut() = Some(user_token.clone());
        let mut client = dev.client(&user_token, storefront.as_deref().unwrap_or("us"));
        *self.client.borrow_mut() = Some(client.clone());
        self.refresh_page();

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if storefront.is_none() {
                let c = client.clone();
                match crate::bg(async move { c.fetch_storefront().await }).await {
                    Ok(sf) => {
                        let Some(inner) = weak.upgrade() else { return };
                        client.set_storefront(&sf);
                        *inner.client.borrow_mut() = Some(client);
                        inner.config.borrow_mut().storefront = Some(sf);
                        inner.config.borrow().save();
                    }
                    Err(e) => {
                        if let Some(inner) = weak.upgrade() {
                            inner.api_error(e);
                        }
                        return;
                    }
                }
            }
            if let Some(inner) = weak.upgrade() {
                inner.refresh_library();
            }
        });
    }

    fn sign_in(self: &Rc<Self>) {
        if self.developer_token.borrow().is_none() {
            self.start_session(true);
            return;
        }
        self.ensure_engine();
        match self.engine.borrow().as_ref() {
            Some(engine) => engine.authorize(),
            None => {
                let why = self.engine_error.borrow().clone().unwrap_or_default();
                self.toast(&format!("Can't sign in without the playback engine. {why}"));
            }
        }
    }

    /// Forgets the user. `expired`: Apple rejected the token, keep the cache.
    fn sign_out(self: &Rc<Self>, expired: bool) {
        *self.client.borrow_mut() = None;
        *self.user_token.borrow_mut() = None;
        glib::spawn_future_local(secrets::delete_user_token());
        if expired {
            self.toast("Your Apple Music session expired. Sign in again.");
        } else {
            if let Some(engine) = self.engine.borrow().as_ref() {
                engine.unauthorize();
            }
            LibraryCache::clear();
            *self.cache.borrow_mut() = LibraryCache::default();
            self.songs_table.set_songs(&[]);
            self.playlist_table.set_songs(&[]);
            self.rebuild_playlists();
            self.config.borrow_mut().storefront = None;
            self.config.borrow().save();
        }
        self.signin_button.set_visible(true);
        self.refresh_page();
    }

    fn api_error(self: &Rc<Self>, e: applemusic::Error) {
        log::warn!("{e}");
        let web = self.developer_token.borrow().as_ref().is_some_and(DevToken::is_web);
        match e {
            // Apple may have rotated the web player token: fetch it once
            // more before blaming the user's sign-in.
            applemusic::Error::Unauthorized(_) if web && !self.token_refreshed.get() => {
                self.token_refreshed.set(true);
                self.start_session(true);
            }
            applemusic::Error::Unauthorized(_) if self.client.borrow().is_some() => self.sign_out(true),
            applemusic::Error::Unauthorized(_) if web => {
                self.toast("Apple Music rejected the web player token")
            }
            applemusic::Error::Unauthorized(_) => self.toast("Apple Music rejected the MusicKit key"),
            e => self.toast(&e.to_string()),
        }
    }

    // ── Library ───────────────────────────────────────────────────────────

    fn refresh_library(self: &Rc<Self>) {
        let Some(client) = self.client.borrow().clone() else {
            self.sign_in();
            return;
        };
        self.begin_loading();
        self.title.set_subtitle("Loading library…");
        let (progress_tx, progress_rx) = async_channel::unbounded::<usize>();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(n) = progress_rx.recv().await {
                match weak.upgrade() {
                    Some(inner) if inner.loading.get() > 0 => {
                        inner.title.set_subtitle(&format!("Loading library… {n} songs"))
                    }
                    _ => break,
                }
            }
        });

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let c = client.clone();
            let playlists = crate::bg(async move { c.library_playlists().await }).await;
            if let (Ok(playlists), Some(inner)) = (&playlists, weak.upgrade()) {
                inner.cache.borrow_mut().playlists = playlists.clone();
                inner.rebuild_playlists();
            }
            let songs = crate::bg(async move {
                client
                    .library_songs(move |n| {
                        let _ = progress_tx.try_send(n);
                    })
                    .await
            })
            .await;
            let Some(inner) = weak.upgrade() else { return };
            inner.end_loading();
            match (playlists, songs) {
                (Ok(_), Ok(songs)) => {
                    inner.songs_table.set_songs(&songs);
                    inner.cache.borrow_mut().songs = songs;
                    inner.cache.borrow().save();
                }
                (Err(e), _) | (_, Err(e)) => inner.api_error(e),
            }
            inner.update_title();
            let page = inner.page.borrow().clone();
            if let Page::Playlist(id) = page {
                inner.load_playlist(id);
            }
        });
    }

    fn load_playlist(self: &Rc<Self>, id: String) {
        let cached = self.cache.borrow().playlist_tracks.get(&id).cloned();
        self.playlist_table.set_songs(cached.as_deref().unwrap_or(&[]));
        let Some(client) = self.client.borrow().clone() else { return };
        self.begin_loading();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let pid = id.clone();
            let tracks = crate::bg(async move { client.playlist_tracks(&pid).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.end_loading();
            match tracks {
                Ok(tracks) => {
                    if *inner.page.borrow() == Page::Playlist(id.clone()) {
                        inner.playlist_table.set_songs(&tracks);
                    }
                    inner.cache.borrow_mut().playlist_tracks.insert(id, tracks);
                    inner.cache.borrow().save();
                    inner.update_title();
                }
                Err(e) => inner.api_error(e),
            }
        });
    }

    fn search(self: &Rc<Self>, term: String) {
        let generation = self.search_generation.get() + 1;
        self.search_generation.set(generation);
        if term.is_empty() {
            self.search_table.set_songs(&[]);
            self.search_table.set_empty_text("Search Apple Music", "Find songs in the Apple Music catalog.");
            return;
        }
        let Some(client) = self.catalog_client() else {
            self.toast("Set up your MusicKit key to search");
            return;
        };
        self.begin_loading();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let t = term.clone();
            let found = crate::bg(async move { client.search_catalog(&t, SEARCH_RESULTS).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.end_loading();
            // Ignore answers to queries the user has typed past.
            if inner.search_generation.get() != generation {
                return;
            }
            match found {
                Ok(songs) => {
                    inner.search_table.clear_filter();
                    inner.search_table.set_empty_text("No Results", &format!("Nothing found for “{term}”."));
                    inner.search_table.set_songs(&songs);
                }
                Err(e) => inner.api_error(e),
            }
        });
    }

    /// The signed-in client, or a catalog-only one from the developer token.
    fn catalog_client(&self) -> Option<Client> {
        if let Some(c) = self.client.borrow().as_ref() {
            return Some(c.clone());
        }
        let dev = self.developer_token.borrow().clone()?;
        let sf = self.config.borrow().storefront.clone().unwrap_or_else(|| "us".into());
        Some(dev.client("", &sf))
    }

    // ── Table requests ────────────────────────────────────────────────────

    fn handle_request(self: &Rc<Self>, req: Request) {
        match req {
            Request::Play { queue, start } => self.play(&queue, start),
            Request::AddToLibrary(songs) => self.add_to_library(songs),
            Request::AddToPlaylist { playlist_id, songs } => self.add_to_playlist(playlist_id, songs),
            Request::NewPlaylist(songs) => self.new_playlist_dialog(songs),
        }
    }

    fn play(self: &Rc<Self>, queue: &[Song], start: usize) {
        if start >= queue.len() {
            return;
        }
        self.ensure_engine();
        let engine = self.engine.borrow().clone();
        let Some(engine) = engine else {
            let why = self.engine_error.borrow().clone().unwrap_or_default();
            self.toast(&format!("Playback unavailable. {why}"));
            return;
        };
        let end = (start + QUEUE_MAX).min(queue.len());
        let ids: Vec<&str> = queue[start..end].iter().map(Song::playback_id).collect();
        engine.play(&ids, 0);
        engine.set_volume(self.player.volume());
    }

    fn add_to_library(self: &Rc<Self>, songs: Vec<Song>) {
        let Some(client) = self.signed_in_client() else { return };
        let ids: Vec<String> = songs.iter().filter_map(|s| s.catalog_id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = ids.len();
            let res = crate::bg(async move {
                let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                client.add_to_library(&refs).await
            })
            .await;
            let Some(inner) = weak.upgrade() else { return };
            match res {
                Ok(()) => {
                    inner.toast(&plural(n, "song added to your library", "songs added to your library"));
                    // Apple applies the change asynchronously.
                    glib::timeout_future_seconds(5).await;
                    if let Some(inner) = weak.upgrade() {
                        inner.refresh_library();
                    }
                }
                Err(e) => inner.api_error(e),
            }
        });
    }

    fn add_to_playlist(self: &Rc<Self>, playlist_id: String, songs: Vec<Song>) {
        let Some(client) = self.signed_in_client() else { return };
        if songs.is_empty() {
            return;
        }
        let name = self.playlist(&playlist_id).map(|p| p.name).unwrap_or_default();
        let refs: Vec<_> = songs.iter().map(Song::track_ref).collect();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let pid = playlist_id.clone();
            let res = crate::bg(async move { client.add_to_playlist(&pid, &refs).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            match res {
                Ok(()) => {
                    inner.toast(&format!("Added to “{name}”"));
                    inner.cache.borrow_mut().playlist_tracks.remove(&playlist_id);
                    if *inner.page.borrow() == Page::Playlist(playlist_id.clone()) {
                        inner.load_playlist(playlist_id);
                    }
                }
                Err(e) => inner.api_error(e),
            }
        });
    }

    fn create_playlist(self: &Rc<Self>, name: String, songs: Vec<Song>) {
        let Some(client) = self.signed_in_client() else { return };
        let refs: Vec<_> = songs.iter().map(Song::track_ref).collect();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let n = name.clone();
            let res = crate::bg(async move { client.create_playlist(&n, None, &refs).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            match res {
                Ok(playlist) => {
                    let id = playlist.id.clone();
                    inner.cache.borrow_mut().playlists.push(playlist);
                    inner.cache.borrow_mut().playlist_tracks.insert(id.clone(), songs);
                    inner.cache.borrow().save();
                    inner.rebuild_playlists();
                    inner.select_sidebar(&format!("playlist:{id}"));
                    inner.toast(&format!("Created “{name}”"));
                }
                Err(e) => inner.api_error(e),
            }
        });
    }

    fn signed_in_client(self: &Rc<Self>) -> Option<Client> {
        let client = self.client.borrow().clone();
        if client.is_none() {
            self.toast("Sign in to Apple Music first");
        }
        client
    }

    // ── Pages and sidebar ─────────────────────────────────────────────────

    fn show_page(self: &Rc<Self>, page: Page) {
        let changed = *self.page.borrow() != page;
        *self.page.borrow_mut() = page.clone();
        if let (true, Page::Playlist(id)) = (changed, &page) {
            self.playlist_table.clear_filter();
            self.load_playlist(id.clone());
        }
        self.refresh_page();
        if page == Page::Search {
            self.search_entry.grab_focus();
        }
    }

    /// Picks the visible stack child from page + session state.
    fn refresh_page(&self) {
        // Only a failed token fetch blocks the window; while it's still
        // loading, the cached library shows.
        let token_error = self.token_error.borrow().clone();
        if let Some(e) = &token_error {
            self.setup_page.set_description(Some(&glib::markup_escape_text(e)));
        }
        let has_key = token_error.is_none();
        let signed_in = self.client.borrow().is_some();
        let page = self.page.borrow().clone();
        let child = match page {
            _ if !has_key => "setup",
            Page::Search => "search",
            // Show the cached library while offline/signed out, if we have one.
            Page::Songs if signed_in || !self.cache.borrow().songs.is_empty() => "songs",
            Page::Playlist(_) if signed_in || !self.cache.borrow().playlists.is_empty() => "playlist",
            _ => "signin",
        };
        if child == "signin" {
            let engine_missing = self.engine.borrow().is_none();
            let why = self.engine_error.borrow().clone();
            self.signin_page.set_description(Some(&match (engine_missing, why) {
                (true, Some(why)) => format!("The playback engine is needed to sign in. {why}"),
                _ => "Apple's sign-in window opens to connect your library and subscription.".to_owned(),
            }));
        }
        self.stack.set_visible_child_name(child);
        self.update_title();
    }

    fn update_title(&self) {
        let page = self.page.borrow().clone();
        let cache = self.cache.borrow();
        let (title, count) = match &page {
            Page::Search => ("Search".to_owned(), None),
            Page::Songs => ("Songs".to_owned(), Some(cache.songs.len())),
            Page::Playlist(id) => (
                cache.playlists.iter().find(|p| &p.id == id).map(|p| p.name.clone()).unwrap_or_default(),
                cache.playlist_tracks.get(id).map(Vec::len),
            ),
        };
        self.title.set_title(&title);
        // No count on the setup and sign-in screens.
        let showing_list = matches!(self.stack.visible_child_name().as_deref(), Some("songs" | "playlist"));
        let count = count.filter(|_| showing_list);
        if self.loading.get() == 0 {
            self.title.set_subtitle(&count.map(|n| plural(n, "song", "songs")).unwrap_or_default());
        }
    }

    fn rebuild_playlists(&self) {
        // Drop old playlist rows (everything after the "Playlists" header).
        while let Some(row) = self.sidebar.row_at_index(3) {
            self.sidebar.remove(&row);
        }
        let mut playlists = self.cache.borrow().playlists.clone();
        playlists.sort_by_key(|p| p.name.to_lowercase());
        self.playlists_menu.remove_all();
        for p in &playlists {
            let icon = if p.can_edit { "view-list-symbolic" } else { "starred-symbolic" };
            self.sidebar.append(&sidebar_row(icon, &p.name, &format!("playlist:{}", p.id)));
            // Only the user's own playlists accept new tracks.
            if p.can_edit {
                let item = gio::MenuItem::new(Some(&p.name), None);
                item.set_action_and_target_value(Some("table.add-to-playlist"), Some(&p.id.to_variant()));
                self.playlists_menu.append_item(&item);
            }
        }
        // Keep the current playlist selected after a rebuild.
        if let Page::Playlist(id) = &*self.page.borrow() {
            let name = format!("playlist:{id}");
            if let Some(row) = self.find_row(&name) {
                self.sidebar.select_row(Some(&row));
            }
        }
    }

    fn find_row(&self, name: &str) -> Option<gtk::ListBoxRow> {
        (0..).map_while(|i| self.sidebar.row_at_index(i)).find(|r| r.widget_name() == name)
    }

    fn select_sidebar(&self, name: &str) {
        if let Some(row) = self.find_row(name) {
            self.sidebar.select_row(Some(&row));
        }
    }

    fn playlist(&self, id: &str) -> Option<Playlist> {
        self.cache.borrow().playlists.iter().find(|p| p.id == id).cloned()
    }

    // ── Dialogs ───────────────────────────────────────────────────────────

    fn new_playlist_dialog(self: &Rc<Self>, songs: Vec<Song>) {
        if self.signed_in_client().is_none() {
            return;
        }
        let entry = gtk::Entry::builder().placeholder_text("Playlist name").activates_default(true).build();
        let body = if songs.is_empty() {
            String::new()
        } else {
            plural(songs.len(), "song", "songs") + " will be added."
        };
        let dialog = adw::AlertDialog::new(Some("New Playlist"), Some(&body));
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));
        dialog.set_close_response("cancel");
        dialog.set_response_enabled("create", false);
        let d = dialog.clone();
        entry.connect_changed(move |e| d.set_response_enabled("create", !e.text().trim().is_empty()));
        let weak = Rc::downgrade(self);
        let songs = RefCell::new(Some(songs));
        dialog.connect_response(Some("create"), move |_, _| {
            let name = entry.text().trim().to_owned();
            if let (Some(inner), Some(songs)) = (weak.upgrade(), songs.borrow_mut().take()) {
                inner.create_playlist(name, songs);
            }
        });
        dialog.present(self.window.upgrade().as_ref());
    }

    fn show_key_dialog(self: &Rc<Self>) {
        let config = self.config.borrow().clone();
        let dialog = adw::PreferencesDialog::builder().title("MusicKit Key").build();
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .title("Apple Developer")
            .description(
                "A MusicKit private key from developer.apple.com (Certificates, IDs &amp; Profiles → Keys). \
                 Figaro signs its developer token with it locally.",
            )
            .build();
        let team = adw::EntryRow::builder().title("Team ID").text(&config.team_id).build();
        let key_id = adw::EntryRow::builder().title("Key ID").text(&config.key_id).build();
        let path = RefCell::new(config.key_path.clone());
        let file_row = adw::ActionRow::builder()
            .title("Private Key (.p8)")
            .subtitle(if config.key_path.is_empty() { "None" } else { &config.key_path })
            .build();
        let choose = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
        file_row.add_suffix(&choose);
        group.add(&team);
        group.add(&key_id);
        group.add(&file_row);
        page.add(&group);
        dialog.add(&page);

        let path = Rc::new(path);
        let (p, row, win) = (path.clone(), file_row.clone(), self.window.clone());
        choose.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("MusicKit key (.p8)"));
            filter.add_pattern("*.p8");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let chooser = gtk::FileDialog::builder().title("Choose MusicKit Key").filters(&filters).build();
            let (p, row) = (p.clone(), row.clone());
            chooser.open(win.upgrade().as_ref(), None::<&gio::Cancellable>, move |res| {
                if let Some(file) = res.ok().and_then(|f| f.path()) {
                    let s = file.to_string_lossy().into_owned();
                    row.set_subtitle(&s);
                    // Key IDs are in the file name: AuthKey_<KEYID>.p8.
                    *p.borrow_mut() = s;
                }
            });
        });

        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            let mut new = inner.config.borrow().clone();
            new.team_id = team.text().trim().to_owned();
            new.key_id = key_id.text().trim().to_owned();
            new.key_path = path.borrow().clone();
            if new.key_id.is_empty() {
                if let Some(id) = key_id_from_path(&new.key_path) {
                    new.key_id = id;
                }
            }
            let old = inner.config.borrow().clone();
            if (new.team_id.as_str(), new.key_id.as_str(), new.key_path.as_str())
                == (old.team_id.as_str(), old.key_id.as_str(), old.key_path.as_str())
            {
                return;
            }
            new.save();
            *inner.config.borrow_mut() = new;
            inner.start_session(false);
        });
        dialog.present(self.window.upgrade().as_ref());
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Figaro")
            .application_icon("audio-x-generic")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Your Apple Music library and playlists, on GNOME.")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(self.window.upgrade().as_ref());
    }

    // ── Misc ──────────────────────────────────────────────────────────────

    fn load_artwork(self: &Rc<Self>, template: String) {
        let Some(client) = self.catalog_client() else { return };
        let generation = self.artwork_generation.get() + 1;
        self.artwork_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let scale = 2; // sharp on HiDPI, cheap anyway
            let res =
                crate::bg(async move { client.fetch_artwork(&template, ARTWORK_SIZE * scale).await }).await;
            let Some(inner) = weak.upgrade() else { return };
            if let (Ok(bytes), true) = (res, inner.artwork_generation.get() == generation) {
                inner.player.set_artwork(&bytes);
            }
        });
    }

    fn begin_loading(&self) {
        self.loading.set(self.loading.get() + 1);
        self.spinner.start();
    }

    fn end_loading(&self) {
        self.loading.set(self.loading.get().saturating_sub(1));
        if self.loading.get() == 0 {
            self.spinner.stop();
            self.update_title();
        }
    }

    fn toast(&self, message: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(message));
        toast.set_timeout(4);
        self.toasts.add_toast(toast);
    }
}

fn with_engine(weak: &Weak<Inner>, f: impl FnOnce(&Engine)) {
    if let Some(inner) = weak.upgrade() {
        if let Some(engine) = inner.engine.borrow().as_ref() {
            f(engine);
        }
    }
}

fn engine_action(weak: &Weak<Inner>, f: fn(&Engine)) -> Box<dyn Fn()> {
    let weak = weak.clone();
    Box::new(move || with_engine(&weak, f))
}

fn sidebar_row(icon: &str, label: &str, name: &str) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&gtk::Image::from_icon_name(icon));
    content.append(
        &gtk::Label::builder().label(label).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build(),
    );
    let row = gtk::ListBoxRow::builder().child(&content).name(name).build();
    row.set_tooltip_text(Some(label));
    row
}

fn key_id_from_path(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_stem()?.to_str()?;
    name.strip_prefix("AuthKey_").map(str::to_owned)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_key_id_from_file_name() {
        assert_eq!(key_id_from_path("/home/me/AuthKey_ABC123DEF4.p8").as_deref(), Some("ABC123DEF4"));
        assert_eq!(key_id_from_path("/home/me/key.p8"), None);
    }
}
