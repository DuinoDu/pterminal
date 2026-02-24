use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio::time::timeout;

use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct IpcClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    pub fn new(socket_path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: socket_path.as_ref().to_path_buf(),
            timeout: Duration::from_secs(3),
        }
    }

    pub fn default_socket_path() -> PathBuf {
        pterminal_core::Config::config_dir().join("pterminal.sock")
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        #[cfg(not(unix))]
        {
            let _ = method;
            let _ = params;
            return Err(anyhow!("IPC client is only implemented for unix in this build"));
        }

        #[cfg(unix)]
        {
            let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
            let request = JsonRpcRequest::new(id, method.to_string(), params);

            let mut stream = timeout(self.timeout, UnixStream::connect(&self.socket_path))
                .await
                .context("IPC connect timeout")?
                .with_context(|| format!("failed to connect to socket {}", self.socket_path.display()))?;

            let payload = serde_json::to_vec(&request)?;
            timeout(self.timeout, stream.write_all(&payload))
                .await
                .context("IPC write timeout")??;
            timeout(self.timeout, stream.write_all(b"\n"))
                .await
                .context("IPC write timeout")??;

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            let n = timeout(self.timeout, reader.read_line(&mut line))
                .await
                .context("IPC read timeout")??;
            if n == 0 {
                return Err(anyhow!("IPC connection closed by server"));
            }

            let response: JsonRpcResponse = serde_json::from_str(line.trim())
                .context("failed to parse IPC response")?;
            if let Some(err) = response.error {
                return Err(anyhow!("RPC error {}: {}", err.code, err.message));
            }
            Ok(response.result.unwrap_or(Value::Null))
        }
    }
}
