//! The running lightboard: live looks, fades, the automation that is playing, master and
//! blackout. `tick` is called about 40 times a second and builds the DMX frame.

use std::collections::BTreeMap;

use crate::effect::EffectParams;
use crate::fixture::{Fixture, Look, Rgb, UNIVERSE};
use crate::show::{Action, Automation, Scene, Show, Step, Target, Trigger};
use crate::sound::LightCue;

/// Seconds the Identify flash lasts.
pub const IDENTIFY_S: f64 = 3.0;
/// Seconds blackout and master take to reach their new level.
pub const GAIN_RAMP_S: f32 = 0.25;
/// Shortest step a sequence takes, so a sequence of zero-length steps can't spin.
pub const MIN_STEP_S: f64 = 0.1;
/// A sound's scene cue fades no slower than this, so it lands with the sound.
pub const CUE_FADE_MAX: f32 = 0.5;
/// Seconds the lights take to go back when a sound ends.
pub const CUE_BACK_S: f32 = 0.6;
/// Seconds a sound's flash takes to die away.
pub const FLASH_S: f64 = 0.7;
/// Seconds a pulse takes to fall back after a beat.
const PULSE_RELEASE_S: f32 = 0.25;

/// Wall-clock time, for daily automations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// 0..1440
    pub minute: u16,
    /// 0 = Monday
    pub weekday: u8,
    /// Days since some fixed date (only used to tell days apart).
    pub day: i64,
}

#[derive(Clone, Debug)]
struct Fade {
    from: BTreeMap<u32, Look>,
    to: BTreeMap<u32, Look>,
    start: f64,
    dur: f64,
}

/// Lights following a sound (Pulse) or a flash at a sound's start.
#[derive(Clone, Debug)]
struct Pulse {
    owner: u64,
    color: Option<Rgb>,
    /// Some(start) for a flash.
    flash: Option<f64>,
    target: f32,
    level: f32,
    /// How much of the light the pulse has (fades in and out).
    mix: f32,
    ending: bool,
}

impl Pulse {
    fn apply(&self, under: Look, now: f64) -> Look {
        match self.flash {
            Some(start) => {
                let k = (1.0 - (now - start) / FLASH_S).clamp(0.0, 1.0) as f32;
                under.lerp(Look::new(1.0, self.color.unwrap_or(Rgb::WHITE)), k * k)
            }
            None => under.lerp(Look::new(self.level, self.color.unwrap_or(under.color)), self.mix),
        }
    }
}

/// How the lights were before a sound's cue, to go back to when it ends.
#[derive(Clone, Debug)]
struct Restore {
    looks: BTreeMap<u32, Look>,
    blackout: bool,
    running: Option<u32>,
    scene: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Running {
    Sequence { id: u32, step: usize, started: f64 },
    Effect { id: u32, started: f64 },
}

impl Running {
    pub fn id(&self) -> u32 {
        match *self {
            Running::Sequence { id, .. } | Running::Effect { id, .. } => id,
        }
    }
}

pub struct Engine {
    pub show: Show,
    pub blackout: bool,
    gain: f32,
    fade: Option<Fade>,
    running: Option<Running>,
    identify: Option<(u32, f64)>,
    now: f64,
    started: bool,
    last_minute: Option<i64>,
    /// The scene last recalled, while nothing has changed since.
    pub active_scene: Option<u32>,
    /// The DMX universe as last built.
    pub frame: [u8; UNIVERSE],
    /// What each light looks like right now (after effects, master and blackout).
    pub shown: BTreeMap<u32, Rgb>,
    /// The show changed and should be saved.
    pub dirty: bool,
    /// Short messages for the user ("Stopped Party").
    pub notices: Vec<String>,
    pulse: Option<Pulse>,
    /// The sound whose cue the lights show now.
    cue_owner: Option<u64>,
    restore: Option<Restore>,
}

fn smooth(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    t * t * (3.0 - 2.0 * t)
}

impl Engine {
    pub fn new(mut show: Show) -> Engine {
        show.repair();
        Engine {
            show,
            blackout: false,
            gain: 0.0, // lights come up gently when Gaffer starts
            fade: None,
            running: None,
            identify: None,
            now: 0.0,
            started: false,
            last_minute: None,
            active_scene: None,
            frame: [0; UNIVERSE],
            shown: BTreeMap::new(),
            dirty: false,
            notices: Vec::new(),
            pulse: None,
            cue_owner: None,
            restore: None,
        }
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    // ---- lights ------------------------------------------------------------------------------

    pub fn look(&self, id: u32) -> Look {
        self.show.live.get(&id).copied().unwrap_or(Look::OFF)
    }

    /// The user sets a light by hand. Takes it out of any fade and stops an automation that
    /// drives it, so the user is always in control.
    pub fn set_look(&mut self, id: u32, look: Look) {
        if self.show.fixture(id).is_none() {
            return;
        }
        if let Some(f) = &mut self.fade {
            f.to.remove(&id);
            f.from.remove(&id);
        }
        if self.drives(id) {
            self.stop_with_notice();
        }
        self.user_takes_over();
        if let Some(p) = &mut self.pulse {
            p.ending = true;
        }
        self.show.live.insert(id, look.clamped());
        self.active_scene = None;
        self.dirty = true;
    }

    /// Sets every light (brightness and/or colour).
    pub fn set_all(&mut self, level: Option<f32>, color: Option<Rgb>) {
        let ids: Vec<u32> = self.show.fixtures.iter().map(|f| f.id).collect();
        for id in ids {
            let mut l = self.look(id);
            if let Some(v) = level {
                l.level = v;
            }
            if let Some(c) = color {
                l.color = c;
                if level.is_none() && l.level <= 0.001 {
                    l.level = 1.0; // picking a colour for dark lights turns them on
                }
            }
            self.set_look(id, l);
        }
    }

    pub fn set_master(&mut self, v: f32) {
        self.show.master = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        self.dirty = true;
    }

    pub fn set_blackout(&mut self, on: bool) {
        self.blackout = on;
    }

    pub fn identify(&mut self, id: u32) {
        self.identify = Some((id, self.now + IDENTIFY_S));
    }

    pub fn identifying(&self) -> Option<u32> {
        self.identify.filter(|(_, until)| self.now < *until).map(|(id, _)| id)
    }

    pub fn add_fixture(&mut self, mut f: Fixture) -> Result<u32, String> {
        check_fixture(&f)?;
        f.id = self.show.new_id();
        let id = f.id;
        self.show.live.insert(id, Look::new(1.0, Rgb::WHITE));
        self.show.fixtures.push(f);
        self.dirty = true;
        Ok(id)
    }

    pub fn update_fixture(&mut self, f: Fixture) -> Result<(), String> {
        check_fixture(&f)?;
        let slot = self.show.fixtures.iter_mut().find(|o| o.id == f.id).ok_or("that light is gone")?;
        *slot = f;
        self.dirty = true;
        Ok(())
    }

    pub fn remove_fixture(&mut self, id: u32) {
        self.show.fixtures.retain(|f| f.id != id);
        self.show.live.remove(&id);
        for s in &mut self.show.scenes {
            s.looks.remove(&id);
        }
        for a in &mut self.show.automations {
            if let Action::Effect(e) = &mut a.action {
                e.fixtures.retain(|&f| f != id);
            }
        }
        if let Some(f) = &mut self.fade {
            f.to.remove(&id);
            f.from.remove(&id);
        }
        self.dirty = true;
    }

    pub fn move_fixture(&mut self, id: u32, delta: i32) {
        let Some(i) = self.show.fixtures.iter().position(|f| f.id == id) else { return };
        let j = (i as i32 + delta).clamp(0, self.show.fixtures.len() as i32 - 1) as usize;
        if i != j {
            let f = self.show.fixtures.remove(i);
            self.show.fixtures.insert(j, f);
            self.dirty = true;
        }
    }

    // ---- scenes ------------------------------------------------------------------------------

    pub fn save_scene(&mut self, name: &str) -> u32 {
        let id = self.show.new_id();
        let looks = self.snapshot();
        self.show.scenes.push(Scene { id, name: clean_name(name, "Scene"), looks });
        self.active_scene = Some(id);
        self.dirty = true;
        id
    }

    /// Stores the current look into an existing scene.
    pub fn update_scene(&mut self, id: u32) {
        let looks = self.snapshot();
        if let Some(s) = self.show.scenes.iter_mut().find(|s| s.id == id) {
            s.looks = looks;
            self.active_scene = Some(id);
            self.dirty = true;
        }
    }

    pub fn rename_scene(&mut self, id: u32, name: &str) {
        if let Some(s) = self.show.scenes.iter_mut().find(|s| s.id == id) {
            s.name = clean_name(name, &s.name);
            self.dirty = true;
        }
    }

    pub fn delete_scene(&mut self, id: u32) {
        self.show.scenes.retain(|s| s.id != id);
        self.show.sounds.forget(Some(id), None);
        if self.active_scene == Some(id) {
            self.active_scene = None;
        }
        self.dirty = true;
    }

    pub fn move_scene(&mut self, id: u32, delta: i32) {
        let Some(i) = self.show.scenes.iter().position(|s| s.id == id) else { return };
        let j = (i as i32 + delta).clamp(0, self.show.scenes.len() as i32 - 1) as usize;
        if i != j {
            let s = self.show.scenes.remove(i);
            self.show.scenes.insert(j, s);
            self.dirty = true;
        }
    }

    fn snapshot(&self) -> BTreeMap<u32, Look> {
        self.show.fixtures.iter().map(|f| (f.id, self.look(f.id))).collect()
    }

    /// The user recalls a scene: it fades in, and whatever automation was playing stops.
    pub fn recall(&mut self, id: u32, fade: Option<f32>) -> bool {
        let Some(scene) = self.show.scene(id) else { return false };
        let to = scene.looks.clone();
        self.user_takes_over();
        if self.running.is_some() {
            self.stop_with_notice();
        }
        self.go_to(to, fade.unwrap_or(self.show.scene_fade));
        self.active_scene = Some(id);
        self.dirty = true;
        true
    }

    /// Recall the scene after (or before) the active one, wrapping round.
    pub fn recall_next(&mut self, delta: i32) -> Option<u32> {
        let n = self.show.scenes.len() as i32;
        if n == 0 {
            return None;
        }
        let cur = self.active_scene.and_then(|id| self.show.scenes.iter().position(|s| s.id == id)).map(|i| i as i32);
        let next = match cur {
            Some(i) => (i + delta).rem_euclid(n),
            None if delta >= 0 => 0,
            None => n - 1,
        };
        let id = self.show.scenes[next as usize].id;
        self.recall(id, None);
        Some(id)
    }

    /// Fades every light out.
    pub fn all_off(&mut self, fade: f32) {
        self.user_takes_over();
        if self.running.is_some() {
            self.stop_with_notice();
        }
        let to = self.off_looks();
        self.go_to(to, fade);
        self.active_scene = None;
        self.dirty = true;
    }

    fn off_looks(&self) -> BTreeMap<u32, Look> {
        self.show.fixtures.iter().map(|f| (f.id, Look { level: 0.0, color: self.look(f.id).color })).collect()
    }

    fn go_to(&mut self, to: BTreeMap<u32, Look>, fade: f32) {
        let to: BTreeMap<u32, Look> = to.into_iter().filter(|(id, _)| self.show.fixture(*id).is_some()).collect();
        if fade <= 0.0 || !fade.is_finite() {
            for (id, l) in to {
                self.show.live.insert(id, l);
            }
            self.fade = None;
            return;
        }
        let from = to.keys().map(|&id| (id, self.look(id))).collect();
        self.fade = Some(Fade { from, to, start: self.now, dur: fade as f64 });
    }

    pub fn fading(&self) -> bool {
        self.fade.is_some()
    }

    // ---- automations -------------------------------------------------------------------------

    pub fn save_automation(&mut self, mut a: Automation) -> u32 {
        a.name = clean_name(&a.name, "Automation");
        if let Action::Effect(e) = &mut a.action {
            e.repair();
        }
        if let Some(slot) = self.show.automations.iter_mut().find(|o| o.id == a.id && a.id != 0) {
            *slot = a.clone();
            // Restart it so the changes show.
            if self.running.map(|r| r.id()) == Some(a.id) {
                self.play(a.id);
            }
        } else {
            a.id = self.show.new_id();
            self.show.automations.push(a.clone());
        }
        self.dirty = true;
        a.id
    }

    pub fn delete_automation(&mut self, id: u32) {
        if self.running.map(|r| r.id()) == Some(id) {
            self.running = None;
        }
        self.show.automations.retain(|a| a.id != id);
        self.show.sounds.forget(None, Some(id));
        self.dirty = true;
    }

    pub fn running(&self) -> Option<Running> {
        self.running
    }

    pub fn running_name(&self) -> Option<String> {
        self.running.and_then(|r| self.show.automation(r.id())).map(|a| a.name.clone())
    }

    pub fn play(&mut self, id: u32) -> bool {
        let Some(a) = self.show.automation(id).cloned() else { return false };
        self.user_takes_over();
        self.blackout = false;
        match &a.action {
            Action::Sequence { steps, .. } => {
                if steps.is_empty() {
                    self.notices.push(format!("{} has no steps yet", a.name));
                    return false;
                }
                self.running = Some(Running::Sequence { id, step: 0, started: self.now });
                self.start_step(steps[0]);
            }
            Action::Effect(_) => {
                self.running = Some(Running::Effect { id, started: self.now });
            }
        }
        self.active_scene = None;
        true
    }

    pub fn stop(&mut self) {
        self.running = None;
    }

    /// Play/stop for one key: stops what plays, else plays the automation after the last one.
    pub fn play_next(&mut self, delta: i32) -> Option<u32> {
        let n = self.show.automations.len() as i32;
        if n == 0 {
            return None;
        }
        let cur = self.running.and_then(|r| self.show.automations.iter().position(|a| a.id == r.id())).map(|i| i as i32);
        let next = match cur {
            Some(i) => (i + delta).rem_euclid(n),
            None => 0,
        };
        let id = self.show.automations[next as usize].id;
        self.play(id);
        Some(id)
    }

    fn stop_with_notice(&mut self) {
        if let Some(name) = self.running_name() {
            self.notices.push(format!("Stopped {name}"));
        }
        self.running = None;
    }

    /// Does the playing automation control this light?
    fn drives(&self, id: u32) -> bool {
        match self.running {
            Some(Running::Sequence { .. }) => true,
            Some(Running::Effect { id: a, .. }) => match self.show.automation(a).map(|a| &a.action) {
                Some(Action::Effect(e)) => e.fixtures.is_empty() || e.fixtures.contains(&id),
                _ => false,
            },
            None => false,
        }
    }

    fn start_step(&mut self, st: Step) {
        let to = match st.target {
            Target::Scene(sid) => match self.show.scene(sid) {
                Some(s) => s.looks.clone(),
                None => return, // scene was deleted: just wait out the step
            },
            Target::AllOff => self.off_looks(),
        };
        self.go_to(to, st.fade);
    }

    fn effect_of(&self, id: u32) -> Option<&EffectParams> {
        match self.show.automation(id).map(|a| &a.action) {
            Some(Action::Effect(e)) => Some(e),
            _ => None,
        }
    }

    // ---- sound cues ------------------------------------------------------------------------

    /// Something the user did by hand wins over a sound's cue: the lights won't jump back when
    /// the sound ends.
    fn user_takes_over(&mut self) {
        self.restore = None;
        self.cue_owner = None;
    }

    fn capture(&self) -> Restore {
        let mut looks = self.snapshot();
        if let Some(f) = &self.fade {
            looks.extend(f.to.clone());
        }
        Restore { looks, blackout: self.blackout, running: self.running.map(|r| r.id()), scene: self.active_scene }
    }

    /// A sound (`owner` = any number that tells sounds apart) starts: its light cue happens.
    /// With `restore`, the lights go back to how they are now when the sound ends. The latest
    /// sound's cue wins; going back returns to how things were before the first of them.
    /// Pulse and Flash only lay over the lights, and fade away by themselves.
    pub fn cue_start(&mut self, owner: u64, cue: &LightCue, restore: bool) {
        match *cue {
            LightCue::None => return,
            // Overlays: they fade out on their own, so they leave the other cues alone.
            LightCue::Pulse { color } => {
                let carry = self.pulse.as_ref().filter(|p| p.flash.is_none() && !p.ending).map(|p| (p.mix, p.level));
                let (mix, level) = carry.unwrap_or((0.0, 0.0));
                self.pulse = Some(Pulse { owner, color, flash: None, target: 0.0, level, mix, ending: false });
                return;
            }
            LightCue::Flash { color } => {
                self.pulse = Some(Pulse { owner, color: Some(color), flash: Some(self.now), target: 1.0, level: 1.0, mix: 1.0, ending: false });
                return;
            }
            _ => {}
        }
        let prev = self.restore.take();
        let back = if restore { Some(prev.unwrap_or_else(|| self.capture())) } else { None };
        // A new look takes the lights from a pulse.
        if let Some(p) = &mut self.pulse {
            p.ending = true;
        }
        match *cue {
            LightCue::None | LightCue::Pulse { .. } | LightCue::Flash { .. } => {}
            LightCue::Scene { id } => {
                let f = self.show.scene_fade.min(CUE_FADE_MAX);
                self.recall(id, Some(f));
            }
            LightCue::Automation { id } => {
                self.play(id);
            }
            LightCue::Blackout => self.blackout = true,
        }
        self.restore = back;
        self.cue_owner = Some(owner);
    }

    /// How loud the sound is now (0..1, from a LevelFollower), for a Pulse cue.
    pub fn cue_level(&mut self, owner: u64, level: f32) {
        if let Some(p) = &mut self.pulse {
            if p.owner == owner && p.flash.is_none() && !p.ending {
                p.target = if level.is_finite() { level.clamp(0.0, 1.0) } else { 0.0 };
            }
        }
    }

    /// The sound ended or was stopped.
    pub fn cue_end(&mut self, owner: u64) {
        if let Some(p) = &mut self.pulse {
            if p.owner == owner && p.flash.is_none() {
                p.ending = true;
            }
        }
        if self.cue_owner != Some(owner) {
            return;
        }
        self.cue_owner = None;
        let Some(r) = self.restore.take() else { return };
        // Nothing was done by hand since the cue (that drops the restore), so what plays now
        // was started by sounds.
        self.running = None;
        self.go_to(r.looks, CUE_BACK_S);
        if let Some(id) = r.running {
            if self.show.automation(id).is_some() {
                self.play(id);
                self.restore = None;
            }
        }
        self.blackout = r.blackout;
        self.active_scene = r.scene;
        self.dirty = true;
    }

    /// Is a sound pulsing or flashing the lights?
    pub fn pulsing(&self) -> bool {
        self.pulse.as_ref().is_some_and(|p| !p.ending)
    }

    fn advance_pulse(&mut self, dt: f32) {
        let now = self.now;
        let Some(p) = &mut self.pulse else { return };
        let done = match p.flash {
            Some(start) => now - start >= FLASH_S,
            None => {
                p.level = if p.target >= p.level { p.target } else { (p.level - dt / PULSE_RELEASE_S).max(p.target) };
                p.mix = if p.ending { p.mix - dt / CUE_BACK_S } else { (p.mix + dt / 0.08).min(1.0) };
                p.ending && p.mix <= 0.0
            }
        };
        if done {
            self.pulse = None;
        }
    }

    // ---- the frame ---------------------------------------------------------------------------

    /// Advances to `now` (seconds, monotonic) and rebuilds the frame.
    pub fn tick(&mut self, now: f64, clock: Option<Clock>) -> &[u8; UNIVERSE] {
        let dt = (now - self.now).clamp(0.0, 1.0) as f32;
        self.now = now;
        self.triggers(clock);
        self.advance_sequence();
        self.advance_fade();
        self.advance_pulse(dt);

        let target = if self.blackout { 0.0 } else { self.show.master };
        let step = dt / GAIN_RAMP_S;
        self.gain = if self.gain < target { (self.gain + step).min(target) } else { (self.gain - step).max(target) };

        let effect = match self.running {
            Some(Running::Effect { id, started }) => self.effect_of(id).cloned().map(|e| (e, (now - started) as f32)),
            _ => None,
        };
        let targets: Vec<u32> = match &effect {
            Some((e, _)) => self.show.fixtures.iter().map(|f| f.id).filter(|id| e.fixtures.is_empty() || e.fixtures.contains(id)).collect(),
            None => vec![],
        };
        let ident = self.identifying();
        self.frame = [0; UNIVERSE];
        self.shown.clear();
        for f in &self.show.fixtures {
            let mut look = self.show.live.get(&f.id).copied().unwrap_or(Look::OFF);
            let mut gain = self.gain;
            if let Some((e, t)) = &effect {
                if let Some(i) = targets.iter().position(|&id| id == f.id) {
                    look = e.look(*t, i, targets.len(), look);
                }
            }
            if let Some(p) = &self.pulse {
                look = p.apply(look, now);
            }
            if ident == Some(f.id) {
                let left = self.identify.map_or(0.0, |(_, u)| u - now);
                look = Look::new(if (left * 5.0) as i64 % 2 == 0 { 1.0 } else { 0.0 }, Rgb::WHITE);
                gain = 1.0;
            }
            f.render(look, gain, &mut self.frame);
            self.shown.insert(f.id, f.shown(look, gain));
        }
        &self.frame
    }

    fn triggers(&mut self, clock: Option<Clock>) {
        if !self.started {
            self.started = true;
            if let Some(id) = self.show.automations.iter().find(|a| a.trigger == Trigger::OnStart).map(|a| a.id) {
                self.play(id);
            }
        }
        let Some(c) = clock else { return };
        let key = c.day * 1440 + c.minute as i64;
        if self.last_minute == Some(key) {
            return;
        }
        let first = self.last_minute.is_none();
        self.last_minute = Some(key);
        if first {
            return; // only fire on the minute changing, never for a minute already under way
        }
        let due: Vec<u32> = self
            .show
            .automations
            .iter()
            .filter(|a| matches!(a.trigger, Trigger::Daily { minute, days } if minute == c.minute && days & (1 << c.weekday.min(6)) != 0))
            .map(|a| a.id)
            .collect();
        if let Some(&id) = due.last() {
            let name = self.show.automation(id).map(|a| a.name.clone()).unwrap_or_default();
            self.play(id);
            self.notices.push(format!("Started {name} (scheduled)"));
        }
    }

    fn advance_sequence(&mut self) {
        let Some(Running::Sequence { id, step, started }) = self.running else { return };
        let Some(Action::Sequence { steps, repeat }) = self.show.automation(id).map(|a| a.action.clone()) else {
            self.running = None;
            return;
        };
        let Some(st) = steps.get(step).copied() else {
            self.running = None;
            return;
        };
        let length = (st.fade as f64 + st.hold as f64).max(MIN_STEP_S);
        if self.now - started < length {
            return;
        }
        let next = step + 1;
        let next = if next < steps.len() {
            next
        } else if repeat {
            0
        } else {
            self.running = None;
            self.dirty = true;
            return;
        };
        self.running = Some(Running::Sequence { id, step: next, started: self.now });
        self.start_step(steps[next]);
    }

    fn advance_fade(&mut self) {
        let Some(f) = &self.fade else { return };
        let t = (self.now - f.start) / f.dur.max(1e-6);
        if t >= 1.0 {
            let to = f.to.clone();
            self.show.live.extend(to);
            self.fade = None;
            self.dirty = true;
            return;
        }
        let e = smooth(t);
        let next: Vec<(u32, Look)> = f.to.iter().map(|(&id, &to)| (id, f.from.get(&id).copied().unwrap_or(Look::OFF).lerp(to, e))).collect();
        self.show.live.extend(next);
    }

    /// The show to save (live looks mid-fade are saved as their target).
    pub fn show_to_save(&self) -> Show {
        let mut s = self.show.clone();
        if let Some(f) = &self.fade {
            s.live.extend(f.to.clone());
        }
        s
    }
}

fn check_fixture(f: &Fixture) -> Result<(), String> {
    if f.chans.is_empty() {
        return Err("the light needs at least one channel".into());
    }
    if !f.fits() {
        return Err(format!("channels {}–{} go past 512", f.address, f.last_address()));
    }
    Ok(())
}

fn clean_name(name: &str, fallback: &str) -> String {
    let n: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect();
    if n.is_empty() { fallback.to_string() } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;
    use crate::fixture::parse_layout;

    fn engine() -> Engine {
        let mut e = Engine::new(Show::default());
        for (name, layout) in [("A", "rgb"), ("B", "drgb")] {
            let addr = e.show.next_free_address(layout.len()).unwrap();
            e.add_fixture(Fixture { id: 0, name: name.into(), address: addr, chans: parse_layout(layout).unwrap(), profile: "Custom".into() }).unwrap();
        }
        e
    }

    fn run(e: &mut Engine, from: f64, to: f64) {
        let mut t = from;
        while t <= to + 1e-9 {
            e.tick(t, None);
            t += 0.025;
        }
    }

    #[test]
    fn starts_gently_then_full() {
        let mut e = engine();
        e.tick(0.0, None);
        assert_eq!(&e.frame[0..3], &[0, 0, 0]);
        run(&mut e, 0.0, 0.5);
        assert_eq!(&e.frame[0..7], &[255, 255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn blackout_and_master() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_blackout(true);
        run(&mut e, 0.5, 1.0);
        assert!(e.frame.iter().all(|&v| v == 0));
        e.set_blackout(false);
        e.set_master(0.5);
        run(&mut e, 1.0, 1.5);
        assert_eq!(e.frame[0], 128); // RGB light: colour scaled
        assert_eq!(e.frame[3], 128); // dimmer channel of light B
        assert_eq!(e.frame[4], 255); // its colour stays full
    }

    #[test]
    fn scene_fades_in() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(0.0), None);
        let red = e.save_scene("  Red   wash ");
        assert_eq!(e.show.scene(red).unwrap().name, "Red wash");
        e.set_all(Some(1.0), Some(Rgb::new(1.0, 0.0, 0.0)));
        let full = e.save_scene("");
        assert_eq!(e.show.scene(full).unwrap().name, "Scene");
        e.recall(red, Some(2.0));
        run(&mut e, 0.5, 1.5);
        let mid = e.look(e.show.fixtures[0].id).level;
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
        run(&mut e, 1.5, 2.6);
        assert!(!e.fading());
        assert_eq!(e.look(e.show.fixtures[0].id).level, 0.0);
        assert_eq!(e.active_scene, Some(red));
        assert_eq!(e.recall_next(1), Some(full));
        assert_eq!(e.recall_next(1), Some(red));
        assert_eq!(e.recall_next(-1), Some(full));
    }

    #[test]
    fn touching_a_light_takes_it_out_of_the_fade() {
        let mut e = engine();
        let a = e.show.fixtures[0].id;
        e.set_all(Some(0.0), None);
        let off = e.save_scene("Off");
        e.set_all(Some(1.0), None);
        e.recall(off, Some(10.0));
        run(&mut e, 0.0, 1.0);
        e.set_look(a, Look::new(0.7, Rgb::WHITE));
        run(&mut e, 1.0, 11.0);
        assert_eq!(e.look(a).level, 0.7);
        assert_eq!(e.look(e.show.fixtures[1].id).level, 0.0);
    }

    #[test]
    fn sequence_runs_and_repeats() {
        let mut e = engine();
        e.set_all(Some(1.0), Some(Rgb::new(1.0, 0.0, 0.0)));
        let red = e.save_scene("Red");
        e.set_all(Some(1.0), Some(Rgb::new(0.0, 0.0, 1.0)));
        let blue = e.save_scene("Blue");
        let steps = vec![
            Step { target: Target::Scene(red), fade: 0.0, hold: 1.0 },
            Step { target: Target::Scene(blue), fade: 0.0, hold: 1.0 },
        ];
        let id = e.save_automation(Automation { id: 0, name: "RB".into(), trigger: Trigger::Manual, action: Action::Sequence { steps, repeat: true } });
        e.play(id);
        let a = e.show.fixtures[0].id;
        run(&mut e, 0.0, 0.5);
        assert_eq!(e.look(a).color, Rgb::new(1.0, 0.0, 0.0));
        run(&mut e, 0.5, 1.5);
        assert_eq!(e.look(a).color, Rgb::new(0.0, 0.0, 1.0));
        run(&mut e, 1.5, 2.5);
        assert_eq!(e.look(a).color, Rgb::new(1.0, 0.0, 0.0));
        // Touching a light stops it, with a notice.
        e.set_look(a, Look::new(0.3, Rgb::WHITE));
        assert!(e.running().is_none());
        assert_eq!(e.notices, vec!["Stopped RB".to_string()]);
    }

    #[test]
    fn one_shot_sequence_ends_and_all_off_step() {
        let mut e = engine();
        let steps = vec![Step { target: Target::AllOff, fade: 1.0, hold: 0.0 }];
        let id = e.save_automation(Automation { id: 0, name: "Off".into(), trigger: Trigger::Manual, action: Action::Sequence { steps, repeat: false } });
        e.play(id);
        run(&mut e, 0.0, 1.2);
        assert!(e.running().is_none());
        assert!(e.frame.iter().all(|&v| v == 0));
        // Zero-length repeating steps don't spin.
        let steps = vec![Step { target: Target::AllOff, fade: 0.0, hold: 0.0 }; 3];
        let id = e.save_automation(Automation { id: 0, name: "Z".into(), trigger: Trigger::Manual, action: Action::Sequence { steps, repeat: true } });
        e.play(id);
        run(&mut e, 1.2, 1.3);
        assert!(e.running().is_some());
    }

    #[test]
    fn effects_only_touch_their_lights() {
        let mut e = engine();
        let (a, b) = (e.show.fixtures[0].id, e.show.fixtures[1].id);
        let mut p = EffectParams::new(EffectKind::Strobe);
        p.fixtures = vec![a];
        p.speed = 0.0; // 1 Hz, on for the first 35%
        let id = e.save_automation(Automation { id: 0, name: "S".into(), trigger: Trigger::Manual, action: Action::Effect(p) });
        run(&mut e, 0.0, 0.5);
        e.play(id);
        e.tick(0.6, None);
        assert_eq!(e.frame[0], 255);
        e.tick(1.1, None);
        assert_eq!(e.frame[0], 0);
        assert_eq!(e.frame[3], 255); // light B untouched
        e.set_look(b, Look::new(0.5, Rgb::WHITE));
        assert!(e.running().is_some(), "B isn't in the effect");
        e.set_look(a, Look::new(0.5, Rgb::WHITE));
        assert!(e.running().is_none());
    }

    #[test]
    fn schedules_fire_once_on_the_minute() {
        let mut e = engine();
        let id = e.save_automation(Automation {
            id: 0,
            name: "Evening".into(),
            trigger: Trigger::Daily { minute: 19 * 60, days: 1 << 2 }, // Wednesdays
            action: Action::Effect(EffectParams::new(EffectKind::Candle)),
        });
        let at = |minute, weekday, day| Some(Clock { minute, weekday, day });
        e.tick(0.0, at(19 * 60, 2, 10)); // already 19:00 when opened: not fired
        assert!(e.running().is_none());
        e.tick(1.0, at(19 * 60 + 1, 2, 10));
        e.tick(2.0, at(19 * 60, 3, 11)); // Thursday
        assert!(e.running().is_none());
        e.tick(3.0, at(18 * 60 + 59, 2, 17));
        e.tick(4.0, at(19 * 60, 2, 17));
        assert_eq!(e.running().map(|r| r.id()), Some(id));
        assert_eq!(e.notices.last().unwrap(), "Started Evening (scheduled)");
    }

    #[test]
    fn on_start_plays_and_removal_cleans_up() {
        let mut show = Show::default();
        show.automations.push(Automation { id: 5, name: "Hello".into(), trigger: Trigger::OnStart, action: Action::Effect(EffectParams::new(EffectKind::Rainbow)) });
        let mut e = Engine::new(show);
        e.tick(0.0, None);
        assert_eq!(e.running().map(|r| r.id()), Some(5));
        let mut e = engine();
        let a = e.show.fixtures[0].id;
        e.save_scene("x");
        e.remove_fixture(a);
        assert!(e.show.scenes[0].looks.get(&a).is_none());
        e.tick(0.0, None);
        assert!(!e.shown.contains_key(&a));
    }

    #[test]
    fn identify_flashes_through_blackout() {
        let mut e = engine();
        let b = e.show.fixtures[1].id;
        e.set_blackout(true);
        e.identify(b);
        e.tick(0.01, None);
        assert_eq!(&e.frame[3..7], &[255, 255, 255, 255]);
        assert_eq!(&e.frame[0..3], &[0, 0, 0]);
        run(&mut e, 0.01, 3.2);
        assert!(e.identifying().is_none());
        assert!(e.frame.iter().all(|&v| v == 0));
    }

    fn lv(e: &Engine, i: usize) -> f32 {
        let id = e.show.fixtures[i].id;
        e.shown.get(&id).map_or(0.0, |c| c.r.max(c.g).max(c.b))
    }

    #[test]
    fn pulse_follows_the_sound_then_lets_go() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(0.5), Some(Rgb::new(0.0, 0.0, 1.0)));
        e.cue_start(7, &LightCue::Pulse { color: Some(Rgb::new(1.0, 0.0, 0.0)) }, true);
        e.cue_level(7, 1.0);
        run(&mut e, 0.5, 0.7);
        assert!(e.pulsing());
        assert_eq!(&e.frame[0..3], &[255, 0, 0]);
        e.cue_level(7, 0.0);
        e.cue_level(8, 1.0); // another sound's level is ignored
        run(&mut e, 0.7, 1.0);
        assert_eq!(e.frame[0], 0);
        // Ends: the lights fade back to how they were set.
        e.cue_end(7);
        run(&mut e, 1.0, 2.0);
        assert!(!e.pulsing());
        assert_eq!(&e.frame[0..3], &[0, 0, 128]);
    }

    #[test]
    fn pulse_keeps_own_colour_and_hand_wins() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(1.0), Some(Rgb::new(0.0, 1.0, 0.0)));
        e.cue_start(1, &LightCue::Pulse { color: None }, false);
        e.cue_level(1, 0.5);
        run(&mut e, 0.5, 0.8);
        assert_eq!(&e.frame[0..3], &[0, 128, 0]);
        let id = e.show.fixtures[0].id;
        e.set_look(id, Look::new(1.0, Rgb::WHITE));
        run(&mut e, 0.8, 1.6);
        assert_eq!(&e.frame[0..3], &[255, 255, 255]);
    }

    #[test]
    fn flash_dies_away() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(0.0), None);
        e.cue_start(1, &LightCue::Flash { color: Rgb::WHITE }, true);
        run(&mut e, 0.5, 0.55);
        assert!(lv(&e, 0) > 0.8);
        run(&mut e, 0.55, 1.4);
        assert_eq!(lv(&e, 0), 0.0);
        assert!(!e.pulsing());
    }

    #[test]
    fn scene_cue_goes_back_when_the_last_sound_ends() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(1.0), Some(Rgb::new(1.0, 0.0, 0.0)));
        let red = e.save_scene("Red");
        e.set_all(Some(1.0), Some(Rgb::new(0.0, 1.0, 0.0)));
        let green = e.save_scene("Green");
        e.set_all(Some(0.2), Some(Rgb::WHITE));
        e.cue_start(1, &LightCue::Scene { id: red }, true);
        run(&mut e, 0.5, 1.2);
        assert_eq!(&e.frame[0..3], &[255, 0, 0]);
        e.cue_start(2, &LightCue::Scene { id: green }, true);
        run(&mut e, 1.2, 1.9);
        e.cue_end(1); // not the latest: nothing happens
        run(&mut e, 1.9, 2.6);
        assert_eq!(&e.frame[0..3], &[0, 255, 0]);
        e.cue_end(2);
        run(&mut e, 2.6, 3.4);
        assert_eq!(&e.frame[0..3], &[51, 51, 51]);
        assert_eq!(e.active_scene, None);
    }

    #[test]
    fn hand_change_cancels_going_back() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        e.set_all(Some(1.0), Some(Rgb::new(1.0, 0.0, 0.0)));
        let red = e.save_scene("Red");
        e.set_all(Some(0.0), None);
        e.cue_start(1, &LightCue::Scene { id: red }, true);
        run(&mut e, 0.5, 1.2);
        let id = e.show.fixtures[1].id;
        e.set_look(id, Look::new(1.0, Rgb::WHITE));
        e.cue_end(1);
        run(&mut e, 1.2, 2.2);
        assert_eq!(&e.frame[0..3], &[255, 0, 0]);
    }

    #[test]
    fn automation_and_blackout_cues() {
        let mut e = engine();
        run(&mut e, 0.0, 0.5);
        let id = e.save_automation(Automation { id: 0, name: "Rb".into(), trigger: Trigger::Manual, action: Action::Effect(EffectParams::new(EffectKind::Rainbow)) });
        e.cue_start(1, &LightCue::Automation { id }, true);
        assert_eq!(e.running().map(|r| r.id()), Some(id));
        e.cue_end(1);
        assert_eq!(e.running(), None);
        // Without going back, it keeps playing.
        e.cue_start(2, &LightCue::Automation { id }, false);
        e.cue_end(2);
        assert_eq!(e.running().map(|r| r.id()), Some(id));
        e.cue_start(3, &LightCue::Blackout, true);
        assert!(e.blackout);
        e.cue_end(3);
        assert!(!e.blackout);
        assert_eq!(e.running().map(|r| r.id()), Some(id), "was playing before the blackout cue");
        // Deleting the automation clears cues that use it.
        e.show.sounds.sounds.push(crate::sound::Sound { cue: LightCue::Automation { id }, ..crate::sound::Sound::new("a.wav") });
        e.delete_automation(id);
        assert!(e.show.sounds.sounds[0].cue.is_none());
    }
}
