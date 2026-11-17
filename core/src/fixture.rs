//! Lights ("fixtures"), their channel layouts, and how a look (brightness + colour) becomes DMX
//! channel values.

use serde::{Deserialize, Serialize};

pub const UNIVERSE: usize = 512;

/// A colour, each part 0..=1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const WHITE: Rgb = Rgb { r: 1.0, g: 1.0, b: 1.0 };
    pub const BLACK: Rgb = Rgb { r: 0.0, g: 0.0, b: 0.0 };

    pub const fn new(r: f32, g: f32, b: f32) -> Rgb {
        Rgb { r, g, b }
    }

    pub fn clamped(self) -> Rgb {
        let c = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        Rgb { r: c(self.r), g: c(self.g), b: c(self.b) }
    }

    pub fn lerp(self, o: Rgb, t: f32) -> Rgb {
        Rgb { r: self.r + (o.r - self.r) * t, g: self.g + (o.g - self.g) * t, b: self.b + (o.b - self.b) * t }
    }

    pub fn scale(self, k: f32) -> Rgb {
        Rgb { r: self.r * k, g: self.g * k, b: self.b * k }
    }

    /// Hue 0..1, saturation and value 0..1.
    pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
        let h = (h.rem_euclid(1.0)) * 6.0;
        let i = h.floor() as i32;
        let f = h - i as f32;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Rgb { r, g, b }
    }

    pub fn hex(self) -> String {
        let c = self.clamped();
        format!("#{:02x}{:02x}{:02x}", byte(c.r), byte(c.g), byte(c.b))
    }
}

/// 0..1 to a DMX value.
pub fn byte(v: f32) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// What a light is told to do: how bright, and which colour.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub level: f32,
    pub color: Rgb,
}

impl Look {
    pub const OFF: Look = Look { level: 0.0, color: Rgb::WHITE };

    pub fn new(level: f32, color: Rgb) -> Look {
        Look { level, color }
    }

    pub fn clamped(self) -> Look {
        Look { level: if self.level.is_finite() { self.level.clamp(0.0, 1.0) } else { 0.0 }, color: self.color.clamped() }
    }

    pub fn lerp(self, o: Look, t: f32) -> Look {
        // A light fading in from off takes its new colour at once, so it doesn't pass through
        // the old one (and the other way round when it fades out).
        let color = if self.level <= 0.001 {
            o.color
        } else if o.level <= 0.001 {
            self.color
        } else {
            self.color.lerp(o.color, t)
        };
        Look { level: self.level + (o.level - self.level) * t, color }
    }
}

impl Default for Look {
    fn default() -> Look {
        Look::OFF
    }
}

/// One DMX channel of a light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chan {
    Dimmer,
    Red,
    Green,
    Blue,
    White,
    Amber,
    Uv,
    /// Always 0 (strobe/mode/speed channels of most LED PARs: 0 = plain colour).
    Zero,
    /// Always 255.
    Full,
}

impl Chan {
    pub fn letter(self) -> char {
        match self {
            Chan::Dimmer => 'd',
            Chan::Red => 'r',
            Chan::Green => 'g',
            Chan::Blue => 'b',
            Chan::White => 'w',
            Chan::Amber => 'a',
            Chan::Uv => 'u',
            Chan::Zero => '0',
            Chan::Full => 'f',
        }
    }

    pub fn from_letter(c: char) -> Option<Chan> {
        Some(match c.to_ascii_lowercase() {
            'd' | 'i' => Chan::Dimmer,
            'r' => Chan::Red,
            'g' => Chan::Green,
            'b' => Chan::Blue,
            'w' => Chan::White,
            'a' => Chan::Amber,
            'u' => Chan::Uv,
            '0' | 'x' | 's' | 'm' => Chan::Zero,
            'f' => Chan::Full,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Chan::Dimmer => "Dimmer",
            Chan::Red => "Red",
            Chan::Green => "Green",
            Chan::Blue => "Blue",
            Chan::White => "White",
            Chan::Amber => "Amber",
            Chan::Uv => "UV",
            Chan::Zero => "Off (0)",
            Chan::Full => "Full (255)",
        }
    }
}

/// Channel order typed as letters, e.g. "drgb" or "d r g b 0 0 0".
pub fn parse_layout(s: &str) -> Result<Vec<Chan>, String> {
    let mut out = Vec::new();
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != ',' && *c != '-') {
        out.push(Chan::from_letter(c).ok_or_else(|| format!("'{c}' isn't a channel letter"))?);
    }
    if out.is_empty() {
        return Err("type at least one channel".into());
    }
    if out.len() > 32 {
        return Err("a light can have at most 32 channels".into());
    }
    Ok(out)
}

pub fn layout_string(ch: &[Chan]) -> String {
    ch.iter().map(|c| c.letter()).collect()
}

/// A ready-made channel layout.
pub struct Profile {
    pub name: &'static str,
    pub layout: &'static str,
    pub hint: &'static str,
}

pub const PROFILES: &[Profile] = &[
    Profile { name: "RGB", layout: "rgb", hint: "LED light or strip with red, green, blue (3 channels)" },
    Profile { name: "Dimmer + RGB", layout: "drgb", hint: "Brightness, red, green, blue (4 channels)" },
    Profile { name: "RGBW", layout: "rgbw", hint: "Red, green, blue, white (4 channels)" },
    Profile { name: "Dimmer + RGBW", layout: "drgbw", hint: "Brightness, red, green, blue, white (5 channels)" },
    Profile { name: "RGBA", layout: "rgba", hint: "Red, green, blue, amber (4 channels)" },
    Profile { name: "LED PAR 7 ch", layout: "drgb000", hint: "Brightness, R, G, B, strobe, mode, speed (common cheap PAR)" },
    Profile { name: "LED PAR 8 ch", layout: "drgbw000", hint: "Brightness, R, G, B, W, strobe, mode, speed" },
    Profile { name: "Dimmer", layout: "d", hint: "Plain dimmer channel or single-colour light (1 channel)" },
];

pub fn profile_layout(name: &str) -> Option<Vec<Chan>> {
    PROFILES.iter().find(|p| p.name == name).and_then(|p| parse_layout(p.layout).ok())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub id: u32,
    pub name: String,
    /// First DMX channel, 1..=512.
    pub address: u16,
    pub chans: Vec<Chan>,
    /// The profile it was made from ("Custom" when typed by hand).
    pub profile: String,
}

impl Fixture {
    pub fn width(&self) -> usize {
        self.chans.len()
    }

    pub fn last_address(&self) -> usize {
        self.address as usize + self.width().max(1) - 1
    }

    pub fn has_color(&self) -> bool {
        self.chans.iter().any(|c| matches!(c, Chan::Red | Chan::Green | Chan::Blue))
    }

    pub fn has_dimmer(&self) -> bool {
        self.chans.contains(&Chan::Dimmer)
    }

    pub fn fits(&self) -> bool {
        self.address >= 1 && !self.chans.is_empty() && self.last_address() <= UNIVERSE
    }

    pub fn overlaps(&self, o: &Fixture) -> bool {
        (self.address as usize) <= o.last_address() && (o.address as usize) <= self.last_address()
    }

    /// Channel values for a look at the given output gain (master), in channel order.
    pub fn values(&self, look: Look, gain: f32) -> Vec<u8> {
        let look = look.clamped();
        let level = look.level * gain.clamp(0.0, 1.0);
        let has_dim = self.has_dimmer();
        let has_rgb = self.has_color();
        // Without a dimmer channel, brightness goes into the colour channels.
        // A dark light gets every colour channel at 0 too (some lights ignore their dimmer in
        // some modes, and blackout must really be dark).
        let k = if has_dim && level > 0.0 { 1.0 } else { level };
        let Rgb { mut r, mut g, mut b } = look.color;
        let white = if has_rgb {
            if self.chans.contains(&Chan::White) {
                // White channel takes the part all three colours share (purer, brighter whites).
                let w = r.min(g).min(b);
                r -= w;
                g -= w;
                b -= w;
                w
            } else {
                0.0
            }
        } else {
            1.0
        };
        let amber = if has_rgb { 0.0 } else { 1.0 };
        self.chans
            .iter()
            .map(|c| match c {
                Chan::Dimmer => byte(level),
                Chan::Red => byte(r * k),
                Chan::Green => byte(g * k),
                Chan::Blue => byte(b * k),
                Chan::White => byte(white * k),
                Chan::Amber => byte(amber * k),
                Chan::Uv => 0,
                Chan::Zero => 0,
                Chan::Full => 255,
            })
            .collect()
    }

    /// Writes the light into a universe (channel 1 is index 0); channels past 512 are dropped.
    pub fn render(&self, look: Look, gain: f32, frame: &mut [u8; UNIVERSE]) {
        let start = self.address.max(1) as usize - 1;
        for (i, v) in self.values(look, gain).into_iter().enumerate() {
            if let Some(slot) = frame.get_mut(start + i) {
                *slot = v;
            }
        }
    }

    /// What the light looks like (for the on-screen stage), 0..1 per colour.
    pub fn shown(&self, look: Look, gain: f32) -> Rgb {
        let look = look.clamped();
        let level = look.level * gain.clamp(0.0, 1.0);
        if self.has_color() {
            look.color.scale(level)
        } else {
            Rgb::new(1.0, 0.86, 0.66).scale(level) // a plain lamp: warm white
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(layout: &str, address: u16) -> Fixture {
        Fixture { id: 1, name: "t".into(), address, chans: parse_layout(layout).unwrap(), profile: "Custom".into() }
    }

    #[test]
    fn rgb_without_dimmer_scales_colour() {
        let f = fx("rgb", 1);
        assert_eq!(f.values(Look::new(0.5, Rgb::new(1.0, 0.0, 0.5)), 1.0), vec![128, 0, 64]);
    }

    #[test]
    fn dimmer_channel_takes_level() {
        let f = fx("drgb000", 1);
        assert_eq!(f.values(Look::new(0.5, Rgb::new(1.0, 0.0, 0.5)), 1.0), vec![128, 255, 0, 128, 0, 0, 0]);
        assert_eq!(f.values(Look::new(1.0, Rgb::WHITE), 0.5)[0], 128);
    }

    #[test]
    fn white_channel_takes_shared_part() {
        let f = fx("rgbw", 1);
        assert_eq!(f.values(Look::new(1.0, Rgb::WHITE), 1.0), vec![0, 0, 0, 255]);
        assert_eq!(f.values(Look::new(1.0, Rgb::new(1.0, 0.5, 0.5)), 1.0), vec![128, 0, 0, 128]);
    }

    #[test]
    fn plain_dimmer_and_white_only() {
        assert_eq!(fx("d", 1).values(Look::new(0.25, Rgb::new(1.0, 0.0, 0.0)), 1.0), vec![64]);
        assert_eq!(fx("w", 1).values(Look::new(1.0, Rgb::BLACK), 1.0), vec![255]);
    }

    #[test]
    fn render_places_by_address_and_clips() {
        let mut frame = [0u8; UNIVERSE];
        fx("rgb", 10).render(Look::new(1.0, Rgb::new(1.0, 0.0, 1.0)), 1.0, &mut frame);
        assert_eq!(&frame[9..12], &[255, 0, 255]);
        let mut frame = [0u8; UNIVERSE];
        let f = fx("rgb", 511);
        assert!(!f.fits());
        f.render(Look::new(1.0, Rgb::WHITE), 1.0, &mut frame);
        assert_eq!(&frame[510..], &[255, 255]);
    }

    #[test]
    fn layouts_and_overlap() {
        assert_eq!(layout_string(&parse_layout("D R G B, s m").unwrap()), "drgb00");
        assert!(parse_layout("rgq").is_err());
        assert!(parse_layout("  ").is_err());
        assert!(fx("rgb", 1).overlaps(&fx("rgb", 3)));
        assert!(!fx("rgb", 1).overlaps(&fx("rgb", 4)));
        for p in PROFILES {
            assert!(profile_layout(p.name).is_some(), "{}", p.name);
        }
    }

    #[test]
    fn junk_values_are_safe() {
        let f = fx("rgb", 1);
        assert_eq!(f.values(Look::new(f32::NAN, Rgb::new(f32::INFINITY, -3.0, 2.0)), 1.0), vec![0, 0, 0]);
        assert_eq!(Rgb::hsv(0.0, 1.0, 1.0), Rgb::new(1.0, 0.0, 0.0));
        assert_eq!(Rgb::new(1.0, 0.5, 0.0).hex(), "#ff8000");
    }
}
