use super::*;

pub(super) fn check_result(item: StatusItem) -> CheckResult {
    CheckResult {
        item,
        timestamp: now(),
        cpu_sample: None,
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn native_cpu(check: &CheckConfig, previous: Option<CpuSample>) -> CheckResult {
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
pub(super) fn native_cpu(check: &CheckConfig, _previous: Option<CpuSample>) -> CheckResult {
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
pub(super) fn read_cpu_sample() -> Option<CpuSample> {
    let text = fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    cpu_sample_from_proc_stat_line(line)
}

#[cfg(any(test, target_os = "macos"))]
pub(super) fn load_average_percent(load: f64, cores: usize) -> u64 {
    ((load * 100.0 / cores.max(1) as f64).round() as u64).min(100)
}

#[cfg(target_os = "macos")]
pub(super) fn mac_load_average() -> Option<f64> {
    let mut values = [0.0];
    // getloadavg writes at most the single f64 slot provided here.
    let count = unsafe { getloadavg(values.as_mut_ptr(), 1) };
    (count == 1).then_some(values[0])
}

#[cfg(any(test, not(target_os = "macos")))]
pub(super) fn cpu_sample_from_proc_stat_line(line: &str) -> Option<CpuSample> {
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
pub(super) fn cpu_busy_percent(current: CpuSample, previous: Option<CpuSample>) -> Option<u64> {
    let previous = previous.unwrap_or(CpuSample { active: 0, idle: 0 });
    let active = current.active.checked_sub(previous.active)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let total = active + idle;
    active.checked_mul(100)?.checked_div(total)
}

pub(super) fn memory_used_percent(total: u64, available: u64) -> u64 {
    if total == 0 {
        0
    } else {
        (total.saturating_sub(available).saturating_mul(100) / total).min(100)
    }
}

#[cfg(target_os = "linux")]
pub(super) async fn native_mem(check: &CheckConfig) -> StatusItem {
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
pub(super) struct MacMetrics {
    memory_used_pct: u64,
    cpu_temp_c: f64,
}

#[cfg(target_os = "macos")]
pub(super) fn read_mac_metrics() -> Result<MacMetrics> {
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
pub(super) async fn native_mem(check: &CheckConfig) -> StatusItem {
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
pub(super) async fn native_mem(check: &CheckConfig) -> StatusItem {
    StatusItem::new(&check.name, Health::Unknown, " --")
        .with_detail("memory collector is unavailable on this platform")
}

#[cfg(target_os = "linux")]
pub(super) async fn native_temp(check: &CheckConfig) -> StatusItem {
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
pub(super) async fn native_temp(check: &CheckConfig) -> StatusItem {
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
pub(super) async fn native_temp(check: &CheckConfig) -> StatusItem {
    StatusItem::new(&check.name, Health::Unknown, "")
        .with_detail("temperature collector is unavailable on this platform")
}

pub(super) fn native_script_status(check: &CheckConfig) -> StatusItem {
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

pub(super) fn script_status_failures(text: &str, today: &str) -> Option<Vec<String>> {
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

pub(super) async fn native_alertmanager(check: &CheckConfig) -> StatusItem {
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

pub(super) fn alertmanager_error(check: &CheckConfig, detail: &str) -> StatusItem {
    StatusItem::new(&check.name, Health::Warning, "󰔟").with_detail(detail)
}

pub(super) fn alert_counts(
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

pub(super) fn amtool_alertmanager_url(config: &str) -> Option<&str> {
    config.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "alertmanager.url").then(|| {
            value
                .trim()
                .trim_matches(|character| character == '"' || character == '\'')
        })
    })
}

pub(super) fn check_source(check: &CheckConfig, default_relative: &str) -> Option<PathBuf> {
    check
        .source
        .clone()
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(default_relative)))
}

pub(super) async fn native_weather(check: &CheckConfig) -> StatusItem {
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

pub(super) fn native_time_panel(check: &CheckConfig) -> StatusItem {
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

pub(super) fn timezones_for(check: &CheckConfig) -> Vec<TimezoneConfig> {
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

pub(super) fn format_timezone_line(label: &str, zone: &str, timestamp: i64) -> String {
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

pub(super) fn month_calendar(year: i32, month: u32) -> Vec<String> {
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

pub(super) fn days_in_month(year: i32, month: u32) -> u32 {
    let next = if month == 12 {
        chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    next.and_then(|date| date.pred_opt())
        .map_or(30, |date| date.day())
}

pub(super) fn month_name(month: u32) -> &'static str {
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

pub(super) fn encode_wttr_location(location: &str) -> String {
    location.trim().replace(' ', "+")
}

pub(super) fn weather_icon(condition: &str) -> &'static str {
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

pub(super) fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) async fn native_todo_panel(name: &str) -> StatusItem {
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

pub(super) fn native_safe(name: &str) -> StatusItem {
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

pub(super) async fn native_resolv(check: &CheckConfig) -> StatusItem {
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

pub(super) fn native_resolv_blocking(name: &str) -> StatusItem {
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

pub(super) fn resolv_nameserver() -> Option<SocketAddr> {
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

pub(super) fn dns_query_packet(domain: &str, id: u16) -> Option<Vec<u8>> {
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

pub(super) fn dns_query(server: SocketAddr, domain: &str, deadline: Duration) -> bool {
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

pub(super) async fn native_time_offset(check: &CheckConfig) -> StatusItem {
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

pub(super) fn native_time_offset_blocking(name: &str, deadline: Duration) -> StatusItem {
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

pub(super) fn http_header<'a>(response: &'a str, wanted: &str) -> Option<&'a str> {
    response.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(wanted).then(|| value.trim())
    })
}

pub(super) fn native_docker(name: &str) -> StatusItem {
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

pub(super) fn docker_host_socket(host: &str) -> Option<PathBuf> {
    host.strip_prefix("unix://")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

pub(super) fn docker_context_socket(home: &Path) -> Option<PathBuf> {
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

pub(super) fn docker_socket_candidates() -> Vec<PathBuf> {
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

pub(super) fn docker_counts() -> Result<(usize, usize)> {
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

pub(super) fn docker_container_count(socket: &Path, path: &str) -> Result<usize> {
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

pub(super) async fn run_command_check(check: &CheckConfig) -> StatusItem {
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

pub(super) async fn run_check_with_previous(
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
