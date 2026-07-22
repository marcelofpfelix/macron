use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Local, TimeZone};
use chrono_tz::Tz;
use clap::{Parser, Subcommand, ValueEnum};
use rush_core::{Health, StatusItem, SurfaceFormat, render_status_line, render_tui_panel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
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
    /// Config file. Defaults to built-in local bar compatibility checks.
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
        format: RenderFormat,
        surface: Option<String>,
    },
    /// Run one check and print JSON.
    Check { name: String },
    /// List configured checks.
    Checks,
    /// Print Prometheus textfile-compatible metrics from fresh check results.
    Metrics,
    /// Validate config and print resolved runtime paths.
    Doctor,
    /// Render a compact Ratatui board view from cache-first state.
    Tui { surface: Option<String> },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RenderFormat {
    Plain,
    Text,
    Tmux,
    Quickshell,
}

impl From<RenderFormat> for SurfaceFormat {
    fn from(value: RenderFormat) -> Self {
        match value {
            RenderFormat::Plain | RenderFormat::Text => Self::Plain,
            RenderFormat::Tmux => Self::Tmux,
            RenderFormat::Quickshell => Self::Quickshell,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    #[serde(default)]
    check: Vec<CheckConfig>,
    #[serde(default)]
    surface: Vec<SurfaceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckConfig {
    name: String,
    kind: CheckKind,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default = "default_interval")]
    interval: HumanDuration,
    #[serde(default = "default_timeout")]
    timeout: HumanDuration,
    #[serde(default)]
    fresh_for: Option<HumanDuration>,
    command: Option<Vec<String>>,
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
    NativeDocker,
    Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SurfaceConfig {
    name: String,
    checks: Vec<String>,
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
    let config = load_config(cli.config.as_deref())?;
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
            format,
            surface,
        } => {
            let items = if fresh {
                render_surface_fresh(&config, surface.as_deref()).await?
            } else {
                render_surface_cache_first(&config, &cache_dir, surface.as_deref()).await?
            };
            let output = match format {
                RenderFormat::Text => render_text_items(&items),
                _ => render_status_line(&items, format.into()),
            };
            println!("{output}");
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
        Commands::Checks => {
            for check in &config.check {
                let state = if check.enabled { "enabled" } else { "disabled" };
                println!(
                    "{} {:?} {} every {}",
                    check.name,
                    check.kind,
                    state,
                    String::from(check.interval)
                );
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
            println!("cache_dir: {}", cache_dir.display());
        }
        Commands::Tui { surface } => {
            let items = render_surface_cache_first(&config, &cache_dir, surface.as_deref()).await?;
            let rows = items
                .iter()
                .map(|item| format!("{:<10} {:<8} {}", item.name, item.health, item.text))
                .collect::<Vec<_>>();
            println!("{}", render_tui_panel("board", &rows, 100, 24));
        }
    }

    Ok(())
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

fn render_text_items(items: &[StatusItem]) -> String {
    items
        .iter()
        .filter_map(|item| {
            let text = item.text.trim();
            (!text.is_empty()).then_some(text)
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
    let mut results = Vec::new();
    for check in checks.iter().filter(|check| check.enabled) {
        results.push(run_check(check).await);
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
        CheckKind::NativeCpu => {
            native_cpu(&check.name, previous.and_then(|result| result.cpu_sample))
        }
        CheckKind::NativeMem => check_result(native_mem(&check.name)),
        CheckKind::NativeTemp => check_result(native_temp(&check.name)),
        CheckKind::NativeWeather => check_result(native_weather(&check.name).await),
        CheckKind::NativeTimePanel => check_result(native_time_panel(&check.name)),
        CheckKind::NativeTodoPanel => check_result(native_todo_panel(&check.name).await),
        CheckKind::NativeSafe => check_result(native_safe(&check.name)),
        CheckKind::NativeResolv => check_result(native_resolv(&check.name)),
        CheckKind::NativeTimeOffset => check_result(native_time_offset(&check.name).await),
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

fn native_cpu(name: &str, previous: Option<CpuSample>) -> CheckResult {
    match read_cpu_sample() {
        Some(sample) => {
            let busy_pct = cpu_busy_percent(sample, previous).unwrap_or(0);
            CheckResult {
                item: StatusItem::new(
                    name,
                    health_percent(busy_pct, 60, 85),
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

fn read_cpu_sample() -> Option<CpuSample> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    cpu_sample_from_proc_stat_line(line)
}

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

fn cpu_busy_percent(current: CpuSample, previous: Option<CpuSample>) -> Option<u64> {
    let previous = previous.unwrap_or(CpuSample { active: 0, idle: 0 });
    let active = current.active.checked_sub(previous.active)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let total = active + idle;
    if total == 0 {
        None
    } else {
        Some(active * 100 / total)
    }
}

fn native_mem(name: &str) -> StatusItem {
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
                return StatusItem::new(name, Health::Unknown, "  0");
            }
            let used_pct = 100 - (available * 100 / total);
            StatusItem::new(
                name,
                health_percent(used_pct, 60, 85),
                format!(" {used_pct:2}"),
            )
        }
        Err(_) => StatusItem::new(name, Health::Unknown, "  0"),
    }
}

fn native_temp(name: &str) -> StatusItem {
    let zones = match fs::read_dir("/sys/class/thermal") {
        Ok(zones) => zones,
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰔐  0"),
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
            health_percent(temp as u64, 60, 80),
            format!("󰔐 {temp:2}"),
        ),
        None => StatusItem::new(name, Health::Unknown, "󰔐  0"),
    }
}

async fn native_weather(name: &str) -> StatusItem {
    let name = name.to_string();
    let fallback_name = name.clone();
    task::spawn_blocking(move || native_weather_blocking(&name))
        .await
        .unwrap_or_else(|_| StatusItem::new(fallback_name, Health::Unknown, "󰖐 --"))
}

fn native_weather_blocking(name: &str) -> StatusItem {
    let location = std::env::var("WEATHER_LOCATION").unwrap_or_else(|_| "Lisbon".to_string());
    let url = format!(
        "https://wttr.in/{}?format=%C+%t",
        encode_wttr_location(&location)
    );
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰖐 --"),
    };
    let text = match client.get(url).send().and_then(|response| response.text()) {
        Ok(text) => collapse_whitespace(&text),
        Err(_) => return StatusItem::new(name, Health::Unknown, "󰖐 --"),
    };
    if text.is_empty() {
        return StatusItem::new(name, Health::Unknown, "󰖐 --");
    }

    let (condition, temp) = text.rsplit_once(' ').unwrap_or(("", &text));
    let icon = weather_icon(condition);
    StatusItem::new(name, Health::Ok, format!("{icon} {temp}"))
}

fn native_time_panel(name: &str) -> StatusItem {
    let now = Local::now();
    let mut lines = vec!["Timezones".to_string()];
    for (label, zone) in [
        ("Lisbon", "Europe/Lisbon"),
        ("UTC", "UTC"),
        ("New York", "America/New_York"),
        ("SF", "America/Los_Angeles"),
    ] {
        lines.push(format_timezone_line(label, zone, now.timestamp()));
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

fn native_resolv(name: &str) -> StatusItem {
    let deadline = Duration::from_secs(1);
    let dns_ok = ("bandonga.com", 80)
        .to_socket_addrs()
        .map(|mut addrs| addrs.next().is_some())
        .unwrap_or(false);
    if dns_ok {
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

async fn native_time_offset(name: &str) -> StatusItem {
    let name = name.to_string();
    let fallback_name = name.clone();
    task::spawn_blocking(move || native_time_offset_blocking(&name))
        .await
        .unwrap_or_else(|_| StatusItem::new(fallback_name, Health::Unknown, "time --"))
}

fn native_time_offset_blocking(name: &str) -> StatusItem {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => client,
        Err(_) => return StatusItem::new(name, Health::Unknown, "time --"),
    };
    let response = match client.head("http://1.1.1.1").send() {
        Ok(response) => response,
        Err(_) => return StatusItem::new(name, Health::Unknown, "time --"),
    };
    let Some(date) = response.headers().get(reqwest::header::DATE) else {
        return StatusItem::new(name, Health::Unknown, "time --");
    };
    let Ok(date) = date.to_str() else {
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
        Err(_) => StatusItem::new(name, Health::Unknown, " --"),
    }
}

fn docker_counts() -> Result<(usize, usize)> {
    let running = docker_container_count("/containers/json")?;
    let stopped =
        docker_container_count("/containers/json?filters=%7B%22status%22%3A%5B%22exited%22%5D%7D")?;
    Ok((running, stopped))
}

fn docker_container_count(path: &str) -> Result<usize> {
    let mut stream =
        UnixStream::connect("/var/run/docker.sock").context("connect docker socket")?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n"
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
            return StatusItem::new(
                &check.name,
                Health::Critical,
                format!("{} timeout", check.name),
            );
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

fn default_config() -> Config {
    Config {
        check: vec![
            native_check("cpu", CheckKind::NativeCpu, "5s"),
            native_check("mem", CheckKind::NativeMem, "10s"),
            native_check("temp", CheckKind::NativeTemp, "30s"),
            disabled_native_check("weather", CheckKind::NativeWeather, "15m"),
            native_check("time-panel", CheckKind::NativeTimePanel, "1s"),
            native_check("todo-panel", CheckKind::NativeTodoPanel, "60s"),
            quickshell_command_check("scripts", "check-scripts", "10s"),
            quickshell_command_check("alerts", "alerts", "10s"),
            native_check("time", CheckKind::NativeTimeOffset, "10s"),
            native_check("resolv", CheckKind::NativeResolv, "15s"),
            native_check("safe", CheckKind::NativeSafe, "30s"),
            command_check("gpg", "check-gpg", "30s"),
            native_check("docker", CheckKind::NativeDocker, "30s"),
            command_check("agents", "check-agents", "10s"),
        ],
        surface: vec![
            SurfaceConfig {
                name: "tmux-top".to_string(),
                checks: vec!["agents", "gpg", "docker", "cpu", "mem"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
            SurfaceConfig {
                name: "quickshell-bar".to_string(),
                checks: vec![
                    "scripts", "alerts", "time", "resolv", "safe", "gpg", "docker", "agents",
                    "cpu", "mem", "temp", "weather",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
            SurfaceConfig {
                name: "quickshell-weather".to_string(),
                checks: vec!["weather"].into_iter().map(str::to_string).collect(),
            },
            SurfaceConfig {
                name: "quickshell-time-panel".to_string(),
                checks: vec!["time-panel"].into_iter().map(str::to_string).collect(),
            },
            SurfaceConfig {
                name: "quickshell-todo-panel".to_string(),
                checks: vec!["todo-panel"].into_iter().map(str::to_string).collect(),
            },
            SurfaceConfig {
                name: "quickshell-panels".to_string(),
                checks: vec!["time-panel", "todo-panel"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            },
        ],
    }
}

fn native_check(name: &str, kind: CheckKind, interval: &str) -> CheckConfig {
    CheckConfig {
        name: name.to_string(),
        kind,
        interval: HumanDuration(parse_duration(interval).expect("valid default interval")),
        timeout: default_timeout(),
        fresh_for: None,
        command: None,
        enabled: true,
    }
}

fn disabled_native_check(name: &str, kind: CheckKind, interval: &str) -> CheckConfig {
    CheckConfig {
        enabled: false,
        ..native_check(name, kind, interval)
    }
}

fn command_check(name: &str, command: &str, interval: &str) -> CheckConfig {
    CheckConfig {
        name: name.to_string(),
        kind: CheckKind::Command,
        interval: HumanDuration(parse_duration(interval).expect("valid default interval")),
        timeout: HumanDuration(Duration::from_secs(2)),
        fresh_for: None,
        command: Some(vec![command.to_string()]),
        enabled: true,
    }
}

fn quickshell_command_check(name: &str, command: &str, interval: &str) -> CheckConfig {
    CheckConfig {
        name: name.to_string(),
        kind: CheckKind::Command,
        interval: HumanDuration(parse_duration(interval).expect("valid default interval")),
        timeout: HumanDuration(Duration::from_secs(2)),
        fresh_for: None,
        command: Some(vec![
            "env".to_string(),
            "BAR_COLOR_FORMAT=quickshell".to_string(),
            command.to_string(),
        ]),
        enabled: true,
    }
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

fn health_percent(value: u64, warning: u64, critical: u64) -> Health {
    if value >= critical {
        Health::Critical
    } else if value >= warning {
        Health::Warning
    } else {
        Health::Ok
    }
}

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
    fn reads_meminfo_value() {
        assert_eq!(
            meminfo_value("MemAvailable:   42 kB", "MemAvailable:"),
            Some(42)
        );
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
    fn text_renderer_omits_health_and_empty_items() {
        let items = vec![
            StatusItem::new("empty", Health::Ok, ""),
            StatusItem::new("panel", Health::Warning, "hello"),
        ];
        assert_eq!(render_text_items(&items), "hello");
    }

    #[test]
    fn renders_native_time_panel_without_shelling_out() {
        let item = native_time_panel("time-panel");
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
    fn parses_docker_http_body_count() {
        let response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"Id\":\"1\"},{\"Id\":\"2\"}]";
        let (_, body) = response.split_once("\r\n\r\n").unwrap();
        assert_eq!(serde_json::from_str::<Vec<Value>>(body).unwrap().len(), 2);
    }

    #[test]
    fn stale_cache_expires_after_fresh_window() {
        let check = CheckConfig {
            name: "cpu".to_string(),
            kind: CheckKind::NativeCpu,
            interval: HumanDuration(Duration::from_secs(5)),
            timeout: HumanDuration(Duration::from_secs(1)),
            fresh_for: Some(HumanDuration(Duration::from_secs(10))),
            command: None,
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
}
