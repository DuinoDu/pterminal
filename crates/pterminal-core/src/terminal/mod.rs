mod pty;
pub mod emulator;

pub use pty::PtyHandle;
pub use emulator::{TerminalEmulator, TerminalEmulatorHandle, GridLine, GridCell};
