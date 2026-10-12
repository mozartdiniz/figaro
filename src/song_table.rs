//! A sortable, filterable song list: the core view of Songs, Playlists and
//! Search, like Music.app's song table.
//!
//! Model chain: `ListStore<Row>` → `FilterListModel` (search box) →
//! `SortListModel` (column headers) → `MultiSelection` → `ColumnView`.
//! The `#` column is the original order, so clicking it restores the
//! playlist order after sorting by something else.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use applemusic::Song;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// What the user asked for; the window decides what to do with it.
pub enum Request {
    /// Play `queue` (the list as currently sorted and filtered) from `start`.
    Play {
        queue: Vec<Song>,
        start: usize,
    },
    AddToLibrary(Vec<Song>),
    AddToPlaylist {
        playlist_id: String,
        songs: Vec<Song>,
    },
    NewPlaylist(Vec<Song>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Library,
    Playlist,
    /// Catalog search results: offers "Add to Library", no filter box.
    Search,
}

struct Row {
    index: usize,
    song: Song,
}

#[derive(Clone)]
pub struct SongTable(Rc<Inner>);

struct Inner {
    root: gtk::Box,
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    sorted: gtk::SortListModel,
    selection: gtk::MultiSelection,
    view: gtk::ColumnView,
    filter_entry: gtk::SearchEntry,
    stack: gtk::Stack,
    empty: adw::StatusPage,
    /// Title and description shown when the list itself is empty.
    empty_text: RefCell<(String, String)>,
    query: RefCell<String>,
}

impl SongTable {
    pub fn new(kind: Kind, playlists_menu: &gio::Menu, on_request: Rc<dyn Fn(Request)>) -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let query = RefCell::new(String::new());
        let filter = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let view = gtk::ColumnView::new(None::<gtk::SelectionModel>);
        view.set_show_column_separators(false);
        view.set_reorderable(false);
        view.add_css_class("data-table");
        let sorted = gtk::SortListModel::new(Some(filtered), view.sorter());
        let selection = gtk::MultiSelection::new(Some(sorted.clone()));
        view.set_model(Some(&selection));

        let filter_entry = gtk::SearchEntry::builder().placeholder_text("Filter songs").hexpand(true).build();
        let filter_bar =
            gtk::Box::builder().margin_start(12).margin_end(12).margin_top(6).margin_bottom(6).build();
        filter_bar.append(&filter_entry);
        filter_bar.set_visible(kind != Kind::Search);

        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let empty = adw::StatusPage::builder().icon_name("audio-x-generic-symbolic").vexpand(true).build();
        let stack = gtk::Stack::new();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty, Some("empty"));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&filter_bar);
        root.append(&stack);

        let table = SongTable(Rc::new(Inner {
            root,
            store,
            filter,
            sorted,
            selection,
            view,
            filter_entry,
            stack,
            empty,
            empty_text: RefCell::default(),
            query,
        }));
        table.setup_filter();
        table.setup_columns(kind);
        table.setup_actions(kind, playlists_menu, on_request);
        table.update_empty();
        table
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.0.root
    }

    pub fn set_songs(&self, songs: &[Song]) {
        let rows: Vec<glib::BoxedAnyObject> = songs
            .iter()
            .enumerate()
            .map(|(index, song)| glib::BoxedAnyObject::new(Row { index, song: song.clone() }))
            .collect();
        self.0.store.splice(0, self.0.store.n_items(), &rows);
        self.update_empty();
    }

    pub fn set_empty_text(&self, title: &str, description: &str) {
        *self.0.empty_text.borrow_mut() = (title.to_owned(), description.to_owned());
        self.update_empty();
    }

    pub fn focus_filter(&self) {
        if self.0.filter_entry.is_visible() {
            self.0.filter_entry.grab_focus();
        }
    }

    pub fn clear_filter(&self) {
        self.0.filter_entry.set_text("");
    }

    /// The list as the user sees it: filtered and sorted.
    fn visible_songs(&self) -> Vec<Song> {
        (0..self.0.sorted.n_items()).filter_map(|i| self.song_at(i)).collect()
    }

    fn song_at(&self, position: u32) -> Option<Song> {
        let obj = self.0.sorted.item(position)?.downcast::<glib::BoxedAnyObject>().ok()?;
        let row = obj.borrow::<Row>();
        Some(row.song.clone())
    }

    fn selected_positions(&self) -> Vec<u32> {
        let set = self.0.selection.selection();
        (0..set.size()).map(|i| set.nth(i as u32)).collect()
    }

    fn selected_songs(&self) -> Vec<Song> {
        self.selected_positions().into_iter().filter_map(|p| self.song_at(p)).collect()
    }

    fn update_empty(&self) {
        let empty = self.0.sorted.n_items() == 0;
        self.0.stack.set_visible_child_name(if empty { "empty" } else { "list" });
        if self.0.store.n_items() > 0 {
            self.0.empty.set_title("No Matches");
            self.0.empty.set_description(Some("No songs match the filter."));
        } else {
            let (title, description) = &*self.0.empty_text.borrow();
            self.0.empty.set_title(title);
            self.0.empty.set_description(Some(description));
        }
    }

    fn setup_filter(&self) {
        let weak = Rc::downgrade(&self.0);
        self.0.filter.set_filter_func(move |obj| {
            let Some(inner) = weak.upgrade() else { return true };
            let query = inner.query.borrow();
            if query.is_empty() {
                return true;
            }
            let row = obj.downcast_ref::<glib::BoxedAnyObject>().expect("row").borrow::<Row>();
            let s = &row.song;
            // Every word must match somewhere, so "beatles help" works.
            query
                .split_whitespace()
                .all(|w| contains_ci(&s.name, w) || contains_ci(&s.artist, w) || contains_ci(&s.album, w))
        });

        let this = self.clone();
        self.0.filter_entry.connect_search_changed(move |entry| {
            *this.0.query.borrow_mut() = entry.text().to_lowercase();
            this.0.filter.changed(gtk::FilterChange::Different);
        });
    }

    fn setup_columns(&self, kind: Kind) {
        let view = &self.0.view;
        let index_title = if kind == Kind::Search { "#" } else { "No." };
        let index_col = column(index_title, true, false, |r| (r.index + 1).to_string());
        index_col.set_sorter(Some(&sorter(|a, b| a.index.cmp(&b.index))));
        view.append_column(&index_col);

        let title = column("Title", false, true, |r| r.song.name.clone());
        title.set_sorter(Some(&sorter(|a, b| {
            cmp_ci(&a.song.name, &b.song.name).then_with(|| by_artist(a, b))
        })));
        view.append_column(&title);

        let artist = column("Artist", false, true, |r| r.song.artist.clone());
        artist.set_sorter(Some(&sorter(by_artist)));
        view.append_column(&artist);

        let album = column("Album", false, true, |r| r.song.album.clone());
        album.set_sorter(Some(&sorter(by_album)));
        view.append_column(&album);

        let genre = column("Genre", false, false, |r| r.song.genre.clone());
        genre.set_fixed_width(130);
        genre.set_sorter(Some(&sorter(|a, b| {
            cmp_ci(&a.song.genre, &b.song.genre).then_with(|| by_artist(a, b))
        })));
        view.append_column(&genre);

        let year = column("Year", true, false, |r| r.song.year().map(|y| y.to_string()).unwrap_or_default());
        year.set_sorter(Some(&sorter(|a, b| {
            a.song.year().cmp(&b.song.year()).then_with(|| by_artist(a, b))
        })));
        view.append_column(&year);

        let time = column("Time", true, false, |r| format_duration(r.song.duration_ms as f64 / 1000.0));
        time.set_sorter(Some(&sorter(|a, b| a.song.duration_ms.cmp(&b.song.duration_ms))));
        view.append_column(&time);

        let this = self.clone();
        let sorted = self.0.sorted.clone();
        sorted.connect_items_changed(move |_, _, _, _| this.update_empty());
    }

    fn setup_actions(&self, kind: Kind, playlists_menu: &gio::Menu, on_request: Rc<dyn Fn(Request)>) {
        let group = gio::SimpleActionGroup::new();

        let play = gio::SimpleAction::new("play", None);
        let (this, req) = (self.clone(), on_request.clone());
        play.connect_activate(move |_, _| {
            let start = this.selected_positions().first().copied().unwrap_or(0) as usize;
            req(Request::Play { queue: this.visible_songs(), start });
        });
        group.add_action(&play);

        let add_lib = gio::SimpleAction::new("add-to-library", None);
        let (this, req) = (self.clone(), on_request.clone());
        add_lib.connect_activate(move |_, _| req(Request::AddToLibrary(this.selected_songs())));
        group.add_action(&add_lib);

        let add_pl = gio::SimpleAction::new("add-to-playlist", Some(glib::VariantTy::STRING));
        let (this, req) = (self.clone(), on_request.clone());
        add_pl.connect_activate(move |_, param| {
            let Some(playlist_id) = param.and_then(|p| p.get::<String>()) else { return };
            req(Request::AddToPlaylist { playlist_id, songs: this.selected_songs() });
        });
        group.add_action(&add_pl);

        let new_pl = gio::SimpleAction::new("new-playlist", None);
        let (this, req) = (self.clone(), on_request.clone());
        new_pl.connect_activate(move |_, _| req(Request::NewPlaylist(this.selected_songs())));
        group.add_action(&new_pl);

        self.0.root.insert_action_group("table", Some(&group));

        // Double-click / Enter plays from that row, with the rest of the
        // visible list as the queue.
        let (this, req) = (self.clone(), on_request);
        self.0.view.connect_activate(move |_, position| {
            req(Request::Play { queue: this.visible_songs(), start: position as usize });
        });

        // Context menu.
        let menu = gio::Menu::new();
        let top = gio::Menu::new();
        top.append(Some("Play"), Some("table.play"));
        if kind == Kind::Search {
            top.append(Some("Add to Library"), Some("table.add-to-library"));
        }
        menu.append_section(None, &top);
        let add_to = gio::Menu::new();
        add_to.append(Some("New Playlist…"), Some("table.new-playlist"));
        add_to.append_section(None, playlists_menu);
        menu.append_submenu(Some("Add to Playlist"), &add_to);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&self.0.view);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);

        let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let this = self.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let view = &this.0.view;
            // Find which row was clicked: pick the widget under the pointer
            // and walk up to the row widget, then map it to a position.
            if let Some(position) = row_position_at(view, x, y, &this) {
                if !this.0.selection.is_selected(position) {
                    this.0.selection.select_item(position, true);
                }
            }
            if this.selected_positions().is_empty() {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
        self.0.view.add_controller(click);
    }
}

/// The row under (x, y) in `view` coordinates. Cell labels carry their
/// list position (set in `bind`) as widget data under `ROW_POS`.
fn row_position_at(view: &gtk::ColumnView, x: f64, y: f64, table: &SongTable) -> Option<u32> {
    let mut widget = view.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(w) = widget {
        if let Some(label) = w.downcast_ref::<gtk::Label>() {
            if let Some(pos) = cell_position(label) {
                return (pos < table.0.sorted.n_items()).then_some(pos);
            }
        }
        // Clicked on cell padding: descend into the cell's label instead.
        if let Some(child) = w.first_child() {
            if let Some(label) = child.downcast_ref::<gtk::Label>() {
                if let Some(pos) = cell_position(label) {
                    return Some(pos);
                }
            }
        }
        if w == *view.upcast_ref::<gtk::Widget>() {
            break;
        }
        widget = w.parent();
    }
    None
}

const ROW_POS: &str = "figaro-row-position";

fn cell_position(label: &gtk::Label) -> Option<u32> {
    // SAFETY: only `column` stores data under ROW_POS, always as u32.
    unsafe { label.data::<u32>(ROW_POS).map(|p| *p.as_ref()) }
}

fn column(title: &str, numeric: bool, expand: bool, text: fn(&Row) -> String) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let label = gtk::Label::builder()
            .xalign(if numeric { 1.0 } else { 0.0 })
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .build();
        if numeric {
            label.add_css_class("numeric");
            label.add_css_class("dim-label");
        }
        item.downcast_ref::<gtk::ListItem>().expect("ListItem").set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = item.child().and_downcast::<gtk::Label>().expect("label");
        let obj = item.item().and_downcast::<glib::BoxedAnyObject>().expect("row");
        label.set_text(&text(&obj.borrow::<Row>()));
        // SAFETY: see `cell_position`.
        unsafe { label.set_data(ROW_POS, item.position()) };
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col.set_resizable(true);
    col
}

fn sorter(cmp: fn(&Row, &Row) -> Ordering) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let a = a.downcast_ref::<glib::BoxedAnyObject>().expect("row").borrow::<Row>();
        let b = b.downcast_ref::<glib::BoxedAnyObject>().expect("row").borrow::<Row>();
        cmp(&a, &b).into()
    })
}

/// Artist, then album in track order — how Music.app sorts by Artist.
fn by_artist(a: &Row, b: &Row) -> Ordering {
    cmp_ci(&a.song.artist, &b.song.artist).then_with(|| by_album(a, b))
}

fn by_album(a: &Row, b: &Row) -> Ordering {
    cmp_ci(&a.song.album, &b.song.album)
        .then(a.song.disc_number.cmp(&b.song.disc_number))
        .then(a.song.track_number.cmp(&b.song.track_number))
        .then_with(|| cmp_ci(&a.song.name, &b.song.name))
}

/// Case-insensitive, ignoring a leading "The " like Music.app does.
fn cmp_ci(a: &str, b: &str) -> Ordering {
    let key = |s: &str| {
        let s = s.trim();
        let s = s.strip_prefix("The ").or_else(|| s.strip_prefix("the ")).unwrap_or(s);
        s.to_lowercase()
    };
    // Empty values sort last, not first.
    match (a.trim().is_empty(), b.trim().is_empty()) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => key(a).cmp(&key(b)),
    }
}

fn contains_ci(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return String::new();
    }
    let s = seconds.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_like_music_app() {
        assert_eq!(cmp_ci("The Beatles", "beck"), Ordering::Less); // "beatles" < "beck"
        assert_eq!(cmp_ci("abba", "ABBA"), Ordering::Equal);
        assert_eq!(cmp_ci("", "Zappa"), Ordering::Greater);
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0.0), "");
        assert_eq!(format_duration(65.4), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }
}
