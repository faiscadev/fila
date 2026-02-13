use std::process;

use clap::{Parser, Subcommand};
use fila_proto::fila_admin_client::FilaAdminClient;
use fila_proto::{
    CreateQueueRequest, DeleteQueueRequest, GetConfigRequest, GetStatsRequest, ListConfigRequest,
    ListQueuesRequest, QueueConfig, RedriveRequest, SetConfigRequest,
};
use tonic::transport::Channel;

#[derive(Parser)]
#[command(name = "fila", about = "Fila message broker CLI")]
struct Cli {
    /// Broker address
    #[arg(long, default_value = "http://localhost:5555", global = true)]
    addr: String,

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

async fn connect(addr: &str) -> FilaAdminClient<Channel> {
    match FilaAdminClient::connect(addr.to_string()).await {
        Ok(client) => client,
        Err(_) => {
            eprintln!("Error: cannot connect to broker at {addr}");
            process::exit(1);
        }
    }
}

fn format_rpc_error(status: tonic::Status, context: &str) -> String {
    match status.code() {
        tonic::Code::NotFound => format!("Error: {context} does not exist"),
        tonic::Code::AlreadyExists => format!("Error: {context} already exists"),
        tonic::Code::InvalidArgument => format!("Error: {}", status.message()),
        tonic::Code::FailedPrecondition => format!("Error: {}", status.message()),
        tonic::Code::ResourceExhausted => "Error: broker overloaded, try again".to_string(),
        tonic::Code::Unavailable => "Error: cannot connect to broker".to_string(),
        _ => format!("Error: {}", status.message()),
    }
}

async fn cmd_queue_create(
    client: &mut FilaAdminClient<Channel>,
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

async fn cmd_queue_delete(client: &mut FilaAdminClient<Channel>, name: String) {
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

async fn cmd_queue_list(client: &mut FilaAdminClient<Channel>) {
    match client.list_queues(ListQueuesRequest {}).await {
        Ok(resp) => {
            let queues = resp.into_inner().queues;
            if queues.is_empty() {
                println!("No queues found.");
                return;
            }

            // Calculate column widths
            let name_width = queues
                .iter()
                .map(|q| q.name.len())
                .max()
                .unwrap_or(4)
                .max(4);

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
        Err(status) => {
            eprintln!("{}", format_rpc_error(status, "queues"));
            process::exit(1);
        }
    }
}

async fn cmd_queue_inspect(client: &mut FilaAdminClient<Channel>, name: String) {
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

async fn cmd_config_set(client: &mut FilaAdminClient<Channel>, key: String, value: String) {
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

async fn cmd_config_get(client: &mut FilaAdminClient<Channel>, key: String) {
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

async fn cmd_config_list(client: &mut FilaAdminClient<Channel>, prefix: String) {
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

async fn cmd_redrive(client: &mut FilaAdminClient<Channel>, dlq_name: String, count: u64) {
    // Derive parent name from DLQ name
    let parent_name = dlq_name
        .strip_suffix(".dlq")
        .unwrap_or(&dlq_name)
        .to_string();

    match client
        .redrive(RedriveRequest {
            dlq_queue: dlq_name.clone(),
            count,
        })
        .await
    {
        Ok(resp) => {
            let redriven = resp.into_inner().redriven;
            println!(
                "Redrived {redriven} message{} from \"{dlq_name}\" to \"{parent_name}\"",
                if redriven == 1 { "" } else { "s" }
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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut client = connect(&cli.addr).await;

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
    }
}
