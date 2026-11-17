//! Gaffer: a simple DMX lightboard. `gaffer` opens the window; `gaffer ctl <action>` controls the
//! running window (blackout, scenes, automations…), which is what the Touch Bar keys use.

mod audio;
mod bar;
mod dialogs;
mod soundboard;
mod store;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use gaffer_core::Engine;
use gaffer_output::{Kind, Output, Target};
use gtk::{gio, glib};

const APP_ID: &str = "dev.gaffer.Gaffer";

fn usage() {
    println!(
        "Gaffer {} – a simple DMX lightboard\n\n\
         Usage:\n  gaffer                      open the window\n  gaffer ctl <action>         control the open window\n  gaffer --list-ports         show USB serial ports that might be DMX interfaces\n  gaffer --install-bar-scenes add Gaffer's Touch Bar scenes (barkeep)\n  gaffer --remove-bar-scenes  take them out again\n\n{}\n\nThe show is saved in {}",
        env!("CARGO_PKG_VERSION"),
        bar::HELP,
        store::show_path().display()
    );
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("ctl") => {
            let Some(a) = args.get(1) else {
                eprintln!("gaffer ctl <action>\n{}", bar::HELP);
                return glib::ExitCode::from(2);
            };
            return glib::ExitCode::from(bar::send(a) as u8);
        }
        Some("-h" | "--help" | "help") => {
            usage();
            return glib::ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("gaffer {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Some("--list-ports") => {
            let ports = gaffer_output::list_ports();
            if ports.is_empty() {
                println!("No USB serial ports (ttyUSB*/ttyACM*) found.");
            }
            for p in ports {
                let what = match p.guess {
                    Some(Kind::Pro) => "Enttec Pro protocol",
                    Some(Kind::Open) => "Open DMX (cheap USB-serial cable)",
                    Some(Kind::Udmx) => "uDMX dongle",
                    _ => "not recognised as DMX",
                };
                println!("{}  {}  [{}:{} serial {}]  {}", p.path, p.label(), p.vendor, p.product_id, if p.serial.is_empty() { "-" } else { &p.serial }, what);
            }
            return glib::ExitCode::SUCCESS;
        }
        Some(flag @ ("--install-bar-scenes" | "--remove-bar-scenes")) => {
            let (show, _) = store::load();
            let s: Vec<String> = show.scenes.iter().map(|s| s.name.clone()).collect();
            let a: Vec<String> = show.automations.iter().map(|a| a.name.clone()).collect();
            let p = soundboard::pad_names(&show.sounds);
            return match bar::install(&s, &a, &p, flag == "--remove-bar-scenes") {
                Ok(Some(m)) => {
                    println!("{m}");
                    glib::ExitCode::SUCCESS
                }
                Ok(None) => {
                    println!("barkeep isn't set up here (~/.config/barkeep missing): skipped the Touch Bar scenes");
                    glib::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("gaffer: {e}");
                    glib::ExitCode::FAILURE
                }
            };
        }
        Some(other) => {
            eprintln!("gaffer: unknown option '{other}'");
            usage();
            return glib::ExitCode::from(2);
        }
        None => {}
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    let state: Rc<RefCell<Option<Rc<ui::Ui>>>> = Rc::default();
    let st = state.clone();
    app.connect_activate(move |app| {
        if let Some(u) = st.borrow().as_ref() {
            u.win.present();
            return;
        }
        let (show, load_err) = store::load();
        let target = match &show.output.port {
            Some(path) => Target::Port { path: path.clone(), serial: show.output.serial.clone(), kind: Kind::from_key(&show.output.kind) },
            None => Target::Simulator,
        };
        let eng = Arc::new(Mutex::new(Engine::new(show)));
        let e2 = eng.clone();
        let out = Output::start(
            target,
            Box::new(move |t| {
                let clock = store::clock();
                let mut e = e2.lock().unwrap_or_else(|e| e.into_inner());
                *e.tick(t, clock)
            }),
        );
        let u = ui::Ui::build(app, eng, out);
        add_actions(app, &u);
        if let Some(rx) = bar::listen() {
            let u2 = u.clone();
            glib::spawn_future_local(async move {
                while let Ok(c) = rx.recv().await {
                    u2.ctl(c);
                }
            });
        }
        u.win.present();
        if let Some(e) = load_err {
            u.toast(&e);
        }
        *st.borrow_mut() = Some(u);
    });
    let st = state.clone();
    app.connect_shutdown(move |_| {
        if let Some(u) = st.borrow_mut().take() {
            u.stop_all_sounds();
            u.save_now(); // the lights keep their last look, like a console's
        }
    });
    app.run_with_args(&["gaffer"])
}

fn add_actions(app: &adw::Application, u: &Rc<ui::Ui>) {
    let act = |name: &str, accels: &[&str], f: Box<dyn Fn()>| {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| f());
        app.add_action(&a);
        if !accels.is_empty() {
            app.set_accels_for_action(&format!("app.{name}"), accels);
        }
    };
    for (i, p) in bar::PAGES.iter().enumerate() {
        let u = u.clone();
        let key = format!("<Control>{}", i + 1);
        act(&format!("page-{p}"), &[&key], Box::new(move || u.stack.set_visible_child_name(p)));
    }
    let c = |c: bar::Ctl| {
        let u = u.clone();
        Box::new(move || u.ctl(c)) as Box<dyn Fn()>
    };
    act("blackout", &["<Control>b"], c(bar::Ctl::Blackout(None)));
    act("master-up", &["<Control>Up"], c(bar::Ctl::Master(10)));
    act("master-down", &["<Control>Down"], c(bar::Ctl::Master(-10)));
    act("scene-next", &["<Alt>Right"], c(bar::Ctl::SceneStep(1)));
    act("scene-prev", &["<Alt>Left"], c(bar::Ctl::SceneStep(-1)));
    act("stop", &["<Control>period"], c(bar::Ctl::Stop));
    for n in 1..=soundboard::KEYED {
        let u = u.clone();
        let key = format!("F{n}");
        act(&format!("sound-{n}"), &[&key], Box::new(move || u.sound_nth(n)));
    }
    let u2 = u.clone();
    act("sounds-stop", &["<Shift>Escape"], Box::new(move || u2.stop_all_sounds()));
    let u2 = u.clone();
    act("sounds-toggle", &["<Control>k"], Box::new(move || u2.toggle_sounds()));
    let u2 = u.clone();
    act("sounds-add", &[], Box::new(move || u2.add_sounds_dialog()));
    let u2 = u.clone();
    act("sounds-open-folder", &[], Box::new(move || u2.open_sound_folder()));
    let u2 = u.clone();
    act("sounds-choose-folder", &[], Box::new(move || u2.choose_sound_folder()));
    let u2 = u.clone();
    act("sounds-rescan", &[], Box::new(move || u2.rebuild_sounds()));
    let u2 = u.clone();
    act("save-scene", &["<Control>s"], Box::new(move || dialogs::save_scene(&u2)));
    let u2 = u.clone();
    act("add-light", &["<Control>n"], Box::new(move || dialogs::light(&u2, None)));
}
