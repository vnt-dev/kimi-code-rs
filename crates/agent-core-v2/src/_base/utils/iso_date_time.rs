// Original: packages/agent-core-v2/src/_base/utils/isoDateTime.ts.
// The wire primitive is shared with the already migrated protocol crate.
pub use kimi_code_protocol::time::{
    IsoDateTime, IsoDateTimeError, now_iso_date_time, parse_iso_date_time,
};
