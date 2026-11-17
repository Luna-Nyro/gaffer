//! Built-in moving effects. Each is a pure function of time, so it can be tested and scrubbed.

use std::f32::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::fixture::{Look, Rgb};

/// Flashes per second never go above this (comfort and photosensitivity).
pub const STROBE_MAX_HZ: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Breathe,
    Rainbow,
    Chase,
    ColorCycle,
    Candle,
    Strobe,
}

pub const EFFECTS: &[EffectKind] = &[EffectKind::Breathe, EffectKind::Rainbow, EffectKind::Chase, EffectKind::ColorCycle, EffectKind::Candle, EffectKind::Strobe];

impl EffectKind {
    pub fn name(self) -> &'static str {
        match self {
            EffectKind::Breathe => "Breathe",
            EffectKind::Rainbow => "Rainbow",
            EffectKind::Chase => "Chase",
            EffectKind::ColorCycle => "Colour cycle",
            EffectKind::Candle => "Candle",
            EffectKind::Strobe => "Strobe",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            EffectKind::Breathe => "Slowly brightens and dims, like breathing",
            EffectKind::Rainbow => "Rolls through every colour, spread across the lights",
            EffectKind::Chase => "One light after another, in your colours",
            EffectKind::ColorCycle => "All lights fade from one of your colours to the next",
            EffectKind::Candle => "Warm, gently flickering firelight",
            EffectKind::Strobe => "Fast flashes (can trigger photosensitive seizures)",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            EffectKind::Breathe => "weather-clear-night-symbolic",
            EffectKind::Rainbow => "applications-graphics-symbolic",
            EffectKind::Chase => "media-seek-forward-symbolic",
            EffectKind::ColorCycle => "view-refresh-symbolic",
            EffectKind::Candle => "weather-clear-symbolic",
            EffectKind::Strobe => "dialog-warning-symbolic",
        }
    }

    /// Colours a new effect of this kind starts with.
    pub fn default_colors(self) -> Vec<Rgb> {
        match self {
            EffectKind::Breathe => vec![],
            EffectKind::Rainbow => vec![],
            EffectKind::Chase => vec![Rgb::new(1.0, 0.0, 0.4), Rgb::new(0.0, 0.4, 1.0)],
            EffectKind::ColorCycle => vec![Rgb::new(1.0, 0.0, 0.0), Rgb::new(0.0, 1.0, 0.0), Rgb::new(0.0, 0.0, 1.0)],
            EffectKind::Candle => vec![Rgb::new(1.0, 0.45, 0.08)],
            EffectKind::Strobe => vec![Rgb::WHITE],
        }
    }

    /// Does the effect use the colour list at all?
    pub fn uses_colors(self) -> bool {
        !matches!(self, EffectKind::Rainbow)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectParams {
    pub kind: EffectKind,
    /// Lights it runs on; empty = all.
    pub fixtures: Vec<u32>,
    /// Empty = keep each light's own colour (Breathe) or the effect's natural colour.
    pub colors: Vec<Rgb>,
    /// 0 = slow, 1 = fast.
    pub speed: f32,
    /// Brightness 0..1.
    pub level: f32,
}

impl Default for EffectParams {
    fn default() -> EffectParams {
        EffectParams::new(EffectKind::Breathe)
    }
}

impl EffectParams {
    pub fn new(kind: EffectKind) -> EffectParams {
        EffectParams { kind, fixtures: vec![], colors: kind.default_colors(), speed: 0.4, level: 1.0 }
    }

    pub fn repair(&mut self) {
        let c = |v: f32, d: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { d };
        self.speed = c(self.speed, 0.4);
        self.level = c(self.level, 1.0);
        self.colors.truncate(8);
        for col in &mut self.colors {
            *col = col.clamped();
        }
    }

    fn lerp(&self, slow: f32, fast: f32) -> f32 {
        // Geometric, so the slider feels even from slow to fast.
        slow * (fast / slow).powf(self.speed.clamp(0.0, 1.0))
    }

    /// The look of light number `i` of `n` at `t` seconds into the effect. `own` is the light's
    /// live look (used when the effect keeps the light's colour).
    pub fn look(&self, t: f32, i: usize, n: usize, own: Look) -> Look {
        let n = n.max(1);
        let lv = self.level;
        let first = self.colors.first().copied();
        let pick = |k: usize| -> Rgb {
            if self.colors.is_empty() { own.color } else { self.colors[k % self.colors.len()] }
        };
        match self.kind {
            EffectKind::Breathe => {
                let p = self.lerp(12.0, 1.2);
                let wave = 0.5 - 0.5 * (TAU * t / p).cos();
                let base = if self.colors.is_empty() { own.level.max(0.05) } else { 1.0 };
                Look::new(lv * base * (0.08 + 0.92 * wave * wave), first.unwrap_or(own.color))
            }
            EffectKind::Rainbow => {
                let p = self.lerp(40.0, 2.0);
                Look::new(lv, Rgb::hsv(t / p + i as f32 / n as f32, 1.0, 1.0))
            }
            EffectKind::Chase => {
                let step = self.lerp(2.0, 0.12);
                let pos = t / step;
                let lap = (pos / n as f32).floor() as usize;
                let at = pos.rem_euclid(n as f32);
                // Distance behind the moving head, with a short soft tail.
                let behind = (at - i as f32).rem_euclid(n as f32);
                let k = if behind < 1.0 { 1.0 } else { (1.0 - (behind - 1.0) / 0.6).max(0.0) };
                let lap_here = if (i as f32) <= at { lap } else { lap.wrapping_sub(1) };
                Look::new(lv * k, pick(lap_here))
            }
            EffectKind::ColorCycle => {
                let hold = self.lerp(12.0, 0.5);
                let pos = t / hold;
                let k = pos.floor() as usize;
                let f = pos - pos.floor();
                // Hold 60% of the time, then fade to the next colour.
                let mix = ((f - 0.6) / 0.4).clamp(0.0, 1.0);
                let mix = mix * mix * (3.0 - 2.0 * mix);
                Look::new(lv, pick(k).lerp(pick(k + 1), mix))
            }
            EffectKind::Candle => {
                let rate = self.lerp(2.5, 14.0);
                let seed = i as f32 * 37.13;
                let flick = 0.6 * noise(t * rate + seed) + 0.4 * noise(t * rate * 2.7 + seed * 1.9);
                let k = 0.45 + 0.55 * flick;
                let warm = first.unwrap_or(Rgb::new(1.0, 0.45, 0.08));
                // Dimmer flames are redder.
                Look::new(lv * k, warm.lerp(Rgb::new(1.0, 0.25, 0.02), (1.0 - k) * 0.6))
            }
            EffectKind::Strobe => {
                let hz = self.lerp(1.0, STROBE_MAX_HZ).min(STROBE_MAX_HZ);
                let ph = (t * hz).rem_euclid(1.0);
                Look::new(if ph < 0.35 { lv } else { 0.0 }, first.unwrap_or(Rgb::WHITE))
            }
        }
    }
}

/// Smooth value noise in 0..1.
pub fn noise(x: f32) -> f32 {
    let i = x.floor();
    let f = x - i;
    let a = hash(i as i64);
    let b = hash(i as i64 + 1);
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}

fn hash(n: i64) -> f32 {
    let mut x = n as u64 ^ 0x9e37_79b9_7f4a_7c15;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Look = Look { level: 0.8, color: Rgb { r: 0.0, g: 1.0, b: 0.0 } };

    #[test]
    fn everything_stays_in_range() {
        for &k in EFFECTS {
            let mut p = EffectParams::new(k);
            for speed in [0.0, 0.5, 1.0] {
                p.speed = speed;
                for step in 0..400 {
                    let t = step as f32 * 0.037;
                    for i in 0..5 {
                        let l = p.look(t, i, 5, OWN);
                        assert!((0.0..=1.0).contains(&l.level), "{k:?} {l:?}");
                        assert_eq!(l.color, l.color.clamped(), "{k:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn breathe_keeps_own_colour_and_breathes() {
        let p = EffectParams::new(EffectKind::Breathe);
        let a = p.look(0.0, 0, 1, OWN);
        assert_eq!(a.color, OWN.color);
        let period = p.lerp(12.0, 1.2);
        let b = p.look(period / 2.0, 0, 1, OWN);
        assert!(a.level < 0.1 && b.level > 0.75, "{a:?} {b:?}");
    }

    #[test]
    fn chase_lights_one_at_a_time() {
        let mut p = EffectParams::new(EffectKind::Chase);
        p.speed = 0.0; // 2 s a light
        let lit = |t: f32| (0..4).filter(|&i| p.look(t, i, 4, OWN).level > 0.99).collect::<Vec<_>>();
        assert_eq!(lit(0.5), vec![0]);
        assert_eq!(lit(2.5), vec![1]);
        assert_eq!(lit(6.5), vec![3]);
        assert_eq!(lit(8.5), vec![0]);
        // Second lap takes the second colour.
        assert_eq!(p.look(8.5, 0, 4, OWN).color, p.colors[1]);
    }

    #[test]
    fn strobe_is_capped() {
        let mut p = EffectParams::new(EffectKind::Strobe);
        p.speed = 1.0;
        let mut flashes = 0;
        let mut was = false;
        for s in 0..10_000 {
            let on = p.look(s as f32 / 10_000.0 * 10.0, 0, 1, OWN).level > 0.0;
            if on && !was {
                flashes += 1;
            }
            was = on;
        }
        assert!(flashes <= (STROBE_MAX_HZ * 10.0) as i32 + 1, "{flashes}");
    }

    #[test]
    fn rainbow_spreads() {
        let p = EffectParams::new(EffectKind::Rainbow);
        assert_ne!(p.look(1.0, 0, 3, OWN).color, p.look(1.0, 1, 3, OWN).color);
    }
}
