# TorroEQ

A precise, keyboard-first system equalizer for PipeWire, designed as a polished terminal user interface.

TorroEQ is currently in the design phase. The repository contains the product plan, technical architecture, and three interface studies that establish the direction before audio processing begins.

## Product direction

- Ten-band graphic equalizer from 31 Hz to 16 kHz
- Real-time spectrum analyzer from 20 Hz to 20 kHz
- Keyboard and mouse operation
- System-wide PipeWire processing with a bypass that remains safe
- Presets, per-output profiles, and clipping protection
- A restrained visual language shared with TorroMail

## Design studies

The recommended direction is **Studio**, with two alternatives for comparison:

- [`studio.svg`](docs/design/studio.svg): spectrum, faders, and session controls in one balanced workspace
- [`focus.svg`](docs/design/focus.svg): maximum room for the analyzer and active band
- [`classic-rack.svg`](docs/design/classic-rack.svg): dense hardware-inspired presentation

Open the SVG files directly in a browser or image viewer. See [`docs/design/README.md`](docs/design/README.md) for the rationale.

## Plan

The phased implementation plan and key technical decisions are documented in [`docs/PLAN.md`](docs/PLAN.md).

## Status

Planning scaffold only. No audio is modified yet.
