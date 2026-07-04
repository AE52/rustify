//! AWS EC2 client + provisioning orchestration (aws-provision track).
//!
//! The AWS twin of [`crate::hetzner`]. Everything AWS talks to sits behind the
//! [`AwsApi`] seam so the provisioning flow is unit-tested without touching real
//! AWS; production uses [`Ec2Client`], built from a region + the *decrypted*
//! cloud token via a static credentials provider — never the ambient
//! environment. Secrets are only ever held to build the credentials provider or
//! materialise the SSH key file; they are never logged.
//!
//! Flow (idempotent, tagged `rustify:managed=true`): ensure network (prefer the
//! region default VPC/subnet) → ensure `rustify-sg` (swarm ports only for
//! multi-node) → import the key pair → resolve the latest Ubuntu 24.04 AMI via
//! SSM → RunInstances(count) → wait running+status-ok → register a Rustify
//! server per instance and enqueue `server_validate` → for `count > 1`, form a
//! Docker Swarm (init the manager, join the workers). Partial-failure safe.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use rustify_core::{CommandExecutor, ExecOpts, ServerConn};
use rustify_db::DbError;
use rustify_db::repos::{NewAwsServer, Server, ServerRepo};
use rustify_jobs::JobQueue;

/// SSM public parameter for the latest Ubuntu 24.04 LTS amd64 gp3 AMI id.
pub const UBUNTU_2404_SSM_PARAM: &str =
    "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id";
/// The security-group name Rustify manages.
pub const SECURITY_GROUP_NAME: &str = "rustify-sg";
/// The tag every Rustify-managed AWS resource carries.
pub const MANAGED_TAG_KEY: &str = "rustify:managed";
pub const MANAGED_TAG_VALUE: &str = "true";

// --------------------------------------------------------------------------
// Credentials + data types
// --------------------------------------------------------------------------

/// The decrypted AWS token, stored as encrypted JSON in `cloud_provider_tokens`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AwsError {
    #[error("aws api error: {0}")]
    Api(String),
    #[error("aws returned no {0}")]
    Missing(String),
    #[error("timed out waiting for instances to become ready")]
    Timeout,
}

/// The VPC + subnet an instance is launched into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRefs {
    pub vpc_id: String,
    pub subnet_id: String,
}

/// The live view of one EC2 instance the flow needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInfo {
    pub instance_id: String,
    pub public_ip: Option<String>,
    pub private_ip: Option<String>,
    pub state: String,
}

/// Parameters for a single `RunInstances` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub ami: String,
    pub instance_type: String,
    pub count: i32,
    pub security_group_id: String,
    pub subnet_id: String,
    pub key_name: String,
    pub name_tag: String,
}

/// The AWS operations the provisioning flow performs, behind a seam so tests
/// mock them and never call real AWS.
#[async_trait]
pub trait AwsApi: Send + Sync {
    /// Ensure a usable VPC + subnet (prefer the region default; create only if
    /// none exists).
    async fn ensure_network(&self) -> Result<NetworkRefs, AwsError>;
    /// Ensure the `rustify-sg` security group. When `open_swarm_ports` the swarm
    /// mgmt/gossip/overlay ports are opened to the SG itself (multi-node only).
    async fn ensure_security_group(
        &self,
        vpc_id: &str,
        open_swarm_ports: bool,
    ) -> Result<String, AwsError>;
    /// Import the given public key as an EC2 key pair (dedupe by name).
    async fn ensure_key_pair(&self, name: &str, public_key: &str) -> Result<String, AwsError>;
    /// Resolve the latest Ubuntu 24.04 AMI id via SSM.
    async fn latest_ubuntu_ami(&self) -> Result<String, AwsError>;
    /// Launch `spec.count` instances; returns their instance ids.
    async fn run_instances(&self, spec: &RunSpec) -> Result<Vec<String>, AwsError>;
    /// Wait until every instance is `running` + status-ok; returns their live view.
    async fn wait_running(&self, instance_ids: &[String]) -> Result<Vec<InstanceInfo>, AwsError>;
    /// Non-destructive point-in-time view of the instances (status sync).
    async fn describe_instances(
        &self,
        instance_ids: &[String],
    ) -> Result<Vec<InstanceInfo>, AwsError>;
}

// --------------------------------------------------------------------------
// Curated static metadata (no token / no API call needed)
// --------------------------------------------------------------------------

/// The AWS commercial regions Rustify offers (SDK known-region list). Kept
/// static so `GET /aws/regions` needs no credentials.
pub fn known_regions() -> Vec<&'static str> {
    vec![
        "us-east-1",
        "us-east-2",
        "us-west-1",
        "us-west-2",
        "ca-central-1",
        "eu-central-1",
        "eu-west-1",
        "eu-west-2",
        "eu-west-3",
        "eu-north-1",
        "eu-south-1",
        "ap-south-1",
        "ap-southeast-1",
        "ap-southeast-2",
        "ap-northeast-1",
        "ap-northeast-2",
        "ap-northeast-3",
        "ap-east-1",
        "sa-east-1",
        "me-south-1",
        "af-south-1",
    ]
}

/// One curated instance type offered in the provision UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstanceType {
    pub name: &'static str,
    pub vcpus: u32,
    pub mem_gb: u32,
}

/// A curated common set of instance types (full DescribeInstanceTypes is huge).
pub fn curated_instance_types() -> Vec<InstanceType> {
    vec![
        InstanceType {
            name: "t3.small",
            vcpus: 2,
            mem_gb: 2,
        },
        InstanceType {
            name: "t3.medium",
            vcpus: 2,
            mem_gb: 4,
        },
        InstanceType {
            name: "t3.large",
            vcpus: 2,
            mem_gb: 8,
        },
        InstanceType {
            name: "t3.xlarge",
            vcpus: 4,
            mem_gb: 16,
        },
        InstanceType {
            name: "m6i.large",
            vcpus: 2,
            mem_gb: 8,
        },
        InstanceType {
            name: "m6i.xlarge",
            vcpus: 4,
            mem_gb: 16,
        },
        InstanceType {
            name: "c6i.large",
            vcpus: 2,
            mem_gb: 4,
        },
    ]
}

// --------------------------------------------------------------------------
// Swarm command builders (golden-tested)
// --------------------------------------------------------------------------

/// Idempotent docker-ensure: install via get.docker.com only when absent. Twin
/// of the `server_validate` step so swarm formation does not race the async
/// install pipeline.
pub fn docker_ensure_command() -> String {
    "command -v docker >/dev/null 2>&1 || curl -fsSL https://get.docker.com | sh".to_string()
}

/// `docker swarm init` on the manager, advertising its private address.
pub fn swarm_init_command(advertise_addr: &str) -> String {
    format!("docker swarm init --advertise-addr {advertise_addr}")
}

/// Print the worker join token (captured from stdout).
pub fn swarm_join_token_command() -> String {
    "docker swarm join-token -q worker".to_string()
}

/// `docker swarm join` a worker to the manager at `<manager_addr>:2377`.
pub fn swarm_join_command(token: &str, manager_addr: &str) -> String {
    format!("docker swarm join --token {token} {manager_addr}:2377")
}

// --------------------------------------------------------------------------
// Provisioning orchestration
// --------------------------------------------------------------------------

/// Everything the provisioning flow needs that is not an AWS call.
pub struct ProvisionInput {
    pub team_id: i64,
    pub name: String,
    pub region: String,
    pub instance_type: String,
    pub count: i32,
    pub key_id: i64,
    pub key_name: String,
    pub key_public: String,
    /// The decrypted private key material, materialised `0600` for swarm SSH.
    pub key_material: String,
    pub token_id: i64,
}

/// The result of a provisioning run.
pub struct ProvisionOutput {
    pub servers: Vec<Server>,
    pub swarm: bool,
    /// True when some non-fatal step failed (an instance had no public IP, or a
    /// worker failed to join the swarm) but the run still registered servers.
    pub partial: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error(transparent)]
    Aws(#[from] AwsError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Other(String),
}

/// Run the full AWS provisioning flow. Fatal AWS/DB errors propagate; per-node
/// issues (no public IP, worker join failure) are recorded as `partial` without
/// aborting the run.
#[allow(clippy::too_many_arguments)]
pub async fn provision_aws(
    aws: &dyn AwsApi,
    exec: &dyn CommandExecutor,
    pool: &sqlx::PgPool,
    queue: &JobQueue,
    key_dir: &Path,
    input: ProvisionInput,
) -> Result<ProvisionOutput, ProvisionError> {
    let multi = input.count > 1;

    // 1-4: network → security group → key pair → AMI.
    let net = aws.ensure_network().await?;
    let sg_id = aws.ensure_security_group(&net.vpc_id, multi).await?;
    let key_name = aws
        .ensure_key_pair(&input.key_name, &input.key_public)
        .await?;
    let ami = aws.latest_ubuntu_ami().await?;

    // 5-6: launch and wait.
    let spec = RunSpec {
        ami,
        instance_type: input.instance_type.clone(),
        count: input.count,
        security_group_id: sg_id,
        subnet_id: net.subnet_id,
        key_name,
        name_tag: input.name.clone(),
    };
    let instance_ids = aws.run_instances(&spec).await?;
    let infos = aws.wait_running(&instance_ids).await?;

    // 7: register a Rustify server per instance + enqueue validate. An instance
    // with no public IP is skipped (partial) rather than aborting the batch.
    let repo = ServerRepo::new(pool.clone());
    let mut partial = false;
    let mut nodes: Vec<(Server, Option<String>)> = Vec::new();
    for (idx, info) in infos.iter().enumerate() {
        let Some(public_ip) = info.public_ip.clone() else {
            partial = true;
            continue;
        };
        let node_name = if input.count > 1 {
            format!("{}-{}", input.name, idx + 1)
        } else {
            input.name.clone()
        };
        let server = repo
            .create_aws(NewAwsServer {
                team_id: input.team_id,
                name: node_name,
                ip: public_ip,
                port: 22,
                ssh_user: "ubuntu".to_string(),
                private_key_id: input.key_id,
                aws_instance_id: info.instance_id.clone(),
                aws_region: input.region.clone(),
                cloud_provider_token_id: input.token_id,
            })
            .await?;
        let _ = queue
            .enqueue(
                "server_validate",
                serde_json::json!({ "server_uuid": server.uuid }),
                None,
            )
            .await
            .map_err(|e| ProvisionError::Other(e.to_string()))?;
        nodes.push((server, info.private_ip.clone()));
    }

    // 8: single node → done; multi-node → form a Docker Swarm.
    let swarm = multi && nodes.len() > 1;
    if swarm {
        partial |= form_swarm(exec, &repo, key_dir, &input.key_material, &nodes).await;
    }

    Ok(ProvisionOutput {
        servers: nodes.into_iter().map(|(s, _)| s).collect(),
        swarm,
        partial,
    })
}

/// Form a Docker Swarm across `nodes` (first = manager). Returns `true` if any
/// worker failed to join (partial); a worker failure never fails the manager.
async fn form_swarm(
    exec: &dyn CommandExecutor,
    repo: &ServerRepo,
    key_dir: &Path,
    key_material: &str,
    nodes: &[(Server, Option<String>)],
) -> bool {
    let mut partial = false;
    let (manager, mgr_private) = &nodes[0];
    let mgr_addr = match mgr_private {
        Some(ip) => ip.clone(),
        None => {
            tracing::warn!(server = %manager.uuid, "swarm manager has no private IP; skipping");
            return true;
        }
    };

    let mgr_conn = conn_for(manager, key_dir, key_material);
    if let Err(e) = exec.exec(&mgr_conn, &docker_ensure_command(), opts()).await {
        tracing::warn!(server = %manager.uuid, error = %e, "docker ensure failed on manager");
        return true;
    }
    if let Err(e) = exec
        .exec(&mgr_conn, &swarm_init_command(&mgr_addr), opts())
        .await
    {
        tracing::warn!(server = %manager.uuid, error = %e, "swarm init failed");
        return true;
    }
    let _ = repo.set_swarm_role(manager.id, true, false).await;

    let token = match exec
        .exec(&mgr_conn, &swarm_join_token_command(), opts())
        .await
    {
        Ok(out) => out.stdout.trim().to_string(),
        Err(e) => {
            tracing::warn!(server = %manager.uuid, error = %e, "swarm join-token failed");
            return true;
        }
    };

    for (worker, _) in &nodes[1..] {
        let conn = conn_for(worker, key_dir, key_material);
        if exec
            .exec(&conn, &docker_ensure_command(), opts())
            .await
            .is_err()
        {
            partial = true;
            continue;
        }
        match exec
            .exec(&conn, &swarm_join_command(&token, &mgr_addr), opts())
            .await
        {
            Ok(_) => {
                let _ = repo.set_swarm_role(worker.id, false, true).await;
            }
            Err(e) => {
                tracing::warn!(server = %worker.uuid, error = %e, "swarm join failed");
                partial = true;
            }
        }
    }
    partial
}

fn opts() -> ExecOpts {
    ExecOpts::default()
}

/// Materialise the server's private key `0600` and build a direct `ServerConn`.
fn conn_for(server: &Server, key_dir: &Path, key_material: &str) -> ServerConn {
    let key_path: PathBuf = key_dir.join(&server.uuid);
    let _ = std::fs::create_dir_all(key_dir);
    if std::fs::write(&key_path, key_material).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    ServerConn {
        uuid: server.uuid.clone(),
        host: server.ip.clone(),
        port: server.port as u16,
        user: server.ssh_user.clone(),
        key_path,
        connection_timeout_secs: 10,
        proxy_command: None,
    }
}

// --------------------------------------------------------------------------
// Real EC2 + SSM client (built from the decrypted token; never ambient env)
// --------------------------------------------------------------------------

/// Build a region-scoped [`aws_config::SdkConfig`] from the decrypted token via
/// a static credentials provider. Never consults the ambient environment.
pub fn sdk_config(creds: &AwsCredentials, region: &str) -> aws_config::SdkConfig {
    let provider = aws_credential_types::Credentials::new(
        creds.access_key_id.clone(),
        creds.secret_access_key.clone(),
        None,
        None,
        "rustify",
    );
    aws_config::SdkConfig::builder()
        .behavior_version(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .credentials_provider(
            aws_credential_types::provider::SharedCredentialsProvider::new(provider),
        )
        .build()
}

/// Production [`AwsApi`] backed by the AWS SDK.
pub struct Ec2Client {
    ec2: aws_sdk_ec2::Client,
    ssm: aws_sdk_ssm::Client,
}

impl Ec2Client {
    /// Build region-scoped EC2 + SSM clients from the decrypted token.
    pub fn new(creds: &AwsCredentials, region: &str) -> Self {
        let conf = sdk_config(creds, region);
        Self {
            ec2: aws_sdk_ec2::Client::new(&conf),
            ssm: aws_sdk_ssm::Client::new(&conf),
        }
    }

    /// Region names via `DescribeRegions` (requires the token).
    pub async fn describe_regions(&self) -> Result<Vec<String>, AwsError> {
        let resp = self
            .ec2
            .describe_regions()
            .send()
            .await
            .map_err(api_err_sdk)?;
        Ok(resp
            .regions()
            .iter()
            .filter_map(|r| r.region_name().map(str::to_string))
            .collect())
    }
}

#[async_trait]
impl AwsApi for Ec2Client {
    async fn ensure_network(&self) -> Result<NetworkRefs, AwsError> {
        use aws_sdk_ec2::types::Filter;

        // Prefer the region default VPC.
        let default_vpc = self
            .ec2
            .describe_vpcs()
            .filters(Filter::builder().name("isDefault").values("true").build())
            .send()
            .await
            .map_err(api_err_sdk)?;
        let vpc_id = default_vpc
            .vpcs()
            .iter()
            .find_map(|v| v.vpc_id().map(str::to_string));

        // Else a previously Rustify-created VPC (tagged managed).
        let vpc_id = match vpc_id {
            Some(id) => id,
            None => {
                let tagged = self
                    .ec2
                    .describe_vpcs()
                    .filters(
                        Filter::builder()
                            .name(format!("tag:{MANAGED_TAG_KEY}"))
                            .values(MANAGED_TAG_VALUE)
                            .build(),
                    )
                    .send()
                    .await
                    .map_err(api_err_sdk)?;
                match tagged
                    .vpcs()
                    .iter()
                    .find_map(|v| v.vpc_id().map(str::to_string))
                {
                    Some(id) => id,
                    None => self.create_network().await?,
                }
            }
        };

        // A subnet in that VPC.
        let subnets = self
            .ec2
            .describe_subnets()
            .filters(Filter::builder().name("vpc-id").values(&vpc_id).build())
            .send()
            .await
            .map_err(api_err_sdk)?;
        let subnet_id = subnets
            .subnets()
            .iter()
            .find_map(|s| s.subnet_id().map(str::to_string))
            .ok_or_else(|| AwsError::Missing("subnet".to_string()))?;

        Ok(NetworkRefs { vpc_id, subnet_id })
    }

    async fn ensure_security_group(
        &self,
        vpc_id: &str,
        open_swarm_ports: bool,
    ) -> Result<String, AwsError> {
        use aws_sdk_ec2::types::Filter;

        let existing = self
            .ec2
            .describe_security_groups()
            .filters(
                Filter::builder()
                    .name("group-name")
                    .values(SECURITY_GROUP_NAME)
                    .build(),
            )
            .filters(Filter::builder().name("vpc-id").values(vpc_id).build())
            .send()
            .await
            .map_err(api_err_sdk)?;
        if let Some(id) = existing
            .security_groups()
            .iter()
            .find_map(|g| g.group_id().map(str::to_string))
        {
            return Ok(id);
        }

        let created = self
            .ec2
            .create_security_group()
            .group_name(SECURITY_GROUP_NAME)
            .description("Rustify-managed security group")
            .vpc_id(vpc_id)
            .send()
            .await
            .map_err(api_err_sdk)?;
        let sg_id = created
            .group_id()
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("group id".to_string()))?;

        self.authorize_ingress(&sg_id, open_swarm_ports).await?;
        Ok(sg_id)
    }

    async fn ensure_key_pair(&self, name: &str, public_key: &str) -> Result<String, AwsError> {
        use aws_sdk_ec2::primitives::Blob;
        use aws_sdk_ec2::types::Filter;

        let key_name = sanitize_key_name(name);
        let existing = self
            .ec2
            .describe_key_pairs()
            .filters(Filter::builder().name("key-name").values(&key_name).build())
            .send()
            .await
            .map_err(api_err_sdk)?;
        if existing
            .key_pairs()
            .iter()
            .any(|k| k.key_name() == Some(key_name.as_str()))
        {
            return Ok(key_name);
        }

        self.ec2
            .import_key_pair()
            .key_name(&key_name)
            .public_key_material(Blob::new(public_key.as_bytes()))
            .send()
            .await
            .map_err(api_err_sdk)?;
        Ok(key_name)
    }

    async fn latest_ubuntu_ami(&self) -> Result<String, AwsError> {
        let resp = self
            .ssm
            .get_parameter()
            .name(UBUNTU_2404_SSM_PARAM)
            .send()
            .await
            .map_err(api_err_sdk)?;
        resp.parameter()
            .and_then(|p| p.value())
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("ubuntu ami".to_string()))
    }

    async fn run_instances(&self, spec: &RunSpec) -> Result<Vec<String>, AwsError> {
        use aws_sdk_ec2::types::{
            InstanceType as SdkInstanceType, ResourceType, Tag, TagSpecification,
        };

        let tags = TagSpecification::builder()
            .resource_type(ResourceType::Instance)
            .tags(Tag::builder().key("Name").value(&spec.name_tag).build())
            .tags(
                Tag::builder()
                    .key(MANAGED_TAG_KEY)
                    .value(MANAGED_TAG_VALUE)
                    .build(),
            )
            .build();

        let resp = self
            .ec2
            .run_instances()
            .image_id(&spec.ami)
            .instance_type(SdkInstanceType::from(spec.instance_type.as_str()))
            .min_count(spec.count)
            .max_count(spec.count)
            .security_group_ids(&spec.security_group_id)
            .subnet_id(&spec.subnet_id)
            .key_name(&spec.key_name)
            .tag_specifications(tags)
            .send()
            .await
            .map_err(api_err_sdk)?;

        let ids: Vec<String> = resp
            .instances()
            .iter()
            .filter_map(|i| i.instance_id().map(str::to_string))
            .collect();
        if ids.is_empty() {
            return Err(AwsError::Missing("launched instances".to_string()));
        }
        Ok(ids)
    }

    async fn wait_running(&self, instance_ids: &[String]) -> Result<Vec<InstanceInfo>, AwsError> {
        // Cap the wait so a stuck launch cannot hang a worker forever.
        for _ in 0..40 {
            let infos = self.describe_instances(instance_ids).await?;
            let all_running = !infos.is_empty() && infos.iter().all(|i| i.state == "running");
            if all_running && self.status_ok(instance_ids).await? {
                return Ok(infos);
            }
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        }
        Err(AwsError::Timeout)
    }

    async fn describe_instances(
        &self,
        instance_ids: &[String],
    ) -> Result<Vec<InstanceInfo>, AwsError> {
        let mut req = self.ec2.describe_instances();
        for id in instance_ids {
            req = req.instance_ids(id);
        }
        let resp = req.send().await.map_err(api_err_sdk)?;
        let mut out = Vec::new();
        for reservation in resp.reservations() {
            for inst in reservation.instances() {
                let Some(id) = inst.instance_id() else {
                    continue;
                };
                out.push(InstanceInfo {
                    instance_id: id.to_string(),
                    public_ip: inst.public_ip_address().map(str::to_string),
                    private_ip: inst.private_ip_address().map(str::to_string),
                    state: inst
                        .state()
                        .and_then(|s| s.name())
                        .map(|n| n.as_str().to_string())
                        .unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }
}

impl Ec2Client {
    /// Whether every instance reports system + instance status `ok`.
    async fn status_ok(&self, instance_ids: &[String]) -> Result<bool, AwsError> {
        use aws_sdk_ec2::types::SummaryStatus;

        let mut req = self.ec2.describe_instance_status();
        for id in instance_ids {
            req = req.instance_ids(id);
        }
        let resp = req.send().await.map_err(api_err_sdk)?;
        let statuses = resp.instance_statuses();
        if statuses.len() < instance_ids.len() {
            return Ok(false);
        }
        Ok(statuses.iter().all(|s| {
            let inst_ok = s
                .instance_status()
                .and_then(|x| x.status())
                .map(|st| *st == SummaryStatus::Ok)
                .unwrap_or(false);
            let sys_ok = s
                .system_status()
                .and_then(|x| x.status())
                .map(|st| *st == SummaryStatus::Ok)
                .unwrap_or(false);
            inst_ok && sys_ok
        }))
    }

    /// Best-effort ingress rules for a freshly created SG: 22/80/443 open, plus
    /// the swarm ports (referencing the SG itself) when multi-node.
    async fn authorize_ingress(&self, sg_id: &str, open_swarm_ports: bool) -> Result<(), AwsError> {
        use aws_sdk_ec2::types::{IpPermission, IpRange, UserIdGroupPair};

        let cidr = |proto: &str, port: i32| {
            IpPermission::builder()
                .ip_protocol(proto)
                .from_port(port)
                .to_port(port)
                .ip_ranges(IpRange::builder().cidr_ip("0.0.0.0/0").build())
                .build()
        };
        let mut perms = vec![cidr("tcp", 22), cidr("tcp", 80), cidr("tcp", 443)];

        if open_swarm_ports {
            let sg_scoped = |proto: &str, port: i32| {
                IpPermission::builder()
                    .ip_protocol(proto)
                    .from_port(port)
                    .to_port(port)
                    .user_id_group_pairs(UserIdGroupPair::builder().group_id(sg_id).build())
                    .build()
            };
            perms.push(sg_scoped("tcp", 2377));
            perms.push(sg_scoped("tcp", 7946));
            perms.push(sg_scoped("udp", 7946));
            perms.push(sg_scoped("udp", 4789));
        }

        self.ec2
            .authorize_security_group_ingress()
            .group_id(sg_id)
            .set_ip_permissions(Some(perms))
            .send()
            .await
            .map_err(api_err_sdk)?;
        Ok(())
    }

    /// Create a minimal VPC + subnet + IGW + default route (only when the region
    /// has neither a default nor a previously Rustify-created VPC).
    async fn create_network(&self) -> Result<String, AwsError> {
        use aws_sdk_ec2::types::{ResourceType, Tag, TagSpecification};

        let managed_tags = |rt: ResourceType| {
            TagSpecification::builder()
                .resource_type(rt)
                .tags(
                    Tag::builder()
                        .key(MANAGED_TAG_KEY)
                        .value(MANAGED_TAG_VALUE)
                        .build(),
                )
                .build()
        };

        let vpc = self
            .ec2
            .create_vpc()
            .cidr_block("10.0.0.0/16")
            .tag_specifications(managed_tags(ResourceType::Vpc))
            .send()
            .await
            .map_err(api_err_sdk)?;
        let vpc_id = vpc
            .vpc()
            .and_then(|v| v.vpc_id())
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("vpc id".to_string()))?;

        let subnet = self
            .ec2
            .create_subnet()
            .vpc_id(&vpc_id)
            .cidr_block("10.0.1.0/24")
            .tag_specifications(managed_tags(ResourceType::Subnet))
            .send()
            .await
            .map_err(api_err_sdk)?;
        let subnet_id = subnet
            .subnet()
            .and_then(|s| s.subnet_id())
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("subnet id".to_string()))?;

        let igw = self
            .ec2
            .create_internet_gateway()
            .tag_specifications(managed_tags(ResourceType::InternetGateway))
            .send()
            .await
            .map_err(api_err_sdk)?;
        let igw_id = igw
            .internet_gateway()
            .and_then(|g| g.internet_gateway_id())
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("igw id".to_string()))?;

        self.ec2
            .attach_internet_gateway()
            .internet_gateway_id(&igw_id)
            .vpc_id(&vpc_id)
            .send()
            .await
            .map_err(api_err_sdk)?;

        let rt = self
            .ec2
            .create_route_table()
            .vpc_id(&vpc_id)
            .tag_specifications(managed_tags(ResourceType::RouteTable))
            .send()
            .await
            .map_err(api_err_sdk)?;
        let rt_id = rt
            .route_table()
            .and_then(|t| t.route_table_id())
            .map(str::to_string)
            .ok_or_else(|| AwsError::Missing("route table id".to_string()))?;

        self.ec2
            .create_route()
            .route_table_id(&rt_id)
            .destination_cidr_block("0.0.0.0/0")
            .gateway_id(&igw_id)
            .send()
            .await
            .map_err(api_err_sdk)?;

        self.ec2
            .associate_route_table()
            .route_table_id(&rt_id)
            .subnet_id(&subnet_id)
            .send()
            .await
            .map_err(api_err_sdk)?;

        Ok(vpc_id)
    }
}

/// Sanitize a private-key name into an EC2 key-pair name.
fn sanitize_key_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("rustify-{}", cleaned.trim_matches('-'))
}

/// Map an SDK `SdkError` to [`AwsError::Api`], walking the source chain so the
/// service message (`AuthFailure`, `UnauthorizedOperation`,
/// `InsufficientInstanceCapacity`, …) is preserved. `SdkError` is the same type
/// re-exported by every service crate, so this handles EC2 and SSM alike.
fn api_err_sdk<E, R>(e: aws_sdk_ec2::error::SdkError<E, R>) -> AwsError
where
    E: std::error::Error + 'static,
    R: std::fmt::Debug,
{
    use std::error::Error as _;
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    AwsError::Api(msg.replace('\n', " "))
}

// --------------------------------------------------------------------------
// Periodic instance-state sync (extends the cloud status task set)
// --------------------------------------------------------------------------

/// Scheduler task: reconcile every AWS-provisioned server's cached instance
/// state. Non-destructive (cache only), mirroring the Hetzner sync.
pub fn aws_status_sync_task(
    pool: sqlx::PgPool,
) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + 'static {
    move || {
        let pool = pool.clone();
        Box::pin(async move {
            if let Err(e) = sync_aws_all(&pool).await {
                tracing::warn!(error = %e, "aws status sync failed");
            }
        })
    }
}

/// One AWS instance-state reconciliation sweep across all provisioned servers.
pub async fn sync_aws_all(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let repo = ServerRepo::new(pool.clone());
    let servers = repo
        .aws_servers()
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    for server in servers {
        let (Some(instance_id), Some(region), Some(token_id)) = (
            server.aws_instance_id.clone(),
            server.aws_region.clone(),
            server.cloud_provider_token_id,
        ) else {
            continue;
        };
        let enc: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT token_enc FROM cloud_provider_tokens WHERE id = $1")
                .bind(token_id)
                .fetch_optional(pool)
                .await?;
        let Some(enc) = enc else { continue };
        let Ok(plain) = rustify_core::crypto::decrypt(&enc) else {
            continue;
        };
        let Ok(creds) = serde_json::from_slice::<AwsCredentials>(&plain) else {
            continue;
        };
        let client = Ec2Client::new(&creds, &region);
        match client.describe_instances(&[instance_id]).await {
            Ok(infos) => {
                if let Some(info) = infos.first() {
                    let _ = repo.set_aws_status(server.id, &info.state).await;
                }
            }
            Err(e) => {
                tracing::debug!(server = %server.uuid, error = %e, "aws describe_instances failed");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
