// SPDX-License-Identifier: MPL-2.0
//! Adapts decoded audio (channels, sample rate) to the output device.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use super::super::output::OutputFormat;

/// Block size of the FFT resampler (~21 ms at 48 kHz): latency / CPU cost trade-off.
const RESAMPLER_CHUNK_FRAMES: usize = 1024;

/// Remaps channels then resamples to the output device's format.
///
/// Resampling (`rubato::Fft`, high quality, fixed ratio) works on fixed-size blocks:
/// decoded packets, whose size varies by codec, are accumulated in `pending`.
/// When the sample rates match, it is bypassed (direct copy).
pub(super) struct Converter {
    pub(super) src_rate: u32,
    pub(super) src_channels: usize,
    dst_channels: usize,
    remapped: Vec<f32>,
    pub(super) resampler: Option<Fft<f32>>,
    /// Remapped frames not yet fed to the resampler (interleaved).
    pending: Vec<f32>,
    /// Output of one block (interleaved), sized to the resampler's maximum.
    scratch: Vec<f32>,
    /// Initial silent frames introduced by the filter, to be discarded.
    skip_frames: usize,
}

impl Converter {
    pub(super) fn new(
        src_rate: u32,
        src_channels: usize,
        dst: OutputFormat,
    ) -> Result<Self, String> {
        let (resampler, scratch, skip_frames) = if src_rate == dst.sample_rate {
            (None, Vec::new(), 0)
        } else {
            let resampler = Fft::<f32>::new(
                src_rate as usize,
                dst.sample_rate as usize,
                RESAMPLER_CHUNK_FRAMES,
                dst.channels,
                FixedSync::Input,
            )
            .map_err(|e| format!("Cannot resample {src_rate} → {} Hz: {e}", dst.sample_rate))?;
            let scratch = vec![0.0; resampler.output_frames_max() * dst.channels];
            let delay = resampler.output_delay();
            (Some(resampler), scratch, delay)
        };

        Ok(Self {
            src_rate,
            src_channels,
            dst_channels: dst.channels,
            remapped: Vec::new(),
            resampler,
            pending: Vec::new(),
            scratch,
            skip_frames,
        })
    }

    pub(super) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.remap(input);
        let Some(resampler) = self.resampler.as_mut() else {
            out.extend_from_slice(&self.remapped);
            return;
        };

        let ch = self.dst_channels;
        self.pending.extend_from_slice(&self.remapped);
        loop {
            let needed = resampler.input_frames_next();
            if self.pending.len() < needed * ch {
                break;
            }
            let capacity = resampler.output_frames_max();
            let (Ok(frames_in), Ok(mut frames_out)) = (
                InterleavedSlice::new(&self.pending[..needed * ch], ch, needed),
                InterleavedSlice::new_mut(&mut self.scratch[..capacity * ch], ch, capacity),
            ) else {
                unreachable!("sizes computed from the resampler");
            };

            match resampler.process_into_buffer(&frames_in, &mut frames_out, None) {
                Ok((read, written)) => {
                    let skip = self.skip_frames.min(written);
                    self.skip_frames -= skip;
                    out.extend_from_slice(&self.scratch[skip * ch..written * ch]);
                    self.pending.drain(..read * ch);
                }
                Err(e) => {
                    // Should not happen; drop the block rather than loop.
                    tracing::warn!("resampling: {e}");
                    self.pending.clear();
                    break;
                }
            }
        }
    }

    fn remap(&mut self, input: &[f32]) {
        let (src, dst) = (self.src_channels, self.dst_channels);
        self.remapped.clear();
        if src == dst {
            self.remapped.extend_from_slice(input);
            return;
        }
        for frame in input.chunks_exact(src) {
            for c in 0..dst {
                self.remapped.push(match (src, dst) {
                    (1, _) => frame[0],
                    // Downmix to mono: average of front left/right.
                    (_, 1) => (frame[0] + frame[1]) * 0.5,
                    _ => frame.get(c).copied().unwrap_or(0.0),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn format(rate: u32, channels: usize) -> OutputFormat {
        OutputFormat {
            sample_rate: rate,
            channels,
        }
    }

    fn sine(rate: u32, hz: f32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|n| (TAU * hz * n as f32 / rate as f32).sin())
            .collect()
    }

    /// Frequency estimated by counting rising zero crossings.
    fn frequency(signal: &[f32], rate: u32) -> f32 {
        let crossings = signal
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        crossings as f32 * rate as f32 / signal.len() as f32
    }

    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt()
    }

    #[test]
    fn same_format_is_passthrough() {
        let mut c = Converter::new(48_000, 2, format(48_000, 2)).unwrap();
        let mut out = Vec::new();
        c.process(&[0.1, 0.2, 0.3, 0.4], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn mono_is_duplicated_and_stereo_is_downmixed() {
        let mut up = Converter::new(48_000, 1, format(48_000, 2)).unwrap();
        let mut out = Vec::new();
        up.process(&[0.5, -0.5], &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);

        let mut down = Converter::new(48_000, 2, format(48_000, 1)).unwrap();
        out.clear();
        down.process(&[1.0, 0.0, 0.5, 0.5], &mut out);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn upsampling_preserves_pitch_and_level_across_irregular_packets() {
        // 1 kHz at 44.1 kHz → 48 kHz, in MP3-sized packets (1152): no pitch or level drift.
        let mut c = Converter::new(44_100, 1, format(48_000, 1)).unwrap();
        let mut out = Vec::new();
        for chunk in sine(44_100, 1000.0, 2.0).chunks(1152) {
            c.process(chunk, &mut out);
        }
        // The last blocks stay pending: the loss is bounded by the block size.
        assert!(
            out.len() > 2 * 48_000 - 3 * RESAMPLER_CHUNK_FRAMES * 2,
            "len = {}",
            out.len()
        );

        let steady = &out[4800..];
        assert!((frequency(steady, 48_000) - 1000.0).abs() < 5.0);
        assert!(
            (rms(steady) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
            "rms = {}",
            rms(steady)
        );
    }

    #[test]
    fn downsampling_stereo_keeps_both_channels_in_order() {
        // Left channel = 440 Hz, right channel = silence: must stay separate after resampling.
        let left = sine(48_000, 440.0, 1.0);
        let interleaved: Vec<f32> = left.iter().flat_map(|&l| [l, 0.0]).collect();
        let mut c = Converter::new(48_000, 2, format(24_000, 2)).unwrap();
        let mut out = Vec::new();
        for chunk in interleaved.chunks(2 * 1000) {
            c.process(chunk, &mut out);
        }
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        let r: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        assert!((frequency(&l[2000..], 24_000) - 440.0).abs() < 5.0);
        assert!(rms(&r) < 1e-3, "leak into the right channel: {}", rms(&r));
    }

    #[test]
    fn unsupported_rate_is_an_error_not_a_panic() {
        assert!(Converter::new(0, 2, format(48_000, 2)).is_err());
    }
}
