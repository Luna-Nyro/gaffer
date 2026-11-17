//! Plays the soundboard's sounds with GStreamer: one playbin per playing sound (the sound server
//! mixes them), each with a `level` meter whose readings are handed out in time with what is
//! heard, so lights can follow the sound.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;

use gaffer_core::LevelFollower;
use gst::prelude::*;
use gtk::glib;

/// Meter readings per second: 40, like the DMX output.
const METER_NS: u64 = 25_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A playing sound's loudness, 0..1 (already shaped for lights).
    Level(u64, f32),
    /// It finished (or was stopped from elsewhere).
    Ended(u64),
    Failed(u64, String),
}

struct Voice {
    pipeline: gst::Element,
    _watch: gst::bus::BusWatchGuard,
    /// (running time the reading ends at, loudest channel's RMS dB)
    levels: Rc<RefCell<VecDeque<(u64, f64)>>>,
    follower: LevelFollower,
    volume: f32,
}

pub struct Player {
    voices: RefCell<HashMap<u64, Voice>>,
    events: Rc<RefCell<Vec<Event>>>,
    next: Cell<u64>,
    master: Cell<f32>,
    /// Test hook: an element name used instead of the speakers (e.g. "fakesink").
    sink: Option<String>,
    /// Why sound can't play here, if it can't.
    pub problem: Option<String>,
}

/// Slider position → GStreamer volume (cubic, so the slider sounds even).
fn gain(v: f32) -> f64 {
    (v.clamp(0.0, 1.0) as f64).powi(3)
}

impl Player {
    pub fn new(sink: Option<&str>) -> Player {
        let mut problem = gst::init().err().map(|e| format!("GStreamer didn't start: {e}"));
        if problem.is_none() {
            for el in ["playbin", "level"] {
                if gst::ElementFactory::find(el).is_none() {
                    problem = Some(format!("GStreamer's “{el}” is missing (install gstreamer1.0-plugins-base and -good)"));
                }
            }
        }
        Player { voices: RefCell::default(), events: Rc::default(), next: Cell::new(1), master: Cell::new(1.0), sink: sink.map(str::to_string), problem }
    }

    /// Starts a sound; returns its id (ids are never reused).
    pub fn play(&self, path: &Path, volume: f32, repeat: bool) -> Result<u64, String> {
        if let Some(p) = &self.problem {
            return Err(p.clone());
        }
        let uri = glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;
        let id = self.next.get();
        self.next.set(id + 1);
        let err = |e: glib::BoolError| e.to_string();
        let pipeline = gst::ElementFactory::make("playbin").property("uri", uri.as_str()).build().map_err(err)?;
        // Audio only: album art in a file must not open a video window.
        pipeline.set_property_from_str("flags", "audio+soft-volume");
        let meter = gst::ElementFactory::make("level").property("interval", METER_NS).property("post-messages", true).build().map_err(err)?;
        pipeline.set_property("audio-filter", &meter);
        if let Some(s) = &self.sink {
            let sink = gst::ElementFactory::make(s).build().map_err(err)?;
            if sink.has_property("sync") {
                sink.set_property("sync", true);
            }
            pipeline.set_property("audio-sink", &sink);
        }
        pipeline.set_property("volume", gain(volume * self.master.get()));

        let levels: Rc<RefCell<VecDeque<(u64, f64)>>> = Rc::default();
        let (lv, ev, weak) = (levels.clone(), self.events.clone(), pipeline.downgrade());
        let bus = pipeline.bus().ok_or("no bus")?;
        let watch = bus
            .add_watch_local(move |_, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Element(e) => {
                        if let Some(s) = e.structure().filter(|s| s.name() == "level") {
                            let end = s.get::<u64>("running-time").unwrap_or(0) + s.get::<u64>("duration").unwrap_or(0);
                            let db = s
                                .get::<glib::ValueArray>("rms")
                                .ok()
                                .and_then(|a| a.iter().filter_map(|v| v.get::<f64>().ok()).reduce(f64::max))
                                .unwrap_or(f64::NEG_INFINITY);
                            let mut q = lv.borrow_mut();
                            if q.len() < 400 {
                                q.push_back((end, db));
                            }
                        }
                    }
                    MessageView::Eos(_) => {
                        let looped = repeat && weak.upgrade().is_some_and(|p| p.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO).is_ok());
                        if looped {
                            lv.borrow_mut().clear(); // running time starts again after the seek
                        } else {
                            ev.borrow_mut().push(Event::Ended(id));
                        }
                    }
                    MessageView::Error(e) => {
                        ev.borrow_mut().push(Event::Failed(id, e.error().to_string()));
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .map_err(|e| e.to_string())?;
        if let Err(e) = pipeline.set_state(gst::State::Playing) {
            let _ = pipeline.set_state(gst::State::Null);
            return Err(format!("couldn't start playing ({e})"));
        }
        self.voices.borrow_mut().insert(id, Voice { pipeline, _watch: watch, levels, follower: LevelFollower::default(), volume });
        Ok(id)
    }

    /// Stops a sound; false if it wasn't playing.
    pub fn stop(&self, id: u64) -> bool {
        let v = self.voices.borrow_mut().remove(&id);
        match v {
            Some(v) => {
                let _ = v.pipeline.set_state(gst::State::Null);
                true
            }
            None => false,
        }
    }

    /// Stops every sound; returns their ids.
    pub fn stop_all(&self) -> Vec<u64> {
        let ids: Vec<u64> = self.voices.borrow().keys().copied().collect();
        for &id in &ids {
            self.stop(id);
        }
        ids
    }

    #[cfg(test)]
    pub fn is_playing(&self, id: u64) -> bool {
        self.voices.borrow().contains_key(&id)
    }

    pub fn any_playing(&self) -> bool {
        !self.voices.borrow().is_empty()
    }

    /// The whole board's volume (0..1); applies to what's playing at once.
    pub fn set_master(&self, v: f32) {
        self.master.set(v);
        for voice in self.voices.borrow().values() {
            voice.pipeline.set_property("volume", gain(voice.volume * v));
        }
    }

    pub fn set_volume(&self, id: u64, v: f32) {
        if let Some(voice) = self.voices.borrow_mut().get_mut(&id) {
            voice.volume = v;
            voice.pipeline.set_property("volume", gain(v * self.master.get()));
        }
    }

    /// How far through a sound is (0..1), when its length is known.
    pub fn progress(&self, id: u64) -> Option<f64> {
        let voices = self.voices.borrow();
        let p = &voices.get(&id)?.pipeline;
        let pos = p.query_position::<gst::ClockTime>()?;
        let dur = p.query_duration::<gst::ClockTime>()?;
        (dur.nseconds() > 0).then(|| (pos.nseconds() as f64 / dur.nseconds() as f64).clamp(0.0, 1.0))
    }

    /// What happened since the last call: loudness of each playing sound (only readings whose
    /// audio is being heard now), and sounds that ended or failed (they are cleaned up).
    pub fn poll(&self) -> Vec<Event> {
        let mut out: Vec<Event> = std::mem::take(&mut *self.events.borrow_mut());
        for e in &out {
            if let Event::Ended(id) | Event::Failed(id, _) = e {
                self.stop(*id);
            }
        }
        for (&id, v) in self.voices.borrow_mut().iter_mut() {
            let Some(now) = v.pipeline.current_running_time() else { continue };
            let now = now.nseconds();
            let mut q = v.levels.borrow_mut();
            let mut loudest: Option<f32> = None;
            while let Some(&(end, db)) = q.front() {
                if end > now {
                    break;
                }
                q.pop_front();
                let x = v.follower.feed(db, METER_NS as f32 / 1e9);
                loudest = Some(loudest.map_or(x, |l| l.max(x)));
            }
            if let Some(x) = loudest {
                out.push(Event::Level(id, x));
            }
        }
        out
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A mono 16-bit WAV: `beats` loud bursts with silence between.
    fn wav(path: &Path, secs: f32, beats: usize) {
        let rate = 22050u32;
        let n = (secs * rate as f32) as usize;
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let beat = (t * beats as f32 / secs).fract() < 0.4;
            let s = if beat { (t * 440.0 * std::f32::consts::TAU).sin() * 0.8 } else { 0.0 };
            data.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * 2).to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&data);
        std::fs::write(path, f).unwrap();
    }

    fn pump(p: &Player, ctx: &glib::MainContext, until: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let start = Instant::now();
        let mut all = Vec::new();
        while start.elapsed() < Duration::from_secs(10) {
            while ctx.iteration(false) {}
            all.extend(p.poll());
            if until(&all) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        all
    }

    #[test]
    fn plays_meters_and_ends() {
        let ctx = glib::MainContext::default();
        let _own = ctx.acquire().unwrap();
        let dir = std::env::temp_dir().join(format!("gaffer-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("beat.wav");
        wav(&file, 1.2, 4);
        let p = Player::new(Some("fakesink"));
        assert_eq!(p.problem, None);
        let started = Instant::now();
        let id = p.play(&file, 1.0, false).unwrap();
        assert!(p.is_playing(id));
        let ev = pump(&p, &ctx, |e| e.iter().any(|e| matches!(e, Event::Ended(_))));
        assert!(ev.contains(&Event::Ended(id)), "{ev:?}");
        // Played in real time (the levels came out in time with it), not all at once.
        assert!(started.elapsed() > Duration::from_millis(1000));
        let lv: Vec<f32> = ev.iter().filter_map(|e| if let Event::Level(_, v) = e { Some(*v) } else { None }).collect();
        assert!(lv.len() > 20, "{}", lv.len());
        assert!(lv.iter().any(|&v| v > 0.9) && lv.iter().any(|&v| v < 0.1), "{lv:?}");
        assert!(!p.is_playing(id));

        // Repeat keeps going until stopped.
        let id = p.play(&file, 0.5, true).unwrap();
        let ev = pump(&p, &ctx, |_| false || p.progress(id).is_some_and(|x| x > 0.95));
        assert!(!ev.iter().any(|e| matches!(e, Event::Ended(_))));
        std::thread::sleep(Duration::from_millis(300));
        while ctx.iteration(false) {}
        assert!(p.poll().iter().all(|e| matches!(e, Event::Level(..))));
        assert!(p.is_playing(id));
        assert!(p.stop(id));
        assert!(!p.stop(id));

        // A file that isn't audio fails cleanly.
        let bad = dir.join("bad.mp3");
        std::fs::write(&bad, b"not audio at all").unwrap();
        let id = p.play(&bad, 1.0, false).unwrap();
        let ev = pump(&p, &ctx, |e| e.iter().any(|e| matches!(e, Event::Failed(..))));
        assert!(ev.iter().any(|e| matches!(e, Event::Failed(i, _) if *i == id)), "{ev:?}");
        assert!(!p.any_playing());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
