//! A callsite is the place where instrumentation is emitted from.
//!
//! Callsite information is static for a process and can be stored only once for each callsite.

use serde::{Deserialize, Serialize};

use crate::{Field, FieldName, Kind, Level};

/// An instrumented location in an application
///
/// A callsite contains all the const data for a specific location in the application where
/// instrumentation is emitted from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Callsite {
    pub callsite_id: CallsiteId,
    pub level: Level,
    pub kind: Kind,
    pub const_fields: Vec<Field>,
    pub split_field_names: Vec<FieldName>,
}

/// The callsite Id defines a unique callsite.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct CallsiteId(u64);

impl From<u64> for CallsiteId {
    /// Create a CallsiteId from a `u64` value.
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl CallsiteId {
    /// The `u64` representation of the callsite Id.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}
