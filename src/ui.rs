use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Sparkline,
    Widget, Wrap,
};

use crate::app::{App, Dialog, ViewMode};
use crate::dsp::{BAND_COUNT, BAND_FREQUENCIES, MAX_GAIN_DB, MIN_GAIN_DB};

const BRAND: Color = Color::Rgb(213, 12, 12);
const ACCENT: Color = Color::Rgb(238, 58, 51);
const MUTED: Color = Color::Rgb(151, 138, 141);
const FAINT: Color = Color::Rgb(106, 94, 97);
const BORDER: Color = Color::Rgb(90, 76, 80);
const PANEL: Color = Color::Rgb(23, 19, 21);
const SELECTED: Color = Color::Rgb(52, 39, 43);
const GREEN: Color = Color::Rgb(134, 207, 125);
const AMBER: Color = Color::Rgb(236, 183, 85);

pub fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    if area.width < 72 || area.height < 22 {
        frame.render_widget(
            Paragraph::new("TorroEQ needs at least 72 columns x 22 rows")
                .alignment(Alignment::Center)
                .block(panel(" TERMINAL TOO SMALL ", true)),
            centered(area, 60, 7),
        );
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(2),
        ])
        .split(area);
    render_header(frame, rows[0], app);
    match app.view {
        ViewMode::Studio => render_studio(frame, rows[1], app),
        ViewMode::Focus => render_focus(frame, rows[1], app),
    }
    render_footer(frame, rows[2]);
    if let Some(dialog) = app.dialog.clone() {
        render_dialog(frame, area, &dialog, app);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let status = if app.telemetry.running {
        Span::styled("● LIVE", Style::default().fg(GREEN).bold())
    } else {
        Span::styled("○ OFFLINE", Style::default().fg(AMBER).bold())
    };
    let line = Line::from(vec![
        Span::styled(" TORRO", Style::default().fg(Color::White).bold()),
        Span::styled("EQ", Style::default().fg(Color::Gray).bold()),
        Span::raw(" "),
        Span::styled(
            match app.view {
                ViewMode::Studio => "STUDIO",
                ViewMode::Focus => "FOCUS",
            },
            Style::default().fg(Color::White),
        ),
        Span::raw("  "),
        status,
    ]);
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().bg(BRAND))
            .alignment(Alignment::Left)
            .block(Block::default().style(Style::default().bg(BRAND))),
        area,
    );
    let right = format!("48 kHz / 2ch  {:>6.1} ms ", app.telemetry.latency_ms);
    frame.render_widget(
        Paragraph::new(right)
            .style(Style::default().fg(Color::White).bg(BRAND))
            .alignment(Alignment::Right),
        area,
    );
}

fn render_studio(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let wide = area.width >= 100;
    let columns = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(42)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(42)])
            .split(area)
    };
    if wide {
        render_session(frame, columns[0], app);
    }
    let main = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(columns[1]);
    render_spectrum(frame, main[0], app, false);
    render_faders(frame, main[1], app);
}

fn render_session(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let inner = panel(" SESSION ", false).inner(area);
    frame.render_widget(panel(" SESSION ", false), area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(2),
        ])
        .split(inner);

    app.hit_regions.output = rows[0];
    app.hit_regions.preset = rows[1];
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("OUTPUT", Style::default().fg(MUTED)),
            Line::from(
                app.selected_output()
                    .map_or("No output", |o| &o.description),
            ),
        ]),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled("PRESET", Style::default().fg(MUTED)),
            Line::from(vec![
                Span::styled("▌ ", Style::default().fg(ACCENT)),
                Span::raw(&app.preset_name),
                Span::styled(
                    if app.dirty { " *" } else { "" },
                    Style::default().fg(AMBER),
                ),
            ]),
        ])
        .style(Style::default().bg(SELECTED)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("PREAMP  ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{:+.1} dB", app.params.preamp_db),
                Style::default().bold(),
            ),
        ])),
        rows[2],
    );
    let headroom = suggested_headroom(app);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().title("HEADROOM".fg(MUTED)))
            .gauge_style(Style::default().fg(if headroom > 0.0 { GREEN } else { AMBER }))
            .ratio(((headroom + 12.0) / 24.0).clamp(0.0, 1.0) as f64)
            .label(format!("{headroom:.1} dB")),
        rows[3],
    );
    app.hit_regions.bypass = rows[4];
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if app.params.bypass { "○" } else { "●" },
                Style::default().fg(if app.params.bypass { AMBER } else { GREEN }),
            ),
            Span::raw(if app.params.bypass {
                " BYPASSED"
            } else {
                " EQ ACTIVE"
            }),
            Span::styled("  [b]", Style::default().fg(MUTED)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(BORDER)),
        ),
        rows[4],
    );
    let status = if app.telemetry.clipped {
        ("▲ CLIPPING", ACCENT)
    } else if app.telemetry.limited {
        ("▲ LIMITING", AMBER)
    } else if app.telemetry.running {
        ("● HEALTHY", GREEN)
    } else {
        ("○ OFFLINE", MUTED)
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![Span::styled(
                status.0,
                Style::default().fg(status.1).bold(),
            )]),
            Line::styled(
                format!(
                    "Peak  {:>6.1} dBFS",
                    amplitude_db(app.telemetry.output_peak)
                ),
                Style::default().fg(MUTED),
            ),
            Line::styled(
                format!("Input {:>6.1} dBFS", amplitude_db(app.telemetry.input_peak)),
                Style::default().fg(FAINT),
            ),
            Line::styled(
                if app.route_active {
                    "SYSTEM ROUTING ON [a]"
                } else {
                    "Press [a] to activate"
                },
                Style::default().fg(if app.route_active { GREEN } else { AMBER }),
            ),
        ]),
        rows[5],
    );
}

fn render_spectrum(frame: &mut Frame<'_>, area: Rect, app: &App, focused: bool) {
    let data = app
        .telemetry
        .spectrum
        .iter()
        .map(|db| (db + 72.0).clamp(0.0, 72.0) as u64)
        .collect::<Vec<_>>();
    frame.render_widget(
        Sparkline::default()
            .block(panel(" POST-EQ SPECTRUM  20 Hz ... 20 kHz ", focused))
            .data(&data)
            .style(Style::default().fg(ACCENT))
            .absent_value_symbol(symbols::shade::LIGHT),
        area,
    );
}

fn render_faders(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    frame.render_widget(panel(" GRAPHIC EQ  +/-12 dB ", true), area);
    let inner = area.inner(Margin::new(2, 1));
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, BAND_COUNT as u32); BAND_COUNT])
        .split(inner);
    for (index, column) in columns.iter().copied().enumerate() {
        app.hit_regions.bands[index] = column;
        frame.render_widget(
            Fader {
                frequency: BAND_FREQUENCIES[index],
                gain: app.params.band_gains_db[index],
                enabled: app.params.band_enabled[index],
                selected: app.selected_band == index,
            },
            column,
        );
    }
}

fn render_focus(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(7)])
        .split(area);
    render_spectrum(frame, rows[0], app, false);
    frame.render_widget(panel(" ACTIVE BAND ", true), rows[1]);
    let inner = rows[1].inner(Margin::new(2, 1));
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Min(20),
            Constraint::Length(14),
        ])
        .split(inner);
    let band = app.selected_band;
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                format_frequency(BAND_FREQUENCIES[band]),
                Style::default().bold(),
            ),
            Line::styled(band_description(band), Style::default().fg(MUTED)),
        ]),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{:+.1} dB", app.params.band_gains_db[band]))
            .style(Style::default().fg(ACCENT).bold()),
        columns[1],
    );
    let ratio =
        ((app.params.band_gains_db[band] - MIN_GAIN_DB) / (MAX_GAIN_DB - MIN_GAIN_DB)) as f64;
    frame.render_widget(
        Gauge::default()
            .ratio(ratio)
            .gauge_style(Style::default().fg(ACCENT))
            .label("-12       0       +12"),
        columns[2],
    );
    frame.render_widget(
        Paragraph::new(if app.params.band_enabled[band] {
            "[ ENABLED ]"
        } else {
            "[ DISABLED ]"
        })
        .alignment(Alignment::Center)
        .style(Style::default().fg(if app.params.band_enabled[band] {
            GREEN
        } else {
            MUTED
        })),
        columns[3],
    );
    app.hit_regions.bands[band] = rows[1];
}

fn render_footer(frame: &mut Frame<'_>, area: Rect) {
    let hints = Line::from(vec![
        Span::styled(" move ", Style::default().fg(MUTED)),
        Span::raw("h/l  "),
        Span::styled("gain ", Style::default().fg(MUTED)),
        Span::raw("j/k  "),
        Span::styled("toggle ", Style::default().fg(MUTED)),
        Span::raw("space  "),
        Span::styled("activate ", Style::default().fg(MUTED)),
        Span::raw("a  "),
        Span::styled("view ", Style::default().fg(MUTED)),
        Span::raw("v  "),
        Span::styled("preset ", Style::default().fg(MUTED)),
        Span::raw("p  "),
        Span::styled("save ", Style::default().fg(MUTED)),
        Span::raw("s  "),
        Span::styled("help ", Style::default().fg(MUTED)),
        Span::raw("?  "),
        Span::styled("quit ", Style::default().fg(MUTED)),
        Span::raw("q"),
    ]);
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().bg(PANEL)),
        area,
    );
}

fn render_dialog(frame: &mut Frame<'_>, area: Rect, dialog: &Dialog, app: &App) {
    let modal = match dialog {
        Dialog::Help => centered(area, 68, 20),
        Dialog::Error(_) => centered(area, 68, 9),
        Dialog::SavePreset { .. } => centered(area, 60, 9),
        Dialog::Presets { .. } | Dialog::Outputs { .. } => centered(area, 64, 16),
    };
    frame.render_widget(Clear, modal);
    match dialog {
        Dialog::Help => {
            let help = Text::from(vec![
                Line::from("Left/Right, h/l     Select frequency band"),
                Line::from("Up/Down, j/k        Adjust by 0.5 dB"),
                Line::from("Page Up/Page Down   Adjust by 2 dB"),
                Line::from("0                    Reset selected band"),
                Line::from("Space                Enable/disable band"),
                Line::from("b                    Toggle global bypass"),
                Line::from("a                    Activate system routing"),
                Line::from("m                    Toggle limiter"),
                Line::from("[ / ]                Adjust preamp"),
                Line::from("p / o                Presets / outputs"),
                Line::from("s                    Save current preset"),
                Line::from("v                    Studio/focus view"),
                Line::from("Mouse                Click, wheel, double-click"),
                Line::from("Esc                  Close dialog"),
            ]);
            frame.render_widget(
                Paragraph::new(help)
                    .block(panel(" HELP  [?] ", true))
                    .wrap(Wrap { trim: false }),
                modal,
            );
        }
        Dialog::Presets { selected } => {
            let items = app
                .presets
                .iter()
                .map(|preset| ListItem::new(preset.name.clone()))
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(" PRESETS  [Enter] apply ", true))
                    .highlight_style(Style::default().bg(SELECTED).fg(ACCENT).bold())
                    .highlight_symbol("▌ "),
                modal,
                &mut state,
            );
        }
        Dialog::Outputs { selected } => {
            let items = app
                .outputs
                .iter()
                .map(|output| ListItem::new(output.description.clone()))
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(panel(" OUTPUTS  [Enter] connect ", true))
                    .highlight_style(Style::default().bg(SELECTED).fg(ACCENT).bold())
                    .highlight_symbol("▌ "),
                modal,
                &mut state,
            );
        }
        Dialog::SavePreset { name } => frame.render_widget(
            Paragraph::new(vec![
                Line::styled("Name", Style::default().fg(MUTED)),
                Line::styled(format!("> {name}_"), Style::default().fg(ACCENT).bold()),
                Line::styled("Enter to save / Esc to cancel", Style::default().fg(MUTED)),
            ])
            .block(panel(" SAVE PRESET ", true)),
            modal,
        ),
        Dialog::Error(message) => frame.render_widget(
            Paragraph::new(message.as_str())
                .block(panel(" ERROR ", true))
                .wrap(Wrap { trim: true }),
            modal,
        ),
    }
}

struct Fader {
    frequency: f32,
    gain: f32,
    enabled: bool,
    selected: bool,
}

impl Widget for Fader {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.width < 3 || area.height < 6 {
            return;
        }
        let style = if self.enabled {
            Style::default().fg(BORDER)
        } else {
            Style::default().fg(FAINT).add_modifier(Modifier::DIM)
        };
        let label_rows = 2;
        let track_top = area.y + 1;
        let track_bottom = area.bottom().saturating_sub(label_rows + 1);
        let center = area.x + area.width / 2;
        for y in track_top..=track_bottom {
            buffer[(center, y)].set_symbol("│").set_style(style);
        }
        let ratio = ((self.gain - MIN_GAIN_DB) / (MAX_GAIN_DB - MIN_GAIN_DB)).clamp(0.0, 1.0);
        let travel = track_bottom.saturating_sub(track_top) as f32;
        let knob_y = track_bottom.saturating_sub((ratio * travel).round() as u16);
        let knob = if area.width >= 6 {
            "━━━━"
        } else {
            "━━"
        };
        let knob_x = center.saturating_sub((knob.chars().count() / 2) as u16);
        buffer.set_string(
            knob_x,
            knob_y,
            knob,
            Style::default()
                .fg(if self.selected { ACCENT } else { Color::White })
                .add_modifier(Modifier::BOLD),
        );
        if self.selected {
            buffer[(area.x, knob_y)].set_symbol("▌").set_fg(ACCENT);
        }
        let frequency = format_frequency(self.frequency);
        let label_x = area.x + area.width.saturating_sub(frequency.len() as u16) / 2;
        buffer.set_string(
            label_x,
            area.bottom() - 2,
            frequency,
            Style::default().fg(if self.selected { ACCENT } else { Color::White }),
        );
        let gain = format!("{:+.1}", self.gain);
        let gain_x = area.x + area.width.saturating_sub(gain.len() as u16) / 2;
        buffer.set_string(gain_x, area.bottom() - 1, gain, Style::default().fg(MUTED));
    }
}

fn panel<'a>(title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .style(Style::default().bg(PANEL))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn format_frequency(frequency: f32) -> String {
    if frequency >= 1_000.0 {
        format!("{}k", frequency as u32 / 1_000)
    } else {
        format!("{}", frequency as u32)
    }
}

fn band_description(index: usize) -> &'static str {
    [
        "sub bass",
        "bass weight",
        "warmth",
        "body",
        "low mids",
        "midrange",
        "presence",
        "clarity",
        "detail",
        "air",
    ][index]
}

fn suggested_headroom(app: &App) -> f32 {
    let boost = app
        .params
        .band_gains_db
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    -(app.params.preamp_db + boost)
}

fn amplitude_db(amplitude: f32) -> f32 {
    20.0 * amplitude.max(1.0e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::OutputDevice;
    use crate::storage::{Preset, SavedState};
    use crate::telemetry::Telemetry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn frequency_labels_are_compact() {
        assert_eq!(format_frequency(31.0), "31");
        assert_eq!(format_frequency(4_000.0), "4k");
    }

    #[test]
    fn studio_and_focus_render_at_supported_sizes() {
        for (width, height) in [(120, 36), (100, 30), (80, 24)] {
            let telemetry = Telemetry::default();
            let mut app = App::new(
                SavedState::default(),
                vec![Preset::flat()],
                vec![OutputDevice {
                    id: 1,
                    name: "test_sink".into(),
                    description: "Test output".into(),
                    is_default: true,
                }],
                telemetry.snapshot(),
            );
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            assert!(app.hit_regions.bands[0].width > 0);

            app.view = ViewMode::Focus;
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            assert!(app.hit_regions.bands[app.selected_band].height > 0);
        }
    }
}
