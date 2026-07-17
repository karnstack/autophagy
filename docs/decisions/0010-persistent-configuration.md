# ADR 0010: Persistent configuration, precedence, and versioning

- Status: accepted
- Date: 2026-07-17

## Context

Every tunable in the CLI was a per-command flag with no persistence: the adapter
set, indexing and content gates, detector thresholds, the watch interval, and
the synthesis provider all had to be retyped on every invocation. First-time
setup was a single guided command, but changing anything afterwards meant
remembering the exact flags again, and nothing recorded what a user had chosen.
The product goal is that initial setup *and* every later change are dead easy,
without giving up the full power of the flags.

This needs three decisions: where configuration lives, how it composes with
flags, and how it is versioned for forward compatibility.

## Decision

### A single TOML file next to the database

Configuration is one TOML file, `config.toml`, in the platform-local
application-support directory resolved by `ProjectDirs` for
`sh.autophagy.Autophagy` — the same directory the default database lives in
(`~/Library/Application Support/sh.autophagy.Autophagy/` on macOS,
`$XDG_DATA_HOME/…` on Linux).

The config location is **fixed to that directory and does not move with
`--database`.** A database override is frequently a throwaway path (`/tmp`, a
fixture, a copy for inspection); tying config to it would scatter or lose
settings unpredictably. A stable, predictable location is worth more than
mirroring an ad-hoc database path. Tests and advanced users can relocate the
file with the `AUTOPHAGY_CONFIG_DIR` environment variable, which is also how the
test suite stays hermetic.

The schema is a versioned header (`config_version`) plus four sections:

- `[import]` — `adapters`, `index_tool_input`, `include_content`,
  `index_metadata`, `exclude_paths`
- `[detect]` — `min_occurrences`, `min_sessions`, `min_support_ratio_bps`
- `[watch]` — `interval_seconds`
- `[synthesis]` — `provider`, `manifest_path`

The file **never contains secrets.** None of these keys are secret, and provider
API-key environment-variable names stay in the synthesis manifest, not here.

### Precedence: default < config file < explicit flag

Effective values compose in one fixed order: a built-in default, overridden by
the config file, overridden by an explicitly-passed CLI flag. "Explicitly
passed" is decided by clap's `ValueSource::CommandLine`, never by comparing a
flag against its default — so a flag set to the same value as the default still
counts as an override, and the mechanism is not a heuristic. The single set of
built-in default constants is shared by clap's `default_value_t` and the config
resolver, so `config list` and the commands can never disagree about what
"default" means. Precedence is applied by `import`, `digest`, `patterns`,
`search` (where applicable), `watch`, `daemon install`, `reindex`, `setup`, and
`mutations propose`/`synthesize`.

Because a bare boolean flag (`--index-tool-input`) can only express "on", config
can enable it and a flag can only strengthen it; this is inherent to store-true
flags and is documented rather than worked around with a paired `--no-` flag.

### Forward-compatible loading

A **missing** file is silent defaults (unchanged behavior). **Unknown** sections
and keys, and a `config_version` newer than the running binary, **warn** on
stderr and are otherwise ignored, so a newer build's file does not break an older
one. Only a **malformed** file (bad TOML syntax, or a known key with the wrong
type or an out-of-range value) is a hard error, reported with the offending key
named. `config set`/`unset` preserve unknown keys already on disk, so an older
binary editing one value does not destroy a newer binary's settings.

### `status` and rerunnable `setup`

Two commands make the state legible and mutable:

- `autophagy status` is a fast, read-only snapshot: database path/size/schema
  version, row counts, per-adapter import freshness, index and daemon state, the
  detector thresholds in effect, findings and candidates-by-state, and the config
  path. It is COUNT-style queries plus one deterministic detection pass, and
  works against an empty database and with no config file.
- `autophagy setup` is now rerunnable: when a config exists it pre-fills the
  current values as prompt defaults, writes the chosen values back to config on
  completion, and applies consequences — a newly enabled `index_tool_input`
  triggers an in-place `reindex`, and a changed adapter set or interval offers to
  reinstall an already-installed daemon through the existing reversible daemon
  seam. It is still never destructive, and first-run behavior is unchanged except
  that it now also writes the config file.

### The daemon keeps explicit, baked-in arguments

The generated launchd/systemd unit still lists explicit arguments so the unit is
deterministic and self-describing. Those arguments are **derived from config at
install time**; a config change that affects the daemon therefore requires
`autophagy daemon install` again. `setup` offers this automatically, and `status`
labels the interval it reports as the *configured* interval to make the reinstall
requirement clear.

## Consequences

- The config lives in the CLI layer only: a new `config.rs` module in
  `autophagy-cli`, no store migration and no new crate. The store gains two
  read-only aggregate queries (`adapter_activity`, `mutation_state_counts`) for
  `status`.
- `mutations synthesize --manifest` became optional, falling back to
  `synthesis.manifest_path`; omitting both is a precise error.
- The config file is CLI surface, not a wire or stored-schema contract, but it is
  versioned (`config_version = 1`) so future layout changes are explicit.
- Tests pin `AUTOPHAGY_CONFIG_DIR` so they never read a developer's real config;
  precedence, round-trip, validation, status, and rerunnable-setup paths are
  covered.
