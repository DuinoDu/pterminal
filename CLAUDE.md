# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build (optimized with LTO)

# Run main GUI application
cargo run --release

# CLI tool for IPC control
cargo run --release -p pterminal-cli -- <command>
# Examples:
#   ping, capabilities
#   workspace.list, workspace.new, workspace.close, workspace.select
#   pane.list, pane.read_screen, pane.capture
#   terminal.send "command" --pane_id 0
#   notification.send, notification.list, notification.clear

# Benchmarking
cargo run --release -p pterminal-cli -- bench --cols 120 --rows 40 --iterations 200
```

## Architecture

PTerminal is a GPU-accelerated terminal emulator with a modular crate architecture:

```
pterminal/
├── src/main.rs              # Entry point, launches pterminal-ui
├── cli/                     # pterminal-cli: IPC control tool
└── crates/
    ├── pterminal-core/      # Terminal emulation, PTY, config, workspaces
    ├── pterminal-render/    # wgpu GPU rendering (text via glyphon, backgrounds)
    ├── pterminal-ui/        # Slint UI, event handling, app state machine
    └── pterminal-ipc/       # JSON-RPC 2.0 over Unix sockets
```

### Core Components

**pterminal-core**: Wraps `alacritty_terminal` for ANSI parsing. Key modules:
- `terminal/emulator.rs` - Terminal emulator with dedicated parser thread, grid delta extraction
- `terminal/pty.rs` - PTY spawning via `portable-pty`, separate reader/writer threads
- `split/mod.rs` - Binary tree for split panes (horizontal/vertical)
- `workspace/mod.rs` - Multiple workspaces with independent split trees
- `config/` - TOML config from `~/.config/pterminal/`, theme system

**pterminal-render**: GPU pipeline using wgpu:
- `renderer.rs` - wgpu device/queue/surface management, coordinates text+background rendering
- `text.rs` - Per-pane text buffers via glyphon, per-line change detection, cursor/selection rendering
- `bg.rs` - Instanced background rendering (65K cell capacity), uses `bg_instanced.wgsl`

**pterminal-ui**: Application logic:
- `app.rs` - Main state machine: input handling, text selection, IME, context menus, split resizing, IPC server, FPS limiting (8ms ≈ 120fps)
- `slint_app.rs` - Slint integration, macOS titlebar customization, display scale detection
- `ui/app.slint` - Tab bar, sidebar, context menu, split panes

**pterminal-ipc**: JSON-RPC 2.0 protocol over Unix sockets with tokio async runtime

### Threading Model

- **Parser Thread**: Dedicated ANSI parsing for low-latency input
- **PTY Reader/Writer Threads**: Non-blocking shell I/O
- **IPC Server Thread**: Tokio runtime for socket handling
- **Main UI Thread**: Winit event loop

### Rendering Pipeline

1. Terminal emulator processes input → updates grid
2. Delta extraction (only changed rows)
3. Update glyphon text buffers for dirty lines
4. Collect background color rectangles
5. GPU render pass → submit to wgpu
6. Atlas trimming and cleanup

### Key Dependencies

- Terminal: `alacritty_terminal`, `portable-pty`
- GPU: `wgpu`, `glyphon`, `cosmic-text`
- UI: `slint` (with wgpu-28, winit-030 features), `winit`
- Async: `tokio`

## Platform Notes

- Primary target: macOS (CoreGraphics integration, transparent titlebar)
- IPC: Unix sockets only (not Windows)
