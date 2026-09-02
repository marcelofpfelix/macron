# textwarrior

`textwarrior` is a small Rust CLI for the local-first task format:

```text
todo.txt <-> todo.md <-> Taskwarrior JSON
```

It uses the `todo-txt` Rust crate for todo.txt parsing.

## Commands

```sh
cargo run -p textwarrior -- lint-md tasks/personal-todo.md
cargo run -p textwarrior -- --color always lint-md tasks/personal-todo.md
cargo run -p textwarrior -- lint-txt tasks/todo.txt
cargo run -p textwarrior -- normalize-md tasks/todo.md > tasks/demo-todo.md
cargo run -p textwarrior -- txt-to-md tasks/todo.txt > tasks/todo.md
cargo run -p textwarrior -- md-to-txt tasks/todo.md > tasks/todo.txt
task export | cargo run -p textwarrior -- task-json-to-md > tasks/todo.md
cargo run -p textwarrior -- md-to-task-json tasks/todo.md > task-import.json
task import task-import.json
textwarrior lint
textwarrior sync --plan
textwarrior sync --profile personal --plan
textwarrior sync --task-filter +personal --task-filter status:pending --plan
textwarrior sync --list-profiles
textwarrior sync --list-strategies
textwarrior sync --reset-state --dry-run
textwarrior sync --dry-run
textwarrior sync --strategy md
textwarrior sync
```

`lint-md` fails on import-safety errors and prints style warnings without
failing. Use `normalize-md` to autofix mechanical ordering/formatting into the
compact one-line `todo.md` form. Use
`--color auto|always|never` for colored lint output.

`sync --plan` prints item-level new/changed/moved/deleted/unchanged counts
against `.textwarrior/state.json` without writing files or importing into
Taskwarrior.
When the same tracked task changed in both `todo.md` and Taskwarrior,
`--strategy fail` stops, `--strategy md` keeps the local task, and
`--strategy taskwarrior` keeps the Taskwarrior export.
`sync --list-resolution-strategies` is accepted as a syncall-compatible alias
for `sync --list-strategies`.
`sync --reset-state` removes only `.textwarrior/state.json`; it does not delete
todo files or Taskwarrior tasks.

Compact files can use frontmatter defaults to avoid repeating obvious fields:

```md
---
textwarrior: 1
defaults:
  project: personal
  source: personal.todo
  status: not_started
render:
  status: false
  uuid: compact
id_prefix: p
details_path: details
---

- [ ] Fix exporter u:qkmid436dqiq7ec0o8j0cqlc6g id:p-1
- [/] Call bank u:54c3... id:p-2
- [?] Review docs u:91ea... id:p-3 md:p-3
```

## Development

```sh
make check
make container-e2e
make install
```

`make install` runs `cargo install --path . --locked`.
`make container-e2e` builds the Linux release binary inside `Dockerfile.e2e`
and runs isolated sync tests with a fake Taskwarrior command.

See [SPEC.md](SPEC.md) for the task format, lint rules, import/export behavior,
and what is intentionally out of scope.

## Sync Config

Do not auto-import into Taskwarrior directly from file-change events. Run
`textwarrior sync --plan` or `textwarrior sync --dry-run` first, then
`textwarrior sync` when the count and target files look right.

Suggested config shape:

```toml
todo_dir = "~/sync/tasks"
task_command = "task"

[[files]]
name = "personal"
path = "personal-todo.md"
export_txt = "personal-todo.txt"

[[files]]
name = "work"
path = "work-todo.md"
export_txt = "work-todo.txt"

[[profiles]]
name = "personal"
files = ["personal"]
task_filter = ["+personal", "status:pending"]
```

Each file gets its own generated `todo.txt` shadow, or the configured
`export_txt` path. Sync exports Taskwarrior to `todo.md` first, merges
non-conflicting local file edits, then imports `todo.md` back to Taskwarrior.
Successful syncs write derived item state to `.textwarrior/state.json`.
Profiles sync only named configured files. If `task_filter` is set,
profile sync passes those args to `task` before `export`. Repeat
`--task-filter` for one-off Taskwarrior filter args; they append after any
profile `task_filter`. Filtered sync preserves tracked local tasks that are
absent only because of the filter.
