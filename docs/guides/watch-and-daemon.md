# Watch mode and the daemon

Autophagy can ingest new agent activity continuously instead of one import at a
time. This is purely additive: watch mode runs the same native adapters as
`autophagy import`, under the same redaction, path-exclusion, and search-
projection gates. It only ingests — it never executes, installs, or acts on the
evidence it collects, and nothing leaves the machine.

## Foreground: `autophagy watch`

Run a loop that periodically discovers and incrementally imports native agent
history:

```sh
autophagy watch                       # all native adapters, 60s interval
autophagy watch --adapter claude-code # one adapter
autophagy watch --interval 30 --once  # a single cycle, then exit
```

Each cycle discovers transcripts and imports only what is new; the store's source
cursors make repeat cycles cheap and idempotent, so a second cycle over unchanged
history inserts nothing. Watch prints a summary line per adapter only when there
is something to report (events inserted, conflicts, rejections, or a failure) and
stays quiet otherwise.

- `--adapter <name>` selects native adapters (`claude-code`, `codex`); repeat it
  to watch several. The default is every available native adapter.
- `--interval <seconds>` sets the delay between cycles (default 60).
- `--once` runs a single cycle and exits — useful under a supervisor and in
  tests.
- `--include-content`, `--project`, and `--exclude-path` behave exactly as they
  do for `import`.

An import failure in one cycle is logged and does not stop the loop; the next
cycle retries. A failure that repeats identically is reported once, then
suppressed until it changes, so a persistent error does not flood the output.

`watch` shuts down gracefully on `SIGINT` (Ctrl-C) or `SIGTERM`: it finishes the
in-flight import and its transaction, then exits 0. With `--output json` it emits
one JSON object per non-quiet cycle followed by a final summary object.

## Background: `autophagy daemon`

The daemon runs `autophagy watch` under the platform's init system. Installing it
is explicit opt-in and adds no capability the foreground command lacks.

```sh
autophagy daemon install                 # generate and load the unit
autophagy daemon install --interval 120  # custom interval
autophagy daemon status                  # is it installed and loaded?
autophagy daemon uninstall               # unload and remove the unit
```

- **macOS (launchd, first-class).** `install` writes a user agent plist to
  `~/Library/LaunchAgents/sh.autophagy.watch.plist` (with the absolute binary
  path, the chosen interval, and log paths under the app-support data directory)
  and loads it with `launchctl`. `install` prints exactly what it wrote and where.
- **Linux (systemd user unit).** `install` writes
  `${XDG_CONFIG_HOME:-~/.config}/systemd/user/autophagy-watch.service` and
  enables it with `systemctl --user`.
- **Other platforms.** `daemon` reports that lifecycle management is not supported
  yet and points you to run `autophagy watch` under your own supervisor.

Every generated unit carries a managed-by marker. Autophagy refuses to overwrite
a unit at that path it did not author, and `uninstall` refuses to delete a foreign
file — it only removes the unit it wrote, leaving nothing behind.

## Logs

The daemon writes standard output and error to the app-support data directory:

- macOS: `~/Library/Application Support/sh.autophagy.Autophagy/watch.log` and
  `watch.err.log`.
- Linux: `~/.local/share/autophagy/watch.log` and `watch.err.log`.

`daemon status` shows the last log line when it can read one cheaply.

## Privacy

Watch and the daemon apply the same privacy gates as manual import (see
`privacy-and-lifecycle.md`): secrets are redacted at ingestion, path exclusions
are honoured, and native prompt/response text is omitted unless you pass
`--include-content`. The daemon only ingests; it never executes or installs
anything, and no data is sent off the machine. `daemon uninstall` removes the unit
file completely.
