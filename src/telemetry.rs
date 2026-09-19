use std::array;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const SPECTRUM_BINS: usize = 10;

pub struct Telemetry {
    input_peak: AtomicU32,
    output_peak: AtomicU32,
    latency_ms: AtomicU32,
    clipped: AtomicBool,
    limited: AtomicBool,
    running: AtomicBool,
    spectrum: [AtomicU32; SPECTRUM_BINS],
    spectrum_peaks: [AtomicU32; SPECTRUM_BINS],
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            input_peak: AtomicU32::new(0.0_f32.to_bits()),
            output_peak: AtomicU32::new(0.0_f32.to_bits()),
            latency_ms: AtomicU32::new(0.0_f32.to_bits()),
            clipped: AtomicBool::new(false),
            limited: AtomicBool::new(false),
            running: AtomicBool::new(false),
            spectrum: array::from_fn(|_| AtomicU32::new((-72.0_f32).to_bits())),
            spectrum_peaks: array::from_fn(|_| AtomicU32::new((-72.0_f32).to_bits())),
        }
    }
}

impl Telemetry {
    pub fn publish_audio(&self, input_peak: f32, output_peak: f32, clipped: bool, limited: bool) {
        self.input_peak
            .store(input_peak.to_bits(), Ordering::Relaxed);
        self.output_peak
            .store(output_peak.to_bits(), Ordering::Relaxed);
        self.clipped.store(clipped, Ordering::Relaxed);
        self.limited.store(limited, Ordering::Relaxed);
    }

    pub fn publish_spectrum(&self, bins: &[f32; SPECTRUM_BINS], peaks: &[f32; SPECTRUM_BINS]) {
        for (target, value) in self.spectrum.iter().zip(bins) {
            target.store(value.to_bits(), Ordering::Relaxed);
        }
        for (target, value) in self.spectrum_peaks.iter().zip(peaks) {
            target.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Release);
    }

    pub fn set_latency_ms(&self, latency_ms: f32) {
        self.latency_ms
            .store(latency_ms.to_bits(), Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot {
            input_peak: f32::from_bits(self.input_peak.load(Ordering::Relaxed)),
            output_peak: f32::from_bits(self.output_peak.load(Ordering::Relaxed)),
            latency_ms: f32::from_bits(self.latency_ms.load(Ordering::Relaxed)),
            clipped: self.clipped.swap(false, Ordering::Relaxed),
            limited: self.limited.load(Ordering::Relaxed),
            running: self.running.load(Ordering::Acquire),
            spectrum: array::from_fn(|i| f32::from_bits(self.spectrum[i].load(Ordering::Relaxed))),
            spectrum_peaks: array::from_fn(|i| {
                f32::from_bits(self.spectrum_peaks[i].load(Ordering::Relaxed))
            }),
        }
    }
}

#[derive(Clone)]
pub struct TelemetrySnapshot {
    pub input_peak: f32,
    pub output_peak: f32,
    pub latency_ms: f32,
    pub clipped: bool,
    pub limited: bool,
    pub running: bool,
    pub spectrum: [f32; SPECTRUM_BINS],
    pub spectrum_peaks: [f32; SPECTRUM_BINS],
}
