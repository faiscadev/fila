use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use bytes::BytesMut;
use clap::{Parser, Subcommand};
use fila_fibp::{
    AclPermission, CreateApiKeyRequest, CreateApiKeyResponse, CreateQueueResponse,
    DeleteQueueResponse, ErrorCode, ErrorFrame, GetAclResponse, GetConfigResponse,
    GetStatsResponse, Handshake, HandshakeOk, ListApiKeysResponse, ListConfigResponse,
    ListQueuesResponse, Opcode, ProtocolMessage, RawFrame, RedriveResponse, RevokeApiKeyResponse,
    SetAclResponse, SetConfigResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

#[derive(Parser)]
#[command(name = "fila", about = "Fila message broker CLI")]
struct Cli {
    /// Broker address (host:port for binary protocol)
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
    /// produce, consume, admin — and pattern is a queue name or wildcard
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

// ---------------------------------------------------------------------------
// Stream abstraction (plain TCP or TLS)
// ---------------------------------------------------------------------------

enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl tokio::io::AsyncRead for Stream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Stream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

// ---------------------------------------------------------------------------
// CLI Client
// ---------------------------------------------------------------------------

struct CliClient {
    stream: Stream,
    read_buf: BytesMut,
    next_request_id: u32,
}

impl CliClient {
    async fn send_recv(&mut self, frame: &RawFrame) -> Result<RawFrame, String> {
        // Encode the frame
        let mut wire = BytesMut::new();
        frame.encode(&mut wire);

        self.stream
            .write_all(&wire)
            .await
            .map_err(|e| format!("write error: {e}"))?;
        self.stream
            .flush()
            .await
            .map_err(|e| format!("flush error: {e}"))?;

        // Read response
        loop {
            match RawFrame::decode(&mut self.read_buf) {
                Ok(Some(resp)) => return Ok(resp),
                Ok(None) => {
                    let n = self
                        .stream
                        .read_buf(&mut self.read_buf)
                        .await
                        .map_err(|e| format!("read error: {e}"))?;
                    if n == 0 {
                        return Err("connection closed by server".to_string());
                    }
                }
                Err(e) => return Err(format!("frame decode error: {e}")),
            }
        }
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// Connection & handshake
// ---------------------------------------------------------------------------

async fn connect(
    addr: &str,
    tls: bool,
    tls_ca_cert: Option<&PathBuf>,
    tls_cert: Option<&PathBuf>,
    tls_key: Option<&PathBuf>,
    api_key: Option<String>,
) -> CliClient {
    // Strip http:// or https:// prefix if provided (backward compat with old CLI usage)
    let addr = addr
        .strip_prefix("http://")
        .or_else(|| addr.strip_prefix("https://"))
        .unwrap_or(addr);

    let tls_enabled = tls || tls_ca_cert.is_some() || tls_cert.is_some();

    let tcp = match TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: cannot connect to broker at {addr}: {e}");
            process::exit(1);
        }
    };

    let stream = if tls_enabled {
        let mut root_store = rustls::RootCertStore::empty();

        if let Some(ca_path) = tls_ca_cert {
            let ca_pem = match std::fs::read(ca_path) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Error: cannot read CA cert {}: {e}", ca_path.display());
                    process::exit(1);
                }
            };
            let certs = rustls_pemfile::certs(&mut &ca_pem[..])
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|e| {
                    eprintln!("Error: cannot parse CA cert: {e}");
                    process::exit(1);
                });
            for cert in certs {
                root_store.add(cert).unwrap_or_else(|e| {
                    eprintln!("Error: cannot add CA cert: {e}");
                    process::exit(1);
                });
            }
        } else {
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }

        let config = if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
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

            let certs = rustls_pemfile::certs(&mut &cert_pem[..])
                .collect::<Result<Vec<_>, _>>()
                .unwrap_or_else(|e| {
                    eprintln!("Error: cannot parse client cert: {e}");
                    process::exit(1);
                });
            let key = rustls_pemfile::private_key(&mut &key_pem[..])
                .unwrap_or_else(|e| {
                    eprintln!("Error: cannot parse client key: {e}");
                    process::exit(1);
                })
                .unwrap_or_else(|| {
                    eprintln!("Error: no private key found in key file");
                    process::exit(1);
                });

            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_client_auth_cert(certs, key)
                .unwrap_or_else(|e| {
                    eprintln!("Error: TLS client auth configuration failed: {e}");
                    process::exit(1);
                })
        } else {
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth()
        };

        // Extract hostname from addr for SNI
        let hostname = addr.split(':').next().unwrap_or("localhost");
        let server_name = rustls::pki_types::ServerName::try_from(hostname.to_string())
            .unwrap_or_else(|e| {
                eprintln!("Error: invalid server name '{hostname}': {e}");
                process::exit(1);
            });

        let connector = TlsConnector::from(Arc::new(config));
        match connector.connect(server_name, tcp).await {
            Ok(tls_stream) => Stream::Tls(Box::new(tls_stream)),
            Err(e) => {
                eprintln!("Error: TLS handshake failed: {e}");
                process::exit(1);
            }
        }
    } else {
        Stream::Plain(tcp)
    };

    let mut client = CliClient {
        stream,
        read_buf: BytesMut::with_capacity(8192),
        next_request_id: 1,
    };

    // Perform FIBP handshake
    let handshake = Handshake {
        protocol_version: 1,
        api_key: api_key.clone(),
    };
    let req_id = client.next_id();
    let frame = handshake.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: handshake failed: {e}");
            process::exit(1);
        }
    };

    let opcode = Opcode::from_u8(resp.opcode);
    match opcode {
        Some(Opcode::HandshakeOk) => {
            // Successfully connected
            let _ok = HandshakeOk::decode(resp.payload).unwrap_or_else(|e| {
                eprintln!("Error: cannot decode handshake response: {e}");
                process::exit(1);
            });
        }
        Some(Opcode::Error) => {
            let err = ErrorFrame::decode(resp.payload).unwrap_or_else(|e| {
                eprintln!("Error: cannot decode error frame: {e}");
                process::exit(1);
            });
            eprintln!(
                "Error: handshake rejected: {:?}: {}",
                err.error_code, err.message
            );
            process::exit(1);
        }
        _ => {
            eprintln!(
                "Error: unexpected response opcode 0x{:02X} during handshake",
                resp.opcode
            );
            process::exit(1);
        }
    }

    client
}

// ---------------------------------------------------------------------------
// Error formatting
// ---------------------------------------------------------------------------

fn format_error_code(code: ErrorCode, context: &str) -> String {
    match code {
        ErrorCode::QueueNotFound => format!("Error: {context} does not exist"),
        ErrorCode::QueueAlreadyExists => format!("Error: {context} already exists"),
        ErrorCode::ApiKeyNotFound => format!("Error: {context} does not exist"),
        ErrorCode::Unauthorized => "Error: unauthorized (invalid or missing API key)".to_string(),
        ErrorCode::Forbidden => "Error: forbidden (insufficient permissions)".to_string(),
        ErrorCode::NotLeader => "Error: not the leader node for this operation".to_string(),
        ErrorCode::NodeNotReady => "Error: node not ready, try again".to_string(),
        ErrorCode::ChannelFull => "Error: broker overloaded, try again".to_string(),
        ErrorCode::InvalidConfigValue => format!("Error: invalid config value for {context}"),
        ErrorCode::LuaCompilationError => format!("Error: Lua compilation error for {context}"),
        ErrorCode::NotADLQ => format!("Error: {context} is not a dead-letter queue"),
        ErrorCode::ParentQueueNotFound => {
            format!("Error: parent queue for {context} does not exist")
        }
        ErrorCode::InternalError => "Error: internal server error".to_string(),
        _ => format!("Error: {:?}", code),
    }
}

/// Check if an Error frame was received instead of the expected response.
/// If so, print the error and exit.
fn check_error_frame(resp: &RawFrame, context: &str) {
    if let Some(Opcode::Error) = Opcode::from_u8(resp.opcode) {
        let err = ErrorFrame::decode(resp.payload.clone()).unwrap_or_else(|e| {
            eprintln!("Error: cannot decode error frame: {e}");
            process::exit(1);
        });
        if err.message.is_empty() {
            eprintln!("{}", format_error_code(err.error_code, context));
        } else {
            eprintln!("Error: {}", err.message);
        }
        process::exit(1);
    }
}

/// Check the error_code field in a response and exit on failure.
fn check_response_error(error_code: ErrorCode, context: &str) {
    if error_code != ErrorCode::Ok {
        eprintln!("{}", format_error_code(error_code, context));
        process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

async fn cmd_queue_create(
    client: &mut CliClient,
    name: String,
    on_enqueue: Option<String>,
    on_failure: Option<String>,
    visibility_timeout: Option<u64>,
) {
    let req = fila_fibp::CreateQueueRequest {
        name: name.clone(),
        on_enqueue_script: on_enqueue,
        on_failure_script: on_failure,
        visibility_timeout_ms: visibility_timeout.unwrap_or(0),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("queue \"{name}\""));
    let result = CreateQueueResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("queue \"{name}\""));
    println!("Created queue \"{name}\"");
}

async fn cmd_queue_delete(client: &mut CliClient, name: String) {
    let req = fila_fibp::DeleteQueueRequest {
        queue: name.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("queue \"{name}\""));
    let result = DeleteQueueResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("queue \"{name}\""));
    println!("Deleted queue \"{name}\"");
}

async fn cmd_queue_list(client: &mut CliClient) {
    let req = fila_fibp::ListQueuesRequest;
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, "queues");
    let result = ListQueuesResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, "queues");

    let queues = result.queues;
    if queues.is_empty() {
        println!("No queues found.");
        return;
    }

    let is_cluster = result.cluster_node_count > 0;

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
        println!("\nCluster nodes: {}", result.cluster_node_count);
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

async fn cmd_queue_inspect(client: &mut CliClient, name: String) {
    let req = fila_fibp::GetStatsRequest {
        queue: name.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("queue \"{name}\""));
    let stats = GetStatsResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(stats.error_code, &format!("queue \"{name}\""));

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

async fn cmd_config_set(client: &mut CliClient, key: String, value: String) {
    let req = fila_fibp::SetConfigRequest {
        key: key.clone(),
        value: value.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("config key \"{key}\""));
    let result = SetConfigResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("config key \"{key}\""));
    println!("Set \"{key}\" = \"{value}\"");
}

async fn cmd_config_get(client: &mut CliClient, key: String) {
    let req = fila_fibp::GetConfigRequest { key: key.clone() };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("config key \"{key}\""));
    let result = GetConfigResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("config key \"{key}\""));
    if result.value.is_empty() {
        println!("{key}: (not set)");
    } else {
        println!("{key}: {}", result.value);
    }
}

async fn cmd_config_list(client: &mut CliClient, prefix: String) {
    let req = fila_fibp::ListConfigRequest {
        prefix: prefix.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, "config");
    let result = ListConfigResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, "config");

    let entries = result.entries;
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

async fn cmd_redrive(client: &mut CliClient, dlq_name: String, count: u64) {
    let req = fila_fibp::RedriveRequest {
        dlq_queue: dlq_name.clone(),
        count,
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("queue \"{dlq_name}\""));
    let result = RedriveResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("queue \"{dlq_name}\""));
    let n = result.redriven;
    println!(
        "Redrive complete: {n} message{} moved from \"{dlq_name}\"",
        if n == 1 { "" } else { "s" }
    );
}

async fn cmd_auth_create(
    client: &mut CliClient,
    name: String,
    expires_at: Option<u64>,
    is_superadmin: bool,
) {
    let req = CreateApiKeyRequest {
        name: name.clone(),
        expires_at_ms: expires_at.unwrap_or(0),
        is_superadmin,
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, "api key");
    let r = CreateApiKeyResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(r.error_code, "api key");
    println!("Created API key \"{name}\"");
    println!("  Key ID     : {}", r.key_id);
    println!("  Token      : {}", r.key);
    if r.is_superadmin {
        println!("  Superadmin : yes");
    }
    println!("Store the token securely — it will not be shown again.");
}

async fn cmd_auth_acl_set(client: &mut CliClient, key_id: String, permissions: Vec<String>) {
    let parsed: Vec<AclPermission> = match permissions
        .iter()
        .map(|p| {
            let (kind, pattern) = p.split_once(':').ok_or(p.as_str())?;
            match kind {
                "produce" | "consume" | "admin" => {}
                _ => {
                    eprintln!(
                        "Error: invalid permission kind \"{kind}\" — must be one of: produce, consume, admin"
                    );
                    process::exit(1);
                }
            }
            Ok(AclPermission {
                kind: kind.to_string(),
                pattern: pattern.to_string(),
            })
        })
        .collect::<Result<Vec<_>, &str>>()
    {
        Ok(v) => v,
        Err(bad) => {
            eprintln!(
                "Error: invalid permission format \"{bad}\" — expected <kind>:<pattern> (e.g. produce:orders)"
            );
            process::exit(1);
        }
    };

    let req = fila_fibp::SetAclRequest {
        key_id: key_id.clone(),
        permissions: parsed,
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("api key \"{key_id}\""));
    let result = SetAclResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("api key \"{key_id}\""));
    println!("ACL updated for key {key_id}");
}

async fn cmd_auth_acl_get(client: &mut CliClient, key_id: String) {
    let req = fila_fibp::GetAclRequest {
        key_id: key_id.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("api key \"{key_id}\""));
    let r = GetAclResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(r.error_code, &format!("api key \"{key_id}\""));
    println!("Key ID : {}", r.key_id);
    if r.is_superadmin {
        println!("Superadmin: yes (all permissions granted)");
    } else if r.permissions.is_empty() {
        println!("Permissions: (none)");
    } else {
        println!("Permissions:");
        for p in &r.permissions {
            println!("  {}:{}", p.kind, p.pattern);
        }
    }
}

async fn cmd_auth_revoke(client: &mut CliClient, key_id: String) {
    let req = fila_fibp::RevokeApiKeyRequest {
        key_id: key_id.clone(),
    };
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, &format!("api key \"{key_id}\""));
    let result = RevokeApiKeyResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, &format!("api key \"{key_id}\""));
    println!("Revoked API key {key_id}");
}

async fn cmd_auth_list(client: &mut CliClient) {
    let req = fila_fibp::ListApiKeysRequest;
    let req_id = client.next_id();
    let frame = req.encode(req_id);
    let resp = match client.send_recv(&frame).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    };

    check_error_frame(&resp, "api keys");
    let result = ListApiKeysResponse::decode(resp.payload).unwrap_or_else(|e| {
        eprintln!("Error: cannot decode response: {e}");
        process::exit(1);
    });
    check_response_error(result.error_code, "api keys");

    let keys = result.keys;
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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut client = connect(
        &cli.addr,
        cli.tls,
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
            AuthCommands::Create {
                name,
                expires_at,
                superadmin,
            } => cmd_auth_create(&mut client, name, expires_at, superadmin).await,
            AuthCommands::Revoke { key_id } => cmd_auth_revoke(&mut client, key_id).await,
            AuthCommands::List => cmd_auth_list(&mut client).await,
            AuthCommands::Acl(acl_cmd) => match acl_cmd {
                AclCommands::Set {
                    key_id,
                    permissions,
                } => cmd_auth_acl_set(&mut client, key_id, permissions).await,
                AclCommands::Get { key_id } => cmd_auth_acl_get(&mut client, key_id).await,
            },
        },
    }
}
