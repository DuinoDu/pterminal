use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use tracing::{debug, error};

/// Handle to a running PTY process
pub struct PtyHandle {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PtyHandle {
    /// Spawn a new shell in a PTY
    pub fn spawn(
        shell: &str,
        working_dir: &std::path::Path,
        cols: u16,
        rows: u16,
        on_output: impl Fn(&[u8]) + Send + 'static,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();

        let pair: PtyPair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(working_dir);
        // Inherit environment
        for (key, value) in std::env::vars() {
            cmd.env(key, value);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd)?;
        debug!(shell = shell, "PTY process spawned");

        // Drop slave — we only need the master side
        drop(pair.slave);

        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

        // Spawn reader thread
        let mut reader = pair.master.try_clone_reader()?;
        let reader_thread = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => on_output(&buf[..n]),
                        Err(e) => {
                            error!("PTY read error: {}", e);
                            break;
                        }
                    }
                }
            })?;

        Ok(Self {
            writer,
            master: pair.master,
            reader_thread: Some(reader_thread),
            _child: child,
        })
    }

    /// Write bytes to the PTY (keyboard input)
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Resize the PTY
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}
