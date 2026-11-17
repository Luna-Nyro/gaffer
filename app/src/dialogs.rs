//! Dialogs: add/edit a light, name a scene, edit an effect or a sequence, confirm a delete.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gaffer_core::fixture::{PROFILES, layout_string, parse_layout, profile_layout};
use gaffer_core::{Action, Automation, EFFECTS, EffectKind, EffectParams, Fixture, Rgb, Step, Target, Trigger};
use gtk::glib;

use crate::ui::{Ui, rgb, rgba};

fn page_dialog(ui: &Rc<Ui>, title: &str, save_label: &str) -> (adw::Dialog, adw::PreferencesPage, gtk::Button, gtk::Button, adw::HeaderBar) {
    let d = adw::Dialog::builder().title(title).content_width(if title.contains("sequence") { 660 } else { 520 }).content_height(680).build();
    let page = adw::PreferencesPage::new();
    let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder().label(save_label).css_classes(["suggested-action"]).build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let tb = adw::ToolbarView::new();
    tb.add_top_bar(&header);
    tb.set_content(Some(&page));
    d.set_child(Some(&tb));
    let _ = ui;
    (d, page, save, cancel, header)
}

pub fn confirm(ui: &Rc<Ui>, heading: &str, body: &str, action: &str, f: impl Fn() + 'static) {
    let d = adw::AlertDialog::new(Some(heading), (!body.is_empty()).then_some(body));
    d.add_responses(&[("cancel", "Cancel"), ("do", action)]);
    d.set_response_appearance("do", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.connect_response(None, move |_, r| {
        if r == "do" {
            f();
        }
    });
    d.present(Some(&ui.win));
}

pub fn ask_name(ui: &Rc<Ui>, heading: &str, initial: &str, action: &str, f: impl Fn(String) + 'static) {
    let d = adw::AlertDialog::new(Some(heading), None);
    let entry = gtk::Entry::builder().text(initial).activates_default(true).build();
    d.set_extra_child(Some(&entry));
    d.add_responses(&[("cancel", "Cancel"), ("ok", action)]);
    d.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("ok"));
    d.set_close_response("cancel");
    let e2 = entry.clone();
    d.connect_response(None, move |_, r| {
        if r == "ok" {
            f(e2.text().to_string());
        }
    });
    d.present(Some(&ui.win));
    entry.grab_focus();
    entry.select_region(0, -1);
}

pub fn save_scene(ui: &Rc<Ui>) {
    if ui.e().show.fixtures.is_empty() {
        ui.toast("Add a light first");
        return;
    }
    let n = ui.e().show.scenes.len() + 1;
    let u = ui.clone();
    ask_name(ui, "Save the current look", &format!("Scene {n}"), "Save", move |name| {
        let id = u.e().save_scene(&name);
        u.changed();
        u.rebuild_scenes();
        u.rebuild_autos();
        let name = u.e().show.scene(id).map(|s| s.name.clone()).unwrap_or_default();
        u.toast(&format!("Saved “{name}”"));
    });
}

pub fn rename_scene(ui: &Rc<Ui>, id: u32) {
    let Some(old) = ui.e().show.scene(id).map(|s| s.name.clone()) else { return };
    let u = ui.clone();
    ask_name(ui, "Rename scene", &old, "Rename", move |name| {
        u.e().rename_scene(id, &name);
        u.changed();
        u.rebuild_scenes();
        u.rebuild_autos();
    });
}

// ---- lights -------------------------------------------------------------------------------------

pub fn light(ui: &Rc<Ui>, id: Option<u32>) {
    let existing = id.and_then(|id| ui.e().show.fixture(id).cloned());
    let (d, page, save, cancel, _) = page_dialog(ui, if existing.is_some() { "Edit light" } else { "Add a light" }, if existing.is_some() { "Save" } else { "Add" });
    let group = adw::PreferencesGroup::new();
    let n = ui.e().show.fixtures.len() + 1;
    let name = adw::EntryRow::builder().title("Name").text(existing.as_ref().map_or(format!("Light {n}"), |f| f.name.clone())).build();
    let mut names: Vec<&str> = PROFILES.iter().map(|p| p.name).collect();
    names.push("Custom (type the channel order)");
    let types = adw::ComboRow::builder().title("Type").model(&gtk::StringList::new(&names)).build();
    let custom = adw::EntryRow::builder().title("Channel order, as letters").build();
    let legend = gtk::Label::builder().label("d = dimmer, r g b = red green blue, w = white, a = amber, u = UV, 0 = always off (strobe, mode…), f = always full. Example for a 7-channel PAR: d r g b 0 0 0").wrap(true).xalign(0.0).css_classes(["dim-label", "caption"]).margin_start(12).margin_end(12).margin_top(6).build();
    let addr = adw::SpinRow::with_range(1.0, 512.0, 1.0);
    addr.set_title("Start address");
    let count = adw::SpinRow::with_range(1.0, 32.0, 1.0);
    count.set_title("How many of these");
    count.set_subtitle("Adds identical lights at consecutive addresses");
    let info = adw::ActionRow::builder().title("Uses").build();
    group.add(&name);
    group.add(&types);
    group.add(&custom);
    group.add(&addr);
    if existing.is_none() {
        group.add(&count);
    }
    group.add(&info);
    page.add(&group);
    let lg = adw::PreferencesGroup::new();
    lg.add(&legend);
    page.add(&lg);

    match &existing {
        Some(f) => {
            let i = PROFILES.iter().position(|p| p.name == f.profile && profile_layout(p.name).as_deref() == Some(&f.chans[..])).unwrap_or(PROFILES.len());
            types.set_selected(i as u32);
            custom.set_text(&layout_string(&f.chans));
            addr.set_value(f.address as f64);
        }
        None => {
            types.set_selected(1);
            let w = profile_layout(PROFILES[1].name).map_or(4, |c| c.len());
            addr.set_value(ui.e().show.next_free_address(w).unwrap_or(1) as f64);
        }
    }
    // The light(s) as the form stands, or why not.
    let form = {
        let (types, custom, addr, count, name, ui) = (types.clone(), custom.clone(), addr.clone(), count.clone(), name.clone(), ui.clone());
        let fid = existing.as_ref().map_or(0, |f| f.id);
        move || -> Result<Vec<Fixture>, String> {
            let t = types.selected() as usize;
            let (chans, profile) = if t < PROFILES.len() { (profile_layout(PROFILES[t].name).unwrap_or_default(), PROFILES[t].name.to_string()) } else { (parse_layout(&custom.text())?, "Custom".to_string()) };
            let base = name.text().trim().to_string();
            let base = if base.is_empty() { "Light".to_string() } else { base };
            let n = if fid == 0 { count.value() as usize } else { 1 };
            let a0 = addr.value() as usize;
            let mut out = Vec::new();
            for i in 0..n {
                let address = a0 + i * chans.len();
                let f = Fixture { id: fid, name: if n > 1 { format!("{base} {}", i + 1) } else { base.clone() }, address: address.min(512) as u16, chans: chans.clone(), profile: profile.clone() };
                if !f.fits() || address > 512 {
                    return Err(format!("Channels {}–{} go past 512", a0, a0 + n * chans.len() - 1));
                }
                out.push(f);
            }
            let _ = &ui;
            Ok(out)
        }
    };
    let refresh = {
        let (form, info, custom, types, save, ui, legend) = (form.clone(), info.clone(), custom.clone(), types.clone(), save.clone(), ui.clone(), legend.clone());
        move || {
            let t = types.selected() as usize;
            custom.set_visible(t >= PROFILES.len());
            legend.set_visible(t >= PROFILES.len());
            types.set_subtitle(PROFILES.get(t).map_or("Any channel order", |p| p.hint));
            match form() {
                Ok(fs) => {
                    let first = fs[0].address;
                    let last = fs.last().map_or(first as usize, |f| f.last_address());
                    let show = ui.e().show.clone();
                    let clash: Vec<String> = fs.iter().flat_map(|f| show.clashes(f)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                    let mut s = if first as usize == last { format!("Channel {first}") } else { format!("Channels {first}–{last}") };
                    if !clash.is_empty() {
                        s.push_str(&format!(" · shares channels with {} (they will follow each other)", clash.join(", ")));
                    }
                    info.set_subtitle(&glib::markup_escape_text(&s));
                    save.set_sensitive(true);
                }
                Err(e) => {
                    info.set_subtitle(&glib::markup_escape_text(&e));
                    save.set_sensitive(false);
                }
            }
        }
    };
    refresh();
    let r = refresh.clone();
    types.connect_selected_notify(move |_| r());
    let r = refresh.clone();
    custom.connect_changed(move |_| r());
    let r = refresh.clone();
    addr.connect_value_notify(move |_| r());
    let r = refresh.clone();
    count.connect_value_notify(move |_| r());
    let r = refresh.clone();
    name.connect_changed(move |_| r());
    // Changing the type of a new light moves it to the next address where it fits.
    if existing.is_none() {
        let (ui2, addr2, form2) = (ui.clone(), addr.clone(), form.clone());
        types.connect_selected_notify(move |_| {
            if let Ok(fs) = form2() {
                if let Some(a) = ui2.e().show.next_free_address(fs[0].width()) {
                    addr2.set_value(a as f64);
                }
            }
        });
    }
    let d2 = d.clone();
    cancel.connect_clicked(move |_| {
        d2.close();
    });
    let (u, d2) = (ui.clone(), d.clone());
    let editing = existing.is_some();
    save.connect_clicked(move |_| {
        let fs = match form() {
            Ok(f) => f,
            Err(e) => return u.toast(&e),
        };
        let mut first = None;
        let mut err = None;
        {
            let mut e = u.e();
            for f in fs.iter().cloned() {
                let r = if editing { e.update_fixture(f.clone()).map(|_| f.id) } else { e.add_fixture(f) };
                match r {
                    Ok(id) => {
                        first.get_or_insert(id);
                    }
                    Err(x) => err = Some(x),
                }
            }
            if let Some(id) = first {
                e.identify(id);
            }
        }
        if let Some(x) = err {
            u.toast(&x);
        } else if !editing {
            u.toast(if fs.len() > 1 { "Added. The first one is flashing now" } else { "Added. It's flashing now: if another light flashes, check its address" });
        }
        u.changed();
        u.rebuild_all();
        d2.close();
    });
    d.present(Some(&ui.win));
}

// ---- when it starts -------------------------------------------------------------------------------

/// Rows for picking a trigger, and a function that reads it back.
fn trigger_group(t: Trigger) -> (adw::PreferencesGroup, Rc<dyn Fn() -> Trigger>) {
    let g = adw::PreferencesGroup::builder().title("When it starts").build();
    let kind = adw::ComboRow::builder().title("Starts").model(&gtk::StringList::new(&["By hand", "At a time of day", "When Gaffer opens"])).build();
    let hours = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
    let mins = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
    for s in [&hours, &mins] {
        s.set_valign(gtk::Align::Center);
        s.set_wrap(true);
        s.set_width_chars(2);
        s.connect_output(|s| {
            s.set_text(&format!("{:02}", s.value() as i32));
            glib::Propagation::Stop
        });
    }
    let time = adw::ActionRow::builder().title("Time").build();
    let tb = gtk::Box::builder().spacing(4).build();
    tb.append(&hours);
    tb.append(&gtk::Label::new(Some(":")));
    tb.append(&mins);
    time.add_suffix(&tb);
    let days_row = adw::ActionRow::builder().title("Days").build();
    let db = gtk::Box::builder().spacing(2).valign(gtk::Align::Center).css_classes(["linked"]).build();
    let days: Vec<gtk::ToggleButton> = ["M", "T", "W", "T", "F", "S", "S"]
        .iter()
        .zip(["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"])
        .map(|(l, n)| {
            let b = gtk::ToggleButton::builder().label(*l).tooltip_text(n).build();
            db.append(&b);
            b
        })
        .collect();
    days_row.add_suffix(&db);
    g.add(&kind);
    g.add(&time);
    g.add(&days_row);
    let (minute, mask) = match t {
        Trigger::Daily { minute, days } => (minute, days),
        _ => (19 * 60, 0x7f),
    };
    hours.set_value((minute / 60) as f64);
    mins.set_value((minute % 60) as f64);
    for (i, b) in days.iter().enumerate() {
        b.set_active(mask & (1 << i) != 0);
    }
    kind.set_selected(match t {
        Trigger::Manual => 0,
        Trigger::Daily { .. } => 1,
        Trigger::OnStart => 2,
    });
    let vis = {
        let (time, days_row) = (time.clone(), days_row.clone());
        move |k: &adw::ComboRow| {
            time.set_visible(k.selected() == 1);
            days_row.set_visible(k.selected() == 1);
        }
    };
    vis(&kind);
    kind.connect_selected_notify(vis);
    let read = Rc::new(move || match kind.selected() {
        1 => Trigger::Daily {
            minute: (hours.value() as u16) * 60 + mins.value() as u16,
            days: days.iter().enumerate().filter(|(_, b)| b.is_active()).fold(0u8, |m, (i, _)| m | (1 << i)),
        },
        2 => Trigger::OnStart,
        _ => Trigger::Manual,
    });
    (g, read)
}

/// Try / Save / Cancel for an automation editor. `build` reads the form.
fn wire_automation(ui: &Rc<Ui>, d: &adw::Dialog, header: &adw::HeaderBar, save: &gtk::Button, cancel: &gtk::Button, original: Option<Automation>, build: Rc<dyn Fn() -> Automation>) {
    let try_btn = gtk::Button::builder().label("Try it").tooltip_text("Play it now on the lights").build();
    header.pack_end(&try_btn);
    // While trying a new one it is stored under a temporary id, removed again on Cancel.
    let temp: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    let tried = Rc::new(Cell::new(false));
    let (u, b, t, o, tr) = (ui.clone(), build.clone(), temp.clone(), original.clone(), tried.clone());
    try_btn.connect_clicked(move |_| {
        tr.set(true);
        let mut a = b();
        a.id = o.as_ref().map(|o| o.id).or(t.get()).unwrap_or(0);
        let id = {
            let mut e = u.e();
            let id = e.save_automation(a);
            e.play(id);
            id
        };
        if o.is_none() {
            t.set(Some(id));
        }
    });
    let (u, d2, t, o, tr) = (ui.clone(), d.clone(), temp.clone(), original.clone(), tried.clone());
    let undo = move || {
        let mut e = u.e();
        if let Some(id) = t.take() {
            e.delete_automation(id);
        }
        if let Some(o) = &o {
            if e.show.automation(o.id) != Some(o) {
                e.save_automation(o.clone());
            }
            if tr.get() {
                e.stop_if(o.id);
            }
        }
    };
    let un = undo.clone();
    cancel.connect_clicked(move |_| {
        un();
        d2.close();
    });
    // Closing with Escape is a cancel too (the save path clears `temp` and `done` first).
    let done = Rc::new(Cell::new(false));
    let (dn, un) = (done.clone(), undo);
    d.connect_closed(move |_| {
        if !dn.get() {
            un();
        }
    });
    let (u, d2) = (ui.clone(), d.clone());
    save.connect_clicked(move |_| {
        let mut a = build();
        a.id = original.as_ref().map(|o| o.id).or(temp.take()).unwrap_or(0);
        u.e().save_automation(a);
        done.set(true);
        u.changed();
        u.rebuild_autos();
        d2.close();
    });
}

trait StopIf {
    fn stop_if(&mut self, id: u32);
}

impl StopIf for gaffer_core::Engine {
    fn stop_if(&mut self, id: u32) {
        if self.running().map(|r| r.id()) == Some(id) {
            self.stop();
        }
    }
}

// ---- effects --------------------------------------------------------------------------------------

pub fn effect(ui: &Rc<Ui>, id: Option<u32>, kind: Option<EffectKind>) {
    let original = id.and_then(|id| ui.e().show.automation(id).cloned());
    let (start, trig, title) = match &original {
        Some(Automation { action: Action::Effect(p), trigger, name, .. }) => (p.clone(), *trigger, name.clone()),
        _ => {
            let k = kind.unwrap_or(EffectKind::Breathe);
            (EffectParams::new(k), Trigger::Manual, k.name().to_string())
        }
    };
    let (d, page, save, cancel, header) = page_dialog(ui, if original.is_some() { "Edit effect" } else { "New effect" }, "Save");
    let p = Rc::new(RefCell::new(start));
    let fixtures = ui.e().show.fixtures.clone();

    let g = adw::PreferencesGroup::new();
    let name = adw::EntryRow::builder().title("Name").text(&title).build();
    let names: Vec<&str> = EFFECTS.iter().map(|k| k.name()).collect();
    let kind_row = adw::ComboRow::builder().title("Effect").model(&gtk::StringList::new(&names)).selected(EFFECTS.iter().position(|&k| k == p.borrow().kind).unwrap_or(0) as u32).build();
    g.add(&name);
    g.add(&kind_row);
    let warn = adw::ActionRow::builder().title("Flashing light warning").subtitle("Strobing can trigger seizures in people with photosensitive epilepsy. Gaffer caps it at 8 flashes a second; keep it slow when people are around.").build();
    warn.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    g.add(&warn);
    page.add(&g);

    let look = adw::PreferencesGroup::builder().title("Look").build();
    let slider = |title: &str, v: f32, lo: &str, hi: &str| {
        let row = adw::ActionRow::builder().title(title).build();
        let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        s.set_value(v as f64);
        s.set_draw_value(false);
        s.set_width_request(220);
        s.set_valign(gtk::Align::Center);
        let b = gtk::Box::builder().spacing(6).build();
        b.append(&gtk::Label::builder().label(lo).css_classes(["dim-label", "caption"]).build());
        b.append(&s);
        b.append(&gtk::Label::builder().label(hi).css_classes(["dim-label", "caption"]).build());
        row.add_suffix(&b);
        (row, s)
    };
    let (speed_row, speed) = slider("Speed", p.borrow().speed, "Slow", "Fast");
    let (lvl_row, level) = slider("Brightness", p.borrow().level, "Dim", "Full");
    let own = adw::SwitchRow::builder().title("Keep each light's own colour").active(p.borrow().colors.is_empty()).build();
    let colors_row = adw::ActionRow::builder().title("Colours").build();
    let colors_box = gtk::Box::builder().spacing(4).valign(gtk::Align::Center).build();
    colors_row.add_suffix(&colors_box);
    look.add(&speed_row);
    look.add(&lvl_row);
    look.add(&own);
    look.add(&colors_row);
    page.add(&look);

    let lg = adw::PreferencesGroup::builder().title("Lights").build();
    let all = adw::SwitchRow::builder().title("All lights").active(p.borrow().fixtures.is_empty()).build();
    lg.add(&all);
    let mut checks = Vec::new();
    for (i, f) in fixtures.iter().enumerate() {
        let c = gtk::CheckButton::builder().active(p.borrow().fixtures.contains(&f.id) || p.borrow().fixtures.is_empty()).valign(gtk::Align::Center).build();
        let row = adw::ActionRow::builder().title(glib::markup_escape_text(&f.name)).subtitle(format!("Light {} · {}", i + 1, crate::ui::channels_text(f))).activatable_widget(&c).build();
        row.add_prefix(&c);
        lg.add(&row);
        checks.push((f.id, c, row));
    }
    page.add(&lg);
    let (tg, trigger) = trigger_group(trig);
    page.add(&tg);

    // Colour buttons, rebuilt when one is added or removed.
    let rebuild_colors: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    {
        let (p, bx, rc) = (p.clone(), colors_box.clone(), rebuild_colors.clone());
        let f: Rc<dyn Fn()> = Rc::new(move || {
            crate::ui::clear(&bx);
            let n = p.borrow().colors.len();
            for i in 0..n {
                let dlg = gtk::ColorDialog::builder().with_alpha(false).build();
                let b = gtk::ColorDialogButton::new(Some(dlg));
                b.set_rgba(&rgba(p.borrow().colors[i]));
                let p2 = p.clone();
                b.connect_rgba_notify(move |b| {
                    if let Some(c) = p2.borrow_mut().colors.get_mut(i) {
                        *c = rgb(&b.rgba());
                    }
                });
                bx.append(&b);
            }
            let minus = gtk::Button::builder().icon_name("list-remove-symbolic").tooltip_text("One colour fewer").css_classes(["flat", "circular"]).sensitive(n > 1).build();
            let plus = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add a colour").css_classes(["flat", "circular"]).sensitive(n < 6).build();
            bx.append(&minus);
            bx.append(&plus);
            let (p2, rc2) = (p.clone(), rc.clone());
            minus.connect_clicked(move |_| {
                p2.borrow_mut().colors.pop();
                if let Some(f) = rc2.borrow().clone() {
                    f();
                }
            });
            let (p2, rc2) = (p.clone(), rc.clone());
            plus.connect_clicked(move |_| {
                let next = crate::ui::PRESETS[(p2.borrow().colors.len() * 3 + 2) % crate::ui::PRESETS.len()].1;
                p2.borrow_mut().colors.push(next);
                if let Some(f) = rc2.borrow().clone() {
                    f();
                }
            });
        });
        *rebuild_colors.borrow_mut() = Some(f.clone());
        f();
    }
    let show_rows = {
        let (p, own, colors_row, warn, rc) = (p.clone(), own.clone(), colors_row.clone(), warn.clone(), rebuild_colors.clone());
        move || {
            let k = p.borrow().kind;
            own.set_visible(k == EffectKind::Breathe);
            let uses = k.uses_colors() && !(k == EffectKind::Breathe && own.is_active());
            colors_row.set_visible(uses);
            if uses && p.borrow().colors.is_empty() {
                p.borrow_mut().colors = if k == EffectKind::Breathe { vec![Rgb::new(0.0, 0.4, 1.0)] } else { k.default_colors() };
                if let Some(f) = rc.borrow().clone() {
                    f();
                }
            }
            warn.set_visible(k == EffectKind::Strobe);
        }
    };
    show_rows();
    let (p2, sr, name2) = (p.clone(), show_rows.clone(), name.clone());
    kind_row.connect_selected_notify(move |r| {
        let k = EFFECTS[(r.selected() as usize).min(EFFECTS.len() - 1)];
        let old = p2.borrow().kind;
        if k != old {
            p2.borrow_mut().kind = k;
            p2.borrow_mut().colors = k.default_colors();
            // Follow the effect's name unless the user named it.
            if name2.text() == old.name() {
                name2.set_text(k.name());
            }
        }
        kind_row_subtitle(r, k);
        sr();
    });
    kind_row_subtitle(&kind_row, p.borrow().kind);
    let (p2, sr) = (p.clone(), show_rows.clone());
    own.connect_active_notify(move |s| {
        if s.is_active() {
            p2.borrow_mut().colors.clear();
        }
        sr();
    });
    let vis_checks = {
        let checks: Vec<adw::ActionRow> = checks.iter().map(|(_, _, r)| r.clone()).collect();
        move |all: bool| {
            for r in &checks {
                r.set_visible(!all);
            }
        }
    };
    vis_checks(all.is_active());
    all.connect_active_notify(move |s| vis_checks(s.is_active()));

    let build: Rc<dyn Fn() -> Automation> = {
        let (p, name, speed, level, all, own) = (p.clone(), name.clone(), speed.clone(), level.clone(), all.clone(), own.clone());
        let checks: Vec<(u32, gtk::CheckButton)> = checks.iter().map(|(id, c, _)| (*id, c.clone())).collect();
        Rc::new(move || {
            let mut e = p.borrow().clone();
            e.speed = speed.value() as f32;
            e.level = level.value() as f32;
            e.fixtures = if all.is_active() { vec![] } else { checks.iter().filter(|(_, c)| c.is_active()).map(|(id, _)| *id).collect() };
            if !all.is_active() && e.fixtures.is_empty() {
                e.fixtures = vec![]; // none ticked: all of them, rather than nothing
            }
            if e.kind == EffectKind::Breathe && own.is_active() {
                e.colors.clear();
            }
            Automation { id: 0, name: name.text().to_string(), trigger: trigger(), action: Action::Effect(e) }
        })
    };
    wire_automation(ui, &d, &header, &save, &cancel, original, build);
    d.present(Some(&ui.win));
}

fn kind_row_subtitle(r: &adw::ComboRow, k: EffectKind) {
    r.set_subtitle(k.blurb());
}

// ---- sequences ------------------------------------------------------------------------------------

pub fn sequence(ui: &Rc<Ui>, id: Option<u32>) {
    let original = id.and_then(|id| ui.e().show.automation(id).cloned());
    let scenes: Vec<(u32, String)> = ui.e().show.scenes.iter().map(|s| (s.id, s.name.clone())).collect();
    let (steps0, repeat0, trig, title) = match &original {
        Some(Automation { action: Action::Sequence { steps, repeat }, trigger, name, .. }) => (steps.clone(), *repeat, *trigger, name.clone()),
        _ => {
            let steps = scenes.iter().take(3).map(|(id, _)| Step { target: Target::Scene(*id), fade: 3.0, hold: 10.0 }).collect();
            (steps, true, Trigger::Manual, "Sequence".to_string())
        }
    };
    let (d, page, save, cancel, header) = page_dialog(ui, if original.is_some() { "Edit sequence" } else { "New sequence" }, "Save");
    let g = adw::PreferencesGroup::new();
    let name = adw::EntryRow::builder().title("Name").text(&title).build();
    let repeat = adw::SwitchRow::builder().title("Repeat").subtitle("Start again after the last step").active(repeat0).build();
    g.add(&name);
    g.add(&repeat);
    page.add(&g);

    let steps = Rc::new(RefCell::new(steps0));
    let holder = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    let sg_wrap = adw::PreferencesGroup::new();
    sg_wrap.add(&holder);
    page.add(&sg_wrap);
    let (tg, trigger) = trigger_group(trig);
    page.add(&tg);

    let mut targets: Vec<String> = scenes.iter().map(|(_, n)| n.clone()).collect();
    targets.push("All off".into());
    let rebuild: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    {
        let (steps, holder, rb) = (steps.clone(), holder.clone(), rebuild.clone());
        let f: Rc<dyn Fn()> = Rc::new(move || {
            crate::ui::clear(&holder);
            let group = adw::PreferencesGroup::builder().title("Steps").description("Each step fades into a scene, then stays there.").build();
            let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Add a step").css_classes(["flat"]).valign(gtk::Align::Center).build();
            group.set_header_suffix(Some(&add));
            let n = steps.borrow().len();
            if n == 0 {
                group.add(&adw::ActionRow::builder().title("No steps yet").subtitle("Press + to add one.").build());
            }
            for i in 0..n {
                let st = steps.borrow()[i];
                let row = adw::ActionRow::new();
                row.add_prefix(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["badge"]).valign(gtk::Align::Center).tooltip_text(format!("Step {}", i + 1)).build());
                let model = gtk::StringList::new(&targets.iter().map(|s| s.as_str()).collect::<Vec<_>>());
                let dd = gtk::DropDown::builder().model(&model).valign(gtk::Align::Center).build();
                let sel = match st.target {
                    Target::Scene(id) => scenes.iter().position(|(s, _)| *s == id).unwrap_or(usize::MAX),
                    Target::AllOff => scenes.len(),
                };
                if sel == usize::MAX {
                    row.set_subtitle("Its scene was deleted: pick another");
                    dd.set_selected(gtk::INVALID_LIST_POSITION);
                } else {
                    dd.set_selected(sel as u32);
                }
                let spin = |v: f32, max: f64, tip: &str| {
                    let s = gtk::SpinButton::with_range(0.0, max, 0.5);
                    s.set_digits(1);
                    s.set_value(v as f64);
                    s.set_valign(gtk::Align::Center);
                    s.set_tooltip_text(Some(tip));
                    s.set_width_chars(4);
                    s
                };
                let fade = spin(st.fade, 600.0, "Seconds to fade into it");
                let hold = spin(st.hold, 7200.0, "Seconds to stay before the next step");
                let bx = gtk::Box::builder().spacing(6).build();
                bx.append(&dd);
                bx.append(&gtk::Label::builder().label("fade").css_classes(["dim-label"]).build());
                bx.append(&fade);
                bx.append(&gtk::Label::builder().label("stay").css_classes(["dim-label"]).build());
                bx.append(&hold);
                let up = gtk::Button::builder().icon_name("go-up-symbolic").css_classes(["flat"]).valign(gtk::Align::Center).sensitive(i > 0).tooltip_text("Move up").build();
                let del = gtk::Button::builder().icon_name("list-remove-symbolic").css_classes(["flat"]).valign(gtk::Align::Center).tooltip_text("Remove step").build();
                bx.append(&up);
                bx.append(&del);
                row.add_suffix(&bx);
                let (s2, sc) = (steps.clone(), scenes.clone());
                dd.connect_selected_notify(move |dd| {
                    let k = dd.selected() as usize;
                    if let Some(st) = s2.borrow_mut().get_mut(i) {
                        st.target = sc.get(k).map(|(id, _)| Target::Scene(*id)).unwrap_or(Target::AllOff);
                    }
                });
                let s2 = steps.clone();
                fade.connect_value_changed(move |s| {
                    if let Some(st) = s2.borrow_mut().get_mut(i) {
                        st.fade = s.value() as f32;
                    }
                });
                let s2 = steps.clone();
                hold.connect_value_changed(move |s| {
                    if let Some(st) = s2.borrow_mut().get_mut(i) {
                        st.hold = s.value() as f32;
                    }
                });
                let (s2, rb2) = (steps.clone(), rb.clone());
                up.connect_clicked(move |_| {
                    s2.borrow_mut().swap(i - 1, i);
                    if let Some(f) = rb2.borrow().clone() {
                        f();
                    }
                });
                let (s2, rb2) = (steps.clone(), rb.clone());
                del.connect_clicked(move |_| {
                    s2.borrow_mut().remove(i);
                    if let Some(f) = rb2.borrow().clone() {
                        f();
                    }
                });
                group.add(&row);
            }
            let (s2, rb2, sc) = (steps.clone(), rb.clone(), scenes.clone());
            add.connect_clicked(move |_| {
                let next = {
                    let st = s2.borrow();
                    // Suggest the scene after the last step's.
                    let last = st.last().and_then(|s| match s.target {
                        Target::Scene(id) => sc.iter().position(|(x, _)| *x == id),
                        Target::AllOff => None,
                    });
                    let k = last.map_or(0, |k| (k + 1) % sc.len().max(1));
                    Step { target: sc.get(k).map(|(id, _)| Target::Scene(*id)).unwrap_or(Target::AllOff), fade: st.last().map_or(3.0, |s| s.fade), hold: st.last().map_or(10.0, |s| s.hold) }
                };
                s2.borrow_mut().push(next);
                if let Some(f) = rb2.borrow().clone() {
                    f();
                }
            });
            holder.append(&group);
        });
        *rebuild.borrow_mut() = Some(f.clone());
        f();
    }
    let build: Rc<dyn Fn() -> Automation> = {
        let (steps, name, repeat) = (steps.clone(), name.clone(), repeat.clone());
        Rc::new(move || Automation { id: 0, name: name.text().to_string(), trigger: trigger(), action: Action::Sequence { steps: steps.borrow().clone(), repeat: repeat.is_active() } })
    };
    wire_automation(ui, &d, &header, &save, &cancel, original, build);
    d.present(Some(&ui.win));
}
