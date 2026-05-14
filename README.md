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

Install locally with Cargo:

```sh
make install
```

This runs `cargo install --path . --locked` and installs `macron` into Cargo's
local bin directory, usually:

```text
~/.cargo/bin/macron
```

Make sure `~/.cargo/bin` is on `PATH`. Remove it with:

```sh
make uninstall
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
macron import --template logged [plist-or-directory ...]
macron export [-o output-directory]
macron template init
macron template list
macron template add <name> <template.plist.j2>
macron template select <label> <name-or-path>
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

Templates are reusable MiniJinja plist files under:

```text
~/.macron/templates/
```

Imports do not create per-job templates. Imported jobs stay as plain crontab
entries unless you explicitly assign a template.

Create editable starter templates:

```sh
macron template init
```

That writes:

```text
~/.macron/templates/basic.plist.j2
~/.macron/templates/logged.plist.j2
~/.macron/templates/environment.plist.j2
```

Use a template while importing:

```sh
macron import --template logged
```

Or assign one later by label:

```sh
macron template select com.example.backup logged
```

The crontab metadata is just:

```cron
# macron:label=com.example.backup
# macron:template=logged
0 2 * * * /Users/me/bin/backup
```

Add your own template:

```sh
macron template add launchctl-safe ~/templates/launchctl-safe.plist.j2
```

Templates receive these variables:

```text
label
safe_label
index
command
home
log_dir
program_arguments_xml
schedule_xml
interval_seconds
schedule.minute
schedule.hour
schedule.day_of_month
schedule.month
schedule.day_of_week
```

Example template:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
<key>Label</key>
<string>{{ label|escape }}</string>
{{ program_arguments_xml|safe }}
{{ schedule_xml|safe }}
<key>RunAtLoad</key>
<false/>
<key>StandardOutPath</key>
<string>{{ log_dir|escape }}/{{ safe_label|escape }}.out.log</string>
</dict>
</plist>
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
