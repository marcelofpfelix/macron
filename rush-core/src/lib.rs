use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Health {
    Ok,
    Warning,
    Critical,
    Unknown,
}

impl Health {
    pub fn from_exit_code(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Warning,
            2.. => Self::Critical,
            _ => Self::Unknown,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warn",
            Self::Critical => "crit",
            Self::Unknown => "unk",
        }
    }

    pub fn style(self) -> Style {
        match self {
            Self::Ok => Style::default(),
            Self::Warning => Style::default().fg(Color::Yellow),
            Self::Critical => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            Self::Unknown => Style::default().fg(Color::Gray),
        }
    }
}

impl fmt::Display for Health {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Critical => "critical",
            Self::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusItem {
    pub name: String,
    pub health: Health,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl StatusItem {
    pub fn new(name: impl Into<String>, health: Health, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            health,
            text: text.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceFormat {
    Plain,
    Terminal,
    TerminalPlain,
    Tmux,
    Quickshell,
}

pub fn render_status_line(items: &[StatusItem], format: SurfaceFormat) -> String {
    let parts = items
        .iter()
        .filter(|item| !item.text.trim().is_empty())
        .map(|item| render_status_item(item, format))
        .collect::<Vec<_>>();
    parts.join(" ")
}

pub fn render_status_item(item: &StatusItem, format: SurfaceFormat) -> String {
    let text = strip_quickshell_markup(&item.text);
    match format {
        SurfaceFormat::Plain => format!("{} {}", item.health.icon(), text),
        SurfaceFormat::Terminal => match item.health {
            Health::Ok | Health::Unknown => text,
            Health::Warning => format!("\x1b[33m{text}\x1b[0m"),
            Health::Critical => format!("\x1b[1;31m{text}\x1b[0m"),
        },
        SurfaceFormat::TerminalPlain => text,
        SurfaceFormat::Tmux => match item.health {
            Health::Ok | Health::Unknown => text,
            Health::Warning => format!("#[fg=yellow]{text}#[default]"),
            Health::Critical => format!("#[fg=red]{text}#[default]"),
        },
        SurfaceFormat::Quickshell => {
            if item.text.contains("<span") {
                return item.text.clone();
            }
            match item.health {
                Health::Ok | Health::Unknown => html_escape(&item.text),
                Health::Warning => format!(
                    "<span style=\"color:#f9e2af\">{}</span>",
                    html_escape(&item.text)
                ),
                Health::Critical => format!(
                    "<span style=\"color:#f38ba8\">{}</span>",
                    html_escape(&item.text)
                ),
            }
        }
    }
}

pub fn strip_quickshell_markup(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("<span") {
        output.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            output.push_str(&rest[start..]);
            return output.replace("</span>", "");
        };
        rest = &rest[start + end + 1..];
    }
    output.push_str(rest);
    output.replace("</span>", "")
}

pub fn normalize_key(input: &str) -> String {
    input
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(normalize_key_part)
        .collect::<Vec<_>>()
        .join("+")
}

fn normalize_key_part(part: &str) -> String {
    match part.to_ascii_lowercase().as_str() {
        "super" | "mod4" | "cmd" | "win" => "Super".to_string(),
        "shift" => "S".to_string(),
        "ctrl" | "control" => "C".to_string(),
        "alt" | "mod1" => "A".to_string(),
        "return" | "enter" => "Return".to_string(),
        "escape" | "esc" => "Esc".to_string(),
        "space" => "Space".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "Backspace".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        _ if part.len() == 1 => part.to_ascii_uppercase(),
        _ => {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

pub fn render_tui_panel(title: &str, rows: &[String], width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(_) => return String::new(),
    };
    let _ = terminal.draw(|frame| {
        let items = rows
            .iter()
            .map(|row| ListItem::new(row.as_str()))
            .collect::<Vec<_>>();
        let list = List::new(items).block(Block::default().title(title).borders(Borders::ALL));
        frame.render_widget(list, frame.area());
    });

    let buffer = terminal.backend().buffer();
    let mut lines = Vec::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tmux_status() {
        let item = StatusItem::new("cpu", Health::Warning, " 91");
        assert_eq!(
            render_status_item(&item, SurfaceFormat::Tmux),
            "#[fg=yellow] 91#[default]"
        );

        let ok = StatusItem::new("cpu", Health::Ok, " 15");
        assert_eq!(render_status_item(&ok, SurfaceFormat::Tmux), " 15");
    }

    #[test]
    fn escapes_quickshell_plain_text() {
        let item = StatusItem::new("x", Health::Critical, "a < b");
        assert_eq!(
            render_status_item(&item, SurfaceFormat::Quickshell),
            "<span style=\"color:#f38ba8\">a &lt; b</span>"
        );

        let ok = StatusItem::new("x", Health::Ok, "a < b");
        assert_eq!(
            render_status_item(&ok, SurfaceFormat::Quickshell),
            "a &lt; b"
        );
    }

    #[test]
    fn preserves_existing_quickshell_spans() {
        let item = StatusItem::new(
            "x",
            Health::Critical,
            "<span style=\"color:#f38ba8\">x</span>",
        );
        assert_eq!(
            render_status_item(&item, SurfaceFormat::Quickshell),
            "<span style=\"color:#f38ba8\">x</span>"
        );
    }

    #[test]
    fn renders_terminal_colors_without_quickshell_markup() {
        let item = StatusItem::new(
            "x",
            Health::Critical,
            "<span style=\"color:#fff\">failed</span>",
        );
        assert_eq!(
            render_status_item(&item, SurfaceFormat::Terminal),
            "\x1b[1;31mfailed\x1b[0m"
        );
    }

    #[test]
    fn renders_terminal_text_without_colors() {
        let item = StatusItem::new(
            "x",
            Health::Critical,
            "<span style=\"color:#fff\">failed</span>",
        );
        assert_eq!(
            render_status_item(&item, SurfaceFormat::TerminalPlain),
            "failed"
        );
    }

    #[test]
    fn normalizes_common_key_names() {
        assert_eq!(normalize_key("SUPER + SHIFT + RETURN"), "Super+S+Return");
    }

    #[test]
    fn renders_tui_panel_with_title_and_rows() {
        let panel = render_tui_panel("board", &["  1".to_string()], 20, 5);
        assert!(panel.contains("board"));
        assert!(panel.contains("  1"));
    }
}
