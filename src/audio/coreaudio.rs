//! macOS backend: a muted global Core Audio process tap feeds a private
//! aggregate device whose main sub-device is the selected output. One IOProc
//! reads the tap, runs the equalizer, and writes to the real output, so no
//! virtual driver is needed. Requires macOS 14.2 or newer and the "System
//! Audio Recording" permission for the hosting terminal.

use std::ffi::{CStr, c_void};
use std::fs;
use std::mem::size_of;
use std::process::Command;
use std::ptr::{self, NonNull};
use std::slice;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertyScope, AudioObjectPropertySelector,
    CATapDescription, CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceIsStackedKey, kAudioAggregateDeviceMainSubDeviceKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceSubDeviceListKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyBufferFrameSize,
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyStreams, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
    kAudioSubDeviceUIDKey, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSMutableDictionary, NSNumber, NSString};
use rtrb::{Producer, RingBuffer};

use super::OutputDevice;
use crate::analyzer;
use crate::dsp::{BlockMetrics, ParamSnapshot, SharedParams, StereoEq};
use crate::telemetry::Telemetry;

const SYSTEM_OBJECT: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
const AGGREGATE_UID_PREFIX: &str = "com.torro.torroeq.aggregate";
const ANALYZER_RING_SAMPLES: usize = 32_768;

pub fn discover_outputs() -> Result<Vec<OutputDevice>> {
    let default_id: AudioObjectID = get_value(
        SYSTEM_OBJECT,
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
    )
    .unwrap_or(0);
    let devices: Vec<AudioObjectID> = get_array(
        SYSTEM_OBJECT,
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    )
    .context("could not list Core Audio devices")?;
    let mut outputs = Vec::new();
    for id in devices {
        if stream_count(id, kAudioObjectPropertyScopeOutput) == 0 {
            continue;
        }
        let Ok(uid) = get_string(id, kAudioDevicePropertyDeviceUID) else {
            continue;
        };
        if uid.starts_with(AGGREGATE_UID_PREFIX) {
            continue;
        }
        let description = get_string(id, kAudioObjectPropertyName).unwrap_or_else(|_| uid.clone());
        outputs.push(OutputDevice {
            id,
            name: uid,
            description,
            is_default: id == default_id,
        });
    }
    outputs.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.description.cmp(&right.description))
    });
    Ok(outputs)
}

pub fn check_audio() -> Result<()> {
    let output = discover_outputs()?
        .into_iter()
        .next()
        .context("no Core Audio output found")?;
    let params = Arc::new(SharedParams::default());
    let telemetry = Arc::new(Telemetry::default());
    println!("Starting TorroEQ on {}...", output.description);
    let mut engine = AudioEngine::start(output, params, Arc::clone(&telemetry))?;
    engine.activate()?;

    let tone = std::env::temp_dir().join(format!("torroeq-check-{}.wav", std::process::id()));
    fs::write(&tone, test_tone_wav(44_100, 0.6)).context("could not write test tone")?;
    let mut player = Command::new("afplay")
        .arg(&tone)
        .spawn()
        .context("could not start afplay")?;
    // The aggregate device and afplay both take a moment to start, so keep
    // measuring until the tone has finished and the last buffers arrived.
    let mut input_peak = 0.0_f32;
    let mut finished: Option<Instant> = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && finished.is_none_or(|at| at.elapsed() < Duration::from_millis(300))
    {
        input_peak = input_peak.max(telemetry.snapshot().input_peak);
        if finished.is_none() && player.try_wait()?.is_some() {
            finished = Some(Instant::now());
        }
        thread::sleep(Duration::from_millis(5));
    }
    let _ = player.kill();
    let _ = player.wait();
    let _ = fs::remove_file(&tone);
    let running = telemetry.snapshot().running;
    engine.deactivate()?;
    engine.stop();

    if !running || input_peak < 0.05 {
        bail!(
            "audio check failed (running={running}, input_peak={input_peak:.3}). If the peak is \
             zero, allow your terminal under System Settings > Privacy & Security > Screen & \
             System Audio Recording."
        );
    }
    println!("Audio engine healthy; system audio tapped, equalized, and released cleanly.");
    Ok(())
}

pub struct AudioEngine {
    output: OutputDevice,
    params: Arc<SharedParams>,
    telemetry: Arc<Telemetry>,
    route: Option<Route>,
}

impl AudioEngine {
    pub fn start(
        output: OutputDevice,
        params: Arc<SharedParams>,
        telemetry: Arc<Telemetry>,
    ) -> Result<Self> {
        let rate: f64 = get_value(
            output.id,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )
        .with_context(|| format!("output {} is not available", output.description))?;
        telemetry.set_sample_rate(rate.round() as u32);
        Ok(Self {
            output,
            params,
            telemetry,
            route: None,
        })
    }

    /// Taps all system audio (muting its direct path) and plays it through
    /// the equalizer on the selected output.
    pub fn activate(&mut self) -> Result<()> {
        if self.route.is_none() {
            self.route = Some(Route::open(
                &self.output,
                Arc::clone(&self.params),
                Arc::clone(&self.telemetry),
            )?);
        }
        Ok(())
    }

    /// Removes the tap; applications play directly to their devices again.
    pub fn deactivate(&mut self) -> Result<()> {
        self.route = None;
        Ok(())
    }

    pub fn is_activated(&self) -> bool {
        self.route.is_some()
    }

    pub fn stop(&mut self) {
        self.route = None;
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

/// An active tap, aggregate device, and IOProc. Dropping it tears them down
/// in reverse order, including after a partially failed `open`.
struct Route {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    started: bool,
    state: *mut ProcState,
    telemetry: Arc<Telemetry>,
    analyzer_stop: Arc<AtomicBool>,
    analyzer_thread: Option<JoinHandle<()>>,
}

impl Route {
    fn open(
        output: &OutputDevice,
        params: Arc<SharedParams>,
        telemetry: Arc<Telemetry>,
    ) -> Result<Self> {
        let mut route = Self {
            tap: 0,
            aggregate: 0,
            io_proc: None,
            started: false,
            state: ptr::null_mut(),
            telemetry: Arc::clone(&telemetry),
            analyzer_stop: Arc::new(AtomicBool::new(false)),
            analyzer_thread: None,
        };

        // Exclude TorroEQ itself, or its own output would be tapped again.
        let pid = std::process::id() as i32;
        let own_process: AudioObjectID = get_qualified_value(
            SYSTEM_OBJECT,
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            &pid,
        )
        .context("could not identify TorroEQ's Core Audio process object")?;
        if own_process == 0 {
            bail!("Core Audio does not know TorroEQ's process yet");
        }
        let excluded = NSArray::from_retained_slice(&[NSNumber::new_u32(own_process)]);
        let description = unsafe {
            CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excluded,
            )
        };
        unsafe {
            description.setName(&NSString::from_str("TorroEQ"));
            description.setPrivate(true);
            description.setMuteBehavior(CATapMuteBehavior::MutedWhenTapped);
        }
        let tap_uid = unsafe { description.UUID().UUIDString() };
        check(
            unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut route.tap) },
            "could not create the system audio tap (macOS 14.2 or newer is required)",
        )?;

        let aggregate_uid = format!("{AGGREGATE_UID_PREFIX}.{tap_uid}");
        let sub_device = dictionary(&[(kAudioSubDeviceUIDKey, &*NSString::from_str(&output.name))]);
        let sub_tap = dictionary(&[
            (kAudioSubTapUIDKey, &*tap_uid as &AnyObject),
            (kAudioSubTapDriftCompensationKey, &*NSNumber::new_bool(true)),
        ]);
        let aggregate = dictionary(&[
            (
                kAudioAggregateDeviceNameKey,
                &*NSString::from_str("TorroEQ") as &AnyObject,
            ),
            (
                kAudioAggregateDeviceUIDKey,
                &*NSString::from_str(&aggregate_uid),
            ),
            (
                kAudioAggregateDeviceMainSubDeviceKey,
                &*NSString::from_str(&output.name),
            ),
            (
                kAudioAggregateDeviceIsPrivateKey,
                &*NSNumber::new_bool(true),
            ),
            (
                kAudioAggregateDeviceIsStackedKey,
                &*NSNumber::new_bool(false),
            ),
            (
                kAudioAggregateDeviceTapAutoStartKey,
                &*NSNumber::new_bool(true),
            ),
            (
                kAudioAggregateDeviceSubDeviceListKey,
                &*NSArray::from_retained_slice(&[sub_device]),
            ),
            (
                kAudioAggregateDeviceTapListKey,
                &*NSArray::from_retained_slice(&[sub_tap]),
            ),
        ]);
        let aggregate_ref = unsafe { &*(Retained::as_ptr(&aggregate).cast::<CFDictionary>()) };
        check(
            unsafe {
                AudioHardwareCreateAggregateDevice(
                    aggregate_ref,
                    NonNull::from(&mut route.aggregate),
                )
            },
            "could not create the TorroEQ aggregate device",
        )?;

        let rate: f64 = get_value(
            route.aggregate,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )?;
        let buffer_frames: u32 = get_value(
            route.aggregate,
            kAudioDevicePropertyBufferFrameSize,
            kAudioObjectPropertyScopeGlobal,
        )
        .unwrap_or(512);
        let output_latency = get_value::<u32>(
            output.id,
            kAudioDevicePropertyLatency,
            kAudioObjectPropertyScopeOutput,
        )
        .unwrap_or(0)
            + get_value::<u32>(
                output.id,
                kAudioDevicePropertySafetyOffset,
                kAudioObjectPropertyScopeOutput,
            )
            .unwrap_or(0);

        let (producer, consumer) = RingBuffer::new(ANALYZER_RING_SAMPLES);
        route.analyzer_thread = Some(analyzer::spawn(
            consumer,
            Arc::clone(&telemetry),
            rate as f32,
            Arc::clone(&route.analyzer_stop),
        ));
        route.state = Box::into_raw(Box::new(ProcState {
            eq: StereoEq::new(rate as f32),
            params,
            analyzer: producer,
            telemetry: Arc::clone(&telemetry),
            last_params: ParamSnapshot::default(),
            // Aggregate inputs list the sub-device's own input streams first,
            // then the tap.
            tap_buffer: stream_count(output.id, kAudioObjectPropertyScopeInput),
        }));
        check(
            unsafe {
                AudioDeviceCreateIOProcID(
                    route.aggregate,
                    Some(io_proc),
                    route.state.cast(),
                    NonNull::from(&mut route.io_proc),
                )
            },
            "could not register the TorroEQ audio callback",
        )?;
        check(
            unsafe { AudioDeviceStart(route.aggregate, route.io_proc) },
            "could not start the TorroEQ aggregate device",
        )?;
        route.started = true;

        telemetry.set_sample_rate(rate.round() as u32);
        telemetry.set_latency_ms((buffer_frames + output_latency) as f32 * 1000.0 / rate as f32);
        telemetry.set_running(true);
        Ok(route)
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        self.telemetry.set_running(false);
        unsafe {
            if self.started {
                AudioDeviceStop(self.aggregate, self.io_proc);
            }
            if self.io_proc.is_some() {
                AudioDeviceDestroyIOProcID(self.aggregate, self.io_proc);
            }
            // The IOProc is stopped and destroyed, so nothing else touches it.
            if !self.state.is_null() {
                drop(Box::from_raw(self.state));
            }
            if self.aggregate != 0 {
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != 0 {
                AudioHardwareDestroyProcessTap(self.tap);
            }
        }
        self.analyzer_stop.store(true, Ordering::Release);
        if let Some(thread) = self.analyzer_thread.take() {
            let _ = thread.join();
        }
    }
}

struct ProcState {
    eq: StereoEq,
    params: Arc<SharedParams>,
    analyzer: Producer<f32>,
    telemetry: Arc<Telemetry>,
    last_params: ParamSnapshot,
    tap_buffer: usize,
}

/// A channel inside an interleaved Core Audio buffer.
#[derive(Clone, Copy)]
struct Channel {
    data: *mut f32,
    stride: usize,
    frames: usize,
}

impl Channel {
    fn find(buffers: &[AudioBuffer], first: usize, index: usize) -> Option<Self> {
        let mut remaining = index;
        for buffer in buffers.get(first..)? {
            let channels = buffer.mNumberChannels as usize;
            if remaining < channels && !buffer.mData.is_null() {
                return Some(Self {
                    data: buffer.mData.cast::<f32>().wrapping_add(remaining),
                    stride: channels,
                    frames: buffer.mDataByteSize as usize / (channels * size_of::<f32>()),
                });
            }
            remaining = remaining.saturating_sub(channels);
        }
        None
    }
}

unsafe fn buffers<'a>(list: NonNull<AudioBufferList>) -> &'a mut [AudioBuffer] {
    // SAFETY: Core Audio passes a list with mNumberBuffers trailing entries.
    unsafe {
        let list = list.as_ptr();
        slice::from_raw_parts_mut(
            (*list).mBuffers.as_mut_ptr(),
            (*list).mNumberBuffers as usize,
        )
    }
}

unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    if client.is_null() {
        return 0;
    }
    // SAFETY: `client` is the ProcState owned by the Route, which outlives
    // the IOProc registration.
    let state = unsafe { &mut *client.cast::<ProcState>() };
    let inputs = unsafe { buffers(input) };
    let outputs = unsafe { buffers(output) };
    for buffer in outputs.iter_mut() {
        if !buffer.mData.is_null() {
            unsafe {
                ptr::write_bytes(buffer.mData.cast::<u8>(), 0, buffer.mDataByteSize as usize)
            };
        }
    }

    let tap = state.tap_buffer.min(inputs.len().saturating_sub(1));
    let Some(in_left) = Channel::find(inputs, tap, 0) else {
        return 0;
    };
    let in_right = Channel::find(inputs, tap, 1).unwrap_or(in_left);
    let Some(out_left) = Channel::find(outputs, 0, 0) else {
        return 0;
    };
    let out_right = Channel::find(outputs, 0, 1);
    let frames = in_left
        .frames
        .min(in_right.frames)
        .min(out_left.frames)
        .min(out_right.map_or(usize::MAX, |channel| channel.frames));

    let params = state.params.try_snapshot(state.last_params);
    state.last_params = params;
    state.eq.apply_params(&params);
    let mut metrics = BlockMetrics::default();
    for index in 0..frames {
        let left = unsafe { *in_left.data.add(index * in_left.stride) };
        let right = unsafe { *in_right.data.add(index * in_right.stride) };
        let processed = state.eq.process_frame(left, right);
        unsafe {
            match out_right {
                Some(out_right) => {
                    *out_left.data.add(index * out_left.stride) = processed.left;
                    *out_right.data.add(index * out_right.stride) = processed.right;
                }
                None => {
                    *out_left.data.add(index * out_left.stride) =
                        (processed.left + processed.right) * 0.5;
                }
            }
        }
        metrics.add_frame(processed.metrics);
        let _ = state
            .analyzer
            .push((processed.left + processed.right) * 0.5);
    }
    state.telemetry.publish_audio(
        metrics.input_peak,
        metrics.output_peak,
        metrics.clipped,
        metrics.limited,
    );
    0
}

fn check(status: i32, message: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(anyhow!("{message} ({})", four_char_code(status)))
    }
}

/// Renders an OSStatus as its four-character code when it is one.
fn four_char_code(status: i32) -> String {
    let bytes = status.to_be_bytes();
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        format!("'{}'", String::from_utf8_lossy(&bytes))
    } else {
        status.to_string()
    }
}

fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn get_value<T: Copy + Default>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<T> {
    get_raw(object, address(selector, scope), 0, ptr::null())
}

fn get_qualified_value<T: Copy + Default, Q>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    qualifier: &Q,
) -> Result<T> {
    get_raw(
        object,
        address(selector, kAudioObjectPropertyScopeGlobal),
        size_of::<Q>() as u32,
        (qualifier as *const Q).cast(),
    )
}

fn get_raw<T: Copy + Default>(
    object: AudioObjectID,
    address: AudioObjectPropertyAddress,
    qualifier_size: u32,
    qualifier: *const c_void,
) -> Result<T> {
    let mut value = T::default();
    let mut size = size_of::<T>() as u32;
    check(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                qualifier_size,
                qualifier,
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        },
        "could not read Core Audio property",
    )?;
    Ok(value)
}

fn get_array<T: Copy + Default>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<T>> {
    let address = address(selector, scope);
    let mut size = 0_u32;
    check(
        unsafe {
            AudioObjectGetPropertyDataSize(
                object,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut size),
            )
        },
        "could not size Core Audio property",
    )?;
    let mut values = vec![T::default(); size as usize / size_of::<T>()];
    if values.is_empty() {
        return Ok(values);
    }
    check(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(values.as_mut_ptr()).cast(),
            )
        },
        "could not read Core Audio property",
    )?;
    values.truncate(size as usize / size_of::<T>());
    Ok(values)
}

fn get_string(object: AudioObjectID, selector: AudioObjectPropertySelector) -> Result<String> {
    let raw: usize = get_value(object, selector, kAudioObjectPropertyScopeGlobal)?;
    // SAFETY: string properties return a +1 CFStringRef, toll-free bridged
    // to NSString.
    let string = unsafe { Retained::from_raw(raw as *mut NSString) }
        .context("Core Audio returned an empty string")?;
    Ok(string.to_string())
}

fn stream_count(device: AudioObjectID, scope: AudioObjectPropertyScope) -> usize {
    get_array::<AudioObjectID>(device, kAudioDevicePropertyStreams, scope)
        .map_or(0, |streams| streams.len())
}

fn dictionary(
    entries: &[(&CStr, &AnyObject)],
) -> Retained<NSMutableDictionary<NSString, AnyObject>> {
    let dictionary = NSMutableDictionary::new();
    for (key, value) in entries {
        dictionary.insert(&*NSString::from_str(&key.to_string_lossy()), *value);
    }
    dictionary
}

/// A short, quiet 440 Hz stereo tone as a 32-bit float WAV file.
fn test_tone_wav(rate: u32, seconds: f32) -> Vec<u8> {
    let frames = (rate as f32 * seconds) as u32;
    let data_size = frames * 2 * 4;
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&3_u16.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 8).to_le_bytes());
    wav.extend_from_slice(&8_u16.to_le_bytes());
    wav.extend_from_slice(&32_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for frame in 0..frames {
        let sample = (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / rate as f32).sin() * 0.1;
        wav.extend_from_slice(&sample.to_le_bytes());
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}
