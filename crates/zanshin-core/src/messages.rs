//! In-app messages: campaigns targeted at identities, delivered into a per-identity inbox.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    flags::validate_key,
    hash,
    properties::PropertyMap,
    rules::{Condition, Context},
    Error, Result,
};

pub const MAX_TEXT: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageCampaign {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
    /// Title / body may reference properties: `Hello {{custom.nickname}}`.
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default = "Condition::always")]
    pub condition: Condition,
    #[serde(default)]
    pub audiences: Vec<String>,
    #[serde(default = "default_percentage")]
    pub percentage: f64,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    /// Seconds a delivered copy stays in the inbox; `None` = until consumed.
    #[serde(default)]
    pub ttl_secs: Option<i64>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_percentage() -> f64 {
    100.0
}

impl MessageCampaign {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        self.condition.validate()?;
        if self.title.trim().is_empty() || self.title.len() > 512 {
            return Err(Error::invalid("title must be 1..512 chars"));
        }
        if self.body.len() > MAX_TEXT || self.image_url.len() > 2048 {
            return Err(Error::invalid("body or image_url too long"));
        }
        if !(0.0..=100.0).contains(&self.percentage) || self.percentage.is_nan() {
            return Err(Error::invalid("percentage must be within 0..=100"));
        }
        if let (Some(s), Some(e)) = (self.starts_at, self.ends_at) {
            if e <= s {
                return Err(Error::invalid("ends_at must be after starts_at"));
            }
        }
        if self.ttl_secs.is_some_and(|t| t <= 0) {
            return Err(Error::invalid("ttl_secs must be positive"));
        }
        Ok(())
    }

    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && self.starts_at.is_none_or(|s| now >= s)
            && self.ends_at.is_none_or(|e| now < e)
    }

    pub fn targets(
        &self,
        subject: &str,
        props: &PropertyMap,
        audiences: &dyn Fn(&str) -> bool,
    ) -> bool {
        self.condition.matches(&Context { props })
            && self.audiences.iter().all(|a| audiences(a))
            && hash::in_rollout(
                &format!("message:{}", self.key),
                "",
                subject,
                self.percentage,
            )
    }

    /// Render `{{prop.path}}` placeholders from flattened properties; unknown paths become empty.
    pub fn render(template: &str, props: &PropertyMap) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let key = after[..end].trim();
                    match props.get(key) {
                        Some(Value::String(s)) => out.push_str(s),
                        Some(Value::Null) | None => {}
                        Some(v) => out.push_str(&v.to_string()),
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}

/// A delivered copy in an identity's inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxMessage {
    pub id: Uuid,
    /// Campaign key, or `null` for direct messages.
    pub key: Option<String>,
    pub title: String,
    pub body: String,
    pub image_url: String,
    pub payload: Value,
    pub priority: i32,
    pub delivered_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_placeholders() {
        let mut p = PropertyMap::new();
        p.insert("custom.nickname".into(), json!("Neo"));
        p.insert("custom.level".into(), json!(42));
        assert_eq!(
            MessageCampaign::render(
                "Hi {{custom.nickname}}, lvl {{ custom.level }}! {{missing}}.",
                &p
            ),
            "Hi Neo, lvl 42! ."
        );
        assert_eq!(MessageCampaign::render("broken {{x", &p), "broken {{x");
    }
}
