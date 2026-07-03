//! Servers aggregate, plus the 1:1 `server_settings` row and `destinations`
//! (docker networks) that hang off a server.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use rustify_core::ids;

use crate::DbResult;

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Server {
    pub id: i64,
    pub uuid: String,
    pub team_id: i64,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub ssh_user: String,
    pub private_key_id: i64,
    pub reachable: bool,
    pub usable: bool,
    pub validation_logs: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ServerSettings {
    pub id: i64,
    pub server_id: i64,
    pub concurrent_builds: i32,
    pub deployment_queue_limit: i32,
    pub dynamic_timeout: i32,
    pub connection_timeout: i32,
    pub proxy_type: String,
    pub proxy_status: String,
    pub proxy_custom_config: Option<String>,
    pub is_build_server: bool,
    /// Whether the interactive web terminal (PTY over SSH) is allowed for this
    /// server (migration 0010; parity with Coolify `isTerminalEnabled`).
    pub is_terminal_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Destination {
    pub id: i64,
    pub uuid: String,
    pub server_id: i64,
    pub network: String,
    pub created_at: DateTime<Utc>,
}

const SERVER_COLS: &str = "id, uuid, team_id, name, ip, port, ssh_user, private_key_id, \
     reachable, usable, validation_logs, created_at, updated_at";

/// Fields required to register a server.
#[derive(Debug, Clone)]
pub struct NewServer {
    pub team_id: i64,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub ssh_user: String,
    pub private_key_id: i64,
}

#[derive(Clone)]
pub struct ServerRepo {
    pool: PgPool,
}

impl ServerRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Register a server together with its default `server_settings` row and a
    /// default `destinations` row on the `rustify` network (contract C7), all
    /// in one transaction.
    pub async fn create(&self, new: NewServer) -> DbResult<Server> {
        let mut tx = self.pool.begin().await?;
        let uuid = ids::new_uuid();
        let server = sqlx::query_as::<_, Server>(&format!(
            "INSERT INTO servers (uuid, team_id, name, ip, port, ssh_user, private_key_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {SERVER_COLS}"
        ))
        .bind(&uuid)
        .bind(new.team_id)
        .bind(&new.name)
        .bind(&new.ip)
        .bind(new.port)
        .bind(&new.ssh_user)
        .bind(new.private_key_id)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query("INSERT INTO server_settings (server_id) VALUES ($1)")
            .bind(server.id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO destinations (uuid, server_id, network) VALUES ($1, $2, 'rustify')",
        )
        .bind(ids::new_uuid())
        .bind(server.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(server)
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> DbResult<Option<Server>> {
        let row = sqlx::query_as::<_, Server>(&format!(
            "SELECT {SERVER_COLS} FROM servers WHERE uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Resolve a server by numeric id — used by the API to render the
    /// `server_uuid` of deployments/applications (contract C5 shapes).
    pub async fn get_by_id(&self, id: i64) -> DbResult<Option<Server>> {
        let row = sqlx::query_as::<_, Server>(&format!(
            "SELECT {SERVER_COLS} FROM servers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Resolve a destination by numeric id — maps an application's
    /// `destination_id` back to its owning server (contract C5 shapes).
    pub async fn destination_by_id(&self, id: i64) -> DbResult<Option<Destination>> {
        let row = sqlx::query_as::<_, Destination>(
            "SELECT id, uuid, server_id, network, created_at
             FROM destinations WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Partial update for `PATCH /servers/{uuid}` (contract C5). `NULL` args
    /// leave the corresponding column unchanged. Returns the updated row, or
    /// `None` if the uuid is unknown.
    pub async fn update(
        &self,
        uuid: &str,
        name: Option<&str>,
        ip: Option<&str>,
        port: Option<i32>,
        ssh_user: Option<&str>,
        private_key_id: Option<i64>,
    ) -> DbResult<Option<Server>> {
        let row = sqlx::query_as::<_, Server>(&format!(
            "UPDATE servers
                SET name = COALESCE($2, name),
                    ip = COALESCE($3, ip),
                    port = COALESCE($4, port),
                    ssh_user = COALESCE($5, ssh_user),
                    private_key_id = COALESCE($6, private_key_id),
                    updated_at = now()
              WHERE uuid = $1
              RETURNING {SERVER_COLS}"
        ))
        .bind(uuid)
        .bind(name)
        .bind(ip)
        .bind(port)
        .bind(ssh_user)
        .bind(private_key_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Persist the proxy's saved custom config (`PATCH /servers/{uuid}/proxy`).
    pub async fn set_proxy_custom_config(
        &self,
        server_id: i64,
        proxy_custom_config: Option<&str>,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE server_settings SET proxy_custom_config = $2, updated_at = now()
             WHERE server_id = $1",
        )
        .bind(server_id)
        .bind(proxy_custom_config)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self, team_id: i64) -> DbResult<Vec<Server>> {
        let rows = sqlx::query_as::<_, Server>(&format!(
            "SELECT {SERVER_COLS} FROM servers WHERE team_id = $1 ORDER BY id"
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Record the outcome of a reachability/usability probe (track E/server
    /// validation), storing the captured logs.
    pub async fn set_reachability(
        &self,
        id: i64,
        reachable: bool,
        usable: bool,
        validation_logs: Option<&str>,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE servers SET reachable = $2, usable = $3, validation_logs = $4, updated_at = now()
             WHERE id = $1",
        )
        .bind(id)
        .bind(reachable)
        .bind(usable)
        .bind(validation_logs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn settings(&self, server_id: i64) -> DbResult<Option<ServerSettings>> {
        let row = sqlx::query_as::<_, ServerSettings>(
            "SELECT id, server_id, concurrent_builds, deployment_queue_limit, dynamic_timeout,
                    connection_timeout, proxy_type, proxy_status, proxy_custom_config,
                    is_build_server, is_terminal_enabled, created_at, updated_at
             FROM server_settings WHERE server_id = $1",
        )
        .bind(server_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// The default `rustify`-network destination for a server.
    pub async fn default_destination(&self, server_id: i64) -> DbResult<Option<Destination>> {
        let row = sqlx::query_as::<_, Destination>(
            "SELECT id, uuid, server_id, network, created_at
             FROM destinations WHERE server_id = $1 ORDER BY id LIMIT 1",
        )
        .bind(server_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn delete(&self, uuid: &str) -> DbResult<bool> {
        let result = sqlx::query("DELETE FROM servers WHERE uuid = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() == 1)
    }
}
