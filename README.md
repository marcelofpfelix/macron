# rush

`rush` is a Rust workspace for small personal and operations CLI tools.

All apps in this repo are intended to be agent-native and written in Rust. This
repo holds source for tools that are too substantial for dotfiles shell snippets,
but too small to justify one repository per command.

## Repositories

- [`amt`](amt/): small Intel AMT power and boot control CLI.
- [`board`](board/): local status collector and renderer for tmux,
  Quickshell, and future dashboard surfaces.
- [`che`](che/): searchable local keybinding index for Hyprland, tmux, Herdr,
  and Neovim.
- [`macron`](macron/): keeps a local crontab-format file and converts
  calendar-based macOS `launchd` plist jobs to and from that file.
- [`mux`](mux/): declarative Herdr workspaces and server dashboards.
- [`rush-core`](rush-core/): small shared models and render helpers used by
  multiple `rush` tools.
- [`textwarrior`](textwarrior/): local-first todo.txt, Markdown, and
  Taskwarrior synchronization CLI.

## Development

This repo is a Cargo workspace. Run checks from the repository root:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

For local desktop integration, build release binaries and install them into
`~/bin`:

```sh
make install
```

The install target writes `amt`, `board`, `che`, `macron`, `mux`, and
`textwarrior` to `~/bin`. The dotfiles checkout already has a shell script
named `che`; keep that collision in mind before switching PATH order or
replacing the old cheatsheet wrapper.

Build distributable macOS bundles for both Apple Silicon and Intel Macs with:

```sh
make release-macos
```

The output is written to `dist/aarch64-apple-darwin/` and
`dist/x86_64-apple-darwin/`. These artifacts are reproducible and ignored by
Git.
