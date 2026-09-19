//! Real-time-safe stereo equalizer core.
//!
//! Parameter writers may run on any non-audio thread. The audio thread takes one
//! coherent [`ParamSnapshot`] per block and then performs no allocation or locking.

use std::array;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const BAND_COUNT: usize = 10;
pub const BAND_FREQUENCIES: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];
pub const MIN_GAIN_DB: f32 = -12.0;
pub const MAX_GAIN_DB: f32 = 12.0;
pub const MIN_PREAMP_DB: f32 = -18.0;
pub const MAX_PREAMP_DB: f32 = 6.0;
pub const MIN_LIMITER_THRESHOLD: f32 = 0.1;
pub const MAX_LIMITER_THRESHOLD: f32 = 1.0;

const DEFAULT_LIMITER_THRESHOLD: f32 = 0.98;
const MIN_SAMPLE_RATE: f32 = 8_000.0;
const MAX_SAMPLE_RATE: f32 = 384_000.0;
const INPUT_LIMIT: f32 = 16.0;
const BAND_Q: f32 = 1.4;
const SMOOTHING_SECONDS: f32 = 0.010;
const SMOOTHING_EPSILON: f32 = 1.0e-5;

/// A complete, coherent set of user-controlled DSP parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSnapshot {
    pub band_gains_db: [f32; BAND_COUNT],
    pub band_enabled: [bool; BAND_COUNT],
    pub preamp_db: f32,
    pub bypass: bool,
    pub limiter_enabled: bool,
    pub limiter_threshold: f32,
}

impl Default for ParamSnapshot {
    fn default() -> Self {
        Self {
            band_gains_db: [0.0; BAND_COUNT],
            band_enabled: [true; BAND_COUNT],
            preamp_db: 0.0,
            bypass: false,
            limiter_enabled: true,
            limiter_threshold: DEFAULT_LIMITER_THRESHOLD,
        }
    }
}

impl ParamSnapshot {
    /// Returns a finite snapshot restricted to the DSP's safety bounds.
    pub fn sanitized(mut self) -> Self {
        for gain in &mut self.band_gains_db {
            *gain = finite_clamp(*gain, 0.0, MIN_GAIN_DB, MAX_GAIN_DB);
        }
        self.preamp_db = finite_clamp(self.preamp_db, 0.0, MIN_PREAMP_DB, MAX_PREAMP_DB);
        self.limiter_threshold = finite_clamp(
            self.limiter_threshold,
            DEFAULT_LIMITER_THRESHOLD,
            MIN_LIMITER_THRESHOLD,
            MAX_LIMITER_THRESHOLD,
        );
        self
    }
}

/// Lock-free shared parameters using a sequence counter for coherent snapshots.
///
/// Writers are serialized by an atomic compare-exchange. Readers never observe
/// a mixture of two updates. Keep update closures short because readers retry
/// while a writer owns the sequence.
pub struct SharedParams {
    sequence: AtomicU64,
    band_gains_db: [AtomicU32; BAND_COUNT],
    band_enabled: [AtomicU32; BAND_COUNT],
    preamp_db: AtomicU32,
    bypass: AtomicU32,
    limiter_enabled: AtomicU32,
    limiter_threshold: AtomicU32,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new(ParamSnapshot::default())
    }
}

impl SharedParams {
    pub fn new(params: ParamSnapshot) -> Self {
        let params = params.sanitized();
        Self {
            sequence: AtomicU64::new(0),
            band_gains_db: array::from_fn(|i| AtomicU32::new(params.band_gains_db[i].to_bits())),
            band_enabled: array::from_fn(|i| AtomicU32::new(params.band_enabled[i] as u32)),
            preamp_db: AtomicU32::new(params.preamp_db.to_bits()),
            bypass: AtomicU32::new(params.bypass as u32),
            limiter_enabled: AtomicU32::new(params.limiter_enabled as u32),
            limiter_threshold: AtomicU32::new(params.limiter_threshold.to_bits()),
        }
    }

    /// Reads all parameters as one coherent version without locking.
    pub fn snapshot(&self) -> ParamSnapshot {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                spin_loop();
                continue;
            }

            let params = self.load_relaxed();
            let after = self.sequence.load(Ordering::Acquire);
            if before == after {
                return params;
            }
            spin_loop();
        }
    }

    /// Attempts one coherent read and returns `fallback` if a writer is active.
    /// This is the real-time-safe reader: it never spins or waits.
    pub fn try_snapshot(&self, fallback: ParamSnapshot) -> ParamSnapshot {
        let before = self.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            return fallback;
        }
        let params = self.load_relaxed();
        let after = self.sequence.load(Ordering::Acquire);
        if before == after { params } else { fallback }
    }

    /// Atomically publishes all changes made by `update` as one version.
    pub fn update<R>(&self, update: impl FnOnce(&mut ParamSnapshot) -> R) -> R {
        let sequence = loop {
            let current = self.sequence.load(Ordering::Relaxed);
            if current & 1 == 0
                && self
                    .sequence
                    .compare_exchange_weak(
                        current,
                        current.wrapping_add(1),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                break current;
            }
            spin_loop();
        };

        // If the closure panics, Drop makes the old values readable again.
        let publish = PublishOnDrop {
            sequence: &self.sequence,
            value: sequence.wrapping_add(2),
        };
        let mut params = self.load_relaxed();
        let result = update(&mut params);
        self.store_relaxed(params.sanitized());
        drop(publish);
        result
    }

    pub fn replace(&self, params: ParamSnapshot) {
        self.update(|current| *current = params);
    }

    pub fn set_band_gain(&self, band: usize, gain_db: f32) -> bool {
        if band >= BAND_COUNT {
            return false;
        }
        self.update(|params| params.band_gains_db[band] = gain_db);
        true
    }

    pub fn set_band_enabled(&self, band: usize, enabled: bool) -> bool {
        if band >= BAND_COUNT {
            return false;
        }
        self.update(|params| params.band_enabled[band] = enabled);
        true
    }

    pub fn set_preamp_db(&self, preamp_db: f32) {
        self.update(|params| params.preamp_db = preamp_db);
    }

    pub fn set_bypass(&self, bypass: bool) {
        self.update(|params| params.bypass = bypass);
    }

    pub fn set_limiter(&self, enabled: bool, threshold: f32) {
        self.update(|params| {
            params.limiter_enabled = enabled;
            params.limiter_threshold = threshold;
        });
    }

    fn load_relaxed(&self) -> ParamSnapshot {
        ParamSnapshot {
            band_gains_db: array::from_fn(|i| {
                f32::from_bits(self.band_gains_db[i].load(Ordering::Relaxed))
            }),
            band_enabled: array::from_fn(|i| self.band_enabled[i].load(Ordering::Relaxed) != 0),
            preamp_db: f32::from_bits(self.preamp_db.load(Ordering::Relaxed)),
            bypass: self.bypass.load(Ordering::Relaxed) != 0,
            limiter_enabled: self.limiter_enabled.load(Ordering::Relaxed) != 0,
            limiter_threshold: f32::from_bits(self.limiter_threshold.load(Ordering::Relaxed)),
        }
    }

    fn store_relaxed(&self, params: ParamSnapshot) {
        for i in 0..BAND_COUNT {
            self.band_gains_db[i].store(params.band_gains_db[i].to_bits(), Ordering::Relaxed);
            self.band_enabled[i].store(params.band_enabled[i] as u32, Ordering::Relaxed);
        }
        self.preamp_db
            .store(params.preamp_db.to_bits(), Ordering::Relaxed);
        self.bypass.store(params.bypass as u32, Ordering::Relaxed);
        self.limiter_enabled
            .store(params.limiter_enabled as u32, Ordering::Relaxed);
        self.limiter_threshold
            .store(params.limiter_threshold.to_bits(), Ordering::Relaxed);
    }
}

struct PublishOnDrop<'a> {
    sequence: &'a AtomicU64,
    value: u64,
}

impl Drop for PublishOnDrop<'_> {
    fn drop(&mut self) {
        self.sequence.store(self.value, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameMetrics {
    pub input_peak: f32,
    pub output_peak: f32,
    /// True when the pre-limiter signal reached or exceeded digital full scale.
    pub clipped: bool,
    pub limited: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockMetrics {
    pub input_peak: f32,
    pub output_peak: f32,
    pub clipped: bool,
    pub limited: bool,
    pub clipped_frames: u32,
    pub limited_frames: u32,
}

impl BlockMetrics {
    pub fn add_frame(&mut self, frame: FrameMetrics) {
        self.input_peak = self.input_peak.max(frame.input_peak);
        self.output_peak = self.output_peak.max(frame.output_peak);
        self.clipped |= frame.clipped;
        self.limited |= frame.limited;
        self.clipped_frames = self.clipped_frames.saturating_add(frame.clipped as u32);
        self.limited_frames = self.limited_frames.saturating_add(frame.limited as u32);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcessedFrame {
    pub left: f32,
    pub right: f32,
    pub metrics: FrameMetrics,
}

#[derive(Clone, Copy, Debug, Default)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefficients {
    fn peaking(sample_rate: f32, frequency: f32, gain_db: f32) -> Self {
        let frequency = frequency.min(sample_rate * 0.475);
        let omega = 2.0 * std::f32::consts::PI * frequency / sample_rate;
        let (sin, cos) = omega.sin_cos();
        let a = 10.0_f32.powf(gain_db / 40.0);
        let alpha = sin / (2.0 * BAND_Q);
        let a0 = 1.0 + alpha / a;

        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn process(&mut self, input: f32, coefficients: Coefficients) -> f32 {
        let output = coefficients.b0 * input + self.z1;
        self.z1 = flush_denormal(coefficients.b1 * input - coefficients.a1 * output + self.z2);
        self.z2 = flush_denormal(coefficients.b2 * input - coefficients.a2 * output);
        finite_or_zero(output)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Stateful stereo ten-band equalizer. One instance must belong to one audio stream.
pub struct StereoEq {
    sample_rate: f32,
    smooth_factor: f32,
    band_gains_db: [f32; BAND_COUNT],
    target_band_gains_db: [f32; BAND_COUNT],
    coefficients: [Coefficients; BAND_COUNT],
    filters: [[Biquad; BAND_COUNT]; 2],
    preamp_db: f32,
    target_preamp_db: f32,
    bypass_mix: f32,
    target_bypass_mix: f32,
    limiter_enabled: bool,
    limiter_threshold: f32,
}

impl StereoEq {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = valid_sample_rate(sample_rate);
        let coefficients =
            array::from_fn(|i| Coefficients::peaking(sample_rate, BAND_FREQUENCIES[i], 0.0));
        Self {
            sample_rate,
            smooth_factor: smoothing_factor(sample_rate),
            band_gains_db: [0.0; BAND_COUNT],
            target_band_gains_db: [0.0; BAND_COUNT],
            coefficients,
            filters: [[Biquad::default(); BAND_COUNT]; 2],
            preamp_db: 0.0,
            target_preamp_db: 0.0,
            bypass_mix: 0.0,
            target_bypass_mix: 0.0,
            limiter_enabled: true,
            limiter_threshold: DEFAULT_LIMITER_THRESHOLD,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Changes the sample rate and clears filter history.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = valid_sample_rate(sample_rate);
        self.smooth_factor = smoothing_factor(self.sample_rate);
        for (i, frequency) in BAND_FREQUENCIES.iter().copied().enumerate() {
            self.coefficients[i] =
                Coefficients::peaking(self.sample_rate, frequency, self.band_gains_db[i]);
        }
        self.reset();
    }

    /// Sets smoothing targets. Disabled bands smoothly move to unity gain.
    pub fn apply_params(&mut self, params: &ParamSnapshot) {
        let params = params.sanitized();
        for i in 0..BAND_COUNT {
            self.target_band_gains_db[i] = if params.band_enabled[i] {
                params.band_gains_db[i]
            } else {
                0.0
            };
        }
        self.target_preamp_db = params.preamp_db;
        self.target_bypass_mix = params.bypass as u8 as f32;
        self.limiter_enabled = params.limiter_enabled;
        self.limiter_threshold = params.limiter_threshold;
    }

    /// Immediately adopts parameters, useful when starting or restoring a stream.
    pub fn apply_params_immediate(&mut self, params: &ParamSnapshot) {
        self.apply_params(params);
        self.band_gains_db = self.target_band_gains_db;
        self.preamp_db = self.target_preamp_db;
        self.bypass_mix = self.target_bypass_mix;
        for (i, frequency) in BAND_FREQUENCIES.iter().copied().enumerate() {
            self.coefficients[i] =
                Coefficients::peaking(self.sample_rate, frequency, self.band_gains_db[i]);
        }
    }

    pub fn reset(&mut self) {
        for channel in &mut self.filters {
            for filter in channel {
                filter.reset();
            }
        }
    }

    pub fn process_frame(&mut self, left: f32, right: f32) -> ProcessedFrame {
        let left = sanitize_input(left);
        let right = sanitize_input(right);
        let input_peak = left.abs().max(right.abs());

        self.advance_smoothing();
        let preamp = 10.0_f32.powf(self.preamp_db / 20.0);
        let mut wet = [left * preamp, right * preamp];
        for (channel, sample) in wet.iter_mut().enumerate() {
            for band in 0..BAND_COUNT {
                *sample = self.filters[channel][band].process(*sample, self.coefficients[band]);
            }
        }

        let dry = [left, right];
        let mut output = [
            wet[0] + (dry[0] - wet[0]) * self.bypass_mix,
            wet[1] + (dry[1] - wet[1]) * self.bypass_mix,
        ];
        output[0] = finite_or_zero(output[0]);
        output[1] = finite_or_zero(output[1]);

        let pre_limiter_peak = output[0].abs().max(output[1].abs());
        let clipped = pre_limiter_peak >= 1.0;
        let limited = self.limiter_enabled && pre_limiter_peak > self.limiter_threshold;
        if limited {
            let scale = self.limiter_threshold / pre_limiter_peak;
            output[0] *= scale;
            output[1] *= scale;
        }

        ProcessedFrame {
            left: output[0],
            right: output[1],
            metrics: FrameMetrics {
                input_peak,
                output_peak: output[0].abs().max(output[1].abs()),
                clipped,
                limited,
            },
        }
    }

    /// Processes interleaved stereo samples in place. A trailing unpaired sample
    /// is sanitized but otherwise left unchanged.
    pub fn process_interleaved(
        &mut self,
        samples: &mut [f32],
        params: &ParamSnapshot,
    ) -> BlockMetrics {
        self.apply_params(params);
        let mut metrics = BlockMetrics::default();
        let (frames, remainder) = samples.as_chunks_mut::<2>();
        for frame in frames {
            let processed = self.process_frame(frame[0], frame[1]);
            frame[0] = processed.left;
            frame[1] = processed.right;
            metrics.add_frame(processed.metrics);
        }
        if let [sample] = remainder {
            *sample = sanitize_input(*sample);
        }
        metrics
    }

    /// Processes separate channel buffers. Only the shared prefix is processed.
    pub fn process_planar(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        params: &ParamSnapshot,
    ) -> BlockMetrics {
        self.apply_params(params);
        let mut metrics = BlockMetrics::default();
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            let processed = self.process_frame(*left, *right);
            *left = processed.left;
            *right = processed.right;
            metrics.add_frame(processed.metrics);
        }
        metrics
    }

    fn advance_smoothing(&mut self) {
        for (i, frequency) in BAND_FREQUENCIES.iter().copied().enumerate() {
            let previous = self.band_gains_db[i];
            self.band_gains_db[i] =
                smooth(previous, self.target_band_gains_db[i], self.smooth_factor);
            if self.band_gains_db[i] != previous {
                self.coefficients[i] =
                    Coefficients::peaking(self.sample_rate, frequency, self.band_gains_db[i]);
            }
        }
        self.preamp_db = smooth(self.preamp_db, self.target_preamp_db, self.smooth_factor);
        self.bypass_mix = smooth(self.bypass_mix, self.target_bypass_mix, self.smooth_factor);
    }
}

fn finite_clamp(value: f32, fallback: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

fn valid_sample_rate(sample_rate: f32) -> f32 {
    finite_clamp(sample_rate, 48_000.0, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE)
}

fn smoothing_factor(sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (SMOOTHING_SECONDS * sample_rate)).exp()
}

fn smooth(current: f32, target: f32, factor: f32) -> f32 {
    let next = current + (target - current) * factor;
    if (next - target).abs() <= SMOOTHING_EPSILON {
        target
    } else {
        next
    }
}

fn sanitize_input(sample: f32) -> f32 {
    finite_clamp(sample, 0.0, -INPUT_LIMIT, INPUT_LIMIT)
}

fn finite_or_zero(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

fn flush_denormal(sample: f32) -> f32 {
    if sample.abs() < 1.0e-30 {
        0.0
    } else {
        finite_or_zero(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const SAMPLE_RATE: f32 = 48_000.0;

    #[test]
    fn zero_db_is_identity() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        let params = ParamSnapshot {
            limiter_enabled: false,
            ..ParamSnapshot::default()
        };
        eq.apply_params_immediate(&params);

        for i in 0..20_000 {
            let left = ((i as f32) * 0.017).sin() * 0.4;
            let right = ((i as f32) * 0.031).cos() * 0.3;
            let output = eq.process_frame(left, right);
            assert!((output.left - left).abs() < 2.0e-5);
            assert!((output.right - right).abs() < 2.0e-5);
        }
    }

    #[test]
    fn gain_at_each_band_center_is_close_to_requested() {
        for (band, frequency) in BAND_FREQUENCIES.iter().copied().enumerate() {
            if frequency >= SAMPLE_RATE * 0.45 {
                continue;
            }
            let mut params = ParamSnapshot::default();
            params.band_gains_db[band] = 6.0;
            params.limiter_enabled = false;
            let mut eq = StereoEq::new(SAMPLE_RATE);
            eq.apply_params_immediate(&params);

            let mut input_energy = 0.0_f64;
            let mut output_energy = 0.0_f64;
            for i in 0..24_000 {
                let sample =
                    (2.0 * std::f32::consts::PI * frequency * i as f32 / SAMPLE_RATE).sin() * 0.05;
                let output = eq.process_frame(sample, 0.0).left;
                if i >= 4_000 {
                    input_energy += (sample * sample) as f64;
                    output_energy += (output * output) as f64;
                }
            }
            let measured_db = 10.0 * (output_energy / input_energy).log10();
            assert!(
                (measured_db - 6.0).abs() < 0.15,
                "band {band} at {frequency} Hz measured {measured_db:.3} dB"
            );
        }
    }

    #[test]
    fn parameters_and_samples_are_finite_and_bounded() {
        let shared = SharedParams::new(ParamSnapshot {
            band_gains_db: [f32::INFINITY; BAND_COUNT],
            preamp_db: f32::NAN,
            limiter_threshold: -50.0,
            ..ParamSnapshot::default()
        });
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.band_gains_db, [0.0; BAND_COUNT]);
        assert_eq!(snapshot.preamp_db, 0.0);
        assert_eq!(snapshot.limiter_threshold, MIN_LIMITER_THRESHOLD);

        shared.set_band_gain(0, 100.0);
        shared.set_preamp_db(-100.0);
        shared.set_limiter(true, f32::NAN);
        let snapshot = shared.snapshot();
        assert_eq!(snapshot.band_gains_db[0], MAX_GAIN_DB);
        assert_eq!(snapshot.preamp_db, MIN_PREAMP_DB);
        assert_eq!(snapshot.limiter_threshold, DEFAULT_LIMITER_THRESHOLD);
        assert!(!shared.set_band_gain(BAND_COUNT, 1.0));

        let mut eq = StereoEq::new(f32::NAN);
        eq.apply_params_immediate(&snapshot);
        for input in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30] {
            let output = eq.process_frame(input, -input);
            assert!(output.left.is_finite());
            assert!(output.right.is_finite());
            assert!(output.metrics.input_peak.is_finite());
            assert!(output.metrics.output_peak <= snapshot.limiter_threshold + f32::EPSILON);
        }
    }

    #[test]
    fn channels_have_independent_filter_state_and_linked_limiting() {
        let mut params = ParamSnapshot::default();
        params.band_gains_db[4] = 12.0;
        params.limiter_enabled = false;
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.apply_params_immediate(&params);

        for i in 0..2_000 {
            let left = if i == 0 { 0.5 } else { 0.0 };
            let output = eq.process_frame(left, 0.0);
            assert_eq!(output.right, 0.0);
        }

        params = ParamSnapshot::default();
        params.limiter_threshold = 0.5;
        eq.reset();
        eq.apply_params_immediate(&params);
        let output = eq.process_frame(2.0, 1.0);
        assert!(output.metrics.limited);
        assert!((output.left - 0.5).abs() < 1.0e-5);
        assert!((output.right - 0.25).abs() < 1.0e-5);
    }

    #[test]
    fn disabled_band_smoothly_returns_to_zero_db() {
        let mut params = ParamSnapshot::default();
        params.band_gains_db[3] = 12.0;
        params.limiter_enabled = false;
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.apply_params_immediate(&params);
        params.band_enabled[3] = false;
        eq.apply_params(&params);
        assert_eq!(eq.target_band_gains_db[3], 0.0);
        assert_eq!(eq.band_gains_db[3], 12.0);
        for _ in 0..8_000 {
            eq.process_frame(0.0, 0.0);
        }
        assert_eq!(eq.band_gains_db[3], 0.0);
    }

    #[test]
    fn block_metrics_count_clipping_and_limiting() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        let params = ParamSnapshot::default();
        let mut samples = [0.1, -0.2, 2.0, 1.0, f32::NAN, f32::INFINITY];
        let metrics = eq.process_interleaved(&mut samples, &params);
        assert_eq!(metrics.clipped_frames, 1);
        assert_eq!(metrics.limited_frames, 1);
        assert!(metrics.clipped);
        assert!(metrics.limited);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert!(
            samples
                .iter()
                .all(|sample| sample.abs() <= 0.98 + f32::EPSILON)
        );
    }

    #[test]
    fn atomic_updates_are_coherent() {
        let shared = Arc::new(SharedParams::default());
        let writer_params = Arc::clone(&shared);
        let writer = thread::spawn(move || {
            for generation in 1..=20_000_u32 {
                let value = (generation % 17) as f32;
                writer_params.update(|params| {
                    params.band_gains_db = [value; BAND_COUNT];
                    params.preamp_db = value.min(MAX_PREAMP_DB);
                    params.bypass = generation & 1 != 0;
                });
            }
        });

        while !writer.is_finished() {
            let snapshot = shared.snapshot();
            assert!(
                snapshot
                    .band_gains_db
                    .iter()
                    .all(|gain| *gain == snapshot.band_gains_db[0])
            );
            assert_eq!(
                snapshot.preamp_db,
                snapshot.band_gains_db[0].min(MAX_PREAMP_DB)
            );
        }
        writer.join().unwrap();
    }

    #[test]
    fn update_return_value_and_replacement_work() {
        let shared = SharedParams::default();
        let result = shared.update(|params| {
            params.bypass = true;
            42
        });
        assert_eq!(result, 42);
        assert!(shared.snapshot().bypass);

        let replacement = ParamSnapshot {
            preamp_db: -6.0,
            ..ParamSnapshot::default()
        };
        shared.replace(replacement);
        assert_eq!(shared.snapshot(), replacement);
    }
}
