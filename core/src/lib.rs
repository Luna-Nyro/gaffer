//! Gaffer's lightboard logic: lights, looks, scenes, automations, effects, soundboard settings and the engine that
//! turns them into a DMX universe. Pure Rust, no I/O, so it is fully unit-tested.

pub mod effect;
pub mod engine;
pub mod fixture;
pub mod show;
pub mod sound;

pub use effect::{EffectKind, EffectParams, EFFECTS};
pub use engine::{Clock, Engine, Running};
pub use fixture::{Chan, Fixture, Look, Rgb, PROFILES, UNIVERSE};
pub use show::{Action, Automation, Scene, Show, Step, Target, Trigger};
pub use sound::{LevelFollower, LightCue, Sound, Soundboard};
