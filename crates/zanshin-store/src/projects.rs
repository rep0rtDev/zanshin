//! Project registry (lives in the default project's schema): slug, name, lifecycle.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use zanshin_core::{Error, Result};

use crate::map_sqlx;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub const DEFAULT_SLUG: &str = "default";

/// Slugs are lowercase `[a-z0-9_]`, 2..32 chars, start with a letter (they become schema / database suffixes).
pub fn validate_slug(slug: &str) -> Result<()> {
    let ok = (2..=32).contains(&slug.len())
        && slug.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        return Err(Error::invalid(
            "project slug must be 2..32 chars of [a-z0-9_] starting with a letter",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub struct ProjectRepo {
    pool: PgPool,
}

const COLS: &str = "slug, name, description, archived, created_at, updated_at";

impl ProjectRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<Project>> {
        sqlx::query_as(&format!("SELECT {COLS} FROM projects ORDER BY slug"))
            .fetch_all(&self.pool)
            .await
            .map_err(map_sqlx)
    }

    pub async fn get(&self, slug: &str) -> Result<Project> {
        sqlx::query_as(&format!("SELECT {COLS} FROM projects WHERE slug = $1"))
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::not_found(format!("project {slug}")))
    }

    pub async fn create(&self, slug: &str, name: &str, description: &str) -> Result<Project> {
        validate_slug(slug)?;
        sqlx::query_as(&format!(
            "INSERT INTO projects (slug, name, description) VALUES ($1, $2, $3) RETURNING {COLS}"
        ))
        .bind(slug)
        .bind(name)
        .bind(description)
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx)
    }

    pub async fn update(
        &self,
        slug: &str,
        name: Option<&str>,
        description: Option<&str>,
        archived: Option<bool>,
    ) -> Result<Project> {
        sqlx::query_as(&format!(
            "UPDATE projects SET name = COALESCE($2, name), description = COALESCE($3, description),
                    archived = COALESCE($4, archived), updated_at = now()
             WHERE slug = $1 RETURNING {COLS}"
        ))
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(archived)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx)?
        .ok_or_else(|| Error::not_found(format!("project {slug}")))
    }
}
