use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use zanshin_core::{properties::PropertyMap, Error, Identity, PropertySet, Result};

use crate::map_sqlx;

#[derive(FromRow)]
struct IdentityRow {
    id: Uuid,
    external_id: String,
    default_properties: Value,
    computed_properties: Value,
    custom_properties: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

fn as_map(v: Value) -> PropertyMap {
    match v {
        Value::Object(m) => m,
        _ => PropertyMap::new(),
    }
}

impl From<IdentityRow> for Identity {
    fn from(r: IdentityRow) -> Self {
        Identity {
            id: r.id,
            external_id: r.external_id,
            properties: PropertySet {
                default: as_map(r.default_properties),
                computed: as_map(r.computed_properties),
                custom: as_map(r.custom_properties),
            },
            created_at: r.created_at,
            updated_at: r.updated_at,
            last_seen_at: r.last_seen_at,
        }
    }
}

const COLS: &str = "id, external_id, default_properties, computed_properties, custom_properties, created_at, updated_at, last_seen_at";

#[derive(Clone)]
pub struct IdentityRepo {
    pg: PgPool,
}

impl IdentityRepo {
    pub fn new(pg: PgPool) -> Self {
        Self { pg }
    }

    /// Find-or-create by external id, merging the supplied properties and touching `last_seen_at`.
    pub async fn authenticate(
        &self,
        external_id: &str,
        default: &PropertyMap,
        custom: &PropertyMap,
    ) -> Result<(Identity, bool)> {
        let id = Uuid::now_v7();
        let row: (bool,) = sqlx::query_as(
            r#"INSERT INTO identities (id, external_id, default_properties, custom_properties)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (external_id) DO UPDATE SET
                   default_properties = identities.default_properties || EXCLUDED.default_properties,
                   custom_properties  = identities.custom_properties  || EXCLUDED.custom_properties,
                   last_seen_at = now(),
                   updated_at = CASE WHEN EXCLUDED.default_properties = '{}'::jsonb AND EXCLUDED.custom_properties = '{}'::jsonb
                                     THEN identities.updated_at ELSE now() END
               RETURNING (xmax = 0) AS created"#,
        )
        .bind(id)
        .bind(external_id)
        .bind(Value::Object(default.clone()))
        .bind(Value::Object(custom.clone()))
        .fetch_one(&self.pg)
        .await
        .map_err(map_sqlx)?;
        let identity = self.get_by_external_id(external_id).await?;
        Ok((identity, row.0))
    }

    pub async fn get(&self, id: Uuid) -> Result<Identity> {
        let row: Option<IdentityRow> =
            sqlx::query_as(&format!("SELECT {COLS} FROM identities WHERE id = $1"))
                .bind(id)
                .fetch_optional(&self.pg)
                .await
                .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found("identity not found"))
    }

    pub async fn get_by_external_id(&self, external_id: &str) -> Result<Identity> {
        let row: Option<IdentityRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM identities WHERE external_id = $1"
        ))
        .bind(external_id)
        .fetch_optional(&self.pg)
        .await
        .map_err(map_sqlx)?;
        row.map(Into::into)
            .ok_or_else(|| Error::not_found("identity not found"))
    }

    /// Merge property patches (JSON null deletes a key). Returns the updated identity.
    pub async fn update_properties(
        &self,
        id: Uuid,
        default: &PropertyMap,
        custom: &PropertyMap,
    ) -> Result<Identity> {
        let mut tx = self.pg.begin().await.map_err(map_sqlx)?;
        let row: Option<IdentityRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM identities WHERE id = $1 FOR UPDATE"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        let mut identity: Identity = row
            .map(Into::into)
            .ok_or_else(|| Error::not_found("identity not found"))?;
        zanshin_core::properties::merge(&mut identity.properties.default, default.clone());
        zanshin_core::properties::merge(&mut identity.properties.custom, custom.clone());
        let row: IdentityRow = sqlx::query_as(&format!(
            "UPDATE identities SET default_properties = $2, custom_properties = $3, updated_at = now(), last_seen_at = now() WHERE id = $1 RETURNING {COLS}"
        ))
        .bind(id)
        .bind(Value::Object(identity.properties.default))
        .bind(Value::Object(identity.properties.custom))
        .fetch_one(&mut *tx)
        .await
        .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(row.into())
    }

    pub async fn set_computed(&self, id: Uuid, computed: &PropertyMap) -> Result<()> {
        sqlx::query(
            "UPDATE identities SET computed_properties = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(Value::Object(computed.clone()))
        .execute(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn touch(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE identities SET last_seen_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("DELETE FROM identities WHERE id = $1")
            .bind(id)
            .execute(&self.pg)
            .await
            .map_err(map_sqlx)?;
        if res.rows_affected() == 0 {
            return Err(Error::not_found("identity not found"));
        }
        Ok(())
    }

    pub async fn count(&self) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM identities")
            .fetch_one(&self.pg)
            .await
            .map_err(map_sqlx)?;
        Ok(n)
    }

    pub async fn list(&self, after: Option<Uuid>, limit: i64) -> Result<Vec<Identity>> {
        let rows: Vec<IdentityRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM identities WHERE ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2"
        ))
        .bind(after)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pg)
        .await
        .map_err(map_sqlx)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
