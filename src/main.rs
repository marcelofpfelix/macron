use clap::{Parser, Subcommand, ValueEnum};
use owo_colors::{OwoColorize, Style};
use plist::{Dictionary, Value};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const STORE_ENV: &str = "MACRON_FILE";
const DEFAULT_LABEL_PREFIX: &str = "local.macron";

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Convert macOS launchd calendar jobs to and from crontab format"
)]
struct Cli {
    /// Edit the local macron crontab file using $VISUAL, $EDITOR, or vi.
    #[arg(short = 'e', long = "edit", global = true)]
    edit: bool,

    /// Include Apple OS-managed /System/Library launchd jobs when discovering schedules.
    #[arg(long = "system", global = true)]
    system: bool,

    /// Use a custom local crontab-format file.
    #[arg(short = 'f', long = "file", global = true)]
    file: Option<PathBuf>,

    /// When to colorize crontab output.
    #[arg(long = "color", value_enum, default_value_t = OutputColor::Auto, global = true)]
    color: OutputColor,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputColor {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Print the local crontab-format file.
    List,
    /// Import launchd plist files into the local crontab-format file.
    Import {
        /// Plist files or directories to import. Defaults to the common LaunchAgents/LaunchDaemons paths.
        paths: Vec<PathBuf>,
    },
    /// Export the local crontab-format file to launchd plist files.
    Export {
        /// Directory where launchd plist files will be written.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Print the resolved local crontab-format file path.
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronEntry {
    schedule: CronSchedule,
    command: String,
    label: Option<String>,
    source: Option<PathBuf>,
    interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronSchedule {
    minute: String,
    hour: String,
    day_of_month: String,
    month: String,
    day_of_week: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LaunchdJob {
    label: String,
    program_arguments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_calendar_interval: Option<CalendarInterval>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    working_directory: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum CalendarInterval {
    One(CalendarFields),
    Many(Vec<CalendarFields>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CalendarFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    minute: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hour: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    day: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    month: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weekday: Option<u64>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("macron: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let store = cli.file.unwrap_or_else(default_store_path);

    if cli.edit {
        seed_store_if_needed(&store, cli.system)?;
        edit_store(&store)?;
        export_store(&store, None)?;
        return Ok(());
    }

    match cli.command.unwrap_or(Commands::List) {
        Commands::List => {
            refresh_store_from_paths(&store, default_import_paths(cli.system), io::sink())?;
            print_store(&store, cli.color)?;
        }
        Commands::Import { paths } => import_plists(&store, paths, cli.system)?,
        Commands::Export { output } => export_store(&store, output)?,
        Commands::File => println!("{}", store.display()),
    }

    Ok(())
}

fn default_store_path() -> PathBuf {
    if let Some(path) = env::var_os(STORE_ENV) {
        return PathBuf::from(path);
    }
    home_dir().join(".macron").join("crontab")
}

fn default_export_dir() -> PathBuf {
    home_dir().join("Library").join("LaunchAgents")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn edit_store(store: &Path) -> Result<(), Box<dyn std::error::Error>> {
    ensure_parent(store)?;
    if !store.exists() {
        fs::write(
            store,
            "# macron crontab\n# minute hour day-of-month month day-of-week command\n",
        )?;
    }

    let editor = env::var_os("VISUAL")
        .or_else(|| env::var_os("EDITOR"))
        .unwrap_or_else(|| OsString::from("vi"));
    let status = Command::new(editor).arg(store).status()?;
    if !status.success() {
        return Err("editor exited unsuccessfully".into());
    }
    Ok(())
}

fn print_store(store: &Path, color: OutputColor) -> io::Result<()> {
    match fs::read_to_string(store) {
        Ok(contents) => {
            print!("{}", render_store(&contents, should_color(color)));
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn should_color(color: OutputColor) -> bool {
    match color {
        OutputColor::Always => true,
        OutputColor::Never => false,
        OutputColor::Auto => io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
    }
}

fn render_store(contents: &str, color: bool) -> String {
    if !color {
        return contents.to_string();
    }

    let mut rendered = String::new();
    for line in contents.lines() {
        rendered.push_str(&colorize_line(line));
        rendered.push('\n');
    }
    rendered
}

fn colorize_line(line: &str) -> String {
    let trimmed = line.trim_start();
    let leading_len = line.len() - trimmed.len();
    let leading = &line[..leading_len];

    if trimmed.is_empty() {
        return line.to_string();
    }

    if let Some(metadata) = trimmed.strip_prefix("# macron:") {
        let (key, value) = metadata.split_once('=').unwrap_or((metadata, ""));
        return format!(
            "{}{}{}{}{}",
            leading,
            "# macron:".style(comment_style()),
            key.style(metadata_key_style()),
            "=".style(comment_style()),
            value.style(metadata_value_style())
        );
    }

    if trimmed.starts_with('#') {
        return format!("{leading}{}", trimmed.style(comment_style()));
    }

    colorize_cron_line(line)
}

fn colorize_cron_line(line: &str) -> String {
    let fields = line.split_whitespace().take(6).collect::<Vec<_>>();
    if fields.len() < 6 {
        return line.to_string();
    }

    let Some(command_start) = nth_field_start(line, 5) else {
        return line.to_string();
    };
    let leading_len = line.len() - line.trim_start().len();
    let leading = &line[..leading_len];
    let command = line[command_start..].trim();

    format!(
        "{}{} {} {} {} {} {}",
        leading,
        fields[0].style(minute_style()),
        fields[1].style(hour_style()),
        fields[2].style(day_style()),
        fields[3].style(month_style()),
        fields[4].style(weekday_style()),
        colorize_shell(command)
    )
}

fn colorize_shell(command: &str) -> String {
    let Ok(tokens) = shell_words::split(command) else {
        return command.style(command_style()).to_string();
    };

    let mut rendered = Vec::new();
    for token in tokens {
        let styled = if token.starts_with('-') {
            token.style(shell_flag_style()).to_string()
        } else if token.contains('/') || token.ends_with(".sh") {
            shell_words::quote(&token)
                .style(shell_path_style())
                .to_string()
        } else {
            shell_words::quote(&token)
                .style(command_style())
                .to_string()
        };
        rendered.push(styled);
    }
    rendered.join(" ")
}

fn comment_style() -> Style {
    Style::new().bright_black()
}

fn metadata_key_style() -> Style {
    Style::new().blue().bold()
}

fn metadata_value_style() -> Style {
    Style::new().cyan()
}

fn minute_style() -> Style {
    Style::new().cyan().bold()
}

fn hour_style() -> Style {
    Style::new().green().bold()
}

fn day_style() -> Style {
    Style::new().yellow().bold()
}

fn month_style() -> Style {
    Style::new().magenta().bold()
}

fn weekday_style() -> Style {
    Style::new().red().bold()
}

fn command_style() -> Style {
    Style::new().white()
}

fn shell_path_style() -> Style {
    Style::new().bright_green()
}

fn shell_flag_style() -> Style {
    Style::new().bright_yellow()
}

fn seed_store_if_needed(
    store: &Path,
    include_system: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let needs_seed = match fs::read_to_string(store) {
        Ok(contents) => contents.trim().is_empty(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };

    if needs_seed {
        refresh_store_from_paths(store, default_import_paths(include_system), io::sink())?;
    }

    Ok(())
}

fn import_plists(
    store: &Path,
    paths: Vec<PathBuf>,
    include_system: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths = if paths.is_empty() {
        default_import_paths(include_system)
    } else {
        paths
    };

    let imported = refresh_store_from_paths(store, paths, io::stderr())?;
    println!("imported {} job(s) into {}", imported, store.display());
    Ok(())
}

fn refresh_store_from_paths<W: Write>(
    store: &Path,
    paths: Vec<PathBuf>,
    mut warnings: W,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut imported = Vec::new();
    for plist_path in plist_files(paths)? {
        match plist_to_entries(&plist_path) {
            Ok(mut entries) => imported.append(&mut entries),
            Err(error) => writeln!(
                warnings,
                "macron: skipped {}: {error}",
                plist_path.display()
            )?,
        }
    }

    ensure_parent(store)?;
    let contents = format_entries_file(&imported);
    fs::write(store, contents)?;
    Ok(imported.len())
}

fn format_entries_file(entries: &[CronEntry]) -> String {
    let mut contents =
        "# macron crontab\n# minute hour day-of-month month day-of-week command\n".to_string();
    for entry in entries {
        if let Some(source) = &entry.source {
            contents.push_str(&format!("# macron:source={}\n", source.display()));
        }
        if let Some(label) = &entry.label {
            contents.push_str(&format!("# macron:label={label}\n"));
        }
        if let Some(interval_seconds) = entry.interval_seconds {
            contents.push_str(&format!("# macron:start-interval={interval_seconds}\n"));
        }
        contents.push_str(&format!("{}\n", format_entry(entry)));
    }
    contents
}

fn export_store(store: &Path, output: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let output = output.unwrap_or_else(default_export_dir);
    fs::create_dir_all(&output)?;

    let contents = fs::read_to_string(store)?;
    let entries = parse_crontab(&contents)?;
    let mut exported = 0;
    for (index, entry) in entries.iter().enumerate() {
        let job = entry_to_launchd_job(entry, index)?;
        let file_name = format!("{}.plist", job.label);
        let target = output.join(file_name);
        let value = plist::to_value(&job)?;
        value.to_file_xml(&target)?;
        exported += 1;
    }
    println!("exported {exported} job(s) to {}", output.display());
    Ok(())
}

fn default_import_paths(include_system: bool) -> Vec<PathBuf> {
    let mut paths = vec![
        home_dir().join("Library").join("LaunchAgents"),
        PathBuf::from("/Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchDaemons"),
    ];
    if include_system {
        paths.push(PathBuf::from("/System/Library/LaunchAgents"));
        paths.push(PathBuf::from("/System/Library/LaunchDaemons"));
    }
    paths
}

fn plist_files(paths: Vec<PathBuf>) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_file() {
            if path
                .extension()
                .is_some_and(|extension| extension == "plist")
            {
                files.push(path);
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "plist")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn plist_to_entries(path: &Path) -> Result<Vec<CronEntry>, Box<dyn std::error::Error>> {
    let value = Value::from_file(path)?;
    let dict = value
        .as_dictionary()
        .ok_or("plist root is not a dictionary")?;
    let label = dict
        .get("Label")
        .and_then(Value::as_string)
        .map(str::to_owned);
    let command = launchd_command(dict).ok_or("missing ProgramArguments or Program")?;
    let (schedules, interval_seconds) = if let Some(intervals) = dict.get("StartCalendarInterval") {
        (calendar_value_to_schedules(intervals)?, None)
    } else if let Some(interval) = dict.get("StartInterval") {
        let seconds = start_interval_seconds(interval)?;
        (vec![start_interval_to_schedule(seconds)?], Some(seconds))
    } else {
        return Err("missing StartCalendarInterval or StartInterval".into());
    };

    Ok(schedules
        .into_iter()
        .map(|schedule| CronEntry {
            schedule,
            command: command.clone(),
            label: label.clone(),
            source: Some(path.to_path_buf()),
            interval_seconds,
        })
        .collect())
}

fn launchd_command(dict: &Dictionary) -> Option<String> {
    if let Some(args) = dict.get("ProgramArguments").and_then(Value::as_array) {
        let words = args
            .iter()
            .map(Value::as_string)
            .collect::<Option<Vec<_>>>()?;
        if words.len() >= 3 && words[0] == "/bin/sh" && words[1] == "-lc" {
            return Some(words[2..].join(" "));
        }
        return Some(shell_words::join(words));
    }

    dict.get("Program")
        .and_then(Value::as_string)
        .map(str::to_owned)
}

fn start_interval_seconds(value: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    let seconds = value
        .as_unsigned_integer()
        .ok_or("StartInterval is not an unsigned integer")?;
    Ok(seconds)
}

fn start_interval_to_schedule(seconds: u64) -> Result<CronSchedule, Box<dyn std::error::Error>> {
    if seconds == 0 || !seconds.is_multiple_of(60) {
        return Err(
            format!("StartInterval {seconds} cannot be represented in crontab minutes").into(),
        );
    }

    let minutes = seconds / 60;
    let (minute, hour) = match minutes {
        1 => ("*".to_string(), "*".to_string()),
        2..=59 => (format!("*/{minutes}"), "*".to_string()),
        60 => ("0".to_string(), "*".to_string()),
        61..=1439 if minutes.is_multiple_of(60) => ("0".to_string(), format!("*/{}", minutes / 60)),
        1440 => ("0".to_string(), "0".to_string()),
        _ => {
            return Err(format!(
                "StartInterval {seconds} cannot be represented as a simple five-field crontab schedule"
            )
            .into());
        }
    };

    Ok(CronSchedule {
        minute,
        hour,
        day_of_month: "*".to_string(),
        month: "*".to_string(),
        day_of_week: "*".to_string(),
    })
}

fn calendar_value_to_schedules(
    value: &Value,
) -> Result<Vec<CronSchedule>, Box<dyn std::error::Error>> {
    if let Some(dict) = value.as_dictionary() {
        return Ok(vec![calendar_dict_to_schedule(dict)]);
    }
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .map(|item| {
                item.as_dictionary()
                    .map(calendar_dict_to_schedule)
                    .ok_or_else(|| "StartCalendarInterval array contains a non-dictionary".into())
            })
            .collect();
    }
    Err("StartCalendarInterval must be a dictionary or an array of dictionaries".into())
}

fn calendar_dict_to_schedule(dict: &Dictionary) -> CronSchedule {
    CronSchedule {
        minute: plist_u64(dict, "Minute"),
        hour: plist_u64(dict, "Hour"),
        day_of_month: plist_u64(dict, "Day"),
        month: plist_u64(dict, "Month"),
        day_of_week: plist_u64(dict, "Weekday"),
    }
}

fn plist_u64(dict: &Dictionary, key: &str) -> String {
    dict.get(key)
        .and_then(Value::as_unsigned_integer)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "*".to_string())
}

fn parse_crontab(contents: &str) -> Result<Vec<CronEntry>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    let mut pending_label = None;
    let mut pending_source = None;
    let mut pending_interval_seconds = None;

    for (line_number, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(metadata) = line.strip_prefix("# macron:") {
            if let Some(label) = metadata.strip_prefix("label=") {
                pending_label = Some(label.trim().to_string());
            } else if let Some(source) = metadata.strip_prefix("source=") {
                pending_source = Some(PathBuf::from(source.trim()));
            } else if let Some(interval_seconds) = metadata.strip_prefix("start-interval=") {
                pending_interval_seconds = Some(interval_seconds.trim().parse::<u64>()?);
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }

        let parts = raw_line.split_whitespace().take(6).collect::<Vec<_>>();
        if parts.len() < 6 {
            return Err(format!(
                "line {}: expected five schedule fields and a command",
                line_number + 1
            )
            .into());
        }
        let command_start = nth_field_start(raw_line, 5)
            .ok_or_else(|| format!("line {}: missing command", line_number + 1))?;
        let entry = CronEntry {
            schedule: CronSchedule {
                minute: parts[0].to_string(),
                hour: parts[1].to_string(),
                day_of_month: parts[2].to_string(),
                month: parts[3].to_string(),
                day_of_week: parts[4].to_string(),
            },
            command: raw_line[command_start..].trim().to_string(),
            label: pending_label.take(),
            source: pending_source.take(),
            interval_seconds: pending_interval_seconds.take(),
        };
        entries.push(entry);
    }

    Ok(entries)
}

fn nth_field_start(line: &str, field_index: usize) -> Option<usize> {
    let mut in_field = false;
    let mut current = 0;
    for (index, character) in line.char_indices() {
        if character.is_whitespace() {
            in_field = false;
            continue;
        }
        if !in_field {
            if current == field_index {
                return Some(index);
            }
            current += 1;
            in_field = true;
        }
    }
    None
}

fn format_entry(entry: &CronEntry) -> String {
    format!(
        "{} {} {} {} {} {}",
        entry.schedule.minute,
        entry.schedule.hour,
        entry.schedule.day_of_month,
        entry.schedule.month,
        entry.schedule.day_of_week,
        entry.command
    )
}

fn entry_to_launchd_job(
    entry: &CronEntry,
    index: usize,
) -> Result<LaunchdJob, Box<dyn std::error::Error>> {
    let label = entry
        .label
        .clone()
        .unwrap_or_else(|| generated_label(entry, index));
    Ok(LaunchdJob {
        label,
        program_arguments: vec![
            "/bin/sh".to_string(),
            "-lc".to_string(),
            entry.command.clone(),
        ],
        start_calendar_interval: if entry.interval_seconds.is_some() {
            None
        } else {
            Some(schedule_to_calendar_interval(&entry.schedule)?)
        },
        start_interval: entry.interval_seconds,
        working_directory: None,
    })
}

fn generated_label(entry: &CronEntry, index: usize) -> String {
    let mut hasher = DefaultHasher::new();
    entry.command.hash(&mut hasher);
    entry.schedule.hash(&mut hasher);
    format!("{DEFAULT_LABEL_PREFIX}.{index}.{:x}", hasher.finish())
}

fn schedule_to_calendar_interval(
    schedule: &CronSchedule,
) -> Result<CalendarInterval, Box<dyn std::error::Error>> {
    let minutes = expand_cron_field(&schedule.minute, 0, 59, "minute")?;
    let hours = expand_cron_field(&schedule.hour, 0, 23, "hour")?;
    let days = expand_cron_field(&schedule.day_of_month, 1, 31, "day-of-month")?;
    let months = expand_cron_field(&schedule.month, 1, 12, "month")?;
    let weekdays = expand_cron_field(&schedule.day_of_week, 0, 7, "day-of-week")?;

    let minute_values = concrete_values(&minutes);
    let hour_values = concrete_values(&hours);
    let day_values = concrete_values(&days);
    let month_values = concrete_values(&months);
    let weekday_values = concrete_values(&weekdays);

    let mut intervals = Vec::new();
    for minute in minute_values {
        for hour in &hour_values {
            for day in &day_values {
                for month in &month_values {
                    for weekday in &weekday_values {
                        intervals.push(CalendarFields {
                            minute,
                            hour: *hour,
                            day: *day,
                            month: *month,
                            weekday: weekday.map(|value| if value == 7 { 0 } else { value }),
                        });
                    }
                }
            }
        }
    }

    if intervals.is_empty() {
        return Err("schedule produced no launchd calendar intervals".into());
    }
    if intervals.len() == 1 {
        Ok(CalendarInterval::One(intervals.remove(0)))
    } else {
        Ok(CalendarInterval::Many(intervals))
    }
}

fn concrete_values(field: &CronField) -> Vec<Option<u64>> {
    match field {
        CronField::Any => vec![None],
        CronField::Values(values) => values.iter().copied().map(Some).collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CronField {
    Any,
    Values(Vec<u64>),
}

fn expand_cron_field(
    field: &str,
    min: u64,
    max: u64,
    name: &str,
) -> Result<CronField, Box<dyn std::error::Error>> {
    if field == "*" {
        return Ok(CronField::Any);
    }

    let mut values = Vec::new();
    for part in field.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((range_part, step)) => (range_part, step.parse::<u64>()?),
            None => (part, 1),
        };
        if step == 0 {
            return Err(format!("{name}: step cannot be zero").into());
        }

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((start, end)) = range_part.split_once('-') {
            (start.parse::<u64>()?, end.parse::<u64>()?)
        } else {
            let value = range_part.parse::<u64>()?;
            (value, value)
        };

        if start < min || end > max || start > end {
            return Err(format!("{name}: value range {start}-{end} is outside {min}-{max}").into());
        }

        let mut value = start;
        while value <= end {
            values.push(value);
            value += step;
        }
    }

    values.sort_unstable();
    values.dedup();
    Ok(CronField::Values(values))
}

impl Hash for CronSchedule {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.minute.hash(state);
        self.hour.hash(state);
        self.day_of_month.hash(state);
        self.month.hash(state);
        self.day_of_week.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crontab_with_metadata() {
        let contents = "# macron:label=com.example.job\n# macron:start-interval=300\n15 3 * * 1 /usr/bin/true\n";
        let entries = parse_crontab(contents).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label.as_deref(), Some("com.example.job"));
        assert_eq!(entries[0].interval_seconds, Some(300));
        assert_eq!(entries[0].schedule.minute, "15");
        assert_eq!(entries[0].command, "/usr/bin/true");
    }

    #[test]
    fn formats_entries_file_with_header_and_metadata() {
        let entry = CronEntry {
            schedule: CronSchedule {
                minute: "0".to_string(),
                hour: "9".to_string(),
                day_of_month: "*".to_string(),
                month: "*".to_string(),
                day_of_week: "*".to_string(),
            },
            command: "/usr/bin/true".to_string(),
            label: Some("com.example.job".to_string()),
            source: Some(PathBuf::from("/tmp/com.example.job.plist")),
            interval_seconds: Some(300),
        };

        assert_eq!(
            format_entries_file(&[entry]),
            "# macron crontab\n\
             # minute hour day-of-month month day-of-week command\n\
             # macron:source=/tmp/com.example.job.plist\n\
             # macron:label=com.example.job\n\
             # macron:start-interval=300\n\
             0 9 * * * /usr/bin/true\n"
        );
    }

    #[test]
    fn render_store_plain_preserves_contents() {
        let contents = "# comment\n0 9 * * * /bin/bash -c 'echo hi'\n";
        assert_eq!(render_store(contents, false), contents);
    }

    #[test]
    fn render_store_colorizes_comments_and_cron_lines() {
        let contents = "# comment\n0 9 * * * /bin/bash -c 'echo hi'\n";
        let rendered = render_store(contents, true);
        assert!(rendered.contains("\u{1b}["));
        assert!(rendered.contains("# comment"));
        assert!(rendered.contains("/bin/bash"));
    }

    #[test]
    fn start_interval_entry_exports_as_start_interval() {
        let entry = CronEntry {
            schedule: CronSchedule {
                minute: "*/5".to_string(),
                hour: "*".to_string(),
                day_of_month: "*".to_string(),
                month: "*".to_string(),
                day_of_week: "*".to_string(),
            },
            command: "/usr/bin/true".to_string(),
            label: Some("com.example.interval".to_string()),
            source: None,
            interval_seconds: Some(300),
        };

        let job = entry_to_launchd_job(&entry, 0).unwrap();
        assert!(job.start_calendar_interval.is_none());
        assert_eq!(job.start_interval, Some(300));
    }

    #[test]
    fn expands_step_fields_for_launchd() {
        let schedule = CronSchedule {
            minute: "*/30".to_string(),
            hour: "9-10".to_string(),
            day_of_month: "*".to_string(),
            month: "*".to_string(),
            day_of_week: "1".to_string(),
        };
        let interval = schedule_to_calendar_interval(&schedule).unwrap();
        match interval {
            CalendarInterval::Many(fields) => assert_eq!(fields.len(), 4),
            CalendarInterval::One(_) => panic!("expected multiple fields"),
        }
    }

    #[test]
    fn launchd_calendar_dict_becomes_cron_schedule() {
        let mut dict = Dictionary::new();
        dict.insert("Minute".to_string(), Value::Integer(20.into()));
        dict.insert("Hour".to_string(), Value::Integer(7.into()));
        dict.insert("Weekday".to_string(), Value::Integer(2.into()));
        let schedule = calendar_dict_to_schedule(&dict);
        assert_eq!(schedule.minute, "20");
        assert_eq!(schedule.hour, "7");
        assert_eq!(schedule.day_of_month, "*");
        assert_eq!(schedule.day_of_week, "2");
    }

    #[test]
    fn launchd_start_interval_becomes_cron_step() {
        let schedule = start_interval_to_schedule(300).unwrap();
        assert_eq!(schedule.minute, "*/5");
        assert_eq!(schedule.hour, "*");
        assert_eq!(schedule.day_of_month, "*");
    }

    #[test]
    fn launchd_hourly_start_interval_becomes_hourly_cron() {
        let schedule = start_interval_to_schedule(3600).unwrap();
        assert_eq!(schedule.minute, "0");
        assert_eq!(schedule.hour, "*");
    }
}
