// SPDX-License-Identifier: MPL-2.0
//! Sound card output via cpal.
//!
//! The format is the device's default: the decoder must adapt to it
//! (see `decoder::Converter`), never the other way around. The real-time callback only
//! pops from an SPSC ring buffer, applies the volume and converts to the native sample type.
//!
//! The output can die under the player (sound card gone, sound server restarted, wake from
//! sleep): [`Health`] tells the engine, which then opens a new one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rtrb::{Consumer, Producer, RingBuffer};

use super::Controls;

/// Ring buffer depth (the decoder is throttled when it is full).
const BUFFER_SECONDS: usize = 2;
/// Pre-buffer at startup: deliberately minimal, time to first sound matters more than headroom.
const START_BUFFER_MS: usize = 120;
/// Pre-buffer after an underrun: we already had to wait, so build up some headroom.
const REBUFFER_MS: usize = 1000;
/// The callback normally runs every few milliseconds, paused or not: this long without it (awake
/// time) means the output is dead.
const SILENT_CALLBACK: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    pub sample_rate: u32,
    pub channels: usize,
}

/// Keeps the cpal `Stream` alive; dropping it silences the sound.
pub struct Output {
    pub format: OutputFormat,
    pub health: Arc<Health>,
    _stream: Stream,
}

/// Whether the output still plays, written from the audio callbacks.
pub struct Health {
    failed: AtomicBool,
    base: Instant,
    last_callback_ms: AtomicU64,
}

impl Health {
    fn new() -> Self {
        Self {
            failed: AtomicBool::new(false),
            base: Instant::now(),
            last_callback_ms: AtomicU64::new(0),
        }
    }

    fn alive(&self) {
        self.last_callback_ms
            .store(self.base.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// The device is gone, or the callback stopped running.
    pub fn is_lost(&self) -> bool {
        let last = Duration::from_millis(self.last_callback_ms.load(Ordering::Relaxed));
        self.failed.load(Ordering::Relaxed)
            || self.base.elapsed().saturating_sub(last) > SILENT_CALLBACK
    }
}

pub fn open(controls: Arc<Controls>) -> Result<(Output, Producer<f32>), String> {
    let started = std::time::Instant::now();
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No audio output device available")?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Audio configuration unavailable: {e}"))?;

    let format = OutputFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels() as usize,
    };
    let samples_per_second = format.sample_rate as usize * format.channels;
    let (producer, consumer) = RingBuffer::<f32>::new(samples_per_second * BUFFER_SECONDS);
    let thresholds = Thresholds {
        start: samples_per_second * START_BUFFER_MS / 1000,
        rebuffer: samples_per_second * REBUFFER_MS / 1000,
    };

    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();

    let health = Arc::new(Health::new());
    let h = health.clone();
    let stream = match sample_format {
        SampleFormat::F32 => build::<f32>(&device, &config, consumer, controls, thresholds, h),
        SampleFormat::I16 => build::<i16>(&device, &config, consumer, controls, thresholds, h),
        SampleFormat::U16 => build::<u16>(&device, &config, consumer, controls, thresholds, h),
        SampleFormat::I32 => build::<i32>(&device, &config, consumer, controls, thresholds, h),
        other => return Err(format!("Unsupported sample format: {other}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("Cannot start the audio output: {e}"))?;

    tracing::info!(
        rate = format.sample_rate,
        channels = format.channels,
        ?sample_format,
        ms = started.elapsed().as_millis() as u64,
        "audio output opened"
    );
    Ok((
        Output {
            format,
            health,
            _stream: stream,
        },
        producer,
    ))
}

#[derive(Clone, Copy)]
struct Thresholds {
    start: usize,
    rebuffer: usize,
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut consumer: Consumer<f32>,
    controls: Arc<Controls>,
    thresholds: Thresholds,
    health: Arc<Health>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    // True while the pre-buffer is filling (at startup, then after each underrun).
    let mut buffering = true;
    let mut needed = thresholds.start;
    let errors = health.clone();

    device
        .build_output_stream(
            *config,
            move |data: &mut [T], _| {
                health.alive();
                if buffering && consumer.slots() >= needed {
                    buffering = false;
                }
                let volume = controls.volume();
                let paused = controls.is_paused();

                for out in data.iter_mut() {
                    let sample = if buffering {
                        0.0
                    } else {
                        match consumer.pop() {
                            // While paused we keep consuming so playback stays live.
                            Ok(s) if !paused => s * volume,
                            Ok(_) => 0.0,
                            Err(_) => {
                                buffering = true;
                                needed = thresholds.rebuffer;
                                0.0
                            }
                        }
                    };
                    *out = T::from_sample(sample);
                }
            },
            move |err: cpal::Error| {
                tracing::warn!("audio output stream error: {err}");
                if matches!(
                    err.kind(),
                    cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated
                ) {
                    errors.failed.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| format!("Cannot open the audio output: {e}"))
}
