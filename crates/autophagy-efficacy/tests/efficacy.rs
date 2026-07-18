//! Deterministic evaluation math, verdict thresholds, and schema conformance.

use std::collections::BTreeSet;

use autophagy_efficacy::{
    CoverageInput, EfficacyObservations, EfficacyReport, FailureOccurrence, InsufficientReason,
    MatchingRule, Verdict, WindowBounds, evaluate,
};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const RESULT_SCHEMA: &str = include_str!("../../../docs/specs/efficacy/0.1/result.schema.json");
const VALID: &[&str] = &[
    include_str!("../../../docs/specs/efficacy/0.1/valid/improved.json"),
    include_str!("../../../docs/specs/efficacy/0.1/valid/regressed_no_baseline.json"),
    include_str!("../../../docs/specs/efficacy/0.1/valid/insufficient_data.json"),
    include_str!("../../../docs/specs/efficacy/0.1/valid/selector_grammar_mismatch.json"),
];
const INVALID: &[&str] = &[
    include_str!("../../../docs/specs/efficacy/0.1/invalid/bad_spec_version.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/bad_efficacy_id.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/unknown_verdict.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/bad_matching_rule.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/model_used_true.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/unknown_field.json"),
    include_str!("../../../docs/specs/efficacy/0.1/invalid/bad_insufficient_reason.json"),
];

fn at(timestamp: &str) -> OffsetDateTime {
    OffsetDateTime::parse(timestamp, &Rfc3339).expect("timestamp")
}

fn occurrence(event_id: &str, session_id: &str, timestamp: &str) -> FailureOccurrence {
    FailureOccurrence {
        event_id: event_id.to_owned(),
        session_id: session_id.to_owned(),
        occurred_at: at(timestamp),
    }
}

fn observations(
    occurrences: Vec<FailureOccurrence>,
    coverage: CoverageInput,
) -> EfficacyObservations {
    // Default: a current-grammar (v2) selector against a v2 index — no mismatch.
    selectors_against_index(
        vec!["failure/v2|shell|go build|exit:1".to_owned()],
        occurrences,
        coverage,
        BTreeSet::from([2]),
    )
}

fn selectors_against_index(
    signature_selectors: Vec<String>,
    occurrences: Vec<FailureOccurrence>,
    coverage: CoverageInput,
    index_grammar_versions: BTreeSet<u32>,
) -> EfficacyObservations {
    EfficacyObservations {
        mutation_id: "mut_efficacy-fixture".to_owned(),
        mutation_version: "0.1.0".to_owned(),
        signature_selectors,
        matching_rule: MatchingRule::FailureSignatureRecurrence,
        occurrences,
        coverage,
        index_grammar_versions,
    }
}

#[test]
fn improvement_is_detected_with_a_negative_rate_delta() {
    // Three failures in the pre-window, none after install: a clear reduction.
    let occurrences = vec![
        occurrence("evt_p1", "ses_a", "2026-01-10T00:00:00Z"),
        occurrence("evt_p2", "ses_b", "2026-02-01T00:00:00Z"),
        occurrence("evt_p3", "ses_b", "2026-03-01T00:00:00Z"),
    ];
    let installed_at = at("2026-04-01T00:00:00Z");
    let now = at("2026-07-01T00:00:00Z");
    let report = evaluate(
        &observations(
            occurrences,
            CoverageInput {
                classifiable_failures: 50,
                total_failures: 50,
            },
        ),
        installed_at,
        now,
    )
    .expect("evaluate");

    assert_eq!(report.verdict, Verdict::Improved);
    assert!(report.insufficient_reasons.is_empty());
    assert_eq!(report.windows.pre.occurrences, 3);
    assert_eq!(report.windows.pre.distinct_sessions, 2);
    assert_eq!(report.windows.post.occurrences, 0);
    assert_eq!(report.rate_delta_bps, Some(-10_000));
    assert_eq!(report.evidence.pre_event_count, 3);
    assert_eq!(report.evidence.post_event_count, 0);
    assert!(!report.model_used);
    assert!(report.efficacy_id.starts_with("eff_"));
}

#[test]
fn a_new_recurrence_after_install_regresses_without_a_baseline() {
    let occurrences = vec![
        occurrence("evt_q1", "ses_a", "2026-05-01T00:00:00Z"),
        occurrence("evt_q2", "ses_a", "2026-06-01T00:00:00Z"),
    ];
    let report = evaluate(
        &observations(
            occurrences,
            CoverageInput {
                classifiable_failures: 20,
                total_failures: 20,
            },
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-01T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::Regressed);
    assert_eq!(report.rate_delta_bps, None);
    assert_eq!(report.windows.pre.occurrences, 0);
    assert_eq!(report.windows.post.occurrences, 2);
}

#[test]
fn a_small_change_within_the_band_is_unchanged() {
    // 4 pre, 4 post — identical rate.
    let mut occurrences = Vec::new();
    for (index, day) in ["2026-01-05", "2026-01-15", "2026-02-05", "2026-02-15"]
        .into_iter()
        .enumerate()
    {
        occurrences.push(occurrence(
            &format!("evt_pre{index}"),
            "ses_pre",
            &format!("{day}T00:00:00Z"),
        ));
    }
    for (index, day) in ["2026-04-05", "2026-04-15", "2026-05-05", "2026-05-15"]
        .into_iter()
        .enumerate()
    {
        occurrences.push(occurrence(
            &format!("evt_post{index}"),
            "ses_post",
            &format!("{day}T00:00:00Z"),
        ));
    }
    let report = evaluate(
        &observations(
            occurrences,
            CoverageInput {
                classifiable_failures: 30,
                total_failures: 30,
            },
        ),
        at("2026-03-01T00:00:00Z"),
        at("2026-06-01T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::Unchanged);
    assert_eq!(report.rate_delta_bps, Some(0));
}

#[test]
fn a_short_post_window_is_insufficient() {
    let occurrences = vec![
        occurrence("evt_r1", "ses_a", "2026-06-25T00:00:00Z"),
        occurrence("evt_r2", "ses_a", "2026-07-02T00:00:00Z"),
    ];
    // Post window is only three days.
    let report = evaluate(
        &observations(
            occurrences,
            CoverageInput {
                classifiable_failures: 10,
                total_failures: 10,
            },
        ),
        at("2026-06-30T00:00:00Z"),
        at("2026-07-03T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::InsufficientData);
    assert!(
        report
            .insufficient_reasons
            .contains(&InsufficientReason::PostWindowTooShort)
    );
}

#[test]
fn partial_index_coverage_forces_insufficient_data() {
    let occurrences = vec![
        occurrence("evt_s1", "ses_a", "2026-01-10T00:00:00Z"),
        occurrence("evt_s2", "ses_a", "2026-02-10T00:00:00Z"),
        occurrence("evt_s3", "ses_a", "2026-03-10T00:00:00Z"),
    ];
    // Only 4 of 40 in-span failures are classifiable: 1000 bps, well below 5000.
    let report = evaluate(
        &observations(
            occurrences,
            CoverageInput {
                classifiable_failures: 4,
                total_failures: 40,
            },
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-01T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::InsufficientData);
    assert!(
        report
            .insufficient_reasons
            .contains(&InsufficientReason::PartialIndexCoverage)
    );
    assert_eq!(report.coverage.coverage_bps, 1_000);
    assert!(!report.coverage.complete);
}

#[test]
fn a_v1_selector_against_a_v2_index_is_a_grammar_mismatch() {
    // The real defect: the mutation's trigger selector is grammar v1, but the
    // index was re-minted to v2, so the v1 operation key matches zero rows. The
    // eight pre-install failures exist — they are simply indexed under v2 — so
    // "0 → 0, no prior baseline" would be misleading.
    let report = evaluate(
        &selectors_against_index(
            vec!["failure/v1|shell|cd zuzoto && go build ./... 2>&1|exit:1".to_owned()],
            Vec::new(),
            CoverageInput {
                classifiable_failures: 161,
                total_failures: 200,
            },
            BTreeSet::from([2]),
        ),
        at("2026-07-18T00:00:00Z"),
        at("2026-11-16T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::InsufficientData);
    assert!(
        report
            .insufficient_reasons
            .contains(&InsufficientReason::SelectorGrammarMismatch)
    );
}

#[test]
fn matching_grammars_do_not_trip_the_mismatch_reason() {
    // A current-grammar selector against a current-grammar index measures
    // normally: no grammar-mismatch reason regardless of the verdict.
    let occurrences = vec![
        occurrence("evt_m1", "ses_a", "2026-01-10T00:00:00Z"),
        occurrence("evt_m2", "ses_b", "2026-02-01T00:00:00Z"),
        occurrence("evt_m3", "ses_b", "2026-03-01T00:00:00Z"),
    ];
    let report = evaluate(
        &selectors_against_index(
            vec!["failure/v2|shell|go build|exit:1".to_owned()],
            occurrences,
            CoverageInput {
                classifiable_failures: 50,
                total_failures: 50,
            },
            BTreeSet::from([2]),
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-01T00:00:00Z"),
    )
    .expect("evaluate");
    assert_eq!(report.verdict, Verdict::Improved);
    assert!(
        !report
            .insufficient_reasons
            .contains(&InsufficientReason::SelectorGrammarMismatch)
    );
}

#[test]
fn an_old_selector_still_present_in_the_index_is_not_a_mismatch() {
    // A partial or skipped reindex leaves the selector's own grammar in the
    // index: it can still match those rows, so this is measured, not flagged.
    let occurrences = vec![
        occurrence("evt_o1", "ses_a", "2026-01-10T00:00:00Z"),
        occurrence("evt_o2", "ses_b", "2026-02-01T00:00:00Z"),
    ];
    let report = evaluate(
        &selectors_against_index(
            vec!["failure/v1|shell|go build|exit:1".to_owned()],
            occurrences,
            CoverageInput {
                classifiable_failures: 40,
                total_failures: 40,
            },
            BTreeSet::from([1, 2]),
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-01T00:00:00Z"),
    )
    .expect("evaluate");
    assert!(
        !report
            .insufficient_reasons
            .contains(&InsufficientReason::SelectorGrammarMismatch)
    );
}

#[test]
fn identical_inputs_produce_an_identical_content_id() {
    let make = || {
        evaluate(
            &observations(
                vec![
                    occurrence("evt_p1", "ses_a", "2026-01-10T00:00:00Z"),
                    occurrence("evt_p2", "ses_a", "2026-02-10T00:00:00Z"),
                    occurrence("evt_p3", "ses_a", "2026-03-10T00:00:00Z"),
                ],
                CoverageInput {
                    classifiable_failures: 50,
                    total_failures: 50,
                },
            ),
            at("2026-04-01T00:00:00Z"),
            at("2026-07-01T00:00:00Z"),
        )
        .expect("evaluate")
    };
    let first = make();
    let second = make();
    assert_eq!(first.efficacy_id, second.efficacy_id);
    // A later evaluation clock yields a different content id (history, not a
    // collision): the same corpus at a new `now` is a new report.
    let later = evaluate(
        &observations(
            vec![
                occurrence("evt_p1", "ses_a", "2026-01-10T00:00:00Z"),
                occurrence("evt_p2", "ses_a", "2026-02-10T00:00:00Z"),
                occurrence("evt_p3", "ses_a", "2026-03-10T00:00:00Z"),
            ],
            CoverageInput {
                classifiable_failures: 50,
                total_failures: 50,
            },
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-02T00:00:00Z"),
    )
    .expect("evaluate");
    assert_ne!(first.efficacy_id, later.efficacy_id);
}

#[test]
fn a_backwards_clock_is_rejected() {
    let result = WindowBounds::derive(at("2026-07-01T00:00:00Z"), at("2026-06-01T00:00:00Z"));
    assert!(result.is_err());
}

#[test]
fn a_computed_report_conforms_to_its_own_schema() {
    let report = evaluate(
        &observations(
            vec![
                occurrence("evt_p1", "ses_a", "2026-01-10T00:00:00Z"),
                occurrence("evt_p2", "ses_b", "2026-02-10T00:00:00Z"),
                occurrence("evt_p3", "ses_b", "2026-03-10T00:00:00Z"),
            ],
            CoverageInput {
                classifiable_failures: 48,
                total_failures: 50,
            },
        ),
        at("2026-04-01T00:00:00Z"),
        at("2026-07-01T00:00:00Z"),
    )
    .expect("evaluate");
    let schema: Value = serde_json::from_str(RESULT_SCHEMA).expect("schema JSON");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let instance = serde_json::to_value(&report).expect("serialize report");
    assert!(
        validator.is_valid(&instance),
        "schema rejected a freshly computed report"
    );
}

#[test]
fn fixtures_round_trip_through_the_schema_and_types() {
    let schema: Value = serde_json::from_str(RESULT_SCHEMA).expect("schema JSON");
    assert_eq!(
        schema["properties"]["spec_version"]["const"],
        "efficacy/0.1"
    );
    let validator = jsonschema::validator_for(&schema).expect("compile schema");

    for fixture in VALID {
        let instance: Value = serde_json::from_str(fixture).expect("valid fixture JSON");
        assert!(validator.is_valid(&instance), "schema rejected {fixture}");
        // Every valid fixture also round-trips through the Rust type.
        let report: EfficacyReport =
            serde_json::from_str(fixture).expect("deserialize into report");
        let reserialized = serde_json::to_value(&report).expect("reserialize");
        assert!(
            validator.is_valid(&reserialized),
            "round-trip broke {fixture}"
        );
    }
    for fixture in INVALID {
        let instance: Value = serde_json::from_str(fixture).expect("invalid fixture JSON");
        assert!(!validator.is_valid(&instance), "schema accepted {fixture}");
    }
}
