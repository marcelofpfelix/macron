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

Running `macron` with no subcommand prints the local crontab-format file.
Running `macron -e` edits that same file and then exports plist files to
`~/Library/LaunchAgents`.

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

Imported jobs are stored as normal five-field crontab lines with nearby metadata
comments preserving the launchd label and source file.

## Supported Schedules

`macron` reads and writes `StartCalendarInterval` launchd jobs. It supports:

- single `StartCalendarInterval` dictionaries
- arrays of `StartCalendarInterval` dictionaries
- numeric cron fields, `*`, comma lists, ranges, and steps

Exported jobs use `ProgramArguments` with `/bin/sh -lc <command>` so shell-style
crontab commands are preserved.
