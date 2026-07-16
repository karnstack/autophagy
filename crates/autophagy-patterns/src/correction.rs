use std::collections::BTreeMap;

use autophagy_events::{Event, EventKind};

use crate::{
    DetectorConfig, DetectorKind, EvidencePacket, EvidenceReference, EvidenceSpecVersion,
    score::{qualifies, score},
    signature::{correction_counterexample, correction_signature, finding_id},
};

pub(crate) fn detect(events: &[Event], config: DetectorConfig) -> Vec<EvidencePacket> {
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

    corrections
        .into_iter()
        .filter_map(|(signature, evidence)| {
            let opposite = counterexamples
                .get(&signature)
                .map_or(&[][..], Vec::as_slice);
            let recurrence = score(&evidence, opposite)?;
            let packet_signature = format!("correction/v1|{signature}");
            qualifies(&recurrence, config).then(|| EvidencePacket {
                spec_version: EvidenceSpecVersion::V0_1,
                finding_id: finding_id(
                    DetectorKind::RepeatedUserCorrection.as_str(),
                    &packet_signature,
                ),
                detector: DetectorKind::RepeatedUserCorrection,
                signature: packet_signature,
                title: format!("Repeated user correction: {signature}"),
                score: recurrence,
                evidence: references(&evidence),
                counterexamples: references(opposite),
            })
        })
        .collect()
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
