//! Push notification tokens (FCM / APNs) registered by client devices.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Fcm,
    Apns,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Fcm => "fcm",
            Platform::Apns => "apns",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "fcm" => Ok(Platform::Fcm),
            "apns" => Ok(Platform::Apns),
            other => Err(Error::invalid(format!("unknown push platform {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushToken {
    pub identity_id: Uuid,
    pub platform: Platform,
    pub token: String,
    #[serde(default)]
    pub device_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub failures: i32,
}

pub fn validate_token(platform: Platform, token: &str) -> Result<()> {
    let t = token.trim();
    if t.is_empty() || t.len() > 4096 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::invalid(
            "push token must be 1..4096 non-blank characters",
        ));
    }
    if platform == Platform::Apns && !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid("APNs device tokens are hex strings"));
    }
    Ok(())
}

/// A notification ready for a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub image_url: String,
    /// Delivered as data payload; the inbox message id lets the client open it.
    pub data: serde_json::Map<String, serde_json::Value>,
    /// Rich push extras (actions, sound, badge, collapse key, deep link).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<PushOptions>,
}

/// Rich push extras shared by campaigns, direct messages and journeys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PushOptions {
    /// Action buttons (FCM: `data.actions` + click action; APNs: `category` = first action id).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<PushAction>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sound: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<i32>,
    /// Android notification channel id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_id: String,
    /// Collapse key (FCM) / `apns-collapse-id`: newer pushes replace older ones.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub collapse_key: String,
    /// Deep link opened on tap.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deep_link: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PushAction {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

impl PushOptions {
    pub fn validate(&self) -> Result<()> {
        if self.actions.len() > 4 {
            return Err(Error::invalid("at most 4 push actions"));
        }
        for a in &self.actions {
            if a.id.is_empty() || a.id.len() > 64 || a.title.is_empty() || a.title.len() > 64 {
                return Err(Error::invalid(
                    "push action: id and title must be 1..64 chars",
                ));
            }
        }
        if self.badge.is_some_and(|b| b < 0) {
            return Err(Error::invalid("push badge must be >= 0"));
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_validation() {
        assert!(validate_token(Platform::Fcm, "abc:APA91b-def_ghi").is_ok());
        assert!(validate_token(Platform::Fcm, "").is_err());
        assert!(validate_token(Platform::Fcm, "has space").is_err());
        assert!(validate_token(Platform::Apns, "0123456789abcdef").is_ok());
        assert!(validate_token(Platform::Apns, "not-hex!").is_err());
    }
}
