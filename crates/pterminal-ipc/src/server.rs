use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tracing::{error, warn};

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

pub type RpcHandler = Arc<dyn Fn(JsonRpcRequest) -> JsonRpcResponse + Send + Sync>;

pub struct IpcServer {
    socket_path: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl IpcServer {
    pub fn start(socket_path: impl AsRef<Path>, handler: RpcHandler) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        #[cfg(unix)]
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        #[cfg(not(unix))]
        {
            let _ = handler;
            anyhow::bail!("IPC server is only implemented for unix in this build");
        }

        #[cfg(unix)]
        {
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            let path_for_thread = socket_path.clone();
            let thread = std::thread::Builder::new()
                .name("pterminal-ipc-server".to_string())
                .spawn(move || {
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .enable_time()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            error!("failed to build tokio runtime for IPC: {e}");
                            return;
                        }
                    };
                    rt.block_on(async move {
                        let listener = match UnixListener::bind(&path_for_thread) {
                            Ok(listener) => listener,
                            Err(e) => {
                                error!("failed to bind IPC socket {}: {e}", path_for_thread.display());
                                return;
                            }
                        };
                        run_accept_loop(listener, handler, shutdown_rx).await;
                    });
                })?;

            Ok(Self {
                socket_path,
                shutdown_tx: Some(shutdown_tx),
                thread: Some(thread),
            })
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

#[cfg(unix)]
async fn run_accept_loop(listener: UnixListener, handler: RpcHandler, mut shutdown_rx: oneshot::Receiver<()>) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            handle_client(stream, handler).await;
                        });
                    }
                    Err(e) => {
                        warn!("ipc accept failed: {e}");
                    }
                }
            }
        }
    }
}

#[cfg(unix)]
async fn handle_client(stream: UnixStream, handler: RpcHandler) {
    let (reader_half, mut writer_half) = stream.into_split();
    let mut reader = BufReader::new(reader_half);
    let mut line = String::new();

    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                warn!("ipc read failed: {e}");
                break;
            }
        };
        if n == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => {
                if req.jsonrpc != "2.0" {
                    JsonRpcResponse::invalid_request(req.id)
                } else {
                    (handler)(req)
                }
            }
            Err(_) => JsonRpcResponse::parse_error(),
        };

        let payload = match serde_json::to_vec(&response) {
            Ok(data) => data,
            Err(e) => {
                warn!("ipc serialize response failed: {e}");
                break;
            }
        };
        if writer_half.write_all(&payload).await.is_err() || writer_half.write_all(b"\n").await.is_err() {
            break;
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        #[cfg(unix)]
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
    }
}
