use std::io::Write;
use std::io::{self, stdout};
use std::panic;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use torroeq::app::{App, Dialog, ViewMode};
use torroeq::audio::{AudioEngine, discover_outputs};
use torroeq::dsp;
use torroeq::storage::{Preset, Storage};
use torroeq::telemetry::Telemetry;
use torroeq::ui;

fn main() -> Result<()> {
    if std::env::args().any(|argument| argument == "--list-outputs") {
        for output in discover_outputs()? {
            println!("{}\t{}", output.name, output.description);
        }
        return Ok(());
    }
    if std::env::args().any(|argument| argument == "--check-audio") {
        return check_audio();
    }
    let demo = std::env::args().any(|argument| argument == "--demo");
    install_panic_hook();
    let storage = Storage::discover()?;
    let state = storage.load_state().unwrap_or_default();
    let presets = storage
        .load_presets()
        .unwrap_or_else(|_| vec![Preset::flat()]);
    let outputs = discover_outputs().unwrap_or_default();
    let telemetry = Arc::new(Telemetry::default());
    let mut app = App::new(state, presets, outputs, telemetry.snapshot());
    let mut engine = if demo {
        None
    } else {
        start_audio(&mut app, Arc::clone(&telemetry))
    };

    let mut terminal = setup_terminal()?;
    let run_result = run(&mut terminal, &mut app, &storage, &telemetry, &mut engine);
    let restore_result = restore_terminal(&mut terminal);
    if let Some(mut engine) = engine {
        engine.stop();
    }
    storage.save_state(&app.saved_state())?;
    restore_result?;
    run_result
}

fn check_audio() -> Result<()> {
    let output = discover_outputs()?
        .into_iter()
        .next()
        .context("no PipeWire audio output found")?;
    let params = Arc::new(dsp::SharedParams::default());
    let telemetry = Arc::new(Telemetry::default());
    println!("Starting TorroEQ on {}...", output.description);
    let mut engine = AudioEngine::start(output, params, Arc::clone(&telemetry))?;
    std::thread::sleep(Duration::from_millis(200));
    let mut source = std::process::Command::new("pw-cat")
        .args([
            "--playback",
            "--raw",
            "--target",
            "torroeq_sink",
            "--format",
            "f32",
            "--rate",
            "48000",
            "--channels",
            "2",
            "--sample-count",
            "9600",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(stdin) = source.stdin.as_mut() {
        for frame in 0..4_800 {
            let sample = (2.0 * std::f32::consts::PI * 440.0 * frame as f32 / 48_000.0).sin() * 0.1;
            stdin.write_all(&sample.to_le_bytes())?;
            stdin.write_all(&sample.to_le_bytes())?;
        }
    }
    drop(source.stdin.take());
    std::thread::sleep(Duration::from_millis(300));
    let _ = source.kill();
    let _ = source.wait();
    let snapshot = telemetry.snapshot();
    let graph = std::process::Command::new("pw-dump").output()?;
    let registered = String::from_utf8_lossy(&graph.stdout).contains("torroeq_sink");
    engine.activate()?;
    let routed = std::process::Command::new("pactl")
        .arg("get-default-sink")
        .output()?
        .stdout
        == b"torroeq_sink\n";
    engine.deactivate()?;
    engine.stop();
    if !snapshot.running || !registered || !routed || snapshot.input_peak < 0.05 {
        anyhow::bail!(
            "audio check failed (running={}, registered={}, routed={}, input_peak={:.3})",
            snapshot.running,
            registered,
            routed,
            snapshot.input_peak
        );
    }
    println!("Audio engine healthy; virtual sink registered and stopped cleanly.");
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut output = stdout();
    execute!(output, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(output))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen);
        original(info);
    }));
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    storage: &Storage,
    telemetry: &Arc<Telemetry>,
    engine: &mut Option<AudioEngine>,
) -> Result<()> {
    while !app.should_quit {
        app.telemetry = telemetry.snapshot();
        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key, storage)?;
                }
                Event::Mouse(mouse) => handle_mouse(app, mouse),
                _ => {}
            }
        }

        if app.audio_restart_requested {
            app.audio_restart_requested = false;
            app.route_active = false;
            if let Some(active) = engine.as_mut() {
                active.stop();
            }
            *engine = start_audio(app, Arc::clone(telemetry));
        }
        if app.route_toggle_requested {
            app.route_toggle_requested = false;
            if let Some(active) = engine.as_mut() {
                let result = if active.is_activated() {
                    active.deactivate()
                } else {
                    active.activate()
                };
                match result {
                    Ok(()) => app.route_active = active.is_activated(),
                    Err(error) => app.dialog = Some(Dialog::Error(format!("{error:#}"))),
                }
            }
        }
    }
    Ok(())
}

fn start_audio(app: &mut App, telemetry: Arc<Telemetry>) -> Option<AudioEngine> {
    let Some(output) = app.selected_output().cloned() else {
        app.dialog = Some(Dialog::Error("No PipeWire audio output found.".into()));
        return None;
    };
    match AudioEngine::start(output, Arc::clone(&app.shared_params), telemetry) {
        Ok(engine) => {
            app.route_active = engine.is_activated();
            Some(engine)
        }
        Err(error) => {
            app.dialog = Some(Dialog::Error(format!(
                "Audio engine could not start:\n{error:#}"
            )));
            None
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent, storage: &Storage) -> Result<()> {
    if app.dialog.is_some() {
        return handle_dialog_key(app, key, storage);
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.dialog = Some(Dialog::Help),
        KeyCode::Char('h') | KeyCode::Left => app.move_band(-1),
        KeyCode::Char('l') | KeyCode::Right => app.move_band(1),
        KeyCode::Char('k') | KeyCode::Up => app.adjust_gain(0.5),
        KeyCode::Char('j') | KeyCode::Down => app.adjust_gain(-0.5),
        KeyCode::PageUp => app.adjust_gain(2.0),
        KeyCode::PageDown => app.adjust_gain(-2.0),
        KeyCode::Char('0') => app.reset_band(),
        KeyCode::Char(' ') => app.toggle_band(),
        KeyCode::Char('b') => app.toggle_bypass(),
        KeyCode::Char('a') => app.route_toggle_requested = true,
        KeyCode::Char('m') => app.toggle_limiter(),
        KeyCode::Char('[') => app.adjust_preamp(-0.5),
        KeyCode::Char(']') => app.adjust_preamp(0.5),
        KeyCode::Char('v') => {
            app.view = if app.view == ViewMode::Studio {
                ViewMode::Focus
            } else {
                ViewMode::Studio
            };
        }
        KeyCode::Char('p') => {
            let selected = app
                .presets
                .iter()
                .position(|preset| preset.name == app.preset_name)
                .unwrap_or(0);
            app.dialog = Some(Dialog::Presets { selected });
        }
        KeyCode::Char('o') => {
            app.dialog = Some(Dialog::Outputs {
                selected: app.output_index,
            });
        }
        KeyCode::Char('s') => {
            app.dialog = Some(Dialog::SavePreset {
                name: if app.preset_name == "Flat" {
                    String::new()
                } else {
                    app.preset_name.clone()
                },
            });
        }
        _ => {}
    }
    Ok(())
}

fn handle_dialog_key(app: &mut App, key: KeyEvent, storage: &Storage) -> Result<()> {
    if key.code == KeyCode::Esc {
        app.dialog = None;
        return Ok(());
    }
    let Some(dialog) = app.dialog.take() else {
        return Ok(());
    };
    app.dialog = match dialog {
        Dialog::Help | Dialog::Error(_) => {
            if matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')
            ) {
                None
            } else {
                Some(dialog)
            }
        }
        Dialog::Presets { mut selected } => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
                Some(Dialog::Presets { selected })
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(app.presets.len().saturating_sub(1));
                Some(Dialog::Presets { selected })
            }
            KeyCode::Enter => {
                app.apply_preset(selected);
                None
            }
            _ => Some(Dialog::Presets { selected }),
        },
        Dialog::Outputs { mut selected } => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
                Some(Dialog::Outputs { selected })
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1).min(app.outputs.len().saturating_sub(1));
                Some(Dialog::Outputs { selected })
            }
            KeyCode::Enter => {
                app.choose_output(selected);
                None
            }
            _ => Some(Dialog::Outputs { selected }),
        },
        Dialog::SavePreset { mut name } => match key.code {
            KeyCode::Enter if !name.trim().is_empty() => {
                let preset = Preset::from_params(name.trim(), app.params);
                storage.save_preset(&preset)?;
                app.presets.retain(|existing| existing.name != preset.name);
                app.presets.push(preset.clone());
                app.preset_name = preset.name;
                app.dirty = false;
                None
            }
            KeyCode::Backspace => {
                name.pop();
                Some(Dialog::SavePreset { name })
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && name.len() < 40 =>
            {
                name.push(character);
                Some(Dialog::SavePreset { name })
            }
            _ => Some(Dialog::SavePreset { name }),
        },
    };
    Ok(())
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let point = (mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if contains(app.hit_regions.bypass, point) {
                app.toggle_bypass();
                return;
            }
            if contains(app.hit_regions.preset, point) {
                app.dialog = Some(Dialog::Presets { selected: 0 });
                return;
            }
            if contains(app.hit_regions.output, point) {
                app.dialog = Some(Dialog::Outputs {
                    selected: app.output_index,
                });
                return;
            }
            for (index, region) in app.hit_regions.bands.iter().enumerate() {
                if contains(*region, point) {
                    app.selected_band = index;
                    return;
                }
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            if app
                .hit_regions
                .bands
                .iter()
                .any(|region| contains(*region, point))
            {
                app.reset_band();
            }
        }
        MouseEventKind::ScrollUp => {
            select_hovered_band(app, point);
            app.adjust_gain(if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                2.0
            } else {
                0.5
            });
        }
        MouseEventKind::ScrollDown => {
            select_hovered_band(app, point);
            app.adjust_gain(if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                -2.0
            } else {
                -0.5
            });
        }
        _ => {}
    }
}

fn select_hovered_band(app: &mut App, point: (u16, u16)) {
    if let Some(index) = app
        .hit_regions
        .bands
        .iter()
        .position(|region| contains(*region, point))
    {
        app.selected_band = index;
    }
}

fn contains(rect: Rect, point: (u16, u16)) -> bool {
    point.0 >= rect.x && point.0 < rect.right() && point.1 >= rect.y && point.1 < rect.bottom()
}
