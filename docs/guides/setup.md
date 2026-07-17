# Guided setup

`autophagy setup` is the way in. Instead of importing each agent by hand, wiring
up indexing flags, and remembering to start monitoring, one command detects what
is on your machine, imports what you choose, shows you the first results, and
optionally keeps watching. It is local and offline, and it never deletes
anything.

## Run it

```sh
autophagy setup
```

Attached to a terminal, setup walks you through five short steps:

1. **Detection.** For each local coding agent — Claude Code, Codex, Pi, OpenCode
   — it reports how many sessions it found and asks whether to import that one.
   Agents it cannot find are skipped with a note.
2. **Privacy.** It asks at most two questions:
   - *Make the commands your agents ran searchable?* Commands are stored locally
     either way; indexing makes them searchable and enables exact recall.
     Secrets are filtered by the redaction rules first. Recommended.
   - *Also store prompt, response, and tool-result text?* This persists more of
     the conversation for richer search. Still local, still redacted. Off by
     default.
3. **Import.** It imports each selected agent over its default history root,
   printing the events inserted per adapter.
4. **Digest.** It runs the deterministic digest immediately and shows the
   result — the repeated problems it found, or, when nothing crosses threshold,
   the diagnostics the digest reports — so there is something to see right away.
5. **Monitoring.** It offers to keep watching automatically by installing a
   background service (launchd on macOS, systemd on Linux). If you accept, it
   prints how to undo it: `autophagy daemon uninstall`.

On completion it writes your choices to the
[config file](configuration.md) so later commands inherit them, and prints a
short "what next" block pointing at `patterns`, `search`, `mutations propose`,
and the macOS app.

## Re-running setup to change things

Setup is rerunnable. Run it again any time and it pre-fills your current settings
as the defaults shown, so you only change what you want. On completion it writes
the updated config and applies the consequences:

- newly enabling indexing rebuilds the search index in place with `reindex`;
- changing the adapter set or interval while a daemon is installed offers to
  reinstall it so the change takes effect.

It reports what changed, and — like the first run — it never deletes anything.
To see the current state without changing anything, use `autophagy status`; to
change a single value without the wizard, use `autophagy config set`. Both are
covered in the [configuration guide](configuration.md).

## Non-interactive

With no terminal, setup does not prompt — it exits with a message pointing at the
flags that drive the same flow:

```sh
autophagy setup \
  --adapter claude-code \
  --index-tool-input \
  --monitor \
  --yes
```

- `--adapter <name>` restricts detection to the named adapters (repeatable);
  omit it to consider every native adapter.
- `--index-tool-input` and `--include-content` answer the two privacy questions;
  `--index-metadata <key>` indexes an already-redacted metadata key.
- `--monitor` installs background monitoring; `--interval <seconds>` sets its
  cycle interval.
- `--yes` runs without prompting, accepting the flags as given.

With `--output json`, setup prints a single structured report (adapters detected
and imported, whether an existing database was reindexed, digest counts, and
whether monitoring was installed) instead of the guided prose.

## Healing an already-imported database

If your database already has events but an empty search index — for example
history imported before signature indexing existed, or imported without
`--index-tool-input` — reimporting cannot fix it, because an identical event is
an idempotent no-op. When you approve indexing, setup detects this and rebuilds
the search index in place with `reindex` instead of a pointless reimport.

You can also run that rebuild directly:

```sh
autophagy reindex --index-tool-input
```

`reindex` rebuilds the derived search artifacts — the free-text FTS projection
and the exact normalized-signature index — from the events already stored,
applying the current redaction policy. Without `--index-tool-input` only project
paths and tool names are searchable; with it, redacted commands become
searchable and the exact-signature index is rebuilt. It is transactional and
idempotent (running it twice yields identical state) and touches only the derived
projection tables — never events, cursors, or evidence. It reports events
scanned, search rows written, signatures written, and fields redacted. See
[privacy and lifecycle](privacy-and-lifecycle.md#rebuilding-the-search-index)
for the consent semantics.

## What setup never does

Setup only adds. It does not delete sessions, drop the database, or remove
anything you have imported. Monitoring is opt-in and fully reversible with
`autophagy daemon uninstall`. Everything stays on your machine.
