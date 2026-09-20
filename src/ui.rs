use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph,
    Widget, Wrap,
};

use crate::app::{App, Dialog, ViewMode};
use crate::dsp::{
    BAND_COUNT, BAND_FREQUENCIES, MAX_GAIN_DB, MAX_MASTER_DB, MIN_GAIN_DB, MIN_MASTER_DB,
};

const BRAND: Color = Color::Rgb(213, 12, 12);
const ACCENT: Color = Color::Rgb(238, 58, 51);
const MUTED: Color = Color::Rgb(151, 138, 141);
const FAINT: Color = Color::Rgb(106, 94, 97);
const BORDER: Color = Color::Rgb(90, 76, 80);
const PANEL: Color = Color::Rgb(23, 19, 21);
const SELECTED: Color = Color::Rgb(52, 39, 43);
const GREEN: Color = Color::Rgb(134, 207, 125);
const AMBER: Color = Color::Rgb(236, 183, 85);
const VFD: Color = Color::Rgb(92, 218, 207);
const VFD_DIM: Color = Color::Rgb(35, 58, 59);
const SPECTRUM_FLOOR_DB: f32 = -48.0;

pub fn render(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    app.hit_regions = Default::default();
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
            Constraint::Length(1),
            Constraint::Min(16),
            Constraint::Length(1),
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

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let (status_text, status_color) = if app.telemetry.running {
        ("● LIVE", GREEN)
    } else {
        ("○ OFFLINE", AMBER)
    };
    let view = match app.view {
        ViewMode::Studio => "STUDIO",
        ViewMode::Focus => "FOCUS",
    };
    let right = format!("48 kHz / 2ch  {:>4.1} ms  ", app.telemetry.latency_ms);
    let prefix_width = 10 + 2 + 5 + view.len() + 2 + status_text.chars().count() + 6;
    let output_width = usize::from(area.width).saturating_sub(prefix_width + right.len());
    let output = truncate(
        app.selected_output()
            .map_or("No output", |device| device.description.as_str()),
        output_width,
    );
    let line = Line::from(vec![
        Span::styled("  \\_ TORRO", Style::default().fg(Color::White).bold()),
        Span::styled("EQ", Style::default().fg(Color::Gray).bold()),
        Span::styled(" _/  ", Style::default().fg(Color::White).bold()),
        Span::styled(view, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(status_text, Style::default().fg(status_color).bold()),
        Span::styled("  OUT ", Style::default().fg(Color::Rgb(255, 210, 206))),
        Span::styled(output.clone(), Style::default().fg(Color::White)),
        Span::raw(" ".repeat(output_width.saturating_sub(output.chars().count()))),
        Span::styled(right, Style::default().fg(Color::White)),
    ]);
    let output_start = area.x + (prefix_width - 6) as u16;
    app.hit_regions.output =
        Rect::new(output_start, area.y, (5 + output_width) as u16, area.height);
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().bg(BRAND))
            .alignment(Alignment::Left)
            .block(Block::default().style(Style::default().bg(BRAND))),
        area,
    );
}

fn render_studio(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let wide = area.width >= 100;
    let columns = if wide {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(26), Constraint::Min(42)])
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
    let block = panel(" SESSION ", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    app.hit_regions.preset = Rect::new(rows[1].x, rows[1].y, rows[1].width, 2);
    frame.render_widget(
        Paragraph::new(" PRESET").style(Style::default().fg(MUTED)),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("▌ ", Style::default().fg(ACCENT)),
            Span::styled(&app.preset_name, Style::default().bold()),
            Span::styled(
                if app.dirty { " *" } else { "" },
                Style::default().fg(AMBER),
            ),
        ])),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(session_field(
            "MASTER",
            &format!("{:.0} dB", app.params.master_db),
            inner.width,
        )),
        rows[4],
    );
    app.hit_regions.master = rows[5];
    frame.render_widget(
        MasterControl {
            gain_db: app.params.master_db,
            level: app.telemetry.output_peak,
        },
        rows[5],
    );
    frame.render_widget(
        Paragraph::new(session_field(
            "PREAMP",
            &format!("{:+.1} dB", app.params.preamp_db),
            inner.width,
        )),
        rows[7],
    );
    let headroom = suggested_headroom(app);
    frame.render_widget(
        Paragraph::new(session_field(
            "HEADROOM",
            &format!("{headroom:.1} dB"),
            inner.width,
        )),
        rows[8],
    );
    app.hit_regions.bypass = rows[10];
    frame.render_widget(
        Paragraph::new(session_status(
            if app.params.bypass { "○" } else { "●" },
            if app.params.bypass {
                "BYPASSED"
            } else {
                "EQ ACTIVE"
            },
            "[b]",
            if app.params.bypass { AMBER } else { GREEN },
            inner.width,
        )),
        rows[10],
    );
    frame.render_widget(
        Paragraph::new(session_status(
            if app.route_active { "●" } else { "○" },
            if app.route_active {
                "ROUTING ON"
            } else {
                "ROUTING OFF"
            },
            "[a]",
            if app.route_active { GREEN } else { AMBER },
            inner.width,
        )),
        rows[11],
    );
}

fn render_spectrum(frame: &mut Frame<'_>, area: Rect, app: &App, focused: bool) {
    let state = if app.telemetry.clipped {
        "CLIP"
    } else if app.telemetry.limited {
        "LIMIT"
    } else if app.telemetry.running {
        "LIVE"
    } else {
        "OFFLINE"
    };
    let title = format!(
        " ANALYZER  OUT {:>5.1}  IN {:>5.1} dBFS  {state} ",
        amplitude_db(app.telemetry.output_peak),
        amplitude_db(app.telemetry.input_peak),
    );
    frame.render_widget(panel(&title, focused), area);
    frame.render_widget(
        SpectrumMeter {
            levels: &app.telemetry.spectrum,
            peaks: &app.telemetry.spectrum_peaks,
            selected: app.selected_band,
        },
        area.inner(Margin::new(2, 1)),
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
        Span::styled("master ", Style::default().fg(MUTED)),
        Span::raw("-/+  "),
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
                Line::from("- / +                Adjust master volume"),
                Line::from("p / o                Presets / outputs"),
                Line::from("s                    Save current preset"),
                Line::from("v                    Studio/focus view"),
                Line::from("Mouse                Click, wheel, double-click"),
                Line::from("Esc                  Close dialog"),
            ]);
            frame.render_widget(
                Paragraph::new(help)
                    .block(dialog_panel(" HELP  [?] "))
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
                    .block(dialog_panel(" PRESETS  [Enter] apply "))
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
                    .block(dialog_panel(" OUTPUTS  [Enter] connect "))
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
            .block(dialog_panel(" SAVE PRESET ")),
            modal,
        ),
        Dialog::Error(message) => frame.render_widget(
            Paragraph::new(message.as_str())
                .block(dialog_panel(" ERROR "))
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

struct MasterControl {
    gain_db: f32,
    level: f32,
}

impl Widget for MasterControl {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.width < 12 || area.height < 5 {
            return;
        }
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        let label_y = area.bottom() - 1;
        let track_top = area.y;
        let track_bottom = label_y - 1;

        let fader_center = columns[0].x + columns[0].width / 2;
        for y in track_top..=track_bottom {
            buffer[(fader_center, y)].set_symbol("│").set_fg(BORDER);
        }
        let (knob_y, knob_segment) = master_fader_position(self.gain_db, track_top, track_bottom);
        buffer.set_string(
            fader_center.saturating_sub(2),
            knob_y,
            "━━━━",
            Style::default().fg(ACCENT).bold(),
        );
        buffer[(fader_center + 3, knob_y)]
            .set_symbol(knob_segment)
            .set_fg(ACCENT);

        let meter_width = 4_u16.min(columns[1].width);
        let meter_left = columns[1].x + columns[1].width.saturating_sub(meter_width) / 2;
        let meter_height = track_bottom - track_top + 1;
        let level_db = amplitude_db(self.level).clamp(SPECTRUM_FLOOR_DB, 0.0);
        let lit_segments = db_to_segment(level_db, meter_height, true);
        for y in track_top..=track_bottom {
            let from_bottom = (track_bottom - y) as usize;
            let active = from_bottom < lit_segments;
            let segment_db = SPECTRUM_FLOOR_DB
                + (from_bottom + 1) as f32 / meter_height as f32 * -SPECTRUM_FLOOR_DB;
            for x in meter_left..meter_left + meter_width {
                buffer[(x, y)].set_symbol("▄").set_fg(if active {
                    meter_color(segment_db)
                } else {
                    VFD_DIM
                });
            }
        }

        center_text(buffer, columns[0], label_y, "VOLUME", MUTED);
        center_text(buffer, columns[1], label_y, "LEVEL", MUTED);
    }
}

struct SpectrumMeter<'a> {
    levels: &'a [f32; BAND_COUNT],
    peaks: &'a [f32; BAND_COUNT],
    selected: usize,
}

impl Widget for SpectrumMeter<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.width < BAND_COUNT as u16 * 3 || area.height < 3 {
            return;
        }
        let label_y = area.bottom() - 1;
        let meter_height = area.height - 1;
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, BAND_COUNT as u32); BAND_COUNT])
            .split(Rect::new(area.x, area.y, area.width, meter_height));

        for (index, column) in columns.iter().copied().enumerate() {
            let inset = u16::from(column.width >= 4);
            let left = column.x + inset;
            let right = column.right().saturating_sub(inset);
            let level = self.levels[index].clamp(SPECTRUM_FLOOR_DB, 0.0);
            let peak = self.peaks[index].clamp(SPECTRUM_FLOOR_DB, 0.0);
            let lit_segments = db_to_segment(level, meter_height, true);
            let peak_segment = db_to_segment(peak, meter_height, false);

            for y in area.y..label_y {
                let from_bottom = (label_y - 1 - y) as usize;
                let active = from_bottom < lit_segments;
                let segment_db = SPECTRUM_FLOOR_DB
                    + (from_bottom + 1) as f32 / meter_height as f32 * -SPECTRUM_FLOOR_DB;
                let color = if active {
                    meter_color(segment_db)
                } else {
                    VFD_DIM
                };
                for x in left..right {
                    buffer[(x, y)].set_symbol("▄").set_fg(color);
                }
            }

            if peak > SPECTRUM_FLOOR_DB
                && peak_segment >= lit_segments
                && peak_segment < meter_height as usize
            {
                let peak_y = label_y - 1 - peak_segment as u16;
                let peak_color = meter_color(peak);
                for x in left..right {
                    buffer[(x, peak_y)].set_symbol("▄").set_fg(peak_color);
                }
            }

            let label = format_frequency(BAND_FREQUENCIES[index]);
            let label_x = column.x + column.width.saturating_sub(label.len() as u16) / 2;
            buffer.set_string(
                label_x,
                label_y,
                label,
                Style::default()
                    .fg(if index == self.selected {
                        ACCENT
                    } else {
                        MUTED
                    })
                    .add_modifier(if index == self.selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
        }
    }
}

fn db_to_segment(db: f32, height: u16, round_up: bool) -> usize {
    let normalized = ((db - SPECTRUM_FLOOR_DB) / -SPECTRUM_FLOOR_DB).clamp(0.0, 1.0);
    let segments = normalized * height as f32;
    if round_up {
        segments.ceil() as usize
    } else {
        segments.round().min(height.saturating_sub(1) as f32) as usize
    }
}

impl Widget for Fader {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.width < 3 || area.height < 6 {
            return;
        }
        let label_rows = 2;
        let track_top = area.y;
        let track_bottom = area.bottom().saturating_sub(label_rows + 1);
        let center = area.x + area.width / 2;
        let (zero_y, _) = fader_position(0.0, track_top, track_bottom);
        for x in area.x..area.right() {
            buffer[(x, zero_y)].set_symbol("─").set_fg(FAINT);
        }
        for y in track_top..=track_bottom {
            buffer[(center, y)]
                .set_symbol(if y == zero_y { "┼" } else { "│" })
                .set_fg(if self.enabled { BORDER } else { FAINT });
        }
        let (knob_y, knob_segment) = fader_position(self.gain, track_top, track_bottom);
        let knob_width = if area.width >= 6 { 4 } else { 2 };
        let knob_x = center.saturating_sub(knob_width as u16 / 2);
        buffer.set_string(
            knob_x,
            knob_y,
            "━".repeat(knob_width),
            Style::default()
                .fg(if !self.enabled {
                    FAINT
                } else if self.selected {
                    ACCENT
                } else {
                    Color::White
                })
                .add_modifier(Modifier::BOLD),
        );
        let marker_x = if knob_x + (knob_width as u16) < area.right() {
            knob_x + knob_width as u16
        } else {
            knob_x.saturating_sub(1)
        };
        buffer[(marker_x, knob_y)]
            .set_symbol(knob_segment)
            .set_fg(if self.selected { ACCENT } else { MUTED });
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

fn fader_position(gain: f32, track_top: u16, track_bottom: u16) -> (u16, &'static str) {
    const SUBSTEPS: [&str; 4] = ["⣀", "⠤", "⠒", "⠉"];
    const GAIN_STEP_DB: f32 = 0.5;
    const STEPS_PER_ROW: usize = 4;

    let total_steps = ((MAX_GAIN_DB - MIN_GAIN_DB) / GAIN_STEP_DB).round() as usize;
    let step =
        ((gain.clamp(MIN_GAIN_DB, MAX_GAIN_DB) - MIN_GAIN_DB) / GAIN_STEP_DB).round() as usize;
    let row_count = total_steps / STEPS_PER_ROW;
    let coarse_row = step / STEPS_PER_ROW;
    let travel = track_bottom.saturating_sub(track_top);
    let rows_up = ((coarse_row as f32 / row_count as f32) * travel as f32).round() as u16;
    let phase = if step == total_steps {
        STEPS_PER_ROW - 1
    } else {
        step % STEPS_PER_ROW
    };

    (track_bottom.saturating_sub(rows_up), SUBSTEPS[phase])
}

fn master_fader_position(gain: f32, track_top: u16, track_bottom: u16) -> (u16, &'static str) {
    const SUBSTEPS: [&str; 4] = ["⣀", "⠤", "⠒", "⠉"];
    const STEPS_PER_ROW: usize = 4;

    let total_steps = (MAX_MASTER_DB - MIN_MASTER_DB) as usize;
    let step = (gain.clamp(MIN_MASTER_DB, MAX_MASTER_DB) - MIN_MASTER_DB).round() as usize;
    let row_count = total_steps / STEPS_PER_ROW;
    let coarse_row = step / STEPS_PER_ROW;
    let travel = track_bottom.saturating_sub(track_top);
    let rows_up = ((coarse_row as f32 / row_count as f32) * travel as f32).round() as u16;
    let phase = if step == total_steps {
        STEPS_PER_ROW - 1
    } else {
        step % STEPS_PER_ROW
    };

    (track_bottom.saturating_sub(rows_up), SUBSTEPS[phase])
}

fn meter_color(db: f32) -> Color {
    if db > -3.0 {
        ACCENT
    } else if db > -9.0 {
        AMBER
    } else {
        VFD
    }
}

fn center_text(buffer: &mut Buffer, area: Rect, y: u16, text: &str, color: Color) {
    let x = area.x + area.width.saturating_sub(text.len() as u16) / 2;
    buffer.set_string(x, y, text, Style::default().fg(color));
}

fn truncate(value: &str, width: usize) -> String {
    let length = value.chars().count();
    if length <= width {
        return value.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut shortened = value.chars().take(width - 3).collect::<String>();
    shortened.push_str("...");
    shortened
}

fn session_field(label: &str, value: &str, width: u16) -> Line<'static> {
    let gap = usize::from(width).saturating_sub(label.len() + value.len() + 1);
    Line::from(vec![
        Span::styled(format!(" {label}"), Style::default().fg(MUTED)),
        Span::raw(" ".repeat(gap)),
        Span::styled(value.to_string(), Style::default().bold()),
    ])
}

fn session_status(
    indicator: &str,
    label: &str,
    key: &str,
    color: Color,
    width: u16,
) -> Line<'static> {
    let used = indicator.chars().count() + label.chars().count() + key.chars().count() + 2;
    let gap = usize::from(width).saturating_sub(used);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(indicator.to_string(), Style::default().fg(color)),
        Span::raw(format!(" {label}")),
        Span::raw(" ".repeat(gap)),
        Span::styled(key.to_string(), Style::default().fg(MUTED)),
    ])
}

fn panel<'a>(title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .style(Style::default().bg(PANEL))
}

fn dialog_panel(title: &str) -> Block<'_> {
    panel(title, true).padding(Padding::horizontal(1))
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
    fn fader_moves_visually_for_every_half_db_step() {
        let positions = (0..=48)
            .map(|step| {
                let gain = MIN_GAIN_DB + step as f32 * 0.5;
                fader_position(gain, 10, 22)
            })
            .collect::<Vec<_>>();

        assert!(positions.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn master_fader_moves_visually_for_every_db_step() {
        let positions = (0..=60)
            .map(|step| master_fader_position(MIN_MASTER_DB + step as f32, 10, 25))
            .collect::<Vec<_>>();

        assert!(positions.windows(2).all(|pair| pair[0] != pair[1]));
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
            if width == 120 {
                let buffer = terminal.backend().buffer();
                let mut text = String::new();
                for y in 0..buffer.area.height {
                    for x in 0..buffer.area.width {
                        text.push_str(buffer[(x, y)].symbol());
                    }
                }
                assert!(text.contains("\\_ TORROEQ _/"));
                assert!(text.contains("Test output"));
                assert!(text.contains("MASTER"));
                assert!(text.contains("VOLUME"));
                assert!(text.contains("LEVEL"));
            }

            app.view = ViewMode::Focus;
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            assert!(app.hit_regions.bands[app.selected_band].height > 0);
        }
    }
}
