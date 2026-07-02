//! Teams aggregate. Phase 1 is single-team (team #1 seeded at startup) but the
//! `team_id` columns are kept throughout per spec §3.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Team {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, uuid, name, created_at, updated_at";

#[derive(Clone)]
pub struct TeamRepo {
    pool: PgPool,
}

impl TeamRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, name: &str) -> DbResult<Team> {
        let uuid = ids::new_uuid();
        let row = sqlx::query_as::<_, Team>(&format!(
            "INSERT INTO teams (uuid, name) VALUES ($1, $2) RETURNING {COLS}"
        ))
        .bind(&uuid)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<Team>> {
        let row = sqlx::query_as::<_, Team>(&format!("SELECT {COLS} FROM teams WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }
}
