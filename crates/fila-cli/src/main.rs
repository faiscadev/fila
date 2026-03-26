use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use fila_sdk::{FibpTransport, StatusError};

#[derive(Parser)]
#[command(name = "fila", about = "Fila message broker CLI")]
struct Cli {
    /// Broker address (host:port)
    #[arg(long, default_value = "localhost:5555", global = true)]
    addr: String,

    /// Enable TLS using the system trust store.
    /// Use when the server has a certificate from a public or system-trusted CA.
    /// Not needed when --tls-ca-cert is provided (TLS is implied).
    #[arg(long, global = true)]
    tls: bool,

    /// CA certificate for verifying the server's TLS certificate.
    /// Use for self-signed certificates. Implies --tls.
    #[arg(long, global = true)]
    tls_ca_cert: Option<PathBuf>,

    /// Client certificate for mTLS. Requires --tls-key and either --tls or --tls-ca-cert.
    #[arg(long, global = true, requires = "tls_key")]
    tls_cert: Option<PathBuf>,

    /// Client private key for mTLS. Requires --tls-cert.
    #[arg(long, global = true, requires = "tls_cert")]
    tls_key: Option<PathBuf>,

    /// API key for authenticating with the broker.
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

        /// Grant this key superadmin privileges (bypasses all ACL checks)
        #[arg(long)]
        superadmin: bool,
    },

    /// Revoke an API key by its key ID
    Revoke {
        /// Key ID (as returned by `auth create` or `auth list`)
        key_id: String,
    },

    /// List all API keys
    List,

    /// Manage per-queue ACL permissions for an API key
    #[command(subcommand)]
    Acl(AclCommands),
}

#[derive(Subcommand)]
enum AclCommands {
    /// Set ACL permissions for an API key (replaces existing permissions)
    ///
    /// Each permission is `<kind>:<pattern>` where kind is one of:
    /// produce, consume, admin -- and pattern is a queue name or wildcard
    /// (e.g. `*`, `orders.*`).
    Set {
        /// Key ID to configure
        key_id: String,

        /// Permission entries in `<kind>:<pattern>` format (repeatable)
        #[arg(long = "perm", value_name = "KIND:PATTERN")]
        permissions: Vec<String>,
    },

    /// Get ACL permissions for an API key
    Get {
        /// Key ID to inspect
        key_id: String,
    },
}

async fn connect(cli: &Cli) -> FibpTransport {
    let tls_enabled = cli.tls || cli.tls_ca_cert.is_some() || cli.tls_cert.is_some();

    // Strip any http:// or https:// prefix if present.
    let addr = cli
        .addr
        .strip_prefix("http://")
        .or_else(|| cli.addr.strip_prefix("https://"))
        .unwrap_or(&cli.addr);

    if tls_enabled {
        let ca_pem = cli.tls_ca_cert.as_ref().map(|path| {
            std::fs::read(path).unwrap_or_else(|e| {
                eprintln!("Error: cannot read CA cert {}: {e}", path.display());
                process::exit(1);
            })
        });
        let (cert_pem, key_pem) = match (&cli.tls_cert, &cli.tls_key) {
            (Some(cert_path), Some(key_path)) => {
                let cert = std::fs::read(cert_path).unwrap_or_else(|e| {
                    eprintln!(
                        "Error: cannot read client cert {}: {e}",
                        cert_path.display()
                    );
                    process::exit(1);
                });
                let key = std::fs::read(key_path).unwrap_or_else(|e| {
                    eprintln!("Error: cannot read client key {}: {e}", key_path.display());
                    process::exit(1);
                });
                (Some(cert), Some(key))
            }
            _ => (None, None),
        };
        match FibpTransport::connect_tls(
            addr,
            cli.api_key.clone(),
            ca_pem.as_deref(),
            cert_pem.as_deref(),
            key_pem.as_deref(),
        )
        .await
        {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: cannot connect to broker at {addr}: {e}");
                process::exit(1);
            }
        }
    } else {
        match FibpTransport::connect(addr, cli.api_key.clone()).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: cannot connect to broker at {addr}: {e}");
                process::exit(1);
            }
        }
    }
}

fn format_error(err: StatusError, context: &str) -> String {
    match err {
        StatusError::Internal(msg) if msg.contains("not found") => {
            format!("Error: {context} does not exist")
        }
        StatusError::Internal(msg) if msg.contains("already exists") => {
            format!("Error: {context} already exists")
        }
        StatusError::InvalidArgument(msg) => format!("Error: {msg}"),
        StatusError::Unavailable(msg) => format!("Error: broker unavailable: {msg}"),
        StatusError::Internal(msg) => format!("Error: {msg}"),
        StatusError::Protocol(msg) => format!("Error: protocol error: {msg}"),
    }
}

async fn cmd_queue_create(
    transport: &FibpTransport,
    name: String,
    on_enqueue: Option<String>,
    on_failure: Option<String>,
    visibility_timeout: Option<u64>,
) {
    let config = fila_sdk::proto::QueueConfig {
        on_enqueue_script: on_enqueue.unwrap_or_default(),
        on_failure_script: on_failure.unwrap_or_default(),
        visibility_timeout_ms: visibility_timeout.unwrap_or(0),
    };

    match transport.create_queue(&name, Some(config)).await {
        Ok(_) => println!("Created queue \"{name}\""),
        Err(err) => {
            eprintln!("{}", format_error(err, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_queue_delete(transport: &FibpTransport, name: String) {
    match transport.delete_queue(&name).await {
        Ok(_) => println!("Deleted queue \"{name}\""),
        Err(err) => {
            eprintln!("{}", format_error(err, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_queue_list(transport: &FibpTransport) {
    match transport.list_queues().await {
        Ok(resp) => {
            let queues = resp.queues;
            let cluster_node_count = resp.cluster_node_count;
            if queues.is_empty() {
                println!("No queues found.");
                return;
            }

            let is_cluster = cluster_node_count > 0;

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
        Err(err) => {
            eprintln!("{}", format_error(err, "queues"));
            process::exit(1);
        }
    }
}

async fn cmd_queue_inspect(transport: &FibpTransport, name: String) {
    match transport.queue_stats(&name).await {
        Ok(stats) => {
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
        Err(err) => {
            eprintln!("{}", format_error(err, &format!("queue \"{name}\"")));
            process::exit(1);
        }
    }
}

async fn cmd_config_set(_transport: &FibpTransport, key: String, value: String) {
    // Config set/get/list are admin operations available via FIBP admin handler.
    // For now, use create_queue's admin frame pattern. The FIBP transport
    // exposes admin operations directly.
    // TODO: The FibpTransport needs set_config/get_config/list_config methods.
    // For now we'll show a clear error since these admin ops may not be on
    // the FibpTransport yet.
    eprintln!("Error: config set not yet implemented via FIBP CLI");
    eprintln!("  key={key} value={value}");
    process::exit(1);
}

async fn cmd_config_get(_transport: &FibpTransport, key: String) {
    eprintln!("Error: config get not yet implemented via FIBP CLI");
    eprintln!("  key={key}");
    process::exit(1);
}

async fn cmd_config_list(_transport: &FibpTransport, prefix: String) {
    eprintln!("Error: config list not yet implemented via FIBP CLI");
    if !prefix.is_empty() {
        eprintln!("  prefix={prefix}");
    }
    process::exit(1);
}

async fn cmd_redrive(_transport: &FibpTransport, dlq_name: String, count: u64) {
    eprintln!("Error: redrive not yet implemented via FIBP CLI");
    eprintln!("  dlq={dlq_name} count={count}");
    process::exit(1);
}

async fn cmd_auth_create(
    _transport: &FibpTransport,
    name: String,
    expires_at: Option<u64>,
    is_superadmin: bool,
) {
    eprintln!("Error: auth create not yet implemented via FIBP CLI");
    eprintln!("  name={name} expires_at={expires_at:?} superadmin={is_superadmin}");
    process::exit(1);
}

async fn cmd_auth_acl_set(_transport: &FibpTransport, key_id: String, permissions: Vec<String>) {
    eprintln!("Error: auth acl set not yet implemented via FIBP CLI");
    eprintln!("  key_id={key_id} permissions={permissions:?}");
    process::exit(1);
}

async fn cmd_auth_acl_get(_transport: &FibpTransport, key_id: String) {
    eprintln!("Error: auth acl get not yet implemented via FIBP CLI");
    eprintln!("  key_id={key_id}");
    process::exit(1);
}

async fn cmd_auth_revoke(_transport: &FibpTransport, key_id: String) {
    eprintln!("Error: auth revoke not yet implemented via FIBP CLI");
    eprintln!("  key_id={key_id}");
    process::exit(1);
}

async fn cmd_auth_list(_transport: &FibpTransport) {
    eprintln!("Error: auth list not yet implemented via FIBP CLI");
    process::exit(1);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let transport = connect(&cli).await;

    match cli.command {
        Commands::Queue(cmd) => match cmd {
            QueueCommands::Create {
                name,
                on_enqueue,
                on_failure,
                visibility_timeout,
            } => {
                cmd_queue_create(&transport, name, on_enqueue, on_failure, visibility_timeout).await
            }
            QueueCommands::Delete { name } => cmd_queue_delete(&transport, name).await,
            QueueCommands::List => cmd_queue_list(&transport).await,
            QueueCommands::Inspect { name } => cmd_queue_inspect(&transport, name).await,
        },
        Commands::Config(cmd) => match cmd {
            ConfigCommands::Set { key, value } => cmd_config_set(&transport, key, value).await,
            ConfigCommands::Get { key } => cmd_config_get(&transport, key).await,
            ConfigCommands::List { prefix } => cmd_config_list(&transport, prefix).await,
        },
        Commands::Redrive { dlq_name, count } => cmd_redrive(&transport, dlq_name, count).await,
        Commands::Auth(cmd) => match cmd {
            AuthCommands::Create {
                name,
                expires_at,
                superadmin,
            } => cmd_auth_create(&transport, name, expires_at, superadmin).await,
            AuthCommands::Revoke { key_id } => cmd_auth_revoke(&transport, key_id).await,
            AuthCommands::List => cmd_auth_list(&transport).await,
            AuthCommands::Acl(acl_cmd) => match acl_cmd {
                AclCommands::Set {
                    key_id,
                    permissions,
                } => cmd_auth_acl_set(&transport, key_id, permissions).await,
                AclCommands::Get { key_id } => cmd_auth_acl_get(&transport, key_id).await,
            },
        },
    }
}
