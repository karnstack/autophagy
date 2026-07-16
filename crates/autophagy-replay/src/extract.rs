use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use autophagy_events::Event;
use autophagy_mutations::MutationPackage;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    CounterfactualOutcome, DecisionPoint, ExpectedAction, ReplayScenarioSpecVersion, ReplaySuite,
    ReplaySuiteSpecVersion,
};

#[derive(Default)]
struct EvidenceGroup<'a> {
    supporting: Vec<&'a Event>,
    counterexamples: Vec<&'a Event>,
}

/// Derive a reviewable Replay Suite v0.1 draft from exact mutation evidence.
///
/// Supporting and counterexample events become session-scoped decision points.
/// A bounded event window adds nearby structural context, but positive
/// counterfactual outcomes remain `unknown` until a reviewer labels them.
///
/// # Errors
/// Returns an error when evidence is missing, one event has contradictory
/// classifications, or extraction cannot form a valid replay suite.
pub fn extract_review_draft(
    package: &MutationPackage,
    events: &[Event],
    context_events: usize,
) -> Result<ReplaySuite, ReplayDraftError> {
    let supporting_ids = package
        .hypothesis
        .supporting_event_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let counterexample_ids = package
        .hypothesis
        .counterexample_event_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let overlapping_ids = supporting_ids
        .intersection(&counterexample_ids)
        .copied()
        .collect::<Vec<_>>();
    if !overlapping_ids.is_empty() {
        return Err(ReplayDraftError::ConflictingEvidenceIds(
            overlapping_ids.join(", "),
        ));
    }

    let by_id = events
        .iter()
        .map(|event| (event.event_id.as_str(), event))
        .collect::<BTreeMap<_, _>>();
    let missing = supporting_ids
        .iter()
        .chain(&counterexample_ids)
        .filter(|event_id| !by_id.contains_key(**event_id))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ReplayDraftError::MissingEvidence(missing.join(", ")));
    }

    let mut groups: BTreeMap<&str, EvidenceGroup<'_>> = BTreeMap::new();
    for event_id in &supporting_ids {
        let event = by_id[event_id];
        groups
            .entry(event.session_id.as_str())
            .or_default()
            .supporting
            .push(event);
    }
    for event_id in &counterexample_ids {
        let event = by_id[event_id];
        groups
            .entry(event.session_id.as_str())
            .or_default()
            .counterexamples
            .push(event);
    }
    if groups.is_empty() {
        return Err(ReplayDraftError::NoEvidence);
    }

    let mut session_events: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in events {
        session_events
            .entry(event.session_id.as_str())
            .or_default()
            .push(event);
    }
    for session in session_events.values_mut() {
        session.sort_by(|left, right| event_order(left, right));
    }

    let trigger_selectors = package
        .triggers
        .iter()
        .map(|trigger| trigger.selector.clone())
        .collect::<BTreeSet<_>>();
    let mut scenarios = Vec::with_capacity(groups.len());
    for (session_id, group) in groups {
        let session = session_events
            .get(session_id)
            .ok_or_else(|| ReplayDraftError::MissingEvidence(session_id.to_owned()))?;
        append_group_scenarios(
            &mut scenarios,
            package,
            session_id,
            group,
            session,
            context_events,
            &trigger_selectors,
        );
    }
    scenarios.sort_by(|left, right| left.scenario_id.cmp(&right.scenario_id));
    let suite = ReplaySuite {
        spec_version: ReplaySuiteSpecVersion::V0_1,
        mutation_id: package.mutation_id.clone(),
        scenarios,
    };
    suite.validate().map_err(ReplayDraftError::InvalidDraft)?;
    Ok(suite)
}

fn append_group_scenarios(
    scenarios: &mut Vec<DecisionPoint>,
    package: &MutationPackage,
    session_id: &str,
    group: EvidenceGroup<'_>,
    session: &[&Event],
    context_events: usize,
    trigger_selectors: &BTreeSet<String>,
) {
    let mixed_classification = !group.supporting.is_empty() && !group.counterexamples.is_empty();
    let effective_context = if mixed_classification {
        0
    } else {
        context_events
    };
    if !group.supporting.is_empty() {
        scenarios.push(build_scenario(
            package,
            session_id,
            EvidenceGroup {
                supporting: group.supporting,
                counterexamples: Vec::new(),
            },
            session,
            effective_context,
            trigger_selectors,
        ));
    }
    if !group.counterexamples.is_empty() {
        scenarios.push(build_scenario(
            package,
            session_id,
            EvidenceGroup {
                supporting: Vec::new(),
                counterexamples: group.counterexamples,
            },
            session,
            effective_context,
            trigger_selectors,
        ));
    }
}

fn build_scenario(
    package: &MutationPackage,
    session_id: &str,
    mut group: EvidenceGroup<'_>,
    session: &[&Event],
    context_events: usize,
    trigger_selectors: &BTreeSet<String>,
) -> DecisionPoint {
    group
        .supporting
        .sort_by(|left, right| event_order(left, right));
    group
        .counterexamples
        .sort_by(|left, right| event_order(left, right));
    let (evidence, expected_action) = if group.supporting.is_empty() {
        (&group.counterexamples, ExpectedAction::NoOp)
    } else {
        (&group.supporting, ExpectedAction::Intervene)
    };
    let evidence_ids = evidence
        .iter()
        .map(|event| event.event_id.as_str())
        .collect::<BTreeSet<_>>();
    let selected = context_window(session, &evidence_ids, context_events);
    let source_event_ids = selected
        .iter()
        .map(|event| event.event_id.as_str().to_owned())
        .collect::<Vec<_>>();
    let observed_trigger_selectors = match expected_action {
        ExpectedAction::Intervene => trigger_selectors.iter().cloned().collect(),
        ExpectedAction::NoOp => evidence
            .iter()
            .map(|event| event_selector(event))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    let context_kinds = selected
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>()
        .join(" -> ");
    let nearby_count = selected.len().saturating_sub(evidence.len());
    DecisionPoint {
        spec_version: ReplayScenarioSpecVersion::V0_1,
        scenario_id: scenario_id(
            &package.mutation_id,
            session_id,
            expected_action,
            &source_event_ids,
        ),
        source_event_ids,
        observed_trigger_selectors,
        expected_action,
        counterfactual_outcome: (expected_action == ExpectedAction::Intervene)
            .then_some(CounterfactualOutcome::Unknown),
        note: Some(scenario_note(
            expected_action,
            evidence.len(),
            nearby_count,
            session_id,
            &context_kinds,
        )),
    }
}

fn scenario_note(
    expected_action: ExpectedAction,
    evidence_count: usize,
    nearby_count: usize,
    session_id: &str,
    context_kinds: &str,
) -> String {
    match expected_action {
        ExpectedAction::Intervene => format!(
            "Review draft from {evidence_count} exact evidence event(s) and {nearby_count} nearby event(s) in session {session_id}. Context: {context_kinds}. Set counterfactual_outcome to expected_result or contradiction before evaluation."
        ),
        ExpectedAction::NoOp => format!(
            "Review draft from {evidence_count} exact counterexample event(s) and {nearby_count} nearby event(s) in session {session_id}. Context: {context_kinds}. Confirm this remains a no-op case before evaluation."
        ),
    }
}

fn event_order(left: &Event, right: &Event) -> std::cmp::Ordering {
    left.timestamp
        .cmp(&right.timestamp)
        .then_with(|| left.sequence.cmp(&right.sequence))
        .then_with(|| left.event_id.as_str().cmp(right.event_id.as_str()))
}

fn context_window<'a>(
    session: &[&'a Event],
    evidence_ids: &BTreeSet<&str>,
    radius: usize,
) -> Vec<&'a Event> {
    let mut selected = BTreeSet::new();
    for (index, event) in session.iter().enumerate() {
        if evidence_ids.contains(event.event_id.as_str()) {
            let start = index.saturating_sub(radius);
            let end = index.saturating_add(radius).min(session.len() - 1);
            selected.extend(start..=end);
        }
    }
    selected.into_iter().map(|index| session[index]).collect()
}

fn event_selector(event: &Event) -> String {
    let mut selector = format!("event/v1|type:{}", event.kind.as_str());
    if let Some(tool) = &event.tool {
        write!(selector, "|tool:{}", normalize(&tool.name)).expect("writing to String cannot fail");
        if let Some(exit_code) = tool.exit_code {
            write!(selector, "|exit:{exit_code}").expect("writing to String cannot fail");
        }
    }
    if let Some(classification) = [
        "autophagy.signature",
        "correction_signature",
        "correction_key",
    ]
    .iter()
    .find_map(|key| event.metadata.get(*key).and_then(Value::as_str))
    {
        write!(selector, "|classification:{}", normalize(classification))
            .expect("writing to String cannot fail");
    }
    selector
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn scenario_id(
    mutation_id: &str,
    session_id: &str,
    expected_action: ExpectedAction,
    source_event_ids: &[String],
) -> String {
    let action = match expected_action {
        ExpectedAction::Intervene => "intervene",
        ExpectedAction::NoOp => "no_op",
    };
    let digest = Sha256::digest(
        format!(
            "replay-draft/v1\0{mutation_id}\0{session_id}\0{action}\0{}",
            source_event_ids.join("\0")
        )
        .as_bytes(),
    );
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("rps_{encoded}")
}

/// Error produced while deriving an evidence-linked replay review draft.
#[derive(Debug, thiserror::Error)]
pub enum ReplayDraftError {
    /// The package did not retain any supporting or counterexample evidence.
    #[error("mutation package has no evidence to extract")]
    NoEvidence,
    /// One or more package evidence IDs were absent from the supplied event set.
    #[error("mutation evidence is missing from the supplied events: {0}")]
    MissingEvidence(String),
    /// The same event was classified as both supporting and contradictory.
    #[error("mutation evidence IDs have conflicting classifications: {0}")]
    ConflictingEvidenceIds(String),
    /// The derived suite violated Replay Suite v0.1 invariants.
    #[error("derived replay draft is invalid: {0}")]
    InvalidDraft(crate::ReplayValidationErrors),
}
