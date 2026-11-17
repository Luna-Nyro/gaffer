//! Where the show lives on disk, loading and saving it, the wall clock, and a sample show.

use std::path::PathBuf;

use gaffer_core::{Action, Automation, Clock, EffectKind, EffectParams, Fixture, Look, Rgb, Scene, Show, Step, Target, Trigger};

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("gaffer")
}

pub fn show_path() -> PathBuf {
    config_dir().join("show.json")
}

/// The saved show; a broken file is kept aside (never overwritten) and reported.
pub fn load() -> (Show, Option<String>) {
    let path = show_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (Show::default(), None),
    };
    match Show::parse(&text) {
        Ok(s) => (s, None),
        Err(e) => {
            let aside = path.with_extension(format!("broken-{}.json", unix_now()));
            let _ = std::fs::rename(&path, &aside);
            (Show::default(), Some(format!("Couldn't read your saved show ({e}). It was kept as {}", aside.display())))
        }
    }
}

pub fn save(show: &Show) -> std::io::Result<()> {
    let path = show_path();
    std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, show.to_json())?;
    std::fs::rename(&tmp, &path)
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Local wall-clock time.
pub fn clock() -> Option<Clock> {
    let now = unix_now() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return None;
    }
    let local = now as i64 + tm.tm_gmtoff as i64;
    Some(Clock {
        minute: (tm.tm_hour * 60 + tm.tm_min) as u16,
        weekday: ((tm.tm_wday + 6) % 7) as u8, // tm: 0 = Sunday; ours: 0 = Monday
        day: local.div_euclid(86400),
    })
}

/// Four lights, three scenes and two automations to play with, no hardware needed.
pub fn sample_show() -> Show {
    let mut s = Show::default();
    let layouts = [("Left PAR", "LED PAR 7 ch"), ("Right PAR", "LED PAR 7 ch"), ("Back wash", "RGBW"), ("LED strip", "RGB")];
    for (name, prof) in layouts {
        let chans = gaffer_core::fixture::profile_layout(prof).unwrap_or_default();
        let address = s.next_free_address(chans.len()).unwrap_or(1);
        let id = s.new_id();
        s.fixtures.push(Fixture { id, name: name.into(), address, chans, profile: prof.into() });
    }
    let ids: Vec<u32> = s.fixtures.iter().map(|f| f.id).collect();
    let warm = Rgb::new(1.0, 0.62, 0.3);
    let scene = |s: &mut Show, name: &str, looks: Vec<Look>| {
        let id = s.new_id();
        s.scenes.push(Scene { id, name: name.into(), looks: ids.iter().copied().zip(looks).collect() });
        id
    };
    let cosy = scene(&mut s, "Cosy", vec![Look::new(0.6, warm), Look::new(0.6, warm), Look::new(0.3, Rgb::new(1.0, 0.35, 0.1)), Look::new(0.25, warm)]);
    let show = scene(&mut s, "Showtime", vec![Look::new(1.0, Rgb::new(1.0, 0.0, 0.5)), Look::new(1.0, Rgb::new(0.0, 0.4, 1.0)), Look::new(0.8, Rgb::new(0.5, 0.0, 1.0)), Look::new(1.0, Rgb::new(0.0, 0.9, 1.0))]);
    scene(&mut s, "Bright white", vec![Look::new(1.0, Rgb::WHITE); 4]);
    s.live = s.scenes[0].looks.clone();
    let id = s.new_id();
    s.automations.push(Automation {
        id,
        name: "Evening".into(),
        trigger: Trigger::Manual,
        action: Action::Sequence {
            steps: vec![Step { target: Target::Scene(cosy), fade: 5.0, hold: 20.0 }, Step { target: Target::Scene(show), fade: 3.0, hold: 10.0 }],
            repeat: true,
        },
    });
    let id = s.new_id();
    let mut fx = EffectParams::new(EffectKind::Rainbow);
    fx.speed = 0.5;
    s.automations.push(Automation { id, name: "Party rainbow".into(), trigger: Trigger::Manual, action: Action::Effect(fx) });
    s
}
