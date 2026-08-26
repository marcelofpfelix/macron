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
    let checks = checks_for_surface(config, surface_name);
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
    let checks = checks_for_surface(config, surface_name);
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

fn checks_for_surface<'a>(config: &'a Config, surface_name: Option<&str>) -> Vec<&'a CheckConfig> {
    let surface = surface_name
        .and_then(|name| config.surface.iter().find(|surface| surface.name == name))
        .or_else(|| config.surface.first());

    let Some(surface) = surface else {
        return config.check.iter().filter(|check| check.enabled).collect();
    };

    surface
        .checks
        .iter()
        .filter_map(|name| {
            config
                .check
                .iter()
                .find(|check| check.enabled && &check.name == name)
        })
        .collect()
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

async fn run_check_with_previous(
    check: &CheckConfig,
    previous: Option<&CheckResult>,
) -> CheckResult {
    match check.kind {
        CheckKind::NativeCpu => native_cpu(check, previous.and_then(|result| result.cpu_sample)),
        CheckKind::NativeMem => check_result(native_mem(check).await),
        CheckKind::NativeTemp => check_result(native_temp(check).await),
        CheckKind::NativeWeather => check_result(native_weather(check).await),
        CheckKind::NativeTimePanel => check_result(native_time_panel(check)),
        CheckKind::NativeTodoPanel => check_result(native_todo_panel(&check.name).await),
        CheckKind::NativeSafe => check_result(native_safe(&check.name)),
        CheckKind::NativeResolv => check_result(native_resolv(check).await),
        CheckKind::NativeTimeOffset => check_result(native_time_offset(check).await),
        CheckKind::NativeScriptStatus => check_result(native_script_status(check)),
        CheckKind::NativeAlertmanager => check_result(native_alertmanager(check).await),
        CheckKind::NativeDocker => check_result(native_docker(&check.name)),
        CheckKind::Command => check_result(run_command_check(check).await),
    }
}

fn check_result(item: StatusItem) -> CheckResult {
    CheckResult {
        item,
        timestamp: now(),
        cpu_sample: None,
    }
}

#[cfg(not(target_os = "macos"))]
fn native_cpu(check: &CheckConfig, previous: Option<CpuSample>) -> CheckResult {
    let name = &check.name;
    match read_cpu_sample() {
        Some(sample) => {
            let busy_pct = cpu_busy_percent(sample, previous).unwrap_or(0);
            CheckResult {
                item: StatusItem::new(
                    name,
                    health_percent_for(check, busy_pct, 60, 85),
                    format!(" {busy_pct:2}"),
                ),
                timestamp: now(),
                cpu_sample: Some(sample),
            }
        }
        None => CheckResult {
            item: StatusItem::new(name, Health::Unknown, "  0"),
            timestamp: now(),
            cpu_sample: None,
        },
    }
}

#[cfg(target_os = "macos")]
fn native_cpu(check: &CheckConfig, _previous: Option<CpuSample>) -> CheckResult {
    let cores = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let Some(load) = mac_load_average() else {
        return check_result(StatusItem::new(&check.name, Health::Unknown, " --"));
    };
    let busy_pct = load_average_percent(load, cores);
    check_result(
        StatusItem::new(
            &check.name,
            health_percent_for(check, busy_pct, 60, 85),
            format!(" {busy_pct:2}"),
        )
        .with_detail(format!("1m load {load:.2} across {cores} logical CPUs")),
    )
}

#[cfg(not(target_os = "macos"))]
fn read_cpu_sample() -> Option<CpuSample> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    cpu_sample_from_proc_stat_line(line)
}

#[cfg(any(test, target_os = "macos"))]
fn load_average_percent(load: f64, cores: usize) -> u64 {
    ((load * 100.0 / cores.max(1) as f64).round() as u64).min(100)
}

#[cfg(target_os = "macos")]
fn mac_load_average() -> Option<f64> {
    let mut values = [0.0];
    // getloadavg writes at most the single f64 slot provided here.
    let count = unsafe { getloadavg(values.as_mut_ptr(), 1) };
    (count == 1).then_some(values[0])
}

#[cfg(any(test, not(target_os = "macos")))]
fn cpu_sample_from_proc_stat_line(line: &str) -> Option<CpuSample> {
    let nums = line
        .split_whitespace()
        .skip(1)
        .filter_map(|part| part.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if nums.len() < 4 {
        return None;
    }
    Some(CpuSample {
        active: nums[0] + nums[1] + nums[2],
        idle: nums[3],
    })
}

#[cfg(any(test, not(target_os = "macos")))]
fn cpu_busy_percent(current: CpuSample, previous: Option<CpuSample>) -> Option<u64> {
    let previous = previous.unwrap_or(CpuSample { active: 0, idle: 0 });
    let active = current.active.checked_sub(previous.active)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let total = active + idle;
    active.checked_mul(100)?.checked_div(total)
}

fn memory_used_percent(total: u64, available: u64) -> u64 {
    if total == 0 {
        0
    } else {
        (total.saturating_sub(available).saturating_mul(100) / total).min(100)
    }
}

#[cfg(target_os = "linux")]
async fn native_mem(check: &CheckConfig) -> StatusItem {
    let name = &check.name;
    match fs::read_to_string("/proc/meminfo") {
        Ok(text) => {
            let mut total = 0u64;
            let mut available = 0u64;
            for line in text.lines() {
                if let Some(value) = meminfo_value(line, "MemTotal:") {
                    total = value;
                } else if let Some(value) = meminfo_value(line, "MemAvailable:") {
                    available = value;
                }
            }
            if total == 0 {
                return StatusItem::new(name, Health::Unknown, " --");
            }
            let used_pct = memory_used_percent(total, available);
            StatusItem::new(
                name,
                health_percent_for(check, used_pct, 60, 85),
                format!(" {used_pct:2}"),
            )
        }
        Err(err) => StatusItem::new(name, Health::Unknown, " --")
            .with_detail(format!("read /proc/meminfo: {err}")),
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct MacMetrics {
    memory_used_pct: u64,
    cpu_temp_c: f64,
}

#[cfg(target_os = "macos")]
fn read_mac_metrics() -> Result<MacMetrics> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(std::time::Instant, MacMetrics)>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut cached = cache
        .lock()
        .map_err(|_| anyhow::anyhow!("mac metrics cache poisoned"))?;
    if let Some((sampled_at, metrics)) = *cached
        && sampled_at.elapsed() < Duration::from_secs(5)
    {
        return Ok(metrics);
    }
    let mut sampler = macmon::Sampler::new()
        .map_err(|err| anyhow::anyhow!("initialize macOS IOReport sampler: {err}"))?;
    let metrics = sampler
        .get_metrics(100)
        .map_err(|err| anyhow::anyhow!("sample macOS IOReport metrics: {err}"))?;
    let result = MacMetrics {
        memory_used_pct: memory_used_percent(
            metrics.memory.ram_total,
            metrics
                .memory
                .ram_total
                .saturating_sub(metrics.memory.ram_usage),
        ),
        cpu_temp_c: metrics.temp.cpu_temp_avg as f64,
    };
    *cached = Some((std::time::Instant::now(), result));
    Ok(result)
}

#[cfg(target_os = "macos")]
async fn native_mem(check: &CheckConfig) -> StatusItem {
    let name = check.name.clone();
    match task::spawn_blocking(read_mac_metrics).await {
        Ok(Ok(metrics)) => StatusItem::new(
            name,
            health_percent_for(check, metrics.memory_used_pct, 60, 85),
            format!(" {:2}", metrics.memory_used_pct),
        ),
        Ok(Err(err)) => StatusItem::new(name, Health::Unknown, " --").with_detail(err.to_string()),
        Err(err) => StatusItem::new(name, Health::Unknown, " --")
            .with_detail(format!("macOS metrics task failed: {err}")),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn native_mem(check: &CheckConfig) -> StatusItem {
    StatusItem::new(&check.name, Health::Unknown, " --")
        .with_detail("memory collector is unavailable on this platform")
}

#[cfg(target_os = "linux")]
async fn native_temp(check: &CheckConfig) -> StatusItem {
    let name = &check.name;
    let zones = match fs::read_dir("/sys/class/thermal") {
        Ok(zones) => zones,
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰔐 --"),
    };
    let mut max_c = None;
    for entry in zones.flatten() {
        let path = entry.path().join("temp");
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(raw) = text.trim().parse::<i64>() else {
            continue;
        };
        let celsius = if raw > 1000 { raw / 1000 } else { raw };
        max_c = Some(max_c.map_or(celsius, |current: i64| current.max(celsius)));
    }
    match max_c {
        Some(temp) => StatusItem::new(
            name,
            health_percent_for(check, temp as u64, 60, 80),
            format!("󰔐 {temp:2}"),
        ),
        None => StatusItem::new(name, Health::Unknown, "󰔐 --"),
    }
}

#[cfg(target_os = "macos")]
async fn native_temp(check: &CheckConfig) -> StatusItem {
    let name = check.name.clone();
    let result = task::spawn_blocking(read_mac_metrics).await;
    match result {
        Ok(Ok(metrics)) if metrics.cpu_temp_c.is_finite() && metrics.cpu_temp_c > 0.0 => {
            let temp = metrics.cpu_temp_c.round() as u64;
            StatusItem::new(
                name,
                health_percent_for(check, temp, 60, 80),
                format!("󰔐 {temp:2}"),
            )
            .with_detail(format!("CPU average {:.1}°C", metrics.cpu_temp_c))
        }
        Ok(Ok(_)) => StatusItem::new(name, Health::Unknown, "󰔐 --")
            .with_detail("macOS temperature sensor returned no valid sample"),
        Ok(Err(err)) => StatusItem::new(name, Health::Unknown, "󰔐 --").with_detail(err.to_string()),
        Err(err) => StatusItem::new(name, Health::Unknown, "󰔐 --")
            .with_detail(format!("macOS metrics task failed: {err}")),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn native_temp(check: &CheckConfig) -> StatusItem {
    StatusItem::new(&check.name, Health::Unknown, "")
        .with_detail("temperature collector is unavailable on this platform")
}

fn native_script_status(check: &CheckConfig) -> StatusItem {
    let Some(path) = check_source(check, "script-status.ini") else {
        return StatusItem::new(&check.name, Health::Warning, "")
            .with_detail("HOME is unavailable");
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return StatusItem::new(&check.name, Health::Warning, "")
                .with_detail(format!("read {}: {err}", path.display()));
        }
    };
    let today = Local::now().format("# %Y%m%d").to_string();
    let Some(failures) = script_status_failures(&text, &today) else {
        return StatusItem::new(&check.name, Health::Warning, "")
            .with_detail("script status is stale");
    };
    if failures.is_empty() {
        StatusItem::new(&check.name, Health::Ok, "")
    } else {
        StatusItem::new(&check.name, Health::Warning, "")
            .with_detail(format!("failing: {}", failures.join(", ")))
    }
}

fn script_status_failures(text: &str, today: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    if lines.next()? != today {
        return None;
    }
    Some(
        lines
            .filter_map(|line| {
                let (name, rest) = line.split_once(':')?;
                if name == "check-scripts" || name == "alerts" || name.starts_with('#') {
                    return None;
                }
                let code = rest.split_whitespace().next()?.parse::<i32>().ok()?;
                (code != 0).then(|| name.to_string())
            })
            .collect(),
    )
}

async fn native_alertmanager(check: &CheckConfig) -> StatusItem {
    let Some(path) = check_source(check, ".config/amtool/config.yml") else {
        return alertmanager_error(check, "HOME is unavailable");
    };
    let config = match fs::read_to_string(&path) {
        Ok(config) => config,
        Err(err) => return alertmanager_error(check, &format!("read {}: {err}", path.display())),
    };
    let Some(base_url) = amtool_alertmanager_url(&config) else {
        return alertmanager_error(check, "alertmanager.url missing from amtool config");
    };
    let url = format!("{}/api/v2/alerts", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(check.timeout.as_duration())
        .build()
    {
        Ok(client) => client,
        Err(err) => return alertmanager_error(check, &err.to_string()),
    };
    let mut query = vec![
        ("active", "true"),
        ("silenced", "false"),
        ("inhibited", "false"),
        ("unprocessed", "false"),
    ];
    query.extend(
        check
            .filters
            .iter()
            .map(|filter| ("filter", filter.as_str())),
    );
    let response = match client.get(url).query(&query).send().await {
        Ok(response) => response,
        Err(err) => return alertmanager_error(check, &err.to_string()),
    };
    let response = match response.error_for_status() {
        Ok(response) => response,
        Err(err) => return alertmanager_error(check, &err.to_string()),
    };
    let body = match response.text().await {
        Ok(body) => body,
        Err(err) => return alertmanager_error(check, &err.to_string()),
    };
    let alerts = match serde_json::from_str::<Vec<AlertmanagerAlert>>(&body) {
        Ok(alerts) => alerts,
        Err(err) => return alertmanager_error(check, &err.to_string()),
    };
    let (critical, warning) = alert_counts(
        &alerts,
        check
            .critical_destination
            .as_deref()
            .unwrap_or("incidentio"),
        check.warning_destination.as_deref().unwrap_or("slack"),
        check.excluded_channel.as_deref(),
    );
    let (health, text) = if critical > 0 {
        let warning_text = if warning > 0 {
            format!("  {warning}")
        } else {
            String::new()
        };
        (Health::Critical, format!("󰞏 {critical}{warning_text}"))
    } else if warning > 0 {
        (Health::Warning, format!(" {warning}"))
    } else {
        (Health::Ok, "󰩪".to_string())
    };
    StatusItem::new(&check.name, health, text)
        .with_detail(format!("criticals={critical} warnings={warning}"))
}

fn alertmanager_error(check: &CheckConfig, detail: &str) -> StatusItem {
    StatusItem::new(&check.name, Health::Warning, "󰔟").with_detail(detail)
}

fn alert_counts(
    alerts: &[AlertmanagerAlert],
    critical_destination: &str,
    warning_destination: &str,
    excluded_channel: Option<&str>,
) -> (usize, usize) {
    let critical_value = format!("['{critical_destination}']");
    let warning_value = format!("['{warning_destination}']");
    alerts.iter().fold((0, 0), |(critical, warning), alert| {
        let destination = alert.labels.get("destinations").map(String::as_str);
        let excluded = excluded_channel.is_some_and(|channel| {
            alert
                .labels
                .get("channel")
                .is_some_and(|value| value == channel)
        });
        (
            critical + usize::from(destination == Some(critical_value.as_str())),
            warning + usize::from(destination == Some(warning_value.as_str()) && !excluded),
        )
    })
}

fn amtool_alertmanager_url(config: &str) -> Option<&str> {
    config.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "alertmanager.url").then(|| {
            value
                .trim()
                .trim_matches(|character| character == '"' || character == '\'')
        })
    })
}

fn check_source(check: &CheckConfig, default_relative: &str) -> Option<PathBuf> {
    check
        .source
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(default_relative)))
}

async fn native_weather(check: &CheckConfig) -> StatusItem {
    let name = &check.name;
    let location = check.location.as_deref().unwrap_or("Lisbon");
    let url = format!(
        "https://wttr.in/{}?format=%C+%t",
        encode_wttr_location(location)
    );
    let client = match reqwest::Client::builder()
        .timeout(check.timeout.as_duration())
        .build()
    {
        Ok(client) => client,
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰖐 --"),
    };
    let text = match client.get(url).send().await {
        Ok(response) => match response.text().await {
            Ok(text) => collapse_whitespace(&text),
            Err(_) => return StatusItem::new(name, Health::Unknown, "󰖐 --"),
        },
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰖐 --"),
    };
    if text.is_empty() {
        return StatusItem::new(name, Health::Unknown, "󰖐 --");
    }

    let (condition, temp) = text.rsplit_once(' ').unwrap_or(("", &text));
    let icon = weather_icon(condition);
    StatusItem::new(name, Health::Ok, format!("{icon} {temp}"))
}

fn native_time_panel(check: &CheckConfig) -> StatusItem {
    let name = &check.name;
    let now = Local::now();
    let mut lines = vec!["Timezones".to_string()];
    for timezone in timezones_for(check) {
        lines.push(format_timezone_line(
            &timezone.label,
            &timezone.zone,
            now.timestamp(),
        ));
    }
    lines.push(String::new());
    lines.push("Timestamps".to_string());
    lines.push(format!("  Unix     {}", now.timestamp()));
    lines.push(format!("  Hex      0x{:x}", now.timestamp()));
    lines.push(String::new());
    lines.push("Calendar".to_string());
    lines.extend(
        month_calendar(now.year(), now.month())
            .into_iter()
            .map(|line| format!("  {line}")),
    );
    StatusItem::new(name, Health::Ok, lines.join("\n"))
}

fn timezones_for(check: &CheckConfig) -> Vec<TimezoneConfig> {
    if !check.timezones.is_empty() {
        return check.timezones.clone();
    }
    vec![
        TimezoneConfig {
            label: "Lisbon".to_string(),
            zone: "Europe/Lisbon".to_string(),
        },
        TimezoneConfig {
            label: "UTC".to_string(),
            zone: "UTC".to_string(),
        },
    ]
}

fn format_timezone_line(label: &str, zone: &str, timestamp: i64) -> String {
    let Ok(tz) = zone.parse::<Tz>() else {
        return format!("  {label:<8} --");
    };
    let Some(utc) = chrono::Utc.timestamp_opt(timestamp, 0).single() else {
        return format!("  {label:<8} --");
    };
    format!(
        "  {label:<8} {}",
        utc.with_timezone(&tz).format("%a %d %b %H:%M:%S %Z")
    )
}

fn month_calendar(year: i32, month: u32) -> Vec<String> {
    let Some(first) = chrono::NaiveDate::from_ymd_opt(year, month, 1) else {
        return Vec::new();
    };
    let days = days_in_month(year, month);
    let mut lines = vec![
        format!("     {} {}", month_name(month), year),
        "Mo Tu We Th Fr Sa Su".to_string(),
    ];
    let mut line = String::new();
    let offset = first.weekday().num_days_from_monday();
    for _ in 0..offset {
        line.push_str("   ");
    }
    for day in 1..=days {
        if !line.is_empty() && !line.ends_with(' ') {
            line.push(' ');
        }
        line.push_str(&format!("{day:>2}"));
        let weekday = (offset + day - 1) % 7;
        if weekday == 6 {
            lines.push(line);
            line = String::new();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.and_then(|date| date.pred_opt())
        .map_or(30, |date| date.day())
}

fn month_name(month: u32) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("Unknown")
}

fn encode_wttr_location(location: &str) -> String {
    location.trim().replace(' ', "+")
}

fn weather_icon(condition: &str) -> &'static str {
    let lower = condition.to_ascii_lowercase();
    if lower.contains("sunny") || lower.contains("clear") {
        "󰖙"
    } else if lower.contains("rain") || lower.contains("drizzle") || lower.contains("shower") {
        "󰖗"
    } else if lower.contains("thunder") || lower.contains("storm") {
        "󰖓"
    } else if lower.contains("snow") || lower.contains("sleet") || lower.contains("ice") {
        "󰖘"
    } else if lower.contains("fog") || lower.contains("mist") || lower.contains("haze") {
        "󰖑"
    } else {
        "󰖐"
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn native_todo_panel(name: &str) -> StatusItem {
    let output = match timeout(
        Duration::from_secs(2),
        Command::new("task")
            .args([
                "rc.verbose:nothing",
                "+READY",
                "status:pending",
                "limit:6",
                "export",
            ])
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return StatusItem::new(name, Health::Ok, "Taskwarrior not installed"),
        Err(_) => return StatusItem::new(name, Health::Warning, "Taskwarrior timeout"),
    };
    if !output.status.success() {
        return StatusItem::new(name, Health::Ok, "Taskwarrior not installed");
    }
    let items = match serde_json::from_slice::<Vec<Value>>(&output.stdout) {
        Ok(items) => items,
        Err(_) => return StatusItem::new(name, Health::Warning, "Taskwarrior parse error"),
    };
    let lines = items
        .iter()
        .filter_map(|item| item.get("description").and_then(Value::as_str))
        .take(6)
        .map(|description| format!("  - {description}"))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        StatusItem::new(name, Health::Ok, "No ready tasks")
    } else {
        StatusItem::new(name, Health::Ok, lines.join("\n"))
    }
}

fn native_safe(name: &str) -> StatusItem {
    let lock = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/marcelof"))
        .join(".config/safe.lock");
    if lock.exists() {
        StatusItem::new(name, Health::Ok, "")
    } else {
        StatusItem::new(name, Health::Warning, " ")
    }
}

async fn native_resolv(check: &CheckConfig) -> StatusItem {
    let name = check.name.clone();
    let fallback_name = name.clone();
    match timeout(
        check.timeout.as_duration(),
        task::spawn_blocking(move || native_resolv_blocking(&name)),
    )
    .await
    {
        Ok(Ok(item)) => item,
        Ok(Err(_)) => StatusItem::new(fallback_name, Health::Unknown, "󰲝"),
        Err(_) => StatusItem::new(&fallback_name, Health::Warning, "󰲝").with_detail(format!(
            "{} timed out after {}ms",
            fallback_name,
            check.timeout.0.as_millis()
        )),
    }
}

fn native_resolv_blocking(name: &str) -> StatusItem {
    let deadline = Duration::from_secs(1);
    let server = resolv_nameserver().unwrap_or_else(|| "1.1.1.1:53".parse().unwrap());
    if dns_query(server, "bandonga.com", deadline) {
        return StatusItem::new(name, Health::Ok, "");
    }
    let net_ok = TcpStream::connect_timeout(
        &"1.1.1.1:53".parse().expect("valid fallback address"),
        deadline,
    )
    .is_ok();
    if net_ok {
        StatusItem::new(name, Health::Warning, "󰲝")
    } else {
        StatusItem::new(name, Health::Critical, "󰖪")
    }
}

fn resolv_nameserver() -> Option<SocketAddr> {
    let text = fs::read_to_string("/etc/resolv.conf").ok()?;
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "nameserver" {
            return None;
        }
        let ip = parts.next()?.parse::<IpAddr>().ok()?;
        Some(SocketAddr::new(ip, 53))
    })
}

fn dns_query_packet(domain: &str, id: u16) -> Option<Vec<u8>> {
    let mut query = vec![
        (id >> 8) as u8,
        id as u8,
        0x01,
        0x00,
        0x00,
        0x01,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ];
    for label in domain.split(".") {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.extend_from_slice(&[0, 0, 1, 0, 1]);
    Some(query)
}

fn dns_query(server: SocketAddr, domain: &str, deadline: Duration) -> bool {
    let Some(query) = dns_query_packet(domain, (now() & u16::MAX as u64) as u16) else {
        return false;
    };

    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let Ok(socket) = UdpSocket::bind(bind) else {
        return false;
    };
    if socket.set_read_timeout(Some(deadline)).is_err()
        || socket.set_write_timeout(Some(deadline)).is_err()
        || socket.send_to(&query, server).is_err()
    {
        return false;
    }
    let mut response = [0u8; 512];
    let Ok(size) = socket.recv(&mut response) else {
        return false;
    };
    size >= 12
        && response[0..2] == query[0..2]
        && response[3] & 0x0f == 0
        && u16::from_be_bytes([response[6], response[7]]) > 0
}

async fn native_time_offset(check: &CheckConfig) -> StatusItem {
    let name = check.name.clone();
    let fallback_name = name.clone();
    let deadline = check.timeout.as_duration();
    match timeout(
        deadline,
        task::spawn_blocking(move || native_time_offset_blocking(&name, deadline)),
    )
    .await
    {
        Ok(Ok(item)) => item,
        _ => StatusItem::new(fallback_name, Health::Unknown, "time --"),
    }
}

fn native_time_offset_blocking(name: &str, deadline: Duration) -> StatusItem {
    let address = "1.1.1.1:80".parse().expect("valid clock address");
    let mut stream = match TcpStream::connect_timeout(&address, deadline) {
        Ok(stream) => stream,
        Err(_) => return StatusItem::new(name, Health::Unknown, "time --"),
    };
    if stream.set_read_timeout(Some(deadline)).is_err()
        || stream.set_write_timeout(Some(deadline)).is_err()
        || stream
            .write_all(b"HEAD / HTTP/1.1\r\nHost: 1.1.1.1\r\nConnection: close\r\n\r\n")
            .is_err()
    {
        return StatusItem::new(name, Health::Unknown, "time --");
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return StatusItem::new(name, Health::Unknown, "time --");
    }
    let Some(date) = http_header(&response, "date") else {
        return StatusItem::new(name, Health::Unknown, "time --");
    };
    let Ok(remote) = DateTime::parse_from_rfc2822(date) else {
        return StatusItem::new(name, Health::Unknown, "time --");
    };
    let diff = remote.timestamp() - Local::now().timestamp();
    if !(-2..=2).contains(&diff) {
        StatusItem::new(name, Health::Critical, format!("󰥔 {diff}"))
    } else {
        StatusItem::new(name, Health::Ok, "")
    }
}

fn http_header<'a>(response: &'a str, wanted: &str) -> Option<&'a str> {
    response.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(wanted).then(|| value.trim())
    })
}

fn native_docker(name: &str) -> StatusItem {
    match docker_counts() {
        Ok((running, stopped)) => {
            let expected = std::env::var("DOCKER_EXPECTED")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let health = if stopped > 0 {
                Health::Critical
            } else if expected > 0 && running != expected {
                Health::Warning
            } else {
                Health::Ok
            };
            let text = if stopped > 0 {
                format!(" {running}/{stopped}")
            } else {
                format!(" {running}")
            };
            StatusItem::new(name, health, text)
        }
        Err(err) => StatusItem::new(name, Health::Unknown, " --").with_detail(err.to_string()),
    }
}

fn docker_host_socket(host: &str) -> Option<PathBuf> {
    host.strip_prefix("unix://")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn docker_context_socket(home: &Path) -> Option<PathBuf> {
    let config: Value =
        serde_json::from_str(&fs::read_to_string(home.join(".docker/config.json")).ok()?).ok()?;
    let context = config.get("currentContext")?.as_str()?;
    let entries = fs::read_dir(home.join(".docker/contexts/meta")).ok()?;
    for entry in entries.flatten() {
        let Ok(text) = fs::read_to_string(entry.path().join("meta.json")) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if metadata.get("Name").and_then(Value::as_str) != Some(context) {
            continue;
        }
        return metadata
            .pointer("/Endpoints/docker/Host")
            .and_then(Value::as_str)
            .and_then(docker_host_socket);
    }
    None
}

fn docker_socket_candidates() -> Vec<PathBuf> {
    let mut sockets = Vec::new();
    if let Ok(host) = std::env::var("DOCKER_HOST")
        && let Some(path) = docker_host_socket(&host)
    {
        sockets.push(path);
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Some(path) = docker_context_socket(&home) {
            sockets.push(path);
        }
        sockets.push(home.join(".docker/run/docker.sock"));
        if let Ok(entries) = fs::read_dir(home.join(".colima")) {
            let mut profiles = entries
                .flatten()
                .map(|entry| entry.path().join("docker.sock"))
                .collect::<Vec<_>>();
            profiles.sort();
            sockets.extend(profiles);
        }
        sockets.push(home.join(".colima/docker.sock"));
    }
    sockets.push(PathBuf::from("/var/run/docker.sock"));
    sockets.dedup();
    sockets
}

fn docker_counts() -> Result<(usize, usize)> {
    let mut last_error = None;
    for socket in docker_socket_candidates() {
        match (
            docker_container_count(&socket, "/containers/json"),
            docker_container_count(
                &socket,
                "/containers/json?filters=%7B%22status%22%3A%5B%22exited%22%5D%7D",
            ),
        ) {
            (Ok(running), Ok(stopped)) => return Ok((running, stopped)),
            (Err(err), _) | (_, Err(err)) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no Docker socket candidates")))
}

fn docker_container_count(socket: &Path, path: &str) -> Result<usize> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connect Docker socket {}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: docker\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let Some((_, body)) = response.split_once("\r\n\r\n") else {
        anyhow::bail!("malformed docker response");
    };
    Ok(serde_json::from_str::<Vec<Value>>(body)?.len())
}

async fn run_command_check(check: &CheckConfig) -> StatusItem {
    let Some(command) = &check.command else {
        return StatusItem::new(
            &check.name,
            Health::Unknown,
            format!("{} no command", check.name),
        );
    };
    let Some((program, args)) = command.split_first() else {
        return StatusItem::new(
            &check.name,
            Health::Unknown,
            format!("{} empty command", check.name),
        );
    };

    let mut child = Command::new(program);
    child.args(args);
    child.kill_on_drop(true);
    let output = match timeout(check.timeout.as_duration(), child.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return StatusItem::new(
                &check.name,
                Health::Unknown,
                format!("{} {err}", check.name),
            );
        }
        Err(_) => {
            return StatusItem::new(&check.name, Health::Critical, &check.timeout_text)
                .with_detail(format!(
                    "{} timed out after {}ms",
                    check.name,
                    check.timeout.0.as_millis()
                ));
        }
    };

    let code = output.status.code().unwrap_or(3);
    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim()
        .replace('\n', " ");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim()
        .replace('\n', " ");
    let text = if stdout.is_empty() { stderr } else { stdout };
    StatusItem::new(&check.name, Health::from_exit_code(code), text)
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
