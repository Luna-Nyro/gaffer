//! The soundboard's settings: which sounds there are (files in a folder), what each is called,
//! how loud it plays and which light cue goes with it. Playing audio lives in the app; this is
//! the pure, tested part.

use serde::{Deserialize, Serialize};

use crate::fixture::Rgb;

/// File types the soundboard picks up from its folder.
pub const AUDIO_EXTS: &[&str] = &["wav", "mp3", "ogg", "oga", "opus", "flac", "m4a", "aac", "aif", "aiff", "wma", "webm"];

/// Most sounds the settings remember (files that come and go keep their settings).
pub const MAX_SOUNDS: usize = 500;

/// What the lights do when a sound plays.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum LightCue {
    /// The sound doesn't touch the lights.
    #[default]
    None,
    /// The lights follow the sound's loudness. `color` None = each light keeps its own colour.
    Pulse { color: Option<Rgb> },
    /// One bright flash as the sound starts.
    Flash { color: Rgb },
    /// Fade to a scene.
    Scene { id: u32 },
    /// Start an automation.
    Automation { id: u32 },
    /// All lights dark.
    Blackout,
}

impl LightCue {
    pub fn is_none(&self) -> bool {
        matches!(self, LightCue::None)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sound {
    /// File name inside the soundboard folder (the key).
    pub file: String,
    /// The name on the pad; empty = made from the file name.
    pub name: String,
    /// 0..1
    pub volume: f32,
    /// Start again when it ends, until stopped.
    pub repeat: bool,
    pub cue: LightCue,
    /// When the sound ends (or is stopped), put the lights back how they were.
    pub restore: bool,
}

impl Default for Sound {
    fn default() -> Sound {
        Sound { file: String::new(), name: String::new(), volume: 1.0, repeat: false, cue: LightCue::None, restore: true }
    }
}

impl Sound {
    pub fn new(file: &str) -> Sound {
        Sound { file: file.to_string(), ..Default::default() }
    }

    pub fn label(&self) -> String {
        if self.name.trim().is_empty() { nice_name(&self.file) } else { self.name.trim().to_string() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Soundboard {
    /// The folder the sounds come from; empty = the default (~/Music/Gaffer Sounds).
    pub folder: String,
    /// Volume of the whole board, 0..1.
    pub volume: f32,
    /// Per-file settings, in pad order.
    pub sounds: Vec<Sound>,
    /// The sidebar is showing.
    pub open: bool,
}

impl Default for Soundboard {
    fn default() -> Soundboard {
        Soundboard { folder: String::new(), volume: 0.8, sounds: Vec::new(), open: true }
    }
}

impl Soundboard {
    /// Brings the settings in line with the files now in the folder: new files get default
    /// settings at the end (sorted by name). Returns the pads to show, in order, as indices into
    /// `sounds`; settings of files that are gone are kept (they come back if the file does).
    pub fn sync_files(&mut self, files: &[String]) -> Vec<usize> {
        let mut new: Vec<&String> = files.iter().filter(|f| !self.sounds.iter().any(|s| &s.file == *f)).collect();
        new.sort_by_key(|f| f.to_lowercase());
        new.dedup();
        for f in new {
            self.sounds.push(Sound::new(f));
        }
        if self.sounds.len() > MAX_SOUNDS {
            // Forget the settings of missing files first.
            let mut extra = self.sounds.len() - MAX_SOUNDS;
            self.sounds.retain(|s| {
                if extra > 0 && !files.contains(&s.file) {
                    extra -= 1;
                    false
                } else {
                    true
                }
            });
        }
        self.sounds.iter().enumerate().filter(|(_, s)| files.contains(&s.file)).map(|(i, _)| i).collect()
    }

    pub fn get(&self, file: &str) -> Option<&Sound> {
        self.sounds.iter().find(|s| s.file == file)
    }

    pub fn get_mut(&mut self, file: &str) -> Option<&mut Sound> {
        self.sounds.iter_mut().find(|s| s.file == file)
    }

    /// Moves a pad among the pads that are showing (`shown` as from `sync_files`).
    pub fn move_sound(&mut self, file: &str, delta: i32, shown: &[String]) {
        let Some(pos) = shown.iter().position(|f| f == file) else { return };
        let to = pos as i32 + delta;
        if to < 0 || to as usize >= shown.len() {
            return;
        }
        let other = &shown[to as usize];
        let (Some(a), Some(b)) = (self.sounds.iter().position(|s| s.file == file), self.sounds.iter().position(|s| &s.file == other)) else { return };
        self.sounds.swap(a, b);
    }

    pub fn repair(&mut self) {
        let c = |v: f32, d: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { d };
        self.volume = c(self.volume, 0.8);
        let mut seen = std::collections::HashSet::new();
        self.sounds.retain(|s| is_safe_file_name(&s.file) && seen.insert(s.file.clone()));
        self.sounds.truncate(MAX_SOUNDS);
        for s in &mut self.sounds {
            s.volume = c(s.volume, 1.0);
            s.name = s.name.chars().take(60).collect();
            match &mut s.cue {
                LightCue::Pulse { color: Some(col) } | LightCue::Flash { color: col } => *col = col.clamped(),
                _ => {}
            }
        }
    }

    /// A cue pointing at a scene or automation that was deleted goes back to "nothing".
    pub fn forget(&mut self, scene: Option<u32>, automation: Option<u32>) {
        for s in &mut self.sounds {
            let gone = match s.cue {
                LightCue::Scene { id } => Some(id) == scene,
                LightCue::Automation { id } => Some(id) == automation,
                _ => false,
            };
            if gone {
                s.cue = LightCue::None;
            }
        }
    }
}

/// A plain file name (no folders, not hidden), so a hand-edited show can't point outside the folder.
pub fn is_safe_file_name(f: &str) -> bool {
    !f.is_empty() && f.len() <= 255 && !f.contains('/') && !f.contains('\0') && !f.starts_with('.')
}

pub fn is_audio_file(name: &str) -> bool {
    is_safe_file_name(name) && name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && AUDIO_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// "airhorn_final-v2.mp3" → "Airhorn final v2".
pub fn nice_name(file: &str) -> String {
    let stem = match file.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => file,
    };
    let words: Vec<&str> = stem.split(|c: char| c == '_' || c == '-' || c.is_whitespace()).filter(|w| !w.is_empty()).collect();
    let joined = words.join(" ");
    let mut c = joined.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => file.to_string(),
    }
}

/// Turns a stream of loudness readings into a 0..1 light level that pulses with the beat, for
/// quiet and loud sounds alike (it follows the sound's own recent peak).
#[derive(Clone, Debug, Default)]
pub struct LevelFollower {
    peak_db: Option<f32>,
}

/// dB below the recent peak that counts as "dark".
const RANGE_DB: f32 = 15.0;
/// Quieter than this is silence.
const SILENCE_DB: f32 = -50.0;

impl LevelFollower {
    /// `db`: the loudest channel's RMS in dB (0 = full scale); `dt`: seconds since the last reading.
    pub fn feed(&mut self, db: f64, dt: f32) -> f32 {
        let db = if db.is_finite() { (db as f32).clamp(-120.0, 0.0) } else { -120.0 };
        // The peak sinks 3 dB a second, so a quiet passage after a loud one still pulses.
        let peak = self.peak_db.map_or(db, |p| (p - 3.0 * dt.clamp(0.0, 1.0)).max(db)).max(SILENCE_DB + RANGE_DB * 0.5);
        self.peak_db = Some(peak);
        if db <= SILENCE_DB {
            return 0.0;
        }
        let v = ((db - (peak - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
        v * v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(nice_name("airhorn_final-v2.mp3"), "Airhorn final v2");
        assert_eq!(nice_name("  drum roll .wav"), "Drum roll");
        assert_eq!(nice_name("ünïcode.ogg"), "Ünïcode");
        assert!(is_audio_file("a.MP3"));
        assert!(!is_audio_file(".hidden.mp3"));
        assert!(!is_audio_file("notes.txt"));
        assert!(!is_audio_file(".mp3"));
        let mut s = Sound::new("boom_1.wav");
        assert_eq!(s.label(), "Boom 1");
        s.name = "  Big boom ".into();
        assert_eq!(s.label(), "Big boom");
    }

    #[test]
    fn folder_sync_keeps_settings_and_order() {
        let mut b = Soundboard::default();
        let files = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let shown = b.sync_files(&files(&["b.wav", "A.mp3"]));
        assert_eq!(shown.iter().map(|&i| b.sounds[i].file.as_str()).collect::<Vec<_>>(), ["A.mp3", "b.wav"]);
        b.get_mut("b.wav").unwrap().name = "Bee".into();
        b.move_sound("b.wav", -1, &files(&["A.mp3", "b.wav"]));
        // b.wav is removed, c.ogg added, then b.wav comes back with its name and place.
        let shown = b.sync_files(&files(&["A.mp3", "c.ogg"]));
        assert_eq!(shown.iter().map(|&i| b.sounds[i].file.as_str()).collect::<Vec<_>>(), ["A.mp3", "c.ogg"]);
        let shown = b.sync_files(&files(&["A.mp3", "c.ogg", "b.wav"]));
        assert_eq!(shown.iter().map(|&i| b.sounds[i].label()).collect::<Vec<_>>(), ["Bee", "A", "C"]);
    }

    #[test]
    fn repair_drops_unsafe_and_duplicates() {
        let mut b = Soundboard { volume: f32::NAN, ..Default::default() };
        b.sounds = vec![Sound::new("../x.wav"), Sound::new("ok.wav"), Sound::new("ok.wav"), Sound { volume: 7.0, ..Sound::new("v.wav") }];
        b.repair();
        assert_eq!(b.sounds.len(), 2);
        assert_eq!(b.sounds[1].volume, 1.0);
        assert_eq!(b.volume, 0.8);
        b.sounds[0].cue = LightCue::Scene { id: 4 };
        b.forget(Some(4), None);
        assert!(b.sounds[0].cue.is_none());
    }

    #[test]
    fn follower_pulses_for_loud_and_quiet_sounds() {
        for base in [-6.0, -30.0] {
            let mut f = LevelFollower::default();
            let mut hi = 0.0f32;
            let mut lo = 1.0f32;
            for i in 0..200 {
                // A beat: loud every 10th reading, 12 dB quieter between.
                let db = if i % 10 == 0 { base } else { base - 12.0 };
                let v = f.feed(db, 0.03);
                if i > 20 {
                    if i % 10 == 0 { hi = hi.max(v) } else { lo = lo.min(v) }
                }
            }
            assert!(hi > 0.9 && lo < 0.1, "{base}: {hi} {lo}");
        }
        let mut f = LevelFollower::default();
        assert_eq!(f.feed(f64::NEG_INFINITY, 0.03), 0.0);
        assert_eq!(f.feed(-80.0, 0.03), 0.0);
        assert_eq!(f.feed(f64::NAN, 0.03), 0.0);
    }

    #[test]
    fn cue_json() {
        let s = Sound { cue: LightCue::Pulse { color: None }, ..Sound::new("a.wav") };
        let t = serde_json::to_string(&s).unwrap();
        assert!(t.contains("\"kind\":\"pulse\""), "{t}");
        assert_eq!(serde_json::from_str::<Sound>(&t).unwrap(), s);
        assert_eq!(serde_json::from_str::<Sound>("{\"file\":\"x.wav\"}").unwrap().volume, 1.0);
    }
}
