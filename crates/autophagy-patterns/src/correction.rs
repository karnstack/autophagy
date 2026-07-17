use std::collections::BTreeMap;

use autophagy_events::{Event, EventKind, signature::SIGNATURE_SPEC_VERSION};

use crate::{
    Candidate, DetectorConfig, DetectorKind, EvidencePacket, EvidenceReference,
    EvidenceSpecVersion,
    score::{qualifies, score},
    signature::{correction_counterexample, correction_signature, finding_id},
};

pub(crate) fn detect(
    events: &[Event],
    config: DetectorConfig,
) -> (Vec<EvidencePacket>, Vec<Candidate>) {
    let mut corrections: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    let mut counterexamples: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for event in events {
        if event.kind == EventKind::UserCorrectedAgent {
            if let Some(signature) = correction_signature(event) {
                corrections.entry(signature).or_default().push(event);
            }
        } else if event.kind == EventKind::DecisionRecorded {
            if let Some(signature) = correction_counterexample(event) {
                counterexamples.entry(signature).or_default().push(event);
            }
        }
    }

    let mut findings = Vec::new();
    let mut candidates = Vec::new();
    for (signature, evidence) in corrections {
        let opposite = counterexamples
            .get(&signature)
            .map_or(&[][..], Vec::as_slice);
        let Some(recurrence) = score(&evidence, opposite) else {
            continue;
        };
        let packet_signature = format!("correction/{SIGNATURE_SPEC_VERSION}|{signature}");
        let title = format!("Repeated user correction: {signature}");
        if qualifies(&recurrence, config) {
            findings.push(EvidencePacket {
                spec_version: EvidenceSpecVersion::V0_1,
                finding_id: finding_id(
                    DetectorKind::RepeatedUserCorrection.as_str(),
                    &packet_signature,
                ),
                detector: DetectorKind::RepeatedUserCorrection,
                signature: packet_signature.clone(),
                title: title.clone(),
                score: recurrence.clone(),
                evidence: references(&evidence),
                counterexamples: references(opposite),
            });
        }
        candidates.push(Candidate {
            detector: DetectorKind::RepeatedUserCorrection,
            signature: packet_signature,
            title,
            score: recurrence,
        });
    }
    (findings, candidates)
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
