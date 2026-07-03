//! Services aggregate (one-click templated compose stacks) and their child
//! `service_applications`. A service owns a raw template compose (`compose_raw`)
//! and the deploy-ready mutated compose (`compose_mutated`); its env vars live
//! in `environment_variables` with `resource_kind = 'service'`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

/// A full row of the `services` table.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Service {
    pub id: i64,
    pub uuid: String,
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub template_key: String,
    pub compose_raw: String,
    pub compose_mutated: Option<String>,
    pub status: String,
    pub config_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A child container of a service (`service_applications`).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ServiceApplication {
    pub id: i64,
    pub uuid: String,
    pub service_id: i64,
    pub name: String,
    pub fqdn: Option<String>,
    pub image: Option<String>,
    pub status: String,
    pub is_database: bool,
    pub created_at: DateTime<Utc>,
}

/// Fields accepted when creating a service.
#[derive(Debug, Clone)]
pub struct NewService {
    pub environment_id: i64,
    pub destination_id: i64,
    pub name: String,
    pub template_key: String,
    pub compose_raw: String,
}

const COLS: &str = "id, uuid, environment_id, destination_id, name, template_key, compose_raw, \
     compose_mutated, status, config_hash, created_at, updated_at";
const APP_COLS: &str = "id, uuid, service_id, name, fqdn, image, status, is_database, created_at";

#[derive(Clone)]
pub struct ServiceRepo {
    pool: PgPool,
}

impl ServiceRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, new: NewService) -> DbResult<Service> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, Service>(&format!(
            "INSERT INTO services
               (uuid, environment_id, destination_id, name, template_key, compose_raw)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(new.environment_id)
        .bind(new.destination_id)
        .bind(&new.name)
        .bind(&new.template_key)
        .bind(&new.compose_raw)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Service>> {
        let row =
            sqlx::query_as::<_, Service>(&format!("SELECT {COLS} FROM services WHERE uuid = $1"))
                .bind(uuid)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<Service>> {
        let row =
            sqlx::query_as::<_, Service>(&format!("SELECT {COLS} FROM services WHERE id = $1"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    pub async fn list_by_environment(&self, environment_id: i64) -> DbResult<Vec<Service>> {
        let rows = sqlx::query_as::<_, Service>(&format!(
            "SELECT {COLS} FROM services WHERE environment_id = $1 ORDER BY id"
        ))
        .bind(environment_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Rename a service (`PATCH`); returns the updated row or `None`.
    pub async fn rename(&self, uuid: &str, name: &str) -> DbResult<Option<Service>> {
        let row = sqlx::query_as::<_, Service>(&format!(
            "UPDATE services SET name = $2, updated_at = now() WHERE uuid = $1 RETURNING {COLS}"
        ))
        .bind(uuid)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Store the deploy-ready mutated compose + its config hash.
    pub async fn set_mutated(
        &self,
        id: i64,
        compose_mutated: &str,
        config_hash: &str,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE services SET compose_mutated = $2, config_hash = $3, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(compose_mutated)
        .bind(config_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the container status string (`running`, `exited`, ...).
    pub async fn set_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query("UPDATE services SET status = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM services WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }

    // ---- child service applications ----------------------------------------

    /// Insert or update a child container by `(service_id, name)`.
    pub async fn upsert_application(
        &self,
        service_id: i64,
        name: &str,
        fqdn: Option<&str>,
        image: Option<&str>,
        is_database: bool,
    ) -> DbResult<ServiceApplication> {
        let row = sqlx::query_as::<_, ServiceApplication>(&format!(
            "INSERT INTO service_applications (uuid, service_id, name, fqdn, image, is_database)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (service_id, name) DO UPDATE
               SET fqdn = EXCLUDED.fqdn, image = EXCLUDED.image,
                   is_database = EXCLUDED.is_database
             RETURNING {APP_COLS}"
        ))
        .bind(ids::new_uuid())
        .bind(service_id)
        .bind(name)
        .bind(fqdn)
        .bind(image)
        .bind(is_database)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn applications(&self, service_id: i64) -> DbResult<Vec<ServiceApplication>> {
        let rows = sqlx::query_as::<_, ServiceApplication>(&format!(
            "SELECT {APP_COLS} FROM service_applications WHERE service_id = $1 ORDER BY id"
        ))
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn set_application_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query("UPDATE service_applications SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
