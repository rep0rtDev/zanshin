//! Message templates: per-locale title/body in a declared format, rendered with `{{prop}}` placeholders.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    flags::validate_key, messages::MessageCampaign, properties::PropertyMap, Error, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Plaintext,
    Html,
    Json,
}

impl Format {
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Plaintext => "plaintext",
            Format::Html => "html",
            Format::Json => "json",
        }
    }
}

impl std::str::FromStr for Format {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "plaintext" => Ok(Format::Plaintext),
            "html" => Ok(Format::Html),
            "json" => Ok(Format::Json),
            other => Err(Error::invalid(format!("unknown template format {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Translation {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: Format,
    #[serde(default = "default_locale")]
    pub default_locale: String,
    /// locale → title/body. Locales are BCP-47-ish (`en`, `de`, `pt-BR`); lookup falls back to the language part.
    #[serde(default)]
    pub translations: BTreeMap<String, Translation>,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn default_locale() -> String {
    "en".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rendered {
    pub locale: String,
    pub title: String,
    pub body: String,
}

impl Template {
    pub fn validate(&self) -> Result<()> {
        validate_key(&self.key)?;
        if self.default_locale.is_empty() || self.default_locale.len() > 16 {
            return Err(Error::invalid("default_locale must be 1–16 chars"));
        }
        if self.translations.is_empty() {
            return Err(Error::invalid("at least one translation is required"));
        }
        if !self.translations.contains_key(&self.default_locale) {
            return Err(Error::invalid(format!(
                "translations must include the default locale {:?}",
                self.default_locale
            )));
        }
        for (locale, t) in &self.translations {
            if locale.is_empty() || locale.len() > 16 {
                return Err(Error::invalid(format!("bad locale {locale:?}")));
            }
            if t.title.is_empty() && t.body.is_empty() {
                return Err(Error::invalid(format!("translation {locale:?} is empty")));
            }
            if self.format == Format::Json && !t.body.is_empty() && !t.body.contains("{{") {
                serde_json::from_str::<Value>(&t.body).map_err(|e| {
                    Error::invalid(format!(
                        "translation {locale:?} body is not valid JSON: {e}"
                    ))
                })?;
            }
        }
        Ok(())
    }

    /// Pick the best translation for `locale`: exact → language prefix → default → first.
    pub fn pick<'a>(&'a self, locale: Option<&'a str>) -> (&'a str, &'a Translation) {
        if let Some(l) = locale.filter(|l| !l.is_empty()) {
            if let Some(t) = self.translations.get(l) {
                return (l, t);
            }
            let lang = l.split(['-', '_']).next().unwrap_or(l);
            if let Some((k, t)) = self
                .translations
                .iter()
                .find(|(k, _)| k.split(['-', '_']).next() == Some(lang))
            {
                return (k, t);
            }
        }
        if let Some(t) = self.translations.get(&self.default_locale) {
            return (&self.default_locale, t);
        }
        let (k, t) = self
            .translations
            .iter()
            .next()
            .expect("validated non-empty");
        (k, t)
    }

    pub fn render(&self, locale: Option<&str>, props: &PropertyMap) -> Rendered {
        let (l, t) = self.pick(locale);
        Rendered {
            locale: l.to_string(),
            title: MessageCampaign::render(&t.title, props),
            body: MessageCampaign::render(&t.body, props),
        }
    }

    /// The locale a player prefers, from default/custom properties.
    pub fn locale_of(props: &PropertyMap) -> Option<String> {
        [
            "default.locale",
            "default.language",
            "custom.locale",
            "custom.language",
        ]
        .iter()
        .find_map(|k| match props.get(*k) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl() -> Template {
        let mut tr = BTreeMap::new();
        tr.insert(
            "en".into(),
            Translation {
                title: "Hi {{custom.name}}".into(),
                body: "en body".into(),
            },
        );
        tr.insert(
            "de".into(),
            Translation {
                title: "Hallo".into(),
                body: "de body".into(),
            },
        );
        Template {
            id: Uuid::nil(),
            key: "welcome".into(),
            name: String::new(),
            description: String::new(),
            format: Format::Plaintext,
            default_locale: "en".into(),
            translations: tr,
            image_url: String::new(),
            payload: Value::Null,
            tags: vec![],
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn locale_fallback() {
        let t = tpl();
        t.validate().unwrap();
        assert_eq!(t.pick(Some("de-AT")).0, "de");
        assert_eq!(t.pick(Some("fr")).0, "en");
        assert_eq!(t.pick(None).0, "en");
        let mut props = PropertyMap::new();
        props.insert("custom.name".into(), Value::String("Ana".into()));
        assert_eq!(t.render(Some("en"), &props).title, "Hi Ana");
    }
}
