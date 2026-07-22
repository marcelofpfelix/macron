# che and board design

Date: 2026-07-22

This note turns two local-tool ideas into concrete `rush` projects:

- `che`: searchable keybinding and defaults index.
- `board`: native status collector and renderer for Quickshell, tmux, and a
  later dashboard.

Source code should live in this `rush` workspace. Dotfiles should keep thin
wrappers, Quickshell QML integration, tmux snippets, and install/distribution
state.

## Constraints From Current Dotfiles

- Hyprland has two Lua profiles: `default` and `modern`.
- Quickshell owns the Wayland shell surface and already polls status commands
  through `StatusText.qml`.
- tmux and Herdr intentionally share muscle memory. tmux prefix is `C-a`;
  Herdr config mirrors it where possible.
- Neovim keymaps are spread across `lua/config/keymaps.lua`, plugin files, and
  runtime/plugin defaults.
- Existing status scripts already have a useful contract through
  `lib_status.sh`: script exit code, text output, and Prometheus textfile
  metrics.
- `bar`, `tbar`, and Quickshell currently run several small shell commands at
  fixed intervals. Some checks, such as CPU percent, do work every invocation
  that would be cheaper in a long-lived process.

## Project 1: `che`

`che` is the local keybinding and cheatsheet index. It should answer: "what
does this key do here?", "where is this bound?", "what conflicts?", and "what
is different from the default?".

### MVP

Commands:

```sh
che list
che find
che explain '<key>'
che conflicts
che defaults
che dump --format json
```

Default UX:

- `che` with no args opens `fzf` when available and falls back to a plain table.
- Rows include scope, mode/profile, key, action, source, default/current, and
  file path.
- `che explain` shows all matches for one key across Hyprland, tmux, Herdr,
  and Neovim.
- `che conflicts` reports exact key collisions within the same scope and
  meaningful cross-scope overlaps, such as tmux versus Herdr divergence.

### Sources

Hyprland:

- Current sources are Lua profile files under
  `dotfiles/main/desktop/.config/hypr/profiles/`.
- Phase 1 can parse direct `hl.bind(...)` calls plus simple loops for
  workspaces.
- Better phase: add an optional export mode to the Hyprland Lua helper so
  profiles can emit binding JSON without requiring brittle text parsing.
- Defaults should be a checked-in static snapshot per supported Hyprland
  version, refreshed manually with `hyprctl binds -j` or upstream docs when the
  installed version changes.

tmux:

- Current source is `dotfiles/main/desktop/.tmux.conf`.
- Parse `set -g prefix`, `bind`, `bind-key`, `bind -n`, `bind -T`, and
  `unbind`.
- Defaults should come from `tmux -f /dev/null list-keys` when tmux is
  available, cached as a fixture for offline comparisons.

Herdr:

- Current source is `dotfiles/main/desktop/.config/herdr/config.toml`.
- Parse `[keys]` and `[[keys.command]]`.
- Treat Herdr as the logical peer of tmux. The first rule is not "no
  collision"; it is "same muscle memory resolves to the same intent".

Neovim:

- Static parser handles `vim.keymap.set(...)` and
  `vim.api.nvim_set_keymap(...)` in Lua files.
- Runtime mode should run Neovim headless to dump resolved maps, including
  plugin defaults, when `nvim` is available.
- Defaults should come from `nvim --clean --headless` map dumps and checked-in
  fixtures.

### Data Model

```text
Binding {
  scope: hyprland | tmux | herdr | neovim
  profile: default | modern | global | plugin | runtime
  mode: normal | visual | insert | terminal | root | copy-mode-vi | global
  key: normalized key chord
  action: normalized action string
  description: optional human label
  source: file path plus line when known
  origin: current | default | runtime
  confidence: exact | parsed | inferred
}
```

Normalize keys into a single representation: `C-a`, `S-Return`, `Super+D`,
`XF86AudioMute`, `<leader>qq`. Preserve the original string for display.

### Implementation Plan

1. Create a `che` crate with JSON fixtures and snapshot tests for parsers.
2. Implement tmux and Herdr parsers first because both are small and highly
   structured.
3. Implement Neovim static parser for explicit local maps, then add runtime
   dump support.
4. Implement Hyprland direct parser, then add the cleaner Lua export path if
   parser coverage becomes fragile.
5. Add fuzzy UI through external `fzf`; do not build a TUI until the index
   model is stable.
6. Add dotfiles wrapper `che` only after `cargo test` covers representative
   local config snippets.

### Validation

```sh
cargo test -p che
cargo run -p che -- dump --format json
cargo run -p che -- conflicts
```

For integration:

```sh
tmux -f /dev/null list-keys >/tmp/tmux-default-keys.txt
nvim --clean --headless '+map' '+qall'
```

## Project 2: `board`

`board` is the native status runtime. It should replace repeated status-bar
shell polling with one efficient collector process plus cheap render commands.
It should also leave a path to a future dashboard without forcing the dashboard
into the first release.

### MVP

Commands:

```sh
board run
board once
board render quickshell
board render tmux
board check <name>
board checks
board metrics
board doctor
```

Runtime behavior:

- `board run` is a long-lived async collector.
- Each check has its own interval, timeout, freshness limit, and renderer.
- Results are cached under `${XDG_RUNTIME_DIR:-/tmp}/board/`.
- `board render quickshell` and `board render tmux` are one-shot readers of the
  cache, suitable for Quickshell `StatusText` and tmux `#(...)`.
- `board once` runs all due checks once and writes the cache, useful for cron,
  debugging, and non-daemon startup.

### Check Types

Native checks:

- CPU, memory, temperature: read `/proc` and `/sys`; keep previous CPU samples
  in memory so CPU percent does not need `sleep 1`.
- Docker: use the Docker socket or `docker ps` adapter initially; cache with a
  longer interval.
- Agent state: call or embed `agent-tmux` as a compatibility source first.
- Time/date and weather can stay outside initially. Quickshell native services
  or existing scripts are acceptable until they are proven slow.

External checks:

- Existing `check-*` scripts can be wrapped with configured interval, timeout,
  parser, and renderer.
- External script output remains authoritative for old modules until a native
  check replaces it.
- Exit codes follow the current bar convention: `0` ok, `1` warning, `2+`
  critical or command-specific failure.

### Config Shape

```toml
[runtime]
socket = "auto"
cache_dir = "auto"

[[check]]
name = "cpu"
kind = "native.cpu"
interval = "2s"
timeout = "500ms"
fresh_for = "10s"
render = "icon_value"

[[check]]
name = "docker"
kind = "command"
command = ["check-docker"]
interval = "30s"
timeout = "2s"
fresh_for = "2m"

[[surface]]
name = "quickshell-bar"
checks = ["scripts", "alerts", "time", "resolv", "safe", "cpu", "mem"]
format = "quickshell"

[[surface]]
name = "tmux-top"
checks = ["agents", "docker", "cpu", "mem"]
format = "tmux"
```

### Quickshell Integration

Phase 1 should replace many QML `StatusText` process invocations with one or
two cheap commands:

```qml
StatusText { command: ["board", "render", "quickshell", "quickshell-bar"]; interval: 1000; rich: true }
```

Better phase:

- Keep `board run --jsonl` as a single Quickshell `Process`.
- Update QML models from JSON events instead of polling render output.
- Keep battery, audio, tray, workspace, and lock/session controls native in
  Quickshell because those are already better handled by QML services.

### tmux Integration

tmux should not perform heavy scans every status refresh. It should read cached
state:

```tmux
set -ag status-right " #(board render tmux tmux-top)"
```

`agent-tmux` should remain the agent-aware tool until there is a clean native
Rust replacement. `board` can consume its cached output or run it at a configured
interval.

### Dashboard Path

Do not make the dashboard first. The useful foundation is a stable state API:

- `board state --json`
- `board stream --jsonl`
- optional local HTTP/SSE server later
- optional static HTML dashboard later

This keeps `board` distinct from the existing Zoidboard/Craboard work. Zoidboard
is broader agent/session/product dashboard work; `board` is the local machine
status runtime that can feed a dashboard.

### Implementation Plan

1. Create `board` crate with config parsing, result model, and renderer tests.
2. Implement `board once`, command checks, cache writes, and `render tmux`.
3. Add native CPU/memory/temperature collectors with async intervals.
4. Add `board run` with per-check intervals, timeouts, and stale-state handling.
5. Add `render quickshell` and replace only one Quickshell group first.
6. Add dotfiles wrappers for compatibility: `bar`, `tbar`, or selected
   `check-*` commands can delegate to `board` when stable.
7. Add JSON state and JSONL stream for future dashboard work.

### Validation

```sh
cargo test -p board
cargo run -p board -- once --config fixtures/board.toml
cargo run -p board -- render tmux tmux-top
cargo run -p board -- render quickshell quickshell-bar
```

Runtime smoke checks:

```sh
timeout 10s cargo run -p board -- run --config fixtures/board.toml
board doctor
```

## Project Boundary

Build first:

1. `che` parser/index/fzf CLI.
2. `board once` plus cached renderers.
3. `board run` async collector.

Delay:

- full TUI
- web dashboard
- replacing `agent-tmux`
- replacing every `check-*` script
- package/distribution changes in homelab

This keeps both projects useful early while avoiding a large rewrite of working
dotfiles.
