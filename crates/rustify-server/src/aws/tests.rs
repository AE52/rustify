//! Unit tests for the AWS provisioning flow. All AWS calls go through the
//! [`AwsApi`] seam mocked by `RecordingAws`; SSH goes through a `FakeExecutor`.
//! No real AWS or SSH is ever touched.

use super::*;

use std::sync::Mutex;

use async_trait::async_trait;
use rustify_core::{CommandExecutor, ExecError, ExecEvent, ExecOpts, ExecOutput, ServerConn};
use rustify_db::repos::ServerRepo;
use rustify_jobs::JobQueue;
use sqlx::PgPool;
use tokio::sync::mpsc;

fn uid() -> String {
    rustify_core::ids::new_uuid()
}

// --------------------------------------------------------------------------
// Mocks
// --------------------------------------------------------------------------

struct RecordingAws {
    calls: Mutex<Vec<String>>,
    swarm_ports: Mutex<Option<bool>>,
    run_count: Mutex<Option<i32>>,
    instance_ids: Vec<String>,
}

impl RecordingAws {
    fn new(n: usize) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            swarm_ports: Mutex::new(None),
            run_count: Mutex::new(None),
            instance_ids: (0..n).map(|i| format!("i-{i:03}")).collect(),
        }
    }
    fn record(&self, s: &str) {
        self.calls.lock().unwrap().push(s.to_string());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl AwsApi for RecordingAws {
    async fn ensure_network(&self) -> Result<NetworkRefs, AwsError> {
        self.record("ensure_network");
        Ok(NetworkRefs {
            vpc_id: "vpc-1".to_string(),
            subnet_id: "subnet-1".to_string(),
        })
    }
    async fn ensure_security_group(
        &self,
        _vpc_id: &str,
        open_swarm_ports: bool,
    ) -> Result<String, AwsError> {
        self.record("ensure_security_group");
        *self.swarm_ports.lock().unwrap() = Some(open_swarm_ports);
        Ok("sg-1".to_string())
    }
    async fn ensure_key_pair(&self, _name: &str, _public_key: &str) -> Result<String, AwsError> {
        self.record("ensure_key_pair");
        Ok("rustify-k".to_string())
    }
    async fn latest_ubuntu_ami(&self) -> Result<String, AwsError> {
        self.record("latest_ubuntu_ami");
        Ok("ami-123".to_string())
    }
    async fn run_instances(&self, spec: &RunSpec) -> Result<Vec<String>, AwsError> {
        self.record("run_instances");
        *self.run_count.lock().unwrap() = Some(spec.count);
        Ok(self.instance_ids.clone())
    }
    async fn wait_running(&self, instance_ids: &[String]) -> Result<Vec<InstanceInfo>, AwsError> {
        self.record("wait_running");
        Ok(instance_ids
            .iter()
            .enumerate()
            .map(|(i, id)| InstanceInfo {
                instance_id: id.clone(),
                public_ip: Some(format!("203.0.113.{}", i + 1)),
                private_ip: Some(format!("10.0.1.{}", i + 1)),
                state: "running".to_string(),
            })
            .collect())
    }
    async fn describe_instances(
        &self,
        instance_ids: &[String],
    ) -> Result<Vec<InstanceInfo>, AwsError> {
        self.record("describe_instances");
        Ok(instance_ids
            .iter()
            .map(|id| InstanceInfo {
                instance_id: id.clone(),
                public_ip: None,
                private_ip: None,
                state: "running".to_string(),
            })
            .collect())
    }
}

struct FakeExec {
    cmds: Mutex<Vec<String>>,
}

impl FakeExec {
    fn new() -> Self {
        Self {
            cmds: Mutex::new(Vec::new()),
        }
    }
    fn cmds(&self) -> Vec<String> {
        self.cmds.lock().unwrap().clone()
    }
    fn run(&self, script: &str) -> ExecOutput {
        self.cmds.lock().unwrap().push(script.to_string());
        // The join-token command prints the worker token on stdout.
        let stdout = if script.contains("join-token") {
            "SWMTKN-TEST-TOKEN".to_string()
        } else {
            String::new()
        };
        ExecOutput {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }
}

#[async_trait]
impl CommandExecutor for FakeExec {
    async fn exec(
        &self,
        _conn: &ServerConn,
        script: &str,
        _opts: ExecOpts,
    ) -> Result<ExecOutput, ExecError> {
        Ok(self.run(script))
    }
    async fn exec_streaming(
        &self,
        _conn: &ServerConn,
        script: &str,
        _opts: ExecOpts,
        _tx: mpsc::Sender<ExecEvent>,
    ) -> Result<ExecOutput, ExecError> {
        Ok(self.run(script))
    }
    async fn upload(
        &self,
        _conn: &ServerConn,
        _local: &std::path::Path,
        _remote: &str,
    ) -> Result<(), ExecError> {
        Ok(())
    }
}

// --------------------------------------------------------------------------
// Fixtures
// --------------------------------------------------------------------------

/// Seed a team, private key and an `aws` cloud token; returns their ids.
async fn seed(pool: &PgPool) -> (i64, i64, i64) {
    let team: i64 =
        sqlx::query_scalar("INSERT INTO teams (uuid, name) VALUES ($1, 't') RETURNING id")
            .bind(uid())
            .fetch_one(pool)
            .await
            .unwrap();
    let key: i64 = sqlx::query_scalar(
        "INSERT INTO private_keys (uuid, team_id, name, private_key_enc, public_key)
         VALUES ($1, $2, 'k', $3, 'ssh-ed25519 AAAA') RETURNING id",
    )
    .bind(uid())
    .bind(team)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    let token: i64 = sqlx::query_scalar(
        "INSERT INTO cloud_provider_tokens (uuid, team_id, provider, token_enc)
         VALUES ($1, $2, 'aws', $3) RETURNING id",
    )
    .bind(uid())
    .bind(team)
    .bind(vec![0u8; 4])
    .fetch_one(pool)
    .await
    .unwrap();
    (team, key, token)
}

fn input(team: i64, key: i64, token: i64, count: i32) -> ProvisionInput {
    ProvisionInput {
        team_id: team,
        name: "web".to_string(),
        region: "eu-central-1".to_string(),
        instance_type: "t3.medium".to_string(),
        count,
        key_id: key,
        key_name: "k".to_string(),
        key_public: "ssh-ed25519 AAAA".to_string(),
        key_material: "PRIVATE-KEY-MATERIAL".to_string(),
        token_id: token,
    }
}

fn tmp_key_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("rustify-aws-test-{}", uid()))
}

// --------------------------------------------------------------------------
// Provision flow
// --------------------------------------------------------------------------

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn provision_single_node_ordered_calls_no_swarm(pool: PgPool) {
    let (team, key, token) = seed(&pool).await;
    let aws = RecordingAws::new(1);
    let exec = FakeExec::new();
    let queue = JobQueue::new(pool.clone());
    let dir = tmp_key_dir();

    let out = provision_aws(&aws, &exec, &pool, &queue, &dir, input(team, key, token, 1))
        .await
        .unwrap();

    // Ordered AWS call sequence for a single node.
    assert_eq!(
        aws.calls(),
        vec![
            "ensure_network",
            "ensure_security_group",
            "ensure_key_pair",
            "latest_ubuntu_ami",
            "run_instances",
            "wait_running",
        ]
    );
    assert_eq!(*aws.run_count.lock().unwrap(), Some(1));
    // SG must NOT open swarm ports for a single node.
    assert_eq!(*aws.swarm_ports.lock().unwrap(), Some(false));
    assert!(!out.swarm);
    assert!(!out.partial);
    assert_eq!(out.servers.len(), 1);
    assert_eq!(out.servers[0].aws_instance_id.as_deref(), Some("i-000"));
    assert_eq!(out.servers[0].aws_region.as_deref(), Some("eu-central-1"));
    assert_eq!(out.servers[0].ip, "203.0.113.1");
    // No swarm SSH for a single node.
    assert!(exec.cmds().is_empty());

    // Exactly one server_validate job enqueued.
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'server_validate'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn provision_multi_node_forms_swarm(pool: PgPool) {
    let (team, key, token) = seed(&pool).await;
    let aws = RecordingAws::new(3);
    let exec = FakeExec::new();
    let queue = JobQueue::new(pool.clone());
    let dir = tmp_key_dir();

    let out = provision_aws(&aws, &exec, &pool, &queue, &dir, input(team, key, token, 3))
        .await
        .unwrap();

    assert_eq!(
        aws.calls(),
        vec![
            "ensure_network",
            "ensure_security_group",
            "ensure_key_pair",
            "latest_ubuntu_ami",
            "run_instances",
            "wait_running",
        ]
    );
    assert_eq!(*aws.run_count.lock().unwrap(), Some(3));
    // SG opens swarm ports ONLY when count > 1.
    assert_eq!(*aws.swarm_ports.lock().unwrap(), Some(true));
    assert!(out.swarm);
    assert_eq!(out.servers.len(), 3);

    // Golden swarm command sequence: manager ensures docker, inits (advertising
    // its PRIVATE ip), captures the join token; each worker ensures docker then
    // joins at <mgr_private_ip>:2377.
    assert_eq!(
        exec.cmds(),
        vec![
            docker_ensure_command(),
            swarm_init_command("10.0.1.1"),
            swarm_join_token_command(),
            docker_ensure_command(),
            swarm_join_command("SWMTKN-TEST-TOKEN", "10.0.1.1"),
            docker_ensure_command(),
            swarm_join_command("SWMTKN-TEST-TOKEN", "10.0.1.1"),
        ]
    );

    // Swarm role flags persisted: node 0 manager, nodes 1..N workers.
    let repo = ServerRepo::new(pool.clone());
    let mgr = repo
        .get_by_uuid(&out.servers[0].uuid)
        .await
        .unwrap()
        .unwrap();
    let mgr_settings = repo.settings(mgr.id).await.unwrap().unwrap();
    assert!(mgr_settings.is_swarm_manager);
    assert!(!mgr_settings.is_swarm_worker);

    for s in &out.servers[1..] {
        let w = repo.get_by_uuid(&s.uuid).await.unwrap().unwrap();
        let ws = repo.settings(w.id).await.unwrap().unwrap();
        assert!(ws.is_swarm_worker);
        assert!(!ws.is_swarm_manager);
    }

    // Three server_validate jobs enqueued (one per node).
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'server_validate'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(jobs, 3);
}

#[sqlx::test(migrations = "../rustify-db/migrations")]
async fn provisioned_server_persists_aws_columns(pool: PgPool) {
    // Proves migration 0013 applied and the aws columns round-trip through the
    // repo (aws_instance_id / aws_region).
    let (team, key, token) = seed(&pool).await;
    let aws = RecordingAws::new(1);
    let exec = FakeExec::new();
    let queue = JobQueue::new(pool.clone());
    let dir = tmp_key_dir();

    let out = provision_aws(&aws, &exec, &pool, &queue, &dir, input(team, key, token, 1))
        .await
        .unwrap();

    let (instance_id, region): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT aws_instance_id, aws_region FROM servers WHERE uuid = $1")
            .bind(&out.servers[0].uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(instance_id.as_deref(), Some("i-000"));
    assert_eq!(region.as_deref(), Some("eu-central-1"));
}

// --------------------------------------------------------------------------
// Credentials
// --------------------------------------------------------------------------

#[tokio::test]
async fn sdk_config_uses_decrypted_token_not_env() {
    use aws_credential_types::provider::ProvideCredentials;

    let creds = AwsCredentials {
        access_key_id: "AKIAEXAMPLE".to_string(),
        secret_access_key: "topsecretvalue".to_string(),
    };
    let cfg = sdk_config(&creds, "eu-central-1");

    // Region is scoped to the requested region.
    assert_eq!(cfg.region().map(|r| r.as_ref()), Some("eu-central-1"));

    // The resolved credentials come from the token, never the environment.
    let provider = cfg
        .credentials_provider()
        .expect("static credentials provider present");
    let resolved = provider.provide_credentials().await.unwrap();
    assert_eq!(resolved.access_key_id(), "AKIAEXAMPLE");
    assert_eq!(resolved.secret_access_key(), "topsecretvalue");
}

// --------------------------------------------------------------------------
// Swarm command goldens
// --------------------------------------------------------------------------

#[test]
fn swarm_commands_are_golden() {
    assert_eq!(
        swarm_init_command("10.0.1.5"),
        "docker swarm init --advertise-addr 10.0.1.5"
    );
    assert_eq!(
        swarm_join_token_command(),
        "docker swarm join-token -q worker"
    );
    assert_eq!(
        swarm_join_command("SWMTKN-abc", "10.0.1.5"),
        "docker swarm join --token SWMTKN-abc 10.0.1.5:2377"
    );
    assert_eq!(
        docker_ensure_command(),
        "command -v docker >/dev/null 2>&1 || curl -fsSL https://get.docker.com | sh"
    );
}

#[test]
fn curated_metadata_is_present() {
    assert!(known_regions().contains(&"eu-central-1"));
    let types = curated_instance_types();
    assert!(
        types
            .iter()
            .any(|t| t.name == "t3.medium" && t.vcpus == 2 && t.mem_gb == 4)
    );
    assert!(types.iter().any(|t| t.name == "c6i.large"));
}
