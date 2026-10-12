//! Bottom bar: artwork, now playing, transport controls, position, volume.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::playback::NowPlaying;
use crate::song_table::format_duration;

pub const ARTWORK_SIZE: u32 = 48;

#[derive(Clone)]
pub struct PlayerBar(Rc<Inner>);

struct Inner {
    root: gtk::Box,
    artwork: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
    play: gtk::Button,
    position: gtk::Scale,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    volume: gtk::Scale,
    duration: Cell<f64>,
}

/// Transport button handlers, wired by the window.
pub struct Controls {
    pub toggle: Box<dyn Fn()>,
    pub next: Box<dyn Fn()>,
    pub previous: Box<dyn Fn()>,
    pub seek: Box<dyn Fn(f64)>,
    pub volume: Box<dyn Fn(f64)>,
}

impl PlayerBar {
    pub fn new(controls: Controls) -> Self {
        let artwork = gtk::Image::builder()
            .icon_name("audio-x-generic-symbolic")
            .pixel_size(ARTWORK_SIZE as i32)
            .css_classes(["card"])
            .overflow(gtk::Overflow::Hidden)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .label("Not Playing")
            .build();
        let subtitle = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label", "caption"])
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .width_request(220)
            .build();
        text.append(&title);
        text.append(&subtitle);

        let button = |icon: &str, tip: &str| {
            gtk::Button::builder().icon_name(icon).tooltip_text(tip).css_classes(["flat", "circular"]).build()
        };
        let previous = button("media-skip-backward-symbolic", "Previous (Ctrl+B)");
        let play = button("media-playback-start-symbolic", "Play (Ctrl+Space)");
        play.add_css_class("suggested-action");
        let next = button("media-skip-forward-symbolic", "Next (Ctrl+N)");
        let transport = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).build();
        transport.append(&previous);
        transport.append(&play);
        transport.append(&next);

        let position = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
        position.set_hexpand(true);
        position.set_sensitive(false);
        let elapsed =
            gtk::Label::builder().css_classes(["numeric", "caption"]).width_chars(5).xalign(1.0).build();
        let remaining =
            gtk::Label::builder().css_classes(["numeric", "caption"]).width_chars(6).xalign(0.0).build();
        let progress = gtk::Box::builder().spacing(6).hexpand(true).valign(gtk::Align::Center).build();
        progress.append(&elapsed);
        progress.append(&position);
        progress.append(&remaining);

        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.05);
        volume.set_value(1.0);
        volume.set_width_request(100);
        volume.set_valign(gtk::Align::Center);
        let volume_icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");

        let root = gtk::Box::builder()
            .spacing(12)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        root.append(&artwork);
        root.append(&text);
        root.append(&transport);
        root.append(&progress);
        root.append(&volume_icon);
        root.append(&volume);

        let Controls { toggle, next: on_next, previous: on_previous, seek, volume: on_volume } = controls;
        play.connect_clicked(move |_| toggle());
        next.connect_clicked(move |_| on_next());
        previous.connect_clicked(move |_| on_previous());
        // change-value only fires for user input, not for set_value below.
        position.connect_change_value(move |_, _, value| {
            seek(value);
            glib::Propagation::Proceed
        });
        volume.connect_value_changed(move |s| on_volume(s.value()));

        PlayerBar(Rc::new(Inner {
            root,
            artwork,
            title,
            subtitle,
            play,
            position,
            elapsed,
            remaining,
            volume,
            duration: Cell::new(0.0),
        }))
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.0.root
    }

    pub fn set_playing(&self, playing: bool) {
        let (icon, tip) = if playing {
            ("media-playback-pause-symbolic", "Pause (Ctrl+Space)")
        } else {
            ("media-playback-start-symbolic", "Play (Ctrl+Space)")
        };
        self.0.play.set_icon_name(icon);
        self.0.play.set_tooltip_text(Some(tip));
    }

    pub fn set_now_playing(&self, item: Option<&NowPlaying>) {
        let i = &self.0;
        match item {
            Some(n) => {
                i.title.set_text(&n.title);
                let sub = [n.artist.as_str(), n.album.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" — ");
                i.subtitle.set_text(&sub);
                self.set_time(0.0, n.duration);
            }
            None => {
                i.title.set_text("Not Playing");
                i.subtitle.set_text("");
                self.set_time(0.0, 0.0);
            }
        }
        i.artwork.set_icon_name(Some("audio-x-generic-symbolic"));
    }

    pub fn set_artwork(&self, bytes: &[u8]) {
        if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
            self.0.artwork.set_paintable(Some(&texture));
        }
    }

    pub fn set_time(&self, position: f64, duration: f64) {
        let i = &self.0;
        if duration > 0.0 && (duration - i.duration.get()).abs() > 0.5 {
            i.position.set_range(0.0, duration);
        }
        i.duration.set(duration);
        i.position.set_sensitive(duration > 0.0);
        i.position.set_value(position.min(duration.max(0.0)));
        i.elapsed.set_text(&format_duration(position.max(0.0)).if_empty("0:00"));
        let left = (duration - position).max(0.0);
        i.remaining.set_text(&if duration > 0.0 {
            format!("-{}", format_duration(left).if_empty("0:00"))
        } else {
            String::new()
        });
    }

    pub fn volume(&self) -> f64 {
        self.0.volume.value()
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}
