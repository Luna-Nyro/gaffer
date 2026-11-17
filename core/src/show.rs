//! Everything the user makes, saved as one JSON file: lights, scenes, automations, settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::effect::EffectParams;
use crate::fixture::{Fixture, Look, Rgb};
use crate::sound::Soundboard;

pub const VERSION: u32 = 1;

/// A saved look of some lights.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: u32,
    pub name: String,
    pub looks: BTreeMap<u32, Look>,
}

impl Scene {
    /// Up to `n` colours to draw on the scene's button (lit lights first).
    pub fn swatches(&self, n: usize) -> Vec<Rgb> {
        let mut v: Vec<Rgb> = self.looks.values().filter(|l| l.level > 0.01).map(|l| l.color.scale(0.35 + 0.65 * l.level)).collect();
        v.dedup_by(|a, b| (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs() < 0.05);
        v.truncate(n);
        v
    }
}

/// Where a step of a sequence goes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Scene(u32),
    AllOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub target: Target,
    /// Seconds to fade into it.
    pub fade: f32,
    /// Seconds to stay before the next step.
    pub hold: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Sequence { steps: Vec<Step>, repeat: bool },
    Effect(EffectParams),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    /// Minute of the day (0..1440) on the given weekdays (bit 0 = Monday).
    Daily { minute: u16, days: u8 },
    /// When Gaffer starts.
    OnStart,
}

impl Trigger {
    pub fn describe(&self) -> String {
        match *self {
            Trigger::Manual => "Start by hand".into(),
            Trigger::OnStart => "When Gaffer opens".into(),
            Trigger::Daily { minute, days } => {
                let t = format!("{:02}:{:02}", minute / 60, minute % 60);
                let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                let when = match days & 0x7f {
                    0x7f => "Every day".to_string(),
                    0x1f => "Weekdays".to_string(),
                    0x60 => "Weekends".to_string(),
                    0 => "Never".to_string(),
                    d => (0..7).filter(|i| d & (1 << i) != 0).map(|i| names[i]).collect::<Vec<_>>().join(", "),
                };
                format!("{when} at {t}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Automation {
    pub id: u32,
    pub name: String,
    pub trigger: Trigger,
    pub action: Action,
}

impl Automation {
    pub fn describe(&self) -> String {
        let what = match &self.action {
            Action::Sequence { steps, repeat } => {
                let n = steps.len();
                format!("{n} step{}{}", if n == 1 { "" } else { "s" }, if *repeat { ", repeats" } else { "" })
            }
            Action::Effect(e) => format!("{} effect", e.kind.name()),
        };
        format!("{what} · {}", self.trigger.describe())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputCfg {
    /// None: the built-in simulator.
    pub port: Option<String>,
    /// USB serial number of the interface, to find it again under another /dev name.
    pub serial: Option<String>,
    /// "auto", "pro" or "open".
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Show {
    pub version: u32,
    pub fixtures: Vec<Fixture>,
    pub scenes: Vec<Scene>,
    pub automations: Vec<Automation>,
    /// The lights as last set.
    pub live: BTreeMap<u32, Look>,
    pub master: f32,
    /// Seconds a scene recall fades.
    pub scene_fade: f32,
    pub output: OutputCfg,
    pub next_id: u32,
    /// The soundboard: its folder and each sound's name, volume and light cue.
    pub sounds: Soundboard,
}

impl Default for Show {
    fn default() -> Show {
        Show {
            version: VERSION,
            fixtures: Vec::new(),
            scenes: Vec::new(),
            automations: Vec::new(),
            live: BTreeMap::new(),
            master: 1.0,
            scene_fade: 1.5,
            output: OutputCfg { kind: "auto".into(), ..Default::default() },
            next_id: 1,
            sounds: Soundboard::default(),
        }
    }
}

impl Show {
    pub fn new_id(&mut self) -> u32 {
        let used = self.fixtures.iter().map(|f| f.id).chain(self.scenes.iter().map(|s| s.id)).chain(self.automations.iter().map(|a| a.id));
        let id = self.next_id.max(used.max().unwrap_or(0) + 1);
        self.next_id = id + 1;
        id
    }

    pub fn fixture(&self, id: u32) -> Option<&Fixture> {
        self.fixtures.iter().find(|f| f.id == id)
    }

    pub fn scene(&self, id: u32) -> Option<&Scene> {
        self.scenes.iter().find(|s| s.id == id)
    }

    pub fn automation(&self, id: u32) -> Option<&Automation> {
        self.automations.iter().find(|a| a.id == id)
    }

    /// The first address where `width` channels fit without touching another light.
    pub fn next_free_address(&self, width: usize) -> Option<u16> {
        let width = width.max(1);
        let mut a = 1usize;
        'outer: while a + width - 1 <= crate::fixture::UNIVERSE {
            for f in &self.fixtures {
                if a <= f.last_address() && f.address as usize <= a + width - 1 {
                    a = f.last_address() + 1;
                    continue 'outer;
                }
            }
            return Some(a as u16);
        }
        None
    }

    /// Lights that share channels with `f` (other than itself).
    pub fn clashes(&self, f: &Fixture) -> Vec<String> {
        self.fixtures.iter().filter(|o| o.id != f.id && o.overlaps(f)).map(|o| o.name.clone()).collect()
    }

    pub fn parse(text: &str) -> Result<Show, String> {
        let mut s: Show = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if s.version > VERSION {
            return Err(format!("saved by a newer Gaffer (format {})", s.version));
        }
        s.repair();
        Ok(s)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Makes a loaded (possibly hand-edited) show safe to run.
    pub fn repair(&mut self) {
        self.version = VERSION;
        self.master = if self.master.is_finite() { self.master.clamp(0.0, 1.0) } else { 1.0 };
        self.scene_fade = if self.scene_fade.is_finite() { self.scene_fade.clamp(0.0, 600.0) } else { 1.5 };
        self.fixtures.retain(|f| !f.chans.is_empty());
        for f in &mut self.fixtures {
            f.address = f.address.clamp(1, 512);
            f.chans.truncate(32);
        }
        let ids: std::collections::BTreeSet<u32> = self.fixtures.iter().map(|f| f.id).collect();
        self.live.retain(|id, _| ids.contains(id));
        for l in self.live.values_mut() {
            *l = l.clamped();
        }
        for s in &mut self.scenes {
            s.looks.retain(|id, _| ids.contains(id));
            for l in s.looks.values_mut() {
                *l = l.clamped();
            }
        }
        for a in &mut self.automations {
            match &mut a.action {
                Action::Sequence { steps, .. } => {
                    for st in steps.iter_mut() {
                        st.fade = if st.fade.is_finite() { st.fade.clamp(0.0, 3600.0) } else { 0.0 };
                        st.hold = if st.hold.is_finite() { st.hold.clamp(0.0, 86400.0) } else { 0.0 };
                    }
                }
                Action::Effect(e) => e.repair(),
            }
            if let Trigger::Daily { minute, .. } = &mut a.trigger {
                *minute = (*minute).min(1439);
            }
        }
        self.sounds.repair();
        let scene_ids: std::collections::BTreeSet<u32> = self.scenes.iter().map(|s| s.id).collect();
        let auto_ids: std::collections::BTreeSet<u32> = self.automations.iter().map(|a| a.id).collect();
        for snd in &mut self.sounds.sounds {
            let gone = match snd.cue {
                crate::sound::LightCue::Scene { id } => !scene_ids.contains(&id),
                crate::sound::LightCue::Automation { id } => !auto_ids.contains(&id),
                _ => false,
            };
            if gone {
                snd.cue = crate::sound::LightCue::None;
            }
        }
        let max = self.fixtures.iter().map(|f| f.id).chain(self.scenes.iter().map(|s| s.id)).chain(self.automations.iter().map(|a| a.id)).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;
    use crate::fixture::parse_layout;

    fn fx(id: u32, address: u16, layout: &str) -> Fixture {
        Fixture { id, name: format!("L{id}"), address, chans: parse_layout(layout).unwrap(), profile: "Custom".into() }
    }

    #[test]
    fn free_addresses() {
        let mut s = Show::default();
        assert_eq!(s.next_free_address(3), Some(1));
        s.fixtures.push(fx(1, 1, "rgb"));
        s.fixtures.push(fx(2, 7, "drgb"));
        assert_eq!(s.next_free_address(3), Some(4));
        assert_eq!(s.next_free_address(4), Some(11));
        s.fixtures.push(fx(3, 11, &"d".repeat(32)));
        assert_eq!(s.clashes(&fx(9, 12, "rgb")), vec!["L3".to_string()]);
    }

    #[test]
    fn round_trip_and_repair() {
        let mut s = Show::default();
        s.fixtures.push(fx(1, 1, "rgb"));
        s.live.insert(1, Look::new(0.5, Rgb::new(1.0, 0.0, 0.0)));
        s.live.insert(77, Look::OFF);
        s.scenes.push(Scene { id: 2, name: "Red".into(), looks: s.live.clone() });
        s.automations.push(Automation {
            id: 3,
            name: "Party".into(),
            trigger: Trigger::Daily { minute: 1200, days: 0x7f },
            action: Action::Effect(EffectParams::new(EffectKind::Rainbow)),
        });
        s.repair();
        assert!(!s.live.contains_key(&77));
        let back = Show::parse(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.next_id, 4);
        assert!(Show::parse("{\"version\": 99}").is_err());
        assert_eq!(Show::parse("{}").unwrap().master, 1.0);
    }

    #[test]
    fn trigger_words() {
        assert_eq!(Trigger::Daily { minute: 19 * 60 + 5, days: 0x7f }.describe(), "Every day at 19:05");
        assert_eq!(Trigger::Daily { minute: 0, days: 0b101 }.describe(), "Mon, Wed at 00:00");
    }
}
