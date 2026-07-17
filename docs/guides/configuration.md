# Configuration and status

Autophagy remembers your choices so you do not retype flags. One TOML file holds
your defaults; `autophagy status` shows what is set up right now; and `autophagy
setup` can be re-run any time to change things through prompts.

## Where the config lives

Configuration is a single file, `config.toml`, in the platform-local
application-support directory — the same folder the default database lives in:

- macOS: `~/Library/Application Support/sh.autophagy.Autophagy/config.toml`
- Linux: `$XDG_DATA_HOME/autophagy/config.toml` (or `~/.local/share/…`)

Print the exact path any time:

```sh
autophagy config path
```

The location is **fixed** and does **not** move when you pass `--database`. A
database override is often a throwaway path; keeping config in one predictable
place is deliberate. To relocate the file for tests or sandboxing, set
`AUTOPHAGY_CONFIG_DIR` to a directory — the file is then `config.toml` inside it.

The file never contains secrets. Provider API-key environment-variable names
live in the synthesis manifest, not here.

## Precedence: defaults, then config, then flags

Every configurable value is resolved in one fixed order:

1. the built-in default,
2. overridden by the config file,
3. overridden by an explicit flag on the command.

An explicit flag always wins for that one run, even if it happens to match the
default. For example, with `detect.min_occurrences = 5` in config:

```sh
autophagy digest                        # uses 5 (from config)
autophagy digest --min-occurrences 2    # uses 2 (flag wins this run)
```

A bare on/off flag such as `--index-tool-input` can only turn a setting *on*, so
config can enable it and a flag can only reinforce it. To turn such a setting
off, change the config value.

## The keys

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `import.adapters` | list | all native adapters | Which agents `watch`, `daemon`, and `setup` cover. |
| `import.index_tool_input` | bool | `false` | Make the commands your agents ran searchable. |
| `import.include_content` | bool | `false` | Persist prompt/response/tool-result text. |
| `import.index_metadata` | list | empty | Already-redacted metadata keys to index. |
| `import.exclude_paths` | list | empty | Globs to exclude from import and index. |
| `detect.min_occurrences` | integer | `3` | Minimum supporting events for a finding. |
| `detect.min_sessions` | integer | `2` | Minimum distinct sessions for a finding. |
| `detect.min_support_ratio_bps` | integer (0–10000) | `0` | Optional anti-noise support-share floor. |
| `watch.interval_seconds` | integer | `60` | Discovery interval for watch and the daemon. |
| `synthesis.provider` | `deterministic` \| `ollama` \| `openai-compatible` | `deterministic` | Default synthesis provider. |
| `synthesis.manifest_path` | path | none | Default manifest for `mutations synthesize`. |

## Reading and writing config

```sh
autophagy config list                              # every effective value + source
autophagy config get detect.min_occurrences        # one value
autophagy config set detect.min_occurrences 5      # validated, typed write
autophagy config set import.adapters claude-code,codex   # lists are comma-separated
autophagy config unset detect.min_occurrences      # revert to the built-in default
autophagy config edit                              # open $EDITOR, then validate
```

`config list` annotates each value with its source — `[config]` when it comes
from the file, `[default]` when nothing overrides the built-in. `config set`
validates types and ranges and rejects unknown keys; it also preserves any keys
it does not recognize, so a newer build's settings survive an older build editing
one value. `config edit` opens the file in `$VISUAL`/`$EDITOR` (falling back to
`vi`), re-validates the result on save, and — if your edit is invalid — keeps the
broken copy aside and restores the previous good file so the tool keeps working.

Unknown sections/keys and a `config_version` from a newer build produce a
warning, not an error. Only genuinely malformed TOML (or a wrong-typed value) is
an error, and it names the offending key.

`config set`, `config unset`, and `config edit` rewrite the file through the
TOML value model, so **comments and key ordering are not preserved** — the file
is re-serialized in a normalized layout. Unknown keys and your values survive;
hand-written comments do not.

## `autophagy status`

A fast, read-only snapshot of local state — safe against an empty database and
with no config file:

```sh
autophagy status
autophagy status --with-findings   # also count findings (slower on large stores)
autophagy status --output json
```

It reports:

- the database path, size, and schema version, plus event/session/source counts;
- per-adapter import activity and how fresh each adapter's last import is;
- the search index state (signatures, whether commands are searchable);
- whether the background daemon unit is present and loaded, and its configured
  interval;
- the detector thresholds in effect and how many mutation candidates exist in
  each state;
- the config file path and whether a file is present.

By default `status` uses only fast COUNT-style queries. Counting deterministic
findings requires loading every event and running a full detection pass — the
same cost as `digest` — so it is opt-in with `--with-findings` and omitted
otherwise.

## Changing things later with `setup`

`autophagy setup` is rerunnable. When a config already exists it shows your
current answers as the defaults, writes your choices back to config, and applies
the consequences:

- if you newly enable indexing, it rebuilds the search index in place with
  `reindex` (a reimport alone cannot make already-stored events searchable);
- if you change the adapter set or interval and a daemon is installed, it offers
  to reinstall it so the change takes effect.

It finishes with a short summary of what changed. It never deletes anything.

## The daemon and config changes

The background daemon's launchd/systemd unit bakes in explicit arguments at
install time, derived from your config then. A config change that affects the
daemon — the adapter set or the interval — therefore needs the unit rebuilt:

```sh
autophagy daemon install
```

`setup` offers to do this for you, and `status` labels the interval it shows as
the *configured* interval to make the reinstall requirement clear. See the
[watch and daemon guide](watch-and-daemon.md) for the daemon lifecycle.
