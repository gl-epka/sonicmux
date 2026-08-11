use std::{borrow::Cow, time::Duration};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Tabs, Wrap},
};
use sonicmux_core::{OutputStreamPlan, PlanOutcome, StreamInfo};

use crate::model::{AppPhase, Model, Overlay, QueueItem, QueueStatus, Screen};

const MIN_WIDTH: u16 = 50;
const MIN_HEIGHT: u16 = 12;
const WIDE_WIDTH: u16 = 110;
const MEDIUM_WIDTH: u16 = 76;

pub(crate) fn render(frame: &mut Frame<'_>, model: &Model) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(frame, area);
        return;
    }
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area);
    render_header(frame, vertical[0], model);
    render_tabs(frame, vertical[1], model);
    match model.screen {
        Screen::Queue => render_queue_screen(frame, vertical[2], model),
        Screen::Tracks => render_tracks(frame, vertical[2], model),
        Screen::Logs => render_logs(frame, vertical[2], model),
        Screen::Settings => render_settings(frame, vertical[2], model),
    }
    render_footer(frame, vertical[3], model);
    if let Some(overlay) = &model.overlay {
        render_overlay(frame, area, overlay, model.color);
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let phase = match model.phase {
        AppPhase::Idle => "idle",
        AppPhase::Running => "running",
        AppPhase::Cancelling => "cancelling",
    };
    let settings = &model.settings;
    let title = Line::from(vec![
        Span::styled(
            " SonicMux ",
            primary(model.color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "{} · {} {} · jobs {}",
            settings.profile_name(),
            settings.codec.to_ascii_uppercase(),
            settings.bitrate,
            settings.jobs
        )),
        Span::raw("    "),
        Span::styled(phase, phase_style(model.phase, model.color)),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let titles = ["1 Queue", "2 Tracks", "3 Logs", "4 Settings"]
        .into_iter()
        .map(Line::from);
    let tabs = Tabs::new(titles)
        .select(model.screen.index())
        .style(muted(model.color))
        .highlight_style(primary(model.color).add_modifier(Modifier::BOLD))
        .divider(" │ ");
    frame.render_widget(tabs, area);
}

fn render_queue_screen(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    if area.width >= WIDE_WIDTH {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_queue(frame, horizontal[0], model);
        render_tracks(frame, horizontal[1], model);
    } else {
        render_queue(frame, area, model);
    }
}

fn render_queue(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    if model.queue.is_empty() {
        let empty = Paragraph::new(Text::from(vec![
            Line::from("No MKV files in the queue."),
            Line::from(""),
            Line::from("Press a to add a file, directory, or glob pattern."),
        ]))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(focused_block("Queue", model.color));
        frame.render_widget(empty, area);
        return;
    }
    let compact = area.width < MEDIUM_WIDTH;
    let rows = model
        .queue
        .iter()
        .map(|item| queue_row(item, compact, model.color));
    let widths: Vec<Constraint> = if compact {
        vec![
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(8),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(18),
            Constraint::Length(7),
            Constraint::Length(9),
        ]
    };
    let headers: Vec<&str> = if compact {
        vec!["Use", "State", "File"]
    } else {
        vec!["Use", "State", "File", "Done", "ETA"]
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(headers)
                .style(secondary(model.color).add_modifier(Modifier::BOLD))
                .bottom_margin(1),
        )
        .row_highlight_style(selection(model.color))
        .highlight_symbol("> ")
        .block(focused_block("Queue", model.color));
    let mut state = TableState::default().with_selected(model.selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn queue_row(item: &QueueItem, compact: bool, color: bool) -> Row<'static> {
    let enabled = if item.enabled { "[x]" } else { "[ ]" };
    let filename = item
        .input
        .file_name()
        .map_or_else(
            || item.input.to_string_lossy(),
            |value| value.to_string_lossy(),
        )
        .into_owned();
    let mut cells = vec![
        Cell::from(enabled.to_owned()),
        Cell::from(item.status.label().to_owned()).style(status_style(item.status, color)),
        Cell::from(filename),
    ];
    if !compact {
        cells.push(Cell::from(item.progress_milli.map_or_else(
            || "--".to_owned(),
            |value| format!("{:>3}%", value / 10),
        )));
        cells.push(Cell::from(
            item.eta.map_or_else(|| "--".to_owned(), format_duration),
        ));
    }
    Row::new(cells)
}

fn render_tracks(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let block = focused_block("Selected file", model.color);
    let Some(item) = model.selected_item() else {
        frame.render_widget(
            Paragraph::new("Select a queue item to inspect its tracks.")
                .alignment(Alignment::Center)
                .block(block),
            area,
        );
        return;
    };
    let Some(media) = &item.media else {
        let message = item.error.as_deref().unwrap_or("Waiting for FFprobe…");
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    };
    let compact = area.width < 58;
    let rows = media.streams().iter().map(|stream| {
        let common = stream.common();
        let (kind, codec, channels) = match stream {
            StreamInfo::Video(_) => ("video", common.codec_name().to_owned(), "-".to_owned()),
            StreamInfo::Audio(audio) => (
                "audio",
                audio.codec().to_string(),
                audio.channels().count().get().to_string(),
            ),
            StreamInfo::Subtitle(_) => ("subtitle", common.codec_name().to_owned(), "-".to_owned()),
            StreamInfo::Attachment(_) => ("attach", common.codec_name().to_owned(), "-".to_owned()),
            StreamInfo::Data(_) => ("data", common.codec_name().to_owned(), "-".to_owned()),
            StreamInfo::Unknown(stream) => (
                stream.kind(),
                common.codec_name().to_owned(),
                "-".to_owned(),
            ),
            _ => ("unknown", common.codec_name().to_owned(), "-".to_owned()),
        };
        let language = common.metadata().get("language").unwrap_or("-");
        let action = planned_action(item.plan.as_ref(), common.index());
        let default = if common.dispositions().is_default() {
            "default"
        } else {
            "-"
        };
        if compact {
            Row::new(vec![
                Cell::from(common.index().to_string()),
                Cell::from(kind.to_owned()),
                Cell::from(codec),
                Cell::from(action),
            ])
        } else {
            Row::new(vec![
                Cell::from(common.index().to_string()),
                Cell::from(kind.to_owned()),
                Cell::from(codec),
                Cell::from(channels),
                Cell::from(language.to_owned()),
                Cell::from(default),
                Cell::from(action),
            ])
        }
    });
    let (headers, widths): (Vec<&str>, Vec<Constraint>) = if compact {
        (
            vec!["#", "Kind", "Codec", "Action"],
            vec![
                Constraint::Length(3),
                Constraint::Length(9),
                Constraint::Min(9),
                Constraint::Length(11),
            ],
        )
    } else {
        (
            vec!["#", "Kind", "Codec", "Ch", "Lang", "Disp", "Action"],
            vec![
                Constraint::Length(3),
                Constraint::Length(9),
                Constraint::Min(10),
                Constraint::Length(3),
                Constraint::Length(6),
                Constraint::Length(8),
                Constraint::Length(11),
            ],
        )
    };
    let table = Table::new(rows, widths)
        .header(Row::new(headers).style(secondary(model.color).add_modifier(Modifier::BOLD)))
        .block(block);
    frame.render_widget(table, area);
}

fn planned_action(plan: Option<&PlanOutcome>, source: sonicmux_core::StreamIndex) -> String {
    match plan {
        Some(PlanOutcome::Skip(_)) => "none".to_owned(),
        Some(PlanOutcome::Execute(plan)) => {
            let mut copy = false;
            let mut encode = false;
            for operation in plan
                .streams()
                .iter()
                .filter(|value| value.source() == source)
            {
                match operation {
                    OutputStreamPlan::Copy { .. } => copy = true,
                    OutputStreamPlan::EncodeAudio { .. } => encode = true,
                    _ => {}
                }
            }
            match (copy, encode) {
                (true, true) => "copy+encode",
                (false, true) => "encode",
                (true, false) => "copy",
                (false, false) => "omit",
            }
            .to_owned()
        }
        _ => "pending".to_owned(),
    }
}

fn render_logs(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let height = area.height.saturating_sub(2) as usize;
    let start = model.logs.len().saturating_sub(height);
    let lines = model
        .logs
        .iter()
        .skip(start)
        .enumerate()
        .map(|(offset, message)| Line::from(format!("{:>4}  {message}", start + offset + 1)))
        .collect::<Vec<_>>();
    let content = if lines.is_empty() {
        Text::from("No events yet. Discovery and batch lifecycle messages appear here.")
    } else {
        Text::from(lines)
    };
    frame.render_widget(
        Paragraph::new(content)
            .wrap(Wrap { trim: false })
            .block(focused_block("Logs", model.color)),
        area,
    );
}

fn render_settings(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let rows = (0..crate::model::UiSettings::field_count()).map(|index| {
        let (name, value) = model.settings.field_label(index);
        Row::new(vec![Cell::from(name), Cell::from(value)])
    });
    let table = Table::new(rows, [Constraint::Length(16), Constraint::Min(12)])
        .header(
            Row::new(["Setting", "Session value"])
                .style(secondary(model.color).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(selection(model.color))
        .highlight_symbol("> ")
        .block(focused_block(
            "Settings (h/l or Enter to change)",
            model.color,
        ));
    let mut state = TableState::default().with_selected(Some(model.settings.selected_field));
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &Model) {
    let progress = model.overall_progress();
    let first = if let Some(value) = progress {
        let ratio = f64::from(value) / 1_000.0;
        Gauge::default()
            .gauge_style(primary(model.color))
            .ratio(ratio)
            .label(format!("Overall {}%", value / 10))
    } else {
        Gauge::default()
            .gauge_style(muted(model.color))
            .ratio(0.0)
            .label("Overall --")
    };
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    frame.render_widget(first, split[0]);
    let help = if area.width >= 80 {
        "a add  d remove  Space enable  s start  c cancel  j/k move  ? help  q quit"
    } else {
        "a add  s start  c cancel  j/k move  ? help"
    };
    frame.render_widget(Paragraph::new(help).style(muted(model.color)), split[1]);
}

fn render_overlay(frame: &mut Frame<'_>, area: Rect, overlay: &Overlay, color: bool) {
    let (width, height, title, content): (u16, u16, &str, Cow<'_, str>) = match overlay {
        Overlay::Help => (
            68,
            18,
            "Help",
            Cow::Borrowed(
                "1–4 / Tab   switch screens\n\
                 j/k, arrows move selection\n\
                 g/G         first / last item\n\
                 a           add file, directory, or glob\n\
                 d           remove selected idle item\n\
                 Space       enable or disable selected item\n\
                 s           start enabled ready items\n\
                 c, Ctrl+C   cancel and wait for cleanup\n\
                 r           retry failed or cancelled items\n\
                 h/l, Enter  change selected setting\n\
                 q           quit when idle\n\
                 Esc         close an overlay",
            ),
        ),
        Overlay::PathEditor { value, cursor } => {
            let mut visible = value.clone();
            visible.insert(*cursor, '|');
            (
                72,
                7,
                "Add MKV input",
                Cow::Owned(format!(
                    "Enter a file, directory, or glob pattern:\n\n{visible}\n\nEnter: discover   Esc: cancel"
                )),
            )
        }
        Overlay::ConfirmCancel => (
            58,
            7,
            "Cancel active batch?",
            Cow::Borrowed(
                "SonicMux will stop admission, terminate FFmpeg, and clean staging files.\n\nEnter/y: cancel batch   n/Esc: keep running",
            ),
        ),
        Overlay::Notice(message) => (
            68,
            8,
            "Action required",
            Cow::Owned(format!("{message}\n\nEnter or Esc: close")),
        ),
    };
    let popup = centered_rect(width.min(area.width.saturating_sub(4)), height, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(content)
            .wrap(Wrap { trim: false })
            .block(focused_block(title, color)),
        popup,
    );
}

fn render_too_small(frame: &mut Frame<'_>, area: Rect) {
    let message = format!(
        "SonicMux needs at least {MIN_WIDTH}x{MIN_HEIGHT} cells.\nCurrent terminal: {}x{}",
        area.width, area.height
    );
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn focused_block(title: &str, color: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(primary(color))
}

fn primary(color: bool) -> Style {
    if color {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn secondary(color: bool) -> Style {
    if color {
        Style::default().fg(Color::White)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

fn muted(color: bool) -> Style {
    if color {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    }
}

fn selection(color: bool) -> Style {
    if color {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
}

fn phase_style(phase: AppPhase, color: bool) -> Style {
    if !color {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    match phase {
        AppPhase::Idle => Style::default().fg(Color::Green),
        AppPhase::Running => Style::default().fg(Color::Cyan),
        AppPhase::Cancelling => Style::default().fg(Color::Yellow),
    }
}

fn status_style(status: QueueStatus, color: bool) -> Style {
    if !color {
        return Style::default();
    }
    match status {
        QueueStatus::Succeeded | QueueStatus::Skipped | QueueStatus::Compatible => {
            Style::default().fg(Color::Green)
        }
        QueueStatus::Failed => Style::default().fg(Color::Red),
        QueueStatus::Cancelled => Style::default().fg(Color::Yellow),
        QueueStatus::Running | QueueStatus::Preparing | QueueStatus::Probing => {
            Style::default().fg(Color::Cyan)
        }
        _ => Style::default().fg(Color::White),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use sonicmux_runtime::{DefaultConfig, DiscoveryRequest, PartialConfig, merge_config};

    use super::*;
    use crate::model::UiSettings;

    fn model() -> Model {
        let config = merge_config(
            DefaultConfig::default(),
            PartialConfig::default(),
            PartialConfig::default(),
            PartialConfig::default(),
        )
        .expect("default config is valid");
        Model::new(
            UiSettings::from_config(&config, false, None).expect("settings are valid"),
            DiscoveryRequest {
                roots: Vec::new(),
                recursive: false,
                follow_links: false,
                includes: Vec::new(),
                excludes: Vec::new(),
            },
            false,
        )
    }

    fn render_text(width: u16, height: u16, model: &Model) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal is created");
        terminal
            .draw(|frame| render(frame, model))
            .expect("frame renders");
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..height {
            for x in 0..width {
                if let Some(cell) = buffer.cell((x, y)) {
                    output.push_str(cell.symbol());
                }
            }
            output.push('\n');
        }
        output
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_and_small_layouts_are_actionable() {
        let model = model();
        let empty = render_text(80, 24, &model);
        assert!(empty.contains("No MKV files in the queue"));
        assert!(empty.contains("Press a to add"));
        let small = render_text(40, 10, &model);
        assert!(small.contains("needs at least 50x12"));
    }

    #[test]
    fn settings_and_help_keep_textual_navigation() {
        let mut model = model();
        model.screen = Screen::Settings;
        model.overlay = Some(Overlay::Help);
        let screen = render_text(100, 28, &model);
        assert!(screen.contains("Session value"));
        assert!(screen.contains("switch screens"));
        assert!(screen.contains("cancel and wait for cleanup"));
    }

    #[test]
    fn responsive_layout_snapshots() {
        let empty = model();
        insta::assert_snapshot!("empty_queue_80x24", render_text(80, 24, &empty));

        let mut compact = model();
        compact.screen = Screen::Settings;
        insta::assert_snapshot!("settings_60x16", render_text(60, 16, &compact));

        let mut help = model();
        help.overlay = Some(Overlay::Help);
        insta::assert_snapshot!("help_120x30", render_text(120, 30, &help));
    }
}
