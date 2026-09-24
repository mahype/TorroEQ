use std::sync::Arc;

use anyhow::{Result, bail};

use super::OutputDevice;
use crate::dsp::SharedParams;
use crate::telemetry::Telemetry;

const UNSUPPORTED: &str = "system audio routing is not available on this platform yet";

pub fn discover_outputs() -> Result<Vec<OutputDevice>> {
    Ok(Vec::new())
}

pub fn check_audio() -> Result<()> {
    bail!(UNSUPPORTED)
}

pub struct AudioEngine;

impl AudioEngine {
    pub fn start(
        _output: OutputDevice,
        _params: Arc<SharedParams>,
        _telemetry: Arc<Telemetry>,
    ) -> Result<Self> {
        bail!(UNSUPPORTED)
    }

    pub fn activate(&mut self) -> Result<()> {
        bail!(UNSUPPORTED)
    }

    pub fn deactivate(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn is_activated(&self) -> bool {
        false
    }

    pub fn stop(&mut self) {}
}
