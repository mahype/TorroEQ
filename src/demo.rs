//! A synthetic signal for `--demo`: music-like noise, a kick, and a few tones,
//! processed by the real equalizer and analyzer without touching any audio device.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rtrb::RingBuffer;

use crate::analyzer;
use crate::dsp::{BlockMetrics, SharedParams, StereoEq};
use crate::telemetry::Telemetry;

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK_FRAMES: usize = 480;
const ANALYZER_RING_SAMPLES: usize = 32_768;

pub struct DemoSignal {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl DemoSignal {
    pub fn start(params: Arc<SharedParams>, telemetry: Arc<Telemetry>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (mut producer, consumer) = RingBuffer::new(ANALYZER_RING_SAMPLES);
        let analyzer_thread = analyzer::spawn(
            consumer,
            Arc::clone(&telemetry),
            SAMPLE_RATE,
            Arc::clone(&stop),
        );
        telemetry.set_sample_rate(SAMPLE_RATE as u32);
        telemetry.set_latency_ms(BLOCK_FRAMES as f32 * 1000.0 / SAMPLE_RATE);
        telemetry.set_running(true);

        let generator_stop = Arc::clone(&stop);
        let generator = thread::Builder::new()
            .name("torroeq-demo".into())
            .spawn(move || {
                let mut eq = StereoEq::new(SAMPLE_RATE);
                let mut source = Source::default();
                let block = Duration::from_secs_f32(BLOCK_FRAMES as f32 / SAMPLE_RATE);
                let mut deadline = Instant::now();
                while !generator_stop.load(Ordering::Relaxed) {
                    eq.apply_params(&params.snapshot());
                    let mut metrics = BlockMetrics::default();
                    for _ in 0..BLOCK_FRAMES {
                        let (left, right) = source.next_frame();
                        let processed = eq.process_frame(left, right);
                        metrics.add_frame(processed.metrics);
                        let _ = producer.push((processed.left + processed.right) * 0.5);
                    }
                    telemetry.publish_audio(
                        metrics.input_peak,
                        metrics.output_peak,
                        metrics.clipped,
                        metrics.limited,
                    );
                    deadline += block;
                    if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                        thread::sleep(wait);
                    }
                }
                telemetry.set_running(false);
            })
            .expect("failed to start demo thread");

        Self {
            stop,
            threads: vec![generator, analyzer_thread],
        }
    }
}

impl Drop for DemoSignal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Pink noise with a slow swell, a four-on-the-floor kick, hi-hats, and a bass and lead line.
#[derive(Default)]
struct Source {
    frame: u64,
    rng: u32,
    pink: [f32; 7],
    kick_phase: f32,
    bass_phase: f32,
    lead_phase: f32,
}

impl Source {
    fn next_frame(&mut self) -> (f32, f32) {
        use std::f32::consts::TAU;

        let t = self.frame as f32 / SAMPLE_RATE;
        self.frame += 1;

        let beat = t * 2.0;
        let in_beat = beat.fract() / 2.0;
        let kick_frequency = 48.0 + 90.0 * (-in_beat * 30.0).exp();
        self.kick_phase = (self.kick_phase + kick_frequency / SAMPLE_RATE).fract();
        let kick = (self.kick_phase * TAU).sin() * (-in_beat * 9.0).exp() * 0.45;

        const BASS: [f32; 4] = [55.0, 55.0, 73.4, 65.4];
        let bass_frequency = BASS[(beat as usize / 2) % BASS.len()];
        self.bass_phase = (self.bass_phase + bass_frequency / SAMPLE_RATE).fract();
        let bass = (self.bass_phase * TAU).sin() * 0.18;

        const LEAD: [f32; 8] = [659.3, 784.0, 880.0, 784.0, 587.3, 659.3, 523.3, 587.3];
        let lead_frequency = LEAD[(beat as usize) % LEAD.len()];
        self.lead_phase = (self.lead_phase + lead_frequency / SAMPLE_RATE).fract();
        let lead_envelope = (-(beat.fract()) * 3.0).exp();
        let lead = ((self.lead_phase * TAU).sin() + 0.3 * (self.lead_phase * 2.0 * TAU).sin())
            * lead_envelope
            * 0.07;

        let swell = 0.6 + 0.4 * (t * 0.35).sin();
        let noise = self.next_pink() * 0.09 * swell;

        let eighth = (beat * 2.0).fract();
        let hat = self.next_white() * (-eighth * 25.0).exp() * 0.12;

        let mix = kick + bass + lead + noise + hat;
        (mix, mix * 0.96 + noise * 0.04)
    }

    fn next_white(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = 0x9E37_79B9;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32 * 2.0 - 1.0
    }

    /// Paul Kellet's refined pink-noise filter.
    fn next_pink(&mut self) -> f32 {
        let white = self.next_white();
        let b = &mut self.pink;
        b[0] = 0.99886 * b[0] + white * 0.0555179;
        b[1] = 0.99332 * b[1] + white * 0.0750759;
        b[2] = 0.96900 * b[2] + white * 0.153852;
        b[3] = 0.86650 * b[3] + white * 0.3104856;
        b[4] = 0.55000 * b[4] + white * 0.5329522;
        b[5] = -0.7616 * b[5] - white * 0.0168980;
        let pink = b.iter().sum::<f32>() + white * 0.5362;
        b[6] = white * 0.115926;
        pink * 0.11
    }
}
