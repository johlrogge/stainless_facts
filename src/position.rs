//! Positions in a fact stream.

use serde::{Deserialize, Serialize};

/// A resumable position in a fact stream: the byte offset of the next unread
/// line.
///
/// Positions are returned by the stream writers (the position just past the
/// batch they wrote) and by the stream readers (the position just past the
/// last complete line they consumed). Store one and hand it back later to
/// resume reading exactly where you left off, with no facts skipped or
/// repeated. Unlike timestamps, which a whole batch shares, every line has
/// its own position.
///
/// A position is opaque. It can be compared, hashed and persisted (it
/// serializes as a plain integer), but it can only be obtained from this
/// crate, never built from an arbitrary number. [`StreamPosition::START`] is
/// the beginning of every stream.
///
/// A position is only meaningful for the stream it came from. Readers
/// reject a position that is past the end of the stream or not at the start
/// of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamPosition(u64);

impl StreamPosition {
    /// The beginning of a stream.
    pub const START: Self = Self(0);

    /// The position at byte `offset`.
    #[cfg_attr(not(feature = "io"), allow(dead_code))]
    pub(crate) const fn from_offset(offset: u64) -> Self {
        Self(offset)
    }

    /// The byte offset of this position.
    #[cfg_attr(not(feature = "io"), allow(dead_code))]
    pub(crate) const fn offset(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_is_offset_zero_and_the_smallest_position() {
        assert_eq!(StreamPosition::START.offset(), 0);
        assert!(StreamPosition::START < StreamPosition::from_offset(1));
    }

    #[test]
    fn serializes_as_a_plain_integer() {
        let position = StreamPosition::from_offset(42);
        let json = serde_json::to_string(&position).unwrap();
        assert_eq!(json, "42");
        assert_eq!(
            serde_json::from_str::<StreamPosition>(&json).unwrap(),
            position
        );
    }
}
