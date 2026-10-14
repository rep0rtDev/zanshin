//! Message templates catalog.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use zanshin_core::{
    flags::validate_key,
    properties::PropertyMap,
    templates::{default_locale, Format, Rendered, Template, Translation},
    Error, Result,
};
use zanshin_store::templates::TemplateRepo;

use super::flags::normalize_tags;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TemplatePatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(default)]
    pub default_locale: Option<String>,
    #[serde(default)]
    pub translations: Option<BTreeMap<String, Translation>>,
    #[serde(default)]
    pub image_url: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct TemplateService {
    repo: TemplateRepo,
}

impl TemplateService {
    pub fn new(repo: TemplateRepo) -> Self {
        Self { repo }
    }

    pub async fn list(&self) -> Result<Vec<Template>> {
        self.repo.list().await
    }

    pub async fn get(&self, key: &str) -> Result<Template> {
        self.repo.get(key).await
    }

    pub async fn used_by(&self, key: &str) -> Result<Vec<String>> {
        self.repo.used_by(key).await
    }

    pub async fn create(&self, key: &str, p: TemplatePatch) -> Result<Template> {
        validate_key(key)?;
        let now = chrono::Utc::now();
        let t = Template {
            id: Uuid::now_v7(),
            key: key.to_string(),
            name: p.name.unwrap_or_default(),
            description: p.description.unwrap_or_default(),
            format: p.format.unwrap_or_default(),
            default_locale: p.default_locale.unwrap_or_else(default_locale),
            translations: p.translations.unwrap_or_default(),
            image_url: p.image_url.unwrap_or_default(),
            payload: p.payload.unwrap_or(Value::Null),
            tags: normalize_tags(p.tags.unwrap_or_default())?,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        t.validate()?;
        self.repo.create(&t).await
    }

    pub async fn update(&self, key: &str, p: TemplatePatch) -> Result<Template> {
        let mut t = self.repo.get(key).await?;
        if let Some(v) = p.name {
            t.name = v;
        }
        if let Some(v) = p.description {
            t.description = v;
        }
        if let Some(v) = p.format {
            t.format = v;
        }
        if let Some(v) = p.default_locale {
            t.default_locale = v;
        }
        if let Some(v) = p.translations {
            t.translations = v;
        }
        if let Some(v) = p.image_url {
            t.image_url = v;
        }
        if let Some(v) = p.payload {
            t.payload = v;
        }
        if let Some(v) = p.tags {
            t.tags = normalize_tags(v)?;
        }
        t.validate()?;
        self.repo.update(&t).await
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        if let Some(c) = self.repo.used_by(key).await?.first() {
            return Err(Error::conflict(format!(
                "template {key:?} is used by campaign {c:?}"
            )));
        }
        self.repo.delete(key).await
    }

    pub async fn render(
        &self,
        key: &str,
        locale: Option<&str>,
        props: &PropertyMap,
    ) -> Result<(Template, Rendered)> {
        let t = self.repo.get(key).await?;
        let r = t.render(locale, props);
        Ok((t, r))
    }
}
