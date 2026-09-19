use std::ffi::{CString, c_void};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::properties::properties;
use rtrb::{Producer, RingBuffer};
use serde::Deserialize;

use crate::analyzer;
use crate::dsp::{BlockMetrics, ParamSnapshot, SharedParams, StereoEq};
use crate::telemetry::Telemetry;

const SAMPLE_RATE: u32 = 48_000;
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

#[derive(Deserialize)]
struct PulseSink {
    index: u32,
    name: String,
}

#[derive(Deserialize)]
struct SinkInput {
    index: u32,
    sink: u32,
    properties: serde_json::Map<String, serde_json::Value>,
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
        if let Some(previous) = &self.previous_default
            && let (Ok(sinks), Ok(inputs)) = (pulse_sinks(), sink_inputs())
            && let Some(previous_id) = sinks
                .iter()
                .find(|sink| &sink.name == previous)
                .map(|sink| sink.index)
        {
            for input in inputs {
                let node_name = string_prop(&input.properties, "node.name").unwrap_or_default();
                if input.sink == previous_id && node_name != "torroeq_output" {
                    let _ = move_sink_input(input.index, "torroeq_sink");
                }
            }
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
            if let (Ok(sinks), Ok(inputs)) = (pulse_sinks(), sink_inputs())
                && let Some(torroeq_id) = sinks
                    .iter()
                    .find(|sink| sink.name == "torroeq_sink")
                    .map(|sink| sink.index)
            {
                for input in inputs {
                    if input.sink == torroeq_id {
                        let _ = move_sink_input(input.index, previous);
                    }
                }
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

fn pulse_sinks() -> Result<Vec<PulseSink>> {
    pactl_json(&["list", "sinks"])
}

fn sink_inputs() -> Result<Vec<SinkInput>> {
    pactl_json(&["list", "sink-inputs"])
}

fn pactl_json<T: for<'de> Deserialize<'de>>(arguments: &[&str]) -> Result<T> {
    let output = Command::new("pactl")
        .arg("--format=json")
        .args(arguments)
        .output()
        .context("could not inspect PulseAudio-compatible PipeWire objects")?;
    if !output.status.success() {
        bail!("pactl {} failed", arguments.join(" "));
    }
    serde_json::from_slice(&output.stdout).context("could not parse pactl JSON output")
}

fn move_sink_input(index: u32, target: &str) -> Result<()> {
    let status = Command::new("pactl")
        .args(["move-sink-input", &index.to_string(), target])
        .status()
        .with_context(|| format!("could not move audio stream {index}"))?;
    if !status.success() {
        bail!("could not move audio stream {index} to {target}");
    }
    Ok(())
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

struct FilterData {
    eq: StereoEq,
    params: Arc<SharedParams>,
    analyzer: Producer<f32>,
    telemetry: Arc<Telemetry>,
    last_params: ParamSnapshot,
    input_left: *mut c_void,
    input_right: *mut c_void,
    output_left: *mut c_void,
    output_right: *mut c_void,
}

unsafe extern "C" fn process_filter(
    data: *mut c_void,
    position: *mut pw::spa::sys::spa_io_position,
) {
    if data.is_null() || position.is_null() {
        return;
    }
    // SAFETY: PipeWire invokes this callback with the FilterData pointer and
    // position supplied for the lifetime of the connected filter.
    let state = unsafe { &mut *data.cast::<FilterData>() };
    let frames = unsafe { (*position).clock.duration as usize };
    if frames == 0 {
        return;
    }
    let input_left =
        unsafe { pw::sys::pw_filter_get_dsp_buffer(state.input_left, frames as u32).cast::<f32>() };
    let input_right = unsafe {
        pw::sys::pw_filter_get_dsp_buffer(state.input_right, frames as u32).cast::<f32>()
    };
    let output_left = unsafe {
        pw::sys::pw_filter_get_dsp_buffer(state.output_left, frames as u32).cast::<f32>()
    };
    let output_right = unsafe {
        pw::sys::pw_filter_get_dsp_buffer(state.output_right, frames as u32).cast::<f32>()
    };
    if input_left.is_null()
        || input_right.is_null()
        || output_left.is_null()
        || output_right.is_null()
    {
        return;
    }

    let params = state.params.try_snapshot(state.last_params);
    state.last_params = params;
    state.eq.apply_params(&params);
    let mut metrics = BlockMetrics::default();
    for index in 0..frames {
        let left = unsafe { *input_left.add(index) };
        let right = unsafe { *input_right.add(index) };
        let processed = state.eq.process_frame(left, right);
        unsafe {
            *output_left.add(index) = processed.left;
            *output_right.add(index) = processed.right;
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
    let (analyzer_producer, analyzer_consumer) = RingBuffer::new(ANALYZER_RING_SAMPLES);
    let analyzer_thread = analyzer::spawn(
        analyzer_consumer,
        Arc::clone(&telemetry),
        SAMPLE_RATE as f32,
        Arc::clone(&stop),
    );

    let mut filter_data = Box::new(FilterData {
        eq: StereoEq::new(SAMPLE_RATE as f32),
        params,
        analyzer: analyzer_producer,
        telemetry: Arc::clone(&telemetry),
        last_params: ParamSnapshot::default(),
        input_left: ptr::null_mut(),
        input_right: ptr::null_mut(),
        output_left: ptr::null_mut(),
        output_right: ptr::null_mut(),
    });
    let events = Box::new(pw::sys::pw_filter_events {
        version: pw::sys::PW_VERSION_FILTER_EVENTS,
        destroy: None,
        state_changed: None,
        io_changed: None,
        param_changed: None,
        add_buffer: None,
        remove_buffer: None,
        process: Some(process_filter),
        drained: None,
        command: None,
    });
    let filter_name = CString::new("TorroEQ filter")?;
    let filter = unsafe {
        pw::sys::pw_filter_new_simple(
            mainloop.loop_().as_raw_ptr(),
            filter_name.as_ptr(),
            properties! {
                *pw::keys::NODE_NAME => "torroeq_filter",
                *pw::keys::NODE_DESCRIPTION => "TorroEQ DSP",
                *pw::keys::NODE_VIRTUAL => "true",
                *pw::keys::OBJECT_REGISTER => "true",
                *pw::keys::MEDIA_CLASS => "Audio/Filter",
                *pw::keys::MEDIA_TYPE => "Audio",
                *pw::keys::MEDIA_CATEGORY => "Filter",
                *pw::keys::MEDIA_ROLE => "DSP",
                *pw::keys::AUDIO_CHANNELS => "2",
                "audio.position" => "[ FL FR ]",
                "node.autoconnect" => "false",
            }
            .into_raw(),
            events.as_ref(),
            filter_data.as_mut() as *mut FilterData as *mut c_void,
        )
    };
    if filter.is_null() {
        bail!("could not create PipeWire filter");
    }

    filter_data.input_left = add_filter_port(filter, true, "input_FL", "FL")?;
    filter_data.input_right = add_filter_port(filter, true, "input_FR", "FR")?;
    filter_data.output_left = add_filter_port(filter, false, "output_FL", "FL")?;
    filter_data.output_right = add_filter_port(filter, false, "output_FR", "FR")?;
    let connect_result = unsafe {
        pw::sys::pw_filter_connect(
            filter,
            pw::sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS,
            ptr::null_mut(),
            0,
        )
    };
    if connect_result < 0 {
        unsafe { pw::sys::pw_filter_destroy(filter) };
        bail!("could not connect PipeWire filter ({connect_result})");
    }

    let mut loopback = Command::new("pw-loopback")
        .args([
            "--channels",
            "2",
            "--channel-map",
            "[ FL FR ]",
            "--latency",
            "5",
            "--capture-props",
            "{ node.name = torroeq_sink node.description = \"TorroEQ Equalizer\" media.class = Audio/Sink node.virtual = true }",
            "--playback",
            "0",
            "--playback-props",
            "{ node.name = torroeq_feed node.description = \"TorroEQ Feed\" node.autoconnect = false node.passive = true }",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("could not start PipeWire loopback")?;

    let target = output.name.clone();
    let linker = thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(400));
        link_ports("torroeq_feed", "output_FL", "torroeq_filter", "input_FL");
        link_ports("torroeq_feed", "output_FR", "torroeq_filter", "input_FR");
        link_ports("torroeq_filter", "output_FL", &target, "playback_FL");
        link_ports("torroeq_filter", "output_FR", &target, "playback_FR");
    });

    let _stop_source = stop_receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    telemetry.set_latency_ms(5.0);
    telemetry.set_running(true);
    let _ = ready.send(Ok(()));
    mainloop.run();
    stop.store(true, Ordering::Release);
    let _ = linker.join();
    let _ = loopback.kill();
    let _ = loopback.wait();
    unsafe {
        pw::sys::pw_filter_disconnect(filter);
        pw::sys::pw_filter_destroy(filter);
    }
    let _ = analyzer_thread.join();
    Ok(())
}

fn add_filter_port(
    filter: *mut pw::sys::pw_filter,
    input: bool,
    name: &str,
    channel: &str,
) -> Result<*mut c_void> {
    let direction = if input {
        pw::spa::sys::SPA_DIRECTION_INPUT
    } else {
        pw::spa::sys::SPA_DIRECTION_OUTPUT
    };
    let port = unsafe {
        pw::sys::pw_filter_add_port(
            filter,
            direction,
            pw::sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
            0,
            properties! {
                *pw::keys::FORMAT_DSP => "32 bit float mono audio",
                *pw::keys::PORT_NAME => name,
                *pw::keys::AUDIO_CHANNEL => channel,
            }
            .into_raw(),
            ptr::null_mut(),
            0,
        )
    };
    if port.is_null() {
        bail!("could not create PipeWire port {name}");
    }
    Ok(port)
}

fn link_ports(source_node: &str, source_port: &str, target_node: &str, target_port: &str) {
    let source = format!("{source_node}:{source_port}");
    let destination = format!("{target_node}:{target_port}");
    for _ in 0..20 {
        let linked = Command::new("pw-link")
            .args([source.as_str(), destination.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if linked {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
}
