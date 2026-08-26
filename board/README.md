# Board

`board render` reads `$XDG_CONFIG_HOME/board/board.toml` (normally
`~/.config/board/board.toml`) and renders the `quickshell-bar` surface. Terminal
colors default to `auto`, honor `NO_COLOR`, and can be overridden with
`--color always`, `--color never`, or the `--no-color` alias. An explicit
`--config` overrides that file; embedded defaults are used only when neither
exists.

Use `board surfaces` to list configured surfaces. Other render formats are:

- `terminal`: one ANSI-colored line for a terminal
- `json`: stable structured output for agents and integrations
- `plain`: one uncolored line with health tokens (`ok`, `warn`, `crit`, `unk`)
- `text`: uncolored multiline content for human-readable panels
- `tmux`: one line with tmux color markup
- `quickshell`: one line with Quickshell HTML span markup

Examples:

```sh
board render --format terminal --color never
board render --format json --surface quickshell-bar
board render --format quickshell --surface quickshell-bar
```

Unknown surface names fail instead of falling back to another surface. JSON output contains `schema_version`, `surface`, aggregate `health`, and `items`; `--watch --format json` emits one document per line.

The old positional form remains temporarily available for compatibility.

Checks are selected at runtime: use `kind = "native-*"` for in-process Rust
collectors or `kind = "command"` for an argv-array subprocess. Disabled checks
are not scheduled and consume no runtime CPU or memory.

Command timeouts render `󰔟` by default and keep the reason in the JSON `detail`
field. Set `timeout_text` on a check to choose another compact glyph.

[[check]]
name = "scripts"
kind = "native-script-status"
interval = "10s"
timeout = "2s"
fresh_for = "30s"


`native-script-status` reads `source` or `~/script-status.ini`.
`native-alertmanager` reads only `alertmanager.url` from `source` or
`~/.config/amtool/config.yml`, queries active alerts through `/api/v2/alerts`,
and does not launch `amtool`.
[[check]]
name = "alerts"
kind = "native-alertmanager"
filters = ['squad="telephony.proxy.squad"']
critical_destination = "incidentio"
warning_destination = "slack"
excluded_channel = "#term-vendor-alerts"
interval = "10s"
timeout = "2s"
fresh_for = "30s"

[[check]]
name = "time"
kind = "native-time-offset"
interval = "10s"
timeout = "2s"
fresh_for = "30s"

[[check]]
name = "resolv"
kind = "native-resolv"
interval = "15s"
timeout = "2s"
fresh_for = "45s"

[[check]]
name = "safe"
kind = "native-safe"
interval = "30s"
timeout = "2s"
fresh_for = "90s"

[[check]]
name = "agents"
kind = "command"
command = ["check-agents"]
interval = "10s"
timeout = "2s"
fresh_for = "30s"

[[check]]
name = "gpg"
kind = "command"
command = ["check-gpg"]
interval = "30s"
timeout = "2s"
fresh_for = "90s"

[[check]]
name = "docker"
kind = "native-docker"
interval = "30s"
timeout = "2s"
fresh_for = "90s"

[[check]]
name = "cpu"
kind = "native-cpu"
interval = "5s"
fresh_for = "15s"

[[check]]
name = "mem"
kind = "native-mem"
interval = "10s"
fresh_for = "30s"

[[check]]
name = "temp"
kind = "native-temp"
interval = "30s"
fresh_for = "90s"


[[check]]
name = "weather"
kind = "native-weather"
enabled = false
interval = "15m"
timeout = "3s"
fresh_for = "30m"

[[check]]
name = "time-panel"
kind = "native-time-panel"
interval = "1s"
fresh_for = "3s"

[[check]]
name = "todo-panel"
kind = "native-todo-panel"
interval = "60s"
timeout = "2s"
fresh_for = "3m"

[[surface]]
name = "quickshell-bar"
checks = ["scripts", "alerts", "time", "resolv", "safe", "gpg", "docker", "agents", "cpu", "mem", "temp", "weather"]

[[surface]]
name = "tmux-top"
checks = ["agents", "gpg", "docker", "cpu", "mem"]

[[surface]]
name = "quickshell-panels"
checks = ["time-panel", "todo-panel"]

See [docs/design.md](docs/design.md) for the daemon, render, event hook, and reconciliation design.

## Portability

| Check | Linux amd64 | macOS arm64 |
| --- | --- | --- |
| time, time panel, weather | yes | yes |
| safe | yes | yes |
| Docker | `/var/run/docker.sock` | `DOCKER_HOST`, active Docker context, Docker Desktop, and named Colima profiles |
| CPU | `/proc/stat` utilization | native 1-minute load normalized by logical CPUs |
| memory | `/proc/meminfo` | Apple IOReport through the macOS-only `macmon` library |
| temperature | thermal sysfs | Apple IOReport through the macOS-only `macmon` library |
| resolv | yes | yes |
| command | yes | yes, when the command exists |

Core native checks stay compiled by default. Runtime `enabled = false` already
removes their scheduling cost. Cargo feature gates are reserved for future
module families that introduce optional dependencies or meaningful binary
weight; dynamic plugin loading is intentionally out of scope.

## Measured baseline

Measured 2026-08-24 with the release binary:

| Host | Cached render | Daemon RSS / CPU | Full batch |
| --- | --- | --- | --- |
| Linux amd64 | 3.6 ms | 6.4 MB / 0.20% | 2.0 s, bounded by Alertmanager |
| macOS arm64 | 6 ms | 18.2 MB / 0.0% | 1.18 s, including one shared 100 ms hardware sample |

The separate Linux Quickshell watcher used 6.1 MB RSS and 0.05% CPU. On macOS,
memory and temperature share a five-second cache around one 100 ms `macmon`
sample; the dependency is target-specific and its TUI feature is disabled.
