//! Remote control (`gaffer ctl <action>`, used by the Touch Bar and anything else) and the
//! Touch Bar scenes for barkeep. Both are optional: Gaffer works the same without them.
//!
//! Control: the running app reads action names, one per line, from the FIFO
//! `$XDG_RUNTIME_DIR/gaffer.ctl`. Only names from a fixed list are accepted; nothing is executed.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::PathBuf;

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ctl {
    Blackout(Option<bool>),
    Master(i32),
    Scene(usize),
    SceneStep(i32),
    AllOff,
    Stop,
    Play(usize),
    PlayStep(i32),
    PlayToggle,
    Show,
    Page(&'static str),
    Sound(usize),
    SoundsStop,
    SoundsToggle,
}

pub const PAGES: [&str; 4] = ["lights", "scenes", "automations", "setup"];

pub const HELP: &str = "Actions: blackout, blackout-on, blackout-off, master-up, master-down, scene-1…scene-12, scene-next, scene-prev, all-off, play-1…play-12, play-next, play-prev, play-toggle, stop, sound-1…sound-12, sounds-stop, sounds (show/hide the soundboard), show, page-lights, page-scenes, page-automations, page-setup";

pub fn parse(s: &str) -> Option<Ctl> {
    let s = s.trim();
    if s.len() > 40 {
        return None;
    }
    let num = |p: &str| s.strip_prefix(p).and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=12).contains(n));
    Some(match s {
        "blackout" => Ctl::Blackout(None),
        "blackout-on" => Ctl::Blackout(Some(true)),
        "blackout-off" => Ctl::Blackout(Some(false)),
        "master-up" => Ctl::Master(10),
        "master-down" => Ctl::Master(-10),
        "scene-next" => Ctl::SceneStep(1),
        "scene-prev" => Ctl::SceneStep(-1),
        "all-off" => Ctl::AllOff,
        "stop" => Ctl::Stop,
        "play-next" => Ctl::PlayStep(1),
        "play-prev" => Ctl::PlayStep(-1),
        "play-toggle" => Ctl::PlayToggle,
        "show" => Ctl::Show,
        "sounds-stop" => Ctl::SoundsStop,
        "sounds" => Ctl::SoundsToggle,
        _ => {
            if let Some(n) = num("scene-") {
                Ctl::Scene(n)
            } else if let Some(n) = num("play-") {
                Ctl::Play(n)
            } else if let Some(n) = num("sound-") {
                Ctl::Sound(n)
            } else if let Some(p) = s.strip_prefix("page-") {
                Ctl::Page(PAGES.iter().find(|&&x| x == p)?)
            } else {
                return None;
            }
        }
    })
}

pub fn ctl_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).map(|d| d.join("gaffer.ctl"))
}

/// Starts reading the FIFO in a thread; actions arrive on the returned channel.
pub fn listen() -> Option<async_channel::Receiver<Ctl>> {
    let path = ctl_path()?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_fifo() => {}
        Ok(_) => return None, // something else is there: leave it alone
        Err(_) => {
            let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
            if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
                return None;
            }
        }
    }
    // Read+write, so the FIFO never reports end-of-file when a writer goes away.
    let f = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_CLOEXEC).open(&path).ok()?;
    let (tx, rx) = async_channel::bounded(32);
    std::thread::Builder::new()
        .name("gaffer-ctl".into())
        .spawn(move || {
            let mut r = BufReader::new(f);
            let mut line = Vec::new();
            loop {
                line.clear();
                // Lines are read at most 256 bytes at a time; anything that long never parses.
                match (&mut r).take(256).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if let Some(c) = std::str::from_utf8(&line).ok().and_then(parse) {
                    if tx.send_blocking(c).is_err() {
                        return;
                    }
                }
            }
        })
        .ok()?;
    Some(rx)
}

/// `gaffer ctl <action>`: hand the action to the running app. Returns the exit code.
pub fn send(action: &str) -> i32 {
    if parse(action).is_none() {
        eprintln!("gaffer: no action called '{action}'.\n{HELP}");
        return 2;
    }
    let Some(path) = ctl_path() else {
        eprintln!("gaffer: XDG_RUNTIME_DIR isn't set");
        return 1;
    };
    match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(&path) {
        Ok(mut f) => match f.write_all(format!("{}\n", action.trim()).as_bytes()) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("gaffer: {e}");
                1
            }
        },
        Err(_) if action.trim() == "show" => {
            // Not running: start it.
            let exe = std::env::current_exe().unwrap_or_else(|_| "gaffer".into());
            match std::process::Command::new(exe).stdin(std::process::Stdio::null()).spawn() {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!("gaffer: {e}");
                    1
                }
            }
        }
        Err(_) => {
            eprintln!("gaffer: Gaffer isn't running");
            1
        }
    }
}

// ---- Touch Bar scenes ---------------------------------------------------------------------------

const PREFIX: &str = "Gaffer";
const COLORS: [&str; 3] = ["#3a2408", "#8a5a12", "#3a2408"];
const AMBER: &str = "#ffc66b";

fn key_cmd(icon: &str, action: &str, w: f32) -> Value {
    json!({"icon": icon, "w": w, "cmd": format!("gaffer ctl {action}"), "icon_color": AMBER})
}

fn key_label(label: &str, action: &str, w: f32) -> Value {
    let short: String = label.chars().take(10).collect();
    json!({"label": short, "w": w, "cmd": format!("gaffer ctl {action}")})
}

fn esc() -> Value {
    json!({"icon": "esc", "w": 0.8, "combo": "esc"})
}

fn blackout() -> Value {
    json!({"icon": "mdi:lightbulb-off", "w": 1.1, "cmd": "gaffer ctl blackout", "color": "#5c1a14", "icon_color": "#ffb4a8"})
}

fn page(icon: &str, combo: &str) -> Value {
    json!({"icon": icon, "w": 0.8, "combo": combo, "icon_color": "#d8c3a5"})
}

/// The pages' nav keys, minus the page you're on.
fn nav(here: &str) -> Vec<Value> {
    [("lights", "mdi:lightbulb-group", "ctrl+1"), ("scenes", "mdi:palette", "ctrl+2"), ("automations", "mdi:animation-play", "ctrl+3"), ("setup", "mdi:cog", "ctrl+4")]
        .iter()
        .filter(|(p, _, _)| *p != here)
        .map(|(_, i, c)| page(i, c))
        .collect()
}

/// Opens the Sounds key page.
fn sounds_key() -> Value {
    json!({"icon": "mdi:music-box-multiple", "w": 1.0, "scene": SOUNDS, "icon_color": AMBER})
}

const SOUNDS: &str = "Gaffer: Sounds";

fn scene_keys(scenes: &[String], n: usize, w: f32) -> Vec<Value> {
    scenes.iter().take(n).enumerate().map(|(i, name)| key_label(name, &format!("scene-{}", i + 1), w)).collect()
}

/// barkeep scenes for each Gaffer page, plus a page of sound pads that each of them opens. The
/// keys carry the user's own scene, automation and sound names.
pub fn bar_scenes(scenes: &[String], automations: &[String], sounds: &[String]) -> Vec<Value> {
    let title = |p: &str| format!("title:{PREFIX} – {p}");
    let mut out = Vec::new();
    let mut lights = vec![esc(), blackout(), key_cmd("mdi:lightbulb-on-10", "master-down", 1.0), key_cmd("mdi:lightbulb-on-90", "master-up", 1.0)];
    lights.extend(scene_keys(scenes, 4, 1.6));
    lights.push(key_cmd("mdi:skip-next", "scene-next", 1.0));
    lights.push(sounds_key());
    lights.extend(nav("lights"));
    out.push(json!({"name": "Gaffer: Lights", "match": [title("Lights")], "colors": COLORS, "keys": lights}));

    let mut sc = vec![esc(), blackout(), key_cmd("mdi:skip-previous", "scene-prev", 1.0)];
    sc.extend(scene_keys(scenes, 6, 1.5));
    sc.push(key_cmd("mdi:skip-next", "scene-next", 1.0));
    sc.push(sounds_key());
    sc.extend(nav("scenes"));
    out.push(json!({"name": "Gaffer: Scenes", "match": [title("Scenes")], "colors": COLORS, "keys": sc}));

    let mut au = vec![esc(), blackout(), key_cmd("mdi:stop", "stop", 1.0), key_cmd("mdi:skip-previous", "play-prev", 1.0)];
    au.extend(automations.iter().take(4).enumerate().map(|(i, name)| key_label(name, &format!("play-{}", i + 1), 1.6)));
    au.push(key_cmd("mdi:skip-next", "play-next", 1.0));
    au.push(sounds_key());
    au.extend(nav("automations"));
    out.push(json!({"name": "Gaffer: Automations", "match": [title("Automations")], "colors": COLORS, "keys": au}));

    let mut se = vec![esc(), blackout(), key_cmd("mdi:lightbulb-on-10", "master-down", 1.0), key_cmd("mdi:lightbulb-on-90", "master-up", 1.0)];
    se.push(sounds_key());
    se.extend(nav("setup"));
    out.push(json!({"name": "Gaffer: Setup", "match": [title("Setup")], "colors": COLORS, "keys": se}));

    // Key-only page (no match): barkeep adds a back key in front.
    let mut so = vec![json!({"icon": "mdi:stop", "w": 1.0, "cmd": "gaffer ctl sounds-stop", "color": "#5c1a14", "icon_color": "#ffb4a8"})];
    so.extend(sounds.iter().take(8).enumerate().map(|(i, name)| key_label(name, &format!("sound-{}", i + 1), 1.5)));
    if sounds.is_empty() {
        so.push(json!({"label": "Show sounds", "w": 2.0, "cmd": "gaffer ctl sounds"}));
    }
    out.push(json!({"name": SOUNDS, "match": [], "colors": COLORS, "keys": so}));
    out
}

fn barkeep_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let dir = base.join("barkeep");
    dir.is_dir().then(|| dir.join("scenes.json"))
}

fn is_ours(s: &Value) -> bool {
    s["name"].as_str().is_some_and(|n| n.starts_with("Gaffer: "))
}

/// Merges (or with `remove`, takes out) Gaffer's scenes in barkeep's scenes.json.
/// Ok(None) = barkeep isn't set up here (nothing done).
pub fn install(scenes: &[String], automations: &[String], sounds: &[String], remove: bool) -> Result<Option<String>, String> {
    let Some(path) = barkeep_file() else { return Ok(None) };
    let mut doc: Value = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => json!({"scenes": []}),
    };
    if !doc.is_object() || (!doc["scenes"].is_array() && !doc["scenes"].is_null()) {
        return Err(format!("{} isn't a barkeep scenes file", path.display()));
    }
    let backup = path.with_extension("json.bak-before-gaffer");
    if path.exists() && !backup.exists() {
        std::fs::copy(&path, &backup).map_err(|e| e.to_string())?;
    }
    let rest: Vec<Value> = doc["scenes"].as_array().cloned().unwrap_or_default().into_iter().filter(|s| !is_ours(s)).collect();
    let mut all = if remove { vec![] } else { bar_scenes(scenes, automations, sounds) };
    let n = all.len();
    all.extend(rest);
    doc["scenes"] = Value::Array(all);
    let tmp = path.with_extension("json.gaffer-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(Some(if remove { format!("removed Gaffer's Touch Bar scenes from {}", path.display()) } else { format!("installed {n} Gaffer Touch Bar scenes in {}", path.display()) }))
}

/// Are Gaffer's scenes installed (so the app keeps their scene names up to date)?
pub fn installed() -> bool {
    barkeep_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|d| d["scenes"].as_array().is_some_and(|l| l.iter().any(is_ours)))
}

/// The scene/automation names currently on the bar, to tell whether they need updating.
pub fn installed_names() -> Option<Value> {
    let p = barkeep_file()?;
    let d: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    let l = d["scenes"].as_array()?;
    Some(Value::Array(l.iter().filter(|s| is_ours(s)).cloned().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_known_actions() {
        assert_eq!(parse("blackout\n"), Some(Ctl::Blackout(None)));
        assert_eq!(parse("scene-3"), Some(Ctl::Scene(3)));
        assert_eq!(parse("scene-13"), None);
        assert_eq!(parse("scene-0"), None);
        assert_eq!(parse("play-12"), Some(Ctl::Play(12)));
        assert_eq!(parse("page-setup"), Some(Ctl::Page("setup")));
        assert_eq!(parse("page-x"), None);
        assert_eq!(parse("rm -rf /"), None);
        assert_eq!(parse("sound-12"), Some(Ctl::Sound(12)));
        assert_eq!(parse("sound-13"), None);
        assert_eq!(parse("sounds-stop"), Some(Ctl::SoundsStop));
        assert_eq!(parse("sounds"), Some(Ctl::SoundsToggle));
        assert_eq!(parse(&"a".repeat(500)), None);
    }

    #[test]
    fn scenes_are_well_formed() {
        let names: Vec<String> = ["Cosy", "A very long scene name"].iter().map(|s| s.to_string()).collect();
        let pads: Vec<String> = (1..=10).map(|i| format!("Sound {i}")).collect();
        let sc = bar_scenes(&names, &["Party".into()], &pads);
        assert_eq!(sc.len(), 5);
        for s in &sc {
            assert!(is_ours(s));
            let keys = s["keys"].as_array().unwrap();
            if s["name"] == SOUNDS {
                assert_eq!(keys.len(), 9, "stop + 8 pads");
            } else {
                assert_eq!(keys[0]["icon"], "esc");
                assert!(keys.iter().any(|k| k["scene"] == SOUNDS));
            }
            let w: f64 = keys.iter().map(|k| k["w"].as_f64().unwrap()).sum();
            assert!(w <= 17.0 - if s["name"] == SOUNDS { 1.0 } else { 0.0 }, "{} is {w} wide", s["name"]);
            for k in keys {
                if let Some(c) = k["cmd"].as_str() {
                    let a = c.strip_prefix("gaffer ctl ").unwrap();
                    assert!(parse(a).is_some(), "{a}");
                }
            }
        }
        assert_eq!(sc[0]["keys"][4]["label"], "Cosy");
        assert_eq!(sc[0]["keys"][5]["label"], "A very lon");
    }
}
