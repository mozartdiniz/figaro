//! MPRIS2 on the session bus, so the desktop (media keys, GNOME Shell's
//! media controls, MyShell's music applet) can see and control playback.
//!
//! Implements `org.mpris.MediaPlayer2`, `.Player` and `.TrackList`. The
//! track list is the queue as Figaro handed it to the engine; the engine's
//! `now_playing` says which song of it plays. Track ids are
//! `/io/github/mozartdiniz/Figaro/track/<generation>_<index>`, so ids of a
//! replaced queue never match the new one.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Instant;

use gtk::glib::variant::ObjectPath;
use gtk::glib::variant::ToVariant;
use gtk::glib::{self, Variant, VariantTy};
use gtk::gio;

use crate::playback::NowPlaying;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.figaro";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const TRACKLIST_IFACE: &str = "org.mpris.MediaPlayer2.TrackList";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const TRACK_PREFIX: &str = "/io/github/mozartdiniz/Figaro/track/";

const XML: &str = r#"
<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="DesktopEntry" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek"><arg direction="in" name="Offset" type="x"/></method>
    <method name="SetPosition">
      <arg direction="in" name="TrackId" type="o"/>
      <arg direction="in" name="Position" type="x"/>
    </method>
    <method name="OpenUri"><arg direction="in" name="Uri" type="s"/></method>
    <signal name="Seeked"><arg name="Position" type="x"/></signal>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="Rate" type="d" access="readwrite"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read"/>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.TrackList">
    <method name="GetTracksMetadata">
      <arg direction="in" name="TrackIds" type="ao"/>
      <arg direction="out" name="Metadata" type="aa{sv}"/>
    </method>
    <method name="AddTrack">
      <arg direction="in" name="Uri" type="s"/>
      <arg direction="in" name="AfterTrack" type="o"/>
      <arg direction="in" name="SetAsCurrent" type="b"/>
    </method>
    <method name="RemoveTrack"><arg direction="in" name="TrackId" type="o"/></method>
    <method name="GoTo"><arg direction="in" name="TrackId" type="o"/></method>
    <signal name="TrackListReplaced">
      <arg name="Tracks" type="ao"/>
      <arg name="CurrentTrack" type="o"/>
    </signal>
    <property name="Tracks" type="ao" access="read"/>
    <property name="CanEditTracks" type="b" access="read"/>
  </interface>
</node>
"#;

/// A queued song, as MPRIS shows it.
#[derive(Clone, Debug, Default)]
pub struct Track {
    /// Playback id, matched against the engine's `now_playing`.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds.
    pub duration: f64,
    pub art_url: Option<String>,
}

/// What MPRIS clients may ask for; the window wires these to the engine.
pub struct Actions {
    pub toggle: Box<dyn Fn()>,
    pub play: Box<dyn Fn()>,
    pub pause: Box<dyn Fn()>,
    pub next: Box<dyn Fn()>,
    pub previous: Box<dyn Fn()>,
    pub seek: Box<dyn Fn(f64)>,
    /// Plays the queued song with this playback id at this index.
    pub goto: Box<dyn Fn(&str, usize)>,
    pub volume: Box<dyn Fn(f64)>,
    pub raise: Box<dyn Fn()>,
    pub quit: Box<dyn Fn()>,
}

#[derive(Default)]
struct State {
    queue: Vec<Track>,
    generation: u32,
    current: Option<usize>,
    /// What the engine says plays; may be outside the queue.
    now: Option<Track>,
    /// A local copy of the current artwork, preferred over its URL.
    art_file: Option<String>,
    playing: bool,
    position: f64,
    position_at: Option<Instant>,
    duration: f64,
    volume: f64,
}

struct Inner {
    actions: Actions,
    state: RefCell<State>,
    connection: RefCell<Option<gio::DBusConnection>>,
    owner: Cell<Option<gio::OwnerId>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            gio::bus_unown_name(owner);
        }
    }
}

pub struct Mpris(Rc<Inner>);

impl Mpris {
    pub fn new(actions: Actions) -> Mpris {
        let inner = Rc::new(Inner {
            actions,
            state: RefCell::new(State { volume: 1.0, ..Default::default() }),
            connection: RefCell::new(None),
            owner: Cell::new(None),
        });
        let weak = Rc::downgrade(&inner);
        let owner = gio::bus_own_name(
            gio::BusType::Session,
            BUS_NAME,
            gio::BusNameOwnerFlags::NONE,
            move |connection, _| {
                if let Some(inner) = weak.upgrade() {
                    register(&inner, &connection);
                }
            },
            |_, _| {},
            |_, name| log::warn!("lost the bus name {name}; media controls won't see Figaro"),
        );
        inner.owner.set(Some(owner));
        Mpris(inner)
    }

    /// The queue the engine was just given; nothing plays from it yet.
    pub fn set_queue(&self, queue: Vec<Track>) {
        let (tracks, current) = {
            let mut s = self.0.state.borrow_mut();
            s.queue = queue;
            s.generation = s.generation.wrapping_add(1);
            s.current = None;
            (track_ids(&s), current_id(&s))
        };
        self.0.emit_signal(
            TRACKLIST_IFACE,
            "TrackListReplaced",
            Variant::tuple_from_iter([tracks.to_variant(), current.to_variant()]),
        );
        self.0.properties_changed(TRACKLIST_IFACE, &[], &["Tracks"]);
        self.0.properties_changed(PLAYER_IFACE, &["Metadata", "CanGoNext", "CanGoPrevious"], &[]);
    }

    pub fn set_now_playing(&self, item: Option<&NowPlaying>, index: i64) {
        {
            let mut s = self.0.state.borrow_mut();
            s.art_file = None;
            s.now = item.map(|n| Track {
                id: n.id.clone(),
                title: n.title.clone(),
                artist: n.artist.clone(),
                album: n.album.clone(),
                duration: n.duration,
                art_url: n.artwork.as_deref().map(|t| applemusic::artwork_url(t, 512)),
            });
            s.current = item.and_then(|n| find_in_queue(&s.queue, &n.id, index));
            s.duration = item.map_or(0.0, |n| n.duration);
            s.position = 0.0;
            s.position_at = None;
        }
        self.0.properties_changed(
            PLAYER_IFACE,
            &["Metadata", "CanGoNext", "CanGoPrevious", "CanPlay", "CanPause", "CanSeek"],
            &[],
        );
    }

    /// A local file of the current song's artwork (a `file://` URI).
    pub fn set_art_file(&self, uri: String) {
        self.0.state.borrow_mut().art_file = Some(uri);
        self.0.properties_changed(PLAYER_IFACE, &["Metadata"], &[]);
    }

    pub fn set_playing(&self, playing: bool) {
        {
            let mut s = self.0.state.borrow_mut();
            if s.playing == playing {
                return;
            }
            // Freeze or restart the position clock
            s.position = position(&s);
            s.position_at = playing.then(Instant::now);
            s.playing = playing;
        }
        self.0.properties_changed(PLAYER_IFACE, &["PlaybackStatus"], &[]);
    }

    pub fn set_time(&self, position: f64, duration: f64) {
        let jumped = {
            let mut s = self.0.state.borrow_mut();
            let expected = self::position(&s);
            s.position = position;
            s.position_at = s.playing.then(Instant::now);
            let seek_changed = (s.duration > 0.0) != (duration > 0.0);
            s.duration = duration;
            if seek_changed {
                drop(s);
                self.0.properties_changed(PLAYER_IFACE, &["CanSeek"], &[]);
                false
            } else {
                // Time events come once a second; more than that is a seek
                (position - expected).abs() > 2.5
            }
        };
        if jumped {
            self.0.seeked(position);
        }
    }

    pub fn set_volume(&self, volume: f64) {
        {
            let mut s = self.0.state.borrow_mut();
            if (s.volume - volume).abs() < 1e-6 {
                return;
            }
            s.volume = volume;
        }
        self.0.properties_changed(PLAYER_IFACE, &["Volume"], &[]);
    }

    /// Whether a song is loaded, playing or paused.
    pub fn has_song(&self) -> bool {
        self.0.state.borrow().now.is_some()
    }

    /// The engine went away: nothing plays.
    pub fn clear(&self) {
        self.set_playing(false);
        self.set_now_playing(None, -1);
    }
}

fn register(inner: &Rc<Inner>, connection: &gio::DBusConnection) {
    let node = gio::DBusNodeInfo::for_xml(XML).expect("valid MPRIS introspection XML");
    for iface in [ROOT_IFACE, PLAYER_IFACE, TRACKLIST_IFACE] {
        let info = node.lookup_interface(iface).expect("interface in XML");
        let (call, get, set) = (Rc::downgrade(inner), Rc::downgrade(inner), Rc::downgrade(inner));
        let res = connection
            .register_object(PATH, &info)
            .method_call(move |_, _, _, iface, method, params, invocation| {
                match call.upgrade() {
                    Some(inner) => inner.call(iface.unwrap_or_default(), method, &params, invocation),
                    None => invocation.return_dbus_error("org.freedesktop.DBus.Error.Failed", "Figaro is closing"),
                }
            })
            .property(move |_, _, _, iface, prop| match get.upgrade() {
                Some(inner) => inner.get(iface, prop),
                None => false.to_variant(),
            })
            .set_property(move |_, _, _, iface, prop, value| set_property(&set, iface, prop, &value))
            .build();
        if let Err(e) = res {
            log::warn!("can't export {iface}: {e}");
        }
    }
    *inner.connection.borrow_mut() = Some(connection.clone());
}

fn set_property(weak: &Weak<Inner>, iface: &str, prop: &str, value: &Variant) -> bool {
    let Some(inner) = weak.upgrade() else { return false };
    match (iface, prop, value.get::<f64>()) {
        (PLAYER_IFACE, "Volume", Some(v)) => {
            (inner.actions.volume)(v.clamp(0.0, 1.0));
            true
        }
        // Only normal speed; accept it so clients don't error out
        (PLAYER_IFACE, "Rate", Some(_)) => true,
        _ => false,
    }
}

impl Inner {
    fn call(&self, iface: &str, method: &str, params: &Variant, invocation: gio::DBusMethodInvocation) {
        let a = &self.actions;
        match (iface, method) {
            (ROOT_IFACE, "Raise") => (a.raise)(),
            (ROOT_IFACE, "Quit") => (a.quit)(),
            (PLAYER_IFACE, "Next") => (a.next)(),
            (PLAYER_IFACE, "Previous") => (a.previous)(),
            (PLAYER_IFACE, "Pause" | "Stop") => (a.pause)(),
            (PLAYER_IFACE, "Play") => (a.play)(),
            (PLAYER_IFACE, "PlayPause") => (a.toggle)(),
            (PLAYER_IFACE, "Seek") => {
                let Some((offset,)) = params.get::<(i64,)>() else { return bad_args(invocation) };
                let (target, duration) = {
                    let s = self.state.borrow();
                    (position(&s) + offset as f64 / 1e6, s.duration)
                };
                if duration > 0.0 && target >= duration {
                    (a.next)();
                } else {
                    self.seek_to(target.max(0.0));
                }
            }
            (PLAYER_IFACE, "SetPosition") => {
                let Some((track, pos)) = params.get::<(ObjectPath, i64)>() else { return bad_args(invocation) };
                let (current, duration) = {
                    let s = self.state.borrow();
                    (current_id(&s), s.duration)
                };
                let pos = pos as f64 / 1e6;
                // Ignored, per the spec, unless it's for the current track and in range
                if track.as_str() == current.as_str() && pos >= 0.0 && pos <= duration {
                    self.seek_to(pos);
                }
            }
            (PLAYER_IFACE, "OpenUri") | (TRACKLIST_IFACE, "AddTrack" | "RemoveTrack") => {
                return invocation
                    .return_dbus_error("org.freedesktop.DBus.Error.NotSupported", "Figaro can't do that yet");
            }
            (TRACKLIST_IFACE, "GetTracksMetadata") => {
                let Some((ids,)) = params.get::<(Vec<ObjectPath>,)>() else { return bad_args(invocation) };
                let dicts: Vec<Variant> = {
                    let s = self.state.borrow();
                    ids.iter()
                        .filter_map(|id| index_of(&s, id.as_str()).map(|i| metadata(&s.queue[i], id.as_str(), None)))
                        .collect()
                };
                let array = Variant::array_from_iter_with_type(VariantTy::VARDICT, dicts);
                return invocation.return_value(Some(&Variant::tuple_from_iter([array])));
            }
            (TRACKLIST_IFACE, "GoTo") => {
                let Some((id,)) = params.get::<(ObjectPath,)>() else { return bad_args(invocation) };
                let target = {
                    let s = self.state.borrow();
                    index_of(&s, id.as_str()).map(|i| (s.queue[i].id.clone(), i))
                };
                if let Some((song, index)) = target {
                    (a.goto)(&song, index);
                }
            }
            _ => {
                return invocation.return_dbus_error(
                    "org.freedesktop.DBus.Error.UnknownMethod",
                    &format!("no method {iface}.{method}"),
                );
            }
        }
        invocation.return_value(None);
    }

    fn get(&self, iface: &str, prop: &str) -> Variant {
        let s = self.state.borrow();
        let has_queue = !s.queue.is_empty() || s.now.is_some();
        match (iface, prop) {
            (ROOT_IFACE, "CanQuit" | "CanRaise" | "HasTrackList") => true.to_variant(),
            (ROOT_IFACE, "Identity") => "Figaro".to_variant(),
            (ROOT_IFACE, "DesktopEntry") => crate::config::APP_ID.to_variant(),
            (ROOT_IFACE, "SupportedUriSchemes" | "SupportedMimeTypes") => Vec::<String>::new().to_variant(),

            (PLAYER_IFACE, "PlaybackStatus") => {
                let status = match (s.playing, s.now.is_some()) {
                    (true, _) => "Playing",
                    (false, true) => "Paused",
                    (false, false) => "Stopped",
                };
                status.to_variant()
            }
            (PLAYER_IFACE, "Rate" | "MinimumRate" | "MaximumRate") => 1.0f64.to_variant(),
            (PLAYER_IFACE, "Metadata") => match &s.now {
                Some(now) => metadata(now, &current_id(&s), s.art_file.as_deref()),
                None => glib::VariantDict::new(None).end(),
            },
            (PLAYER_IFACE, "Volume") => s.volume.to_variant(),
            (PLAYER_IFACE, "Position") => ((position(&s) * 1e6) as i64).to_variant(),
            (PLAYER_IFACE, "CanGoNext") => s.current.is_some_and(|i| i + 1 < s.queue.len()).to_variant(),
            (PLAYER_IFACE, "CanGoPrevious") => s.now.is_some().to_variant(),
            (PLAYER_IFACE, "CanPlay" | "CanPause") => has_queue.to_variant(),
            (PLAYER_IFACE, "CanSeek") => (s.duration > 0.0).to_variant(),
            (PLAYER_IFACE, "CanControl") => true.to_variant(),

            (TRACKLIST_IFACE, "Tracks") => track_ids(&s).to_variant(),
            (TRACKLIST_IFACE, "CanEditTracks") => false.to_variant(),
            _ => false.to_variant(),
        }
    }

    fn seek_to(&self, seconds: f64) {
        (self.actions.seek)(seconds);
        {
            let mut s = self.state.borrow_mut();
            s.position = seconds;
            s.position_at = s.playing.then(Instant::now);
        }
        self.seeked(seconds);
    }

    fn seeked(&self, seconds: f64) {
        self.emit_signal(PLAYER_IFACE, "Seeked", Variant::tuple_from_iter([((seconds * 1e6) as i64).to_variant()]));
    }

    fn emit_signal(&self, iface: &str, name: &str, params: Variant) {
        if let Some(c) = self.connection.borrow().as_ref() {
            if let Err(e) = c.emit_signal(None, PATH, iface, name, Some(&params)) {
                log::debug!("MPRIS {name}: {e}");
            }
        }
    }

    /// Sends the current values of `changed` and invalidates `invalidated`.
    fn properties_changed(&self, iface: &str, changed: &[&str], invalidated: &[&str]) {
        if self.connection.borrow().is_none() {
            return;
        }
        let dict = glib::VariantDict::new(None);
        for prop in changed {
            dict.insert_value(prop, &self.get(iface, prop));
        }
        let invalidated: Vec<String> = invalidated.iter().map(|s| s.to_string()).collect();
        self.emit_signal(
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Variant::tuple_from_iter([iface.to_variant(), dict.end(), invalidated.to_variant()]),
        );
    }
}

fn bad_args(invocation: gio::DBusMethodInvocation) {
    invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "unexpected arguments");
}

/// Seconds into the song, counting since the last time event while playing.
fn position(s: &State) -> f64 {
    let elapsed = s.position_at.map_or(0.0, |t| t.elapsed().as_secs_f64());
    let pos = s.position + elapsed;
    if s.duration > 0.0 {
        pos.min(s.duration)
    } else {
        pos
    }
}

fn track_path(generation: u32, index: usize) -> String {
    format!("{TRACK_PREFIX}{generation}_{index}")
}

fn track_ids(s: &State) -> Vec<ObjectPath> {
    (0..s.queue.len())
        .map(|i| ObjectPath::try_from(track_path(s.generation, i)).expect("valid object path"))
        .collect()
}

fn current_id(s: &State) -> ObjectPath {
    let path = s.current.map_or_else(|| NO_TRACK.to_owned(), |i| track_path(s.generation, i));
    ObjectPath::try_from(path).expect("valid object path")
}

/// Index into the current queue of a track id, if it is one of ours.
fn index_of(s: &State, id: &str) -> Option<usize> {
    let rest = id.strip_prefix(TRACK_PREFIX)?;
    let (generation, index) = rest.split_once('_')?;
    let index: usize = index.parse().ok()?;
    (generation.parse::<u32>().ok()? == s.generation && index < s.queue.len()).then_some(index)
}

/// Where the engine's song is in our queue: at its reported index if the
/// ids agree there (MusicKit may drop songs it can't play), else by id.
fn find_in_queue(queue: &[Track], id: &str, index: i64) -> Option<usize> {
    if let Ok(i) = usize::try_from(index) {
        if queue.get(i).is_some_and(|t| t.id == id) {
            return Some(i);
        }
    }
    queue.iter().position(|t| t.id == id)
}

fn metadata(track: &Track, id: &str, art_file: Option<&str>) -> Variant {
    let dict = glib::VariantDict::new(None);
    if let Ok(path) = ObjectPath::try_from(id.to_owned()) {
        dict.insert_value("mpris:trackid", &path.to_variant());
    }
    dict.insert_value("xesam:title", &track.title.to_variant());
    if !track.artist.is_empty() {
        dict.insert_value("xesam:artist", &vec![track.artist.clone()].to_variant());
    }
    if !track.album.is_empty() {
        dict.insert_value("xesam:album", &track.album.to_variant());
    }
    if track.duration > 0.0 {
        dict.insert_value("mpris:length", &((track.duration * 1e6) as i64).to_variant());
    }
    if let Some(url) = art_file.or(track.art_url.as_deref()) {
        dict.insert_value("mpris:artUrl", &url.to_variant());
    }
    dict.end()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track { id: id.into(), ..Default::default() }
    }

    #[test]
    fn finds_the_playing_song() {
        let queue = [track("a"), track("b"), track("c")];
        assert_eq!(find_in_queue(&queue, "b", 1), Some(1));
        // MusicKit dropped a song, so its index is one short
        assert_eq!(find_in_queue(&queue, "c", 1), Some(2));
        assert_eq!(find_in_queue(&queue, "x", 0), None);
        assert_eq!(find_in_queue(&queue, "a", -1), Some(0));
    }

    #[test]
    fn track_ids_belong_to_one_queue() {
        let s = State { queue: vec![track("a"), track("b")], generation: 7, ..Default::default() };
        assert_eq!(index_of(&s, &track_path(7, 1)), Some(1));
        assert_eq!(index_of(&s, &track_path(6, 1)), None);
        assert_eq!(index_of(&s, &track_path(7, 2)), None);
        assert_eq!(index_of(&s, NO_TRACK), None);
    }

    #[test]
    fn metadata_has_the_mpris_types() {
        let t = Track {
            id: "a".into(),
            title: "T".into(),
            artist: "A".into(),
            album: "B".into(),
            duration: 2.5,
            art_url: Some("https://x/y.jpg".into()),
        };
        let m = metadata(&t, &track_path(1, 0), Some("file:///tmp/a.jpg"));
        let d = glib::VariantDict::new(Some(&m));
        assert_eq!(d.lookup_value("mpris:trackid", None).unwrap().type_().as_str(), "o");
        assert_eq!(d.lookup_value("xesam:artist", None).unwrap().type_().as_str(), "as");
        assert_eq!(d.lookup_value("mpris:length", None).unwrap().get::<i64>(), Some(2_500_000));
        assert_eq!(d.lookup_value("mpris:artUrl", None).unwrap().get::<String>().as_deref(), Some("file:///tmp/a.jpg"));
    }
}
