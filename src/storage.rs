use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::dsp::{BAND_COUNT, ParamSnapshot};

const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub version: u32,
    pub name: String,
    pub gains_db: [f32; BAND_COUNT],
    pub enabled: [bool; BAND_COUNT],
    pub preamp_db: f32,
    pub limiter_enabled: bool,
}

impl Preset {
    pub fn flat() -> Self {
        Self {
            version: FORMAT_VERSION,
            name: "Flat".into(),
            gains_db: [0.0; BAND_COUNT],
            enabled: [true; BAND_COUNT],
            preamp_db: 0.0,
            limiter_enabled: true,
        }
    }

    pub fn to_params(&self, bypass: bool) -> ParamSnapshot {
        ParamSnapshot {
            band_gains_db: self.gains_db,
            band_enabled: self.enabled,
            preamp_db: self.preamp_db,
            bypass,
            limiter_enabled: self.limiter_enabled,
            limiter_threshold: 0.891_250_9,
        }
        .sanitized()
    }

    pub fn from_params(name: impl Into<String>, params: ParamSnapshot) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: name.into(),
            gains_db: params.band_gains_db,
            enabled: params.band_enabled,
            preamp_db: params.preamp_db,
            limiter_enabled: params.limiter_enabled,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub version: u32,
    pub preset: String,
    pub output_name: Option<String>,
    pub focus_view: bool,
    pub params: Preset,
    pub bypass: bool,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            preset: "Flat".into(),
            output_name: None,
            focus_view: false,
            params: Preset::flat(),
            bypass: false,
        }
    }
}

pub struct Storage {
    state_dir: PathBuf,
    presets_dir: PathBuf,
}

impl Storage {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("com", "Torro", "TorroEQ")
            .context("could not determine user data directories")?;
        Ok(Self {
            state_dir: dirs
                .state_dir()
                .unwrap_or(dirs.data_local_dir())
                .to_path_buf(),
            presets_dir: dirs.data_dir().join("presets"),
        })
    }

    #[cfg(test)]
    fn at(root: &Path) -> Self {
        Self {
            state_dir: root.join("state"),
            presets_dir: root.join("presets"),
        }
    }

    pub fn load_state(&self) -> Result<SavedState> {
        let path = self.state_dir.join("state.toml");
        if !path.exists() {
            return Ok(SavedState::default());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let state: SavedState = toml::from_str(&content)
            .with_context(|| format!("could not parse {}", path.display()))?;
        if state.version != FORMAT_VERSION {
            bail!("unsupported state version {}", state.version);
        }
        Ok(state)
    }

    pub fn save_state(&self, state: &SavedState) -> Result<()> {
        let content = toml::to_string_pretty(state)?;
        atomic_write(&self.state_dir.join("state.toml"), content.as_bytes())
    }

    pub fn load_presets(&self) -> Result<Vec<Preset>> {
        let mut presets = factory_presets();
        if !self.presets_dir.exists() {
            return Ok(presets);
        }
        let mut paths = fs::read_dir(&self.presets_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "toml")
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let content = fs::read_to_string(&path)?;
            let preset: Preset = toml::from_str(&content)
                .with_context(|| format!("could not parse preset {}", path.display()))?;
            if preset.version == FORMAT_VERSION {
                presets.retain(|existing| existing.name != preset.name);
                presets.push(preset);
            }
        }
        Ok(presets)
    }

    pub fn save_preset(&self, preset: &Preset) -> Result<()> {
        let file_name = safe_name(&preset.name);
        if file_name.is_empty() {
            bail!("preset name must contain a letter or number");
        }
        let content = toml::to_string_pretty(preset)?;
        atomic_write(
            &self.presets_dir.join(format!("{file_name}.toml")),
            content.as_bytes(),
        )
    }
}

fn factory_presets() -> Vec<Preset> {
    vec![
        Preset::flat(),
        Preset {
            name: "Warm detail".into(),
            gains_db: [-2.0, -1.0, 1.5, 1.0, 0.0, 1.0, 2.0, 1.5, 2.0, 0.5],
            preamp_db: -3.0,
            ..Preset::flat()
        },
        Preset {
            name: "Bass lift".into(),
            gains_db: [5.0, 4.0, 2.5, 1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0],
            preamp_db: -5.0,
            ..Preset::flat()
        },
        Preset {
            name: "Voice clarity".into(),
            gains_db: [-4.0, -3.0, -2.0, -1.0, 0.5, 2.0, 3.0, 2.0, 0.0, -1.0],
            preamp_db: -3.0,
            ..Preset::flat()
        },
    ]
}

fn safe_name(name: &str) -> String {
    name.chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character == ' ' || character == '-' || character == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(content)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_and_presets_round_trip() {
        let root = std::env::temp_dir().join(format!("torroeq-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let storage = Storage::at(&root);
        let mut state = SavedState {
            preset: "Test state".into(),
            ..SavedState::default()
        };
        state.params.gains_db[4] = 4.5;
        storage.save_state(&state).unwrap();
        assert_eq!(storage.load_state().unwrap().params.gains_db[4], 4.5);

        let preset = Preset::from_params("My Preset", state.params.to_params(false));
        storage.save_preset(&preset).unwrap();
        assert!(
            storage
                .load_presets()
                .unwrap()
                .iter()
                .any(|item| item.name == "My Preset")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
