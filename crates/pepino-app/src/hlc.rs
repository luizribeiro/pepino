use chrono::{DateTime, Datelike, SecondsFormat};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
}

impl Hlc {
    /// Renders the wall time in UTC with millisecond precision.
    ///
    /// Returns `None` for HLC values outside the RFC 3339 year range.
    pub fn utc(self) -> Option<String> {
        let time = DateTime::from_timestamp_millis(self.wall_ms.try_into().ok()?)?;
        (time.year() <= 9999).then(|| time.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut hlc = serializer.serialize_struct("Hlc", 3)?;
        hlc.serialize_field("wall_ms", &self.wall_ms)?;
        hlc.serialize_field("counter", &self.counter)?;
        hlc.serialize_field("utc", &self.utc())?;
        hlc.end()
    }
}

impl From<pepino_core::Hlc> for Hlc {
    fn from(value: pepino_core::Hlc) -> Self {
        Self {
            wall_ms: value.wall_ms,
            counter: value.counter,
        }
    }
}

impl From<Hlc> for pepino_core::Hlc {
    fn from(value: Hlc) -> Self {
        Self {
            wall_ms: value.wall_ms,
            counter: value.counter,
        }
    }
}
