//! System audio routing. Each platform provides the same `AudioEngine`,
//! `discover_outputs`, and `check_audio` surface; the DSP, analyzer, and UI
//! are shared.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    pub id: u32,
    /// Stable identifier: the PipeWire node name or the Core Audio device UID.
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

#[cfg(target_os = "linux")]
mod pipewire;
#[cfg(target_os = "linux")]
pub use pipewire::{AudioEngine, check_audio, discover_outputs};

#[cfg(target_os = "macos")]
mod coreaudio;
#[cfg(target_os = "macos")]
pub use coreaudio::{AudioEngine, check_audio, discover_outputs};

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use unsupported::{AudioEngine, check_audio, discover_outputs};
