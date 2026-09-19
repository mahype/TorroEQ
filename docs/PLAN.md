# TorroEQ implementation plan

## Implementation status

The first end-to-end version is implemented. It includes the Studio and Focus views, keyboard and mouse control, XDG state and presets, output discovery, a lock-free ten-band DSP core, limiter, FFT analyzer, a native PipeWire virtual sink, explicit system-route activation, and a signal-path diagnostic. The remaining production-hardening work is long-duration dropout testing, device hot-plug recovery, and packaging.

## 1. Product goal

TorroEQ is a system-wide ten-band equalizer and spectrum analyzer for Linux terminals. It should feel immediate enough for live adjustment, remain safe when the UI exits unexpectedly, and fit naturally beside TorroMail as part of the same product family.

The first supported platform is this machine's stack: PipeWire 1.6, WirePlumber 0.5, stereo output at 48 kHz, and a terminal with true-color and mouse support.

## 2. Version-one scope

### Included

- Ten ISO octave bands: 31, 62, 125, 250, 500 Hz and 1, 2, 4, 8, 16 kHz
- Gain range of -12 dB to +12 dB in 0.5 dB steps
- Preamp range of -18 dB to +6 dB
- Live spectrum analyzer with logarithmic frequency axis
- Master bypass and per-band enable/disable
- Peak and clipping indicators
- Keyboard, mouse, and scroll-wheel input
- Named presets stored in the user's data directory
- Selection of the PipeWire output to process
- Restore of the last session
- Terminal resize support down to a documented minimum size

### Deferred

- Parametric filters and adjustable Q
- Input and microphone processing
- More than two channels
- Automatic room correction
- VST hosting
- Remote control and plugin APIs

## 3. Interaction model

The application opens in the Studio layout. The selected control has a red border and brighter label; selection is never conveyed by color alone.

| Action | Keyboard | Mouse |
|---|---|---|
| Select band | Left/Right or `h`/`l` | Click fader |
| Fine adjustment | Up/Down or `j`/`k` | Wheel |
| Coarse adjustment | Page Up/Page Down | Shift+wheel |
| Reset selected band | `0` | Right-click |
| Toggle selected band | Space | Click band label |
| Toggle global bypass | `b` | Click Bypass |
| Choose preset | `p` | Click preset field |
| Save preset | `s` | Click Save |
| Change output | `o` | Click output field |
| Help | `?` | Click key hint |
| Quit | `q` | Click Quit |

Tab and Shift+Tab move between the sidebar, analyzer, faders, and footer controls. Escape closes a modal without applying its pending changes.

## 4. Visual system

- Inherit the terminal background instead of painting the entire viewport.
- Use Torro red `#D50C0C` for the brand bar and `#EE3A33` for focus.
- Use neutral borders `#5A4C50`, muted copy `#978A8D`, and selection fill `#34272B`.
- Use green `#86CF7D`, amber `#ECB755`, and blue `#79BFD3` only for status semantics.
- Keep all meter values useful without color through labels, symbols, and numeric dB readings.
- Favor one-cell rules, rounded box-drawing characters, generous gaps, and monospaced numbers.
- Reduce gracefully: hide secondary help, then the session sidebar; never hide clipping or bypass state.

The SVG studies use a fixed canvas to make the hierarchy reviewable. Ratatui will render the corresponding layout in terminal cells rather than pixels.

## 5. Technical architecture

Use a Rust workspace once the first implementation phase starts:

```text
crates/
  torroeq-app/       state machine, commands, persistence
  torroeq-audio/     PipeWire graph and stream lifecycle
  torroeq-dsp/       filters, gain smoothing, FFT, meters
  torroeq-tui/       Ratatui views and input mapping
src/main.rs          process lifecycle and dependency wiring
```

Initial dependencies to evaluate and pin deliberately:

- `ratatui` for rendering
- `crossterm` for terminal input, mouse capture, and alternate screen
- `pipewire` for native graph integration
- `rustfft` for analyzer bins
- `biquad` or a small audited implementation for equalizer filters
- `serde` and `toml` for presets and configuration
- `directories` for XDG-compliant paths
- `tracing` and `tracing-subscriber` for diagnostics outside the TUI

Do not perform DSP work on the UI thread. The audio callback must avoid allocation, locks, filesystem access, and logging. UI updates travel through bounded channels and may be dropped when rendering falls behind; audio samples may not.

## 6. Audio design

### Signal path

```text
applications -> TorroEQ virtual sink -> preamp -> 10 bands -> limiter -> hardware sink
                                      \-> analyzer tap -> TUI
```

TorroEQ should create a named virtual PipeWire sink and make it the default only after the processing graph is ready. On graceful shutdown it restores the previous default. If the UI closes, audio processing should either continue in a small engine process or atomically bypass to the hardware sink; this decision is validated during the PipeWire prototype.

Each EQ band is implemented as a peaking biquad centered on its ISO frequency. Parameter changes are smoothed over a short ramp to prevent zipper noise. A conservative preamp suggestion follows the largest positive boost, but is not silently applied after the user overrides it.

The analyzer reads a post-EQ stereo downmix, applies a Hann window, computes an FFT, maps bins logarithmically, converts magnitude to dBFS, and uses separate attack/release smoothing. Analyzer load must never affect the audio callback.

## 7. Persistence and safety

- Config: `${XDG_CONFIG_HOME:-~/.config}/torroeq/config.toml`
- Presets: `${XDG_DATA_HOME:-~/.local/share}/torroeq/presets/*.toml`
- State: `${XDG_STATE_HOME:-~/.local/state}/torroeq/state.toml`
- Atomic writes via temporary file plus rename
- Configuration version in every persisted document
- Hard gain limits in the DSP layer, independent of UI validation
- Automatic limiter enabled by default
- Startup must not reroute audio if initialization is incomplete
- Panic hook must restore terminal state before reporting the failure

## 8. Delivery phases

### Phase 0: validate assumptions

- Confirm direct PipeWire capture and playback on the current S/PDIF sink.
- Measure round-trip and processing latency.
- Prototype virtual sink creation and reliable rollback.
- Decide whether the engine lives in-process or as a user service.

Exit criterion: a command-line passthrough runs for one hour without dropouts and survives sink reconnects.

### Phase 1: static TUI prototype

- Implement the Studio layout with deterministic fixture data.
- Add keyboard, mouse, focus, modal, and resize behavior.
- Add snapshot tests for 120x36, 100x30, and 80x24 terminals.
- Confirm readable monochrome and 256-color fallbacks.

Exit criterion: every version-one interaction can be exercised without audio.

### Phase 2: DSP core

- Implement and unit-test ten filter bands and preamp.
- Add click-free parameter smoothing and bypass.
- Add FFT analysis and peak metering.
- Test impulse response, frequency response, gain bounds, and NaN handling.

Exit criterion: generated test tones match the requested gain within a documented tolerance.

### Phase 3: PipeWire integration

- Connect the DSP core to PipeWire.
- Add output discovery and reconnect behavior.
- Introduce safe graph activation, bypass, and shutdown.
- Exercise suspend, unplug, default-device change, and sample-rate change.

Exit criterion: normal desktop applications can select TorroEQ and remain audible through failures.

### Phase 4: presets and polish

- Implement persistence, preset workflows, first-run help, and diagnostics.
- Add factory presets only after listening tests.
- Profile CPU use and tune analyzer refresh independently of audio block size.
- Package for Arch Linux and document installation.

Exit criterion: release candidate passes the manual audio matrix and automated test suite.

## 9. Test strategy

- Unit tests for state transitions, key maps, DSP coefficients, smoothing, and persistence migration
- Render snapshots for layouts, focus states, clipping, bypass, and dialogs
- Property tests ensuring finite bounded output for finite bounded input
- Golden frequency-response tests generated from deterministic signals
- Integration tests against a disposable PipeWire instance where practical
- Manual matrix covering S/PDIF, Wave XLR, HDMI, sink loss, terminal resize, mouse/no mouse, and clean/unclean exit

## 10. Decisions needed after prototypes

1. Keep the audio engine alive when the TUI exits, or treat the application as one process?
2. Set TorroEQ as the default sink automatically, or require an explicit user action?
3. Display pre-EQ, post-EQ, or switchable spectrum data?
4. Ship the Studio layout only, or retain Focus as a runtime view?

The recommendation is a persistent engine, explicit first-time activation, switchable pre/post analyzer, and Studio plus Focus views.
