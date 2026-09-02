# textwarrior Spec

## Goal

`textwarrior` keeps local-first task files usable from editors, Syncthing,
Taskwarrior, and simple todo.txt tools.

The canonical rich file is `todo.md`. Generated `todo.txt` and Taskwarrior JSON
are interchange formats, not the source of truth.

## Source Files

Auto source discovery uses filenames, not required frontmatter:

- include `*-todo.md`;
- exclude `inbox.md`;
- exclude `done-*-todo.md` and `*-done-todo.md`;
- skip `archive/`, `.git/`, and `.obsidian/`.

Use `inbox.md` for capture/review. Do not auto-import it.

## Config

Default config path:

```text
~/.config/textwarrior/config.toml
```

Example:

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

Relative file paths resolve under `todo_dir`. `~` is expanded.

`textwarrior lint` with no paths lints configured files. If no `[[files]]` are
configured, it scans `todo_dir` for `.md` and `.txt` files, skipping archive and
editor metadata folders.

`textwarrior sync` runs both directions in a fixed order:

```text
Taskwarrior export -> todo.md -> todo.txt shadow files -> Taskwarrior import
```

It exports Taskwarrior first so the file view includes current Taskwarrior UUIDs
and annotations, then it imports the configured `todo.md` files back by UUID.
Normal sync uses item-level state to preserve non-conflicting local changes,
detect changed-on-both-sides tasks, and handle moves between files. On first
sync, or when item-level state is unavailable, it falls back to a whole-file
guard that refuses to overwrite different non-empty content unless `--force` is
used.
Formatting-only differences are allowed by comparing normalized content. When
no `[[files]]` are configured, `sync` discovers `*-todo.md` files and writes
matching `*.txt` shadow files beside them.

Use a profile to sync only a named set of configured files:

```sh
textwarrior sync --profile personal --plan
textwarrior sync --task-filter +personal --task-filter status:pending --plan
textwarrior sync --list-profiles
textwarrior sync --list-strategies
textwarrior sync --reset-state --dry-run
```

Profiles are local sync combinations. They require `[[files]]` entries and
refer to those entries by `name`. `task_filter` is optional; when set,
`textwarrior` passes those arguments to `task` before `export`, like:

```sh
task +personal status:pending export
```

Repeat `--task-filter` for one-off Taskwarrior filter args. These append after
the profile's saved `task_filter`. Filtered syncs preserve tracked local tasks
that are absent only because of the filter; absence is treated as deletion or a
move only when that task identity appears elsewhere in the same filtered export
or when no filter is active.

`textwarrior sync --plan` performs the same discovery and Taskwarrior export,
but does not write files or import into Taskwarrior. It prints item-level counts
against the last sync state:

```text
sync plan: 2 new, 1 changed, 1 moved, 0 deleted, 33 unchanged
```

After a successful non-dry-run sync, `textwarrior` writes:

```text
<todo_dir>/.textwarrior/state.json
```

The state file records each task identity, current source path, and normalized
hash. It is derived state, not source of truth. It exists to support
syncall-style item change detection and future conflict handling.

If a task changed in both `todo.md` and Taskwarrior since the last sync,
`textwarrior sync` refuses to overwrite the file and reports the conflicting
task identities. Use `--force` only when Taskwarrior should win that export.
If only `todo.md` changed, the local task line is preserved and imported. If
only Taskwarrior changed, the Taskwarrior export updates the file. Local
deletion plus remote modification is treated as a conflict.

Conflict strategies:

- `--strategy fail`: default; stop on changed-on-both-sides tasks.
- `--strategy md`: keep the local `todo.md` task for changed-on-both-sides
  tasks and import it into Taskwarrior.
- `--strategy taskwarrior`: keep the Taskwarrior export for
  changed-on-both-sides tasks.

Use `textwarrior sync --list-strategies` to print the available strategies.
`--list-resolution-strategies` is accepted as a syncall-compatible alias.

`--force` remains stronger than `--strategy`: it lets the Taskwarrior export
overwrite the file, including fallback whole-file conflicts.

Reset:

- `sync --reset-state` removes only `<todo_dir>/.textwarrior/state.json`.
- `sync --reset-state --dry-run` and `sync --reset-state --plan` print the
  state file path without removing it.
- Resetting state does not delete todo files, generated todo.txt shadows, or
  Taskwarrior tasks. It only makes the next sync behave like a first sync.

## todo.md Format

Each task starts with one Markdown checkbox heading:

```markdown
- [ ] (H) 2026-07-05 Fix exporter +work @desktop id:w-1 status:review source:manual due:2026-07-10

  Freeform note.
```

With frontmatter defaults, repeated fields should be omitted from task lines:

```markdown
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

- [ ] 2026-07-05 Fix exporter u:qkmid436dqiq7ec0o8j0cqlc6g id:p-1 due:2026-07-10
- [/] Call bank u:54c3... id:p-2
- [?] Review broker docs u:91ea... id:p-3 md:p-3
- [x] 2026-07-06 2026-07-05 Filed receipt u:8ad2... id:p-4
```

Supported checkbox states:

- `- [ ]`: open;
- `- [x]`: done;
- `- [-]`: deleted/dropped.
- `- [/]`: in progress when `render.status: false`;
- `- [?]`: in review when `render.status: false`;
- `- [!]`: blocked when `render.status: false`.

Preferred metadata order:

```text
u uuid id status entry end due wait t scheduled recur priority project tags depends source md ext assignee parent estimate updated issue pm aliases
```

`normalize-md` is the autofix path for mechanical formatting and field order.
It keeps metadata inline when the value has no whitespace. Use indented metadata
only for values that cannot safely fit in the one-line form.

`u:` is a reversible compact encoding of the Taskwarrior UUID. `uuid:` remains
accepted and is still the canonical Taskwarrior import key after decode. Use
shorter human IDs in `id:` such as `p-1`; `id_prefix: p` compacts exported IDs
like `personal-001` to `p-1`.

Dates follow todo.txt ordering:

- open task: optional creation date after priority, or first when no priority;
- done task: `[x] completion-date creation-date title`;
- old `entry:` and `end:` imports are accepted, but normalize to positional
  dates.

`md:` points to a Markdown details file by filename stem only, without `.md`.
`details_path` defines the folder.

Linked tasks use Taskwarrior's native dependency field:

```markdown
- [ ] Build importer id:p-1
- [ ] Review importer id:p-2 depends:p-1
```

Taskwarrior accepts `depends:<id-or-uuid,...>`. `textwarrior` sends UUID
dependencies to Taskwarrior `depends`; local IDs stay as `textwarrior depends:`
annotations so they are not confused with Taskwarrior's internal numeric IDs.

## Frontmatter

Frontmatter is optional. `defaults:` may fill missing task metadata:

```yaml
---
textwarrior: 1
defaults:
  project: work
  source: manual
  status: not_started
render:
  status: false
  uuid: compact
id_prefix: w
details_path: details
---
```

Defaults must not hide important per-task state. If a task differs from the
file default, put the field on the task or use a checkbox marker. `md:` is
stored as Taskwarrior annotation metadata.

## Lint Rules

`lint-md` reports errors and warnings.

Lint output supports `--color auto|always|never`, matching the `macron` CLI
style. `auto` disables color when stdout is not a terminal or `NO_COLOR` is set.

Errors fail the command:

- file cannot parse as `todo.md`;
- task is missing `id:`, `uuid:`, or `u:`;
- date metadata is not `YYYY-MM-DD`.

Warnings do not fail the command:

- output differs from `normalize-md`;
- completed checkbox has no completion date;
- checkbox and `status:` disagree in a way that may surprise import/export.

`lint-txt` checks lossy todo.txt files for stable identity, completion dates,
empty metadata values, and date shape.

## Import/Export Rules

Taskwarrior import uses JSON from `md-to-task-json`.

- `uuid:` and decoded `u:` update existing Taskwarrior tasks.
- `id:`, `source:`, and non-Taskwarrior statuses are stored as annotations.
- Taskwarrior annotation entries like `[20260705T172805Z] note` round-trip back
  to annotation `entry` fields, not nested note text.
- Non-UUID dependency IDs stay as annotations; UUID dependencies use
  Taskwarrior `depends`.

Taskwarrior export uses `task-json-to-md`.

- Taskwarrior `project` becomes `+project` in the task heading.
- Taskwarrior tags become `@tag`.
- `textwarrior key:value` annotations are restored to metadata fields.

Generated `todo.md`, configured `export_txt` shadow files, and state files are
written atomically via a temporary sibling path and rename. Temporary
Taskwarrior import JSON is written under `.textwarrior/tmp/` and removed after
the import attempt.

## Beads Bridge

Beads support is a real bridge, not dummy test data:

- `beads-json-to-md` reads `bd --readonly list --json`;
- `beads-jsonl-to-md` reads `.beads/backup/*.jsonl`.

The bridge is one-way until conflict semantics are designed:

```text
Beads -> todo.md -> Taskwarrior
```

Mapped Beads fields: `id`, `title`, `description`, `notes`, `status`,
`priority`, `issue_type`, `labels`, `external_ref`, `assignee`, `due_at`,
`defer_until`, and dependencies. Beads fields that do not fit Taskwarrior cleanly
should stay as textwarrior metadata or notes, especially issue type, external
reference, assignee, local dependency IDs, and rich descriptions.

## Super Productivity Bridge

Super Productivity has important fields that Taskwarrior does not model
directly:

- time tracking: `timeSpent`, `timeSpentOnDay`, `timeEstimate`;
- planning: `dueDay`, `dueWithTime`, `hasPlannedTime`;
- deadlines/reminders: `deadlineDay`, `deadlineWithTime`, `deadlineRemindAt`,
  `reminderId`, `remindAt`;
- hierarchy and repetition: `parentId`, sub-tasks, `repeatCfgId`;
- attachments and issue-provider state: `attachments`, `issueId`,
  `issueProviderId`, `issueType`, `issueTimeTracked`, `issuePoints`,
  `issueLastSyncedValues`.

Bridge these as metadata only when needed. Good compact candidates are
`spent:`, `est:`, `deadline:`, `rem:`, `repeat:`, `parent:`, `issue:`,
`provider:`, `points:`, and `attach:`. Do not implement all of them until there
is real data that needs round-tripping.

Super Productivity plugins can read/update tasks through the plugin API and
store plugin data with `persistDataSynced`/`loadSyncedData`. Secrets are
per-device plugin storage and are not part of sync/export/backups.

Storage:

- app task state uses the `SUP_OPS` operation-log database, backed by IndexedDB
  today and with a SQLite adapter path in the source;
- Electron settings/plugin consent use the Electron `userData` folder,
  especially the `simpleSettings` file;
- use Super Productivity export/sync/plugin APIs for a bridge, not direct DB
  edits.

## syncall Lessons

syncall feature decisions:

| syncall idea | textwarrior decision |
| --- | --- |
| Item-level sync state | Implemented as `.textwarrior/state.json`, keyed by `id:` before `uuid`. |
| Push/pull bidirectional sync | Implemented for Taskwarrior `<->` todo.md with merge before import. |
| Plan/count summary | Implemented as `sync --plan`: new, changed, moved, deleted, unchanged. |
| Changed-on-both-sides detection | Implemented before export overwrites local todo.md content. |
| Resolution strategies | Implemented: `fail`, `md`, `taskwarrior`; `newest` waits for per-side modification timestamps. |
| Strategy listing | Implemented as `sync --list-strategies`; `--list-resolution-strategies` is an alias. |
| Named combinations | Adapted as configured `[[profiles]]` over named `[[files]]`. |
| List combinations | Adapted as `sync --list-profiles`. |
| Taskwarrior tag/project filters | Implemented through profile `task_filter` and repeated `--task-filter`; no separate tag/project flags. |
| Modified-last-N-days filter | Use a normal Taskwarrior filter in `task_filter`; no special flag. |
| Addition/update/delete action summaries per side | Deferred until sync has richer side/action tracking. |
| Side abstraction for new backends | Future idea for `TodoMdSide`, `TaskwarriorSide`, `BeadsSide`, and `SuperProductivitySide`. |
| Remote service ID mapping | Future idea for bridges; current local sync uses task identity and derived state. |
| Atomic writes and temp cleanup | Implemented for generated files and Taskwarrior import JSON. |
| Reset from scratch | Implemented only as safe `sync --reset-state`; it removes derived state, not tasks/files. |
| Generic many-service framework | Not copied; too broad for the local-first core. |
| Opaque pickle-like caches | Not copied; state stays JSON and derived. |
| OAuth/service connectors | Not copied into core; Google, Notion, Asana, and CalDAV bridges should be separate commands. |
| External service as source of truth | Not copied; todo.md remains the canonical rich local file. |
| Shell completions | Deferred until the CLI stabilizes. |
| Broad integration dependencies | Not copied until real bridge data requires them. |

## Compact Aliases

Implemented:

- `u:` -> Taskwarrior `uuid`;
- checkbox marker -> hidden `status`;
- positional creation/completion dates -> `entry`/`end`;
- `+project` and `@tag` in the title;
- `md:stem` for details files.

Documented candidates:

- `dep:` -> `depends`;
- `s:` -> `scheduled`;
- `rec:` -> `recur`;
- `est:` -> `estimate`;
- `spent:` -> tracked time;
- `deadline:` -> deadline date/time;
- `provider:`/`issue:`/`points:` -> external issue fields.

## Out Of Scope

- No generic TSV/table importer. The `/tmp/personal.todo` conversion was a
  one-off migration command, not product code.
- No automatic file-watch import into Taskwarrior yet.
- No Ratatui UI yet. A TUI can be added later for interactive review, but the
  lint and normalize rules should stabilize first.
