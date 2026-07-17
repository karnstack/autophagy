use std::collections::{BTreeMap, BTreeSet};

use autophagy_events::{Event, EventKind};

use crate::{
    Candidate, DetectorConfig, DetectorKind, EvidencePacket, EvidenceReference,
    EvidenceSpecVersion,
    score::{qualifies, score},
    signature::{FailureOperation, failure_operation, finding_id},
};

struct RecoveryOccurrence<'a> {
    failure: &'a Event,
    recovery: &'a Event,
    recovered: &'a Event,
}

struct DirectRecovery<'a> {
    failure: &'a Event,
    recovered: &'a Event,
}

struct RecoveryGroup<'a> {
    label: String,
    failure_signature: String,
    occurrences: Vec<RecoveryOccurrence<'a>>,
}

pub(crate) fn detect(
    events: &[Event],
    config: DetectorConfig,
) -> (Vec<EvidencePacket>, Vec<Candidate>) {
    let mut sessions: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in events {
        sessions
            .entry(event.session_id.as_str())
            .or_default()
            .push(event);
    }
    let mut groups: BTreeMap<String, RecoveryGroup<'_>> = BTreeMap::new();
    let mut direct: BTreeMap<String, Vec<DirectRecovery<'_>>> = BTreeMap::new();
    for session in sessions.values_mut() {
        session.sort_by(|left, right| {
            left.timestamp
                .cmp(&right.timestamp)
                .then_with(|| left.sequence.cmp(&right.sequence))
                .then_with(|| left.event_id.as_str().cmp(right.event_id.as_str()))
        });
        collect_session(session, &mut groups, &mut direct);
    }
    let mut findings = Vec::new();
    let mut candidates = Vec::new();
    for (signature, group) in groups {
        let support_anchors = group
            .occurrences
            .iter()
            .map(|occurrence| occurrence.recovery)
            .collect::<Vec<_>>();
        let direct_recoveries = direct
            .get(&group.failure_signature)
            .map_or(&[][..], Vec::as_slice);
        let counterexample_anchors = direct_recoveries
            .iter()
            .map(|occurrence| occurrence.recovered)
            .collect::<Vec<_>>();
        let Some(recurrence) = score(&support_anchors, &counterexample_anchors) else {
            continue;
        };
        let title = format!("Repeated successful recovery: {}", group.label);
        if qualifies(&recurrence, config) {
            findings.push(EvidencePacket {
                spec_version: EvidenceSpecVersion::V0_1,
                finding_id: finding_id(
                    DetectorKind::RepeatedSuccessfulRecovery.as_str(),
                    &signature,
                ),
                detector: DetectorKind::RepeatedSuccessfulRecovery,
                signature: signature.clone(),
                title: title.clone(),
                score: recurrence.clone(),
                evidence: recovery_references(&group.occurrences),
                counterexamples: direct_references(direct_recoveries),
            });
        }
        candidates.push(Candidate {
            detector: DetectorKind::RepeatedSuccessfulRecovery,
            signature,
            title,
            score: recurrence,
        });
    }
    (findings, candidates)
}

fn collect_session<'a>(
    session: &[&'a Event],
    groups: &mut BTreeMap<String, RecoveryGroup<'a>>,
    direct: &mut BTreeMap<String, Vec<DirectRecovery<'a>>>,
) {
    for (recovered_index, recovered) in session.iter().enumerate() {
        if recovered.kind != EventKind::ToolCompleted {
            continue;
        }
        let Some(target) = failure_operation(recovered, false) else {
            continue;
        };
        let Some((failure_index, failure, target_failure)) =
            preceding_failure(&session[..recovered_index], &target.success_key)
        else {
            continue;
        };
        let recovery = session[failure_index + 1..recovered_index]
            .iter()
            .rev()
            .find_map(|event| {
                (event.kind == EventKind::ToolCompleted)
                    .then(|| failure_operation(event, false))
                    .flatten()
                    .filter(|operation| operation.success_key != target.success_key)
                    .map(|operation| (*event, operation))
            });
        if let Some((recovery, recovery_operation)) = recovery {
            let signature = recovery_signature(&target_failure, &recovery_operation);
            groups
                .entry(signature)
                .or_insert_with(|| RecoveryGroup {
                    label: format!("{} before {}", recovery_operation.label, target.label),
                    failure_signature: target_failure.signature.clone(),
                    occurrences: Vec::new(),
                })
                .occurrences
                .push(RecoveryOccurrence {
                    failure,
                    recovery,
                    recovered,
                });
        } else {
            direct
                .entry(target_failure.signature)
                .or_default()
                .push(DirectRecovery { failure, recovered });
        }
    }
}

fn preceding_failure<'a>(
    preceding: &[&'a Event],
    success_key: &str,
) -> Option<(usize, &'a Event, FailureOperation)> {
    for (index, event) in preceding.iter().enumerate().rev() {
        if event.kind == EventKind::ToolCompleted
            && failure_operation(event, false)
                .is_some_and(|operation| operation.success_key == success_key)
        {
            return None;
        }
        if event.kind == EventKind::ToolFailed {
            let Some(operation) = failure_operation(event, true) else {
                continue;
            };
            if operation.success_key == success_key {
                return Some((index, event, operation));
            }
        }
    }
    None
}

fn recovery_signature(failure: &FailureOperation, recovery: &FailureOperation) -> String {
    format!(
        "recovery/v1|{}|via|{}",
        failure
            .signature
            .strip_prefix("failure/v1|")
            .unwrap_or(&failure.signature),
        recovery
            .success_key
            .strip_prefix("operation/v1|")
            .unwrap_or(&recovery.success_key)
    )
}

fn recovery_references(occurrences: &[RecoveryOccurrence<'_>]) -> Vec<EvidenceReference> {
    references(occurrences.iter().flat_map(|occurrence| {
        [
            occurrence.failure,
            occurrence.recovery,
            occurrence.recovered,
        ]
    }))
}

fn direct_references(occurrences: &[DirectRecovery<'_>]) -> Vec<EvidenceReference> {
    references(
        occurrences
            .iter()
            .flat_map(|occurrence| [occurrence.failure, occurrence.recovered]),
    )
}

fn references<'a>(events: impl Iterator<Item = &'a Event>) -> Vec<EvidenceReference> {
    let mut seen = BTreeSet::new();
    let mut references = events
        .filter(|event| seen.insert(event.event_id.as_str()))
        .map(EvidenceReference::from_event)
        .collect::<Vec<_>>();
    references.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    references
}
