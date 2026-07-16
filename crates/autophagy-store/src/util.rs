use sha2::{Digest, Sha256};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

pub(crate) fn sha256(input: &[u8]) -> [u8; 32] {
    Sha256::digest(input).into()
}

pub(crate) fn canonical_timestamp(value: OffsetDateTime) -> Result<String, time::error::Format> {
    value.to_offset(UtcOffset::UTC).format(&Rfc3339)
}

pub(crate) fn now_timestamp() -> Result<String, time::error::Format> {
    canonical_timestamp(OffsetDateTime::now_utc())
}
