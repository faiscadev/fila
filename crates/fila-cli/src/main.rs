use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::{
    CreateApiKeyRequest, CreateQueueRequest, DeleteQueueRequest, GetConfigRequest, GetStatsRequest,
    ListApiKeysRequest, ListConfigRequest, ListQueuesRequest, QueueConfig, RedriveRequest,
    RevokeApiKeyRequest, SetConfigRequest,
};
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

#[derive(Parser)]
#[command(name = "fila", about = "Fila message broker CLI")]
struct Cli {
    /// Broker address (use https:// when TLS is enabled)
    #[arg(long, default_value = "http://localhost:5555", global = true)]
    addr: String,

    /// CA certificate for verifying the server's TLS certificate.
    /// Required for any TLS connection.
    #[arg(long, global = true)]
    tls_ca_cert: Option<PathBuf>,

    /// Client certificate for mTLS. Requires --tls-key and --tls-ca-cert.
    #[arg(long, global = true, requires = "tls_key", requires = "tls_ca_cert")]
    tls_cert: Option<PathBuf>,

    /// Client private key for mTLS. Requires --tls-cert.
    #[arg(long, global = true, requires = "tls_cert")]
    tls_key: Option<PathBuf>,

    /// API key for authenticating with the broker (sent as `authorization: Bearer <key>`).
    #[arg(long, global = true)]
    api_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage queues
    #[command(subcommand)]
    Queue(QueueCommands),

    /// Manage runtime configuration
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Redrive messages from a dead-letter queue back to the source queue
    Redrive {
        /// DLQ queue name (e.g. "orders.dlq")
        dlq_name: String,

        /// Maximum number of messages to redrive (0 = all)
        #[arg(long, default_value = "0")]
        count: u64,
    },

    /// Manage API keys
    #[command(subcommand)]
    Auth(AuthCommands),
}

#[derive(Subcommand)]
enum QueueCommands {
    /// Create a new queue
    Create {
        /// Queue name
        name: String,

        /// Lua script executed on enqueue
        #[arg(long)]
        on_enqueue: Option<String>,

        /// Lua script executed on failure
        #[arg(long)]
        on_failure: Option<String>,

        /// Visibility timeout in milliseconds
        #[arg(long)]
        visibility_timeout: Option<u64>,
    },

    /// Delete a queue
    Delete {
        /// Queue name
        name: String,
    },

    /// List all queues
    List,

    /// Show detailed queue statistics
    Inspect {
        /// Queue name
        name: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Set a runtime config value
    Set {
        /// Config key
        key: String,

        /// Config value
        value: String,
    },

    /// Get a runtime config value
    Get {
        /// Config key
        key: String,
    },

    /// List config entries
    List {
        /// Filter by key prefix
        #[arg(long, default_value = "")]
        prefix: String,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Create a new API key
    Create {
        /// Human-readable name for the key
        #[arg(long)]
        name: String,

        /// Expiry as Unix timestamp in milliseconds (optional; omit for no expiry)
        #[arg(long)]
        expires_at: Option<u64>,
    },

    /// Revoke an API key by its key ID
    Revoke {
        /// Key ID (as returned by `auth create` or `auth list`)
        key_id: String,
    },

    /// List all API keys
    List,
}

/// Interceptor that attaches an API key as `authorization: Bearer <key>`.
#[derive(Clone)]
struct ApiKeyInterceptor(Option<String>);

impl tonic::service::Interceptor for ApiKeyInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(ref key) = self.0 {
            if let Ok(val) =
                tonic::metadata::MetadataValue::try_from(format!("Bearer {key}").as_str())
            {
                req.metadata_mut().insert("authorization", val);
            }
        }
        Ok(req)
    }
}

type AdminClient = FilaAdminClient<InterceptedService<Channel, ApiKeyInterceptor>>;

async fn connect(
    addr: &str,
    tls_ca_cert: Option<&PathBuf>,
    tls_cert: Option<&PathBuf>,
    tls_key: Option<&PathBuf>,
    api_key: Option<String>,
) -> AdminClient {
    let channel = if let Some(ca_path) = tls_ca_cert {
        let ca_pem = match std::fs::read(ca_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error: cannot read CA cert {}: {e}", ca_path.display());
                process::exit(1);
            }
        };
        let mut tls = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(ca_pem));
        if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
            let cert_pem = match std::fs::read(cert_path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "Error: cannot read client cert {}: {e}",
                        cert_path.display()
                    );
                    process::exit(1);
                }
            };
            let key_pem = match std::fs::read(key_path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Error: cannot read client key {}: {e}", key_path.display());
                    process::exit(1);
                }
            };
            tls = tls.identity(Identity::from_pem(cert_pem, key_pem));
        }
        let endpoint = match Channel::from_shared(addr.to_string()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Error: invalid broker address {addr}: {e}");
                process::exit(1);
            }
        };
        let endpoint = match endpoint.tls_config(tls) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Error: TLS configuration failed: {e}");
                process::exit(1);
            }
        };
        match endpoint.connect().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error: cannot connect to broker at {addr}: {e}");
                process::exit(1);
            }
        }
    } else {
        match Channel::from_shared(addr.to_string()) {
            Ok(e) => match e.connect().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: cannot connect to broker at {addr}: {e}");
                    process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error: invalid broker address {addr}: {e}");
                process::exit(1);
            }
        }
    };
    FilaAdminClient::with_interceptor(channel, ApiKeyInterceptor(api_key))
}

fn format_rpc_error(status: tonic::Status, context: &str) -> String {
    match status.code() {
        tonic::Code::NotFound => format!("Error: {context} does not exist"),
        tonic::Code::AlreadyExists => format!("Error: {context} already exists"),
        tonic::Code::InvalidArgument => format!("Error: {}", status.message()),
        tonic::Code::FailedPrecondition => format!("Error: {}", status.message()),
        tonic::Code::ResourceExhausted => "Error: broker overloaded, try again".to_string(),
        tonic::Code::Unavailable => {
            "Error: broker unavailable (connection lost or server down)".to_string()
        }
        _ => format!("Error: {}", status.message()),
    }
}

async fn cmd_queue_create(
    client: &mut AdminClient,
    name: String,
    on_enqueue: Option<String>,
    on_failure: Option<String>,
    visibility_timeout: Option<u64>,
) {
    let config = QueueConfig {
        on_enqueue_script: on_enqueue.unwrap_or_default(),
        on_failure_script: on_failure.unwrap_or_default(),
        visibility_timeout_ms: visibility_timeout.unwrap_or(0),
    };

    match client
        .create_queue(CreateQueueRequest {
            name: name.clone(),
            config: Some(config),
        })
        .await
    {
        Ok(_) => println!("Created queue \"{name}\""),
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_queue_delete(client: &mut AdminClient, name: String) {
    match client
        .delete_queue(DeleteQueueRequest {
            queue: name.clone(),
        })
        .await
    {
        Ok(_) => println!("Deleted queue \"{name}\""),
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_queue_list(client: &mut AdminClient) {
    match client.list_queues(ListQueuesRequest {}).await {
        Ok(resp) => {
            let inner = resp.into_inner();
            let queues = inner.queues;
            let cluster_node_count = inner.cluster_node_count;
            if queues.is_empty() {
                println!("No queues found.");
                return;
            }

            let is_cluster = cluster_node_count > 0;

            // Calculate column widths
            let name_width = queues
                .iter()
                .map(|q| q.name.len())
                .max()
                .unwrap_or(4)
                .max(4);

            if is_cluster {
                println!(
                    "{:<name_width$}  {:>7}  {:>9}  {:>9}  {:>6}",
                    "NAME", "DEPTH", "IN_FLIGHT", "CONSUMERS", "LEADER"
                );
                for q in &queues {
                    println!(
                        "{:<name_width$}  {:>7}  {:>9}  {:>9}  {:>6}",
                        q.name, q.depth, q.in_flight, q.active_consumers, q.leader_node_id
                    );
                }
                println!("\nCluster nodes: {cluster_node_count}");
            } else {
                println!(
                    "{:<name_width$}  {:>7}  {:>9}  {:>9}",
                    "NAME", "DEPTH", "IN_FLIGHT", "CONSUMERS"
                );
                for q in &queues {
                    println!(
                        "{:<name_width$}  {:>7}  {:>9}  {:>9}",
                        q.name, q.depth, q.in_flight, q.active_consumers
                    );
                }
            }
        }
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, "queues"));
            process::exit(1);
        }
    }
}

async fn cmd_queue_inspect(client: &mut AdminClient, name: String) {
    match client
        .get_stats(GetStatsRequest {
            queue: name.clone(),
        })
        .await
    {
        Ok(resp) => {
            let stats = resp.into_inner();
            println!("Queue: {name}");
            println!("  Depth:                {}", stats.depth);
            println!("  In-flight:            {}", stats.in_flight);
            println!("  Active fairness keys: {}", stats.active_fairness_keys);
            println!("  Active consumers:     {}", stats.active_consumers);
            println!("  Quantum:              {}", stats.quantum);
            if stats.leader_node_id > 0 || stats.replication_count > 0 {
                println!("  Raft leader:          node {}", stats.leader_node_id);
                println!("  Replicas:             {}", stats.replication_count);
            }

            if !stats.per_key_stats.is_empty() {
                println!();
                let key_width = stats
                    .per_key_stats
                    .iter()
                    .map(|k| k.key.len())
                    .max()
                    .unwrap_or(3)
                    .max(3);
                println!(
                    "  {:<key_width$}  {:>7}  {:>8}  {:>6}",
                    "KEY", "PENDING", "DEFICIT", "WEIGHT"
                );
                for k in &stats.per_key_stats {
                    println!(
                        "  {:<key_width$}  {:>7}  {:>8}  {:>6}",
                        k.key, k.pending_count, k.current_deficit, k.weight
                    );
                }
            }

            if !stats.per_throttle_stats.is_empty() {
                println!();
                let key_width = stats
                    .per_throttle_stats
                    .iter()
                    .map(|k| k.key.len())
                    .max()
                    .unwrap_or(3)
                    .max(3);
                println!(
                    "  {:<key_width$}  {:>8}  {:>8}  {:>8}",
                    "KEY", "TOKENS", "RATE/S", "BURST"
                );
                for k in &stats.per_throttle_stats {
                    println!(
                        "  {:<key_width$}  {:>8.1}  {:>8.1}  {:>8.1}",
                        k.key, k.tokens, k.rate_per_second, k.burst
                    );
                }
            }
        }
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_config_set(client: &mut AdminClient, key: String, value: String) {
    match client
        .set_config(SetConfigRequest {
            key: key.clone(),
            value: value.clone(),
        })
        .await
    {
        Ok(_) => println!("Set \"{key}\" = \"{value}\""),
        Err(status) => {
            eprintln!(
                "{}",
                format_rpc_error(status, &format!("config key \"{key}\""))
            );
            process::exit(1);
        }
    }
}

async fn cmd_config_get(client: &mut AdminClient, key: String) {
    match client
        .get_config(GetConfigRequest { key: key.clone() })
        .await
    {
        Ok(resp) => {
            let value = resp.into_inner().value;
            if value.is_empty() {
                println!("{key}: (not set)");
            } else {
                println!("{key}: {value}");
            }
        }
        Err(status) => {
            eprintln!(
                "{}",
                format_rpc_error(status, &format!("config key \"{key}\""))
            );
            process::exit(1);
        }
    }
}

async fn cmd_config_list(client: &mut AdminClient, prefix: String) {
    match client
        .list_config(ListConfigRequest {
            prefix: prefix.clone(),
        })
        .await
    {
        Ok(resp) => {
            let entries = resp.into_inner().entries;
            if entries.is_empty() {
                if prefix.is_empty() {
                    println!("No config entries found.");
                } else {
                    println!("No config entries matching prefix \"{prefix}\".");
                }
                return;
            }

            let key_width = entries
                .iter()
                .map(|e| e.key.len())
                .max()
                .unwrap_or(3)
                .max(3);

            println!("{:<key_width$}  VALUE", "KEY");
            for e in &entries {
                println!("{:<key_width$}  {}", e.key, e.value);
            }
        }
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, "config"));
            process::exit(1);
        }
    }
}

async fn cmd_redrive(client: &mut AdminClient, dlq_name: String, count: u64) {
    match client
        .redrive(RedriveRequest {
            dlq_queue: dlq_name.clone(),
            count,
        })
        .await
    {
        Ok(resp) => {
            let n = resp.into_inner().redriven;
            println!(
                "Redrive complete: {n} message{} moved from \"{dlq_name}\"",
                if n == 1 { "" } else { "s" }
            );
        }
        Err(status) => {
            eprintln!(
                "{}",
                format_rpc_error(status, &format!("queue \"{dlq_name}\""))
            );
            process::exit(1);
        }
    }
}

async fn cmd_auth_create(client: &mut AdminClient, name: String, expires_at: Option<u64>) {
    match client
        .create_api_key(CreateApiKeyRequest {
            name: name.clone(),
            expires_at_ms: expires_at.unwrap_or(0),
        })
        .await
    {
        Ok(resp) => {
            let r = resp.into_inner();
            println!("Created API key \"{name}\"");
            println!("  Key ID : {}", r.key_id);
            println!("  Token  : {}", r.key);
            println!("Store the token securely — it will not be shown again.");
        }
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, "api key"));
            process::exit(1);
        }
    }
}

async fn cmd_auth_revoke(client: &mut AdminClient, key_id: String) {
    match client
        .revoke_api_key(RevokeApiKeyRequest {
            key_id: key_id.clone(),
        })
        .await
    {
        Ok(_) => {
            println!("Revoked API key {key_id}");
        }
        Err(status) => {
            eprintln!(
                "{}",
                format_rpc_error(status, &format!("api key \"{key_id}\""))
            );
            process::exit(1);
        }
    }
}

async fn cmd_auth_list(client: &mut AdminClient) {
    match client.list_api_keys(ListApiKeysRequest {}).await {
        Ok(resp) => {
            let keys = resp.into_inner().keys;
            if keys.is_empty() {
                println!("No API keys found.");
                return;
            }

            let id_width = keys
                .iter()
                .map(|k| k.key_id.len())
                .max()
                .unwrap_or(6)
                .max(6);
            let name_width = keys.iter().map(|k| k.name.len()).max().unwrap_or(4).max(4);

            println!(
                "{:<id_width$}  {:<name_width$}  CREATED_AT_MS  EXPIRES_AT_MS",
                "KEY_ID", "NAME"
            );
            for k in &keys {
                let expires = if k.expires_at_ms == 0 {
                    "never".to_string()
                } else {
                    k.expires_at_ms.to_string()
                };
                println!(
                    "{:<id_width$}  {:<name_width$}  {:>13}  {expires}",
                    k.key_id, k.name, k.created_at_ms
                );
            }
        }
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, "api keys"));
            process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut client = connect(
        &cli.addr,
        cli.tls_ca_cert.as_ref(),
        cli.tls_cert.as_ref(),
        cli.tls_key.as_ref(),
        cli.api_key,
    )
    .await;

    match cli.command {
        Commands::Queue(cmd) => match cmd {
            QueueCommands::Create {
                name,
                on_enqueue,
                on_failure,
                visibility_timeout,
            } => {
                cmd_queue_create(
                    &mut client,
                    name,
                    on_enqueue,
                    on_failure,
                    visibility_timeout,
                )
                .await
            }
            QueueCommands::Delete { name } => cmd_queue_delete(&mut client, name).await,
            QueueCommands::List => cmd_queue_list(&mut client).await,
            QueueCommands::Inspect { name } => cmd_queue_inspect(&mut client, name).await,
        },
        Commands::Config(cmd) => match cmd {
            ConfigCommands::Set { key, value } => cmd_config_set(&mut client, key, value).await,
            ConfigCommands::Get { key } => cmd_config_get(&mut client, key).await,
            ConfigCommands::List { prefix } => cmd_config_list(&mut client, prefix).await,
        },
        Commands::Redrive { dlq_name, count } => cmd_redrive(&mut client, dlq_name, count).await,
        Commands::Auth(cmd) => match cmd {
            AuthCommands::Create { name, expires_at } => {
                cmd_auth_create(&mut client, name, expires_at).await
            }
            AuthCommands::Revoke { key_id } => cmd_auth_revoke(&mut client, key_id).await,
            AuthCommands::List => cmd_auth_list(&mut client).await,
        },
    }
}
