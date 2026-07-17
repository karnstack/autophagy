# Signature grammar v2

A signature is the deterministic, model-free identity that groups tool
operations for recurrence detection and exact retrieval. It is a versioned
public contract: signatures are minted into evidence packets, indexed for exact
recall, and embedded verbatim in mutation trigger selectors.

- [`schema.json`](schema.json) is normative for the selector *envelope* grammar.
- [`valid/`](valid) holds selectors the schema accepts.
- [`invalid/`](invalid) holds selectors the schema rejects.

The canonical implementation is
`autophagy_events::signature::normalize_operation`; `SIGNATURE_SPEC_VERSION`
(`v2`) is its single source of truth. See
[ADR 0014](../../../decisions/0014-signature-normalization-v2.md) for the
rationale, the real-data yield numbers, and the `v1 → v2` compatibility story.

## Selector families

Each selector is `family/v2|…`:

| Family | Shape |
| --- | --- |
| operation | `operation/v2\|<tool>\|<command>` |
| failure | `failure/v2\|<tool>\|<command>\|exit:<code>` |
| recovery | `recovery/v2\|<tool>\|<command>\|exit:<code>\|via\|<tool>\|<command>` |
| correction | `correction/v2\|<rule>` |

`<tool>` is the alias-folded tool name (`bash`/`exec`/`shell`/`terminal` →
`shell`). `<command>` is normalized command text and may itself contain `|`
(shell pipes). `<code>` is a decimal exit code. `<rule>` is a lowercased,
whitespace-collapsed user-authored correction rule.

## Command normalization (v2)

`<command>` is produced by a pure, total, idempotent pass. After folding the
tool alias, replacing the concrete project prefix with `$PROJECT`, and collapsing
whitespace runs to single spaces, volatile tokens are replaced with stable
placeholders, most specific first:

1. URLs (`scheme://…`) → `«url»`
2. Absolute POSIX paths with ≥2 segments (`/a/b…`) → `«path»`
3. Home-relative paths (`~/…`) → `«path»`
4. UUIDs (RFC 4122 `8-4-4-4-12`) → `«uuid»`
5. Hex runs of ≥8 characters containing at least one `a–f` letter → `«hex»`
6. Digit runs of ≥4 → `«n»`

Command *structure* — binaries, subcommands, flags, and shell operators — is
preserved, so `cargo test -p a` and `cargo test -p b` stay distinct while
`cd /x && go build` and `cd /y && go build` collapse to one shape. The pass
never consults a model, the clock, the locale, the filesystem, or the network,
and normalizing an already-normalized command is a fixed point.

## Compatibility with v1

`v1` selectors embedded literal command text. The grammars are intentionally
non-interoperable: a `v1` selector never matches a freshly minted `v2` signature.
Already-registered mutations keep their immutable `v1` selectors as valid audit
records; stored and indexed signatures are re-minted under `v2` by
`autophagy reindex --index-tool-input`. No historical evidence is rewritten.
