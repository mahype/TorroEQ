use std::mem;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::Pod;
use rtrb::{Consumer, Producer, RingBuffer};
use serde::Deserialize;

use crate::analyzer;
use crate::dsp::{SharedParams, StereoEq};
use crate::telemetry::Telemetry;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
const AUDIO_RING_SAMPLES: usize = 65_536;
const ANALYZER_RING_SAMPLES: usize = 32_768;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

#[derive(Deserialize)]
struct DumpObject {
    id: u32,
    #[serde(rename = "type")]
    kind: String,
    info: Option<DumpInfo>,
}

#[derive(Deserialize)]
struct DumpInfo {
    props: Option<serde_json::Map<String, serde_json::Value>>,
}

pub fn discover_outputs() -> Result<Vec<OutputDevice>> {
    let default_name = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_string());
    let output = Command::new("pw-dump")
        .output()
        .context("could not run pw-dump")?;
    if !output.status.success() {
        bail!("pw-dump failed with {}", output.status);
    }
    let objects: Vec<DumpObject> = serde_json::from_slice(&output.stdout)?;
    let mut outputs = Vec::new();
    for object in objects {
        if object.kind != "PipeWire:Interface:Node" {
            continue;
        }
        let Some(props) = object.info.and_then(|info| info.props) else {
            continue;
        };
        if string_prop(&props, "media.class") != Some("Audio/Sink") {
            continue;
        }
        let Some(name) = string_prop(&props, "node.name") else {
            continue;
        };
        if name == "torroeq_sink" {
            continue;
        }
        outputs.push(OutputDevice {
            id: object.id,
            name: name.to_string(),
            description: string_prop(&props, "node.description")
                .unwrap_or(name)
                .to_string(),
            is_default: default_name.as_deref() == Some(name),
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

fn string_prop<'a>(
    props: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    props.get(key)?.as_str()
}

pub struct AudioEngine {
    stop_sender: pw::channel::Sender<()>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    previous_default: Option<String>,
    activated: bool,
}

impl AudioEngine {
    pub fn start(
        output: OutputDevice,
        params: Arc<SharedParams>,
        telemetry: Arc<Telemetry>,
    ) -> Result<Self> {
        let previous_default = default_sink_name()
            .filter(|name| name != "torroeq_sink")
            .or_else(|| Some(output.name.clone()));
        let (stop_sender, stop_receiver) = pw::channel::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);

        let thread = thread::Builder::new()
            .name("torroeq-pipewire".into())
            .spawn(move || {
                let error_sender = ready_sender.clone();
                let result = run_pipewire(
                    &output,
                    params,
                    Arc::clone(&telemetry),
                    stop_receiver,
                    Arc::clone(&thread_stop),
                    ready_sender,
                );
                telemetry.set_running(false);
                if let Err(error) = result {
                    let _ = error_sender.send(Err(format!("{error:#}")));
                    eprintln!("TorroEQ audio engine: {error:#}");
                }
            })?;

        match ready_receiver.recv()? {
            Ok(()) => {
                let activated = default_sink_name().as_deref() == Some("torroeq_sink");
                Ok(Self {
                    stop_sender,
                    stop,
                    thread: Some(thread),
                    previous_default,
                    activated,
                })
            }
            Err(message) => {
                let _ = thread.join();
                Err(anyhow!(message))
            }
        }
    }

    pub fn activate(&mut self) -> Result<()> {
        let status = Command::new("pactl")
            .args(["set-default-sink", "torroeq_sink"])
            .status()
            .context("could not activate TorroEQ sink")?;
        if !status.success() {
            bail!("pactl could not activate TorroEQ sink");
        }
        self.activated = true;
        Ok(())
    }

    pub fn deactivate(&mut self) -> Result<()> {
        if let Some(previous) = &self.previous_default {
            let status = Command::new("pactl")
                .args(["set-default-sink", previous])
                .status()
                .context("could not restore previous audio output")?;
            if !status.success() {
                bail!("pactl could not restore previous audio output");
            }
        }
        self.activated = false;
        Ok(())
    }

    pub fn is_activated(&self) -> bool {
        self.activated
    }

    pub fn stop(&mut self) {
        if self.activated {
            let _ = self.deactivate();
        }
        self.stop.store(true, Ordering::Release);
        let _ = self.stop_sender.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn default_sink_name() -> Option<String> {
    Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_string())
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

struct CaptureData {
    format: spa::param::audio::AudioInfoRaw,
    eq: StereoEq,
    params: Arc<SharedParams>,
    audio: Producer<f32>,
    analyzer: Producer<f32>,
    telemetry: Arc<Telemetry>,
    last_params: crate::dsp::ParamSnapshot,
}

struct PlaybackData {
    audio: Consumer<f32>,
    capture: *mut pw::sys::pw_stream,
}

fn run_pipewire(
    output: &OutputDevice,
    params: Arc<SharedParams>,
    telemetry: Arc<Telemetry>,
    stop_receiver: pw::channel::Receiver<()>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let (audio_producer, audio_consumer) = RingBuffer::new(AUDIO_RING_SAMPLES);
    let (analyzer_producer, analyzer_consumer) = RingBuffer::new(ANALYZER_RING_SAMPLES);
    let analyzer_thread = analyzer::spawn(
        analyzer_consumer,
        Arc::clone(&telemetry),
        SAMPLE_RATE as f32,
        Arc::clone(&stop),
    );

    let capture = pw::stream::StreamBox::new(
        &core,
        "TorroEQ input",
        properties! {
            *pw::keys::NODE_NAME => "torroeq_sink",
            *pw::keys::NODE_DESCRIPTION => "TorroEQ Equalizer",
            *pw::keys::NODE_VIRTUAL => "true",
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::AUDIO_CHANNELS => "2",
        },
    )?;
    let playback = pw::stream::StreamBox::new(
        &core,
        "TorroEQ output",
        properties! {
            *pw::keys::NODE_NAME => "torroeq_output",
            *pw::keys::NODE_DESCRIPTION => "TorroEQ processed output",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::TARGET_OBJECT => output.name.as_str(),
            *pw::keys::AUDIO_CHANNELS => "2",
        },
    )?;

    let capture_data = CaptureData {
        format: Default::default(),
        eq: StereoEq::new(SAMPLE_RATE as f32),
        params,
        audio: audio_producer,
        analyzer: analyzer_producer,
        telemetry: Arc::clone(&telemetry),
        last_params: crate::dsp::ParamSnapshot::default(),
    };
    let _capture_listener = capture
        .add_local_listener_with_user_data(capture_data)
        .param_changed(|_, data, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else { return };
            if data.format.parse(param).is_ok() {
                data.eq.set_sample_rate(data.format.rate() as f32);
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(plane) = buffer.datas_mut().first_mut() else {
                return;
            };
            let byte_count = plane.chunk().size() as usize;
            let Some(bytes) = plane.data() else { return };
            let params = data.params.try_snapshot(data.last_params);
            data.last_params = params;
            data.eq.apply_params(&params);
            let mut metrics = crate::dsp::BlockMetrics::default();
            let (frames, _) = bytes[..byte_count.min(bytes.len())].as_chunks::<8>();
            for frame in frames {
                let left = f32::from_le_bytes(frame[0..4].try_into().unwrap_or([0; 4]));
                let right = f32::from_le_bytes(frame[4..8].try_into().unwrap_or([0; 4]));
                let processed = data.eq.process_frame(left, right);
                metrics.add_frame(processed.metrics);
                if data.audio.slots() >= 2 {
                    let _ = data.audio.push(processed.left);
                    let _ = data.audio.push(processed.right);
                }
                let _ = data.analyzer.push((processed.left + processed.right) * 0.5);
            }
            data.telemetry.publish_audio(
                metrics.input_peak,
                metrics.output_peak,
                metrics.clipped,
                metrics.limited,
            );
        })
        .register()?;

    let _playback_listener = playback
        .add_local_listener_with_user_data(PlaybackData {
            audio: audio_consumer,
            capture: capture.as_raw_ptr(),
        })
        .process(|stream, data| {
            // The hardware playback graph provides the clock. Trigger the
            // unlinked virtual sink once per cycle and consume its prior block.
            // SAFETY: the capture stream is created before this listener and
            // remains alive until after the listener is dropped on this loop.
            unsafe {
                pw::sys::pw_stream_trigger_process(data.capture);
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(plane) = buffer.datas_mut().first_mut() else {
                return;
            };
            let Some(bytes) = plane.data() else { return };
            let frames = bytes.len() / 8;
            let (frame_bytes, _) = bytes[..frames * 8].as_chunks_mut::<8>();
            for frame in frame_bytes {
                let left = data.audio.pop().unwrap_or(0.0);
                let right = data.audio.pop().unwrap_or(0.0);
                frame[0..4].copy_from_slice(&left.to_le_bytes());
                frame[4..8].copy_from_slice(&right.to_le_bytes());
            }
            let chunk = plane.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = (mem::size_of::<f32>() * CHANNELS as usize) as i32;
            *chunk.size_mut() = (frames * 8) as u32;
        })
        .register()?;

    let capture_format_bytes = audio_format_bytes();
    let mut capture_format = [Pod::from_bytes(&capture_format_bytes)
        .ok_or_else(|| anyhow!("could not build capture format"))?];
    capture.connect(
        spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS
            | pw::stream::StreamFlags::TRIGGER,
        &mut capture_format,
    )?;
    let playback_format_bytes = audio_format_bytes();
    let mut playback_format = [Pod::from_bytes(&playback_format_bytes)
        .ok_or_else(|| anyhow!("could not build playback format"))?];
    playback.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut playback_format,
    )?;

    let _stop_source = stop_receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    telemetry.set_latency_ms(1_000.0 * 1_024.0 / SAMPLE_RATE as f32);
    telemetry.set_running(true);
    let _ = ready.send(Ok(()));
    mainloop.run();
    stop.store(true, Ordering::Release);
    let _ = analyzer_thread.join();
    Ok(())
}

fn audio_format_bytes() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS);
    let mut positions = [0; spa::param::audio::MAX_CHANNELS];
    positions[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    positions[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(positions);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .expect("audio format serialization must succeed")
    .0
    .into_inner()
}
