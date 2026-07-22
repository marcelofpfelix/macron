use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use rush_core::{normalize_key, render_tui_panel};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DEFAULT_DOTFILES: &str = "/home/marcelof/gwt/marcelofpfelix/dotfiles/main";

#[derive(Debug, Parser)]
#[command(author, version, about = "Search local keybindings and cheatsheets")]
struct Cli {
    /// Dotfiles checkout to index.
    #[arg(long, default_value = DEFAULT_DOTFILES, global = true)]
    dotfiles: PathBuf,

    /// Limit the index to one app: vim, mux, or hyp.
    #[arg(long, value_enum, global = true)]
    app: Option<App>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table, global = true)]
    format: OutputFormat,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum App {
    /// Neovim keymaps.
    Vim,
    /// tmux key bindings.
    Mux,
    /// Hyprland key bindings.
    Hyp,
}

impl App {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "vim" | "nvim" | "neovim" => Some(Self::Vim),
            "mux" | "tmux" => Some(Self::Mux),
            "hyp" | "hypr" | "hyprland" => Some(Self::Hyp),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Vim => "vim",
            Self::Mux => "mux",
            Self::Hyp => "hyp",
        }
    }

    fn matches(self, binding: &Binding) -> bool {
        binding.app == self.label()
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// List indexed bindings.
    List { app_name: Option<String> },
    /// Fuzzy-find indexed bindings with fzf when available.
    Find { app_name: Option<String> },
    /// Explain one key across indexed apps.
    Explain {
        key: String,
        app_name: Option<String>,
    },
    /// Report duplicate keys within each scope/profile/mode.
    Conflicts,
    /// Dump current binding index as JSON.
    Dump,
    /// Render a compact Ratatui cheatsheet view.
    Tui { key: Option<String> },
    /// Show one legacy YAML cheatsheet from dotfiles/desktop/che.
    Sheet { name: String },
    /// App alias such as `che vim`, `che mux`, or `che hyp`.
    #[command(external_subcommand)]
    LegacySheet(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Binding {
    app: String,
    scope: String,
    profile: String,
    mode: String,
    key: String,
    original_key: String,
    action: String,
    source: String,
    line: usize,
    origin: String,
    confidence: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let bindings = index_bindings(&cli.dotfiles)?;

    match cli.command.unwrap_or(Commands::Find { app_name: None }) {
        Commands::List { app_name } => {
            let filtered = filter_bindings(
                &bindings,
                cli.app.or_else(|| app_from_arg(app_name.as_deref())),
            );
            print_bindings(&filtered, cli.format);
            Ok(())
        }
        Commands::Find { app_name } => {
            let filtered = filter_bindings(
                &bindings,
                cli.app.or_else(|| app_from_arg(app_name.as_deref())),
            );
            fuzzy_or_table(&filtered)
        }
        Commands::Explain { key, app_name } => {
            let key = normalize_key(&key);
            let filtered = filter_bindings(
                &bindings,
                cli.app.or_else(|| app_from_arg(app_name.as_deref())),
            );
            let matches = filtered
                .into_iter()
                .filter(|binding| binding.key == key || binding.original_key == key)
                .collect::<Vec<_>>();
            print_bindings(&matches, cli.format);
            Ok(())
        }
        Commands::Conflicts => {
            let filtered = filter_bindings(&bindings, cli.app);
            print_conflicts(&filtered, cli.format)
        }
        Commands::Dump => {
            let filtered = filter_bindings(&bindings, cli.app);
            print_bindings(&filtered, OutputFormat::Json);
            Ok(())
        }
        Commands::Tui { key } => {
            let filtered = filter_bindings(&bindings, cli.app);
            let rows = binding_rows(match key {
                Some(key) => {
                    let normalized = normalize_key(&key);
                    filtered
                        .iter()
                        .filter(|binding| {
                            binding.key == normalized || binding.original_key == normalized
                        })
                        .collect::<Vec<_>>()
                }
                None => filtered.iter().take(20).collect::<Vec<_>>(),
            });
            println!("{}", render_tui_panel("che", &rows, 100, 24));
            Ok(())
        }
        Commands::Sheet { name } => print_sheet(&cli.dotfiles, &name),
        Commands::LegacySheet(args) => match args.first() {
            Some(name) if App::from_name(name).is_some() => {
                let filtered = filter_bindings(&bindings, App::from_name(name));
                fuzzy_or_table(&filtered)
            }
            Some(name) => print_sheet(&cli.dotfiles, name),
            None => {
                let filtered = filter_bindings(&bindings, cli.app);
                fuzzy_or_table(&filtered)
            }
        },
    }
}

fn app_from_arg(app: Option<&str>) -> Option<App> {
    app.and_then(App::from_name)
}

fn filter_bindings(bindings: &[Binding], app: Option<App>) -> Vec<Binding> {
    bindings
        .iter()
        .filter(|binding| app.is_none_or(|app| app.matches(binding)))
        .cloned()
        .collect()
}

fn print_sheet(dotfiles: &Path, name: &str) -> Result<()> {
    let path = dotfiles.join("desktop/che").join(format!("{name}.yml"));
    if !path.exists() {
        anyhow::bail!("cheatsheet not found: {}", path.display());
    }

    if command_exists("bat") {
        let status = Command::new("bat").arg(&path).status().context("run bat")?;
        if status.success() {
            return Ok(());
        }
    }

    print!(
        "{}",
        fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?
    );
    Ok(())
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn index_bindings(dotfiles: &Path) -> Result<Vec<Binding>> {
    let mut bindings = Vec::new();
    parse_tmux(dotfiles, &mut bindings)?;
    parse_hyprland(dotfiles, &mut bindings)?;
    parse_neovim(dotfiles, &mut bindings)?;
    bindings.sort_by(|a, b| {
        (&a.app, &a.profile, &a.mode, &a.key, &a.action)
            .cmp(&(&b.app, &b.profile, &b.mode, &b.key, &b.action))
    });
    Ok(bindings)
}

fn parse_tmux(dotfiles: &Path, bindings: &mut Vec<Binding>) -> Result<()> {
    let path = dotfiles.join("desktop/.tmux.conf");
    let text = read_optional(&path)?;
    let mut prefix = "C-b".to_string();

    let mut pending_comment: Option<String> = None;
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let raw = line.trim();
        if let Some(comment) = leading_comment(raw, '#') {
            pending_comment = clean_comment(comment);
            continue;
        }
        let trimmed = strip_comment(line).trim();
        if trimmed.is_empty() {
            pending_comment = None;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("set -g prefix ") {
            prefix = rest.trim_matches('\'').trim_matches('"').to_string();
            continue;
        }

        if !(trimmed.starts_with("bind ") || trimmed.starts_with("bind-key ")) {
            continue;
        }

        let parts = shell_words(trimmed);
        if parts.len() < 3 {
            continue;
        }

        let mut idx = 1;
        let mut mode = "prefix".to_string();
        let mut uses_prefix = true;
        while idx < parts.len() && parts[idx].starts_with('-') {
            match parts[idx].as_str() {
                "-n" => {
                    uses_prefix = false;
                    mode = "global".to_string();
                    idx += 1;
                }
                "-T" if idx + 1 < parts.len() => {
                    mode = parts[idx + 1].clone();
                    idx += 2;
                }
                _ => idx += 1,
            }
        }
        if idx >= parts.len() {
            continue;
        }

        let original_key = if uses_prefix {
            format!("{} {}", prefix, parts[idx])
        } else {
            parts[idx].clone()
        };
        let raw_action = parts.get(idx + 1..).unwrap_or_default().join(" ");
        let action = pending_comment
            .take()
            .unwrap_or_else(|| explain_action(&raw_action));
        push_binding(
            bindings,
            BindingParts {
                scope: "tmux",
                profile: "global",
                mode: &mode,
                key: &original_key,
                action: &action,
                source: &path,
                line: line_no,
                confidence: "parsed",
            },
        );
    }

    Ok(())
}

fn parse_hyprland(dotfiles: &Path, bindings: &mut Vec<Binding>) -> Result<()> {
    let dir = dotfiles.join("desktop/.config/hypr/profiles");
    for profile in ["default", "modern"] {
        let path = dir.join(format!("{profile}.lua"));
        let text = read_optional(&path)?;
        let mut pending_comment: Option<String> = None;
        for (idx, line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = line.trim();
            if let Some(comment) = leading_comment(trimmed, '-') {
                pending_comment = clean_comment(comment);
                continue;
            }
            if trimmed.is_empty() {
                pending_comment = None;
                continue;
            }
            if trimmed.starts_with("local function") {
                continue;
            }
            if !trimmed.contains("hl.bind(") && !trimmed.contains("bind_app(") {
                continue;
            }
            let Some(key_expr) = first_lua_arg(trimmed) else {
                continue;
            };
            let key = normalize_lua_key_expr(&key_expr);
            if key == "keys" {
                continue;
            }
            let action = pending_comment
                .take()
                .unwrap_or_else(|| explain_action(trimmed));
            if key.contains("key") {
                for digit in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"] {
                    let expanded = key.replace("key", digit);
                    push_binding(
                        bindings,
                        BindingParts {
                            scope: "hyprland",
                            profile,
                            mode: "global",
                            key: &expanded,
                            action: &action,
                            source: &path,
                            line: line_no,
                            confidence: "inferred",
                        },
                    );
                }
            } else {
                push_binding(
                    bindings,
                    BindingParts {
                        scope: "hyprland",
                        profile,
                        mode: "global",
                        key: &key,
                        action: &action,
                        source: &path,
                        line: line_no,
                        confidence: "parsed",
                    },
                );
            }
        }
    }
    Ok(())
}

fn parse_neovim(dotfiles: &Path, bindings: &mut Vec<Binding>) -> Result<()> {
    let dir = dotfiles.join("desktop/.config/nvim");
    if !dir.exists() {
        return Ok(());
    }
    for path in lua_files(&dir)? {
        let text = read_optional(&path)?;
        let mut pending_comment: Option<String> = None;
        for (idx, line) in text.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = line.trim();
            if let Some(comment) = leading_comment(trimmed, '-') {
                pending_comment = clean_comment(comment);
                continue;
            }
            if trimmed.is_empty() {
                pending_comment = None;
                continue;
            }
            if !(trimmed.starts_with("vim.keymap.set")
                || trimmed.starts_with("vim.api.nvim_set_keymap"))
            {
                continue;
            }
            let args = split_lua_args(trimmed);
            let inline_comment = trailing_lua_comment(trimmed).and_then(clean_comment);
            let (mode, key, raw_action) = if trimmed.contains("vim.api.nvim_set_keymap") {
                if args.len() < 3 {
                    continue;
                }
                let Some(key) = first_quoted(&args[1]) else {
                    continue;
                };
                (
                    first_quoted(&args[0]).unwrap_or_else(|| args[0].clone()),
                    key,
                    first_quoted(&args[2]).unwrap_or_else(|| args[2].clone()),
                )
            } else {
                if args.len() < 2 {
                    continue;
                }
                let Some(key) = first_quoted(&args[1]) else {
                    continue;
                };
                let modes = quoted_strings(&args[0]);
                let mode = if modes.is_empty() {
                    args[0].trim().to_string()
                } else {
                    modes.join(",")
                };
                let action = args
                    .get(2)
                    .and_then(|arg| first_quoted(arg))
                    .unwrap_or_else(|| args.get(2).cloned().unwrap_or_else(|| trimmed.to_string()));
                (mode, key, action)
            };
            let action = keymap_desc(trimmed)
                .or(inline_comment)
                .or_else(|| pending_comment.take())
                .unwrap_or_else(|| explain_action(&raw_action));
            push_binding(
                bindings,
                BindingParts {
                    scope: "neovim",
                    profile: "global",
                    mode: &mode,
                    key: &key,
                    action: &action,
                    source: &path,
                    line: line_no,
                    confidence: "parsed",
                },
            );
        }
    }
    Ok(())
}

struct BindingParts<'a> {
    scope: &'a str,
    profile: &'a str,
    mode: &'a str,
    key: &'a str,
    action: &'a str,
    source: &'a Path,
    line: usize,
    confidence: &'a str,
}

fn push_binding(bindings: &mut Vec<Binding>, parts: BindingParts<'_>) {
    bindings.push(Binding {
        app: app_for_scope(parts.scope).to_string(),
        scope: parts.scope.to_string(),
        profile: parts.profile.to_string(),
        mode: parts.mode.to_string(),
        key: normalize_key(parts.key),
        original_key: parts.key.to_string(),
        action: parts.action.to_string(),
        source: parts.source.display().to_string(),
        line: parts.line,
        origin: "current".to_string(),
        confidence: parts.confidence.to_string(),
    });
}

fn binding_rows(bindings: Vec<&Binding>) -> Vec<String> {
    bindings
        .into_iter()
        .map(|binding| format!("{:<18} {}", binding.original_key, binding.action))
        .collect()
}

fn print_bindings(bindings: &[Binding], format: OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(bindings).expect("serialize bindings")
        ),
        OutputFormat::Table => print_grouped_bindings(bindings),
    }
}

fn fuzzy_or_table(bindings: &[Binding]) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        print_grouped_bindings(bindings);
        return Ok(());
    }

    let table = bindings
        .iter()
        .map(|binding| {
            format!(
                "{:<9} {:<8} {:<12} {:<18} {}",
                binding.scope, binding.profile, binding.mode, binding.original_key, binding.action
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut child = match Command::new("fzf")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            println!("{table}");
            return Ok(());
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(table.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

fn print_conflicts(bindings: &[Binding], format: OutputFormat) -> Result<()> {
    let mut groups: BTreeMap<(&str, &str, &str, &str), Vec<&Binding>> = BTreeMap::new();
    for binding in bindings {
        groups
            .entry((&binding.app, &binding.profile, &binding.mode, &binding.key))
            .or_default()
            .push(binding);
    }
    let conflicts = groups
        .into_values()
        .filter(|items| items.len() > 1)
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    print_bindings(&conflicts, format);
    Ok(())
}

fn read_optional(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in input.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'') | (None, '"') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (_, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn app_for_scope(scope: &str) -> &'static str {
    match scope {
        "neovim" => "vim",
        "tmux" => "mux",
        "hyprland" => "hyp",
        _ => "other",
    }
}

fn print_grouped_bindings(bindings: &[Binding]) {
    let mut current: Option<(&str, &str, &str)> = None;
    for binding in bindings {
        let group = (
            binding.app.as_str(),
            binding.profile.as_str(),
            binding.mode.as_str(),
        );
        if current != Some(group) {
            if current.is_some() {
                println!();
            }
            println!("{} / {} / {}", binding.app, binding.profile, binding.mode);
            current = Some(group);
        }
        println!("  {:<18} {}", binding.original_key, binding.action);
    }
}

fn leading_comment(line: &str, marker: char) -> Option<&str> {
    match marker {
        '#' => line.strip_prefix('#').map(str::trim),
        '-' => line.strip_prefix("--").map(str::trim),
        _ => None,
    }
}

fn clean_comment(comment: &str) -> Option<String> {
    let comment = comment.trim();
    if comment.is_empty() || comment.starts_with("http://") || comment.starts_with("https://") {
        None
    } else {
        Some(comment.to_string())
    }
}

fn trailing_lua_comment(line: &str) -> Option<&str> {
    line.split_once("--").map(|(_, comment)| comment.trim())
}

fn keymap_desc(line: &str) -> Option<String> {
    let desc_idx = line.find("desc")?;
    let after = &line[desc_idx..];
    let (_, value) = after.split_once('=')?;
    first_quoted(value).or_else(|| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.trim_end_matches('}').trim().to_string())
    })
}

fn explain_action(action: &str) -> String {
    let action = action.trim();
    for prefix in ["sh(", "note("] {
        if let Some(rest) = action.strip_prefix(prefix) {
            return rest
                .trim_end_matches(')')
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
        }
    }
    action
        .replace("hl.dsp.", "")
        .replace("vim.cmd.", "")
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn first_lua_arg(line: &str) -> Option<String> {
    let start = line.find('(')? + 1;
    let mut depth = 0usize;
    let mut quote = None;
    let mut out = String::new();
    for ch in line[start..].chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'') | (None, '"') => quote = Some(ch),
            (None, '{') | (None, '(') => {
                depth += 1;
                out.push(ch);
            }
            (None, '}') | (None, ')') if depth > 0 => {
                depth -= 1;
                out.push(ch);
            }
            (None, ',') if depth == 0 => break,
            (_, c) => out.push(c),
        }
    }
    Some(out.trim().to_string())
}

fn normalize_lua_key_expr(expr: &str) -> String {
    expr.replace("mod .. ", "Super")
        .replace(" .. ", "")
        .replace(['"', '\''], "")
        .replace(" + ", "+")
}

fn quoted_strings(line: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some(_), '\\') => escaped = true,
            (Some(q), c) if c == q => {
                strings.push(std::mem::take(&mut current));
                quote = None;
            }
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => quote = Some(ch),
            _ => {}
        }
    }
    strings
}

fn split_lua_args(line: &str) -> Vec<String> {
    let Some(start) = line.find('(') else {
        return Vec::new();
    };
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;

    for ch in line[start + 1..].chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some(_), '\\') => {
                current.push(ch);
                escaped = true;
            }
            (Some(q), c) if c == q => {
                current.push(ch);
                quote = None;
            }
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => {
                current.push(ch);
                quote = Some(ch);
            }
            (None, '{') | (None, '(') | (None, '[') => {
                depth += 1;
                current.push(ch);
            }
            (None, '}') | (None, ')') | (None, ']') if depth > 0 => {
                depth -= 1;
                current.push(ch);
            }
            (None, ')') if depth == 0 => {
                if !current.trim().is_empty() {
                    args.push(current.trim().to_string());
                }
                break;
            }
            (None, ',') if depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            (None, c) => current.push(c),
        }
    }

    args
}

fn first_quoted(line: &str) -> Option<String> {
    quoted_strings(line).into_iter().next()
}

fn lua_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_lua_files(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_lua_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_lua_files(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("lua") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_bind_words() {
        let words = shell_words("bind -T copy-mode-vi 'C-h' select-pane -L");
        assert_eq!(
            words,
            vec!["bind", "-T", "copy-mode-vi", "C-h", "select-pane", "-L"]
        );
    }

    #[test]
    fn parses_lua_first_argument() {
        assert_eq!(
            first_lua_arg("hl.bind(mod .. \" + SHIFT + Q\", hl.dsp.window.close())"),
            Some("mod ..  + SHIFT + Q".to_string())
        );
    }

    #[test]
    fn extracts_quoted_strings() {
        assert_eq!(
            quoted_strings("vim.keymap.set(\"n\", \"<leader>qq\", \"<cmd>qa<cr>\")"),
            vec!["n", "<leader>qq", "<cmd>qa<cr>"]
        );
    }

    #[test]
    fn splits_lua_keymap_arguments() {
        assert_eq!(
            split_lua_args("vim.keymap.set({ 'n', 'v' }, '<Space>', '<Nop>', { silent = true })"),
            vec!["{ 'n', 'v' }", "'<Space>'", "'<Nop>'", "{ silent = true }"]
        );
    }
}
