use std::collections::BTreeMap;

use autophagy_events::{Event, EventKind};

use crate::{
    DetectorConfig, DetectorKind, EvidencePacket, EvidenceReference, EvidenceSpecVersion,
    score::{qualifies, score},
    signature::{FailureOperation, failure_operation, finding_id},
};

struct FailureGroup<'a> {
    label: String,
    success_key: String,
    events: Vec<&'a Event>,
}

pub(crate) fn detect(events: &[Event], config: DetectorConfig) -> Vec<EvidencePacket> {
    let mut failures: BTreeMap<String, FailureGroup<'_>> = BTreeMap::new();
    let mut successes: BTreeMap<String, Vec<&Event>> = BTreeMap::new();

    for event in events {
        match event.kind {
            EventKind::ToolFailed => {
                if let Some(operation) = failure_operation(event, true) {
                    add_failure(&mut failures, operation, event);
                }
            }
            EventKind::ToolCompleted => {
                if let Some(operation) = failure_operation(event, false) {
                    successes
                        .entry(operation.success_key)
                        .or_default()
                        .push(event);
                }
            }
            _ => {}
        }
    }

    failures
        .into_iter()
        .filter_map(|(signature, group)| {
            let counterexamples = successes
                .get(&group.success_key)
                .map_or(&[][..], Vec::as_slice);
            let recurrence = score(&group.events, counterexamples)?;
            qualifies(&recurrence, config).then(|| EvidencePacket {
                spec_version: EvidenceSpecVersion::V0_1,
                finding_id: finding_id(DetectorKind::RepeatedCommandFailure.as_str(), &signature),
                detector: DetectorKind::RepeatedCommandFailure,
                signature,
                title: format!("Repeated command failure: {}", truncate(&group.label, 160)),
                score: recurrence,
                evidence: references(&group.events),
                counterexamples: references(counterexamples),
            })
        })
        .collect()
}

fn add_failure<'a>(
    failures: &mut BTreeMap<String, FailureGroup<'a>>,
    operation: FailureOperation,
    event: &'a Event,
) {
    failures
        .entry(operation.signature)
        .or_insert_with(|| FailureGroup {
            label: operation.label,
            success_key: operation.success_key,
            events: Vec::new(),
        })
        .events
        .push(event);
}

fn references(events: &[&Event]) -> Vec<EvidenceReference> {
    let mut references = events
        .iter()
        .map(|event| EvidenceReference::from_event(event))
        .collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    references
}

fn truncate(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}
