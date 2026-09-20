use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use rtrb::Consumer;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;

use crate::dsp::{BAND_COUNT, BAND_FREQUENCIES};
use crate::telemetry::{SPECTRUM_BINS, Telemetry};

const FFT_SIZE: usize = 2048;
const FLOOR_DB: f32 = -72.0;
const PEAK_HOLD_FRAMES: u8 = 47;
const PEAK_DECAY_DB: f32 = 0.3;
const _: () = assert!(FFT_SIZE >= 2048);
const _: () = assert!(SPECTRUM_BINS == BAND_COUNT);

pub fn spawn(
    mut samples: Consumer<f32>,
    telemetry: Arc<Telemetry>,
    sample_rate: f32,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("torroeq-analyzer".into())
        .spawn(move || {
            let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
            let window: Vec<f32> = (0..FFT_SIZE)
                .map(|i| {
                    0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos()
                })
                .collect();
            let window_sum: f32 = window.iter().sum();
            let mut input = vec![0.0_f32; FFT_SIZE];
            let mut fft_buffer = vec![Complex32::new(0.0, 0.0); FFT_SIZE];
            let mut smoothed = [FLOOR_DB; SPECTRUM_BINS];
            let mut peaks = [FLOOR_DB; SPECTRUM_BINS];
            let mut peak_holds = [0_u8; SPECTRUM_BINS];

            let mut received = 0;
            while received < FFT_SIZE && !stop.load(Ordering::Relaxed) {
                match samples.pop() {
                    Ok(sample) => {
                        input[received] = sample;
                        received += 1;
                    }
                    Err(_) => thread::sleep(std::time::Duration::from_millis(1)),
                }
            }

            while !stop.load(Ordering::Relaxed) {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                for i in 0..FFT_SIZE {
                    fft_buffer[i] = Complex32::new(input[i] * window[i], 0.0);
                }
                fft.process(&mut fft_buffer);

                for (column, value) in smoothed.iter_mut().enumerate() {
                    let start_hz = if column == 0 {
                        20.0
                    } else {
                        (BAND_FREQUENCIES[column - 1] * BAND_FREQUENCIES[column]).sqrt()
                    };
                    let end_hz = if column + 1 == SPECTRUM_BINS {
                        (sample_rate * 0.5).min(20_000.0)
                    } else {
                        (BAND_FREQUENCIES[column] * BAND_FREQUENCIES[column + 1]).sqrt()
                    };
                    let start = ((start_hz * FFT_SIZE as f32 / sample_rate).ceil() as usize)
                        .clamp(1, FFT_SIZE / 2);
                    let end = ((end_hz * FFT_SIZE as f32 / sample_rate).ceil() as usize)
                        .clamp(start + 1, FFT_SIZE / 2 + 1);
                    let magnitude = fft_buffer[start..end]
                        .iter()
                        .map(Complex32::norm_sqr)
                        .sum::<f32>()
                        .sqrt()
                        * 2.0
                        / window_sum;
                    let db = (20.0 * magnitude.max(1.0e-9).log10()).clamp(FLOOR_DB, 0.0);
                    let factor = if db > *value { 0.82 } else { 0.28 };
                    *value += (db - *value) * factor;

                    if *value >= peaks[column] {
                        peaks[column] = *value;
                        peak_holds[column] = PEAK_HOLD_FRAMES;
                    } else if peak_holds[column] > 0 {
                        peak_holds[column] -= 1;
                    } else {
                        peaks[column] = (peaks[column] - PEAK_DECAY_DB).max(*value);
                    }
                }
                telemetry.publish_spectrum(&smoothed, &peaks);

                input.copy_within(FFT_SIZE / 2.., 0);
                let mut overlap = FFT_SIZE / 2;
                while overlap < FFT_SIZE && !stop.load(Ordering::Relaxed) {
                    match samples.pop() {
                        Ok(sample) => {
                            input[overlap] = sample;
                            overlap += 1;
                        }
                        Err(_) => thread::sleep(std::time::Duration::from_millis(1)),
                    }
                }
            }
        })
        .expect("failed to start analyzer thread")
}
