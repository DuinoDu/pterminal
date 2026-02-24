use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use pterminal_ipc::IpcClient;

#[derive(Debug, Parser)]
#[command(name = "pterminal-cli", about = "Control pterminal via JSON-RPC IPC")]
struct Cli {
    /// Override socket path (default: ~/.config/pterminal/pterminal.sock)
    #[arg(long)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Ping,
    Capabilities,
    Identify,
    ListWorkspaces,
    NewWorkspace,
    CloseWorkspace {
        #[arg(long)]
        id: Option<u64>,
    },
    SelectWorkspace {
        #[arg(long)]
        id: Option<u64>,
        #[arg(long)]
        index: Option<usize>,
    },
    ListPanes,
    Send {
        text: String,
        #[arg(long)]
        pane_id: Option<u64>,
    },
    ReadScreen {
        #[arg(long)]
        pane_id: Option<u64>,
    },
    CapturePane {
        #[arg(long)]
        pane_id: Option<u64>,
    },
    Notify {
        title: String,
        body: Option<String>,
    },
    ListNotifications,
    ClearNotifications,
    Rpc {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(IpcClient::default_socket_path);
    let client = IpcClient::new(socket);

    let result = match cli.command {
        Command::Ping => client.call("ping", json!({})).await?,
        Command::Capabilities => client.call("capabilities", json!({})).await?,
        Command::Identify => client.call("identify", json!({})).await?,
        Command::ListWorkspaces => client.call("workspace.list", json!({})).await?,
        Command::NewWorkspace => client.call("workspace.new", json!({})).await?,
        Command::CloseWorkspace { id } => {
            client.call("workspace.close", json!({ "id": id })).await?
        }
        Command::SelectWorkspace { id, index } => {
            if id.is_none() && index.is_none() {
                return Err(anyhow!("either --id or --index is required"));
            }
            client
                .call("workspace.select", json!({ "id": id, "index": index }))
                .await?
        }
        Command::ListPanes => client.call("pane.list", json!({})).await?,
        Command::Send { text, pane_id } => {
            client
                .call("terminal.send", json!({ "text": text, "pane_id": pane_id }))
                .await?
        }
        Command::ReadScreen { pane_id } => {
            client
                .call("pane.read_screen", json!({ "pane_id": pane_id }))
                .await?
        }
        Command::CapturePane { pane_id } => {
            client
                .call("pane.capture", json!({ "pane_id": pane_id }))
                .await?
        }
        Command::Notify { title, body } => {
            client
                .call(
                    "notification.send",
                    json!({
                        "title": title,
                        "body": body.unwrap_or_default()
                    }),
                )
                .await?
        }
        Command::ListNotifications => client.call("notification.list", json!({})).await?,
        Command::ClearNotifications => client.call("notification.clear", json!({})).await?,
        Command::Rpc { method, params } => {
            let value: Value = serde_json::from_str(&params)
                .with_context(|| format!("failed to parse --params JSON: {params}"))?;
            client.call(&method, value).await?
        }
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
