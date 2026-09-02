use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const DEFAULT_LAYOUTS: &str = include_str!("../defaults.toml");

#[derive(Debug, Parser)]
#[command(about = "Open declarative Herdr workspaces from private TOML")]
struct Cli {
    /// Optional TOML overrides for the built-in layouts.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Private target catalog. Defaults to ~/.config/mux/targets.toml.
    #[arg(long)]
    targets: Option<PathBuf>,
    /// Herdr binary to invoke when applying a layout.
    #[arg(long, default_value = "herdr")]
    herdr_bin: String,
    /// Output format for commands that display Herdr state.
    #[arg(long, visible_alias = "output", default_value = "text")]
    format: OutputFormat,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open or focus a target dashboard. `d` is an alias for `dash`.
    #[command(alias = "d")]
    Dash {
        /// Canonical short target, for example pus or deu. Omit for fzf.
        target: Option<String>,
        /// Replace the existing dashboard tab instead of focusing it.
        #[arg(long)]
        replace: bool,
        /// Print the Herdr command plan without changing the session.
        #[arg(long)]
        dry_run: bool,
    },
    /// Open a named TOML layout without a target catalog.
    Open {
        layout: String,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Validate built-in and user-defined layouts, and optional target catalogs.
    Check,
    /// Focus a workspace, or create one at the current directory.
    Attach { workspace: Option<String> },
    /// Create a tab in the focused workspace.
    Window { label: Option<String> },
    /// Print the live Herdr workspace inventory.
    List,
    /// Close a workspace by label.
    Close { workspace: String },
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Deserialize)]
struct Config {
    layouts: BTreeMap<String, Layout>,
}

#[derive(Debug, Deserialize)]
struct TargetsConfig {
    targets: BTreeMap<String, Target>,
}

#[derive(Debug, Deserialize)]
struct Target {
    #[serde(default)]
    profile: String,
    servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Layout {
    #[serde(default = "default_workspace")]
    workspace: String,
    #[serde(default = "default_tab")]
    tab: String,
    #[serde(default)]
    servers: usize,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    restart: Restart,
    #[serde(default = "default_retry_seconds")]
    retry_seconds: u64,
    #[serde(default)]
    tabs: Vec<String>,
    #[serde(default)]
    focus: Option<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    panes: Vec<Pane>,
}

#[derive(Debug, Deserialize)]
struct Pane {
    id: String,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    split: Option<Split>,
    #[serde(default)]
    ratio: Option<f64>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default = "default_persist")]
    persist: bool,
    #[serde(default)]
    environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Split {
    Right,
    Down,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Restart {
    Always,
    OnFailure,
    #[default]
    Never,
}

fn default_workspace() -> String {
    "{workspace}".into()
}
fn default_tab() -> String {
    "{layout}".into()
}
fn default_retry_seconds() -> u64 {
    5
}
fn default_persist() -> bool {
    true
}

fn main() -> Result<()> {
    run(Cli::parse())
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Commands::Dash {
            target,
            replace,
            dry_run,
        }) => {
            let config = load_config(cli.config.as_ref())?;
            let catalog = load_targets(cli.targets.as_ref())?;
            let target = match target {
                Some(target) => target,
                None => match select_target(&catalog)? {
                    Some(target) => target,
                    None => return Ok(()),
                },
            };
            let (name, selected) = resolve_target(&catalog, &target)?;
            let profile = if selected.profile.is_empty() {
                format!("dash{}", selected.servers.len() * 2)
            } else {
                selected.profile.clone()
            };
            let layout = config
                .layouts
                .get(&profile)
                .with_context(|| format!("layout '{profile}' not found"))?;
            if selected.servers.len() < layout.servers {
                bail!(
                    "target '{name}' defines {} servers but layout '{}' requires {}",
                    selected.servers.len(),
                    profile,
                    layout.servers
                );
            }
            let mut variables = BTreeMap::from([("target".into(), name)]);
            variables.insert("layout".into(), profile.clone());
            let target = variables["target"].clone();
            let (service, environment) = target_parts(&target)?;
            variables.insert("service".into(), service.into());
            variables.insert("env".into(), environment);
            for (index, server) in selected.servers.iter().enumerate() {
                variables.insert(format!("server_{}", index + 1), server.clone());
            }
            run_layout(layout, &variables, &cli.herdr_bin, replace, dry_run)?;
        }
        Some(Commands::Open {
            layout,
            replace,
            dry_run,
        }) => {
            let config = load_config(cli.config.as_ref())?;
            open_named_layout(&config, &layout, &cli.herdr_bin, replace, dry_run)?;
        }
        Some(Commands::Check) => check_config(cli.config.as_ref(), cli.targets.as_ref())?,
        Some(Commands::Attach { workspace }) => {
            attach_workspace(&cli.herdr_bin, workspace.as_deref())?
        }
        Some(Commands::Window { label }) => create_window(&cli.herdr_bin, label.as_deref())?,
        Some(Commands::List) => list_workspaces(&cli.herdr_bin, cli.format)?,
        Some(Commands::Close { workspace }) => close_workspace(&cli.herdr_bin, &workspace)?,
        None if matches!(cli.format, OutputFormat::Json) => {
            print_herdr(&cli.herdr_bin, std::iter::empty::<&str>())?
        }
        None => attach_workspace(&cli.herdr_bin, None)?,
    }
    Ok(())
}

fn load_targets(path: Option<&PathBuf>) -> Result<TargetsConfig> {
    let path = path
        .cloned()
        .unwrap_or_else(|| home_dir().join(".config/mux/targets.toml"));
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).context("parse mux targets TOML")
}

fn resolve_target<'a>(catalog: &'a TargetsConfig, input: &str) -> Result<(String, &'a Target)> {
    if let Some(target) = catalog.targets.get(input) {
        return Ok((input.to_string(), target));
    }
    bail!("unknown target '{input}'; run `mux d` to select one")
}

fn select_target(catalog: &TargetsConfig) -> Result<Option<String>> {
    let mut child = Command::new("fzf")
        .args(["--prompt", "mux d> ", "--height", "40%"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("start fzf for mux targets")?;
    child
        .stdin
        .take()
        .context("fzf stdin unavailable")?
        .write_all(
            catalog
                .targets
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
                .as_bytes(),
        )?;
    let output = child.wait_with_output().context("wait for fzf")?;
    if output.status.code() == Some(130) {
        return Ok(None);
    }
    if !output.status.success() {
        bail!("fzf target selection failed with {}", output.status);
    }
    let target = String::from_utf8(output.stdout)
        .context("decode fzf target")?
        .trim()
        .to_string();
    (!target.is_empty())
        .then_some(target)
        .map_or(Ok(None), |target| Ok(Some(target)))
}

fn target_parts(target: &str) -> Result<(&str, String)> {
    let (environment, rest) = target
        .strip_prefix('p')
        .map(|rest| ("prod", rest))
        .or_else(|| target.strip_prefix('d').map(|rest| ("dev", rest)))
        .context("target must start with p (prod) or d (dev)")?;
    if rest.is_empty() {
        bail!("target must include a service after its p/d prefix");
    }
    if let Some((service, suffix)) = rest.rsplit_once('-') {
        return Ok((service, format!("{environment}-{suffix}")));
    }
    Ok((rest, environment.into()))
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn run_layout(
    layout: &Layout,
    vars: &BTreeMap<String, String>,
    herdr: &str,
    replace: bool,
    dry_run: bool,
) -> Result<()> {
    validate(layout)?;
    validate_cwds(layout, vars)?;
    if dry_run {
        for line in compile_herdr(layout, vars)? {
            println!("{line}");
        }
    } else {
        apply_herdr(layout, vars, herdr, replace)?;
    }
    Ok(())
}

fn open_named_layout(
    config: &Config,
    name: &str,
    herdr: &str,
    replace: bool,
    dry_run: bool,
) -> Result<()> {
    let root = config
        .layouts
        .get(name)
        .with_context(|| format!("layout '{name}' not found"))?;
    let workspace = expand(
        &root.workspace,
        &BTreeMap::from([
            ("workspace".into(), name.into()),
            ("layout".into(), name.into()),
        ]),
    )?;
    if root.tabs.is_empty() {
        return run_layout(
            root,
            &BTreeMap::from([
                ("workspace".into(), workspace),
                ("layout".into(), name.into()),
            ]),
            herdr,
            replace,
            dry_run,
        );
    }
    let mut seen = BTreeSet::from([name.to_string()]);
    for (index, tab) in root.tabs.iter().enumerate() {
        if !seen.insert(tab.clone()) {
            bail!("layout bundle '{name}' repeats tab '{tab}'");
        }
        let layout = config
            .layouts
            .get(tab)
            .with_context(|| format!("layout bundle '{name}' references missing layout '{tab}'"))?;
        if !layout.tabs.is_empty() {
            bail!("layout bundle '{name}' cannot nest bundle '{tab}'");
        }
        let vars = BTreeMap::from([
            ("workspace".into(), workspace.clone()),
            ("layout".into(), tab.clone()),
        ]);
        if dry_run {
            validate(layout)?;
            validate_cwds(layout, &vars)?;
            let mut plan = compile_herdr(layout, &vars)?;
            if index > 0 {
                plan[0] = format!(
                    "herdr tab create --workspace {} --label {} --focus",
                    quote(&workspace),
                    quote(&expand(&layout.tab, &vars)?),
                );
            }
            for line in plan {
                println!("{line}");
            }
        } else {
            run_layout(layout, &vars, herdr, replace, false)?;
        }
    }
    if !dry_run {
        if let Some(focus) = &root.focus {
            let id = workspace_id(herdr, &workspace)?;
            let tabs = herdr_json(herdr, ["tab", "list", "--workspace", &id])?;
            let tab = tabs["result"]["tabs"]
                .as_array()
                .and_then(|tabs| tabs.iter().find(|tab| tab["label"] == *focus))
                .and_then(|tab| tab["tab_id"].as_str())
                .with_context(|| format!("bundle '{name}' focus tab '{focus}' not found"))?;
            herdr_json(herdr, ["tab", "focus", tab])?;
        }
        println!("Opened layout bundle '{name}'.");
    }
    Ok(())
}

fn check_config(config_path: Option<&PathBuf>, targets_path: Option<&PathBuf>) -> Result<()> {
    let config = load_config(config_path)?;
    for (name, layout) in &config.layouts {
        validate(layout).with_context(|| format!("layout '{name}'"))?;
        validate_cwds(layout, &BTreeMap::from([("layout".into(), name.clone())]))
            .with_context(|| format!("layout '{name}'"))?;
        for tab in &layout.tabs {
            let referenced = config.layouts.get(tab).with_context(|| {
                format!("layout '{name}' references missing tab layout '{tab}'")
            })?;
            if !referenced.tabs.is_empty() {
                bail!("layout '{name}' nests bundle '{tab}'");
            }
        }
    }
    let path = targets_path
        .cloned()
        .unwrap_or_else(|| home_dir().join(".config/mux/targets.toml"));
    if path.exists() {
        let targets = load_targets(Some(&path))?;
        for (name, target) in targets.targets {
            let profile = if target.profile.is_empty() {
                format!("dash{}", target.servers.len() * 2)
            } else {
                target.profile
            };
            let layout = config
                .layouts
                .get(&profile)
                .with_context(|| format!("target '{name}' selects missing layout '{profile}'"))?;
            if target.servers.len() < layout.servers {
                bail!("target '{name}' has too few servers for layout '{profile}'");
            }
        }
    }
    println!("mux configuration is valid.");
    Ok(())
}

fn validate_cwds(layout: &Layout, vars: &BTreeMap<String, String>) -> Result<()> {
    for (scope, cwd) in std::iter::once(("layout", layout.cwd.as_ref())).chain(
        layout
            .panes
            .iter()
            .map(|pane| (pane.id.as_str(), pane.cwd.as_ref())),
    ) {
        if let Some(cwd) = cwd {
            let cwd = expand(cwd, vars)?;
            let cwd = cwd
                .strip_prefix("~/")
                .map_or_else(|| PathBuf::from(&cwd), |path| home_dir().join(path));
            if !cwd.is_dir() {
                bail!("{scope} cwd '{}' is not a directory", cwd.display());
            }
        }
    }
    Ok(())
}

fn resolved_cwd(value: Option<&String>, vars: &BTreeMap<String, String>) -> Result<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    let value = expand(value, vars)?;
    let path = value
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(&value), |path| home_dir().join(path));
    Ok(Some(path.display().to_string()))
}

fn create_workspace(herdr: &str, label: &str, cwd: Option<&str>) -> Result<serde_json::Value> {
    let mut args = vec![
        "workspace".to_string(),
        "create".into(),
        "--label".into(),
        label.into(),
    ];
    if let Some(cwd) = cwd {
        args.extend(["--cwd".into(), cwd.into()]);
    }
    args.push("--focus".into());
    herdr_json(herdr, args)
}

fn create_tab(
    herdr: &str,
    workspace: &str,
    label: &str,
    cwd: Option<&str>,
) -> Result<serde_json::Value> {
    let mut args = vec![
        "tab".to_string(),
        "create".into(),
        "--workspace".into(),
        workspace.into(),
        "--label".into(),
        label.into(),
    ];
    if let Some(cwd) = cwd {
        args.extend(["--cwd".into(), cwd.into()]);
    }
    args.push("--focus".into());
    herdr_json(herdr, args)
}

fn apply_herdr(
    layout: &Layout,
    vars: &BTreeMap<String, String>,
    herdr: &str,
    replace: bool,
) -> Result<()> {
    let workspace_label = expand(&layout.workspace, vars)?;
    let tab_label = expand(&layout.tab, vars)?;
    let root_cwd = resolved_cwd(
        layout
            .panes
            .first()
            .and_then(|pane| pane.cwd.as_ref())
            .or(layout.cwd.as_ref()),
        vars,
    )?;
    let workspaces = herdr_json(herdr, ["workspace", "list"])?;
    let workspace_id = workspaces["result"]["workspaces"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["label"] == workspace_label))
        .and_then(|item| item["workspace_id"].as_str())
        .map(ToOwned::to_owned);
    let created = if let Some(workspace_id) = workspace_id {
        let tabs = herdr_json(herdr, ["tab", "list", "--workspace", &workspace_id])?;
        let tab_items = tabs["result"]["tabs"]
            .as_array()
            .context("Herdr did not return tabs")?;
        let tab_id = tab_items
            .iter()
            .find(|item| item["label"] == tab_label)
            .and_then(|item| item["tab_id"].as_str())
            .map(ToOwned::to_owned);
        if let Some(tab_id) = tab_id {
            if !replace {
                herdr_json(herdr, ["tab", "focus", &tab_id])?;
                println!("Focused {workspace_label}/{tab_label}.");
                return Ok(());
            }
            if tab_items.len() == 1 {
                herdr_json(herdr, ["workspace", "close", &workspace_id])?;
                create_workspace(herdr, &workspace_label, root_cwd.as_deref())?
            } else {
                herdr_json(herdr, ["tab", "close", &tab_id])?;
                create_tab(herdr, &workspace_id, &tab_label, root_cwd.as_deref())?
            }
        } else {
            create_tab(herdr, &workspace_id, &tab_label, root_cwd.as_deref())?
        }
    } else {
        create_workspace(herdr, &workspace_label, root_cwd.as_deref())?
    };
    let tab = created["result"]["tab"]["tab_id"]
        .as_str()
        .context("Herdr did not return a tab id")?
        .to_string();
    let root = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .context("Herdr did not return a root pane id")?
        .to_string();
    herdr_json(herdr, ["tab", "rename", &tab, &tab_label])?;
    let mut panes: BTreeMap<String, String> = BTreeMap::new();
    for (index, pane) in layout.panes.iter().enumerate() {
        let pane_id = if index == 0 {
            root.clone()
        } else {
            let from = panes
                .get(pane.from.as_ref().unwrap())
                .context("missing resolved pane anchor")?;
            let direction = match pane.split.unwrap() {
                Split::Right => "right",
                Split::Down => "down",
            };
            let ratio = format!("{:.6}", pane.ratio.unwrap_or(0.5));
            let mut args = vec![
                "pane".to_string(),
                "split".into(),
                from.clone(),
                "--direction".into(),
                direction.into(),
                "--ratio".into(),
                ratio,
                "--no-focus".into(),
            ];
            if let Some(cwd) = resolved_cwd(pane.cwd.as_ref(), vars)? {
                args.extend(["--cwd".into(), cwd]);
            }
            herdr_json(herdr, args)?["result"]["pane"]["pane_id"]
                .as_str()
                .context("Herdr did not return a split pane id")?
                .to_string()
        };
        herdr_json(
            herdr,
            [
                "pane",
                "rename",
                &pane_id,
                &expand(pane.label.as_deref().unwrap_or(&pane.id), vars)?,
            ],
        )?;
        if let Some(command) = &pane.command {
            herdr_ok(
                herdr,
                [
                    "pane",
                    "run",
                    &pane_id,
                    &pane_command(layout, pane, command, vars)?,
                ],
            )?;
        }
        panes.insert(pane.id.clone(), pane_id);
    }
    println!("Opened {workspace_label}/{tab_label}.");
    Ok(())
}

fn workspace_id(herdr: &str, label: &str) -> Result<String> {
    herdr_json(herdr, ["workspace", "list"])?["result"]["workspaces"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["label"] == label))
        .and_then(|item| item["workspace_id"].as_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("workspace '{label}' not found"))
}

fn attach_workspace(herdr: &str, label: Option<&str>) -> Result<()> {
    let label = label.map(ToOwned::to_owned).unwrap_or(default_label()?);
    match workspace_id(herdr, &label) {
        Ok(id) => {
            herdr_json(herdr, ["workspace", "focus", &id])?;
        }
        Err(_) => {
            let cwd = env::current_dir()?.display().to_string();
            herdr_json(
                herdr,
                [
                    "workspace",
                    "create",
                    "--label",
                    &label,
                    "--cwd",
                    &cwd,
                    "--focus",
                ],
            )?;
        }
    }
    Ok(())
}

fn create_window(herdr: &str, label: Option<&str>) -> Result<()> {
    let workspaces = herdr_json(herdr, ["workspace", "list"])?;
    let workspace_id = workspaces["result"]["workspaces"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["focused"] == true))
        .and_then(|item| item["workspace_id"].as_str())
        .context("no focused Herdr workspace")?;
    let label = label
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "shell".into());
    let cwd = env::current_dir()?.display().to_string();
    herdr_json(
        herdr,
        [
            "tab",
            "create",
            "--workspace",
            workspace_id,
            "--label",
            &label,
            "--cwd",
            &cwd,
            "--focus",
        ],
    )?;
    Ok(())
}

fn list_workspaces(herdr: &str, format: OutputFormat) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return print_herdr(herdr, ["workspace", "list"]);
    }
    let workspaces = herdr_json(herdr, ["workspace", "list"])?;
    let items = workspaces["result"]["workspaces"]
        .as_array()
        .context("Herdr did not return workspaces")?;
    for workspace in items {
        let label = workspace["label"].as_str().unwrap_or("<unnamed>");
        println!(
            "{} {}",
            if workspace["focused"] == true {
                "*"
            } else {
                " "
            },
            label
        );
    }
    Ok(())
}

fn default_label() -> Result<String> {
    env::current_dir()?
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .context("current directory has no usable name")
}

fn close_workspace(herdr: &str, label: &str) -> Result<()> {
    let id = workspace_id(herdr, label)?;
    herdr_json(herdr, ["workspace", "close", &id])?;
    Ok(())
}

fn print_herdr<I, S>(herdr: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new(herdr)
        .args(args)
        .status()
        .with_context(|| format!("run {herdr}"))?;
    if !status.success() {
        bail!("{herdr} failed with {status}");
    }
    Ok(())
}

fn herdr_json<I, S>(herdr: &str, args: I) -> Result<serde_json::Value>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new(herdr)
        .args(args)
        .output()
        .with_context(|| format!("run {herdr}"))?;
    if !output.status.success() {
        bail!(
            "{herdr} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("parse Herdr JSON response")
}

fn herdr_ok<I, S>(herdr: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new(herdr)
        .args(args)
        .status()
        .with_context(|| format!("run {herdr}"))?;
    if !status.success() {
        bail!("{herdr} failed with {status}");
    }
    Ok(())
}

fn load_config(path: Option<&PathBuf>) -> Result<Config> {
    let mut config =
        toml::from_str::<toml::Value>(DEFAULT_LAYOUTS).context("parse built-in mux layouts")?;
    let path = path
        .cloned()
        .unwrap_or_else(|| home_dir().join(".config/mux/layouts.toml"));
    match fs::read_to_string(&path) {
        Ok(text) => {
            let overrides = toml::from_str::<toml::Value>(&text)
                .with_context(|| format!("parse mux TOML {}", path.display()))?;
            merge_toml(&mut config, overrides);
        }
        Err(error)
            if path.as_path() == home_dir().join(".config/mux/layouts.toml")
                && error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    }
    config.try_into().context("decode mux layouts")
}

fn merge_toml(base: &mut toml::Value, overrides: toml::Value) {
    match (base, overrides) {
        (toml::Value::Table(base), toml::Value::Table(overrides)) => {
            for (key, value) in overrides {
                if let Some(current) = base.get_mut(&key) {
                    merge_toml(current, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overrides) => *base = overrides,
    }
}

fn validate(layout: &Layout) -> Result<()> {
    if !matches!(layout.restart, Restart::Never) && layout.retry_seconds == 0 {
        bail!("retry_seconds must be greater than zero when restart is enabled");
    }
    let mut ids = BTreeSet::new();
    for (index, pane) in layout.panes.iter().enumerate() {
        if !ids.insert(pane.id.clone()) {
            bail!("duplicate pane id '{}'", pane.id);
        }
        if index == 0 {
            if pane.from.is_some() || pane.split.is_some() {
                bail!("first pane '{}' must be the root", pane.id);
            }
        } else {
            let from = pane
                .from
                .as_ref()
                .context("non-root panes require 'from'")?;
            if !ids.contains(from.as_str()) {
                bail!(
                    "pane '{}' references unknown or later pane '{from}'",
                    pane.id
                );
            }
            if pane.split.is_none() {
                bail!("pane '{}' requires 'split'", pane.id);
            }
        }
        if let Some(ratio) = pane.ratio
            && !(0.0 < ratio && ratio < 1.0)
        {
            bail!("pane '{}' ratio must be between 0 and 1", pane.id);
        }
    }
    if layout.panes.is_empty() == layout.tabs.is_empty() {
        bail!("layout must contain panes or tab references, but not both");
    }
    Ok(())
}

fn compile_herdr(layout: &Layout, vars: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let mut out = vec![
        format!(
            "herdr workspace create --label {} --focus",
            quote(&expand(&layout.workspace, vars)?)
        ),
        format!(
            "herdr tab rename ${{TAB}} {}",
            quote(&expand(&layout.tab, vars)?)
        ),
    ];
    for (index, pane) in layout.panes.iter().enumerate() {
        let handle = format!(
            "${{PANE_{}}}",
            pane.id.to_ascii_uppercase().replace('-', "_")
        );
        if index == 0 {
            out.push(format!("{handle}=${{ROOT_PANE}}"));
        } else {
            let from = pane
                .from
                .as_ref()
                .unwrap()
                .to_ascii_uppercase()
                .replace('-', "_");
            let direction = match pane.split.unwrap() {
                Split::Right => "right",
                Split::Down => "down",
            };
            let ratio = pane.ratio.unwrap_or(0.5);
            out.push(format!("{handle}=$(herdr pane split ${{PANE_{from}}} --direction {direction} --ratio {ratio:.6} --no-focus)"));
        }
        let pane_ref = handle;
        out.push(format!(
            "herdr pane rename {pane_ref} {}",
            quote(&expand(pane.label.as_deref().unwrap_or(&pane.id), vars)?)
        ));
        if let Some(command) = &pane.command {
            out.push(format!(
                "herdr pane run {pane_ref} {}",
                quote(&pane_command(layout, pane, command, vars)?)
            ));
        }
    }
    Ok(out)
}

fn pane_command(
    layout: &Layout,
    pane: &Pane,
    command: &str,
    vars: &BTreeMap<String, String>,
) -> Result<String> {
    let command = expand(command, vars)?;
    let environment = layout
        .environment
        .iter()
        .chain(pane.environment.iter())
        .map(|(name, value)| {
            if !name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_'
                    || byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphabetic())
            }) {
                bail!("invalid environment variable name '{name}'");
            }
            Ok(format!("{name}={}", quote(&expand(value, vars)?)))
        })
        .collect::<Result<Vec<_>>>()?;
    let command = if environment.is_empty() {
        command
    } else {
        format!("{} {command}", environment.join(" "))
    };
    match layout.restart {
        Restart::Always => Ok(format!(
            "while true; do {command}; rc=$?; printf '\\n[mux] command exited with status %s; retrying in {}s.\\n' \"$rc\"; sleep {}; done",
            layout.retry_seconds, layout.retry_seconds
        )),
        Restart::OnFailure => Ok(format!(
            "until {command}; do rc=$?; printf '\\n[mux] command exited with status %s; retrying in {}s.\\n' \"$rc\"; sleep {}; done{}",
            layout.retry_seconds,
            layout.retry_seconds,
            if pane.persist {
                "; exec \"${SHELL:-/bin/zsh}\" -l"
            } else {
                ""
            },
        )),
        Restart::Never if pane.persist => Ok(format!(
            "{command}; rc=$?; printf '\\n[mux] command exited with status %s; shell kept open for inspection.\\n' \"$rc\"; exec \"${{SHELL:-/bin/zsh}}\" -l"
        )),
        Restart::Never => Ok(command),
    }
}

fn expand(value: &str, vars: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::new();
    let mut remaining = value;
    while let Some(start) = remaining.find('{') {
        output.push_str(&remaining[..start]);
        let rest = &remaining[start + 1..];
        let end = rest.find('}').context("unclosed variable")?;
        let name = &rest[..end];
        if let Some(replacement) = vars.get(name) {
            output.push_str(replacement);
        } else if name == "target" || name.starts_with("server_") {
            bail!("missing variable '{name}'");
        } else {
            output.push('{');
            output.push_str(name);
            output.push('}');
        }
        remaining = &rest[end + 1..];
    }
    output.push_str(remaining);
    Ok(output)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fake_herdr() -> (PathBuf, PathBuf, PathBuf) {
        let directory = env::temp_dir().join(format!(
            "mux-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("herdr");
        let log = directory.join("calls");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\ncase \"$1 $2\" in\n  'workspace list') printf '{{\\\"result\\\":{{\\\"workspaces\\\":[]}}}}' ;;\n  *) printf '{{\\\"result\\\":{{}}}}' ;;\nesac\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (directory, script, log)
    }

    fn dash8() -> Layout {
        toml::from_str::<Config>(DEFAULT_LAYOUTS)
            .unwrap()
            .layouts
            .remove("dash8")
            .unwrap()
    }
    fn vars() -> BTreeMap<String, String> {
        [
            ("target", "us-prod"),
            ("server_1", "one"),
            ("server_2", "two"),
            ("server_3", "three"),
            ("server_4", "four"),
            ("service", "us"),
            ("env", "prod"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    #[test]
    fn compiles_dash8_with_anchored_equal_splits() {
        let plan = compile_herdr(&dash8(), &vars()).unwrap().join("\n");
        assert!(plan.contains(
            "${PANE_TOP_2}=$(herdr pane split ${PANE_TOP_1} --direction right --ratio 0.250000"
        ));
        assert!(plan.contains("${PANE_BOTTOM_4}=$(herdr pane split ${PANE_BOTTOM_3} --direction right --ratio 0.500000"));
        assert!(plan.contains("ssh -- four"));
        assert!(plan.contains("retrying in 5s"));
    }
    #[test]
    fn only_canonical_short_target_names_resolve() {
        let catalog = TargetsConfig {
            targets: BTreeMap::from([(
                "pus".into(),
                Target {
                    profile: String::new(),
                    servers: vec![],
                },
            )]),
        };
        assert_eq!(resolve_target(&catalog, "pus").unwrap().0, "pus");
        assert!(resolve_target(&catalog, "us-prod").is_err());
    }
    #[test]
    fn derives_dash_environment_from_short_target_names() {
        assert_eq!(target_parts("pus").unwrap(), ("us", "prod".into()));
        assert_eq!(target_parts("dus").unwrap(), ("us", "dev".into()));
        assert_eq!(target_parts("pus-1").unwrap(), ("us", "prod-1".into()));
    }
    #[test]
    fn built_in_layouts_cover_single_and_four_server_targets() {
        let config = toml::from_str::<Config>(DEFAULT_LAYOUTS).unwrap();
        for (servers, profile) in [(1, "dash2"), (4, "dash8")] {
            assert_eq!(format!("dash{}", servers * 2), profile);
            assert_eq!(config.layouts[profile].servers, servers);
        }
    }
    #[test]
    fn preserves_shell_parameter_expansions() {
        assert_eq!(
            expand("exec \"${SHELL:-/bin/zsh}\" -l", &vars()).unwrap(),
            "exec \"${SHELL:-/bin/zsh}\" -l"
        );
    }
    #[test]
    fn bare_mux_creates_or_focuses_a_workspace_instead_of_printing_herdr_json() {
        let (directory, herdr, log) = fake_herdr();
        let cli = Cli {
            config: None,
            targets: None,
            herdr_bin: herdr.display().to_string(),
            format: OutputFormat::Text,
            command: None,
        };

        run(cli).unwrap();

        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.lines().any(|line| line == "workspace list"));
        assert!(
            calls
                .lines()
                .any(|line| line.starts_with("workspace create --label "))
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn adds_tmux_dash_environment_and_reconnects() {
        let layout = Layout {
            workspace: "dash".into(),
            tab: "{target}".into(),
            servers: 4,
            cwd: None,
            restart: Restart::Always,
            retry_seconds: 5,
            tabs: vec![],
            focus: None,
            environment: BTreeMap::from([
                ("LC_LAYOUT".into(), "dash".into()),
                ("LC_SERVICE".into(), "{service}".into()),
            ]),
            panes: vec![],
        };
        let pane = Pane {
            id: "top-2".into(),
            from: None,
            split: None,
            ratio: None,
            label: None,
            command: None,
            cwd: None,
            persist: true,
            environment: BTreeMap::from([("LC_PANE_ROW".into(), "1".into())]),
        };
        let command = pane_command(&layout, &pane, "ssh -- {server_2}", &vars()).unwrap();
        assert!(command.contains("LC_LAYOUT='dash'"));
        assert!(command.contains("LC_SERVICE='us'"));
        assert!(command.contains("LC_PANE_ROW='1'"));
        assert!(!command.contains("LC_VFTERM24_"));
        assert!(command.contains("retrying in 5s"));
    }
    #[test]
    fn merges_a_small_layout_override_over_defaults() {
        let mut defaults = toml::from_str::<toml::Value>(DEFAULT_LAYOUTS).unwrap();
        let override_value = toml::from_str("[layouts.dash8]\nretry_seconds = 10\n").unwrap();
        merge_toml(&mut defaults, override_value);
        let config: Config = defaults.try_into().unwrap();
        let dash8 = config.layouts.get("dash8").unwrap();
        assert_eq!(dash8.retry_seconds, 10);
        assert_eq!(dash8.panes.len(), 8);
        assert_eq!(dash8.environment["LC_SERVICE"], "{service}");
    }
    #[test]
    fn rejects_later_anchor() {
        let mut layout = dash8();
        layout.panes[1].from = Some("top-4".into());
        assert!(
            validate(&layout)
                .unwrap_err()
                .to_string()
                .contains("unknown or later")
        );
    }
}
