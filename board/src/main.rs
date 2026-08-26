use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Local, TimeZone};
use chrono_tz::Tz;
use clap::{Parser, Subcommand, ValueEnum};
use rush_core::{
    Health, StatusItem, SurfaceFormat, render_status_line, render_tui_panel,
    strip_quickshell_markup,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::task;
use tokio::time::{MissedTickBehavior, interval, timeout};

mod modules;
use modules::*;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Local async status board for bars and dashboards"
)]
struct Cli {
    /// Config file. Defaults to $XDG_CONFIG_HOME/board/board.toml when it exists.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Cache directory for check results.
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run all checks once, write cache, and print JSON state.
    Once,
    /// Run checks continuously with their configured intervals.
    Run,
    /// Render a configured surface from cached state, refreshing missing or stale checks.
    Render {
        /// Ignore cached state and run selected checks before rendering.
        #[arg(long)]
        fresh: bool,
        /// Keep rendering the surface as a newline-delimited stream.
        #[arg(long)]
        watch: bool,
        /// Output format.
        #[arg(long, value_enum)]
        format: Option<RenderFormat>,
        /// Configured surface name. Run `board surfaces` to list them.
        #[arg(long)]
        surface: Option<String>,
        /// Terminal color policy. Only applies to terminal output.
        #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
        color: ColorMode,
        /// Alias for --color never.
        #[arg(long, conflicts_with = "color")]
        no_color: bool,
        #[arg(value_enum, hide = true)]
        legacy_format: Option<RenderFormat>,
        #[arg(hide = true)]
        legacy_surface: Option<String>,
    },
    /// Run one check and print JSON.
    Check { name: String },
    /// Run an explicit configured action.
    Action {
        /// Action id, for example personal.refresh.
        name: String,
        /// Print the resolved command without running it.
        #[arg(long)]
        dry_run: bool,
        /// Confirm actions marked confirm=true.
        #[arg(long)]
        yes: bool,
    },
    /// List configured checks.
    Checks,
    /// List configured surfaces and their checks.
    Surfaces,
    /// Print Prometheus textfile-compatible metrics from fresh check results.
    Metrics,
    /// Validate config and print resolved runtime paths.
    Doctor,
    /// Render a compact Ratatui board view from cache-first state.
    Tui { surface: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RenderFormat {
    Terminal,
    Text,
    Json,
    Plain,
    Tmux,
    Quickshell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    check: Vec<CheckConfig>,
    #[serde(default)]
    surface: Vec<SurfaceConfig>,
    #[serde(default)]
    actions: BTreeMap<String, BTreeMap<String, ActionConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckConfig {
    name: String,
    #[serde(default)]
    label: Option<String>,
    kind: CheckKind,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default = "default_interval")]
    interval: HumanDuration,
    #[serde(default = "default_timeout")]
    timeout: HumanDuration,
    #[serde(default = "default_timeout_text")]
    timeout_text: String,
    #[serde(default)]
    fresh_for: Option<HumanDuration>,
    command: Option<Vec<String>>,
    #[serde(default)]
    warning: Option<u64>,
    #[serde(default)]
    critical: Option<u64>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    source: Option<PathBuf>,
    #[serde(default)]
    filters: Vec<String>,
    #[serde(default)]
    critical_destination: Option<String>,
    #[serde(default)]
    warning_destination: Option<String>,
    #[serde(default)]
    excluded_channel: Option<String>,
    #[serde(default)]
    timezones: Vec<TimezoneConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimezoneConfig {
    label: String,
    zone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CheckKind {
    NativeCpu,
    NativeMem,
    NativeTemp,
    NativeWeather,
    NativeTimePanel,
    NativeTodoPanel,
    NativeSafe,
    NativeResolv,
    NativeTimeOffset,
    NativeScriptStatus,
    NativeAlertmanager,
    NativeDocker,
    Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SurfaceConfig {
    name: String,
    checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActionConfig {
    #[serde(default)]
    label: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    confirm: bool,
    #[serde(default = "default_timeout")]
    timeout: HumanDuration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckResult {
    item: StatusItem,
    timestamp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cpu_sample: Option<CpuSample>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct CpuSample {
    active: u64,
    idle: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AlertmanagerAlert {
    labels: BTreeMap<String, String>,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn getloadavg(loadavg: *mut f64, nelem: i32) -> i32;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct HumanDuration(Duration);

impl HumanDuration {
    fn as_duration(self) -> Duration {
        self.0
    }
}

impl TryFrom<String> for HumanDuration {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        parse_duration(&value)
            .map(Self)
            .ok_or_else(|| format!("invalid duration: {value}"))
    }
}

impl From<HumanDuration> for String {
    fn from(value: HumanDuration) -> Self {
        format!("{}ms", value.0.as_millis())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.or_else(default_config_path);
    let config = load_config(config_path.as_deref())?;
    let cache_dir = cli.cache_dir.unwrap_or_else(default_cache_dir);

    match cli.command {
        Commands::Once => {
            let results = run_checks(&config.check).await;
            write_cache(&cache_dir, &results)?;
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        Commands::Run => run_loop(&config, &cache_dir).await?,
        Commands::Render {
            fresh,
            watch,
            format,
            surface,
            color,
            no_color,
            legacy_format,
            legacy_surface,
        } => {
            let format = format.or(legacy_format).unwrap_or(RenderFormat::Terminal);
            let surface = surface
                .or(legacy_surface)
                .unwrap_or_else(|| DEFAULT_SURFACE.to_string());
            let color = terminal_color_enabled(color, no_color);
            if watch {
                render_watch(&config, &cache_dir, fresh, format, &surface, color).await?;
            } else {
                println!(
                    "{}",
                    render_output(&config, &cache_dir, fresh, format, &surface, color).await?
                );
            }
        }
        Commands::Check { name } => {
            let check = config
                .check
                .iter()
                .find(|check| check.name == name)
                .with_context(|| format!("unknown check: {name}"))?;
            let result = if check.enabled {
                run_check(check).await
            } else {
                check_result(StatusItem::new(&check.name, Health::Unknown, "disabled"))
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Action { name, dry_run, yes } => run_action(&config, &name, dry_run, yes).await?,
        Commands::Checks => {
            for check in &config.check {
                let state = if check.enabled { "enabled" } else { "disabled" };
                let label = check.label.as_deref().unwrap_or(&check.name);
                println!(
                    "{} {:<12} {:?} {} every {}",
                    check.name,
                    label,
                    check.kind,
                    state,
                    String::from(check.interval)
                );
            }
        }
        Commands::Surfaces => {
            for surface in &config.surface {
                println!("{}: {}", surface.name, surface.checks.join(", "));
            }
        }
        Commands::Metrics => {
            let results = run_checks(&config.check).await;
            print_metrics(&results);
        }
        Commands::Doctor => {
            let enabled = config.check.iter().filter(|check| check.enabled).count();
            println!("checks: {}", config.check.len());
            println!("enabled_checks: {enabled}");
            println!("surfaces: {}", config.surface.len());
            println!(
                "config: {}",
                config_path
                    .as_deref()
                    .map_or_else(|| "built-in".to_string(), |path| path.display().to_string())
            );
            println!("cache_dir: {}", cache_dir.display());
        }
        Commands::Tui { surface } => {
            let items = render_surface_cache_first(&config, &cache_dir, surface.as_deref()).await?;
            let rows = items
                .iter()
                .map(|item| {
                    format!(
                        "{:<10} {:<8} {}",
                        check_display_label(&config, &item.name),
                        item.health,
                        item.text
                    )
                })
                .collect::<Vec<_>>();
            println!("{}", render_tui_panel("board", &rows, 100, 24));
        }
    }

    Ok(())
}

async fn render_output(
    config: &Config,
    cache_dir: &Path,
    fresh: bool,
    format: RenderFormat,
    surface: &str,
    color: bool,
) -> Result<String> {
    let items = if fresh {
        render_surface_fresh(config, Some(surface)).await?
    } else {
        render_surface_cache_first(config, cache_dir, Some(surface)).await?
    };
    Ok(match format {
        RenderFormat::Text => render_text_items(&items),
        RenderFormat::Json => render_json(surface, &items)?,
        _ => render_status_line(&items, status_format(format, color)),
    })
}

fn status_format(format: RenderFormat, color: bool) -> SurfaceFormat {
    match format {
        RenderFormat::Terminal if color => SurfaceFormat::Terminal,
        RenderFormat::Terminal => SurfaceFormat::TerminalPlain,
        RenderFormat::Plain => SurfaceFormat::Plain,
        RenderFormat::Tmux => SurfaceFormat::Tmux,
        RenderFormat::Quickshell => SurfaceFormat::Quickshell,
        RenderFormat::Text | RenderFormat::Json => SurfaceFormat::TerminalPlain,
    }
}

fn terminal_color_enabled(mode: ColorMode, no_color: bool) -> bool {
    if no_color || mode == ColorMode::Never {
        return false;
    }
    match mode {
        ColorMode::Always => true,
        ColorMode::Auto => {
            std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
        }
        ColorMode::Never => false,
    }
}

#[derive(Serialize)]
struct RenderDocument {
    schema_version: u8,
    surface: String,
    health: Health,
    items: Vec<StatusItem>,
}

fn render_json(surface: &str, items: &[StatusItem]) -> Result<String> {
    let items = items
        .iter()
        .cloned()
        .map(|mut item| {
            item.text = strip_quickshell_markup(&item.text);
            item
        })
        .collect::<Vec<_>>();
    let health = items
        .iter()
        .map(|item| item.health)
        .fold(Health::Ok, |current, health| match (current, health) {
            (Health::Critical, _) | (_, Health::Critical) => Health::Critical,
            (Health::Warning, _) | (_, Health::Warning) => Health::Warning,
            (Health::Unknown, _) | (_, Health::Unknown) => Health::Unknown,
            _ => Health::Ok,
        });
    Ok(serde_json::to_string(&RenderDocument {
        schema_version: 1,
        surface: surface.to_string(),
        health,
        items,
    })?)
}

async fn render_watch(
    config: &Config,
    cache_dir: &Path,
    fresh: bool,
    format: RenderFormat,
    surface: &str,
    color: bool,
) -> Result<()> {
    let mut timer = interval(Duration::from_secs(1));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        println!(
            "{}",
            render_output(config, cache_dir, fresh, format, surface, color).await?
        );
        std::io::stdout().flush()?;
    }
}

async fn run_loop(config: &Config, cache_dir: &Path) -> Result<()> {
    fs::create_dir_all(cache_dir)?;
    if config.check.is_empty() {
        std::future::pending::<()>().await;
        return Ok(());
    }

    for check in config.check.iter().filter(|check| check.enabled).cloned() {
        let cache_dir = cache_dir.to_path_buf();
        tokio::spawn(async move {
            let mut timer = interval(check.interval.as_duration());
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let result = run_check(&check).await;
                if let Err(err) = write_cache_one(&cache_dir, &result) {
                    eprintln!("board: failed to write {} cache: {err}", result.item.name);
                }
            }
        });
    }

    std::future::pending::<()>().await;
    Ok(())
}

async fn render_surface_cache_first(
    config: &Config,
    cache_dir: &Path,
    surface_name: Option<&str>,
) -> Result<Vec<StatusItem>> {
    let checks = checks_for_surface(config, surface_name)?;
    let mut items = Vec::new();
    for check in checks {
        let cached = read_cache_one(cache_dir, &check.name)?;
        let result = match cached {
            Some(result)
                if check.kind != CheckKind::NativeCpu && cache_is_fresh(&result, check) =>
            {
                result
            }
            cached if !render_can_run_check(check) => {
                cached.unwrap_or_else(|| missing_cached_check(check))
            }
            cached => {
                let result = run_check_with_previous(check, cached.as_ref()).await;
                write_cache_one(cache_dir, &result)?;
                result
            }
        };
        items.push(result.item);
    }
    Ok(items)
}

fn render_can_run_check(check: &CheckConfig) -> bool {
    check.kind != CheckKind::Command
}

fn missing_cached_check(check: &CheckConfig) -> CheckResult {
    CheckResult {
        item: StatusItem::new(
            &check.name,
            Health::Unknown,
            format!("{} pending", check.name),
        ),
        timestamp: now(),
        cpu_sample: None,
    }
}

async fn render_surface_fresh(
    config: &Config,
    surface_name: Option<&str>,
) -> Result<Vec<StatusItem>> {
    let checks = checks_for_surface(config, surface_name)?;
    let mut items = Vec::new();
    for check in checks {
        items.push(run_check(check).await.item);
    }
    Ok(items)
}

fn check_display_label<'a>(config: &'a Config, name: &'a str) -> &'a str {
    config
        .check
        .iter()
        .find(|check| check.name == name)
        .and_then(|check| check.label.as_deref())
        .unwrap_or(name)
}

fn find_action<'a>(config: &'a Config, name: &str) -> Result<&'a ActionConfig> {
    let (group, action) = name
        .split_once('.')
        .with_context(|| format!("action name must be group.name: {name}"))?;
    config
        .actions
        .get(group)
        .and_then(|group| group.get(action))
        .with_context(|| format!("unknown action: {name}"))
}

async fn run_action(config: &Config, name: &str, dry_run: bool, yes: bool) -> Result<()> {
    let action = find_action(config, name)?;
    if action.confirm && !yes && !dry_run {
        bail!("action {name} requires --yes");
    }
    let Some((program, args)) = action.command.split_first() else {
        bail!("action {name} has empty command");
    };
    if dry_run {
        println!("{}", action.command.join(" "));
        return Ok(());
    }

    let mut child = Command::new(program);
    child.args(args);
    let status = match timeout(action.timeout.as_duration(), child.status()).await {
        Ok(Ok(status)) => status,
        Ok(Err(err)) => bail!("action {name} failed to start: {err}"),
        Err(_) => bail!("action {name} timeout"),
    };
    if !status.success() {
        bail!("action {name} exited with {}", status.code().unwrap_or(1));
    }
    Ok(())
}

fn render_text_items(items: &[StatusItem]) -> String {
    items
        .iter()
        .filter_map(|item| {
            let text = strip_quickshell_markup(&item.text);
            (!text.trim().is_empty()).then_some(text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn checks_for_surface<'a>(
    config: &'a Config,
    surface_name: Option<&str>,
) -> Result<Vec<&'a CheckConfig>> {
    let surface = match surface_name {
        Some(name) => Some(
            config
                .surface
                .iter()
                .find(|surface| surface.name == name)
                .with_context(|| format!("unknown surface: {name}"))?,
        ),
        None => config.surface.first(),
    };

    let Some(surface) = surface else {
        return Ok(config.check.iter().filter(|check| check.enabled).collect());
    };

    Ok(surface
        .checks
        .iter()
        .filter_map(|name| {
            config
                .check
                .iter()
                .find(|check| check.enabled && &check.name == name)
        })
        .collect())
}

async fn run_checks(checks: &[CheckConfig]) -> Vec<CheckResult> {
    let pending = checks
        .iter()
        .filter(|check| check.enabled)
        .map(|check| {
            let check = check.clone();
            tokio::spawn(async move { run_check(&check).await })
        })
        .collect::<Vec<_>>();
    let mut results = Vec::with_capacity(pending.len());
    for result in pending {
        results.push(result.await.expect("check task panicked"));
    }
    results
}

async fn run_check(check: &CheckConfig) -> CheckResult {
    run_check_with_previous(check, None).await
}

fn load_config(path: Option<&Path>) -> Result<Config> {
    let Some(path) = path else {
        return Ok(default_config());
    };
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    let path = base.join("board/board.toml");
    path.is_file().then_some(path)
}

const DEFAULT_SURFACE: &str = "quickshell-bar";

const DEFAULT_CONFIG_TOML: &str = include_str!("../default.toml");

fn default_config() -> Config {
    toml::from_str(DEFAULT_CONFIG_TOML).expect("built-in board default config parses")
}

fn default_enabled() -> bool {
    true
}

fn read_cache_one(cache_dir: &Path, name: &str) -> Result<Option<CheckResult>> {
    let path = cache_dir.join(format!("{name}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text)
        .map(Some)
        .with_context(|| format!("parse {}", path.display()))
}

fn cache_is_fresh(result: &CheckResult, check: &CheckConfig) -> bool {
    let max_age = check
        .fresh_for
        .map(HumanDuration::as_duration)
        .unwrap_or_else(|| check.interval.as_duration().saturating_mul(3));
    now().saturating_sub(result.timestamp) <= max_age.as_secs()
}

fn write_cache_one(cache_dir: &Path, result: &CheckResult) -> Result<()> {
    fs::create_dir_all(cache_dir)?;
    let path = cache_dir.join(format!("{}.json", result.item.name));
    fs::write(path, serde_json::to_vec(result)?)?;
    Ok(())
}

fn write_cache(cache_dir: &Path, results: &[CheckResult]) -> Result<()> {
    for result in results {
        write_cache_one(cache_dir, result)?;
    }
    Ok(())
}

fn print_metrics(results: &[CheckResult]) {
    for result in results {
        println!(
            "board_check_status{{check=\"{}\"}} {}",
            result.item.name,
            match result.item.health {
                Health::Ok => 0,
                Health::Warning => 1,
                Health::Critical => 2,
                Health::Unknown => 3,
            }
        );
        println!(
            "board_check_last_run_timestamp_seconds{{check=\"{}\"}} {}",
            result.item.name, result.timestamp
        );
    }
}

fn health_percent_for(
    check: &CheckConfig,
    value: u64,
    default_warning: u64,
    default_critical: u64,
) -> Health {
    health_percent(
        value,
        check.warning.unwrap_or(default_warning),
        check.critical.unwrap_or(default_critical),
    )
}

fn health_percent(value: u64, warning: u64, critical: u64) -> Health {
    if value >= critical {
        Health::Critical
    } else if value >= warning {
        Health::Warning
    } else {
        Health::Ok
    }
}

#[cfg(any(test, target_os = "linux"))]
fn meminfo_value(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?;
    rest.split_whitespace().next()?.parse().ok()
}

fn parse_duration(input: &str) -> Option<Duration> {
    let input = input.trim();
    let (num, unit) = input
        .find(|ch: char| !ch.is_ascii_digit())
        .map(|idx| input.split_at(idx))
        .unwrap_or((input, "s"));
    let value = num.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(value)),
        "s" | "" => Some(Duration::from_secs(value)),
        "m" => Some(Duration::from_secs(value * 60)),
        _ => None,
    }
}

fn default_interval() -> HumanDuration {
    HumanDuration(Duration::from_secs(10))
}

fn default_timeout() -> HumanDuration {
    HumanDuration(Duration::from_secs(2))
}

fn default_timeout_text() -> String {
    "󰔟".to_string()
}

fn default_cache_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("board")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert_with_labels(labels: &[(&str, &str)]) -> AlertmanagerAlert {
        AlertmanagerAlert {
            labels: labels
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn check_config(name: &str, kind: CheckKind) -> CheckConfig {
        CheckConfig {
            name: name.to_string(),
            label: None,
            kind,
            interval: HumanDuration(Duration::from_secs(5)),
            timeout: HumanDuration(Duration::from_secs(1)),
            timeout_text: default_timeout_text(),
            fresh_for: None,
            command: None,
            source: None,
            filters: Vec::new(),
            critical_destination: None,
            warning_destination: None,
            excluded_channel: None,
            warning: None,
            critical: None,
            location: None,
            timezones: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn parses_duration_units() {
        assert_eq!(parse_duration("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("5s"), Some(Duration::from_secs(5)));
        assert_eq!(parse_duration("2m"), Some(Duration::from_secs(120)));
    }

    #[test]
    fn parses_cpu_sample_like_check_cpu() {
        let sample = cpu_sample_from_proc_stat_line("cpu  100 20 30 850 99 1 2 3").unwrap();
        assert_eq!(sample.active, 150);
        assert_eq!(sample.idle, 850);
    }

    #[test]
    fn computes_cpu_percent_from_previous_sample_like_check_cpu() {
        let previous = CpuSample {
            active: 100,
            idle: 900,
        };
        let current = CpuSample {
            active: 130,
            idle: 970,
        };
        assert_eq!(cpu_busy_percent(current, Some(previous)), Some(30));
    }

    #[test]
    fn cpu_percent_returns_none_for_empty_delta() {
        let sample = CpuSample {
            active: 100,
            idle: 900,
        };
        assert_eq!(cpu_busy_percent(sample, Some(sample)), None);
    }

    #[test]
    fn reads_meminfo_value() {
        assert_eq!(
            meminfo_value("MemAvailable:   42 kB", "MemAvailable:"),
            Some(42)
        );
    }

    #[test]
    fn render_skips_command_checks() {
        let check = check_config("scripts", CheckKind::Command);
        assert!(!render_can_run_check(&check));
        assert_eq!(missing_cached_check(&check).item.text, "scripts pending");
    }

    #[test]
    fn default_surface_has_bar_checks() {
        let config = default_config();
        let tmux = config
            .surface
            .iter()
            .find(|surface| surface.name == "tmux-top")
            .unwrap();
        assert!(tmux.checks.contains(&"agents".to_string()));
        assert!(tmux.checks.contains(&"cpu".to_string()));
        let quickshell = config
            .surface
            .iter()
            .find(|surface| surface.name == "quickshell-bar")
            .unwrap();
        assert!(quickshell.checks.contains(&"weather".to_string()));
        assert!(
            !checks_for_surface(&config, Some("quickshell-bar"))
                .unwrap()
                .iter()
                .any(|check| check.name == "weather")
        );
        let panels = config
            .surface
            .iter()
            .find(|surface| surface.name == "quickshell-panels")
            .unwrap();
        assert_eq!(panels.checks, ["time-panel", "todo-panel"]);
        assert!(
            config
                .surface
                .iter()
                .any(|surface| surface.name == "quickshell-weather")
        );
    }

    #[test]
    fn unknown_surface_fails() {
        let error = checks_for_surface(&default_config(), Some("missing")).unwrap_err();
        assert_eq!(error.to_string(), "unknown surface: missing");
    }

    #[test]
    fn disabled_checks_are_not_run_by_surfaces_or_batches() {
        let config = default_config();
        let weather = config
            .check
            .iter()
            .find(|check| check.name == "weather")
            .unwrap();
        assert!(!weather.enabled);
        assert!(
            !checks_for_surface(&config, Some("quickshell-weather"))
                .unwrap()
                .iter()
                .any(|check| check.name == "weather")
        );
    }

    #[test]
    fn check_display_label_uses_config_label_without_changing_name() {
        let mut config = default_config();
        let check = config
            .check
            .iter_mut()
            .find(|check| check.name == "cpu")
            .unwrap();
        check.label = Some("Processor".to_string());

        assert_eq!(check_display_label(&config, "cpu"), "Processor");
        assert_eq!(check_display_label(&config, "missing"), "missing");
    }

    #[test]
    fn parses_nested_action_config() {
        let config: Config = toml::from_str(
            r#"
            [actions.personal.refresh]
            label = "Refresh"
            command = ["board", "once"]
            confirm = false
            timeout = "5s"
            "#,
        )
        .unwrap();
        let action = find_action(&config, "personal.refresh").unwrap();
        assert_eq!(
            action.command,
            vec!["board".to_string(), "once".to_string()]
        );
        assert!(!action.confirm);
        assert_eq!(action.timeout.as_duration(), Duration::from_secs(5));
    }

    #[test]
    fn text_renderer_omits_health_and_empty_items() {
        let items = vec![
            StatusItem::new("empty", Health::Ok, ""),
            StatusItem::new("panel", Health::Warning, "hello"),
        ];
        assert_eq!(render_text_items(&items), "hello");
    }

    #[test]
    fn render_defaults_to_terminal_quickshell_bar() {
        let cli = Cli::try_parse_from(["board", "render"]).unwrap();
        let Commands::Render {
            format,
            surface,
            color,
            no_color,
            ..
        } = cli.command
        else {
            panic!("expected render command");
        };
        assert_eq!(format, None);
        assert_eq!(surface, None);
        assert_eq!(color, ColorMode::Auto);
        assert!(!no_color);
    }

    #[test]
    fn parses_agent_friendly_render_options() {
        let cli = Cli::try_parse_from([
            "board",
            "render",
            "--format",
            "json",
            "--surface",
            "tmux-top",
            "--color",
            "never",
        ])
        .unwrap();
        let Commands::Render {
            format,
            surface,
            color,
            ..
        } = cli.command
        else {
            panic!("expected render command");
        };
        assert_eq!(format, Some(RenderFormat::Json));
        assert_eq!(surface.as_deref(), Some("tmux-top"));
        assert_eq!(color, ColorMode::Never);
    }

    #[test]
    fn renders_stable_json_document() {
        let items = vec![StatusItem::new("cpu", Health::Warning, " 91").with_detail("high load")];
        let value: Value = serde_json::from_str(&render_json("bar", &items).unwrap()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["surface"], "bar");
        assert_eq!(value["health"], "warning");
        assert_eq!(value["items"][0]["name"], "cpu");
        assert_eq!(value["items"][0]["detail"], "high load");
    }

    #[test]
    fn applies_explicit_terminal_color_policy() {
        assert!(terminal_color_enabled(ColorMode::Always, false));
        assert!(!terminal_color_enabled(ColorMode::Never, false));
        assert!(!terminal_color_enabled(ColorMode::Always, true));
    }

    #[test]
    fn renders_native_time_panel_without_shelling_out() {
        let item = native_time_panel(&check_config("time-panel", CheckKind::NativeTimePanel));
        assert_eq!(item.health, Health::Ok);
        assert!(item.text.contains("Timezones"));
        assert!(item.text.contains("Timestamps"));
        assert!(item.text.contains("Calendar"));
    }

    #[test]
    fn maps_weather_conditions_to_icons() {
        assert_eq!(weather_icon("Sunny"), "󰖙");
        assert_eq!(weather_icon("light rain"), "󰖗");
        assert_eq!(weather_icon("Cloudy"), "󰖐");
    }

    #[test]
    fn native_safe_reports_open_without_shelling_out() {
        let item = native_safe("safe");
        assert!(matches!(item.health, Health::Ok | Health::Warning));
    }

    #[test]
    fn parses_http_date_header_case_insensitively() {
        let response = "HTTP/1.1 200 OK\r\nDate: Sun, 24 Aug 2026 17:00:00 GMT\r\n\r\n";
        assert_eq!(
            http_header(response, "date"),
            Some("Sun, 24 Aug 2026 17:00:00 GMT")
        );
    }

    #[test]
    fn builds_bounded_dns_query_packet() {
        let query = dns_query_packet("bandonga.com", 0x1234).unwrap();
        assert_eq!(&query[..6], &[0x12, 0x34, 0x01, 0, 0, 1]);
        assert!(query.ends_with(&[0, 0, 1, 0, 1]));
        assert!(dns_query_packet(&"x".repeat(64), 1).is_none());
    }

    #[test]
    fn parses_docker_http_body_count() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"Id\":\"1\"},{\"Id\":\"2\"}]";
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        assert_eq!(serde_json::from_str::<Vec<Value>>(body).unwrap().len(), 2);
    }

    #[test]
    fn parses_unix_docker_host() {
        assert_eq!(
            docker_host_socket("unix:///Users/test/.colima/work/docker.sock"),
            Some(PathBuf::from("/Users/test/.colima/work/docker.sock"))
        );
        assert_eq!(docker_host_socket("tcp://127.0.0.1:2375"), None);
    }

    #[test]
    fn calculates_memory_percent() {
        assert_eq!(memory_used_percent(16, 4), 75);
        assert_eq!(memory_used_percent(0, 0), 0);
    }

    #[tokio::test]
    async fn batch_checks_run_concurrently() {
        let mut first = check_config("first", CheckKind::Command);
        first.command = Some(vec!["sleep".to_string(), "0.25".to_string()]);
        let mut second = first.clone();
        second.name = "second".to_string();

        let started = std::time::Instant::now();
        let results = run_checks(&[first, second]).await;

        assert_eq!(results.len(), 2);
        assert!(started.elapsed() < Duration::from_millis(450));
    }

    #[tokio::test]
    async fn command_timeout_is_compact_and_keeps_detail() {
        let mut check = check_config("slow", CheckKind::Command);
        check.command = Some(vec!["sleep".to_string(), "1".to_string()]);
        check.timeout = HumanDuration(Duration::from_millis(1));
        check.timeout_text = "timer".to_string();

        let item = run_command_check(&check).await;

        assert_eq!(item.health, Health::Critical);
        assert_eq!(item.text, "timer");
        assert_eq!(item.detail.as_deref(), Some("slow timed out after 1ms"));
    }

    #[test]
    fn stale_cache_expires_after_fresh_window() {
        let check = CheckConfig {
            name: "cpu".to_string(),
            label: None,
            kind: CheckKind::NativeCpu,
            interval: HumanDuration(Duration::from_secs(5)),
            timeout: HumanDuration(Duration::from_secs(1)),
            timeout_text: default_timeout_text(),
            fresh_for: Some(HumanDuration(Duration::from_secs(10))),
            command: None,
            source: None,
            filters: Vec::new(),
            critical_destination: None,
            warning_destination: None,
            excluded_channel: None,
            warning: None,
            critical: None,
            location: None,
            timezones: Vec::new(),
            enabled: true,
        };
        let fresh = CheckResult {
            item: StatusItem::new("cpu", Health::Ok, "  1"),
            timestamp: now(),
            cpu_sample: None,
        };
        let stale = CheckResult {
            timestamp: now().saturating_sub(11),
            ..fresh.clone()
        };
        assert!(cache_is_fresh(&fresh, &check));
        assert!(!cache_is_fresh(&stale, &check));
    }
    #[test]
    fn parses_script_status_health() {
        let ok = "# 20260824\ncheck-scripts: 1 self\ngsync: 0 ok\n";
        assert_eq!(
            script_status_failures(ok, "# 20260824").unwrap(),
            Vec::<String>::new()
        );

        let failed = "# 20260824\ngsync: 0 ok\ncheck-claw: 2 down\n";
        assert_eq!(
            script_status_failures(failed, "# 20260824").unwrap(),
            vec!["check-claw".to_string()]
        );
        assert!(script_status_failures(failed, "# 20260825").is_none());
    }

    #[test]
    fn classifies_alertmanager_destinations_and_exclusion() {
        let alerts = vec![
            alert_with_labels(&[("destinations", "['incidentio']")]),
            alert_with_labels(&[("destinations", "['slack']")]),
            alert_with_labels(&[
                ("destinations", "['slack']"),
                ("channel", "#term-vendor-alerts"),
            ]),
        ];
        assert_eq!(
            alert_counts(&alerts, "incidentio", "slack", Some("#term-vendor-alerts")),
            (1, 1)
        );
    }

    #[test]
    fn normalizes_load_average_by_logical_cpu_count() {
        assert_eq!(load_average_percent(4.0, 8), 50);
        assert_eq!(load_average_percent(12.0, 8), 100);
    }
}
