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
    pub hetzner_server_id: Option<i64>,
    pub hetzner_server_status: Option<String>,
    pub ip_previous: Option<String>,
    pub cloud_provider_token_id: Option<i64>,
    /// EC2 instance id (`i-…`) for AWS-provisioned servers (migration 0013).
    pub aws_instance_id: Option<String>,
    /// AWS region the instance lives in (migration 0013).
    pub aws_region: Option<String>,
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
    /// Whether periodic metrics collection runs for this server (migration 0011).
    pub metrics_enabled: bool,
    /// Target seconds between metrics pulls; also drives staleness (migration 0011).
    pub metrics_refresh_rate_seconds: i32,
    /// How many days of samples the retention prune keeps (migration 0011).
    pub metrics_history_days: i32,
    pub is_cloudflare_tunnel: bool,
    /// Whether this server is the manager of an AWS-provisioned Docker Swarm
    /// (migration 0013).
    pub is_swarm_manager: bool,
    /// Whether this server joined an AWS-provisioned Docker Swarm as a worker
    /// (migration 0013).
    pub is_swarm_worker: bool,
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
     reachable, usable, validation_logs, hetzner_server_id, hetzner_server_status, ip_previous, \
     cloud_provider_token_id, aws_instance_id, aws_region, created_at, updated_at";

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

/// Fields required to register a Hetzner-provisioned server.
#[derive(Debug, Clone)]
pub struct NewHetznerServer {
    pub team_id: i64,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub ssh_user: String,
    pub private_key_id: i64,
    pub hetzner_server_id: i64,
    pub cloud_provider_token_id: i64,
}

/// Fields required to register an AWS EC2-provisioned server.
#[derive(Debug, Clone)]
pub struct NewAwsServer {
    pub team_id: i64,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub ssh_user: String,
    pub private_key_id: i64,
    pub aws_instance_id: String,
    pub aws_region: String,
    pub cloud_provider_token_id: i64,
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

    /// Register a Hetzner-provisioned server (user `root`, port 22, proxy
    /// traefik/exited) with its `hetzner_server_id` and cloud token, together
    /// with its default settings + destination row. Parity with Coolify's
    /// `ByHetzner::submit` (app/Livewire/Server/New/ByHetzner.php:508-521).
    pub async fn create_hetzner(&self, new: NewHetznerServer) -> DbResult<Server> {
        let mut tx = self.pool.begin().await?;
        let uuid = ids::new_uuid();
        let server = sqlx::query_as::<_, Server>(&format!(
            "INSERT INTO servers
               (uuid, team_id, name, ip, port, ssh_user, private_key_id,
                hetzner_server_id, hetzner_server_status, cloud_provider_token_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'initializing', $9)
             RETURNING {SERVER_COLS}"
        ))
        .bind(&uuid)
        .bind(new.team_id)
        .bind(&new.name)
        .bind(&new.ip)
        .bind(new.port)
        .bind(&new.ssh_user)
        .bind(new.private_key_id)
        .bind(new.hetzner_server_id)
        .bind(new.cloud_provider_token_id)
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

    /// Register an AWS EC2-provisioned server (user `ubuntu`, port 22) with its
    /// `aws_instance_id`/`aws_region` and cloud token, together with its default
    /// settings + destination row. The AWS twin of [`create_hetzner`].
    pub async fn create_aws(&self, new: NewAwsServer) -> DbResult<Server> {
        let mut tx = self.pool.begin().await?;
        let uuid = ids::new_uuid();
        let server = sqlx::query_as::<_, Server>(&format!(
            "INSERT INTO servers
               (uuid, team_id, name, ip, port, ssh_user, private_key_id,
                aws_instance_id, aws_region, cloud_provider_token_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING {SERVER_COLS}"
        ))
        .bind(&uuid)
        .bind(new.team_id)
        .bind(&new.name)
        .bind(&new.ip)
        .bind(new.port)
        .bind(&new.ssh_user)
        .bind(new.private_key_id)
        .bind(&new.aws_instance_id)
        .bind(&new.aws_region)
        .bind(new.cloud_provider_token_id)
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

    /// Persist a server's Docker Swarm role after cluster formation. Exactly one
    /// of `manager`/`worker` is set true per node.
    pub async fn set_swarm_role(
        &self,
        server_id: i64,
        manager: bool,
        worker: bool,
    ) -> DbResult<()> {
        sqlx::query(
            "UPDATE server_settings
                SET is_swarm_manager = $2, is_swarm_worker = $3, updated_at = now()
              WHERE server_id = $1",
        )
        .bind(server_id)
        .bind(manager)
        .bind(worker)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update the cached AWS instance state (`running`/`stopped`/…) for a server.
    /// Stored in the shared `hetzner_server_status` column, which is a generic
    /// cloud power-state cache reused across providers (non-destructive).
    pub async fn set_aws_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE servers SET hetzner_server_status = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every AWS EC2-provisioned server (`aws_instance_id IS NOT NULL`), used by
    /// the periodic instance-state sync.
    pub async fn aws_servers(&self) -> DbResult<Vec<Server>> {
        let rows = sqlx::query_as::<_, Server>(&format!(
            "SELECT {SERVER_COLS} FROM servers WHERE aws_instance_id IS NOT NULL ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Update the cached Hetzner power state (`running`/`off`/…) for a server.
    pub async fn set_hetzner_status(&self, id: i64, status: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE servers SET hetzner_server_status = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every server in a team that Hetzner tracks (`hetzner_server_id IS NOT
    /// NULL`), used by the periodic power-state sync.
    pub async fn hetzner_servers(&self) -> DbResult<Vec<Server>> {
        let rows = sqlx::query_as::<_, Server>(&format!(
            "SELECT {SERVER_COLS} FROM servers WHERE hetzner_server_id IS NOT NULL ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Toggle a server's Cloudflare-tunnel flag. When enabling, the direct IP is
    /// stashed in `ip_previous` and `ip` is replaced with the SSH hostname; when
    /// disabling, `ip_previous` is restored. Parity with Coolify's
    /// `CloudflareTunnelChanged` handling.
    pub async fn set_cloudflare_tunnel(
        &self,
        id: i64,
        enabled: bool,
        ssh_hostname: Option<&str>,
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE server_settings SET is_cloudflare_tunnel = $2, updated_at = now()
             WHERE server_id = $1",
        )
        .bind(id)
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        if enabled {
            if let Some(host) = ssh_hostname {
                sqlx::query(
                    "UPDATE servers SET ip_previous = ip, ip = $2, updated_at = now()
                     WHERE id = $1",
                )
                .bind(id)
                .bind(host)
                .execute(&mut *tx)
                .await?;
            }
        } else {
            sqlx::query(
                "UPDATE servers
                    SET ip = COALESCE(ip_previous, ip), ip_previous = NULL, updated_at = now()
                  WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Usable build servers in a team (`is_build_server = true`). Parity with
    /// Coolify's `Server::buildServers` scope used by `ApplicationDeploymentJob`.
    pub async fn build_servers(&self, team_id: i64) -> DbResult<Vec<Server>> {
        let rows = sqlx::query_as::<_, Server>(&format!(
            "SELECT s.{cols} FROM servers s
               JOIN server_settings ss ON ss.server_id = s.id
              WHERE s.team_id = $1 AND s.usable = true AND ss.is_build_server = true
              ORDER BY s.id",
            cols = SERVER_COLS.replace(", ", ", s.")
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Deploy/destination-eligible servers in a team: usable and NOT build
    /// servers. Build servers only build+push images and are excluded from
    /// proxy/destination/deploy-target lists.
    pub async fn deploy_targets(&self, team_id: i64) -> DbResult<Vec<Server>> {
        let rows = sqlx::query_as::<_, Server>(&format!(
            "SELECT s.{cols} FROM servers s
               JOIN server_settings ss ON ss.server_id = s.id
              WHERE s.team_id = $1 AND s.usable = true AND ss.is_build_server = false
              ORDER BY s.id",
            cols = SERVER_COLS.replace(", ", ", s.")
        ))
        .bind(team_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
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

    /// Persist the proxy's runtime status (`running` / `exited` / ...), set by
    /// the proxy start/stop/restart lifecycle handlers.
    pub async fn set_proxy_status(&self, server_id: i64, status: &str) -> DbResult<()> {
        sqlx::query(
            "UPDATE server_settings SET proxy_status = $2, updated_at = now()
             WHERE server_id = $1",
        )
        .bind(server_id)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Partial update of a server's operational settings (proxy type, build /
    /// terminal / metrics flags). `None` args leave the column unchanged. Returns
    /// the refreshed settings, or `None` when the server has no settings row.
    pub async fn update_settings(
        &self,
        server_id: i64,
        proxy_type: Option<&str>,
        is_build_server: Option<bool>,
        is_terminal_enabled: Option<bool>,
        metrics_enabled: Option<bool>,
        metrics_refresh_rate_seconds: Option<i32>,
    ) -> DbResult<Option<ServerSettings>> {
        sqlx::query(
            "UPDATE server_settings SET
                proxy_type = COALESCE($2, proxy_type),
                is_build_server = COALESCE($3, is_build_server),
                is_terminal_enabled = COALESCE($4, is_terminal_enabled),
                metrics_enabled = COALESCE($5, metrics_enabled),
                metrics_refresh_rate_seconds = COALESCE($6, metrics_refresh_rate_seconds),
                updated_at = now()
             WHERE server_id = $1",
        )
        .bind(server_id)
        .bind(proxy_type)
        .bind(is_build_server)
        .bind(is_terminal_enabled)
        .bind(metrics_enabled)
        .bind(metrics_refresh_rate_seconds)
        .execute(&self.pool)
        .await?;
        self.settings(server_id).await
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
                    is_build_server, is_terminal_enabled, metrics_enabled,
                    metrics_refresh_rate_seconds, metrics_history_days,
                    is_cloudflare_tunnel, is_swarm_manager, is_swarm_worker,
                    created_at, updated_at
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
