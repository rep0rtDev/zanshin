use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, PropertySet, Result};

/// Maximum length of an external identity id.
pub const MAX_EXTERNAL_ID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub id: Uuid,
    pub external_id: String,
    pub properties: PropertySet,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// Validate a client-supplied external id.
pub fn validate_external_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(Error::invalid("id must not be empty"));
    }
    if id.len() > MAX_EXTERNAL_ID_LEN {
        return Err(Error::invalid(format!(
            "id too long (max {MAX_EXTERNAL_ID_LEN} bytes)"
        )));
    }
    if id.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::invalid(
            "id must not contain whitespace or control characters",
        ));
    }
    Ok(())
}
