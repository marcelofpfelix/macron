use clap::{Parser, Subcommand, ValueEnum};
use owo_colors::{OwoColorize, Style};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use todo_txt::task::Simple;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Lint and convert between todo.txt, todo.md, and Taskwarrior JSON"
)]
struct Cli {
    /// When to colorize command output.
    #[arg(long = "color", value_enum, default_value_t = OutputColor::Auto, global = true)]
    color: OutputColor,

    /// Config file path.
    #[arg(short = 'c', long = "config", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
enum OutputColor {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Lint configured task files, or explicit paths when provided.
    Lint {
        /// Files or folders to lint. Uses config when omitted.
        paths: Vec<PathBuf>,
    },
    /// Sync configured todo.md files to todo.txt shadows and Taskwarrior.
    Sync {
        /// Show what would be synced without writing files or importing into Taskwarrior.
        #[arg(long)]
        dry_run: bool,
        /// Sync only a named profile from config.
        #[arg(long)]
        profile: Option<String>,
        /// Extra Taskwarrior filter argument before export. Repeat for multiple args.
        #[arg(long = "task-filter")]
        task_filter: Vec<String>,
        /// List configured sync profiles and exit.
        #[arg(long)]
        list_profiles: bool,
        /// List available conflict strategies and exit.
        #[arg(long, visible_alias = "list-resolution-strategies")]
        list_strategies: bool,
        /// Remove derived sync state and exit. Does not delete tasks or todo files.
        #[arg(long)]
        reset_state: bool,
        /// Show item-level sync plan and state changes without writing.
        #[arg(long)]
        plan: bool,
        /// Conflict strategy when the same tracked task changed on both sides.
        #[arg(long = "strategy", value_enum, default_value_t = ConflictStrategy::Fail)]
        strategy: ConflictStrategy,
        /// Overwrite existing todo.md files when Taskwarrior export differs.
        #[arg(long)]
        force: bool,
    },
    /// Lint todo.txt lines for the local extended format.
    LintTxt {
        /// todo.txt input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Lint todo.md for import safety and local style.
    LintMd {
        /// todo.md input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Convert todo.txt to todo.md.
    TxtToMd {
        /// todo.txt input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Convert todo.md to todo.txt.
    MdToTxt {
        /// todo.md input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Normalize todo.md task bullets into compact textwarrior one-line fields.
    NormalizeMd {
        /// todo.md input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Convert Taskwarrior `task export` JSON to todo.md.
    TaskJsonToMd {
        /// Taskwarrior JSON input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// Convert todo.md to Taskwarrior import JSON.
    MdToTaskJson {
        /// todo.md input file. Reads stdin when omitted.
        input: Option<PathBuf>,
    },
    /// List auto-import source files under a folder.
    ListSources {
        /// Folder to scan for active *-todo.md files.
        root: PathBuf,
    },
    /// Convert Beads JSONL backup files to compact todo.md.
    BeadsJsonlToMd {
        /// Beads issues.jsonl file.
        #[arg(long)]
        issues: PathBuf,
        /// Beads dependencies.jsonl file.
        #[arg(long)]
        dependencies: Option<PathBuf>,
        /// Beads labels.jsonl file.
        #[arg(long)]
        labels: Option<PathBuf>,
        /// Include closed Beads issues.
        #[arg(long)]
        include_closed: bool,
    },
    /// Convert Beads `bd --readonly list --json` output to compact todo.md.
    BeadsJsonToMd {
        /// Beads JSON input file. Reads stdin when omitted.
        input: Option<PathBuf>,
        /// Beads dependencies.jsonl file.
        #[arg(long)]
        dependencies: Option<PathBuf>,
        /// Beads labels.jsonl file.
        #[arg(long)]
        labels: Option<PathBuf>,
        /// Include closed Beads issues.
        #[arg(long)]
        include_closed: bool,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, ValueEnum)]
enum ConflictStrategy {
    Fail,
    Md,
    Taskwarrior,
}

impl ConflictStrategy {
    fn name(self) -> &'static str {
        match self {
            ConflictStrategy::Fail => "fail",
            ConflictStrategy::Md => "md",
            ConflictStrategy::Taskwarrior => "taskwarrior",
        }
    }

    fn description(self) -> &'static str {
        match self {
            ConflictStrategy::Fail => "stop on changed-on-both-sides tasks",
            ConflictStrategy::Md => "keep the local todo.md task",
            ConflictStrategy::Taskwarrior => "keep the Taskwarrior export",
        }
    }

    fn all() -> [ConflictStrategy; 3] {
        [
            ConflictStrategy::Fail,
            ConflictStrategy::Md,
            ConflictStrategy::Taskwarrior,
        ]
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TaskwarriorTask {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wait: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scheduled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recur: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    annotations: Vec<TaskwarriorAnnotation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TaskwarriorAnnotation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry: Option<String>,
    description: String,
}

#[derive(Debug, Deserialize)]
struct BeadIssue {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    issue_type: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    external_ref: Option<String>,
    #[serde(default)]
    assignee: Option<String>,
    #[serde(default)]
    due_at: Option<String>,
    #[serde(default)]
    defer_until: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BeadDependency {
    issue_id: String,
    depends_on_id: String,
    #[serde(default, rename = "type")]
    _type: String,
}

#[derive(Debug, Deserialize)]
struct BeadLabel {
    issue_id: String,
    label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MdTask {
    checked: CheckState,
    title: String,
    fields: BTreeMap<String, String>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TodoMdDocument {
    defaults: BTreeMap<String, String>,
    show_status: bool,
    compact_uuid: bool,
    id_prefix: Option<String>,
    details_path: Option<String>,
    body: String,
}

impl Default for TodoMdDocument {
    fn default() -> Self {
        Self {
            defaults: BTreeMap::new(),
            show_status: true,
            compact_uuid: false,
            id_prefix: None,
            details_path: None,
            body: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct AppConfig {
    #[serde(default)]
    todo_dir: Option<PathBuf>,
    #[serde(default = "default_task_command")]
    task_command: String,
    #[serde(default)]
    files: Vec<ConfiguredFile>,
    #[serde(default)]
    profiles: Vec<SyncProfile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ConfiguredFile {
    #[serde(default, rename = "name")]
    name: String,
    path: PathBuf,
    #[serde(default)]
    export_txt: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct SyncProfile {
    name: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    task_filter: Vec<String>,
}

#[derive(Debug, Clone)]
struct SyncDocument {
    name: String,
    path: PathBuf,
    export_txt: Option<PathBuf>,
    text: String,
}

#[derive(Debug, Clone)]
struct SourceProfile {
    path: PathBuf,
    source: Option<String>,
    id_prefix: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct SyncState {
    version: u32,
    #[serde(default)]
    tasks: BTreeMap<String, SyncStateTask>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SyncStateTask {
    path: String,
    hash: String,
}

#[derive(Debug, Clone)]
struct MdTaskLocation {
    path: String,
    hash: String,
}

struct SyncOptions<'a> {
    dry_run: bool,
    profile: Option<&'a str>,
    task_filter: &'a [String],
    list_profiles: bool,
    reset_state: bool,
    plan: bool,
    strategy: ConflictStrategy,
    force: bool,
}

struct MergeContext<'a> {
    previous: &'a SyncState,
    todo_dir: &'a Path,
    current_locations: &'a BTreeMap<String, MdTaskLocation>,
    exported_identities: &'a BTreeSet<String>,
    preserve_missing_tasks: bool,
    strategy: ConflictStrategy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SyncPlanCounts {
    new_count: usize,
    changed_count: usize,
    moved_count: usize,
    deleted_count: usize,
    unchanged_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LintSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LintFinding {
    severity: LintSeverity,
    line: Option<usize>,
    message: String,
}

impl LintFinding {
    fn error(line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            severity: LintSeverity::Error,
            line,
            message: message.into(),
        }
    }

    fn warning(line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            severity: LintSeverity::Warning,
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CheckState {
    #[default]
    Open,
    Done,
    Deleted,
    InProgress,
    Review,
    Blocked,
}

impl CheckState {
    fn as_markdown_marker(self) -> &'static str {
        match self {
            Self::Open => " ",
            Self::Done => "x",
            Self::Deleted => "-",
            Self::InProgress => "/",
            Self::Review => "?",
            Self::Blocked => "!",
        }
    }

    fn hidden_status(self) -> Option<&'static str> {
        match self {
            Self::InProgress => Some("in_progress"),
            Self::Review => Some("review"),
            Self::Blocked => Some("blocked"),
            _ => None,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("textwarrior: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    run_cli(Cli::parse())
}

fn run_cli(cli: Cli) -> Result<(), String> {
    let color = should_color(cli.color);

    match cli.command {
        Command::Lint { paths } => {
            let config = if paths.is_empty() {
                Some(read_config(cli.config.as_ref())?)
            } else {
                None
            };
            let files = lint_targets(config.as_ref(), &paths)?;
            let findings = lint_files(&files, color)?;
            let errors = findings
                .iter()
                .filter(|finding| finding.severity == LintSeverity::Error)
                .count();
            if errors == 0 {
                Ok(())
            } else {
                Err(format!("{errors} error(s), {} finding(s)", findings.len()))
            }
        }
        Command::Sync {
            dry_run,
            profile,
            task_filter,
            list_profiles,
            list_strategies,
            reset_state,
            plan,
            strategy,
            force,
        } => {
            if list_strategies {
                print_conflict_strategies();
                Ok(())
            } else {
                sync_config(
                    &read_config(cli.config.as_ref())?,
                    SyncOptions {
                        dry_run,
                        profile: profile.as_deref(),
                        task_filter: &task_filter,
                        list_profiles,
                        reset_state,
                        plan,
                        strategy,
                        force,
                    },
                )
            }
        }
        Command::LintTxt { input } => {
            let text = read_input(input)?;
            let findings = lint_todo_txt(&text);
            for finding in &findings {
                println!("{finding}");
            }
            if findings.is_empty() {
                Ok(())
            } else {
                Err(format!("{} finding(s)", findings.len()))
            }
        }
        Command::LintMd { input } => {
            let text = read_input(input)?;
            let findings = lint_todo_md(&text);
            for finding in &findings {
                println!("{}", format_lint_finding(finding, color));
            }
            let errors = findings
                .iter()
                .filter(|finding| finding.severity == LintSeverity::Error)
                .count();
            if errors == 0 {
                Ok(())
            } else {
                Err(format!("{errors} error(s), {} finding(s)", findings.len()))
            }
        }
        Command::TxtToMd { input } => {
            let text = read_input(input)?;
            print!("{}", todo_txt_to_md(&text));
            Ok(())
        }
        Command::MdToTxt { input } => {
            let text = read_input(input)?;
            print!("{}", md_to_todo_txt(&text)?);
            Ok(())
        }
        Command::NormalizeMd { input } => {
            let text = read_input(input)?;
            print!("{}", normalize_todo_md(&text));
            Ok(())
        }
        Command::TaskJsonToMd { input } => {
            let text = read_input(input)?;
            print!("{}", task_json_to_md(&text)?);
            Ok(())
        }
        Command::MdToTaskJson { input } => {
            let text = read_input(input)?;
            print!("{}", md_to_task_json(&text)?);
            Ok(())
        }
        Command::ListSources { root } => {
            for path in list_source_files(&root)? {
                println!("{}", path.display());
            }
            Ok(())
        }
        Command::BeadsJsonlToMd {
            issues,
            dependencies,
            labels,
            include_closed,
        } => {
            print!(
                "{}",
                beads_jsonl_to_md(
                    &issues,
                    dependencies.as_ref(),
                    labels.as_ref(),
                    include_closed
                )?
            );
            Ok(())
        }
        Command::BeadsJsonToMd {
            input,
            dependencies,
            labels,
            include_closed,
        } => {
            let text = read_input(input)?;
            print!(
                "{}",
                beads_json_to_md(
                    &text,
                    dependencies.as_ref(),
                    labels.as_ref(),
                    include_closed
                )?
            );
            Ok(())
        }
    }
}

fn read_input(path: Option<PathBuf>) -> Result<String, String> {
    match path {
        Some(path) => fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display())),
        None => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| format!("failed to read stdin: {error}"))?;
            Ok(text)
        }
    }
}

fn default_config_path() -> Result<PathBuf, String> {
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/textwarrior/config.toml"))
}

fn read_config(path: Option<&PathBuf>) -> Result<AppConfig, String> {
    let path = match path {
        Some(path) => expand_home(path),
        None => default_config_path()?,
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read config {}: {error}", path.display()))?;
    let mut config: AppConfig = toml::from_str(&text)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))?;

    if let Some(todo_dir) = &config.todo_dir {
        let todo_dir = expand_home(todo_dir);
        for file in &mut config.files {
            file.path = resolve_config_path(&todo_dir, &file.path);
            if let Some(export_txt) = &file.export_txt {
                file.export_txt = Some(resolve_config_path(&todo_dir, export_txt));
            }
        }
        config.todo_dir = Some(todo_dir);
    } else {
        for file in &mut config.files {
            file.path = expand_home(&file.path);
            if let Some(export_txt) = &file.export_txt {
                file.export_txt = Some(expand_home(export_txt));
            }
        }
    }

    Ok(config)
}

fn default_task_command() -> String {
    "task".to_string()
}

fn resolve_config_path(base: &Path, path: &Path) -> PathBuf {
    let path = expand_home(path);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn expand_home(path: &Path) -> PathBuf {
    let Some(value) = path.to_str() else {
        return path.to_path_buf();
    };
    if value == "~" {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf())
    } else if let Some(rest) = value.strip_prefix("~/") {
        env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn read_jsonl<T>(path: &PathBuf) -> Result<Vec<T>, String>
where
    T: for<'de> Deserialize<'de>,
{
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let mut values = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = serde_json::from_str(trimmed)
            .map_err(|error| format!("{}:{}: invalid JSONL: {error}", path.display(), index + 1))?;
        values.push(value);
    }

    Ok(values)
}

fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("textwarrior");
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&tmp_path, text)
        .map_err(|error| format!("failed to write {}: {error}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .map_err(|error| format!("failed to replace {}: {error}", path.display()))?;
    Ok(())
}

fn sync_state_path(todo_dir: &Path) -> PathBuf {
    todo_dir.join(".textwarrior/state.json")
}

fn reset_sync_state(todo_dir: &Path, dry_run: bool) -> Result<(), String> {
    let path = sync_state_path(todo_dir);
    if dry_run {
        println!("would remove {}", path.display());
        return Ok(());
    }
    match fs::remove_file(&path) {
        Ok(()) => {
            println!("removed {}", path.display());
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("sync state already absent: {}", path.display());
            Ok(())
        }
        Err(error) => Err(format!("failed to remove {}: {error}", path.display())),
    }
}

fn load_sync_state(todo_dir: &Path) -> Result<SyncState, String> {
    let path = sync_state_path(todo_dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(SyncState {
            version: 1,
            tasks: BTreeMap::new(),
        });
    };
    serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse sync state {}: {error}", path.display()))
}

fn save_sync_state(todo_dir: &Path, state: &SyncState) -> Result<(), String> {
    let path = sync_state_path(todo_dir);
    let json = serde_json::to_string_pretty(state)
        .map_err(|error| format!("failed to serialize sync state: {error}"))?;
    atomic_write(&path, &format!("{json}\n"))
}

fn sync_import_temp_path(todo_dir: &Path) -> PathBuf {
    todo_dir.join(format!(
        ".textwarrior/tmp/task-import-{}-{}.json",
        std::process::id(),
        unix_timestamp_nanos()
    ))
}

fn unix_timestamp_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn build_sync_state(documents: &[SyncDocument], todo_dir: &Path) -> Result<SyncState, String> {
    let mut tasks: BTreeMap<String, SyncStateTask> = BTreeMap::new();
    let mut uuids: BTreeMap<String, String> = BTreeMap::new();
    for document in documents {
        let path = document
            .path
            .strip_prefix(todo_dir)
            .unwrap_or(&document.path)
            .display()
            .to_string();
        for task in extract_todo_md_tasks(&document.text) {
            let Some(identity) = md_task_identity(&task) else {
                continue;
            };
            if let Some(existing) = tasks.get(&identity) {
                return Err(format!(
                    "duplicate task identity {identity} in {} and {}",
                    existing.path, path
                ));
            }
            if let Some(uuid_identity) = md_task_uuid_identity(&task) {
                if let Some(existing_path) = uuids.get(&uuid_identity) {
                    return Err(format!(
                        "duplicate task uuid {uuid_identity} in {} and {}",
                        existing_path, path
                    ));
                }
                uuids.insert(uuid_identity, path.clone());
            }
            tasks.insert(
                identity,
                SyncStateTask {
                    path: path.clone(),
                    hash: md_task_hash(&task)?,
                },
            );
        }
    }
    Ok(SyncState { version: 1, tasks })
}

fn merge_partial_sync_state(
    previous: &SyncState,
    selected: SyncState,
    documents: &[SyncDocument],
    todo_dir: &Path,
) -> SyncState {
    let selected_paths = documents
        .iter()
        .map(|document| sync_relative_path(&document.path, todo_dir))
        .collect::<BTreeSet<_>>();
    let mut tasks = previous
        .tasks
        .iter()
        .filter(|(_, task)| !selected_paths.contains(&task.path))
        .map(|(identity, task)| (identity.clone(), task.clone()))
        .collect::<BTreeMap<_, _>>();
    tasks.extend(selected.tasks);
    SyncState { version: 1, tasks }
}

fn md_task_identity(task: &MdTask) -> Option<String> {
    task.fields
        .get("id")
        .filter(|value| !value.is_empty())
        .map(|id| format!("id:{id}"))
        .or_else(|| {
            task.fields
                .get("uuid")
                .filter(|value| !value.is_empty())
                .map(|uuid| format!("uuid:{uuid}"))
        })
        .or_else(|| {
            task.fields
                .get("u")
                .and_then(|value| decode_uuid_compact(value))
                .map(|uuid| format!("uuid:{uuid}"))
        })
}

fn md_task_uuid_identity(task: &MdTask) -> Option<String> {
    task.fields
        .get("uuid")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            task.fields
                .get("u")
                .and_then(|value| decode_uuid_compact(value))
        })
        .map(|uuid| format!("uuid:{uuid}"))
}

fn taskwarrior_task_identity(task: &TaskwarriorTask) -> Option<String> {
    let (mut fields, _) = restore_textwarrior_fields(task.annotations.clone());
    insert_optional_field(&mut fields, "uuid", task.uuid.as_deref());
    let md_task = MdTask {
        checked: CheckState::Open,
        title: String::new(),
        fields,
        notes: Vec::new(),
    };
    md_task_identity(&md_task)
}

fn md_task_hash(task: &MdTask) -> Result<String, String> {
    let taskwarrior = taskwarrior_task_from_md(task.clone());
    serde_json::to_string(&taskwarrior)
        .map(|json| stable_hash(&json))
        .map_err(|error| format!("failed to serialize sync state task: {error}"))
}

fn stable_hash(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn print_sync_plan(current: &SyncState, next: &SyncState) {
    let counts = sync_plan_counts(current, next);

    println!(
        "sync plan: {} new, {} changed, {} moved, {} deleted, {} unchanged",
        counts.new_count,
        counts.changed_count,
        counts.moved_count,
        counts.deleted_count,
        counts.unchanged_count
    );
}

fn sync_plan_counts(current: &SyncState, next: &SyncState) -> SyncPlanCounts {
    let mut counts = SyncPlanCounts::default();
    for (identity, next_task) in &next.tasks {
        match current.tasks.get(identity) {
            None => counts.new_count += 1,
            Some(current_task) if current_task.hash != next_task.hash => counts.changed_count += 1,
            Some(current_task) if current_task.path != next_task.path => counts.moved_count += 1,
            Some(_) => counts.unchanged_count += 1,
        }
    }
    for identity in current.tasks.keys() {
        if !next.tasks.contains_key(identity) {
            counts.deleted_count += 1;
        }
    }
    counts
}

fn list_source_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_source_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn lint_targets(config: Option<&AppConfig>, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    if !paths.is_empty() {
        return expand_lint_paths(paths);
    }

    let Some(config) = config else {
        return Ok(Vec::new());
    };
    if !config.files.is_empty() {
        return Ok(config.files.iter().map(|file| file.path.clone()).collect());
    }
    if let Some(todo_dir) = &config.todo_dir {
        return list_lint_files(todo_dir);
    }

    Ok(Vec::new())
}

fn expand_lint_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for path in paths {
        let path = expand_home(path);
        if fs::metadata(&path)
            .map_err(|error| format!("failed to read metadata for {}: {error}", path.display()))?
            .is_dir()
        {
            files.extend(list_lint_files(&path)?);
        } else if is_lint_file(&path) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn list_lint_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_lint_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_lint_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for {}: {error}", path.display()))?;

    if metadata.is_file() {
        if is_lint_file(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }

    if !metadata.is_dir() {
        return Ok(());
    }

    for entry in
        fs::read_dir(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        let child = entry.path();
        if should_skip_source_dir(&child) {
            continue;
        }
        collect_lint_files(&child, files)?;
    }

    Ok(())
}

fn is_lint_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(name, "todo.txt" | "done.txt") || name.ends_with(".txt") || name.ends_with(".md")
}

fn lint_files(files: &[PathBuf], color: bool) -> Result<Vec<LintFinding>, String> {
    let mut all_findings = Vec::new();

    for file in files {
        let text = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let findings = if file.extension().and_then(|ext| ext.to_str()) == Some("md") {
            lint_todo_md(&text)
        } else {
            lint_todo_txt(&text)
                .into_iter()
                .map(|finding| LintFinding::error(None, finding))
                .collect()
        };
        for finding in &findings {
            println!(
                "{}: {}",
                file.display(),
                format_lint_finding(finding, color)
            );
        }
        all_findings.extend(findings);
    }

    Ok(all_findings)
}

fn sync_config(config: &AppConfig, options: SyncOptions<'_>) -> Result<(), String> {
    if options.list_profiles {
        print_sync_profiles(config);
        return Ok(());
    }
    let todo_dir = config
        .todo_dir
        .as_deref()
        .ok_or("config must set todo_dir for sync")?;
    if options.reset_state {
        return reset_sync_state(todo_dir, options.dry_run || options.plan);
    }
    let preview_only = options.dry_run || options.plan;
    let current_state = load_sync_state(todo_dir)?;
    let exported =
        export_taskwarrior_to_todo_md(config, todo_dir, &options, preview_only, &current_state)?;
    let documents = sync_documents(config, todo_dir, options.profile, &exported, preview_only)?;
    let mut tasks = Vec::new();
    let mut detected_tasks = 0usize;

    for document in &documents {
        let findings = lint_todo_md(&document.text);
        let errors = findings
            .iter()
            .filter(|finding| finding.severity == LintSeverity::Error)
            .count();
        if errors > 0 {
            return Err(format!(
                "{} has {errors} lint error(s); run textwarrior lint first",
                document.path.display()
            ));
        }

        let export_txt = document
            .export_txt
            .clone()
            .unwrap_or_else(|| document.path.with_extension("txt"));
        if is_auto_source_file(&document.path) {
            let txt = md_to_todo_txt(&document.text)?;
            if preview_only {
                println!("would write {}", export_txt.display());
            } else {
                if let Some(parent) = export_txt.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!("failed to create {}: {error}", parent.display())
                    })?;
                }
                atomic_write(&export_txt, &txt)?;
                println!("wrote {}", export_txt.display());
            }
        }

        let json = md_to_task_json(&document.text)?;
        let mut file_tasks: Vec<TaskwarriorTask> = serde_json::from_str(&json)
            .map_err(|error| format!("invalid generated JSON: {error}"))?;
        let file_task_count = file_tasks.len();
        detected_tasks += file_task_count;
        tasks.append(&mut file_tasks);
        println!(
            "detected {file_task_count} task(s) in {} ({})",
            document.name,
            document.path.display()
        );
    }

    println!(
        "sync summary: {} file(s), {detected_tasks} task(s) detected",
        documents.len()
    );

    let selected_state = build_sync_state(&documents, todo_dir)?;
    let next_state = if options.profile.is_some() {
        merge_partial_sync_state(&current_state, selected_state, &documents, todo_dir)
    } else {
        selected_state
    };
    print_sync_plan(&current_state, &next_state);
    if options.plan {
        println!("sync summary: plan only; no files written and no Taskwarrior import");
        return Ok(());
    }

    if tasks.is_empty() {
        println!("sync summary: 0 task(s) to import");
        if !options.dry_run {
            save_sync_state(todo_dir, &next_state)?;
        }
        return Ok(());
    }

    let import_json = serde_json::to_string_pretty(&tasks)
        .map_err(|error| format!("failed to serialize Taskwarrior JSON: {error}"))?;
    let import_path = sync_import_temp_path(todo_dir);
    if options.dry_run {
        println!(
            "would import {} task(s) with {}",
            tasks.len(),
            config.task_command
        );
        return Ok(());
    }
    atomic_write(&import_path, &format!("{import_json}\n"))?;
    let status_result = std::process::Command::new(&config.task_command)
        .arg("import")
        .arg(&import_path)
        .status()
        .map_err(|error| format!("failed to run {} import: {error}", config.task_command));
    let _ = fs::remove_file(&import_path);
    let status = status_result?;
    if !status.success() {
        return Err(format!(
            "{} import failed with {status}",
            config.task_command
        ));
    }
    save_sync_state(todo_dir, &next_state)?;
    println!(
        "sync summary: imported {} task(s) with {}",
        tasks.len(),
        config.task_command
    );

    Ok(())
}

fn export_taskwarrior_to_todo_md(
    config: &AppConfig,
    todo_dir: &Path,
    options: &SyncOptions<'_>,
    dry_run: bool,
    current_state: &SyncState,
) -> Result<Vec<SyncDocument>, String> {
    let mut command = std::process::Command::new(&config.task_command);
    for arg in task_export_filter_args(config, options.profile, options.task_filter)? {
        command.arg(arg);
    }
    let output = command
        .arg("export")
        .output()
        .map_err(|error| format!("failed to run {} export: {error}", config.task_command))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{} export failed with {}: {}",
            config.task_command,
            output.status,
            stderr.trim()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|error| format!("{} export was not UTF-8: {error}", config.task_command))?;
    let tasks: Vec<TaskwarriorTask> = serde_json::from_str(&text)
        .map_err(|error| format!("invalid Taskwarrior export: {error}"))?;
    let task_count = tasks.len();
    let mut groups: BTreeMap<PathBuf, Vec<TaskwarriorTask>> = BTreeMap::new();
    let source_profiles = source_profiles(todo_dir)?;
    let source_paths = configured_source_paths(config, todo_dir, options.profile)?;
    let selected_paths = selected_source_paths(config, options.profile);
    let current_task_locations = md_task_locations_from_paths(&source_paths, todo_dir)?;
    let preserve_missing_tasks = has_task_filter(config, options.profile, options.task_filter)?;
    let mut filtered_export_identities = BTreeSet::new();

    for task in tasks {
        let annotation_source = textwarrior_annotation_value(&task.annotations, "source");
        let source = if annotation_source.as_deref() == Some("taskwarrior") {
            infer_source_from_profiles(&task, &source_profiles).or(annotation_source)
        } else {
            annotation_source.or_else(|| infer_source_from_profiles(&task, &source_profiles))
        };
        let active_path = source
            .as_deref()
            .map(|source| taskwarrior_source_path(todo_dir, source))
            .unwrap_or_else(|| todo_dir.join("taskwarrior-todo.md"));
        let path = if task.status.as_deref() == Some("completed") {
            done_source_path(&active_path)
        } else {
            active_path
        };
        if let Some(identity) = taskwarrior_task_identity(&task) {
            filtered_export_identities.insert(identity);
        }
        if selected_paths
            .as_ref()
            .is_some_and(|paths| !paths.contains(&path))
        {
            continue;
        }
        groups.entry(path).or_default().push(task);
    }
    add_stale_tracked_paths(&mut groups, &source_paths, current_state, todo_dir);

    println!(
        "taskwarrior export: {task_count} task(s), {} target file(s)",
        groups.len()
    );

    let mut rendered_groups = BTreeMap::new();
    for (path, tasks) in groups {
        let json = serde_json::to_string_pretty(&tasks)
            .map_err(|error| format!("failed to serialize Taskwarrior group: {error}"))?;
        let existing_document = document_template_for_path(&path);
        let exported_rendered = task_json_to_md_with_document(&json, &existing_document)?;
        rendered_groups.insert(path, (tasks.len(), exported_rendered));
    }

    let mut documents = Vec::new();
    for (path, (task_count, exported_rendered)) in rendered_groups {
        let merge_context = MergeContext {
            previous: current_state,
            todo_dir,
            current_locations: &current_task_locations,
            exported_identities: &filtered_export_identities,
            preserve_missing_tasks,
            strategy: options.strategy,
        };
        let rendered =
            merge_local_file_changes(&path, &exported_rendered, &merge_context, options.force)?;
        if current_state.tasks.is_empty() {
            detect_write_conflict(&path, &rendered, options.force)?;
        }

        if dry_run {
            println!("would write {} task(s) to {}", task_count, path.display());
        } else {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            atomic_write(&path, &rendered)?;
            println!("wrote {} task(s) to {}", task_count, path.display());
        }

        let export_txt = configured_export_txt(config, &path);
        documents.push(SyncDocument {
            name: sync_document_name(&path),
            path,
            export_txt,
            text: rendered,
        });
    }

    Ok(documents)
}

fn add_stale_tracked_paths(
    groups: &mut BTreeMap<PathBuf, Vec<TaskwarriorTask>>,
    source_paths: &[PathBuf],
    current_state: &SyncState,
    todo_dir: &Path,
) {
    if current_state.tasks.is_empty() {
        return;
    }
    let grouped_paths = groups.keys().cloned().collect::<BTreeSet<_>>();
    for path in source_paths {
        if grouped_paths.contains(path) {
            continue;
        }
        let path_key = sync_relative_path(path, todo_dir);
        if current_state
            .tasks
            .values()
            .any(|task| task.path == path_key)
        {
            groups.entry(path.clone()).or_default();
        }
    }
}

fn sync_documents(
    config: &AppConfig,
    todo_dir: &Path,
    profile: Option<&str>,
    exported: &[SyncDocument],
    dry_run: bool,
) -> Result<Vec<SyncDocument>, String> {
    if dry_run {
        let mut documents = Vec::new();
        let exported_paths = exported
            .iter()
            .map(|document| document.path.clone())
            .collect::<Vec<_>>();
        for path in configured_source_paths(config, todo_dir, profile)? {
            if exported_paths.contains(&path) {
                continue;
            }
            documents.push(read_sync_document(config, &path)?);
        }
        documents.extend(exported.iter().cloned());
        documents.sort_by(|left, right| left.path.cmp(&right.path));
        return Ok(documents);
    }

    configured_source_paths(config, todo_dir, profile)?
        .into_iter()
        .map(|path| read_sync_document(config, &path))
        .collect()
}

fn configured_source_paths(
    config: &AppConfig,
    todo_dir: &Path,
    profile: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let files = configured_files_for_profile(config, profile)?;
    if files.is_empty() {
        return list_source_files(todo_dir);
    }
    Ok(expand_with_done_source_paths(
        files.into_iter().map(|file| file.path).collect(),
        false,
    ))
}

fn configured_files_for_profile(
    config: &AppConfig,
    profile: Option<&str>,
) -> Result<Vec<ConfiguredFile>, String> {
    let Some(profile_name) = profile else {
        return Ok(config.files.clone());
    };
    let profile = sync_profile(config, profile_name)?;
    if profile.files.is_empty() {
        return Err(format!("sync profile {profile_name} has no files"));
    }

    let mut files = Vec::new();
    for file_name in &profile.files {
        let file = config
            .files
            .iter()
            .find(|item| item.name == *file_name)
            .ok_or_else(|| {
                format!("sync profile {profile_name} references unknown file {file_name}")
            })?;
        files.push(file.clone());
    }
    Ok(files)
}

fn expand_with_done_source_paths(paths: Vec<PathBuf>, include_missing_done: bool) -> Vec<PathBuf> {
    let mut expanded = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        for candidate in [path.clone(), done_source_path(&path)] {
            if candidate == path || include_missing_done || candidate.exists() {
                if seen.insert(candidate.clone()) {
                    expanded.push(candidate);
                }
            }
        }
    }
    expanded
}

fn sync_profile<'a>(config: &'a AppConfig, profile: &str) -> Result<&'a SyncProfile, String> {
    config
        .profiles
        .iter()
        .find(|item| item.name == profile)
        .ok_or_else(|| format!("unknown sync profile {profile}"))
}

fn task_export_filter_args(
    config: &AppConfig,
    profile: Option<&str>,
    task_filter: &[String],
) -> Result<Vec<String>, String> {
    let mut args = if let Some(profile_name) = profile {
        sync_profile(config, profile_name)?.task_filter.clone()
    } else {
        Vec::new()
    };
    args.extend(task_filter.iter().cloned());
    Ok(args)
}

fn has_task_filter(
    config: &AppConfig,
    profile: Option<&str>,
    task_filter: &[String],
) -> Result<bool, String> {
    Ok(!task_export_filter_args(config, profile, task_filter)?.is_empty())
}

fn print_sync_profiles(config: &AppConfig) {
    if config.profiles.is_empty() {
        println!("no sync profiles configured");
        return;
    }
    for profile in &config.profiles {
        let files = if profile.files.is_empty() {
            "-".to_string()
        } else {
            profile.files.join(",")
        };
        let filter = if profile.task_filter.is_empty() {
            "-".to_string()
        } else {
            profile.task_filter.join(" ")
        };
        println!("{} files={} task_filter={}", profile.name, files, filter);
    }
}

fn print_conflict_strategies() {
    for strategy in ConflictStrategy::all() {
        println!("{} - {}", strategy.name(), strategy.description());
    }
}

fn selected_source_paths(config: &AppConfig, profile: Option<&str>) -> Option<Vec<PathBuf>> {
    profile
        .and_then(|_| configured_files_for_profile(config, profile).ok())
        .map(|files| {
            expand_with_done_source_paths(files.into_iter().map(|file| file.path).collect(), true)
        })
}

fn read_sync_document(config: &AppConfig, path: &Path) -> Result<SyncDocument, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok(SyncDocument {
        name: sync_document_name(path),
        path: path.to_path_buf(),
        export_txt: configured_export_txt(config, path),
        text,
    })
}

fn merge_local_file_changes(
    path: &Path,
    rendered: &str,
    context: &MergeContext<'_>,
    force: bool,
) -> Result<String, String> {
    if force || context.previous.tasks.is_empty() {
        return Ok(rendered.to_string());
    }
    let Ok(current) = fs::read_to_string(path) else {
        return Ok(rendered.to_string());
    };
    let (merged, conflicts) = merge_todo_md_text(path, &current, rendered, context)?;
    if conflicts.is_empty() {
        return Ok(merged);
    }
    Err(format!(
        "conflict: {} has {} task(s) changed in both todo.md and Taskwarrior since last sync: {}; rerun with --force to overwrite",
        path.display(),
        conflicts.len(),
        conflicts.join(", ")
    ))
}

fn merge_todo_md_text(
    path: &Path,
    current: &str,
    rendered: &str,
    context: &MergeContext<'_>,
) -> Result<(String, Vec<String>), String> {
    let document = split_todo_md_document(rendered);
    let current_tasks = extract_todo_md_tasks(current);
    let rendered_tasks = extract_todo_md_tasks(rendered);
    let current_by_identity = md_task_map(current_tasks)?;
    let rendered_by_identity = md_task_map(rendered_tasks.clone())?;
    let path_key = sync_relative_path(path, context.todo_dir);
    let mut conflicts = Vec::new();
    let mut merged_tasks = Vec::new();

    for rendered_task in rendered_tasks {
        let Some(identity) = md_task_identity(&rendered_task) else {
            merged_tasks.push(rendered_task);
            continue;
        };
        let Some(previous_task) = context.previous.tasks.get(&identity) else {
            merged_tasks.push(rendered_task);
            continue;
        };
        if previous_task.path != path_key {
            merged_tasks.push(rendered_task);
            continue;
        }
        let rendered_hash = md_task_hash(&rendered_task)?;
        if let Some(current_location) = context.current_locations.get(&identity)
            && current_location.path != path_key
        {
            let current_changed = current_location.path != previous_task.path
                || current_location.hash != previous_task.hash;
            let rendered_changed = rendered_hash != previous_task.hash;
            if current_changed && rendered_changed && current_location.hash != rendered_hash {
                match context.strategy {
                    ConflictStrategy::Fail => {
                        conflicts.push(identity);
                        merged_tasks.push(rendered_task);
                    }
                    ConflictStrategy::Md => {}
                    ConflictStrategy::Taskwarrior => merged_tasks.push(rendered_task),
                }
            } else if rendered_changed {
                merged_tasks.push(rendered_task);
            }
            continue;
        }
        let Some((current_task, current_hash)) = current_by_identity.get(&identity) else {
            if rendered_hash != previous_task.hash {
                match context.strategy {
                    ConflictStrategy::Fail => conflicts.push(identity),
                    ConflictStrategy::Md => continue,
                    ConflictStrategy::Taskwarrior => {}
                }
            }
            merged_tasks.push(rendered_task);
            continue;
        };
        let current_changed = current_hash != &previous_task.hash;
        let rendered_changed = rendered_hash != previous_task.hash;
        if current_changed && rendered_changed && current_hash != &rendered_hash {
            match context.strategy {
                ConflictStrategy::Fail => {
                    conflicts.push(identity);
                    merged_tasks.push(rendered_task);
                }
                ConflictStrategy::Md => merged_tasks.push(current_task.clone()),
                ConflictStrategy::Taskwarrior => merged_tasks.push(rendered_task),
            }
        } else if current_changed && !rendered_changed {
            merged_tasks.push(current_task.clone());
        } else {
            merged_tasks.push(rendered_task);
        }
    }

    for (identity, (current_task, _)) in current_by_identity {
        if rendered_by_identity.contains_key(&identity) {
            continue;
        }
        if let Some(previous_task) = context.previous.tasks.get(&identity) {
            let current_hash = md_task_hash(&current_task)?;
            if context.preserve_missing_tasks && !context.exported_identities.contains(&identity) {
                merged_tasks.push(current_task);
                continue;
            }
            if current_hash != previous_task.hash {
                match context.strategy {
                    ConflictStrategy::Fail => {
                        conflicts.push(identity);
                        merged_tasks.push(current_task);
                    }
                    ConflictStrategy::Md => merged_tasks.push(current_task),
                    ConflictStrategy::Taskwarrior => {}
                }
            }
            continue;
        }
        merged_tasks.push(current_task);
    }

    Ok((render_todo_md_document(&document, &merged_tasks), conflicts))
}

fn md_task_map(tasks: Vec<MdTask>) -> Result<BTreeMap<String, (MdTask, String)>, String> {
    let mut map = BTreeMap::new();
    let mut uuids = BTreeSet::new();
    for task in tasks {
        let Some(identity) = md_task_identity(&task) else {
            continue;
        };
        let hash = md_task_hash(&task)?;
        if map.contains_key(&identity) {
            return Err(format!("duplicate task identity {identity}"));
        }
        if let Some(uuid_identity) = md_task_uuid_identity(&task)
            && !uuids.insert(uuid_identity.clone())
        {
            return Err(format!("duplicate task uuid {uuid_identity}"));
        }
        map.insert(identity, (task, hash));
    }
    Ok(map)
}

fn md_task_locations_from_paths(
    paths: &[PathBuf],
    todo_dir: &Path,
) -> Result<BTreeMap<String, MdTaskLocation>, String> {
    let mut locations: BTreeMap<String, MdTaskLocation> = BTreeMap::new();
    let mut uuids: BTreeMap<String, String> = BTreeMap::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let path_key = sync_relative_path(path, todo_dir);
        for task in extract_todo_md_tasks(&text) {
            let Some(identity) = md_task_identity(&task) else {
                continue;
            };
            let hash = md_task_hash(&task)?;
            if let Some(existing) = locations.get(&identity) {
                return Err(format!(
                    "duplicate task identity {identity} in {} and {}",
                    existing.path, path_key
                ));
            }
            if let Some(uuid_identity) = md_task_uuid_identity(&task) {
                if let Some(existing_path) = uuids.get(&uuid_identity) {
                    return Err(format!(
                        "duplicate task uuid {uuid_identity} in {} and {}",
                        existing_path, path_key
                    ));
                }
                uuids.insert(uuid_identity, path_key.clone());
            }
            locations.insert(
                identity,
                MdTaskLocation {
                    path: path_key.clone(),
                    hash,
                },
            );
        }
    }
    Ok(locations)
}

fn sync_relative_path(path: &Path, todo_dir: &Path) -> String {
    path.strip_prefix(todo_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn configured_export_txt(config: &AppConfig, path: &Path) -> Option<PathBuf> {
    config
        .files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| file.export_txt.clone())
}

fn sync_document_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("todo")
        .to_string()
}

fn detect_write_conflict(path: &Path, rendered: &str, force: bool) -> Result<(), String> {
    let Ok(current) = fs::read_to_string(path) else {
        return Ok(());
    };
    if current.trim().is_empty()
        || current.trim_end() == rendered.trim_end()
        || normalize_todo_md(&current).trim_end() == normalize_todo_md(rendered).trim_end()
        || force
    {
        return Ok(());
    }
    Err(format!(
        "conflict: {} already has different content; rerun with --force to overwrite",
        path.display()
    ))
}

fn taskwarrior_source_path(todo_dir: &Path, source: &str) -> PathBuf {
    let name = if let Some(prefix) = source.strip_suffix(".todo.md") {
        format!("{prefix}-todo.md")
    } else if let Some(prefix) = source.strip_suffix(".todo") {
        format!("{prefix}-todo.md")
    } else if source.ends_with("-todo.md") {
        source.to_string()
    } else {
        format!("{}-todo.md", sanitize_filename(source))
    };
    todo_dir.join(name)
}

fn done_source_path(path: &Path) -> PathBuf {
    if is_done_source_file(path) {
        return path.to_path_buf();
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.to_path_buf();
    };
    path.with_file_name(format!("done-{name}"))
}

fn active_source_path(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return path.to_path_buf();
    };
    if let Some(active_name) = name.strip_prefix("done-") {
        path.with_file_name(active_name)
    } else {
        path.to_path_buf()
    }
}

fn document_template_for_path(path: &Path) -> TodoMdDocument {
    fs::read_to_string(path)
        .ok()
        .or_else(|| {
            if is_done_source_file(path) {
                fs::read_to_string(active_source_path(path)).ok()
            } else {
                None
            }
        })
        .map(|text| split_todo_md_document(&text))
        .unwrap_or_default()
}

fn sanitize_filename(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    sanitized.trim_matches('-').to_string()
}

fn textwarrior_annotation_value(
    annotations: &[TaskwarriorAnnotation],
    key: &str,
) -> Option<String> {
    let needle = format!("textwarrior {key}:");
    annotations
        .iter()
        .find_map(|annotation| annotation.description.strip_prefix(&needle))
        .map(ToString::to_string)
}

fn source_profiles(todo_dir: &Path) -> Result<Vec<SourceProfile>, String> {
    let mut profiles = Vec::new();
    for path in list_source_files(todo_dir)? {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let document = split_todo_md_document(&text);
        profiles.push(SourceProfile {
            path,
            source: document.defaults.get("source").cloned(),
            id_prefix: document.id_prefix,
        });
    }
    Ok(profiles)
}

fn infer_source_from_profiles(
    task: &TaskwarriorTask,
    profiles: &[SourceProfile],
) -> Option<String> {
    let id = textwarrior_annotation_value(&task.annotations, "id")?;
    profiles
        .iter()
        .find(|profile| {
            profile
                .id_prefix
                .as_deref()
                .is_some_and(|prefix| id.starts_with(&format!("{prefix}-")))
        })
        .and_then(|profile| {
            profile.source.clone().or_else(|| {
                profile
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(ToString::to_string)
            })
        })
}

fn collect_source_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to read metadata for {}: {error}", path.display()))?;

    if metadata.is_file() {
        if is_auto_source_file(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }

    if !metadata.is_dir() {
        return Ok(());
    }

    for entry in
        fs::read_dir(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?
    {
        let entry = entry
            .map_err(|error| format!("failed to read entry in {}: {error}", path.display()))?;
        let child = entry.path();
        if should_skip_source_dir(&child) {
            continue;
        }
        collect_source_files(&child, files)?;
    }

    Ok(())
}

fn should_skip_source_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(name, ".git" | ".obsidian" | "archive")
}

fn is_auto_source_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    name.ends_with("-todo.md") && !name.ends_with("-done-todo.md") && name != "inbox.md"
}

fn is_done_source_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("done-") && name.ends_with("-todo.md"))
}

fn beads_jsonl_to_md(
    issues_path: &PathBuf,
    dependencies_path: Option<&PathBuf>,
    labels_path: Option<&PathBuf>,
    include_closed: bool,
) -> Result<String, String> {
    let issues: Vec<BeadIssue> = read_jsonl(issues_path)?;
    beads_to_md(issues, dependencies_path, labels_path, include_closed)
}

fn beads_json_to_md(
    text: &str,
    dependencies_path: Option<&PathBuf>,
    labels_path: Option<&PathBuf>,
    include_closed: bool,
) -> Result<String, String> {
    let issues: Vec<BeadIssue> =
        serde_json::from_str(text).map_err(|error| format!("invalid Beads JSON array: {error}"))?;
    beads_to_md(issues, dependencies_path, labels_path, include_closed)
}

fn beads_to_md(
    mut issues: Vec<BeadIssue>,
    dependencies_path: Option<&PathBuf>,
    labels_path: Option<&PathBuf>,
    include_closed: bool,
) -> Result<String, String> {
    issues.sort_by(|left, right| {
        bead_priority_rank(left.priority)
            .cmp(&bead_priority_rank(right.priority))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut dependencies: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(path) = dependencies_path {
        for dependency in read_jsonl::<BeadDependency>(path)? {
            dependencies
                .entry(dependency.issue_id)
                .or_default()
                .push(dependency.depends_on_id);
        }
    }

    let mut labels: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(path) = labels_path {
        for label in read_jsonl::<BeadLabel>(path)? {
            labels.entry(label.issue_id).or_default().push(label.label);
        }
    }

    let mut output = String::new();
    for issue in issues {
        if !include_closed && issue.status == "closed" {
            continue;
        }

        write_bead_issue_md(
            &mut output,
            &issue,
            labels.get(&issue.id).map(Vec::as_slice).unwrap_or(&[]),
            dependencies
                .get(&issue.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
    }

    Ok(output)
}

fn write_bead_issue_md(
    output: &mut String,
    issue: &BeadIssue,
    labels: &[String],
    dependencies: &[String],
) {
    let check = if issue.status == "closed" { "x" } else { " " };
    output.push_str(&format!(
        "- [{check}] {}",
        normalize_inline_title(&issue.title)
    ));

    output.push_str(&format!(" +{}", sanitize_tag(&issue.issue_type)));
    for label in &issue.labels {
        output.push_str(&format!(" +{}", sanitize_tag(label)));
    }
    for label in labels {
        output.push_str(&format!(" +{}", sanitize_tag(label)));
    }

    output.push_str(&format!(" id:{} source:beads", issue.id));
    if !issue.status.is_empty() {
        output.push_str(&format!(" status:{}", sanitize_value(&issue.status)));
    }
    if let Some(priority) = issue.priority {
        output.push_str(&format!(" priority:{}", bead_priority_label(priority)));
    }
    if let Some(external_ref) = issue
        .external_ref
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        output.push_str(&format!(" ext:{}", sanitize_value(external_ref)));
    }
    if let Some(assignee) = issue.assignee.as_deref().filter(|value| !value.is_empty()) {
        output.push_str(&format!(" assignee:{}", sanitize_value(assignee)));
    }
    if let Some(due_at) = issue.due_at.as_deref().filter(|value| !value.is_empty()) {
        output.push_str(&format!(" due:{}", normalize_dateish(due_at)));
    }
    if let Some(defer_until) = issue
        .defer_until
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        output.push_str(&format!(" wait:{}", normalize_dateish(defer_until)));
    }
    if !dependencies.is_empty() {
        output.push_str(&format!(" depends:{}", dependencies.join(",")));
    }
    output.push('\n');

    if !issue.description.trim().is_empty() {
        output.push_str(&format!(
            "\n  {}\n",
            indent_note(&normalize_beads_note(&issue.description))
        ));
    }
    if !issue.notes.trim().is_empty() {
        output.push_str(&format!(
            "\n  Notes:\n  {}\n",
            indent_note(&normalize_beads_note(&issue.notes))
        ));
    }
    output.push('\n');
}

fn bead_priority_rank(priority: Option<u8>) -> u8 {
    priority.unwrap_or(2)
}

fn bead_priority_label(priority: u8) -> &'static str {
    match priority {
        0 | 1 => "H",
        2 => "M",
        _ => "L",
    }
}

fn normalize_inline_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_tag(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.') {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    sanitized.trim_matches('-').to_string()
}

fn sanitize_value(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect()
}

fn normalize_beads_note(value: &str) -> String {
    value.replace("\\n", "\n")
}

fn indent_note(value: &str) -> String {
    value
        .trim()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n  ")
}

fn lint_todo_txt(text: &str) -> Vec<String> {
    let mut findings = Vec::new();

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let task: Simple = trimmed.to_string().into();
        if task.subject.trim().is_empty() {
            findings.push(format!("{number}: empty task subject"));
        }

        if task.finished && task.finish_date.is_none() {
            findings.push(format!(
                "{number}: completed task should include completion date"
            ));
        }

        if !has_identity(&task) {
            findings.push(format!("{number}: missing stable id: or uuid: metadata"));
        }

        for word in trimmed.split_whitespace() {
            if word.strip_suffix(':').is_some_and(is_metadata_key) {
                findings.push(format!("{number}: empty metadata value in `{word}`"));
            }
            if let Some((key, value)) = word.split_once(':')
                && matches!(key, "due" | "t")
                && !looks_like_iso_date(value)
            {
                findings.push(format!("{number}: {key}: should be YYYY-MM-DD"));
            }
        }
    }

    findings
}

fn has_identity(task: &Simple) -> bool {
    task.tags.contains_key("id") || task.tags.contains_key("uuid")
}

fn lint_todo_md(text: &str) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let tasks = match parse_todo_md(text) {
        Ok(tasks) => tasks,
        Err(error) => {
            findings.push(LintFinding::error(None, error));
            return findings;
        }
    };
    let task_lines = task_heading_line_numbers(text);

    for (index, task) in tasks.iter().enumerate() {
        let line = task_lines.get(index).copied();
        if !task.fields.contains_key("id")
            && !task.fields.contains_key("uuid")
            && !task.fields.contains_key("u")
        {
            findings.push(LintFinding::error(
                line,
                "missing stable id:, uuid:, or u: metadata",
            ));
        }

        if let Some(value) = task.fields.get("u").filter(|value| !value.is_empty())
            && decode_uuid_compact(value).is_none()
        {
            findings.push(LintFinding::error(
                line,
                "u: should be a compact encoded UUID",
            ));
        }

        for key in ["due", "wait", "t", "scheduled", "entry", "end"] {
            if let Some(value) = task.fields.get(key).filter(|value| !value.is_empty())
                && !looks_like_iso_date(value)
            {
                findings.push(LintFinding::error(
                    line,
                    format!("{key}: should be YYYY-MM-DD"),
                ));
            }
        }

        if task.checked == CheckState::Done && !task.fields.contains_key("end") {
            findings.push(LintFinding::warning(
                line,
                "completed task should include a completion date",
            ));
        }

        if task.checked == CheckState::Open
            && matches!(
                task.fields.get("status").map(String::as_str),
                Some("done" | "completed")
            )
        {
            findings.push(LintFinding::warning(
                line,
                "open checkbox with done/completed status; normalize the checkbox or status",
            ));
        }
    }

    if normalize_todo_md(text).trim_end() != text.trim_end() {
        findings.push(LintFinding::warning(
            None,
            "format differs from normalize-md output",
        ));
    }

    findings
}

fn task_heading_line_numbers(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| parse_task_heading(line).map(|_| index + 1))
        .collect()
}

fn should_color(color: OutputColor) -> bool {
    match color {
        OutputColor::Always => true,
        OutputColor::Never => false,
        OutputColor::Auto => io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
    }
}

fn format_lint_finding(finding: &LintFinding, color: bool) -> String {
    let severity = match finding.severity {
        LintSeverity::Error => "error",
        LintSeverity::Warning => "warning",
    };
    let severity = if color {
        match finding.severity {
            LintSeverity::Error => severity.style(error_style()).to_string(),
            LintSeverity::Warning => severity.style(warning_style()).to_string(),
        }
    } else {
        severity.to_string()
    };
    let message = if color {
        finding.message.style(message_style()).to_string()
    } else {
        finding.message.clone()
    };

    match finding.line {
        Some(line) if color => format!(
            "{}: {severity}: {message}",
            line.to_string().style(line_style())
        ),
        Some(line) => format!("{line}: {severity}: {message}"),
        None => format!("{severity}: {message}"),
    }
}

fn error_style() -> Style {
    Style::new().red().bold()
}

fn warning_style() -> Style {
    Style::new().yellow().bold()
}

fn line_style() -> Style {
    Style::new().bright_black()
}

fn message_style() -> Style {
    Style::new().white()
}

fn looks_like_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

fn todo_txt_to_md(text: &str) -> String {
    let mut output = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let task: Simple = trimmed.to_string().into();
        let title = title_from_simple(&task);
        let marker = if task.finished { "x" } else { " " };
        output.push_str(&format!("- [{marker}] {title}\n"));

        if let Some(uuid) = task.tags.get("uuid") {
            output.push_str(&format!("  uuid: {uuid}\n"));
        }
        if let Some(id) = task.tags.get("id") {
            output.push_str(&format!("  id: {id}\n"));
        }
        if let Some(create_date) = task.create_date {
            output.push_str(&format!("  entry: {}\n", create_date.format("%Y-%m-%d")));
        }
        if let Some(finish_date) = task.finish_date {
            output.push_str(&format!("  end: {}\n", finish_date.format("%Y-%m-%d")));
        }
        if let Some(due_date) = task.due_date {
            output.push_str(&format!("  due: {}\n", due_date.format("%Y-%m-%d")));
        }
        if let Some(threshold_date) = task.threshold_date {
            output.push_str(&format!("  wait: {}\n", threshold_date.format("%Y-%m-%d")));
        }
        for (key, value) in task.tags {
            if key != "id" && key != "uuid" {
                output.push_str(&format!("  {key}: {value}\n"));
            }
        }
        output.push('\n');
    }

    output
}

fn title_from_simple(task: &Simple) -> String {
    let mut title = String::new();
    if !task.priority.is_lowest() {
        title.push_str(&format!("({}) ", task.priority));
    }
    title.push_str(&task.subject);
    title
}

fn md_to_todo_txt(text: &str) -> Result<String, String> {
    let tasks = parse_todo_md(text)?;
    let mut output = String::new();

    for task in tasks {
        let mut line = String::new();
        if taskwarrior_status_from_md(&task) == "completed" {
            line.push_str("x ");
            if let Some(end) = task.fields.get("end").filter(|value| !value.is_empty()) {
                line.push_str(&format!("{} ", normalize_dateish(end)));
            }
            if let Some(entry) = task.fields.get("entry").filter(|value| !value.is_empty()) {
                line.push_str(&format!("{} ", normalize_dateish(entry)));
            }
        }
        line.push_str(&task.title);

        for (key, value) in ordered_todo_txt_fields(&task.fields) {
            if !value.is_empty() && !line_contains_key(&line, key) {
                let txt_key = match key {
                    "wait" => "t",
                    "entry" | "end" | "status" | "depends" | "aliases" | "source" => continue,
                    _ => key,
                };
                line.push_str(&format!(" {txt_key}:{}", normalize_dateish(value)));
            }
        }
        output.push_str(line.trim());
        output.push('\n');
    }

    Ok(output)
}

fn ordered_todo_txt_fields(fields: &BTreeMap<String, String>) -> Vec<(&str, &str)> {
    const PREFERRED: &[&str] = &[
        "id",
        "u",
        "uuid",
        "due",
        "wait",
        "t",
        "scheduled",
        "recur",
        "estimate",
    ];
    let mut ordered = Vec::new();

    for key in PREFERRED {
        if let Some(value) = fields.get(*key) {
            ordered.push((*key, value.as_str()));
        }
    }

    for (key, value) in fields {
        if !PREFERRED.contains(&key.as_str()) {
            ordered.push((key.as_str(), value.as_str()));
        }
    }

    ordered
}

fn normalize_todo_md(text: &str) -> String {
    let document = split_todo_md_document(text);
    render_todo_md_document(&document, &extract_todo_md_tasks(text))
}

fn render_todo_md_document(document: &TodoMdDocument, tasks: &[MdTask]) -> String {
    let mut output = String::new();
    if has_todo_md_frontmatter(document) {
        output.push_str("---\ntextwarrior: 1\n");
        if !document.defaults.is_empty() {
            output.push_str("defaults:\n");
            for (key, value) in &document.defaults {
                output.push_str(&format!("  {key}: {value}\n"));
            }
        }
        if !document.show_status || document.compact_uuid {
            output.push_str("render:\n");
            if !document.show_status {
                output.push_str("  status: false\n");
            }
            if document.compact_uuid {
                output.push_str("  uuid: compact\n");
            }
        }
        if let Some(id_prefix) = &document.id_prefix {
            output.push_str(&format!("id_prefix: {id_prefix}\n"));
        }
        if let Some(details_path) = &document.details_path {
            output.push_str(&format!("details_path: {details_path}\n"));
        }
        output.push_str("---\n\n");
    }
    output.push_str(&render_todo_md_with_options(tasks, document));
    output
}

fn has_todo_md_frontmatter(document: &TodoMdDocument) -> bool {
    !document.defaults.is_empty()
        || !document.show_status
        || document.compact_uuid
        || document.id_prefix.is_some()
        || document.details_path.is_some()
}

fn render_todo_md_with_options(tasks: &[MdTask], document: &TodoMdDocument) -> String {
    let mut output = String::new();

    for task in tasks {
        output.push_str(&render_todo_md_task(task, document));
    }

    output
}

fn render_todo_md_task(task: &MdTask, document: &TodoMdDocument) -> String {
    let mut output = String::new();
    let checked = render_check_state(task, document);
    let title = render_todo_md_title_with_dates(task, document, checked);
    output.push_str(&format!("- [{}] {}", checked.as_markdown_marker(), title));

    let mut multiline_fields = Vec::new();
    for (key, value) in ordered_todo_md_fields(&task.fields) {
        if value.is_empty() {
            continue;
        }
        if matches!(key, "entry" | "end") {
            continue;
        }
        if key == "status" && !document.show_status {
            continue;
        }
        if default_field_matches(document, key, value) {
            continue;
        }
        let Some((key, value)) = render_todo_md_field(document, key, value) else {
            continue;
        };
        if value.split_whitespace().count() <= 1 {
            output.push_str(&format!(" {key}:{value}"));
        } else {
            multiline_fields.push((key, value));
        }
    }
    output.push('\n');

    for (key, value) in &multiline_fields {
        output.push_str(&format!("  {key}: {value}\n"));
    }

    if !task.notes.is_empty() {
        output.push('\n');
        for note in &task.notes {
            output.push_str(&format!("  {note}\n"));
        }
    }

    output.push('\n');
    output
}

fn render_check_state(task: &MdTask, document: &TodoMdDocument) -> CheckState {
    let Some(status) = task.fields.get("status").map(String::as_str) else {
        return task.checked;
    };
    if document.show_status {
        return task.checked;
    }
    match status {
        "in_progress" | "active" | "started" => CheckState::InProgress,
        "review" | "waiting_review" => CheckState::Review,
        "blocked" => CheckState::Blocked,
        "done" | "completed" => CheckState::Done,
        "deleted" | "cancelled" | "canceled" | "dropped" => CheckState::Deleted,
        _ => task.checked,
    }
}

fn render_todo_md_title(title: &str, document: &TodoMdDocument) -> String {
    let Some(project) = document.defaults.get("project") else {
        return title.to_string();
    };
    title
        .split_whitespace()
        .filter(|word| *word != format!("+{project}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_todo_md_title_with_dates(
    task: &MdTask,
    document: &TodoMdDocument,
    checked: CheckState,
) -> String {
    let title = render_todo_md_title(&task.title, document);
    let entry = task.fields.get("entry").filter(|value| !value.is_empty());
    let end = task.fields.get("end").filter(|value| !value.is_empty());

    if checked == CheckState::Done {
        let mut parts = Vec::new();
        if let Some(end) = end {
            parts.push(normalize_dateish(end));
        }
        if let Some(entry) = entry {
            parts.push(normalize_dateish(entry));
        }
        parts.push(title);
        return parts.join(" ");
    }

    let Some(entry) = entry else {
        return title;
    };
    let entry = normalize_dateish(entry);
    if priority_from_title(&title).is_some() {
        format!("{} {entry} {}", &title[..3], title[4..].trim_start())
    } else {
        format!("{entry} {title}")
    }
}

fn default_field_matches(document: &TodoMdDocument, key: &str, value: &str) -> bool {
    document
        .defaults
        .get(key)
        .is_some_and(|default| normalize_inline_metadata_value(key, default) == value)
}

fn compact_rendered_field(document: &TodoMdDocument, key: &str, value: &str) -> String {
    if key == "id"
        && let Some(prefix) = &document.id_prefix
        && let Some(rest) = value.strip_prefix(&format!("{prefix}-"))
    {
        return format!("{prefix}-{}", trim_numeric_id_padding(rest));
    }
    if key == "id"
        && let (Some(prefix), Some(project)) =
            (&document.id_prefix, document.defaults.get("project"))
        && let Some(rest) = value.strip_prefix(&format!("{project}-"))
    {
        return format!("{prefix}-{}", trim_numeric_id_padding(rest));
    }
    value.to_string()
}

fn render_todo_md_field(
    document: &TodoMdDocument,
    key: &str,
    value: &str,
) -> Option<(String, String)> {
    let normalized = normalize_inline_metadata_value(key, value);
    let value = normalized.as_str();
    if key == "uuid"
        && document.compact_uuid
        && let Some(encoded) = encode_uuid_compact(value)
    {
        return Some(("u".to_string(), encoded));
    }
    Some((
        key.to_string(),
        compact_rendered_field(document, key, value),
    ))
}

fn trim_numeric_id_padding(value: &str) -> String {
    if !value.is_empty() && value.chars().all(|char| char.is_ascii_digit()) {
        let trimmed = value.trim_start_matches('0');
        return if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_string()
        };
    }
    value.to_string()
}

fn ordered_todo_md_fields(fields: &BTreeMap<String, String>) -> Vec<(&str, &str)> {
    const PREFERRED: &[&str] = &[
        "uuid",
        "u",
        "id",
        "status",
        "entry",
        "end",
        "due",
        "wait",
        "t",
        "scheduled",
        "recur",
        "priority",
        "project",
        "tags",
        "depends",
        "source",
        "md",
        "ext",
        "assignee",
        "parent",
        "estimate",
        "updated",
        "issue",
        "pm",
        "aliases",
    ];
    let mut ordered = Vec::new();

    for key in PREFERRED {
        if let Some(value) = fields.get(*key) {
            ordered.push((*key, value.as_str()));
        }
    }

    for (key, value) in fields {
        if !PREFERRED.contains(&key.as_str()) {
            ordered.push((key.as_str(), value.as_str()));
        }
    }

    ordered
}

fn line_contains_key(line: &str, key: &str) -> bool {
    let needle = format!("{key}:");
    line.split_whitespace()
        .any(|word| word.starts_with(&needle))
}

fn task_json_to_md(text: &str) -> Result<String, String> {
    task_json_to_md_with_document(text, &TodoMdDocument::default())
}

fn task_json_to_md_with_document(text: &str, document: &TodoMdDocument) -> Result<String, String> {
    let tasks: Vec<TaskwarriorTask> =
        serde_json::from_str(text).map_err(|error| format!("invalid Taskwarrior JSON: {error}"))?;
    let mut md_tasks = Vec::new();

    for task in tasks {
        let checked = match task.status.as_deref() {
            Some("completed") => CheckState::Done,
            Some("deleted") => CheckState::Deleted,
            _ => CheckState::Open,
        };
        let (restored_fields, remaining_annotations) = restore_textwarrior_fields(task.annotations);
        let mut title = String::new();
        if let Some(priority) = &task.priority {
            title.push_str(&format!("({priority}) "));
        }
        title.push_str(&task.description);
        if let Some(project) = &task.project {
            title.push_str(&format!(" +{project}"));
        }
        for tag in &task.tags {
            title.push_str(&format!(" @{tag}"));
        }

        let status = restored_fields
            .get("status")
            .cloned()
            .or(task.status)
            .filter(|status| !status.is_empty());
        let mut fields = restored_fields;
        if fields.get("source").map(String::as_str) == Some("taskwarrior")
            && let Some(default_source) = document.defaults.get("source")
        {
            fields.insert("source".to_string(), default_source.clone());
        }
        insert_optional_field(&mut fields, "uuid", task.uuid.as_deref());
        insert_optional_field(&mut fields, "status", status.as_deref());
        insert_optional_date_field(&mut fields, "entry", task.entry.as_deref());
        insert_optional_date_field(&mut fields, "end", task.end.as_deref());
        insert_optional_date_field(&mut fields, "due", task.due.as_deref());
        insert_optional_date_field(&mut fields, "wait", task.wait.as_deref());
        insert_optional_date_field(&mut fields, "scheduled", task.scheduled.as_deref());
        insert_optional_field(&mut fields, "recur", task.recur.as_deref());
        if !task.depends.is_empty() {
            fields.insert(
                "depends".to_string(),
                format!("[{}]", task.depends.join(", ")),
            );
        }
        let default_source = document
            .defaults
            .get("source")
            .cloned()
            .unwrap_or_else(|| "taskwarrior".to_string());
        fields.entry("source".to_string()).or_insert(default_source);

        let notes = remaining_annotations
            .into_iter()
            .map(|annotation| {
                if let Some(entry) = annotation.entry {
                    format!("[{entry}] {}", annotation.description)
                } else {
                    annotation.description
                }
            })
            .collect();
        md_tasks.push(MdTask {
            checked,
            title,
            fields,
            notes,
        });
    }

    Ok(render_todo_md_document(document, &md_tasks))
}

fn restore_textwarrior_fields(
    annotations: Vec<TaskwarriorAnnotation>,
) -> (BTreeMap<String, String>, Vec<TaskwarriorAnnotation>) {
    let mut fields = BTreeMap::new();
    let mut remaining = Vec::new();

    for annotation in annotations {
        let Some(value) = annotation.description.strip_prefix("textwarrior ") else {
            remaining.push(annotation);
            continue;
        };
        let Some((key, value)) = value.split_once(':') else {
            remaining.push(annotation);
            continue;
        };
        if is_metadata_key(key) && !value.is_empty() {
            fields.insert(key.to_string(), value.to_string());
        } else {
            remaining.push(annotation);
        }
    }

    (fields, remaining)
}

fn insert_optional_field(fields: &mut BTreeMap<String, String>, key: &str, value: Option<&str>) {
    if let Some(value) = value
        && !value.is_empty()
    {
        fields.insert(key.to_string(), value.to_string());
    }
}

fn insert_optional_date_field(
    fields: &mut BTreeMap<String, String>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value
        && !value.is_empty()
    {
        fields.insert(key.to_string(), normalize_dateish(value));
    }
}

fn normalize_dateish(value: &str) -> String {
    let bytes = value.as_bytes();
    if looks_like_iso_date(value) {
        return value.to_string();
    }

    if bytes.len() >= 8 && bytes[..8].iter().all(u8::is_ascii_digit) {
        return format!("{}-{}-{}", &value[0..4], &value[4..6], &value[6..8]);
    }

    value.to_string()
}

fn md_to_task_json(text: &str) -> Result<String, String> {
    let tasks = parse_todo_md(text)?;
    let taskwarrior: Vec<TaskwarriorTask> =
        tasks.into_iter().map(taskwarrior_task_from_md).collect();

    serde_json::to_string_pretty(&taskwarrior)
        .map(|json| format!("{json}\n"))
        .map_err(|error| format!("failed to serialize Taskwarrior JSON: {error}"))
}

fn taskwarrior_task_from_md(task: MdTask) -> TaskwarriorTask {
    let (description, project, tags) = split_description_project_tags(strip_priority(&task.title));
    let explicit_project = task.fields.get("project").cloned();
    let tags = if task.fields.contains_key("project") {
        title_plus_words(strip_priority(&task.title))
            .into_iter()
            .chain(tags)
            .collect()
    } else {
        tags
    };
    let (depends, local_depends) = split_taskwarrior_depends(task.fields.get("depends"));
    let annotations = taskwarrior_annotations_from_md(&task, local_depends);
    let uuid = task.fields.get("uuid").cloned().or_else(|| {
        task.fields
            .get("u")
            .and_then(|value| decode_uuid_compact(value))
    });

    TaskwarriorTask {
        uuid,
        status: Some(taskwarrior_status_from_md(&task).to_string()),
        description,
        entry: task.fields.get("entry").cloned(),
        end: task.fields.get("end").cloned(),
        due: task.fields.get("due").cloned(),
        wait: task.fields.get("wait").cloned(),
        scheduled: task.fields.get("scheduled").cloned(),
        recur: task.fields.get("recur").cloned(),
        priority: priority_from_title(&task.title).or_else(|| task.fields.get("priority").cloned()),
        project: explicit_project.or(project),
        tags: normalized_taskwarrior_tags(tags),
        depends,
        annotations,
    }
}

fn taskwarrior_status_from_md(task: &MdTask) -> &'static str {
    if let Some(status) = task.fields.get("status") {
        match status.as_str() {
            "done" | "completed" => return "completed",
            "deleted" | "cancelled" | "canceled" | "dropped" => return "deleted",
            _ => {}
        }
    }

    match task.checked {
        CheckState::Open => "pending",
        CheckState::Done => "completed",
        CheckState::Deleted => "deleted",
        CheckState::InProgress | CheckState::Review | CheckState::Blocked => "pending",
    }
}

fn taskwarrior_annotations_from_md(
    task: &MdTask,
    local_depends: Vec<String>,
) -> Vec<TaskwarriorAnnotation> {
    let mut annotations = Vec::new();

    for key in ["id", "source", "status", "md", "ext", "assignee", "parent"] {
        let hidden_status = (key == "status")
            .then(|| task.checked.hidden_status())
            .flatten();
        let value = task.fields.get(key).map(String::as_str).or(hidden_status);
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            annotations.push(TaskwarriorAnnotation {
                entry: None,
                description: format!("textwarrior {key}:{value}"),
            });
        }
    }

    if !local_depends.is_empty() {
        annotations.push(TaskwarriorAnnotation {
            entry: None,
            description: format!("textwarrior depends:{}", local_depends.join(",")),
        });
    }

    annotations.extend(
        task.notes
            .iter()
            .map(|description| taskwarrior_annotation_from_note(description)),
    );

    annotations
}

fn taskwarrior_annotation_from_note(note: &str) -> TaskwarriorAnnotation {
    if let Some((entry, description)) = split_annotation_entry(note) {
        TaskwarriorAnnotation {
            entry: Some(entry.to_string()),
            description: description.to_string(),
        }
    } else {
        TaskwarriorAnnotation {
            entry: None,
            description: note.to_string(),
        }
    }
}

fn split_annotation_entry(note: &str) -> Option<(&str, &str)> {
    let rest = note.strip_prefix('[')?;
    let (entry, description) = rest.split_once("] ")?;
    if looks_like_taskwarrior_timestamp(entry) {
        Some((entry, description))
    } else {
        None
    }
}

fn looks_like_taskwarrior_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 16
        && bytes[8] == b'T'
        && bytes[15] == b'Z'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 15) || byte.is_ascii_digit())
}

fn split_taskwarrior_depends(value: Option<&String>) -> (Vec<String>, Vec<String>) {
    let mut taskwarrior = Vec::new();
    let mut local = Vec::new();

    for item in split_list(value) {
        if looks_like_uuid(&item) {
            taskwarrior.push(item);
        } else {
            local.push(item);
        }
    }

    (taskwarrior, local)
}

fn looks_like_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn encode_uuid_compact(value: &str) -> Option<String> {
    let hex = value.replace('-', "");
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = Vec::with_capacity(16);
    for chunk in hex.as_bytes().chunks(2) {
        let text = std::str::from_utf8(chunk).ok()?;
        bytes.push(u8::from_str_radix(text, 16).ok()?);
    }
    Some(base32hex_encode_no_padding(&bytes))
}

fn decode_uuid_compact(value: &str) -> Option<String> {
    let bytes = base32hex_decode_no_padding(value)?;
    if bytes.len() != 16 {
        return None;
    }
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn base32hex_encode_no_padding(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    let mut output = String::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            let index = ((buffer >> (bits - 5)) & 0b11111) as usize;
            output.push(ALPHABET[index] as char);
            bits -= 5;
        }
        if bits > 0 {
            buffer &= (1 << bits) - 1;
        } else {
            buffer = 0;
        }
    }

    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0b11111) as usize;
        output.push(ALPHABET[index] as char);
    }

    output
}

fn base32hex_decode_no_padding(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in value.bytes() {
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'v' => byte - b'a' + 10,
            b'A'..=b'V' => byte - b'A' + 10,
            _ => return None,
        };
        buffer = (buffer << 5) | u32::from(value);
        bits += 5;
        while bits >= 8 {
            output.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
        if bits > 0 {
            buffer &= (1 << bits) - 1;
        } else {
            buffer = 0;
        }
    }

    if bits > 0 && (buffer & ((1 << bits) - 1)) != 0 {
        return None;
    }

    Some(output)
}

fn split_description_project_tags(title: &str) -> (String, Option<String>, Vec<String>) {
    let mut description = Vec::new();
    let mut project = None;
    let mut tags = Vec::new();

    for word in title.split_whitespace() {
        if let Some(value) = word.strip_prefix('+')
            && !value.is_empty()
        {
            if project.is_none() {
                project = Some(value.to_string());
            }
            continue;
        }

        if let Some(value) = word.strip_prefix('@')
            && !value.is_empty()
        {
            tags.push(value.to_string());
            continue;
        }

        description.push(word);
    }

    (description.join(" "), project, tags)
}

fn title_plus_words(title: &str) -> Vec<String> {
    title
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('+'))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalized_taskwarrior_tags(mut tags: Vec<String>) -> Vec<String> {
    tags.sort();
    tags.dedup();
    tags
}

fn parse_todo_md(text: &str) -> Result<Vec<MdTask>, String> {
    let document = split_todo_md_document(text);
    let mut tasks = Vec::new();
    let mut current: Option<MdTask> = None;
    let mut in_notes = false;

    for (index, line) in document.body.lines().enumerate() {
        if let Some((checked, title)) = parse_task_heading(line) {
            if let Some(task) = current.take() {
                tasks.push(task);
            }
            let (title, fields) = extract_todo_md_heading_fields(checked, title);
            let mut task = MdTask {
                checked,
                title,
                fields,
                notes: Vec::new(),
            };
            apply_document_defaults(&mut task, &document.defaults);
            current = Some(task);
            in_notes = false;
            continue;
        }

        let Some(task) = current.as_mut() else {
            if line.trim().is_empty() {
                continue;
            }
            return Err(format!("{}: content before first task", index + 1));
        };

        if line.trim().is_empty() {
            in_notes = true;
            continue;
        }

        let Some(indented) = line.strip_prefix("  ") else {
            return Err(format!("{}: expected indented metadata or note", index + 1));
        };

        if !in_notes && let Some((key, value)) = split_metadata_line(indented) {
            task.fields
                .insert(key.trim().to_string(), value.trim().to_string());
            continue;
        }

        task.notes.push(indented.to_string());
    }

    if let Some(task) = current.take() {
        tasks.push(task);
    }

    Ok(tasks)
}

fn extract_todo_md_tasks(text: &str) -> Vec<MdTask> {
    let document = split_todo_md_document(text);
    let mut tasks = Vec::new();
    let mut current: Option<MdTask> = None;
    let mut in_notes = false;

    for line in document.body.lines() {
        if let Some((checked, title)) = parse_task_heading(line) {
            if let Some(task) = current.take() {
                tasks.push(task);
            }
            let (title, fields) = extract_todo_md_heading_fields(checked, title);
            let mut task = MdTask {
                checked,
                title,
                fields,
                notes: Vec::new(),
            };
            apply_document_defaults(&mut task, &document.defaults);
            current = Some(task);
            in_notes = false;
            continue;
        }

        let Some(task) = current.as_mut() else {
            continue;
        };

        if line.trim().is_empty() {
            in_notes = true;
            continue;
        }

        if let Some(indented) = line.strip_prefix("  ") {
            if !in_notes && let Some((key, value)) = split_metadata_line(indented) {
                task.fields
                    .insert(key.trim().to_string(), value.trim().to_string());
                continue;
            }
            task.notes.push(indented.to_string());
            continue;
        }

        if let Some(task) = current.take() {
            tasks.push(task);
        }
        in_notes = false;
    }

    if let Some(task) = current.take() {
        tasks.push(task);
    }

    tasks
}

fn split_todo_md_document(text: &str) -> TodoMdDocument {
    let Some(rest) = text.strip_prefix("---\n") else {
        return TodoMdDocument {
            defaults: BTreeMap::new(),
            show_status: true,
            compact_uuid: false,
            id_prefix: None,
            details_path: None,
            body: text.to_string(),
        };
    };

    let Some((frontmatter, body)) = rest.split_once("\n---") else {
        return TodoMdDocument {
            defaults: BTreeMap::new(),
            show_status: true,
            compact_uuid: false,
            id_prefix: None,
            details_path: None,
            body: text.to_string(),
        };
    };

    let body = body.strip_prefix('\n').unwrap_or(body).to_string();
    let settings = parse_frontmatter(frontmatter);
    TodoMdDocument {
        defaults: settings.defaults,
        show_status: settings.show_status,
        compact_uuid: settings.compact_uuid,
        id_prefix: settings.id_prefix,
        details_path: settings.details_path,
        body,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TodoMdFrontmatter {
    defaults: BTreeMap<String, String>,
    show_status: bool,
    compact_uuid: bool,
    id_prefix: Option<String>,
    details_path: Option<String>,
}

fn parse_frontmatter(frontmatter: &str) -> TodoMdFrontmatter {
    let mut defaults = BTreeMap::new();
    let mut show_status = true;
    let mut compact_uuid = false;
    let mut id_prefix = None;
    let mut details_path = None;
    let mut section: Option<&str> = None;

    for line in frontmatter.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        if !line.starts_with(' ') && !line.starts_with('\t') {
            let Some((key, value)) = line.split_once(':') else {
                section = None;
                continue;
            };
            let key = key.trim();
            let value = trim_yaml_scalar(value.trim());
            if key == "defaults" {
                section = Some("defaults");
                continue;
            }
            if key == "render" {
                section = Some("render");
                continue;
            }
            section = None;
            if is_metadata_key(key) && !value.is_empty() {
                defaults.insert(key.to_string(), value.to_string());
            }
            if key == "id_prefix" && !value.is_empty() {
                id_prefix = Some(value.to_string());
            }
            if key == "details_path" && !value.is_empty() {
                details_path = Some(value.to_string());
            }
            continue;
        }

        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = trim_yaml_scalar(value.trim());
        match section {
            Some("defaults") => {
                if is_metadata_key(key) && !value.is_empty() {
                    defaults.insert(key.to_string(), value.to_string());
                }
            }
            Some("render") => {
                if key == "status" {
                    show_status = !matches!(value, "false" | "hidden" | "checkbox" | "marker");
                }
                if key == "uuid" {
                    compact_uuid = matches!(value, "compact" | "short" | "u");
                }
            }
            _ => {}
        }
    }

    TodoMdFrontmatter {
        defaults,
        show_status,
        compact_uuid,
        id_prefix,
        details_path,
    }
}

fn trim_yaml_scalar(value: &str) -> &str {
    value.trim_matches('"').trim_matches('\'').trim()
}

fn apply_document_defaults(task: &mut MdTask, defaults: &BTreeMap<String, String>) {
    for (key, value) in defaults {
        task.fields
            .entry(key.clone())
            .or_insert_with(|| normalize_inline_metadata_value(key, value));
    }
}

fn extract_todo_md_heading_fields(
    checked: CheckState,
    title: &str,
) -> (String, BTreeMap<String, String>) {
    let (title, mut fields) = extract_inline_fields(title);
    let mut words = title.split_whitespace().collect::<Vec<_>>();

    if checked == CheckState::Done {
        if words.first().is_some_and(|word| looks_like_iso_date(word)) {
            let end = words.remove(0);
            fields
                .entry("end".to_string())
                .or_insert_with(|| end.to_string());
        }
        if words.first().is_some_and(|word| looks_like_iso_date(word)) {
            let entry = words.remove(0);
            fields
                .entry("entry".to_string())
                .or_insert_with(|| entry.to_string());
        }
        return (words.join(" "), fields);
    }

    if priority_from_title(&title).is_some() && words.len() >= 2 {
        if looks_like_iso_date(words[1]) {
            let entry = words.remove(1);
            fields
                .entry("entry".to_string())
                .or_insert_with(|| entry.to_string());
            return (words.join(" "), fields);
        }
    } else if words.first().is_some_and(|word| looks_like_iso_date(word)) {
        let entry = words.remove(0);
        fields
            .entry("entry".to_string())
            .or_insert_with(|| entry.to_string());
        return (words.join(" "), fields);
    }

    (title, fields)
}

fn extract_inline_fields(title: &str) -> (String, BTreeMap<String, String>) {
    let expanded = title
        .replace("🆔", " 🆔 ")
        .replace("📅", " 📅 ")
        .replace("⏫", " ⏫ ")
        .replace("🔼", " 🔼 ")
        .replace("🔽", " 🔽 ")
        .replace("🔺", " 🔺 ");
    let mut words = expanded.split_whitespace();
    let mut fields = BTreeMap::new();
    let mut title_words = Vec::new();

    while let Some(word) = words.next() {
        match word {
            "🆔" => {
                if let Some(value) = words.next() {
                    fields.insert("id".to_string(), trim_inline_value(value).to_string());
                }
            }
            "📅" => {
                if let Some(value) = words.next() {
                    fields.insert(
                        "due".to_string(),
                        normalize_dateish(trim_inline_value(value)),
                    );
                }
            }
            "🔺" | "⏫" => {
                fields.insert("priority".to_string(), "H".to_string());
            }
            "🔼" => {
                fields.insert("priority".to_string(), "M".to_string());
            }
            "🔽" => {
                fields.insert("priority".to_string(), "L".to_string());
            }
            _ => {
                if let Some((key, value)) = split_inline_metadata_word(word) {
                    fields.insert(key.to_string(), normalize_inline_metadata_value(key, value));
                } else {
                    title_words.push(word.to_string());
                }
            }
        }
    }

    (title_words.join(" "), fields)
}

fn split_inline_metadata_word(word: &str) -> Option<(&str, &str)> {
    let (key, value) = word.split_once(':')?;
    if value.is_empty() || !is_metadata_key(key) {
        return None;
    }
    Some((key, trim_inline_value(value)))
}

fn normalize_inline_metadata_value(key: &str, value: &str) -> String {
    if matches!(key, "due" | "t" | "wait" | "entry" | "end" | "scheduled") {
        normalize_dateish(value)
    } else if key == "md" {
        normalize_md_pointer(value)
    } else {
        value.to_string()
    }
}

fn normalize_md_pointer(value: &str) -> String {
    value.strip_suffix(".md").unwrap_or(value).to_string()
}

fn split_metadata_line(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    let key = key.trim();
    if !is_metadata_key(key) {
        return None;
    }
    Some((key, value))
}

fn is_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "u"
            | "uuid"
            | "status"
            | "entry"
            | "end"
            | "due"
            | "wait"
            | "t"
            | "scheduled"
            | "recur"
            | "priority"
            | "project"
            | "tags"
            | "depends"
            | "aliases"
            | "source"
            | "md"
            | "ext"
            | "assignee"
            | "parent"
            | "estimate"
            | "updated"
            | "issue"
            | "pm"
    )
}

fn trim_inline_value(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ';'))
}

fn parse_task_heading(line: &str) -> Option<(CheckState, &str)> {
    let rest = line.trim_start().strip_prefix("- [")?;
    let (marker, rest) = rest.split_once("] ")?;
    let checked = match marker {
        "" | " " => CheckState::Open,
        "x" | "X" => CheckState::Done,
        "-" => CheckState::Deleted,
        "/" => CheckState::InProgress,
        "?" => CheckState::Review,
        "!" => CheckState::Blocked,
        _ => return None,
    };
    Some((checked, rest))
}

fn priority_from_title(title: &str) -> Option<String> {
    let bytes = title.as_bytes();
    if bytes.len() >= 4
        && bytes[0] == b'('
        && bytes[1].is_ascii_uppercase()
        && bytes[2] == b')'
        && bytes[3] == b' '
    {
        Some((bytes[1] as char).to_string())
    } else {
        None
    }
}

fn strip_priority(title: &str) -> &str {
    if priority_from_title(title).is_some() {
        &title[4..]
    } else {
        title
    }
}

fn split_list(value: Option<&String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    value
        .trim_matches(['[', ']'])
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn unique_temp_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "textwarrior-{name}-{}-{}",
            std::process::id(),
            unix_timestamp_nanos()
        ))
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn write_mock_task_command(dir: &Path, export_json: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let export_path = dir.join("task-export.json");
        let import_path = dir.join("task-imported.json");
        let args_path = dir.join("task-args.txt");
        let script_path = dir.join("mock-task");
        fs::write(&export_path, export_json).unwrap();
        fs::write(
            &script_path,
            format!(
                "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$*\" >> {args}\nlast=''\nfor arg in \"$@\"; do last=\"$arg\"; done\nif [ \"$last\" = 'export' ]; then\n  cat {export}\n  exit 0\nfi\nif [ \"${{1:-}}\" = 'import' ]; then\n  cp \"$2\" {import}\n  exit 0\nfi\necho \"unexpected mock task args: $*\" >&2\nexit 1\n",
                args = shell_quote(&args_path),
                export = shell_quote(&export_path),
                import = shell_quote(&import_path),
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).unwrap();
        script_path
    }

    fn test_merge_context<'a>(
        previous: &'a SyncState,
        todo_dir: &'a Path,
        current_locations: &'a BTreeMap<String, MdTaskLocation>,
        exported_identities: &'a BTreeSet<String>,
        preserve_missing_tasks: bool,
        strategy: ConflictStrategy,
    ) -> MergeContext<'a> {
        MergeContext {
            previous,
            todo_dir,
            current_locations,
            exported_identities,
            preserve_missing_tasks,
            strategy,
        }
    }

    #[test]
    fn lints_missing_identity() {
        let findings = lint_todo_txt("(A) 2026-07-04 fix exporter +finance");
        assert_eq!(
            findings,
            vec!["1: missing stable id: or uuid: metadata".to_string()]
        );
    }

    #[test]
    fn lints_non_iso_todo_txt_dates() {
        let findings = lint_todo_txt("fix exporter id:tw-1 due:20260706T000000Z");
        assert_eq!(findings, vec!["1: due: should be YYYY-MM-DD".to_string()]);
    }

    #[test]
    fn lists_conflict_strategy_names() {
        let names = ConflictStrategy::all()
            .into_iter()
            .map(ConflictStrategy::name)
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["fail", "md", "taskwarrior"]);
    }

    #[test]
    fn parses_syncall_strategy_list_alias() {
        let cli =
            Cli::try_parse_from(["textwarrior", "sync", "--list-resolution-strategies"]).unwrap();

        match cli.command {
            Command::Sync {
                list_strategies, ..
            } => assert!(list_strategies),
            _ => panic!("expected sync command"),
        }
    }

    #[test]
    fn parses_strategy_listing_with_missing_config_path() {
        let cli = Cli::try_parse_from([
            "textwarrior",
            "--config",
            "/no/such/file",
            "sync",
            "--list-strategies",
        ])
        .unwrap();

        assert_eq!(cli.config.as_deref(), Some(Path::new("/no/such/file")));
        match cli.command {
            Command::Sync {
                list_strategies, ..
            } => assert!(list_strategies),
            _ => panic!("expected sync command"),
        }
    }

    #[test]
    fn strategy_listing_runs_without_reading_missing_config() {
        let cli = Cli::try_parse_from([
            "textwarrior",
            "--config",
            "/no/such/file",
            "sync",
            "--list-strategies",
        ])
        .unwrap();

        assert!(run_cli(cli).is_ok());
    }

    #[test]
    fn does_not_lint_title_colon_as_empty_metadata() {
        let findings = lint_todo_txt("Eusébio: Research ways id:tw-1");
        assert!(findings.is_empty());
    }

    #[test]
    fn lints_todo_md_missing_identity_as_error() {
        let findings = lint_todo_md("- [ ] Fix exporter\n");
        assert_eq!(
            findings,
            vec![LintFinding::error(
                Some(1),
                "missing stable id:, uuid:, or u: metadata"
            )]
        );
    }

    #[test]
    fn lints_todo_md_bad_dates_as_error() {
        let findings = lint_todo_md("- [ ] Fix exporter id:tw-1\n  due: 20260706\n");
        assert!(findings.contains(&LintFinding::error(Some(1), "due: should be YYYY-MM-DD")));
    }

    #[test]
    fn warns_when_todo_md_field_order_differs() {
        let findings = lint_todo_md("- [ ] Fix exporter source:manual id:tw-1\n");
        assert_eq!(
            findings,
            vec![LintFinding::warning(
                None,
                "format differs from normalize-md output"
            )]
        );
    }

    #[test]
    fn formats_lint_findings_with_optional_color() {
        let finding = LintFinding::error(Some(7), "missing stable id");
        assert_eq!(
            format_lint_finding(&finding, false),
            "7: error: missing stable id"
        );
        assert!(format_lint_finding(&finding, true).contains("\u{1b}["));
    }

    #[test]
    fn converts_todo_txt_to_markdown() {
        let md =
            todo_txt_to_md("(A) 2026-07-04 fix exporter +finance @desk due:2026-07-06 id:tw-1\n");
        assert!(md.contains("- [ ] (A) fix exporter +finance @desk"));
        assert!(md.contains("  id: tw-1"));
        assert!(md.contains("  entry: 2026-07-04"));
        assert!(md.contains("  due: 2026-07-06"));
    }

    #[test]
    fn converts_markdown_to_todo_txt() {
        let txt =
            md_to_todo_txt("- [ ] (A) Fix exporter +finance @desk id:tw-1\n  due: 2026-07-06\n\n")
                .unwrap();
        assert_eq!(
            txt,
            "(A) Fix exporter +finance @desk id:tw-1 due:2026-07-06\n"
        );
    }

    #[test]
    fn converts_completed_markdown_to_todo_txt_with_dates() {
        let txt =
            md_to_todo_txt("- [x] Done task id:tw-1\n  entry: 2026-07-04\n  end: 2026-07-05\n\n")
                .unwrap();
        assert_eq!(txt, "x 2026-07-05 2026-07-04 Done task id:tw-1\n");
    }

    #[test]
    fn normalizes_todo_md_creation_date_after_priority() {
        let md = normalize_todo_md("- [ ] (H) Fix exporter id:tw-1 entry:2026-07-04\n");

        assert_eq!(md, "- [ ] (H) 2026-07-04 Fix exporter id:tw-1\n\n");
        let json = md_to_task_json(&md).unwrap();
        assert!(json.contains("\"entry\": \"2026-07-04\""));
        assert!(json.contains("\"description\": \"Fix exporter\""));
    }

    #[test]
    fn normalizes_done_todo_md_dates_after_checkbox() {
        let md = normalize_todo_md("- [x] Done task id:tw-1 entry:2026-07-04 end:2026-07-05\n");

        assert_eq!(md, "- [x] 2026-07-05 2026-07-04 Done task id:tw-1\n\n");
        let json = md_to_task_json(&md).unwrap();
        assert!(json.contains("\"entry\": \"2026-07-04\""));
        assert!(json.contains("\"end\": \"2026-07-05\""));
        assert!(json.contains("\"description\": \"Done task\""));
    }

    #[test]
    fn normalizes_markdown_detail_pointer_without_extension() {
        let md = normalize_todo_md("- [ ] Fix exporter id:tw-1 md:tw-1.md\n");

        assert_eq!(md, "- [ ] Fix exporter id:tw-1 md:tw-1\n\n");
        let json = md_to_task_json(&md).unwrap();
        assert!(json.contains("\"description\": \"textwarrior md:tw-1\""));
    }

    #[test]
    fn normalizes_restored_markdown_detail_pointer_without_extension() {
        let md = task_json_to_md(
            r#"[{"uuid":"abc","status":"pending","description":"Fix exporter","annotations":[{"description":"textwarrior id:tw-1"},{"description":"textwarrior md:tw-1.md"}]}]"#,
        )
        .unwrap();

        assert!(md.contains(" md:tw-1"));
        assert!(!md.contains("md:tw-1.md"));
    }

    #[test]
    fn converts_task_json_to_markdown() {
        let md = task_json_to_md(
            r#"[{"uuid":"abc","status":"pending","description":"Fix exporter","priority":"H","project":"finance","tags":["desk"],"due":"20260706T000000Z"}]"#,
        )
        .unwrap();
        assert!(md.contains("- [ ] (H) Fix exporter +finance @desk"));
        assert!(md.contains(" uuid:abc"));
        assert!(md.contains(" due:2026-07-06"));
    }

    #[test]
    fn converts_markdown_to_task_json() {
        let json = md_to_task_json(
            "- [x] (A) Fix exporter\n  uuid: abc\n  due: 2026-07-06\n\n  Done locally.\n",
        )
        .unwrap();
        assert!(json.contains("\"status\": \"completed\""));
        assert!(json.contains("\"priority\": \"A\""));
        assert!(json.contains("\"description\": \"Fix exporter\""));
        assert!(json.contains("\"description\": \"Done locally.\""));
        assert!(!json.contains("null"));
    }

    #[test]
    fn converts_project_and_context_to_taskwarrior_fields() {
        let json = md_to_task_json("- [ ] Fix exporter +finance @desk\n").unwrap();
        assert!(json.contains("\"description\": \"Fix exporter\""));
        assert!(json.contains("\"project\": \"finance\""));
        assert!(json.contains("\"tags\": [\n      \"desk\"\n    ]"));
    }

    #[test]
    fn normalizes_taskwarrior_dates_for_todo_txt() {
        let txt = md_to_todo_txt(
            "- [ ] Fix exporter uuid:abc\n  due: 20260706T000000Z\n  wait: 20260707T000000Z\n",
        )
        .unwrap();
        assert_eq!(txt, "Fix exporter uuid:abc due:2026-07-06 t:2026-07-07\n");
    }

    #[test]
    fn imports_one_line_todo_md_metadata() {
        let json = md_to_task_json("- [ ] (A) Fix exporter +finance @desk id:f-1 due:2026-07-06\n")
            .unwrap();
        assert!(json.contains("\"description\": \"Fix exporter\""));
        assert!(json.contains("\"priority\": \"A\""));
        assert!(json.contains("\"project\": \"finance\""));
        assert!(json.contains("\"tags\": [\n      \"desk\"\n    ]"));
        assert!(json.contains("\"due\": \"2026-07-06\""));
    }

    #[test]
    fn keeps_one_line_todo_md_metadata_in_todo_txt() {
        let txt = md_to_todo_txt("- [ ] (A) Fix exporter +finance @desk id:f-1 due:2026-07-06\n")
            .unwrap();
        assert_eq!(
            txt,
            "(A) Fix exporter +finance @desk id:f-1 due:2026-07-06\n"
        );
    }

    #[test]
    fn normalizes_obsidian_task_metadata() {
        let md = normalize_todo_md("- [ ] test🆔 mgash1b⏫\n- [ ] hg 🆔 6ojzc7📅 2026-07-06\n");
        assert!(md.contains("- [ ] test id:mgash1b priority:H"));
        assert!(md.contains("- [ ] hg id:6ojzc7 due:2026-07-06"));
    }

    #[test]
    fn extracts_tasks_from_mixed_markdown() {
        let md = normalize_todo_md(
            "---\nid: todo\n---\n\n# todo\nprose\n- [ ] telproxy-agent\n  - link\n\nplain text\n- [] kamaworld\n",
        );
        assert!(md.contains("- [ ] telproxy-agent\n\n  - link"));
        assert!(md.contains("- [ ] kamaworld"));
        assert!(!md.contains("prose"));
    }

    #[test]
    fn keeps_indented_url_notes_as_notes() {
        let md = normalize_todo_md("- [ ] telproxy-agent\n  - https://example.com/thread\n");
        assert!(md.contains("  - https://example.com/thread"));
        assert!(!md.contains("https: //"));
    }

    #[test]
    fn keeps_note_labels_out_of_todo_txt_metadata() {
        let txt =
            md_to_todo_txt("- [ ] Fix exporter id:f-1\n  Link: https://example.com\n").unwrap();
        assert_eq!(txt, "Fix exporter id:f-1\n");
    }

    #[test]
    fn keeps_urls_in_titles() {
        let txt = md_to_todo_txt("- [ ] Review https://example.com id:f-1\n").unwrap();
        assert_eq!(txt, "Review https://example.com id:f-1\n");
    }

    #[test]
    fn preserves_local_depends_as_taskwarrior_annotation() {
        let json =
            md_to_task_json("- [ ] Fix exporter id:f-1 status:review depends:bd-1\n").unwrap();
        assert!(!json.contains("\"depends\""));
        assert!(json.contains("\"description\": \"textwarrior id:f-1\""));
        assert!(json.contains("\"description\": \"textwarrior status:review\""));
        assert!(json.contains("\"description\": \"textwarrior depends:bd-1\""));
    }

    #[test]
    fn keeps_uuid_depends_as_taskwarrior_depends() {
        let json =
            md_to_task_json("- [ ] Fix exporter depends:550e8400-e29b-41d4-a716-446655440000\n")
                .unwrap();
        assert!(
            json.contains("\"depends\": [\n      \"550e8400-e29b-41d4-a716-446655440000\"\n    ]")
        );
    }

    #[test]
    fn converts_beads_live_json_to_markdown() {
        let md = beads_json_to_md(
            r#"[{"id":"bd-1","title":"Fix bridge","description":"Line one\n\nLine two","status":"in_progress","priority":1,"issue_type":"task","labels":["repo:test"],"external_ref":"C13","assignee":"Marcelo Felix"}]"#,
            None,
            None,
            false,
        )
        .unwrap();

        assert!(md.contains("- [ ] Fix bridge +task +repo:test id:bd-1 source:beads"));
        assert!(md.contains("status:in_progress priority:H ext:C13 assignee:Marcelo_Felix"));
        assert!(md.contains("  Line one\n  \n  Line two"));
    }

    #[test]
    fn applies_frontmatter_defaults_to_tasks() {
        let json = md_to_task_json(
            "---\ntextwarrior: 1\ndefaults:\n  project: work\n  source: manual\n  status: review\n---\n\n- [ ] Fix bridge id:w-1\n",
        )
        .unwrap();

        assert!(json.contains("\"project\": \"work\""));
        assert!(json.contains("\"status\": \"pending\""));
        assert!(json.contains("\"description\": \"textwarrior source:manual\""));
        assert!(json.contains("\"description\": \"textwarrior status:review\""));
    }

    #[test]
    fn frontmatter_project_keeps_plus_words_as_tags() {
        let json = md_to_task_json(
            "---\ntextwarrior: 1\ndefaults:\n  project: work\n---\n\n- [ ] Fix bridge +task +repo:test @desk id:w-1\n",
        )
        .unwrap();

        assert!(json.contains("\"description\": \"Fix bridge\""));
        assert!(json.contains("\"project\": \"work\""));
        assert!(json.contains("\"task\""));
        assert!(json.contains("\"repo:test\""));
        assert!(json.contains("\"desk\""));
    }

    #[test]
    fn treats_status_done_as_completed() {
        let json =
            md_to_task_json("---\ndefaults:\n  status: done\n---\n\n- [ ] Pay invoice id:p-1\n")
                .unwrap();
        let txt =
            md_to_todo_txt("---\ndefaults:\n  status: done\n---\n\n- [ ] Pay invoice id:p-1\n")
                .unwrap();

        assert!(json.contains("\"status\": \"completed\""));
        assert_eq!(txt, "x Pay invoice id:p-1\n");
    }

    #[test]
    fn detects_auto_source_files_by_dash_suffix() {
        assert!(is_auto_source_file(Path::new("todo/work-todo.md")));
        assert!(is_auto_source_file(Path::new("todo/personal-todo.md")));
        assert!(!is_auto_source_file(Path::new("todo/inbox.md")));
        assert!(!is_auto_source_file(Path::new("todo/work-done-todo.md")));
        assert!(is_auto_source_file(Path::new("todo/done-work-todo.md")));
        assert!(is_done_source_file(Path::new("todo/done-work-todo.md")));
        assert!(!is_auto_source_file(Path::new("todo/work.todo.md")));
    }

    #[test]
    fn restores_textwarrior_annotations_as_fields() {
        let md = task_json_to_md(
            r#"[{"uuid":"550e8400-e29b-41d4-a716-446655440000","status":"pending","description":"Fix bridge","annotations":[{"description":"textwarrior id:p-001"},{"description":"textwarrior source:manual"},{"description":"textwarrior status:review"},{"description":"ordinary note"}]}]"#,
        )
        .unwrap();

        assert!(md.contains(" uuid:550e8400-e29b-41d4-a716-446655440000"));
        assert!(md.contains(" status:review"));
        assert!(md.contains(" id:p-001"));
        assert!(md.contains(" source:manual"));
        assert!(md.contains("\n  ordinary note\n"));
        assert!(!md.contains("textwarrior id:p-001"));
    }

    #[test]
    fn preserves_taskwarrior_annotation_entry_from_notes() {
        let json = md_to_task_json(
            "- [ ] Fix bridge uuid:550e8400-e29b-41d4-a716-446655440000\n\n  [20260705T172805Z] Original priority: 4 w\n",
        )
        .unwrap();

        assert!(json.contains("\"entry\": \"20260705T172805Z\""));
        assert!(json.contains("\"description\": \"Original priority: 4 w\""));
        assert!(!json.contains("[20260705T172805Z] Original priority"));
    }

    #[test]
    fn task_json_markdown_export_is_normalized_with_multiple_notes() {
        let md = task_json_to_md(
            r#"[{"uuid":"abc","status":"pending","description":"Fix bridge","annotations":[{"entry":"20260705T172805Z","description":"first note"},{"entry":"20260705T172806Z","description":"second note"}]}]"#,
        )
        .unwrap();

        assert_eq!(normalize_todo_md(&md).trim_end(), md.trim_end());
        assert!(md.contains("  [20260705T172805Z] first note\n  [20260705T172806Z] second note"));
    }

    #[test]
    fn frontmatter_defaults_compact_repeated_project_source_and_status() {
        let md = normalize_todo_md(
            "---\ntextwarrior: 1\ndefaults:\n  project: personal\n  source: personal.todo\n  status: not_started\nrender:\n  status: false\nid_prefix: p\n---\n\n- [ ] Fix bridge +personal id:personal-001 source:personal.todo status:not_started\n- [ ] Active task +personal id:p-002 source:personal.todo status:in_progress\n",
        );

        assert!(md.contains(
            "defaults:\n  project: personal\n  source: personal.todo\n  status: not_started"
        ));
        assert!(md.contains("- [ ] Fix bridge id:p-1"));
        assert!(md.contains("- [/] Active task id:p-2"));
        assert!(!md.contains("+personal"));
        assert!(!md.contains("source:personal.todo"));
        assert!(!md.contains("status:not_started"));
        assert!(!md.contains("status:in_progress"));
    }

    #[test]
    fn compact_uuid_rendering_round_trips_to_full_uuid() {
        let uuid = "d4452690-6b6e-44f3-b980-c226066aca34";
        let compact = encode_uuid_compact(uuid).unwrap();
        let md = normalize_todo_md(&format!(
            "---\ntextwarrior: 1\nrender:\n  uuid: compact\n---\n\n- [ ] Fix bridge uuid:{uuid} id:p-1\n"
        ));
        let json = md_to_task_json(&format!("- [ ] Fix bridge u:{compact} id:p-1\n")).unwrap();

        assert!(md.contains(&format!(" u:{compact} id:p-1")));
        assert!(!md.contains("uuid:d4452690"));
        assert_eq!(decode_uuid_compact(&compact).as_deref(), Some(uuid));
        assert!(json.contains("\"uuid\": \"d4452690-6b6e-44f3-b980-c226066aca34\""));
    }

    #[test]
    fn lints_invalid_compact_uuid() {
        let findings = lint_todo_md("- [ ] Fix bridge u:not-a-uuid\n");

        assert!(findings.contains(&LintFinding::error(
            Some(1),
            "u: should be a compact encoded UUID"
        )));
    }

    #[test]
    fn hidden_status_marker_round_trips_to_taskwarrior_annotation() {
        let json = md_to_task_json(
            "---\ndefaults:\n  project: personal\n  source: personal.todo\nrender:\n  status: false\n---\n\n- [/] Fix bridge id:p-001\n",
        )
        .unwrap();

        assert!(json.contains("\"project\": \"personal\""));
        assert!(json.contains("\"description\": \"textwarrior status:in_progress\""));
        assert!(json.contains("\"description\": \"textwarrior source:personal.todo\""));
    }

    #[test]
    fn accepts_md_detail_pointer_metadata() {
        let json = md_to_task_json("- [ ] Fix bridge id:p-001 md:p-001.md\n").unwrap();

        assert!(json.contains("\"description\": \"textwarrior md:p-001\""));
    }

    #[test]
    fn detects_conflict_before_overwriting_different_markdown() {
        let path = env::temp_dir().join(format!(
            "textwarrior-conflict-{}-{}.md",
            std::process::id(),
            "different"
        ));
        fs::write(&path, "- [ ] Keep local task\n  id: local-1\n").unwrap();

        let result = detect_write_conflict(&path, "- [ ] Imported task\n  id: tw-1\n", false);

        fs::remove_file(&path).unwrap();
        assert!(result.unwrap_err().contains("conflict:"));
    }

    #[test]
    fn allows_overwriting_normalize_only_markdown_differences() {
        let path = env::temp_dir().join(format!(
            "textwarrior-conflict-{}-{}.md",
            std::process::id(),
            "normalized"
        ));
        fs::write(&path, "- [ ] Fix bridge source:manual id:tw-1\n").unwrap();

        let result = detect_write_conflict(
            &path,
            "- [ ] Fix bridge\n  id: tw-1\n  source: manual\n",
            false,
        );

        fs::remove_file(&path).unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn builds_item_level_sync_state_from_documents() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "build"
        ));
        let path = todo_dir.join("personal-todo.md");
        let document = SyncDocument {
            name: "personal-todo".to_string(),
            path,
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n- [ ] Other task uuid:abc\n".to_string(),
        };

        let state = build_sync_state(&[document], &todo_dir).unwrap();

        assert_eq!(state.version, 1);
        assert_eq!(state.tasks["id:p-1"].path, "personal-todo.md");
        assert_eq!(state.tasks["uuid:abc"].path, "personal-todo.md");
        assert!(!state.tasks["id:p-1"].hash.is_empty());
    }

    #[test]
    fn sync_state_hash_changes_when_task_changes() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "hash"
        ));
        let path = todo_dir.join("personal-todo.md");
        let first = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let second = SyncDocument {
            name: "personal-todo".to_string(),
            path,
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1 due:2026-07-10\n".to_string(),
        };

        let first_state = build_sync_state(&[first], &todo_dir).unwrap();
        let second_state = build_sync_state(&[second], &todo_dir).unwrap();

        assert_ne!(
            first_state.tasks["id:p-1"].hash,
            second_state.tasks["id:p-1"].hash
        );
    }

    #[test]
    fn taskwarrior_identity_prefers_textwarrior_id_annotation() {
        let task = TaskwarriorTask {
            uuid: Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
            status: None,
            description: "Fix bridge".to_string(),
            entry: None,
            end: None,
            due: None,
            wait: None,
            scheduled: None,
            recur: None,
            priority: None,
            project: None,
            tags: Vec::new(),
            depends: Vec::new(),
            annotations: vec![TaskwarriorAnnotation {
                entry: None,
                description: "textwarrior id:p-1".to_string(),
            }],
        };

        assert_eq!(taskwarrior_task_identity(&task).as_deref(), Some("id:p-1"));
    }

    #[test]
    fn build_sync_state_rejects_duplicate_identity() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "duplicate"
        ));
        let first_path = todo_dir.join("personal-todo.md");
        let second_path = todo_dir.join("work-todo.md");
        let result = build_sync_state(
            &[
                SyncDocument {
                    name: "personal-todo".to_string(),
                    path: first_path,
                    export_txt: None,
                    text: "- [ ] Fix bridge id:p-1\n".to_string(),
                },
                SyncDocument {
                    name: "work-todo".to_string(),
                    path: second_path,
                    export_txt: None,
                    text: "- [ ] Duplicate bridge id:p-1\n".to_string(),
                },
            ],
            &todo_dir,
        );

        assert!(
            result
                .unwrap_err()
                .contains("duplicate task identity id:p-1")
        );
    }

    #[test]
    fn build_sync_state_rejects_duplicate_uuid_with_different_ids() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "duplicate-uuid"
        ));
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let result = build_sync_state(
            &[SyncDocument {
                name: "personal-todo".to_string(),
                path: todo_dir.join("personal-todo.md"),
                export_txt: None,
                text: format!("- [ ] First id:p-1 uuid:{uuid}\n- [ ] Second id:p-2 uuid:{uuid}\n"),
            }],
            &todo_dir,
        );

        assert!(result.unwrap_err().contains("duplicate task uuid"));
    }

    #[test]
    fn profile_sync_state_preserves_unselected_paths() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "profile-preserve"
        ));
        let mut previous_tasks = BTreeMap::new();
        previous_tasks.insert(
            "id:p-1".to_string(),
            SyncStateTask {
                path: "personal-todo.md".to_string(),
                hash: "old-personal".to_string(),
            },
        );
        previous_tasks.insert(
            "id:w-1".to_string(),
            SyncStateTask {
                path: "work-todo.md".to_string(),
                hash: "old-work".to_string(),
            },
        );
        let previous = SyncState {
            version: 1,
            tasks: previous_tasks,
        };
        let documents = vec![SyncDocument {
            name: "personal-todo".to_string(),
            path: todo_dir.join("personal-todo.md"),
            export_txt: None,
            text: "- [ ] Updated personal id:p-1\n".to_string(),
        }];
        let selected = build_sync_state(&documents, &todo_dir).unwrap();

        let merged = merge_partial_sync_state(&previous, selected, &documents, &todo_dir);

        assert_eq!(merged.tasks.len(), 2);
        assert_ne!(merged.tasks["id:p-1"].hash, "old-personal");
        assert_eq!(merged.tasks["id:w-1"].hash, "old-work");
        assert_eq!(merged.tasks["id:w-1"].path, "work-todo.md");
    }

    #[test]
    fn integration_sync_imports_exported_task_and_writes_state() {
        let root = unique_temp_dir("integration-sync");
        let todo_dir = root.join("tasks");
        fs::create_dir_all(&todo_dir).unwrap();
        let task_command = write_mock_task_command(
            &root,
            r#"[{"uuid":"550e8400-e29b-41d4-a716-446655440000","status":"pending","description":"Fix bridge","annotations":[{"description":"textwarrior id:p-1"},{"description":"textwarrior source:personal.todo"}]}]"#,
        );
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "todo_dir = \"{}\"\ntask_command = \"{}\"\n\n[[files]]\nname = \"personal\"\npath = \"personal-todo.md\"\nexport_txt = \"personal-todo.txt\"\n",
                todo_dir.display(),
                task_command.display()
            ),
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "textwarrior",
            "--config",
            config_path.to_str().unwrap(),
            "sync",
        ])
        .unwrap();

        run_cli(cli).unwrap();

        let md = fs::read_to_string(todo_dir.join("personal-todo.md")).unwrap();
        let txt = fs::read_to_string(todo_dir.join("personal-todo.txt")).unwrap();
        let imported = fs::read_to_string(root.join("task-imported.json")).unwrap();
        let state = load_sync_state(&todo_dir).unwrap();
        let args = fs::read_to_string(root.join("task-args.txt")).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert!(md.contains("Fix bridge"));
        assert!(md.contains("id:p-1"));
        assert!(txt.contains("Fix bridge"));
        assert!(imported.contains("\"description\": \"Fix bridge\""));
        assert!(state.tasks.contains_key("id:p-1"));
        assert!(args.lines().any(|line| line == "export"));
        assert!(args.lines().any(|line| line.starts_with("import ")));
    }

    #[test]
    fn integration_sync_splits_completed_tasks_to_done_file() {
        let root = unique_temp_dir("integration-done-split");
        let todo_dir = root.join("tasks");
        fs::create_dir_all(&todo_dir).unwrap();
        fs::write(
            todo_dir.join("personal-todo.md"),
            "---\ntextwarrior: 1\ndefaults:\n  project: personal\n  source: personal.todo\nid_prefix: p\n---\n",
        )
        .unwrap();
        let task_command = write_mock_task_command(
            &root,
            r#"[{"uuid":"550e8400-e29b-41d4-a716-446655440000","status":"pending","description":"Fix bridge","annotations":[{"description":"textwarrior id:p-1"},{"description":"textwarrior source:personal.todo"}]},{"uuid":"550e8400-e29b-41d4-a716-446655440001","status":"completed","description":"Done bridge","end":"20260706T000000Z","annotations":[{"description":"textwarrior id:p-2"},{"description":"textwarrior source:personal.todo"}]}]"#,
        );
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "todo_dir = \"{}\"\ntask_command = \"{}\"\n\n[[files]]\nname = \"personal\"\npath = \"personal-todo.md\"\nexport_txt = \"personal-todo.txt\"\n",
                todo_dir.display(),
                task_command.display()
            ),
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "textwarrior",
            "--config",
            config_path.to_str().unwrap(),
            "sync",
            "--force",
        ])
        .unwrap();

        run_cli(cli).unwrap();

        let active_md = fs::read_to_string(todo_dir.join("personal-todo.md")).unwrap();
        let done_md = fs::read_to_string(todo_dir.join("done-personal-todo.md")).unwrap();
        let state = load_sync_state(&todo_dir).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert!(active_md.contains("Fix bridge"));
        assert!(!active_md.contains("Done bridge"));
        assert!(done_md.contains("Done bridge"));
        assert!(
            done_md.contains("source: personal.todo") || done_md.contains("source:personal.todo")
        );
        assert_eq!(state.tasks["id:p-1"].path, "personal-todo.md");
        assert_eq!(state.tasks["id:p-2"].path, "done-personal-todo.md");
    }

    #[test]
    fn integration_profile_sync_preserves_unselected_file_state() {
        let root = unique_temp_dir("integration-profile-state");
        let todo_dir = root.join("tasks");
        fs::create_dir_all(&todo_dir).unwrap();
        let personal_path = todo_dir.join("personal-todo.md");
        let work_path = todo_dir.join("work-todo.md");
        fs::write(
            &personal_path,
            "- [ ] Old personal id:p-1 source:personal.todo\n",
        )
        .unwrap();
        fs::write(&work_path, "- [ ] Keep work id:w-1 source:work.todo\n").unwrap();
        let previous = build_sync_state(
            &[
                SyncDocument {
                    name: "personal-todo".to_string(),
                    path: personal_path.clone(),
                    export_txt: None,
                    text: fs::read_to_string(&personal_path).unwrap(),
                },
                SyncDocument {
                    name: "work-todo".to_string(),
                    path: work_path.clone(),
                    export_txt: None,
                    text: fs::read_to_string(&work_path).unwrap(),
                },
            ],
            &todo_dir,
        )
        .unwrap();
        save_sync_state(&todo_dir, &previous).unwrap();
        let task_command = write_mock_task_command(
            &root,
            r#"[{"status":"pending","description":"Updated personal","annotations":[{"description":"textwarrior id:p-1"},{"description":"textwarrior source:personal.todo"}]}]"#,
        );
        let config_path = root.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "todo_dir = \"{}\"\ntask_command = \"{}\"\n\n[[files]]\nname = \"personal\"\npath = \"personal-todo.md\"\nexport_txt = \"personal-todo.txt\"\n\n[[files]]\nname = \"work\"\npath = \"work-todo.md\"\nexport_txt = \"work-todo.txt\"\n\n[[profiles]]\nname = \"personal\"\nfiles = [\"personal\"]\ntask_filter = [\"+personal\"]\n",
                todo_dir.display(),
                task_command.display()
            ),
        )
        .unwrap();
        let cli = Cli::try_parse_from([
            "textwarrior",
            "--config",
            config_path.to_str().unwrap(),
            "sync",
            "--profile",
            "personal",
        ])
        .unwrap();

        run_cli(cli).unwrap();

        let next = load_sync_state(&todo_dir).unwrap();
        let personal = fs::read_to_string(&personal_path).unwrap();
        let work = fs::read_to_string(&work_path).unwrap();
        let args = fs::read_to_string(root.join("task-args.txt")).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert!(personal.contains("Updated personal"));
        assert!(work.contains("Keep work"));
        assert!(next.tasks.contains_key("id:p-1"));
        assert!(next.tasks.contains_key("id:w-1"));
        assert!(args.lines().any(|line| line == "+personal export"));
    }

    #[test]
    fn saves_and_loads_sync_state() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "save"
        ));
        let mut tasks = BTreeMap::new();
        tasks.insert(
            "id:p-1".to_string(),
            SyncStateTask {
                path: "personal-todo.md".to_string(),
                hash: "abc".to_string(),
            },
        );
        let state = SyncState { version: 1, tasks };

        save_sync_state(&todo_dir, &state).unwrap();
        let loaded = load_sync_state(&todo_dir).unwrap();

        fs::remove_dir_all(&todo_dir).unwrap();
        assert_eq!(loaded.tasks["id:p-1"].hash, "abc");
    }

    #[test]
    fn reset_sync_state_removes_only_state_file() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "reset"
        ));
        let state_path = sync_state_path(&todo_dir);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(&state_path, "{}\n").unwrap();
        let todo_path = todo_dir.join("personal-todo.md");
        fs::write(&todo_path, "- [ ] Keep task id:p-1\n").unwrap();

        reset_sync_state(&todo_dir, false).unwrap();

        let todo_exists = todo_path.exists();
        let state_exists = state_path.exists();
        fs::remove_dir_all(&todo_dir).unwrap();
        assert!(todo_exists);
        assert!(!state_exists);
    }

    #[test]
    fn reset_sync_state_dry_run_keeps_state_file() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "reset-dry-run"
        ));
        let state_path = sync_state_path(&todo_dir);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(&state_path, "{}\n").unwrap();

        reset_sync_state(&todo_dir, true).unwrap();

        let state_exists = state_path.exists();
        fs::remove_dir_all(&todo_dir).unwrap();
        assert!(state_exists);
    }

    #[test]
    fn reset_sync_state_plan_keeps_state_file() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-state-{}-{}",
            std::process::id(),
            "reset-plan"
        ));
        let state_path = sync_state_path(&todo_dir);
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(&state_path, "{}\n").unwrap();
        let config = AppConfig {
            todo_dir: Some(todo_dir.clone()),
            task_command: "task".to_string(),
            files: Vec::new(),
            profiles: Vec::new(),
        };

        sync_config(
            &config,
            SyncOptions {
                dry_run: false,
                profile: None,
                task_filter: &[],
                list_profiles: false,
                reset_state: true,
                plan: true,
                strategy: ConflictStrategy::Fail,
                force: false,
            },
        )
        .unwrap();

        let state_exists = state_path.exists();
        fs::remove_dir_all(&todo_dir).unwrap();
        assert!(state_exists);
    }

    #[test]
    fn detects_item_conflict_when_file_and_taskwarrior_changed() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "both"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (_, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n",
            "- [ ] Fix bridge remotely id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert_eq!(conflicts, vec!["id:p-1"]);
    }

    #[test]
    fn conflict_strategy_md_keeps_local_task() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "md"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n",
            "- [ ] Fix bridge remotely id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Md,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(merged.contains("Fix bridge locally"));
        assert!(!merged.contains("Fix bridge remotely"));
    }

    #[test]
    fn conflict_strategy_taskwarrior_keeps_remote_task() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "taskwarrior-strategy"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n",
            "- [ ] Fix bridge remotely id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Taskwarrior,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(merged.contains("Fix bridge remotely"));
        assert!(!merged.contains("Fix bridge locally"));
    }

    #[test]
    fn does_not_conflict_when_only_file_changed() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "file"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (_, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n",
            "- [ ] Fix bridge id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
    }

    #[test]
    fn merge_preserves_local_only_file_change() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "file"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n- [ ] New local task id:p-2\n",
            "- [ ] Fix bridge id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(merged.contains("Fix bridge locally"));
        assert!(merged.contains("New local task"));
    }

    #[test]
    fn merge_suppresses_old_file_when_task_moved_locally() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "move"
        ));
        fs::create_dir_all(&todo_dir).unwrap();
        let old_path = todo_dir.join("personal-todo.md");
        let new_path = todo_dir.join("work-todo.md");
        fs::write(&new_path, "- [ ] Fix bridge id:p-1\n").unwrap();
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: old_path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();
        let current_locations =
            md_task_locations_from_paths(&[old_path.clone(), new_path], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &old_path,
            "",
            "- [ ] Fix bridge id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &current_locations,
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        fs::remove_dir_all(&todo_dir).unwrap();
        assert!(conflicts.is_empty());
        assert!(!merged.contains("Fix bridge"));
    }

    #[test]
    fn merge_removes_unchanged_task_when_taskwarrior_moved_or_deleted_it() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "remote-move"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge id:p-1\n",
            "",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(!merged.contains("Fix bridge"));
    }

    #[test]
    fn filtered_profile_merge_preserves_tasks_absent_from_filtered_export() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "filtered-preserve"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge id:p-1\n",
            "",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                true,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(merged.contains("Fix bridge"));
    }

    #[test]
    fn filtered_profile_merge_still_removes_tasks_moved_in_filtered_export() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "filtered-move"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();
        let mut exported_identities = BTreeSet::new();
        exported_identities.insert("id:p-1".to_string());

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge id:p-1\n",
            "",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &exported_identities,
                true,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(!merged.contains("Fix bridge"));
    }

    #[test]
    fn taskwarrior_identity_tracks_filtered_moves_outside_selected_profile() {
        let task = TaskwarriorTask {
            uuid: None,
            status: Some("pending".to_string()),
            description: "Fix bridge".to_string(),
            entry: None,
            end: None,
            due: None,
            wait: None,
            scheduled: None,
            recur: None,
            priority: None,
            project: None,
            tags: Vec::new(),
            depends: Vec::new(),
            annotations: vec![TaskwarriorAnnotation {
                entry: None,
                description: "textwarrior id:p-1".to_string(),
            }],
        };
        let mut exported_identities = BTreeSet::new();
        if let Some(identity) = taskwarrior_task_identity(&task) {
            exported_identities.insert(identity);
        }

        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "filtered-cross-profile-move"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge id:p-1\n",
            "",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &exported_identities,
                true,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
        assert!(!merged.contains("Fix bridge"));
    }

    #[test]
    fn merge_conflicts_when_local_changed_and_taskwarrior_deleted_task() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-merge-state-{}-{}",
            std::process::id(),
            "delete-conflict"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (merged, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge locally id:p-1\n",
            "",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert_eq!(conflicts, vec!["id:p-1"]);
        assert!(merged.contains("Fix bridge locally"));
    }

    #[test]
    fn stale_tracked_paths_are_rewritten_for_taskwarrior_side_moves() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-stale-paths-{}-{}",
            std::process::id(),
            "remote-move"
        ));
        let old_path = todo_dir.join("personal-todo.md");
        let new_path = todo_dir.join("work-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: old_path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();
        let mut groups = BTreeMap::new();
        groups.insert(new_path.clone(), Vec::new());

        add_stale_tracked_paths(
            &mut groups,
            &[old_path.clone(), new_path.clone()],
            &previous,
            &todo_dir,
        );

        assert!(groups.contains_key(&old_path));
        assert!(groups.contains_key(&new_path));
    }

    #[test]
    fn does_not_conflict_when_only_taskwarrior_changed() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "taskwarrior"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (_, conflicts) = merge_todo_md_text(
            &path,
            "- [ ] Fix bridge id:p-1\n",
            "- [ ] Fix bridge remotely id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert!(conflicts.is_empty());
    }

    #[test]
    fn detects_conflict_when_local_delete_and_taskwarrior_changed() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-conflict-state-{}-{}",
            std::process::id(),
            "delete"
        ));
        let path = todo_dir.join("personal-todo.md");
        let previous_doc = SyncDocument {
            name: "personal-todo".to_string(),
            path: path.clone(),
            export_txt: None,
            text: "- [ ] Fix bridge id:p-1\n".to_string(),
        };
        let previous = build_sync_state(&[previous_doc], &todo_dir).unwrap();

        let (_, conflicts) = merge_todo_md_text(
            &path,
            "",
            "- [ ] Fix bridge remotely id:p-1\n",
            &test_merge_context(
                &previous,
                &todo_dir,
                &BTreeMap::new(),
                &BTreeSet::new(),
                false,
                ConflictStrategy::Fail,
            ),
        )
        .unwrap();

        assert_eq!(conflicts, vec!["id:p-1"]);
    }

    #[test]
    fn configured_export_txt_uses_custom_shadow_path() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-export-txt-{}-{}",
            std::process::id(),
            "custom"
        ));
        let task_path = todo_dir.join("personal-todo.md");
        let txt_path = todo_dir.join("out/personal.txt");
        let config = AppConfig {
            todo_dir: Some(todo_dir),
            task_command: "task".to_string(),
            profiles: Vec::new(),
            files: vec![ConfiguredFile {
                name: "personal".to_string(),
                path: task_path.clone(),
                export_txt: Some(txt_path.clone()),
            }],
        };

        assert_eq!(configured_export_txt(&config, &task_path), Some(txt_path));
    }

    #[test]
    fn configured_source_paths_can_select_profile_files() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-profile-{}-{}",
            std::process::id(),
            "select"
        ));
        let personal_path = todo_dir.join("personal-todo.md");
        let work_path = todo_dir.join("work-todo.md");
        let config = AppConfig {
            todo_dir: Some(todo_dir.clone()),
            task_command: "task".to_string(),
            profiles: vec![SyncProfile {
                name: "personal".to_string(),
                files: vec!["personal".to_string()],
                task_filter: Vec::new(),
            }],
            files: vec![
                ConfiguredFile {
                    name: "personal".to_string(),
                    path: personal_path.clone(),
                    export_txt: None,
                },
                ConfiguredFile {
                    name: "work".to_string(),
                    path: work_path,
                    export_txt: None,
                },
            ],
        };

        let paths = configured_source_paths(&config, &todo_dir, Some("personal")).unwrap();

        assert_eq!(paths, vec![personal_path]);
    }

    #[test]
    fn profile_task_filter_sets_taskwarrior_export_args() {
        let config = AppConfig {
            todo_dir: None,
            task_command: "task".to_string(),
            profiles: vec![SyncProfile {
                name: "personal".to_string(),
                files: vec!["personal".to_string()],
                task_filter: vec!["+personal".to_string(), "status:pending".to_string()],
            }],
            files: Vec::new(),
        };

        let args = task_export_filter_args(&config, Some("personal"), &[]).unwrap();

        assert_eq!(args, vec!["+personal", "status:pending"]);
    }

    #[test]
    fn ad_hoc_task_filter_appends_to_profile_filter() {
        let config = AppConfig {
            todo_dir: None,
            task_command: "task".to_string(),
            profiles: vec![SyncProfile {
                name: "personal".to_string(),
                files: vec!["personal".to_string()],
                task_filter: vec!["+personal".to_string()],
            }],
            files: Vec::new(),
        };

        let args =
            task_export_filter_args(&config, Some("personal"), &["status:pending".to_string()])
                .unwrap();

        assert_eq!(args, vec!["+personal", "status:pending"]);
    }

    #[test]
    fn default_sync_uses_unfiltered_taskwarrior_export() {
        let config = AppConfig {
            todo_dir: None,
            task_command: "task".to_string(),
            profiles: vec![SyncProfile {
                name: "personal".to_string(),
                files: vec!["personal".to_string()],
                task_filter: vec!["+personal".to_string()],
            }],
            files: Vec::new(),
        };

        let args = task_export_filter_args(&config, None, &[]).unwrap();

        assert!(args.is_empty());
    }

    #[test]
    fn configured_source_paths_rejects_unknown_profile() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-profile-{}-{}",
            std::process::id(),
            "unknown"
        ));
        let config = AppConfig {
            todo_dir: Some(todo_dir.clone()),
            task_command: "task".to_string(),
            profiles: Vec::new(),
            files: Vec::new(),
        };

        let error = configured_source_paths(&config, &todo_dir, Some("personal")).unwrap_err();

        assert!(error.contains("unknown sync profile personal"));
    }

    #[test]
    fn sync_identity_prefers_local_id_over_uuid() {
        let todo_dir = env::temp_dir().join(format!(
            "textwarrior-identity-{}-{}",
            std::process::id(),
            "id"
        ));
        let path = todo_dir.join("personal-todo.md");
        let state = build_sync_state(
            &[SyncDocument {
                name: "personal-todo".to_string(),
                path,
                export_txt: None,
                text: "- [ ] Fix bridge uuid:550e8400-e29b-41d4-a716-446655440000 id:p-1\n"
                    .to_string(),
            }],
            &todo_dir,
        )
        .unwrap();

        assert!(state.tasks.contains_key("id:p-1"));
        assert!(
            !state
                .tasks
                .contains_key("uuid:550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn sync_plan_counts_moved_tasks() {
        let mut current_tasks = BTreeMap::new();
        current_tasks.insert(
            "id:p-1".to_string(),
            SyncStateTask {
                path: "personal-todo.md".to_string(),
                hash: "abc".to_string(),
            },
        );
        let mut next_tasks = BTreeMap::new();
        next_tasks.insert(
            "id:p-1".to_string(),
            SyncStateTask {
                path: "work-todo.md".to_string(),
                hash: "abc".to_string(),
            },
        );

        let counts = sync_plan_counts(
            &SyncState {
                version: 1,
                tasks: current_tasks,
            },
            &SyncState {
                version: 1,
                tasks: next_tasks,
            },
        );

        assert_eq!(counts.moved_count, 1);
        assert_eq!(counts.unchanged_count, 0);
    }
}
