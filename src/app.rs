use std::sync::Arc;

use ratatui::layout::Rect;

use crate::audio::OutputDevice;
use crate::dsp::{BAND_COUNT, MAX_GAIN_DB, MIN_GAIN_DB, ParamSnapshot, SharedParams};
use crate::storage::{Preset, SavedState};
use crate::telemetry::TelemetrySnapshot;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Studio,
    Focus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    Help,
    Presets { selected: usize },
    Outputs { selected: usize },
    SavePreset { name: String },
    Error(String),
}

#[derive(Clone, Default)]
pub struct HitRegions {
    pub bands: [Rect; BAND_COUNT],
    pub bypass: Rect,
    pub preset: Rect,
    pub output: Rect,
}

pub struct App {
    pub selected_band: usize,
    pub params: ParamSnapshot,
    pub shared_params: Arc<SharedParams>,
    pub view: ViewMode,
    pub dialog: Option<Dialog>,
    pub preset_name: String,
    pub presets: Vec<Preset>,
    pub outputs: Vec<OutputDevice>,
    pub output_index: usize,
    pub telemetry: TelemetrySnapshot,
    pub hit_regions: HitRegions,
    pub should_quit: bool,
    pub audio_restart_requested: bool,
    pub route_toggle_requested: bool,
    pub route_active: bool,
    pub dirty: bool,
}

impl App {
    pub fn new(
        state: SavedState,
        presets: Vec<Preset>,
        outputs: Vec<OutputDevice>,
        telemetry: TelemetrySnapshot,
    ) -> Self {
        let params = state.params.to_params(state.bypass);
        let output_index = outputs
            .iter()
            .position(|output| output.is_default)
            .or_else(|| {
                state
                    .output_name
                    .as_ref()
                    .and_then(|name| outputs.iter().position(|output| &output.name == name))
            })
            .unwrap_or(0);
        Self {
            selected_band: 0,
            params,
            shared_params: Arc::new(SharedParams::new(params)),
            view: if state.focus_view {
                ViewMode::Focus
            } else {
                ViewMode::Studio
            },
            dialog: None,
            preset_name: state.preset,
            presets,
            outputs,
            output_index,
            telemetry,
            hit_regions: HitRegions::default(),
            should_quit: false,
            audio_restart_requested: false,
            route_toggle_requested: false,
            route_active: false,
            dirty: false,
        }
    }

    pub fn selected_output(&self) -> Option<&OutputDevice> {
        self.outputs.get(self.output_index)
    }

    pub fn move_band(&mut self, delta: isize) {
        self.selected_band = self
            .selected_band
            .saturating_add_signed(delta)
            .min(BAND_COUNT - 1);
    }

    pub fn adjust_gain(&mut self, delta: f32) {
        let gain = &mut self.params.band_gains_db[self.selected_band];
        *gain = (*gain + delta).clamp(MIN_GAIN_DB, MAX_GAIN_DB);
        *gain = (*gain * 2.0).round() / 2.0;
        self.publish_change();
    }

    pub fn reset_band(&mut self) {
        self.params.band_gains_db[self.selected_band] = 0.0;
        self.publish_change();
    }

    pub fn toggle_band(&mut self) {
        let enabled = &mut self.params.band_enabled[self.selected_band];
        *enabled = !*enabled;
        self.publish_change();
    }

    pub fn toggle_bypass(&mut self) {
        self.params.bypass = !self.params.bypass;
        self.publish_change();
    }

    pub fn toggle_limiter(&mut self) {
        self.params.limiter_enabled = !self.params.limiter_enabled;
        self.publish_change();
    }

    pub fn adjust_preamp(&mut self, delta: f32) {
        self.params.preamp_db = (self.params.preamp_db + delta).clamp(-18.0, 6.0);
        self.params.preamp_db = (self.params.preamp_db * 2.0).round() / 2.0;
        self.publish_change();
    }

    pub fn apply_preset(&mut self, index: usize) {
        if let Some(preset) = self.presets.get(index) {
            self.params = preset.to_params(self.params.bypass);
            self.preset_name = preset.name.clone();
            self.shared_params.replace(self.params);
            self.dirty = false;
        }
    }

    pub fn choose_output(&mut self, index: usize) {
        if index < self.outputs.len() && index != self.output_index {
            self.output_index = index;
            self.audio_restart_requested = true;
        }
    }

    pub fn saved_state(&self) -> SavedState {
        SavedState {
            version: 1,
            preset: self.preset_name.clone(),
            output_name: self.selected_output().map(|output| output.name.clone()),
            focus_view: self.view == ViewMode::Focus,
            params: Preset::from_params(self.preset_name.clone(), self.params),
            bypass: self.params.bypass,
        }
    }

    fn publish_change(&mut self) {
        self.shared_params.replace(self.params);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SavedState;
    use crate::telemetry::Telemetry;

    #[test]
    fn current_default_output_wins_over_stale_saved_output() {
        let state = SavedState {
            output_name: Some("speakers".into()),
            ..SavedState::default()
        };
        let outputs = vec![
            OutputDevice {
                id: 1,
                name: "headphones".into(),
                description: "Headphones".into(),
                is_default: true,
            },
            OutputDevice {
                id: 2,
                name: "speakers".into(),
                description: "Speakers".into(),
                is_default: false,
            },
        ];

        let app = App::new(
            state,
            vec![Preset::flat()],
            outputs,
            Telemetry::default().snapshot(),
        );

        assert_eq!(
            app.selected_output().map(|output| output.name.as_str()),
            Some("headphones")
        );
    }
}
