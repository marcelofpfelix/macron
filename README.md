# rush

`rush` is a Rust workspace for small personal and operations CLI tools.

All apps in this repo are intended to be agent-native and written in Rust. This
repo holds source for tools that are too substantial for dotfiles shell snippets,
but too small to justify one repository per command.

## Repositories

- [`macron`](macron/): keeps a local crontab-format file and converts
  calendar-based macOS `launchd` plist jobs to and from that file.

## Development

This repo is a Cargo workspace. Run checks from the repository root:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
