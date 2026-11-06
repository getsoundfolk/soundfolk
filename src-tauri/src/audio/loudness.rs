// SPDX-License-Identifier: MPL-2.0
//! Constant volume (see `docs/10-loudness.md`): one slowly moving gain per station, so every
//! station plays near the same loudness.
//!
//! Runs on the decoder thread, after conversion to the output format and before the ring buffer:
//! the samples are measured (ITU-R BS.1770 loudness, through `ebur128`), then the station gain is
//! applied to them with a ramp. The slider volume stays in the real-time callback.
//!
//! * A station heard before starts at its remembered loudness ([`LevelMemory`]), from the first
//!   sample. An unknown one starts at a guess (the median of the stations learned), then *learns*
//!   for 10 s: the first value comes after 1.5 s of sound, and the estimate moves fast both ways,
//!   with the boost capped at +3 dB.
//! * After that, the estimate follows the short-term loudness (3 s), fast when the station gets
//!   louder (the gain is cut in ~4 s), slowly when it gets quieter (~40 s), so a quiet song or a
//!   talk break does not pump the gain up. Silence and passages far below the station's level are
//!   ignored (gates).
//! * A boost never takes the recent peak above −1 dBFS: no limiter, no clipping.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ebur128::{EbuR128, Mode};

/// Loudness every station is brought to.
pub const TARGET_LUFS: f64 = -16.0;
/// Starting guess for an unknown station, until enough stations were learned for their median.
pub const DEFAULT_GUESS_LUFS: f64 = -11.0;

const MAX_CUT_DB: f64 = -15.0;
const MAX_BOOST_DB: f64 = 6.0;
const MAX_BOOST_LEARNING_DB: f64 = 3.0;
/// A boost never takes the recent peak above this.
const PEAK_CEILING_DBFS: f64 = -1.0;

/// Below this, it is silence (R128's absolute gate).
const ABSOLUTE_GATE_LUFS: f64 = -50.0;
/// This far below the station's level, it is a quiet passage, not the station's level (R128's
/// relative gate).
const RELATIVE_GATE_LU: f64 = 10.0;
/// After this long with every block below the relative gate, the station itself got quieter.
const RELATIVE_GATE_GIVE_UP: Duration = Duration::from_secs(30);

/// Measurement step.
const BLOCK: Duration = Duration::from_millis(100);
/// An unknown station learns its level during this much sound.
const LEARNING: Duration = Duration::from_secs(10);
/// Sound averaged for an unknown station's first value.
const FIRST_VALUE: Duration = Duration::from_millis(1500);
/// Length of the momentary and short-term windows: values read before are incomplete.
const MOMENTARY_WINDOW: Duration = Duration::from_millis(400);
const SHORT_TERM_WINDOW: Duration = Duration::from_secs(3);
/// Peak taken into account for the headroom.
const PEAK_WINDOW_BLOCKS: usize = 100;

/// Time constants of the estimate. Louder / quieter tuned on 207 stations (`docs/10-loudness.md`
/// §6): 2 s / 20 s moved the gain twice as much once settled (4 dB a minute, a pumping range of
/// 4 dB) for no gain in how close stations end to the target.
const TAU_LEARNING: f64 = 1.0;
const TAU_LOUDER: f64 = 4.0;
const TAU_QUIETER: f64 = 40.0;

/// Gain moves, in dB per second.
const RATE_CUT: f64 = 6.0;
const RATE_BOOST: f64 = 0.5;
const RATE_LEARNING: f64 = 6.0;
/// Switching normalization off or on.
const RATE_SWITCH: f64 = 15.0;

/// During the first minute of a known station, the more was heard before, the slower the live
/// measure lowers its estimate (up to this factor, reached at [`HISTORY_FULL`]).
const HISTORY_SLOWDOWN: f64 = 3.0;
const HISTORY_PERIOD: Duration = Duration::from_secs(60);
const HISTORY_FULL: f64 = 600.0;
/// The live level this far from the remembered one for [`DIVERGENCE_TIME`]: the station changed,
/// the history is dropped.
const DIVERGENCE_DB: f64 = 6.0;
const DIVERGENCE_TIME: Duration = Duration::from_secs(30);

/// Sound needed before a station's level is worth remembering.
const MIN_TO_REMEMBER: Duration = Duration::from_secs(10);
/// Cap of the remembered measuring time.
const MAX_MEASURED_SECS: f64 = 3600.0;
/// How often the level of the playing station is saved.
const SAVE_EVERY: Duration = Duration::from_secs(60);

/// A station's remembered level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredLevel {
    pub loudness_lufs: f64,
    pub peak_dbfs: f64,
    /// Seconds of sound behind the estimate.
    pub measured_secs: f64,
}

/// Where station levels are kept (the database, in the app).
pub trait LevelMemory: Send + Sync {
    fn recall(&self, station: &str) -> Option<StoredLevel>;
    /// Starting point for an unknown station (the median of the stations learned), if any.
    fn guess(&self) -> Option<f64>;
    fn remember(&self, station: &str, level: &StoredLevel);
}

/// What the estimate is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Unknown station, before its first value: momentary energies are summed.
    First { energy: f64, blocks: u32 },
    /// Unknown station, learning: fast both ways.
    Learning,
    /// Level known: asymmetric, gated.
    Steady,
}

/// Measures the samples and applies the station gain (see the module).
pub struct Leveler {
    meter: Option<EbuR128>,
    channels: usize,
    frames_per_block: usize,
    /// Frames fed into the current block.
    block_fill: usize,
    phase: Phase,
    /// Station loudness, LUFS.
    estimate: f64,
    /// Remembered level this session started from (known station).
    stored: Option<StoredLevel>,
    /// Sound measured this session (gated blocks only).
    gated: Duration,
    /// Sound fed this session (silence too), for the meter windows.
    fed: Duration,
    /// Consecutive time spent under the relative gate.
    below_gate: Duration,
    /// Consecutive time spent far from the remembered level.
    diverging: Duration,
    /// History weight, 0 (none) to 1: slows the live measure during the first minute.
    history: f64,
    /// Peak of each recent block, linear.
    peaks: Vec<f32>,
    peak_index: usize,
    block_peak: f32,
    /// Gain applied now, and where it goes, dB.
    gain_db: f64,
    target_db: f64,
    enabled: bool,
    /// The setting was just switched: the gain moves at [`RATE_SWITCH`] until it gets there.
    switching: bool,
}

impl Leveler {
    /// `stored`: the station's remembered level; `guess`: the starting point otherwise.
    pub fn new(
        sample_rate: u32,
        channels: usize,
        enabled: bool,
        stored: Option<StoredLevel>,
        guess: f64,
    ) -> Self {
        let meter = u32::try_from(channels).ok().and_then(|c| {
            EbuR128::new(c, sample_rate, Mode::S)
                .inspect_err(|e| tracing::warn!("no loudness meter: {e:?}"))
                .ok()
        });
        let (phase, estimate, history) = match stored {
            Some(level) => (
                Phase::Steady,
                level.loudness_lufs,
                (level.measured_secs / HISTORY_FULL).clamp(0.0, 1.0),
            ),
            None => (
                Phase::First {
                    energy: 0.0,
                    blocks: 0,
                },
                guess,
                0.0,
            ),
        };
        let mut leveler = Self {
            meter,
            channels: channels.max(1),
            frames_per_block: (sample_rate as usize / 10).max(1),
            block_fill: 0,
            phase,
            estimate,
            stored,
            gated: Duration::ZERO,
            fed: Duration::ZERO,
            below_gate: Duration::ZERO,
            diverging: Duration::ZERO,
            history,
            peaks: vec![0.0; PEAK_WINDOW_BLOCKS],
            peak_index: 0,
            block_peak: 0.0,
            gain_db: 0.0,
            target_db: 0.0,
            enabled,
            switching: false,
        };
        if let Some(level) = stored {
            // The remembered peak stands in until the window fills.
            leveler.peaks.fill(db_to_linear(level.peak_dbfs) as f32);
        }
        leveler.target_db = leveler.target();
        // The first sample already plays at the right level: no ramp from 0 dB.
        leveler.gain_db = leveler.target_db;
        leveler
    }

    /// Gain applied now, dB (0 when normalization is off).
    pub fn gain_db(&self) -> f64 {
        self.gain_db
    }

    fn learning(&self) -> bool {
        !matches!(self.phase, Phase::Steady)
    }

    /// Measures `samples` (interleaved, output format) and applies the station gain to them.
    /// `enabled`: the *Constant volume* setting; off, the gain goes back to 0 dB.
    pub fn process(&mut self, samples: &mut [f32], enabled: bool) {
        if enabled != self.enabled {
            self.enabled = enabled;
            self.switching = true;
            self.target_db = self.target();
        }
        let mut rest = samples;
        while !rest.is_empty() {
            let frames = (rest.len() / self.channels)
                .min(self.frames_per_block - self.block_fill)
                .max(1);
            let len = (frames * self.channels).min(rest.len());
            let (chunk, tail) = rest.split_at_mut(len);
            self.measure(chunk);
            self.apply(chunk, frames);
            self.block_fill += frames;
            if self.block_fill >= self.frames_per_block {
                self.block_fill = 0;
                self.end_block();
            }
            rest = tail;
        }
    }

    fn measure(&mut self, chunk: &[f32]) {
        let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        self.block_peak = self.block_peak.max(peak);
        if let Some(meter) = &mut self.meter {
            if let Err(e) = meter.add_frames_f32(chunk) {
                tracing::warn!("loudness meter failed: {e:?}");
                self.meter = None;
            }
        }
    }

    /// Ramps the gain towards its target over `frames`, linearly in amplitude.
    fn apply(&mut self, chunk: &mut [f32], frames: usize) {
        let seconds = frames as f64 * BLOCK.as_secs_f64() / self.frames_per_block as f64;
        let start = self.gain_db;
        if self.gain_db == self.target_db {
            self.switching = false;
        }
        let rate = if self.switching {
            RATE_SWITCH
        } else if self.target_db < self.gain_db {
            RATE_CUT
        } else if self.learning() {
            RATE_LEARNING
        } else {
            RATE_BOOST
        };
        let step = rate * seconds;
        self.gain_db = if self.target_db > start {
            (start + step).min(self.target_db)
        } else {
            (start - step).max(self.target_db)
        };
        let (from, to) = (
            db_to_linear(start) as f32,
            db_to_linear(self.gain_db) as f32,
        );
        if from == 1.0 && to == 1.0 {
            return;
        }
        let per_frame = (to - from) / frames as f32;
        for (i, frame) in chunk.chunks_mut(self.channels).enumerate() {
            let g = from + per_frame * (i + 1) as f32;
            for s in frame {
                *s *= g;
            }
        }
    }

    fn end_block(&mut self) {
        self.peaks[self.peak_index] = std::mem::take(&mut self.block_peak);
        self.peak_index = (self.peak_index + 1) % self.peaks.len();
        self.fed += BLOCK;
        if let Some(meter) = &self.meter {
            match self.phase {
                Phase::First { .. } | Phase::Learning if self.fed >= MOMENTARY_WINDOW => {
                    if let Ok(value) = meter.loudness_momentary() {
                        self.learn(value);
                    }
                }
                Phase::Steady if self.fed >= SHORT_TERM_WINDOW => {
                    if let Ok(value) = meter.loudness_shortterm() {
                        self.follow(value);
                    }
                }
                _ => {}
            }
        }
        self.target_db = self.target();
    }

    /// Unknown station: first value, then fast both ways until it is learned.
    fn learn(&mut self, value: f64) {
        if !value.is_finite() || value < ABSOLUTE_GATE_LUFS {
            return;
        }
        self.gated += BLOCK;
        match &mut self.phase {
            Phase::First { energy, blocks } => {
                *energy += 10f64.powf(value / 10.0);
                *blocks += 1;
                if self.gated >= FIRST_VALUE {
                    self.estimate = 10.0 * (*energy / f64::from(*blocks)).log10();
                    self.phase = Phase::Learning;
                }
            }
            Phase::Learning => {
                self.estimate += (value - self.estimate) * smoothing(TAU_LEARNING);
                if self.gated >= LEARNING {
                    self.phase = Phase::Steady;
                }
            }
            Phase::Steady => {}
        }
    }

    /// Known level: follows the short-term loudness, fast up, slowly down, gated.
    fn follow(&mut self, value: f64) {
        if !value.is_finite() || value < ABSOLUTE_GATE_LUFS {
            return;
        }
        if value < self.estimate - RELATIVE_GATE_LU {
            self.below_gate += BLOCK;
            if self.below_gate < RELATIVE_GATE_GIVE_UP {
                return;
            }
        } else {
            self.below_gate = Duration::ZERO;
        }
        self.gated += BLOCK;

        if let Some(stored) = self.stored.filter(|_| self.history > 0.0) {
            if (value - stored.loudness_lufs).abs() > DIVERGENCE_DB {
                self.diverging += BLOCK;
                if self.diverging >= DIVERGENCE_TIME {
                    tracing::info!(
                        stored = stored.loudness_lufs,
                        live = value,
                        "station level changed: history dropped"
                    );
                    self.history = 0.0;
                    self.stored = Some(StoredLevel {
                        measured_secs: 0.0,
                        ..stored
                    });
                }
            } else {
                self.diverging = Duration::ZERO;
            }
        }

        let tau = if value > self.estimate {
            // Getting louder: never slowed down (never surprise loud).
            TAU_LOUDER
        } else if self.fed < HISTORY_PERIOD {
            TAU_QUIETER * (1.0 + (HISTORY_SLOWDOWN - 1.0) * self.history)
        } else {
            TAU_QUIETER
        };
        self.estimate += (value - self.estimate) * smoothing(tau);
    }

    /// Recent peak, dBFS.
    fn peak_dbfs(&self) -> f64 {
        let peak = self.peaks.iter().fold(self.block_peak, |m, p| m.max(*p));
        linear_to_db(f64::from(peak))
    }

    fn target(&self) -> f64 {
        if !self.enabled {
            return 0.0;
        }
        let max_boost = if self.learning() {
            MAX_BOOST_LEARNING_DB
        } else {
            MAX_BOOST_DB
        };
        let mut gain = (TARGET_LUFS - self.estimate).min(max_boost);
        if gain > 0.0 {
            gain = gain.min((PEAK_CEILING_DBFS - self.peak_dbfs()).max(0.0));
        }
        gain.max(MAX_CUT_DB)
    }

    /// The level to remember, once enough was heard this session.
    pub fn level(&self) -> Option<StoredLevel> {
        if self.gated < MIN_TO_REMEMBER || self.meter.is_none() {
            return None;
        }
        let before = self.stored.map_or(0.0, |s| s.measured_secs);
        Some(StoredLevel {
            loudness_lufs: self.estimate,
            peak_dbfs: self.peak_dbfs().max(-90.0),
            measured_secs: (before + self.gated.as_secs_f64()).min(MAX_MEASURED_SECS),
        })
    }
}

/// Share of the distance covered in one block with time constant `tau` (seconds).
fn smoothing(tau: f64) -> f64 {
    1.0 - (-BLOCK.as_secs_f64() / tau).exp()
}

fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn linear_to_db(linear: f64) -> f64 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// The leveler of a playback, with where its level is remembered: saved every minute and when the
/// playback ends (dropped).
pub struct StationLevel {
    pub leveler: Leveler,
    memory: Option<Arc<dyn LevelMemory>>,
    station: String,
    saved: Instant,
}

impl StationLevel {
    /// `station`: the Radio-Browser UUID, else the stream URL.
    pub fn new(
        memory: Option<Arc<dyn LevelMemory>>,
        station: String,
        sample_rate: u32,
        channels: usize,
        enabled: bool,
    ) -> Self {
        let stored = memory.as_ref().and_then(|m| m.recall(&station));
        let guess = memory
            .as_ref()
            .and_then(|m| m.guess())
            .unwrap_or(DEFAULT_GUESS_LUFS);
        tracing::debug!(%station, ?stored, guess, "station level");
        Self {
            leveler: Leveler::new(sample_rate, channels, enabled, stored, guess),
            memory,
            station,
            saved: Instant::now(),
        }
    }

    pub fn process(&mut self, samples: &mut [f32], enabled: bool) {
        self.leveler.process(samples, enabled);
        if self.saved.elapsed() >= SAVE_EVERY {
            self.save();
        }
    }

    fn save(&mut self) {
        self.saved = Instant::now();
        if let (Some(memory), Some(level)) = (&self.memory, self.leveler.level()) {
            memory.remember(&self.station, &level);
        }
    }
}

impl Drop for StationLevel {
    fn drop(&mut self) {
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// A stereo 1 kHz sine whose loudness is `lufs` (calibrated with the meter itself).
    fn sine(lufs: f64, seconds: f64) -> Vec<f32> {
        let frames = (seconds * f64::from(RATE)) as usize;
        let raw: Vec<f32> = (0..frames.max(RATE as usize))
            .flat_map(|i| {
                let s = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(RATE)).sin()
                    as f32
                    * 0.1;
                [s, s]
            })
            .collect();
        let mut meter = EbuR128::new(2, RATE, Mode::M).unwrap();
        meter.add_frames_f32(&raw[..RATE as usize]).unwrap();
        let scale = db_to_linear(lufs - meter.loudness_momentary().unwrap()) as f32;
        raw.into_iter()
            .take(frames * 2)
            .map(|s| s * scale)
            .collect()
    }

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0; (seconds * f64::from(RATE)) as usize * 2]
    }

    /// Feeds `signal` in decoder-sized packets; returns the gain after each packet.
    fn play(leveler: &mut Leveler, signal: &[f32], enabled: bool) -> Vec<f64> {
        let mut gains = Vec::new();
        for packet in signal.chunks(1152 * 2) {
            let mut packet = packet.to_vec();
            leveler.process(&mut packet, enabled);
            gains.push(leveler.gain_db());
        }
        gains
    }

    fn unknown() -> Leveler {
        Leveler::new(RATE, 2, true, None, DEFAULT_GUESS_LUFS)
    }

    fn known(lufs: f64) -> Leveler {
        known_with_peak(lufs, lufs + 3.0)
    }

    fn known_with_peak(lufs: f64, peak_dbfs: f64) -> Leveler {
        let stored = StoredLevel {
            loudness_lufs: lufs,
            peak_dbfs,
            measured_secs: 600.0,
        };
        Leveler::new(RATE, 2, true, Some(stored), DEFAULT_GUESS_LUFS)
    }

    #[test]
    fn an_unknown_station_starts_at_the_guess() {
        assert!((unknown().gain_db() - (TARGET_LUFS - DEFAULT_GUESS_LUFS)).abs() < 1e-9);
    }

    #[test]
    fn a_loud_station_is_brought_down_within_three_seconds() {
        let mut leveler = unknown();
        play(&mut leveler, &sine(-8.0, 3.0), true);
        assert!(
            (leveler.gain_db() - -8.0).abs() < 1.0,
            "{}",
            leveler.gain_db()
        );
    }

    #[test]
    fn a_station_at_the_target_is_left_alone() {
        let mut leveler = unknown();
        play(&mut leveler, &sine(-16.0, 4.0), true);
        assert!(leveler.gain_db().abs() < 1.0, "{}", leveler.gain_db());
    }

    #[test]
    fn a_quiet_station_comes_up_capped_while_learning_then_fully() {
        let mut leveler = unknown();
        let gains = play(&mut leveler, &sine(-26.0, 9.0), true);
        assert!(gains.iter().all(|g| *g <= MAX_BOOST_LEARNING_DB + 1e-9));
        assert!(
            (leveler.gain_db() - MAX_BOOST_LEARNING_DB).abs() < 0.1,
            "{}",
            leveler.gain_db()
        );
        play(&mut leveler, &sine(-26.0, 12.0), true);
        assert!(
            (leveler.gain_db() - MAX_BOOST_DB).abs() < 0.1,
            "{}",
            leveler.gain_db()
        );
    }

    #[test]
    fn a_quiet_intro_never_takes_the_gain_above_the_learning_cap() {
        let mut leveler = unknown();
        let mut gains = play(&mut leveler, &sine(-36.0, 5.0), true);
        gains.extend(play(&mut leveler, &sine(-10.0, 5.0), true));
        assert!(gains.iter().all(|g| *g <= MAX_BOOST_LEARNING_DB + 1e-9));
        assert!(leveler.gain_db() < -4.0, "{}", leveler.gain_db());
    }

    #[test]
    fn silence_changes_nothing() {
        let mut leveler = known(-10.0);
        let gains = play(&mut leveler, &silence(30.0), true);
        assert!(gains.iter().all(|g| (*g - -6.0).abs() < 1e-9));
        let mut leveler = unknown();
        play(&mut leveler, &silence(30.0), true);
        assert!((leveler.gain_db() - -5.0).abs() < 1e-9);
        assert!(leveler.level().is_none(), "silence teaches nothing");
    }

    #[test]
    fn a_quiet_passage_does_not_pump_the_gain_up() {
        let mut leveler = known(-12.0);
        play(&mut leveler, &sine(-12.0, 10.0), true);
        let before = leveler.gain_db();
        let gains = play(&mut leveler, &sine(-30.0, 20.0), true);
        assert!(
            gains.iter().all(|g| *g < before + 1.0),
            "{before} → {:?}",
            gains.last()
        );
    }

    #[test]
    fn a_station_that_gets_quieter_for_good_is_followed() {
        let mut leveler = known(-12.0);
        play(&mut leveler, &sine(-30.0, 120.0), true);
        assert!(leveler.gain_db() > 5.0, "{}", leveler.gain_db());
    }

    #[test]
    fn a_known_station_plays_at_its_level_from_the_first_sample() {
        let leveler = known(-10.0);
        assert!((leveler.gain_db() - -6.0).abs() < 1e-9);
        let mut packet = sine(-10.0, 0.01);
        let original = packet.clone();
        let mut leveler = known(-10.0);
        leveler.process(&mut packet, true);
        let ratio = packet[11] / original[11];
        assert!(
            (linear_to_db(f64::from(ratio)) - -6.0).abs() < 0.01,
            "{ratio}"
        );
    }

    #[test]
    fn a_boost_never_takes_the_peak_above_the_ceiling() {
        // Quiet (−24 LUFS) but with peaks at −3 dBFS: 2 dB of boost at most.
        let mut signal = sine(-24.0, 30.0);
        for frame in signal.chunks_mut(2).step_by(RATE as usize / 5) {
            frame.fill(db_to_linear(-3.0) as f32);
        }
        let mut leveler = known_with_peak(-24.0, -3.0);
        let gains = play(&mut leveler, &signal, true);
        assert!(
            gains.iter().all(|g| *g <= 2.0 + 1e-6),
            "{:?}",
            gains.iter().cloned().fold(f64::MIN, f64::max)
        );
    }

    #[test]
    fn switching_off_ramps_back_to_unity() {
        let mut leveler = known(-8.0);
        play(&mut leveler, &sine(-8.0, 1.0), true);
        assert!(leveler.gain_db() < -7.0);
        let gains = play(&mut leveler, &sine(-8.0, 1.0), false);
        assert_eq!(*gains.last().unwrap(), 0.0);
        // A ramp, not a jump.
        assert!(gains[0] < -6.0, "{}", gains[0]);
        play(&mut leveler, &sine(-8.0, 1.0), true);
        assert!(leveler.gain_db() < -7.0, "back on");
    }

    #[test]
    fn a_changed_station_drops_its_history() {
        let mut leveler = known(-20.0);
        play(&mut leveler, &sine(-10.0, 40.0), true);
        assert_eq!(leveler.history, 0.0);
        let level = leveler.level().unwrap();
        assert!((level.loudness_lufs - -10.0).abs() < 1.0, "{level:?}");
        assert!(level.measured_secs < 60.0, "{level:?}");
    }

    #[test]
    fn the_level_is_remembered_after_ten_seconds() {
        let mut leveler = unknown();
        play(&mut leveler, &sine(-12.0, 5.0), true);
        assert!(leveler.level().is_none());
        play(&mut leveler, &sine(-12.0, 8.0), true);
        let level = leveler.level().unwrap();
        assert!((level.loudness_lufs - -12.0).abs() < 0.5, "{level:?}");
        assert!(
            level.peak_dbfs < 0.0 && level.peak_dbfs > -20.0,
            "{level:?}"
        );
    }
}
