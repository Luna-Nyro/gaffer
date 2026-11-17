//! The Gaffer window: Lights, Scenes, Automations and Setup pages, with Master and Blackout
//! always at the bottom.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gaffer_core::{Action, Engine, Fixture, Look, Rgb, Running};
use gaffer_output::{Kind, Link, Output, Port, Target as OutTarget};
use gtk::{gdk, glib};

use crate::bar::{self, Ctl};
use crate::dialogs;
use crate::store;

pub const PRESETS: &[(&str, Rgb)] = &[
    ("Warm white", Rgb::new(1.0, 0.62, 0.3)),
    ("White", Rgb::WHITE),
    ("Red", Rgb::new(1.0, 0.0, 0.0)),
    ("Orange", Rgb::new(1.0, 0.4, 0.0)),
    ("Yellow", Rgb::new(1.0, 0.85, 0.0)),
    ("Green", Rgb::new(0.0, 1.0, 0.1)),
    ("Cyan", Rgb::new(0.0, 0.9, 1.0)),
    ("Blue", Rgb::new(0.0, 0.15, 1.0)),
    ("Purple", Rgb::new(0.55, 0.0, 1.0)),
    ("Pink", Rgb::new(1.0, 0.1, 0.55)),
];

const CSS: &str = "
.light-card { padding: 14px; }
.scene-tile { padding: 10px; min-width: 150px; }
.scene-tile.active { box-shadow: inset 0 0 0 3px @accent_bg_color; }
.stage { background: #141210; border-radius: 12px; padding: 10px; }
.stage label { color: #d8d0c4; font-size: 0.85em; }
.bottom-bar { padding: 6px 12px; }
.badge { font-size: 0.8em; font-weight: bold; padding: 0 6px; border-radius: 99px; background: alpha(currentColor, 0.12); }
.blackout:checked { background: @destructive_bg_color; color: @destructive_fg_color; }
.sound-panel { padding: 12px 12px 8px 12px; }
.sound-pad { padding: 8px 8px 6px 8px; min-height: 64px; }
.sound-pad.playing { box-shadow: inset 0 0 0 3px @accent_bg_color; }
.pad-progress trough, .pad-progress progress { min-height: 3px; }
.pad-menu { min-width: 24px; min-height: 24px; padding: 0; opacity: 0.6; }
";

pub fn rgba(c: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(c.r, c.g, c.b, 1.0)
}

pub fn rgb(c: &gdk::RGBA) -> Rgb {
    Rgb::new(c.red(), c.green(), c.blue()).clamped()
}

/// A filled circle of a colour.
pub fn swatch(color: Rgb, size: i32) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::builder().content_width(size).content_height(size).valign(gtk::Align::Center).build();
    da.set_draw_func(move |_, cr, w, h| {
        let r = (w.min(h) as f64) / 2.0 - 1.0;
        cr.arc(w as f64 / 2.0, h as f64 / 2.0, r, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(color.r as f64, color.g as f64, color.b as f64);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.5, 0.5, 0.5, 0.45);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    da
}

fn swatch_button(name: &str, color: Rgb) -> gtk::Button {
    gtk::Button::builder().child(&swatch(color, 20)).tooltip_text(name).css_classes(["flat", "circular"]).build()
}

/// A glowing lamp for the on-screen stage.
fn lamp(cell: Rc<Cell<Rgb>>) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::builder().content_width(60).content_height(60).halign(gtk::Align::Center).build();
    da.set_draw_func(move |_, cr, w, h| {
        let c = cell.get();
        let (x, y) = (w as f64 / 2.0, h as f64 / 2.0);
        let lum = c.r.max(c.g).max(c.b) as f64;
        // Body, then the light's glow on top.
        cr.arc(x, y, 14.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.22, 0.21, 0.2);
        let _ = cr.fill();
        if lum > 0.004 {
            let g = gtk::cairo::RadialGradient::new(x, y, 2.0, x, y, 29.0);
            let (r, gr, b) = (c.r as f64 / lum, c.g as f64 / lum, c.b as f64 / lum);
            g.add_color_stop_rgba(0.0, 0.6 + 0.4 * r, 0.6 + 0.4 * gr, 0.6 + 0.4 * b, lum);
            g.add_color_stop_rgba(0.35, r, gr, b, 0.85 * lum);
            g.add_color_stop_rgba(1.0, r, gr, b, 0.0);
            let _ = cr.set_source(&g);
            cr.arc(x, y, 29.0, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        cr.arc(x, y, 14.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.12);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
    });
    da
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn page_box() -> gtk::Box {
    gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(18).margin_bottom(24).margin_start(16).margin_end(16).build()
}

fn scrolled(b: &gtk::Box, width: i32) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(width).child(b).build();
    gtk::ScrolledWindow::builder().child(&clamp).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build()
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["dim-label"]).build()
}

fn pct(v: f32) -> String {
    format!("{}%", (v * 100.0).round() as i32)
}

pub fn channels_text(f: &Fixture) -> String {
    if f.width() == 1 { format!("Channel {}", f.address) } else { format!("Channels {}–{}", f.address, f.last_address()) }
}

/// The controls of one light (or of all lights).
struct Card {
    id: Option<u32>,
    scale: gtk::Scale,
    pct: gtk::Label,
    switch: gtk::Switch,
    color: Option<gtk::ColorDialogButton>,
    shown: Cell<Look>,
    last_on: Cell<f32>,
}

pub struct Ui {
    pub eng: Arc<Mutex<Engine>>,
    pub out: Output,
    pub win: adw::ApplicationWindow,
    pub stack: adw::ViewStack,
    toasts: adw::ToastOverlay,
    lights: gtk::Box,
    scenes: gtk::Box,
    autos: gtk::Box,
    setup: gtk::Box,
    cards: RefCell<Vec<Rc<Card>>>,
    lamps: RefCell<Vec<(u32, gtk::DrawingArea, Rc<Cell<Rgb>>)>>,
    scene_tiles: RefCell<Vec<(u32, gtk::Button)>>,
    auto_rows: RefCell<Vec<(u32, gtk::Button, adw::ActionRow, String)>>,
    master: gtk::Scale,
    master_pct: gtk::Label,
    blackout: gtk::ToggleButton,
    now_box: gtk::Box,
    now_lbl: gtk::Label,
    status_icon: gtk::Image,
    status_lbl: gtk::Label,
    status_row: RefCell<Option<adw::ActionRow>>,
    syncing: Cell<bool>,
    save_due: Cell<Option<Instant>>,
    last_status: Cell<Option<Instant>>,
    pub(crate) bar_due: Cell<bool>,
    bar_switch: RefCell<Option<adw::SwitchRow>>,
    pub ports: RefCell<Vec<Port>>,
    pub snd: crate::soundboard::Sounds,
}

impl Ui {
    pub fn e(&self) -> MutexGuard<'_, Engine> {
        self.eng.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn toast(&self, text: &str) {
        let t = adw::Toast::new(&glib::markup_escape_text(text));
        t.set_timeout(3);
        self.toasts.add_toast(t);
    }

    pub fn build(app: &adw::Application, eng: Arc<Mutex<Engine>>, out: Output) -> Rc<Ui> {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        if let Some(d) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }

        let stack = adw::ViewStack::new();
        let (lights, scenes, autos, setup) = (page_box(), page_box(), page_box(), page_box());
        stack.add_titled_with_icon(&scrolled(&lights, 1100), Some("lights"), "Lights", "display-brightness-symbolic");
        stack.add_titled_with_icon(&scrolled(&scenes, 1100), Some("scenes"), "Scenes", "view-grid-symbolic");
        stack.add_titled_with_icon(&scrolled(&autos, 760), Some("automations"), "Automations", "media-playlist-repeat-symbolic");
        stack.add_titled_with_icon(&scrolled(&setup, 760), Some("setup"), "Setup", "emblem-system-symbolic");

        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
        header.set_title_widget(Some(&switcher));

        // Bottom: master, blackout, what's playing, the interface.
        let master = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        master.set_draw_value(false);
        master.set_hexpand(true);
        master.set_size_request(140, -1);
        master.set_tooltip_text(Some("Master: scales every light (Ctrl+Up / Ctrl+Down)"));
        let master_pct = gtk::Label::builder().width_chars(4).xalign(1.0).css_classes(["numeric"]).build();
        let blackout = gtk::ToggleButton::builder().label("Blackout").tooltip_text("All lights dark at once (Ctrl+B)").css_classes(["blackout"]).build();
        let now_lbl = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(22).build();
        let stop = gtk::Button::builder().icon_name("media-playback-stop-symbolic").tooltip_text("Stop the automation (Ctrl+.)").css_classes(["flat", "circular"]).build();
        let now_box = gtk::Box::builder().spacing(6).visible(false).build();
        now_box.append(&gtk::Image::from_icon_name("media-playback-start-symbolic"));
        now_box.append(&now_lbl);
        now_box.append(&stop);
        let status_icon = gtk::Image::from_icon_name("computer-symbolic");
        let status_lbl = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(18).build();
        let sbox = gtk::Box::builder().spacing(6).build();
        sbox.append(&status_icon);
        sbox.append(&status_lbl);
        let status_btn = gtk::Button::builder().child(&sbox).css_classes(["flat"]).tooltip_text("DMX interface: open Setup").build();
        let bottom = gtk::Box::builder().spacing(10).css_classes(["bottom-bar"]).build();
        bottom.append(&gtk::Image::from_icon_name("display-brightness-symbolic"));
        bottom.append(&gtk::Label::new(Some("Master")));
        bottom.append(&master);
        bottom.append(&master_pct);
        bottom.append(&blackout);
        bottom.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        bottom.append(&now_box);
        bottom.append(&status_btn);

        let snd = crate::soundboard::Sounds::new(&stack);
        header.pack_end(&snd.toggle);
        let tb = adw::ToolbarView::new();
        tb.add_top_bar(&header);
        tb.add_bottom_bar(&bottom);
        tb.set_bottom_bar_style(adw::ToolbarStyle::Raised);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&snd.split));
        tb.set_content(Some(&toasts));
        let win = adw::ApplicationWindow::builder().application(app).title("Gaffer – Lights").default_width(1000).default_height(720).content(&tb).build();
        win.set_size_request(380, 400);
        // Narrow window: the soundboard slides over the page instead of beside it.
        if let Ok(cond) = adw::BreakpointCondition::parse("max-width: 820sp") {
            let bp = adw::Breakpoint::new(cond);
            bp.add_setter(&snd.split, "collapsed", Some(&true.to_value()));
            win.add_breakpoint(bp);
        }

        let ui = Rc::new(Ui {
            eng,
            out,
            win,
            stack,
            toasts,
            lights,
            scenes,
            autos,
            setup,
            cards: RefCell::default(),
            lamps: RefCell::default(),
            scene_tiles: RefCell::default(),
            auto_rows: RefCell::default(),
            master,
            master_pct,
            blackout,
            now_box,
            now_lbl,
            status_icon,
            status_lbl,
            status_row: RefCell::default(),
            syncing: Cell::new(false),
            save_due: Cell::new(None),
            last_status: Cell::new(None),
            bar_due: Cell::new(true),
            bar_switch: RefCell::default(),
            ports: RefCell::new(gaffer_output::list_ports()),
            snd,
        });

        let (m, b) = {
            let e = ui.e();
            (e.show.master, e.blackout)
        };
        ui.syncing.set(true);
        ui.master.set_value(m as f64 * 100.0);
        ui.master_pct.set_label(&pct(m));
        ui.blackout.set_active(b);
        ui.syncing.set(false);
        let u = ui.clone();
        ui.master.connect_value_changed(move |s| {
            u.master_pct.set_label(&pct(s.value() as f32 / 100.0));
            if !u.syncing.get() {
                u.e().set_master(s.value() as f32 / 100.0);
            }
        });
        let u = ui.clone();
        ui.blackout.connect_toggled(move |t| {
            if !u.syncing.get() {
                u.e().set_blackout(t.is_active());
            }
        });
        let u = ui.clone();
        stop.connect_clicked(move |_| u.e().stop());
        let u = ui.clone();
        status_btn.connect_clicked(move |_| u.stack.set_visible_child_name("setup"));
        let u = ui.clone();
        ui.stack.connect_visible_child_name_notify(move |s| {
            let name = s.visible_child_name().unwrap_or_default();
            let title = match name.as_str() {
                "scenes" => "Scenes",
                "automations" => "Automations",
                "setup" => "Setup",
                _ => "Lights",
            };
            u.win.set_title(Some(&format!("Gaffer – {title}")));
        });

        ui.rebuild_all();
        ui.setup_sounds();
        let u = ui.clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            u.tick();
            glib::ControlFlow::Continue
        });
        let u = ui.clone();
        ui.win.connect_close_request(move |_| {
            u.save_now();
            glib::Propagation::Proceed
        });
        ui
    }

    pub fn rebuild_all(self: &Rc<Self>) {
        self.rebuild_lights();
        self.rebuild_scenes();
        self.rebuild_autos();
        self.rebuild_setup();
    }

    /// Something the user made changed: save soon.
    pub fn changed(&self) {
        if self.save_due.get().is_none() {
            self.save_due.set(Some(Instant::now() + Duration::from_millis(800)));
        }
        self.bar_due.set(true);
    }

    pub fn save_now(&self) {
        let show = self.e().show_to_save();
        if let Err(e) = store::save(&show) {
            self.toast(&format!("Couldn't save: {e}"));
        }
        self.save_due.set(None);
    }

    // ---- Lights ----------------------------------------------------------------------------

    pub fn rebuild_lights(self: &Rc<Self>) {
        clear(&self.lights);
        self.cards.borrow_mut().clear();
        self.lamps.borrow_mut().clear();
        let fixtures = self.e().show.fixtures.clone();
        if fixtures.is_empty() {
            let add = gtk::Button::builder().label("Add a light").css_classes(["pill", "suggested-action"]).build();
            let sample = gtk::Button::builder().label("Try sample lights").css_classes(["pill"]).build();
            let bx = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).build();
            bx.append(&add);
            bx.append(&sample);
            let sp = adw::StatusPage::builder()
                .icon_name("display-brightness-symbolic")
                .title("No lights yet")
                .description("Add the DMX lights you have plugged in, or try Gaffer with four sample lights. The stage on this page shows what the lights are doing, so you can try everything without hardware.")
                .child(&bx)
                .build();
            self.lights.append(&sp);
            let u = self.clone();
            add.connect_clicked(move |_| dialogs::light(&u, None));
            let u = self.clone();
            sample.connect_clicked(move |_| {
                let show = store::sample_show();
                {
                    let mut e = u.e();
                    let out = e.show.output.clone();
                    *e = Engine::new(gaffer_core::Show { output: out, ..show });
                }
                u.changed();
                u.rebuild_all();
                u.toast("Added four sample lights, three scenes and two automations");
            });
            return;
        }

        // The stage.
        let stage = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(2).max_children_per_line(12).row_spacing(4).column_spacing(4).css_classes(["stage"]).build();
        for f in &fixtures {
            let cell = Rc::new(Cell::new(Rgb::BLACK));
            let da = lamp(cell.clone());
            let bx = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
            bx.append(&da);
            bx.append(&gtk::Label::builder().label(&f.name).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(12).build());
            stage.append(&bx);
            unfocus(&stage);
            self.lamps.borrow_mut().push((f.id, da, cell));
        }
        self.lights.append(&stage);

        let grid = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(1).max_children_per_line(3).row_spacing(12).column_spacing(12).build();
        if fixtures.len() > 1 {
            grid.append(&self.card(None));
        }
        for f in &fixtures {
            grid.append(&self.card(Some(f)));
        }
        unfocus(&grid);
        self.lights.append(&grid);
    }

    fn card(self: &Rc<Self>, f: Option<&Fixture>) -> gtk::Box {
        let id = f.map(|f| f.id);
        let look = match id {
            Some(id) => self.e().look(id),
            None => {
                let e = self.e();
                let n = e.show.fixtures.len().max(1) as f32;
                let level = e.show.fixtures.iter().map(|f| e.look(f.id).level).sum::<f32>() / n;
                Look::new(level, e.show.fixtures.first().map(|f| e.look(f.id).color).unwrap_or(Rgb::WHITE))
            }
        };
        let has_color = f.map_or(true, |f| f.has_color());
        let outer = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).css_classes(["card", "light-card"]).build();
        let head = gtk::Box::builder().spacing(8).build();
        let titles = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
        let (title, sub) = match f {
            Some(f) => (f.name.clone(), format!("{} · {}", channels_text(f), f.profile)),
            None => ("All lights".to_string(), "Changes every light at once".to_string()),
        };
        titles.append(&gtk::Label::builder().label(&title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build());
        titles.append(&gtk::Label::builder().label(&sub).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label", "caption"]).build());
        head.append(&titles);
        if let Some(id) = id {
            let ident = gtk::Button::builder().icon_name("find-location-symbolic").tooltip_text("Identify: flash this light for 3 seconds").css_classes(["flat", "circular"]).valign(gtk::Align::Center).build();
            let u = self.clone();
            ident.connect_clicked(move |_| u.e().identify(id));
            head.append(&ident);
        }
        let switch = gtk::Switch::builder().valign(gtk::Align::Center).active(look.level > 0.0).tooltip_text("On / off").build();
        head.append(&switch);
        outer.append(&head);

        let row = gtk::Box::builder().spacing(6).build();
        row.append(&gtk::Image::from_icon_name("display-brightness-symbolic"));
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_value(look.level as f64 * 100.0);
        let pctl = gtk::Label::builder().label(pct(look.level)).width_chars(4).xalign(1.0).css_classes(["numeric"]).build();
        row.append(&scale);
        row.append(&pctl);
        outer.append(&row);

        let color = has_color.then(|| {
            let d = gtk::ColorDialog::builder().with_alpha(false).title("Light colour").build();
            let b = gtk::ColorDialogButton::new(Some(d));
            b.set_rgba(&rgba(look.color));
            b.set_tooltip_text(Some("Any colour…"));
            b.set_valign(gtk::Align::Center);
            b
        });
        let card = Rc::new(Card { id, scale: scale.clone(), pct: pctl, switch: switch.clone(), color: color.clone(), shown: Cell::new(look), last_on: Cell::new(if look.level > 0.0 { look.level } else { 1.0 }) });

        if let Some(cb) = &color {
            let sw = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(5).min_children_per_line(5).column_spacing(0).row_spacing(0).hexpand(true).build();
            for (name, c) in PRESETS {
                let b = swatch_button(name, *c);
                let (u, card, c) = (self.clone(), card.clone(), *c);
                b.connect_clicked(move |_| u.apply(&card, None, Some(c)));
                sw.append(&b);
            }
            let crow = gtk::Box::builder().spacing(6).build();
            crow.append(&sw);
            crow.append(cb);
            outer.append(&crow);
            let (u, card2) = (self.clone(), card.clone());
            cb.connect_rgba_notify(move |b| {
                if !u.syncing.get() {
                    u.apply(&card2, None, Some(rgb(&b.rgba())));
                }
            });
        }
        let (u, c2) = (self.clone(), card.clone());
        scale.connect_value_changed(move |s| {
            if !u.syncing.get() {
                u.apply(&c2, Some(s.value() as f32 / 100.0), None);
            }
        });
        let (u, c2) = (self.clone(), card.clone());
        switch.connect_active_notify(move |s| {
            if !u.syncing.get() {
                let level = if s.is_active() { c2.last_on.get().max(0.05) } else { 0.0 };
                u.apply(&c2, Some(level), None);
            }
        });
        self.cards.borrow_mut().push(card);
        outer
    }

    /// The user changed a card: brightness and/or colour.
    fn apply(&self, card: &Card, level: Option<f32>, color: Option<Rgb>) {
        let look = {
            let mut e = self.e();
            match card.id {
                Some(id) => {
                    let mut l = e.look(id);
                    if let Some(v) = level {
                        l.level = v;
                    }
                    if let Some(c) = color {
                        l.color = c;
                        if level.is_none() && l.level <= 0.0 {
                            l.level = card.last_on.get().max(0.05);
                        }
                    }
                    e.set_look(id, l);
                    e.look(id)
                }
                None => {
                    let level = level.or_else(|| (color.is_some() && card.shown.get().level <= 0.0).then(|| card.last_on.get().max(0.05)));
                    e.set_all(level, color);
                    let mut l = card.shown.get();
                    if let Some(v) = level {
                        l.level = v;
                    }
                    if let Some(c) = color {
                        l.color = c;
                    }
                    l
                }
            }
        };
        self.show_card(card, look);
        if card.id.is_none() {
            self.sync_cards(); // every light card follows
        }
        self.changed();
    }

    fn show_card(&self, card: &Card, look: Look) {
        if card.shown.get() == look {
            return;
        }
        self.syncing.set(true);
        card.shown.set(look);
        if look.level > 0.0 {
            card.last_on.set(look.level);
        }
        if (card.scale.value() - look.level as f64 * 100.0).abs() > 0.5 {
            card.scale.set_value(look.level as f64 * 100.0);
        }
        card.pct.set_label(&pct(look.level));
        if card.switch.is_active() != (look.level > 0.0) {
            card.switch.set_active(look.level > 0.0);
        }
        if let Some(cb) = &card.color {
            if rgb(&cb.rgba()) != look.color {
                cb.set_rgba(&rgba(look.color));
            }
        }
        self.syncing.set(false);
    }

    fn sync_cards(&self) {
        let cards = self.cards.borrow().clone();
        let looks: Vec<Option<Look>> = {
            let e = self.eng.lock().unwrap_or_else(|e| e.into_inner());
            cards.iter().map(|c| c.id.map(|id| e.look(id))).collect()
        };
        for (c, l) in cards.iter().zip(looks) {
            if let Some(l) = l {
                self.show_card(c, l);
            }
        }
    }

    // ---- Scenes ----------------------------------------------------------------------------

    pub fn rebuild_scenes(self: &Rc<Self>) {
        clear(&self.scenes);
        self.scene_tiles.borrow_mut().clear();
        let (scenes, fade, nfix) = {
            let e = self.e();
            (e.show.scenes.clone(), e.show.scene_fade, e.show.fixtures.len())
        };
        let top = gtk::Box::builder().spacing(12).build();
        top.append(&dim("Tap a scene to fade to it. A scene remembers how every light looks."));
        top.last_child().unwrap().set_hexpand(true);
        let fade_spin = gtk::SpinButton::with_range(0.0, 60.0, 0.5);
        fade_spin.set_digits(1);
        fade_spin.set_value(fade as f64);
        fade_spin.set_valign(gtk::Align::Center);
        fade_spin.set_tooltip_text(Some("Seconds a scene takes to fade in"));
        let fl = gtk::Box::builder().spacing(6).build();
        fl.append(&gtk::Label::new(Some("Fade")));
        fl.append(&fade_spin);
        fl.append(&gtk::Label::new(Some("s")));
        top.append(&fl);
        let save = gtk::Button::builder().label("Save current look").css_classes(["suggested-action"]).tooltip_text("Ctrl+S").sensitive(nfix > 0).build();
        top.append(&save);
        self.scenes.append(&top);
        let u = self.clone();
        fade_spin.connect_value_changed(move |s| {
            u.e().show.scene_fade = s.value() as f32;
            u.changed();
        });
        let u = self.clone();
        save.connect_clicked(move |_| dialogs::save_scene(&u));

        if scenes.is_empty() {
            let text = if nfix == 0 { "No scenes yet. Add your lights first (Setup), then set them how you like and save the look here." } else { "No scenes yet. Set your lights how you like on the Lights page, then press “Save current look”." };
            self.scenes.append(&adw::StatusPage::builder().icon_name("view-grid-symbolic").title("Scenes").description(text).build());
            self.bar_due.set(true);
            return;
        }
        let grid = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).homogeneous(true).min_children_per_line(2).max_children_per_line(5).row_spacing(12).column_spacing(12).build();
        for (i, s) in scenes.iter().enumerate() {
            let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
            let sw = gtk::Box::builder().spacing(4).height_request(28).build();
            let colors = s.swatches(6);
            if colors.is_empty() {
                sw.append(&dim("All off"));
            }
            for c in colors {
                sw.append(&swatch(c, 24));
            }
            body.append(&sw);
            let name_row = gtk::Box::builder().spacing(6).build();
            name_row.append(&gtk::Label::builder().label(&s.name).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build());
            if i < 12 {
                name_row.append(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["badge"]).tooltip_text(format!("Scene {}: Touch Bar key / “gaffer ctl scene-{}”", i + 1, i + 1)).build());
            }
            body.append(&name_row);
            let tile = gtk::Button::builder().child(&body).css_classes(["card", "scene-tile"]).build();
            let (u, id) = (self.clone(), s.id);
            tile.connect_clicked(move |_| {
                u.e().recall(id, None);
                u.changed();
            });
            let menu = self.scene_menu(s.id, i, scenes.len());
            let ov = gtk::Overlay::builder().child(&tile).build();
            ov.add_overlay(&menu);
            grid.append(&ov);
            self.scene_tiles.borrow_mut().push((s.id, tile));
        }
        unfocus(&grid);
        self.scenes.append(&grid);
        self.bar_due.set(true);
        self.refresh_pad_cues();
    }

    fn scene_menu(self: &Rc<Self>, id: u32, i: usize, n: usize) -> gtk::MenuButton {
        let pop = gtk::Popover::new();
        let bx = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let item = |label: &str, enabled: bool| {
            let b = gtk::Button::builder().label(label).css_classes(["flat"]).sensitive(enabled).build();
            b.child().and_downcast::<gtk::Label>().map(|l| l.set_xalign(0.0));
            bx.append(&b);
            b
        };
        let update = item("Save current look into it", true);
        let rename = item("Rename…", true);
        let earlier = item("Move earlier", i > 0);
        let later = item("Move later", i + 1 < n);
        let delete = item("Delete…", true);
        pop.set_child(Some(&bx));
        let mb = gtk::MenuButton::builder().icon_name("view-more-symbolic").popover(&pop).css_classes(["flat", "circular"]).halign(gtk::Align::End).valign(gtk::Align::Start).margin_top(4).margin_end(4).tooltip_text("Scene options").build();
        let (u, p) = (self.clone(), pop.clone());
        update.connect_clicked(move |_| {
            p.popdown();
            u.e().update_scene(id);
            u.changed();
            u.rebuild_scenes();
            u.toast("Scene updated with the current look");
        });
        let (u, p) = (self.clone(), pop.clone());
        rename.connect_clicked(move |_| {
            p.popdown();
            dialogs::rename_scene(&u, id);
        });
        for (b, d) in [(earlier, -1), (later, 1)] {
            let (u, p) = (self.clone(), pop.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                u.e().move_scene(id, d);
                u.changed();
                u.rebuild_scenes();
            });
        }
        let (u, p) = (self.clone(), pop.clone());
        delete.connect_clicked(move |_| {
            p.popdown();
            let name = u.e().show.scene(id).map(|s| s.name.clone()).unwrap_or_default();
            let used = u.e().show.automations.iter().filter(|a| matches!(&a.action, Action::Sequence { steps, .. } if steps.iter().any(|s| s.target == gaffer_core::Target::Scene(id)))).count();
            let body = if used > 0 { format!("{used} automation step(s) use it; they will be skipped.") } else { String::new() };
            let u2 = u.clone();
            dialogs::confirm(&u, &format!("Delete “{name}”?"), &body, "Delete", move || {
                u2.e().delete_scene(id);
                u2.changed();
                u2.rebuild_scenes();
                u2.rebuild_autos();
            });
        });
        mb
    }

    // ---- Automations -----------------------------------------------------------------------

    pub fn rebuild_autos(self: &Rc<Self>) {
        clear(&self.autos);
        self.auto_rows.borrow_mut().clear();
        let (autos, nfix, scenes) = {
            let e = self.e();
            (e.show.automations.clone(), e.show.fixtures.len(), e.show.scenes.len())
        };
        let top = gtk::Box::builder().spacing(12).build();
        top.append(&dim("Effects move your lights on their own. Sequences step through your scenes. Either can start by hand, at a time of day, or when Gaffer opens."));
        top.last_child().unwrap().set_hexpand(true);
        self.autos.append(&top);

        let group = adw::PreferencesGroup::builder().title("Your automations").build();
        let new_fx = gtk::Button::builder().label("New effect").css_classes(["suggested-action"]).sensitive(nfix > 0).build();
        let new_seq = gtk::Button::builder().label("New sequence").sensitive(scenes > 0).tooltip_text(if scenes > 0 { "Steps through scenes with fades" } else { "Save a scene first" }).build();
        let hb = gtk::Box::builder().spacing(6).build();
        hb.append(&new_seq);
        hb.append(&new_fx);
        group.set_header_suffix(Some(&hb));
        let u = self.clone();
        new_fx.connect_clicked(move |_| dialogs::effect(&u, None, None));
        let u = self.clone();
        new_seq.connect_clicked(move |_| dialogs::sequence(&u, None));
        if autos.is_empty() {
            group.add(&adw::ActionRow::builder().title("Nothing here yet").subtitle(if nfix == 0 { "Add a light first (Setup)." } else { "Pick an effect below, or make a sequence from your scenes." }).build());
        }
        for a in &autos {
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&a.name)).subtitle(glib::markup_escape_text(&a.describe())).activatable(true).build();
            let icon = match &a.action {
                Action::Effect(e) => e.kind.icon(),
                Action::Sequence { .. } => "media-playlist-repeat-symbolic",
            };
            row.add_prefix(&gtk::Image::from_icon_name(icon));
            let play = gtk::Button::builder().icon_name("media-playback-start-symbolic").tooltip_text("Play").valign(gtk::Align::Center).css_classes(["circular"]).build();
            let edit = gtk::Button::builder().icon_name("document-edit-symbolic").tooltip_text("Edit").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let del = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Delete").valign(gtk::Align::Center).css_classes(["flat"]).build();
            row.add_suffix(&play);
            row.add_suffix(&edit);
            row.add_suffix(&del);
            let id = a.id;
            let u = self.clone();
            play.connect_clicked(move |_| {
                let playing = u.e().running().map(|r| r.id()) == Some(id);
                if playing {
                    u.e().stop();
                } else if !u.e().play(id) {
                    u.drain_notices();
                }
                u.sync_blackout();
            });
            let u = self.clone();
            let open = move || {
                let a = u.e().show.automation(id).cloned();
                match a.map(|a| a.action) {
                    Some(Action::Effect(_)) => dialogs::effect(&u, Some(id), None),
                    Some(Action::Sequence { .. }) => dialogs::sequence(&u, Some(id)),
                    None => {}
                }
            };
            let o = open.clone();
            edit.connect_clicked(move |_| o());
            row.connect_activated(move |_| open());
            let u = self.clone();
            let name = a.name.clone();
            del.connect_clicked(move |_| {
                let u2 = u.clone();
                dialogs::confirm(&u, &format!("Delete “{name}”?"), "", "Delete", move || {
                    u2.e().delete_automation(id);
                    u2.changed();
                    u2.rebuild_autos();
                });
            });
            group.add(&row);
            self.auto_rows.borrow_mut().push((a.id, play, row, a.describe()));
        }
        self.autos.append(&group);

        if nfix > 0 {
            let gal = adw::PreferencesGroup::builder().title("Start from an effect").description("Opens the editor with the effect ready to try.").build();
            for &k in gaffer_core::EFFECTS {
                let row = adw::ActionRow::builder().title(k.name()).subtitle(k.blurb()).activatable(true).build();
                row.add_prefix(&gtk::Image::from_icon_name(k.icon()));
                row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                let u = self.clone();
                row.connect_activated(move |_| dialogs::effect(&u, None, Some(k)));
                gal.add(&row);
            }
            self.autos.append(&gal);
        }
        self.bar_due.set(true);
        self.refresh_pad_cues();
    }

    // ---- Setup -----------------------------------------------------------------------------

    pub fn rebuild_setup(self: &Rc<Self>) {
        clear(&self.setup);
        *self.status_row.borrow_mut() = None;
        let (cfg, fixtures) = {
            let e = self.e();
            (e.show.output.clone(), e.show.fixtures.clone())
        };
        let ports = self.ports.borrow().clone();

        let iface = adw::PreferencesGroup::builder().title("DMX interface").description("Plug in a USB-DMX interface, then choose it here. Without one, Gaffer runs as a simulator and the Lights page shows what would happen.").build();
        let rescan = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Look for interfaces again").css_classes(["flat"]).valign(gtk::Align::Center).build();
        iface.set_header_suffix(Some(&rescan));
        let mut labels = vec!["Simulator (no hardware)".to_string()];
        labels.extend(ports.iter().map(|p| match p.guess {
            Some(k) => format!("{} – {}", p.label(), k.name()),
            None => format!("{} – may not be DMX", p.label()),
        }));
        let mut paths: Vec<Option<String>> = vec![None];
        paths.extend(ports.iter().map(|p| Some(p.path.clone())));
        if let Some(p) = &cfg.port {
            let by_serial = cfg.serial.as_ref().filter(|s| !s.is_empty()).and_then(|s| ports.iter().position(|q| &q.serial == s));
            if by_serial.is_none() && !paths.contains(&Some(p.clone())) {
                labels.push(format!("{p} (not plugged in)"));
                paths.push(Some(p.clone()));
            }
        }
        let sel = cfg.serial.as_ref().filter(|s| !s.is_empty()).and_then(|s| ports.iter().position(|q| &q.serial == s).map(|i| i + 1)).or_else(|| paths.iter().position(|p| *p == cfg.port)).unwrap_or(0);
        let model = gtk::StringList::new(&labels.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let port_row = adw::ComboRow::builder().title("Send to").model(&model).selected(sel as u32).build();
        let kinds = gtk::StringList::new(&["Automatic", "Enttec Pro protocol (Enttec Pro, DMXking, Eurolite Pro…)", "Open DMX / cheap USB-DMX cable (FTDI, CH340, CP2102)", "uDMX dongle"]);
        let kind_sel = match Kind::from_key(&cfg.kind) {
            Kind::Auto => 0,
            Kind::Pro => 1,
            Kind::Open => 2,
            Kind::Udmx => 3,
        };
        let kind_row = adw::ComboRow::builder().title("Protocol").subtitle("Automatic works for most interfaces").model(&kinds).selected(kind_sel).sensitive(sel != 0).build();
        let status = adw::ActionRow::builder().title("Status").subtitle("…").build();
        status.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
        iface.add(&port_row);
        iface.add(&kind_row);
        iface.add(&status);
        *self.status_row.borrow_mut() = Some(status);
        self.last_status.set(None);
        let apply = {
            let u = self.clone();
            let (port_row, kind_row) = (port_row.clone(), kind_row.clone());
            move || {
                let i = port_row.selected() as usize;
                let path = paths.get(i).cloned().flatten();
                let kind = match kind_row.selected() {
                    1 => Kind::Pro,
                    2 => Kind::Open,
                    3 => Kind::Udmx,
                    _ => Kind::Auto,
                };
                kind_row.set_sensitive(path.is_some());
                let serial = path.as_ref().and_then(|p| u.ports.borrow().iter().find(|q| &q.path == p).map(|q| q.serial.clone())).filter(|s| !s.is_empty());
                {
                    let mut e = u.e();
                    e.show.output = gaffer_core::show::OutputCfg { port: path.clone(), serial: serial.clone(), kind: kind.key().into() };
                }
                u.out.set_target(match path {
                    Some(path) => OutTarget::Port { path, serial, kind },
                    None => OutTarget::Simulator,
                });
                u.last_status.set(None);
                u.changed();
            }
        };
        let a = apply.clone();
        port_row.connect_selected_notify(move |_| a());
        kind_row.connect_selected_notify(move |_| apply());
        let u = self.clone();
        rescan.connect_clicked(move |_| {
            *u.ports.borrow_mut() = gaffer_output::list_ports();
            u.out.retry();
            u.rebuild_setup();
            let n = u.ports.borrow().len();
            u.toast(&match n {
                0 => "No USB serial devices found".to_string(),
                1 => "Found 1 device".to_string(),
                n => format!("Found {n} devices"),
            });
        });
        self.setup.append(&iface);

        let lights = adw::PreferencesGroup::builder().title("Lights").description("Each light needs its own start address, set on the light itself (its display or DIP switches).").build();
        let add = gtk::Button::builder().label("Add light").css_classes(["suggested-action"]).tooltip_text("Ctrl+N").valign(gtk::Align::Center).build();
        lights.set_header_suffix(Some(&add));
        let u = self.clone();
        add.connect_clicked(move |_| dialogs::light(&u, None));
        if fixtures.is_empty() {
            lights.add(&adw::ActionRow::builder().title("No lights yet").subtitle("Press “Add light”.").build());
        }
        let show = self.e().show.clone();
        for (i, f) in fixtures.iter().enumerate() {
            let clashes = show.clashes(f);
            let mut sub = format!("{} · {} ({})", channels_text(f), f.profile, gaffer_core::fixture::layout_string(&f.chans));
            if !clashes.is_empty() {
                sub.push_str(&format!(" · shares channels with {}", clashes.join(", ")));
            }
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(&f.name)).subtitle(glib::markup_escape_text(&sub)).activatable(true).build();
            if !clashes.is_empty() {
                row.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
            }
            let id = f.id;
            let ident = gtk::Button::builder().icon_name("find-location-symbolic").tooltip_text("Identify: flash it").valign(gtk::Align::Center).css_classes(["flat"]).build();
            let up = gtk::Button::builder().icon_name("go-up-symbolic").tooltip_text("Move up").valign(gtk::Align::Center).css_classes(["flat"]).sensitive(i > 0).build();
            let down = gtk::Button::builder().icon_name("go-down-symbolic").tooltip_text("Move down").valign(gtk::Align::Center).css_classes(["flat"]).sensitive(i + 1 < fixtures.len()).build();
            let del = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Remove").valign(gtk::Align::Center).css_classes(["flat"]).build();
            for b in [&ident, &up, &down, &del] {
                row.add_suffix(b);
            }
            let u = self.clone();
            ident.connect_clicked(move |_| u.e().identify(id));
            for (b, d) in [(up, -1), (down, 1)] {
                let u = self.clone();
                b.connect_clicked(move |_| {
                    u.e().move_fixture(id, d);
                    u.changed();
                    u.rebuild_setup();
                    u.rebuild_lights();
                });
            }
            let u = self.clone();
            let name = f.name.clone();
            del.connect_clicked(move |_| {
                let u2 = u.clone();
                dialogs::confirm(&u, &format!("Remove “{name}”?"), "It is also taken out of your scenes and effects.", "Remove", move || {
                    u2.e().remove_fixture(id);
                    u2.changed();
                    u2.rebuild_all();
                });
            });
            let u = self.clone();
            row.connect_activated(move |_| dialogs::light(&u, Some(id)));
            lights.add(&row);
        }
        self.setup.append(&lights);

        // Touch Bar (only where barkeep is set up).
        *self.bar_switch.borrow_mut() = None;
        if dirs_barkeep() {
            let g = adw::PreferencesGroup::builder().title("Touch Bar").build();
            let sw = adw::SwitchRow::builder().title("Show Gaffer on the Touch Bar").subtitle("Blackout, master, your first scenes and automations as keys").active(bar::installed()).build();
            let u = self.clone();
            sw.connect_active_notify(move |s| u.set_bar(s.is_active()));
            g.add(&sw);
            *self.bar_switch.borrow_mut() = Some(sw);
            self.setup.append(&g);
        }

        let help = adw::PreferencesGroup::builder().title("Help").build();
        for (title, text) in HELP {
            let ex = adw::ExpanderRow::builder().title(*title).build();
            let l = gtk::Label::builder().label(*text).wrap(true).xalign(0.0).margin_top(10).margin_bottom(10).margin_start(12).margin_end(12).selectable(true).build();
            ex.add_row(&l);
            help.add(&ex);
        }
        self.setup.append(&help);
    }

    /// Scene or automation names changed: the pads that cue them show the new names.
    fn refresh_pad_cues(self: &Rc<Self>) {
        if self.snd.split.sidebar().is_some() && self.e().show.sounds.sounds.iter().any(|s| matches!(s.cue, gaffer_core::LightCue::Scene { .. } | gaffer_core::LightCue::Automation { .. })) {
            self.rebuild_sounds();
        }
    }

    fn set_bar(&self, on: bool) {
        let (s, a, p) = self.names();
        match bar::install(&s, &a, &p, !on) {
            Ok(Some(_)) => self.toast(if on { "Gaffer is on the Touch Bar" } else { "Removed Gaffer from the Touch Bar" }),
            Ok(None) => self.toast("barkeep isn't set up on this computer"),
            Err(e) => self.toast(&format!("Touch Bar: {e}")),
        }
    }

    fn names(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let e = self.e();
        (e.show.scenes.iter().map(|s| s.name.clone()).collect(), e.show.automations.iter().map(|a| a.name.clone()).collect(), crate::soundboard::pad_names(&e.show.sounds))
    }

    // ---- the heartbeat ---------------------------------------------------------------------

    pub fn drain_notices(&self) {
        let n: Vec<String> = std::mem::take(&mut self.e().notices);
        for t in n {
            self.toast(&t);
        }
    }

    fn sync_blackout(&self) {
        let (b, m) = {
            let e = self.e();
            (e.blackout, e.show.master)
        };
        self.syncing.set(true);
        if self.blackout.is_active() != b {
            self.blackout.set_active(b);
        }
        if (self.master.value() - m as f64 * 100.0).abs() > 0.5 {
            self.master.set_value(m as f64 * 100.0);
        }
        self.syncing.set(false);
    }

    fn tick(&self) {
        let (shown, active, running, dirty) = {
            let mut e = self.e();
            let d = std::mem::replace(&mut e.dirty, false);
            (e.shown.clone(), e.active_scene, e.running(), d)
        };
        for (id, da, cell) in self.lamps.borrow().iter() {
            let c = shown.get(id).copied().unwrap_or(Rgb::BLACK);
            if cell.get() != c {
                cell.set(c);
                da.queue_draw();
            }
        }
        self.sync_cards();
        self.sync_blackout();
        for (id, tile) in self.scene_tiles.borrow().iter() {
            let on = Some(*id) == active;
            if tile.has_css_class("active") != on {
                if on { tile.add_css_class("active") } else { tile.remove_css_class("active") }
            }
        }
        let playing = running.map(|r| r.id());
        for (id, btn, row, desc) in self.auto_rows.borrow().iter() {
            let on = Some(*id) == playing;
            let icon = if on { "media-playback-stop-symbolic" } else { "media-playback-start-symbolic" };
            if btn.icon_name().as_deref() != Some(icon) {
                btn.set_icon_name(icon);
                btn.set_tooltip_text(Some(if on { "Stop" } else { "Play" }));
                if on { btn.add_css_class("suggested-action") } else { btn.remove_css_class("suggested-action") }
                row.set_subtitle(&glib::markup_escape_text(&if on { format!("Playing · {desc}") } else { desc.clone() }));
            }
        }
        let now = running.and_then(|r| {
            let e = self.e();
            let a = e.show.automation(r.id())?;
            Some(match r {
                Running::Sequence { step, .. } => format!("{} · step {}", a.name, step + 1),
                Running::Effect { .. } => a.name.clone(),
            })
        });
        self.now_box.set_visible(now.is_some());
        if let Some(t) = now {
            if self.now_lbl.label() != t {
                self.now_lbl.set_label(&t);
            }
        }
        self.drain_notices();

        if self.last_status.get().is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            self.last_status.set(Some(Instant::now()));
            let st = self.out.status();
            let (icon, short) = match st.link {
                Link::Simulator => ("computer-symbolic", "Simulator".to_string()),
                Link::Connected => ("emblem-ok-symbolic", "DMX connected".to_string()),
                Link::Waiting => ("network-offline-symbolic", "Interface unplugged".to_string()),
                Link::Error => ("dialog-warning-symbolic", "DMX problem".to_string()),
            };
            self.status_icon.set_icon_name(Some(icon));
            if self.status_lbl.label() != short {
                self.status_lbl.set_label(&short);
            }
            if let Some(row) = self.status_row.borrow().as_ref() {
                let detail = match (st.link, st.kind) {
                    (Link::Connected, Some(k)) => format!("{} · {} · {} frames sent", st.detail, k.name(), st.frames),
                    _ => st.detail.clone(),
                };
                row.set_subtitle(&glib::markup_escape_text(&detail));
                if let Some(img) = row.first_child().and_then(|_| find_prefix_image(row)) {
                    img.set_icon_name(Some(icon));
                }
            }
        }

        if dirty {
            self.changed();
        }
        if self.save_due.get().is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }
        if self.bar_due.replace(false) && bar::installed() {
            let (s, a, p) = self.names();
            let want = serde_json::Value::Array(bar::bar_scenes(&s, &a, &p));
            if bar::installed_names().as_ref() != Some(&want) {
                let _ = bar::install(&s, &a, &p, false);
            }
        }
    }

    // ---- remote control ----------------------------------------------------------------------

    pub fn ctl(self: &Rc<Self>, c: Ctl) {
        let mut note: Option<String> = None;
        {
            let mut e = self.e();
            match c {
                Ctl::Blackout(v) => {
                    let on = v.unwrap_or(!e.blackout);
                    e.set_blackout(on);
                }
                Ctl::Master(d) => {
                    let m = ((e.show.master * 100.0).round() as i32 + d).clamp(0, 100);
                    e.set_master(m as f32 / 100.0);
                }
                Ctl::Scene(n) => {
                    if let Some(id) = e.show.scenes.get(n - 1).map(|s| s.id) {
                        e.recall(id, None);
                        note = e.show.scene(id).map(|s| s.name.clone());
                    }
                }
                Ctl::SceneStep(d) => {
                    if let Some(id) = e.recall_next(d) {
                        note = e.show.scene(id).map(|s| s.name.clone());
                    }
                }
                Ctl::AllOff => {
                    let f = e.show.scene_fade;
                    e.all_off(f);
                }
                Ctl::Stop => e.stop(),
                Ctl::Play(n) => {
                    if let Some(id) = e.show.automations.get(n - 1).map(|a| a.id) {
                        e.play(id);
                    }
                }
                Ctl::PlayStep(d) => {
                    e.play_next(d);
                }
                Ctl::PlayToggle => {
                    if e.running().is_some() {
                        e.stop();
                    } else {
                        e.play_next(0);
                    }
                }
                Ctl::Show | Ctl::Page(_) | Ctl::Sound(_) | Ctl::SoundsStop | Ctl::SoundsToggle => {}
            }
        }
        match c {
            Ctl::Show => self.win.present(),
            Ctl::Page(p) => {
                self.stack.set_visible_child_name(p);
                self.win.present();
            }
            Ctl::Sound(n) => self.sound_nth(n),
            Ctl::SoundsStop => self.stop_all_sounds(),
            Ctl::SoundsToggle => self.toggle_sounds(),
            _ => {}
        }
        if let Some(n) = note {
            self.toast(&n);
        }
        self.sync_blackout();
        self.changed();
    }
}

fn find_prefix_image(row: &adw::ActionRow) -> Option<gtk::Image> {
    // The prefix icon we added is the first Image under the row.
    fn walk(w: &gtk::Widget) -> Option<gtk::Image> {
        let mut c = w.first_child();
        while let Some(x) = c {
            if let Ok(i) = x.clone().downcast::<gtk::Image>() {
                return Some(i);
            }
            if let Some(i) = walk(&x) {
                return Some(i);
            }
            c = x.next_sibling();
        }
        None
    }
    walk(row.upcast_ref())
}

/// FlowBox children are only containers here: keep keyboard focus on what's inside them.
fn unfocus(fb: &gtk::FlowBox) {
    let mut c = fb.first_child();
    while let Some(w) = c {
        w.set_focusable(false);
        c = w.next_sibling();
    }
}

fn dirs_barkeep() -> bool {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .is_some_and(|d| d.join("barkeep").is_dir())
}

const HELP: &[(&str, &str)] = &[
    (
        "What hardware do I need?",
        "• A USB-DMX interface. Cheapest (about $20–30): a “USB to DMX512” cable with an FTDI, CH340 or CP2102 chip (e.g. DSD TECH SH-RS09B, or SH-RS09C with a USB-C plug) or a uDMX dongle. Gaffer makes the DMX signal itself for these (Open DMX / uDMX). Steadier, about $160: an Enttec DMX USB Pro or a DMXking box (Enttec Pro protocol). “Automatic” picks the right protocol for all of them.\n\
         • If the interface has a USB-A or USB-B plug, a USB-C adapter or cable for the Mac.\n\
         • DMX cables (3-pin XLR is most common on LED lights; a 5-to-3-pin adapter if your interface has 5 pins).\n\
         • A DMX terminator (120 Ω) in the DMX OUT of the last light in the chain.",
    ),
    (
        "Wiring and addresses",
        "Chain the lights: interface → DMX IN of light 1, its DMX OUT → DMX IN of light 2, and so on. Put each light in DMX mode (often shown as “A001” or “d001”) and set its start address. A light using 7 channels at address 1 uses channels 1–7, so the next light starts at 8. Gaffer suggests the next free address when you add a light, and the light's manual lists its channel order (pick the matching type, or type the order as letters with Custom).",
    ),
    (
        "Nothing lights up?",
        "1. Check the status above says “Sending to …”. If it says no permission, add yourself to the dialout group and log in again (uDMX dongle: run install.sh once).\n\
         2. Is Blackout off and Master up?\n\
         3. Is the light in DMX mode (not auto, sound or master/slave mode), with the address you entered here?\n\
         4. Press Identify: the light should flash. If the wrong one flashes, the addresses are swapped.\n\
         5. Colours mixed up? The channel order is different: edit the light and pick another type, or use Custom.\n\
         6. Flickering on long chains: add the terminator to the last light.\n\
         7. A cheap cable that flickers or does nothing: set Protocol to “Open DMX” by hand; a few cheap cables are Enttec Pro clones, so try that too.",
    ),
    (
        "Soundboard",
        "The ♪ button at the top right opens the soundboard beside your lights (Ctrl+K). Every sound file in its folder (MP3, WAV, OGG, FLAC, M4A…) becomes a button: tap to play, tap again to stop. Several sounds can play at once.\n\
         • Add sounds with +, by dropping files on the panel, or by putting them in the folder (Music/Gaffer Sounds unless you pick another one in the ⋮ menu). New files show up by themselves.\n\
         • Right-click a button (or its ⋮) to rename it, set its volume, make it repeat, or give it a light cue: the lights can pulse with the sound, flash, go to a scene, start an automation or black out. Renaming only changes the button, not the file.\n\
         • “Put the lights back when it ends” returns the lights to how they were before the sound, unless you changed them by hand meanwhile.\n\
         • F1…F12 play the first twelve sounds; Shift+Esc stops all sounds.",
    ),
    (
        "Keyboard shortcuts",
        "Ctrl+1…4  pages · Ctrl+B  blackout · Ctrl+Up / Ctrl+Down  master · Alt+Right / Alt+Left  next / previous scene · Ctrl+S  save scene · Ctrl+.  stop automation · Ctrl+N  add light · Ctrl+K  soundboard · F1…F12  sounds · Shift+Esc  stop all sounds\n\
         From a terminal or script: gaffer ctl blackout | scene-next | scene-2 | play-1 | sound-3 | sounds-stop | stop | …  (gaffer --help lists them all).",
    ),
];
