# mux

`mux` opens explicit TOML pane trees in Herdr. It is intentionally not a tmuxp
parser: TOML is the maintained format and can represent named pane anchors.

```sh
cargo run -p mux -- d                 # fuzzy-select a short target
cargo run -p mux -- d pus --dry-run
cargo run -p mux -- d pus --replace
cargo run -p mux -- open dev --dry-run
cargo run -p mux -- check
cargo run -p mux -- attach
cargo run -p mux -- window logs
cargo run -p mux -- --format json list
```

The private `~/.config/mux/targets.toml` catalog maps **short** targets to SSH
aliases: `p` means production and `d` means development (`pus`, `deu`, `dus`).
`mux d` passes only those canonical names to fzf; long aliases such as `us-dev`
are intentionally unsupported.
Dash6 and dash8 pane trees are built in, so `~/.config/mux/layouts.toml` is
optional and is merged over those defaults. `dash` creates the tab on first use
and focuses it afterwards; `--replace` recreates just that tab.

For standard dashboards, a target needs only its servers: one selects dash2,
three selects dash6, and four selects dash8. Set `profile = "custom"` only to
select a custom layout.
`--dry-run` prints the deterministic Herdr command plan instead of mutating the
current session. Bare `mux` attaches to Herdr. `attach` focuses or creates a
workspace for the current directory, and `window [label]` creates a tab in the
focused workspace. `list` is human-readable by default; use `--format json`
(or `--output json`) for Herdr's JSON response. Bare `mux --format json`
likewise requests Herdr's raw JSON instead of attaching.

Dash layouts declare `restart = "always"` plus the same SSH environment used by
the tmuxp dashboards: `LC_LAYOUT=dash`, `LC_SERVICE={service}`, and
`LC_PANE_ROW=1` for the top row. A reconnecting pane retries after the
configured interval. All of this is configuration, not mux-specific code:

```toml
[layouts.dash8]
workspace = "dash"
tab = "{target}"
servers = 4
restart = "always" # always | onfailure | never
retry_seconds = 5

[layouts.dash8.environment]
LC_LAYOUT = "dash"
LC_SERVICE = "{service}"

[[layouts.dash8.panes]]
id = "top-1"
environment = { LC_PANE_ROW = "1" }
command = "ssh -- {server_1}"
```

Layouts can use `{target}`, `{service}`, `{env}`, and `{server_1}` through
`{server_N}` in tab labels, environment values, pane labels, and commands.
Pane environment overrides layout environment. A minimal override is enough:

```toml
[layouts.dash8]
reconnect_seconds = 10
```

Tables merge recursively; an overridden `panes` array replaces the default
array, which is intentional for a custom pane tree. Targets and their SSH
aliases stay in `targets.toml`.

## Custom layouts

Every `[layouts.<name>]` entry is first-class. Dash6 and dash8 are merely two
built-in entries. A leaf layout opens one Herdr tab; `workspace`, `tab`, and
`cwd` default to the layout name when omitted. Pane `cwd` overrides the layout
directory, and `persist = false` lets a `restart = "never"` command exit instead
of leaving a shell open.

```toml
[layouts.logs]
workspace = "ops"
tab = "logs"
cwd = "~/gwt"
restart = "onfailure"

[[layouts.logs.panes]]
id = "app"
label = "app logs"
command = "tail -f /var/log/app.log"

[[layouts.logs.panes]]
id = "system"
from = "app"
split = "right"
ratio = 0.5
cwd = "~/gwt"
command = "journalctl -f"
```

Use a bundle for a tmuxp-style multi-tab workspace. It references leaf layout
names in order; `focus` is the final Herdr tab label. Bundles cannot nest.

```toml
[layouts.dev]
workspace = "dev"
tabs = ["editor", "server"]
focus = "editor"

[layouts.editor]
workspace = "{workspace}"
tab = "editor"
cwd = "~/gwt/my-project"
[[layouts.editor.panes]]
id = "nvim"
command = "nvim"

[layouts.server]
workspace = "{workspace}"
tab = "server"
[[layouts.server.panes]]
id = "server"
command = "make dev"
```

`mux open dev` opens or focuses the named workspace; `--replace` recreates only
the tabs declared by the bundle. `mux check` validates layout structure, target
profiles/server counts, references, and configured directories before opening
anything.
