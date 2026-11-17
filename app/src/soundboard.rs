//! The soundboard sidebar: a pad for every sound file in a folder. Tap a pad to play it (tap
//! again to stop); each sound can have its own name, volume, repeat and a light cue (pulse with
//! the sound, flash, a scene, an automation or blackout) that starts with it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gaffer_core::sound::{is_audio_file, nice_name};
use gaffer_core::{Action, LightCue, Rgb, Sound};
use gtk::{gdk, gio, glib};

use crate::audio::{Event, Player};
use crate::dialogs;
use crate::ui::{PRESETS, Ui, clear, rgb, rgba, swatch};

/// Pads with a keyboard shortcut (F1…F12) and a `gaffer ctl sound-N` action.
pub const KEYED: usize = 12;

struct Pad {
    file: String,
    button: gtk::Button,
    progress: gtk::ProgressBar,
}

pub struct Sounds {
    pub player: Player,
    pub split: adw::OverlaySplitView,
    pub toggle: gtk::ToggleButton,
    pads_box: gtk::Box,
    volume: gtk::Scale,
    stop_all: gtk::Button,
    folder_lbl: gtk::Label,
    pads: RefCell<Vec<Pad>>,
    /// Files of the pads, in order.
    shown: RefCell<Vec<String>>,
    /// file → the voice playing it
    playing: RefCell<HashMap<String, u64>>,
    monitor: RefCell<Option<(PathBuf, gio::FileMonitor)>>,
    rescan_queued: Cell<bool>,
    timer: Cell<bool>,
}

impl Sounds {
    pub fn new(content: &impl IsA<gtk::Widget>) -> Sounds {
        let panel = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).css_classes(["sound-panel"]).build();
        let head = gtk::Box::builder().spacing(4).build();
        head.append(&gtk::Label::builder().label("Sounds").xalign(0.0).hexpand(true).css_classes(["title-4"]).build());
        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add sounds… (copies them into the sounds folder)").css_classes(["flat"]).action_name("app.sounds-add").build();
        let menu = gio::Menu::new();
        menu.append(Some("Add sounds…"), Some("app.sounds-add"));
        menu.append(Some("Open the sounds folder"), Some("app.sounds-open-folder"));
        menu.append(Some("Use another folder…"), Some("app.sounds-choose-folder"));
        menu.append(Some("Look for new files"), Some("app.sounds-rescan"));
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).tooltip_text("Sound folder").css_classes(["flat"]).build();
        head.append(&add);
        head.append(&more);
        panel.append(&head);

        let vol_row = gtk::Box::builder().spacing(6).build();
        vol_row.append(&gtk::Image::from_icon_name("audio-volume-high-symbolic"));
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        volume.set_draw_value(false);
        volume.set_hexpand(true);
        volume.set_tooltip_text(Some("Soundboard volume"));
        vol_row.append(&volume);
        let stop_all = gtk::Button::builder().icon_name("media-playback-stop-symbolic").tooltip_text("Stop all sounds (Shift+Esc)").css_classes(["circular"]).sensitive(false).build();
        vol_row.append(&stop_all);
        panel.append(&vol_row);

        let pads_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
        let sc = gtk::ScrolledWindow::builder().child(&pads_box).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        panel.append(&sc);
        let foot = gtk::Box::builder().spacing(6).css_classes(["dim-label"]).build();
        foot.append(&gtk::Image::from_icon_name("folder-music-symbolic"));
        let folder_lbl = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).css_classes(["caption"]).build();
        foot.append(&folder_lbl);
        panel.append(&foot);
        panel.append(&gtk::Label::builder().label("Drop sound files here to add them").xalign(0.0).css_classes(["dim-label", "caption"]).build());

        let split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .min_sidebar_width(250.0)
            .max_sidebar_width(340.0)
            .sidebar_width_fraction(0.3)
            .content(content)
            .sidebar(&panel)
            .build();
        let toggle = gtk::ToggleButton::builder().icon_name("audio-x-generic-symbolic").tooltip_text("Soundboard (Ctrl+K)").build();
        toggle.bind_property("active", &split, "show-sidebar").bidirectional().sync_create().build();
        Sounds {
            player: Player::new(std::env::var("GAFFER_AUDIO_SINK").ok().as_deref().filter(|s| *s == "fakesink")),
            split,
            toggle,
            pads_box,
            volume,
            stop_all,
            folder_lbl,
            pads: RefCell::default(),
            shown: RefCell::default(),
            playing: RefCell::default(),
            monitor: RefCell::default(),
            rescan_queued: Cell::new(false),
            timer: Cell::new(false),
        }
    }

}

/// The pads' names, in order (for the Touch Bar and `gaffer --install-bar-scenes`).
pub fn pad_names(board: &gaffer_core::Soundboard) -> Vec<String> {
    let dir = if board.folder.is_empty() { default_folder() } else { PathBuf::from(&board.folder) };
    let files = list_audio(&dir).unwrap_or_default();
    let mut b = board.clone();
    b.sync_files(&files).iter().map(|&i| b.sounds[i].label()).collect()
}

pub fn default_folder() -> PathBuf {
    let music = glib::user_special_dir(glib::UserDirectory::Music).unwrap_or_else(|| glib::home_dir().join("Music"));
    music.join("Gaffer Sounds")
}

/// Audio files in a folder (None: the folder isn't there).
fn list_audio(dir: &Path) -> Option<Vec<String>> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut v: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_audio_file(n))
        .collect();
    v.sort();
    Some(v)
}

/// Copies a file into the folder under a free name; returns the new file name.
fn copy_into(dir: &Path, src: &Path) -> Result<String, String> {
    let name = src.file_name().and_then(|n| n.to_str()).ok_or("odd file name")?;
    if !is_audio_file(name) {
        return Err(format!("{name} isn't a sound file Gaffer knows"));
    }
    if src.parent() == Some(dir) {
        return Ok(name.to_string()); // already there
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let mut target = name.to_string();
    let mut n = 2;
    while dir.join(&target).exists() {
        target = format!("{stem} {n}.{ext}");
        n += 1;
    }
    std::fs::copy(src, dir.join(&target)).map_err(|e| e.to_string())?;
    Ok(target)
}

fn cue_text(ui: &Ui, cue: &LightCue) -> Option<(String, Option<Rgb>, &'static str)> {
    let e = ui.e();
    Some(match cue {
        LightCue::None => return None,
        LightCue::Pulse { color } => ("Pulse".into(), *color, "display-brightness-symbolic"),
        LightCue::Flash { color } => ("Flash".into(), Some(*color), "display-brightness-symbolic"),
        LightCue::Scene { id } => (e.show.scene(*id).map(|s| s.name.clone())?, None, "view-grid-symbolic"),
        LightCue::Automation { id } => {
            let a = e.show.automation(*id)?;
            let icon = match &a.action {
                Action::Effect(fx) => fx.kind.icon(),
                Action::Sequence { .. } => "media-playlist-repeat-symbolic",
            };
            (a.name.clone(), None, icon)
        }
        LightCue::Blackout => ("Blackout".into(), None, "weather-clear-night-symbolic"),
    })
}

impl Ui {
    pub fn sound_folder(&self) -> PathBuf {
        let f = self.e().show.sounds.folder.clone();
        if f.is_empty() { default_folder() } else { PathBuf::from(f) }
    }

    /// Wires up the sidebar (called once, after the window is built).
    pub fn setup_sounds(self: &Rc<Self>) {
        let (vol, open) = {
            let e = self.e();
            (e.show.sounds.volume, e.show.sounds.open)
        };
        self.snd.volume.set_value(vol as f64 * 100.0);
        self.snd.player.set_master(vol);
        self.snd.toggle.set_active(open);
        let u = self.clone();
        self.snd.volume.connect_value_changed(move |s| {
            let v = s.value() as f32 / 100.0;
            u.e().show.sounds.volume = v;
            u.snd.player.set_master(v);
            u.changed();
        });
        let u = self.clone();
        self.snd.toggle.connect_clicked(move |t| {
            u.e().show.sounds.open = t.is_active();
            u.changed();
        });
        let u = self.clone();
        self.snd.stop_all.connect_clicked(move |_| u.stop_all_sounds());

        // Drop sound files on the sidebar to add them.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let u = self.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Ok(list) = value.get::<gdk::FileList>() else { return false };
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            u.add_sound_files(&paths);
            true
        });
        if let Some(sidebar) = self.snd.split.sidebar() {
            sidebar.add_controller(drop);
        }
        self.rebuild_sounds();
    }

    pub fn toggle_sounds(&self) {
        let on = !self.snd.toggle.is_active();
        self.snd.toggle.set_active(on);
        self.e().show.sounds.open = on;
        self.changed();
    }

    fn watch_folder(self: &Rc<Self>, dir: &Path) {
        if self.snd.monitor.borrow().as_ref().is_some_and(|(d, _)| d == dir) {
            return;
        }
        *self.snd.monitor.borrow_mut() = None;
        if !dir.is_dir() {
            return;
        }
        let Ok(m) = gio::File::for_path(dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>) else { return };
        let w = Rc::downgrade(self);
        m.connect_changed(move |_, _, _, _| {
            let Some(u) = w.upgrade() else { return };
            if u.snd.rescan_queued.replace(true) {
                return;
            }
            let w = Rc::downgrade(&u);
            glib::timeout_add_local_once(Duration::from_millis(400), move || {
                if let Some(u) = w.upgrade() {
                    u.snd.rescan_queued.set(false);
                    u.rebuild_sounds();
                }
            });
        });
        *self.snd.monitor.borrow_mut() = Some((dir.to_path_buf(), m));
    }

    /// Reads the folder again and redraws the pads.
    pub fn rebuild_sounds(self: &Rc<Self>) {
        let dir = self.sound_folder();
        let files = list_audio(&dir);
        self.watch_folder(&dir);
        let short = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
        self.snd.folder_lbl.set_label(&short);
        self.snd.folder_lbl.set_tooltip_text(Some(&format!("Sounds come from {}", dir.display())));
        let sounds: Vec<Sound> = {
            let mut e = self.e();
            let before = e.show.sounds.sounds.len();
            let idx = e.show.sounds.sync_files(files.as_deref().unwrap_or(&[]));
            if e.show.sounds.sounds.len() != before {
                e.dirty = true;
            }
            idx.iter().map(|&i| e.show.sounds.sounds[i].clone()).collect()
        };
        // A playing sound whose file went away stops.
        let gone: Vec<String> = self.snd.playing.borrow().keys().filter(|f| !sounds.iter().any(|s| &s.file == *f)).cloned().collect();
        for f in gone {
            self.stop_sound(&f);
        }
        *self.snd.shown.borrow_mut() = sounds.iter().map(|s| s.file.clone()).collect();
        clear(&self.snd.pads_box);
        self.snd.pads.borrow_mut().clear();
        self.bar_due.set(true);

        if let Some(p) = &self.snd.player.problem {
            self.snd.pads_box.append(&adw::StatusPage::builder().icon_name("audio-volume-muted-symbolic").title("No sound").description(glib::markup_escape_text(p)).build());
            return;
        }
        if sounds.is_empty() {
            let text = if files.is_none() {
                "Add sound files (MP3, WAV, OGG, FLAC…) and they show up here as buttons. You can also drop files on this panel."
            } else {
                "This folder has no sound files yet. Add some, or drop them on this panel."
            };
            let st = adw::StatusPage::builder().icon_name("audio-x-generic-symbolic").title("Soundboard").description(text).css_classes(["compact"]).build();
            let bx = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).halign(gtk::Align::Center).build();
            bx.append(&gtk::Button::builder().label("Add sounds…").css_classes(["suggested-action", "pill"]).action_name("app.sounds-add").build());
            bx.append(&gtk::Button::builder().label("Use another folder…").css_classes(["flat"]).action_name("app.sounds-choose-folder").build());
            st.set_child(Some(&bx));
            self.snd.pads_box.append(&st);
            return;
        }
        let grid = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(2).max_children_per_line(2).row_spacing(8).column_spacing(8).valign(gtk::Align::Start).build();
        for (i, s) in sounds.iter().enumerate() {
            grid.append(&self.pad(s, i, sounds.len()));
        }
        let mut c = grid.first_child();
        while let Some(w) = c {
            w.set_focusable(false);
            c = w.next_sibling();
        }
        self.snd.pads_box.append(&grid);
        self.sync_pads();
    }

    fn pad(self: &Rc<Self>, s: &Sound, i: usize, n: usize) -> gtk::Widget {
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        let name = gtk::Label::builder()
            .label(s.label())
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            .margin_start(14)
            .margin_end(14)
            .vexpand(true)
            .css_classes(["heading"])
            .build();
        body.append(&name);
        let info = gtk::Box::builder().spacing(4).halign(gtk::Align::Center).build();
        if i < KEYED {
            info.append(&gtk::Label::builder().label(format!("F{}", i + 1)).css_classes(["badge"]).tooltip_text(format!("Key F{} · “gaffer ctl sound-{}”", i + 1, i + 1)).build());
        }
        if let Some((text, color, icon)) = cue_text(self, &s.cue) {
            match color {
                Some(c) => info.append(&swatch(c, 12)),
                None => info.append(&gtk::Image::from_icon_name(icon)),
            }
            info.append(&gtk::Label::builder().label(&text).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(10).css_classes(["caption", "dim-label"]).build());
        }
        if s.repeat {
            info.append(&gtk::Image::builder().icon_name("media-playlist-repeat-symbolic").tooltip_text("Repeats").build());
        }
        body.append(&info);
        let progress = gtk::ProgressBar::builder().css_classes(["pad-progress"]).build();
        body.append(&progress);
        let button = gtk::Button::builder().child(&body).css_classes(["card", "sound-pad"]).tooltip_text(format!("{} — tap to play, tap again to stop. Right-click for options.", s.file)).build();
        let (u, file) = (self.clone(), s.file.clone());
        button.connect_clicked(move |_| u.sound_press(&file));

        let menu = self.pad_menu(&s.file, i, n);
        let right = gtk::GestureClick::builder().button(3).build();
        let m = menu.clone();
        right.connect_pressed(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            m.popup();
        });
        button.add_controller(right);
        let ov = gtk::Overlay::builder().child(&button).build();
        ov.add_overlay(&menu);
        self.snd.pads.borrow_mut().push(Pad { file: s.file.clone(), button, progress });
        ov.upcast()
    }

    fn pad_menu(self: &Rc<Self>, file: &str, i: usize, n: usize) -> gtk::MenuButton {
        let pop = gtk::Popover::new();
        let bx = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let item = |label: &str, enabled: bool| {
            let b = gtk::Button::builder().label(label).css_classes(["flat"]).sensitive(enabled).build();
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            bx.append(&b);
            b
        };
        let rename = item("Rename…", true);
        let settings = item("Volume and lights…", true);
        let earlier = item("Move earlier", i > 0);
        let later = item("Move later", i + 1 < n);
        let trash = item("Move file to Trash…", true);
        pop.set_child(Some(&bx));
        let mb = gtk::MenuButton::builder().icon_name("view-more-symbolic").popover(&pop).css_classes(["flat", "circular", "pad-menu"]).halign(gtk::Align::End).valign(gtk::Align::Start).margin_top(2).margin_end(2).tooltip_text("Sound options").build();
        let f = file.to_string();
        let (u, p, f2) = (self.clone(), pop.clone(), f.clone());
        rename.connect_clicked(move |_| {
            p.popdown();
            u.rename_sound(&f2);
        });
        let (u, p, f2) = (self.clone(), pop.clone(), f.clone());
        settings.connect_clicked(move |_| {
            p.popdown();
            sound_dialog(&u, &f2);
        });
        for (b, d) in [(earlier, -1), (later, 1)] {
            let (u, p, f2) = (self.clone(), pop.clone(), f.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                let shown = u.snd.shown.borrow().clone();
                u.e().show.sounds.move_sound(&f2, d, &shown);
                u.changed();
                u.rebuild_sounds();
            });
        }
        let (u, p, f2) = (self.clone(), pop.clone(), f.clone());
        trash.connect_clicked(move |_| {
            p.popdown();
            let path = u.sound_folder().join(&f2);
            let (u2, f3) = (u.clone(), f2.clone());
            dialogs::confirm(&u, &format!("Move “{f2}” to the Trash?"), "The file goes to the Trash, so you can get it back from there.", "Move to Trash", move || {
                u2.stop_sound(&f3);
                match gio::File::for_path(&path).trash(None::<&gio::Cancellable>) {
                    Ok(()) => u2.rebuild_sounds(),
                    Err(e) => u2.toast(&format!("Couldn't move it to the Trash: {e}")),
                }
            });
        });
        mb
    }

    pub fn rename_sound(self: &Rc<Self>, file: &str) {
        let Some(old) = self.e().show.sounds.get(file).map(|s| s.label()) else { return };
        let (u, f) = (self.clone(), file.to_string());
        dialogs::ask_name(self, "Rename sound", &old, "Rename", move |name| {
            if let Some(s) = u.e().show.sounds.get_mut(&f) {
                let name = name.trim().chars().take(60).collect::<String>();
                s.name = if name == nice_name(&f) { String::new() } else { name };
            }
            u.changed();
            u.rebuild_sounds();
        });
    }

    // ---- playing ---------------------------------------------------------------------------

    /// A pad was pressed: play it, or stop it if it's playing.
    pub fn sound_press(self: &Rc<Self>, file: &str) {
        if self.snd.playing.borrow().contains_key(file) {
            self.stop_sound(file);
            return;
        }
        let Some(s) = self.e().show.sounds.get(file).cloned() else { return };
        self.play_sound(&s);
    }

    /// The n-th pad (1-based), for F-keys, the Touch Bar and `gaffer ctl sound-N`.
    pub fn sound_nth(self: &Rc<Self>, n: usize) {
        let f = self.snd.shown.borrow().get(n.wrapping_sub(1)).cloned();
        match f {
            Some(f) => self.sound_press(&f),
            None => self.toast(&format!("There's no sound {n}")),
        }
    }

    /// Plays a sound with these settings (the pad's, or the editor's while trying it).
    pub fn play_sound(self: &Rc<Self>, s: &Sound) {
        self.stop_sound(&s.file);
        let path = self.sound_folder().join(&s.file);
        match self.snd.player.play(&path, s.volume, s.repeat) {
            Ok(id) => {
                self.snd.playing.borrow_mut().insert(s.file.clone(), id);
                self.e().cue_start(id, &s.cue, s.restore);
                self.drain_notices();
                self.start_sound_timer();
            }
            Err(e) => self.toast(&format!("Couldn't play {}: {e}", s.label())),
        }
        self.sync_pads();
    }

    pub fn stop_sound(&self, file: &str) {
        let id = self.snd.playing.borrow_mut().remove(file);
        if let Some(id) = id {
            self.snd.player.stop(id);
            self.e().cue_end(id);
        }
        self.sync_pads();
    }

    pub fn stop_all_sounds(&self) {
        let all: Vec<(String, u64)> = self.snd.playing.borrow_mut().drain().collect();
        for (_, id) in all {
            self.snd.player.stop(id);
            self.e().cue_end(id);
        }
        self.sync_pads();
    }

    fn sync_pads(&self) {
        let playing = self.snd.playing.borrow();
        for p in self.snd.pads.borrow().iter() {
            let on = playing.contains_key(&p.file);
            if p.button.has_css_class("playing") != on {
                if on { p.button.add_css_class("playing") } else { p.button.remove_css_class("playing") }
            }
            if !on && p.progress.fraction() != 0.0 {
                p.progress.set_fraction(0.0);
            }
        }
        self.snd.stop_all.set_sensitive(!playing.is_empty());
    }

    /// While sounds play: hand their loudness to the lights, notice when they end, move the
    /// progress bars.
    fn start_sound_timer(self: &Rc<Self>) {
        if self.snd.timer.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(20), move || {
            let Some(u) = w.upgrade() else { return glib::ControlFlow::Break };
            for ev in u.snd.player.poll() {
                match ev {
                    Event::Level(id, v) => u.e().cue_level(id, v),
                    Event::Ended(id) | Event::Failed(id, _) => {
                        let file = u.snd.playing.borrow().iter().find(|(_, v)| **v == id).map(|(f, _)| f.clone());
                        if let Some(f) = &file {
                            u.snd.playing.borrow_mut().remove(f);
                        }
                        u.e().cue_end(id);
                        if let (Event::Failed(_, msg), Some(f)) = (&ev, &file) {
                            u.toast(&format!("Couldn't play {f}: {msg}"));
                        }
                        u.sync_pads();
                    }
                }
            }
            let playing = u.snd.playing.borrow().clone();
            for p in u.snd.pads.borrow().iter() {
                if let Some(&id) = playing.get(&p.file) {
                    match u.snd.player.progress(id) {
                        Some(x) => p.progress.set_fraction(x),
                        None => p.progress.pulse(),
                    }
                }
            }
            if playing.is_empty() && !u.snd.player.any_playing() {
                u.snd.timer.set(false);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    // ---- adding sounds ---------------------------------------------------------------------

    pub fn add_sound_files(self: &Rc<Self>, paths: &[PathBuf]) {
        let dir = self.sound_folder();
        let (mut ok, mut bad) = (0, Vec::new());
        for p in paths {
            match copy_into(&dir, p) {
                Ok(_) => ok += 1,
                Err(e) => bad.push(e),
            }
        }
        if ok > 0 {
            self.toast(&format!("Added {ok} sound{}", if ok == 1 { "" } else { "s" }));
        }
        if let Some(e) = bad.first() {
            self.toast(e);
        }
        self.rebuild_sounds();
        if !self.snd.toggle.is_active() {
            self.toggle_sounds();
        }
    }

    pub fn add_sounds_dialog(self: &Rc<Self>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Sound files"));
        filter.add_mime_type("audio/*");
        for ext in gaffer_core::sound::AUDIO_EXTS {
            filter.add_suffix(ext);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let d = gtk::FileDialog::builder().title("Add sounds").accept_label("Add").filters(&filters).default_filter(&filter).modal(true).build();
        let u = self.clone();
        d.open_multiple(Some(&self.win), None::<&gio::Cancellable>, move |r| {
            let Ok(list) = r else { return };
            let paths: Vec<PathBuf> = (0..list.n_items()).filter_map(|i| list.item(i).and_downcast::<gio::File>()).filter_map(|f| f.path()).collect();
            u.add_sound_files(&paths);
        });
    }

    pub fn choose_sound_folder(self: &Rc<Self>) {
        let d = gtk::FileDialog::builder().title("Folder with your sounds").accept_label("Use this folder").modal(true).initial_folder(&gio::File::for_path(self.sound_folder())).build();
        let u = self.clone();
        d.select_folder(Some(&self.win), None::<&gio::Cancellable>, move |r| {
            let Some(path) = r.ok().and_then(|f| f.path()) else { return };
            u.stop_all_sounds();
            u.e().show.sounds.folder = if path == default_folder() { String::new() } else { path.display().to_string() };
            u.changed();
            u.rebuild_sounds();
            if !u.snd.toggle.is_active() {
                u.toggle_sounds();
            }
        });
    }

    pub fn open_sound_folder(self: &Rc<Self>) {
        let dir = self.sound_folder();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.toast(&format!("Couldn't make {}: {e}", dir.display()));
            return;
        }
        self.rebuild_sounds();
        let u = self.clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(Some(&self.win), None::<&gio::Cancellable>, move |r| {
            if let Err(e) = r {
                u.toast(&format!("Couldn't open the folder: {e}"));
            }
        });
    }
}

// ---- the sound's settings ---------------------------------------------------------------------

/// Which cue each entry of the "When it plays" list means.
#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Nothing,
    Pulse,
    Flash,
    Blackout,
    Scene(u32),
    Automation(u32),
}

pub fn sound_dialog(ui: &Rc<Ui>, file: &str) {
    let Some(orig) = ui.e().show.sounds.get(file).cloned() else { return };
    let (scenes, autos) = {
        let e = ui.e();
        (e.show.scenes.iter().map(|s| (s.id, s.name.clone())).collect::<Vec<_>>(), e.show.automations.iter().map(|a| (a.id, a.name.clone())).collect::<Vec<_>>())
    };
    let d = adw::Dialog::builder().title(orig.label()).content_width(460).content_height(620).build();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let page = adw::PreferencesPage::new();
    let tb = adw::ToolbarView::new();
    tb.add_top_bar(&header);
    tb.set_content(Some(&page));
    d.set_child(Some(&tb));

    let g = adw::PreferencesGroup::builder().title("Sound").description(glib::markup_escape_text(&orig.file)).build();
    let name = adw::EntryRow::builder().title("Name on the button").text(orig.label()).build();
    g.add(&name);
    let vol = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    vol.set_value(orig.volume as f64 * 100.0);
    vol.set_draw_value(false);
    vol.set_hexpand(true);
    vol.set_size_request(170, -1);
    let vrow = adw::ActionRow::builder().title("Volume").build();
    vrow.add_suffix(&vol);
    g.add(&vrow);
    let repeat = adw::SwitchRow::builder().title("Repeat").subtitle("Plays again until you stop it").active(orig.repeat).build();
    g.add(&repeat);
    page.add(&g);

    let lg = adw::PreferencesGroup::builder().title("Lights").description("What the lights do when this sound plays").build();
    let mut picks = vec![(Pick::Nothing, "Nothing".to_string()), (Pick::Pulse, "Pulse with the sound".into()), (Pick::Flash, "Flash".into()), (Pick::Blackout, "Blackout".into())];
    picks.extend(scenes.iter().map(|(id, n)| (Pick::Scene(*id), format!("Scene: {n}"))));
    picks.extend(autos.iter().map(|(id, n)| (Pick::Automation(*id), format!("Automation: {n}"))));
    let model = gtk::StringList::new(&picks.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>());
    let cur = match orig.cue {
        LightCue::None => Pick::Nothing,
        LightCue::Pulse { .. } => Pick::Pulse,
        LightCue::Flash { .. } => Pick::Flash,
        LightCue::Blackout => Pick::Blackout,
        LightCue::Scene { id } => Pick::Scene(id),
        LightCue::Automation { id } => Pick::Automation(id),
    };
    let combo = adw::ComboRow::builder().title("When it plays").model(&model).selected(picks.iter().position(|(p, _)| *p == cur).unwrap_or(0) as u32).build();
    lg.add(&combo);

    let start_color = match orig.cue {
        LightCue::Pulse { color: Some(c) } | LightCue::Flash { color: c } => c,
        _ => Rgb::WHITE,
    };
    let color = Rc::new(Cell::new(start_color));
    let own = adw::SwitchRow::builder().title("Keep each light's colour").subtitle("Only the brightness follows the sound").active(matches!(orig.cue, LightCue::Pulse { color: None })).build();
    lg.add(&own);
    let crow = adw::ActionRow::builder().title("Colour").build();
    let sw_box = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(6).min_children_per_line(5).valign(gtk::Align::Center).build();
    let picker = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).title("Light colour").build()));
    picker.set_rgba(&rgba(start_color));
    picker.set_valign(gtk::Align::Center);
    for (n, c) in PRESETS {
        let b = gtk::Button::builder().child(&swatch(*c, 18)).tooltip_text(*n).css_classes(["flat", "circular"]).build();
        let (cc, p) = (*c, picker.clone());
        b.connect_clicked(move |_| p.set_rgba(&rgba(cc)));
        sw_box.append(&b);
    }
    let cl = color.clone();
    picker.connect_rgba_notify(move |p| cl.set(rgb(&p.rgba())));
    crow.add_suffix(&sw_box);
    crow.add_suffix(&picker);
    lg.add(&crow);
    let restore = adw::SwitchRow::builder().title("Put the lights back when it ends").subtitle("Or when you stop it. Off: the lights stay as the cue left them.").active(orig.restore).build();
    lg.add(&restore);
    page.add(&lg);

    let tg = adw::PreferencesGroup::new();
    let try_btn = gtk::Button::builder().label("Try it").css_classes(["pill"]).halign(gtk::Align::Center).tooltip_text("Plays the sound with these settings").build();
    tg.add(&try_btn);
    page.add(&tg);

    let picks = Rc::new(picks);
    let show_rows = {
        let (combo, own, crow, restore, picks) = (combo.clone(), own.clone(), crow.clone(), restore.clone(), picks.clone());
        move || {
            let p = picks.get(combo.selected() as usize).map(|x| x.0).unwrap_or(Pick::Nothing);
            own.set_visible(p == Pick::Pulse);
            crow.set_visible(p == Pick::Flash || (p == Pick::Pulse && !own.is_active()));
            restore.set_visible(matches!(p, Pick::Scene(_) | Pick::Automation(_) | Pick::Blackout));
        }
    };
    show_rows();
    let s2 = show_rows.clone();
    combo.connect_selected_notify(move |_| s2());
    own.connect_active_notify(move |_| show_rows());

    let draft = {
        let (name, vol, repeat, combo, own, restore, color, picks, orig) = (name.clone(), vol.clone(), repeat.clone(), combo.clone(), own.clone(), restore.clone(), color.clone(), picks.clone(), orig.clone());
        move || -> Sound {
            let n: String = name.text().trim().chars().take(60).collect();
            let cue = match picks.get(combo.selected() as usize).map(|x| x.0).unwrap_or(Pick::Nothing) {
                Pick::Nothing => LightCue::None,
                Pick::Pulse => LightCue::Pulse { color: (!own.is_active()).then(|| color.get()) },
                Pick::Flash => LightCue::Flash { color: color.get() },
                Pick::Blackout => LightCue::Blackout,
                Pick::Scene(id) => LightCue::Scene { id },
                Pick::Automation(id) => LightCue::Automation { id },
            };
            Sound {
                file: orig.file.clone(),
                name: if n == nice_name(&orig.file) { String::new() } else { n },
                volume: vol.value() as f32 / 100.0,
                repeat: repeat.is_active(),
                cue,
                restore: restore.is_active(),
            }
        }
    };
    let tried = Rc::new(Cell::new(false));
    {
        let (u, draft, tried, file) = (ui.clone(), draft.clone(), tried.clone(), orig.file.clone());
        try_btn.connect_clicked(move |_| {
            if u.snd.playing.borrow().contains_key(&file) {
                u.stop_sound(&file);
            } else {
                tried.set(true);
                u.play_sound(&draft());
            }
        });
    }
    {
        let (u, file) = (ui.clone(), orig.file.clone());
        vol.connect_value_changed(move |s| {
            if let Some(&id) = u.snd.playing.borrow().get(&file) {
                u.snd.player.set_volume(id, s.value() as f32 / 100.0);
            }
        });
    }
    let saved = Rc::new(Cell::new(false));
    {
        let (u, d2, saved) = (ui.clone(), d.clone(), saved.clone());
        save.connect_clicked(move |_| {
            let s = draft();
            if let Some(slot) = u.e().show.sounds.get_mut(&s.file) {
                *slot = s;
            }
            saved.set(true);
            u.changed();
            u.rebuild_sounds();
            d2.close();
        });
    }
    let d2 = d.clone();
    cancel.connect_clicked(move |_| {
        d2.close();
    });
    let (u, file) = (ui.clone(), orig.file.clone());
    d.connect_closed(move |_| {
        // A try that's still playing stops with the editor (unless it was saved).
        if tried.get() && !saved.get() {
            u.stop_sound(&file);
        }
    });
    d.present(Some(&ui.win));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_finds_free_names() {
        let base = std::env::temp_dir().join(format!("gaffer-snd-{}", std::process::id()));
        let (src, dir) = (base.join("src"), base.join("Sounds"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("boom.wav"), b"x").unwrap();
        std::fs::write(src.join("notes.txt"), b"x").unwrap();
        assert_eq!(copy_into(&dir, &src.join("boom.wav")).unwrap(), "boom.wav");
        assert_eq!(copy_into(&dir, &src.join("boom.wav")).unwrap(), "boom 2.wav");
        assert_eq!(copy_into(&dir, &dir.join("boom.wav")).unwrap(), "boom.wav");
        assert!(copy_into(&dir, &src.join("notes.txt")).is_err());
        assert_eq!(list_audio(&dir).unwrap(), vec!["boom 2.wav".to_string(), "boom.wav".to_string()]);
        assert!(list_audio(&base.join("nope")).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
