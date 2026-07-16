# Evidence Packet v0.1

Evidence Packet v0.1 is the versioned output of Autophagy's deterministic
detectors. The normative contract is [`schema.json`](schema.json).

Every packet contains a stable content-derived finding ID, detector and
signature versions, an inspectable integer score, exact supporting AEP event
IDs, and exact counterexample IDs. Packets contain no model-generated claims.

## Default recurrence threshold

A finding requires at least three supporting events across at least two
sessions. Support must represent at least 50% of support plus counterexamples.
The basis-point score is deterministic:

```text
score = min(
  10000,
  support_ratio_bps * 0.6
  + min(occurrences - 1, 6) * 500
  + min(distinct_sessions - 1, 4) * 750
)
```

The implementation uses integer arithmetic; the decimal expression above is
only explanatory.

## Signatures and counterexamples

Repeated command failures normalize common shell tool aliases to `shell`,
collapse command whitespace, replace the event's exact project prefix with
`$PROJECT`, and retain the non-zero exit code. A successful matching operation
is a counterexample.

Repeated user corrections require an explicit string metadata key named
`autophagy.signature`, `correction_signature`, or `correction_key`. This avoids
guessing correction semantics from private prompt text. A `decision.recorded`
event with the same signature and an explicit `followed`, `accepted`, or
`complied` outcome is a counterexample.

Repeated successful recoveries group a failed target operation, the last
different successful operation before recovery, and the target's subsequent
success. The composite sequence counts once in `score.occurrences` while all
three exact event IDs remain in `evidence`. A direct fail-to-success retry is a
counterexample sequence.
