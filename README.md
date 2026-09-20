# TorroEQ

A precise, keyboard-first system equalizer for PipeWire, designed as a polished terminal user interface.

## Product direction

- Ten-band graphic equalizer from 31 Hz to 16 kHz with 0.5 dB steps
- Real-time 4096-point FFT spectrum analyzer from 20 Hz to 20 kHz
- Keyboard and mouse operation
- Native system-wide PipeWire processing and explicit safe activation
- Presets, output selection, preamp, bypass, and clipping protection
- A restrained visual language shared with TorroMail

## Requirements

- Linux with PipeWire, WirePlumber, and `pactl`
- Rust 1.80 or newer when building from source
- A true-color terminal; minimum supported size is 72 x 22

## Build and run

```bash
cargo build --release
./target/release/torroeq
```

TorroEQ creates a virtual `TorroEQ Equalizer` sink, makes it the default, and moves streams already playing on the selected output through the equalizer. Press `a` in the TUI to disable or re-enable system routing. The previous default and those streams are restored when TorroEQ exits cleanly.

Useful diagnostics:

```bash
cargo run -- --list-outputs
cargo run -- --check-audio
cargo run -- --demo
```

`--check-audio` registers the virtual sink, passes a short generated signal through the complete processing path, verifies default-route activation, restores the prior output, and exits.

## Controls

| Action | Keys | Mouse |
|---|---|---|
| Select band | Left/Right or `h`/`l` | Click |
| Adjust gain | Up/Down or `j`/`k` | Wheel |
| Coarse adjustment | Page Up/Page Down | Shift+wheel |
| Reset band | `0` | Right-click |
| Enable band | Space | - |
| Activate system routing | `a` | - |
| Global bypass | `b` | Click |
| Limiter | `m` | - |
| Preamp | `[` / `]` | - |
| Master volume | `-` / `+` | Wheel over master |
| Presets / outputs | `p` / `o` | Click |
| Save preset | `s` | - |
| Studio / focus view | `v` | - |
| Help / quit | `?` / `q` | - |

Configuration follows the XDG base-directory convention. Session state is stored below `~/.local/state/torroeq`; user presets are stored below `~/.local/share/torroeq/presets`.

## Design studies

The recommended direction is **Studio**, with two alternatives for comparison:

- [`studio.svg`](docs/design/studio.svg): spectrum, faders, and session controls in one balanced workspace
- [`focus.svg`](docs/design/focus.svg): maximum room for the analyzer and active band
- [`classic-rack.svg`](docs/design/classic-rack.svg): dense hardware-inspired presentation

Open the SVG files directly in a browser or image viewer. See [`docs/design/README.md`](docs/design/README.md) for the rationale.

The ten-band analyzer follows the segmented display language of Sony's late-1980s and 1990s equalizers. See [`docs/SONY_ANALYZER_DESIGN.md`](docs/SONY_ANALYZER_DESIGN.md) for the references and rendering decisions.

## Plan

The phased implementation plan and key technical decisions are documented in [`docs/PLAN.md`](docs/PLAN.md).

## Architecture

The audio callback owns the DSP state and performs no allocation, filesystem access, or locking. UI parameters are read through a lock-free coherent snapshot. FFT analysis runs on a separate worker and may drop visualization samples rather than delay audio. See [`docs/PLAN.md`](docs/PLAN.md) for the complete rationale and safety model.
