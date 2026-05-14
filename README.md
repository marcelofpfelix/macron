# macron

`macron` keeps a local crontab-format file and converts calendar-based macOS
`launchd` plist jobs to and from that file.

The default local file is:

```text
~/.macron/crontab
```

Override it with `MACRON_FILE` or `--file`.

After installing the compiled binary somewhere on `PATH`, run:

```sh
macron
macron -e
```

Running `macron` with no subcommand scans the default user and local launchd
plist directories, refreshes the local crontab-format file, and prints it.
Running `macron -e` edits that same file and then exports plist files to
`~/Library/LaunchAgents`. If the file does not exist yet, `macron -e` seeds it
from the detected launchd schedules before opening the editor.

Add `--system` to include Apple OS-managed jobs from `/System/Library` in
discovery:

```sh
macron --system
macron --system import
```

Crontab output is colorized automatically when stdout is a terminal. Override it
with:

```sh
macron --color always
macron --color never
```

To build a native macOS release binary on macOS:

```sh
make macos-binary
```

The binary is written to `dist/macron-v0.1.0-macos-<arch>`.

## Commands

```sh
macron list
macron import [plist-or-directory ...]
macron export [-o output-directory]
macron -e
```

`macron -e` opens the local file with `$VISUAL`, `$EDITOR`, or `vi`, then exports
the edited jobs to `~/Library/LaunchAgents`.

When importing without paths, `macron` scans:

```text
~/Library/LaunchAgents
/Library/LaunchAgents
/Library/LaunchDaemons
```

With `--system`, it also scans:

```text
/System/Library/LaunchAgents
/System/Library/LaunchDaemons
```

Detected and imported jobs are stored as normal five-field crontab lines with
nearby metadata comments preserving the launchd label and source file. Running
`macron import` refreshes the local file from the selected plist paths instead of
appending duplicate entries.

## Templates

Imported jobs save their original plist as a per-label template under:

```text
~/.macron/templates/
```

On export, `macron` loads the template and updates only the managed fields:

```text
Label
ProgramArguments
StartCalendarInterval
StartInterval
```

That preserves fields such as `EnvironmentVariables`, `StandardOutPath`,
`StandardErrorPath`, `RunAtLoad`, and `KeepAlive`.

For jobs created from scratch in `macron -e`, add one of the generic built-in
templates before the cron line:

```cron
# macron:label=com.example.backup
# macron:template=basic
0 2 * * * /Users/me/bin/backup

# macron:label=com.example.logged
# macron:template=logged
*/15 * * * * /Users/me/bin/sync

# macron:label=com.example.env
# macron:template=environment
0 * * * * /Users/me/bin/job
```

Built-in templates:

- `basic`: minimal launchd plist.
- `logged`: adds `RunAtLoad=false`, stdout log, and stderr log paths.
- `environment`: adds `RunAtLoad=false` and a Homebrew-friendly `PATH`.

Custom templates are also supported:

```cron
# macron:label=com.example.custom
# macron:template=/Users/me/templates/custom.plist
0 3 * * * /Users/me/bin/custom-job
```

## Supported Schedules

`macron` reads and writes `StartCalendarInterval` launchd jobs. It supports:

- single `StartCalendarInterval` dictionaries
- arrays of `StartCalendarInterval` dictionaries
- numeric cron fields, `*`, comma lists, ranges, and steps
- `StartInterval` jobs that can be represented as whole-minute cron intervals

Exported jobs use `ProgramArguments` with `/bin/sh -lc <command>` so shell-style
crontab commands are preserved. Imported `StartInterval` jobs are marked with
`# macron:start-interval=<seconds>` and export back as `StartInterval`.
