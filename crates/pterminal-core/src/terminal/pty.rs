use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use anyhow::Result;
use portable_pty::{CommandBuilder, NativePtySystem, PtyPair, PtySize, PtySystem};
use tracing::{debug, error};

use crate::terminal::emulator::TerminalEmulatorHandle;
use crate::terminal::spsc;

const INPUT_QUEUE_DEPTH: usize = 1024;
const WRITER_IDLE_PARK_MS: u64 = 5;

/// Handle to a running PTY process
pub struct PtyHandle {
    input_tx: Option<spsc::Producer<Vec<u8>>>,
    writer_waker: std::thread::Thread,
    master: Box<dyn portable_pty::MasterPty + Send>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    writer_thread: Option<std::thread::JoinHandle<()>>,
    _child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Set to true when the reader thread exits (shell process ended)
    exited: Arc<AtomicBool>,
}

impl PtyHandle {
    /// Spawn a new shell in a PTY
    pub fn spawn(
        shell: &str,
        working_dir: &std::path::Path,
        cols: u16,
        rows: u16,
        emulator: TerminalEmulatorHandle,
        on_output_ready: impl Fn() + Send + 'static,
        on_exit: impl Fn() + Send + 'static,
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

        let mut writer = pair.master.take_writer()?;
        let (input_tx, input_rx) = spsc::channel::<Vec<u8>>(INPUT_QUEUE_DEPTH);
        let exited = Arc::new(AtomicBool::new(false));
        let exited_clone = exited.clone();

        // Spawn dedicated writer thread so UI/input handling never blocks on PTY writes.
        let writer_thread = std::thread::Builder::new()
            .name("pty-writer".into())
            .spawn(move || loop {
                let mut did_work = false;
                while let Some(chunk) = input_rx.try_pop() {
                    if chunk.is_empty() {
                        continue;
                    }
                    if let Err(e) = writer.write_all(&chunk) {
                        error!("PTY write error: {}", e);
                        return;
                    }
                    did_work = true;
                }

                if !did_work {
                    if input_rx.is_producer_closed() {
                        return;
                    }
                    std::thread::park_timeout(Duration::from_millis(WRITER_IDLE_PARK_MS));
                }
            })?;
        let writer_waker = writer_thread.thread().clone();

        // Spawn reader thread with 1MB buffer for high throughput
        let mut reader = pair.master.try_clone_reader()?;
        let reader_thread = std::thread::Builder::new()
            .name("pty-reader".into())
            .spawn(move || {
                // 1MB heap-allocated buffer for better I/O throughput (vs 8KB stack)
                let mut buf = vec![0u8; 1024 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            emulator.process(&buf[..n]);
                            on_output_ready();
                        }
                        Err(e) => {
                            error!("PTY read error: {}", e);
                            break;
                        }
                    }
                }
                exited_clone.store(true, Ordering::Release);
                on_exit();
            })?;

        Ok(Self {
            input_tx: Some(input_tx),
            writer_waker,
            master: pair.master,
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
            _child: child,
            exited,
        })
    }

    /// Queue bytes for PTY input without blocking on the PTY itself.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        self.input_tx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("PTY input queue disconnected"))?
            .push_blocking(data.to_vec())
            .map_err(|_| anyhow::anyhow!("PTY input queue disconnected"))?;
        self.writer_waker.unpark();
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

    /// Check if the shell process has exited
    pub fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        let _ = self.input_tx.take();
        // Wake parked worker so it can observe queue closure and exit.
        self.writer_waker.unpark();
        // Don't join the reader thread — it may be blocked on read().
        // Just detach it; it will exit when the PTY master fd is closed.
        let _ = self.reader_thread.take();
        // Writer thread will exit once senders are dropped as part of teardown.
        let _ = self.writer_thread.take();
    }
}
