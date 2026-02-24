use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::info;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use pterminal_core::config::theme::Theme;
use pterminal_core::terminal::{PtyHandle, TerminalEmulator};
use pterminal_core::Config;
use pterminal_render::Renderer;

/// Main application state
pub struct App {
    config: Config,
    theme: Theme,
    state: Option<RunningState>,
}

struct RunningState {
    window: Arc<Window>,
    renderer: Renderer,
    emulator: TerminalEmulator,
    pty: PtyHandle,
    dirty: Arc<AtomicBool>,
    cursor_visible: bool,
    last_blink: Instant,
    blink_interval: Duration,
    scale_factor: f64,
    last_content: String,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            theme: Theme::default(),
            state: None,
        }
    }

    pub fn run(self) -> Result<()> {
        let event_loop = EventLoop::new()?;

        let mut handler = AppHandler { app: self };
        event_loop.run_app(&mut handler)?;
        Ok(())
    }
}

struct AppHandler {
    app: App,
}

impl ApplicationHandler for AppHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.state.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("pterminal")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 640.0));

        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let scale_factor = window.scale_factor();
        let size = window.inner_size();
        let font_size = self.app.config.font.size;

        // Create renderer with scale factor
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            scale_factor,
            font_size,
        ))
        .expect("create renderer");

        // Calculate grid in physical pixels (scale-aware)
        let (cell_w, cell_h) = renderer.text_renderer.cell_size();
        let padding = (6.0 * scale_factor as f32) as u32;
        let cols = ((size.width - padding * 2) as f32 / cell_w).max(1.0) as u16;
        let rows = ((size.height - padding * 2) as f32 / cell_h).max(1.0) as u16;

        let emulator = TerminalEmulator::new(cols, rows);

        let shell = self.app.config.shell();
        let cwd = self.app.config.working_directory();

        let dirty = Arc::new(AtomicBool::new(true));

        let emulator_handle = emulator.clone_inner();
        let window_for_redraw = window.clone();
        let dirty_for_pty = Arc::clone(&dirty);

        let pty = PtyHandle::spawn(&shell, &cwd, cols, rows, move |data| {
            emulator_handle.process(data);
            dirty_for_pty.store(true, Ordering::Release);
            window_for_redraw.request_redraw();
        })
        .expect("spawn PTY");

        let blink_ms = self.app.config.cursor.blink_interval_ms;
        info!(cols, rows, %shell, scale_factor, "Terminal started");

        self.app.state = Some(RunningState {
            window,
            renderer,
            emulator,
            pty,
            dirty,
            cursor_visible: true,
            last_blink: Instant::now(),
            blink_interval: Duration::from_millis(blink_ms),
            scale_factor,
            last_content: String::new(),
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.app.state else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.scale_factor = scale_factor;
                state.renderer.text_renderer.update_scale_factor(
                    scale_factor,
                    self.app.config.font.size,
                );
                state.dirty.store(true, Ordering::Relaxed);
            }

            WindowEvent::Resized(new_size) => {
                state.renderer.resize(new_size.width, new_size.height);

                let (cell_w, cell_h) = state.renderer.text_renderer.cell_size();
                let padding = (6.0 * state.scale_factor as f32) as u32;
                let cols = ((new_size.width - padding * 2) as f32 / cell_w).max(1.0) as u16;
                let rows = ((new_size.height - padding * 2) as f32 / cell_h).max(1.0) as u16;

                state.emulator.resize(cols, rows);
                let _ = state.pty.resize(cols, rows);
                state.dirty.store(true, Ordering::Relaxed);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Some(bytes) = key_to_bytes(&event) {
                        let _ = state.pty.write(&bytes);
                        // Reset cursor blink on input
                        state.cursor_visible = true;
                        state.last_blink = Instant::now();
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let theme = &self.app.theme;

                // Build text content with cursor
                let grid = state.emulator.extract_grid(theme);
                let cursor_pos = state.emulator.cursor_position();
                let content = build_display_text(&grid, cursor_pos, state.cursor_visible);

                // Only re-submit text to glyphon if content changed
                if content != state.last_content {
                    state.renderer.text_renderer.set_terminal_content(&content);
                    state.last_content = content;
                }

                state.renderer.text_renderer.prepare(
                    &state.renderer.device,
                    &state.renderer.queue,
                    theme.colors.foreground,
                );

                let _ = state
                    .renderer
                    .render_frame(theme.colors.background, |_| {});
                state.dirty.store(false, Ordering::Relaxed);
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.app.state {
            let _ = state.emulator.poll_events();

            // Cursor blink
            let now = Instant::now();
            if self.app.config.cursor.blink && now.duration_since(state.last_blink) >= state.blink_interval {
                state.cursor_visible = !state.cursor_visible;
                state.last_blink = now;
                state.dirty.store(true, Ordering::Relaxed);
            }

            if state.dirty.load(Ordering::Acquire) {
                state.window.request_redraw();
            }

            // Next wake: at blink time or 100ms (idle), whichever is sooner
            let next_blink = state.last_blink + state.blink_interval;
            let next_wake = next_blink.min(now + Duration::from_millis(100));
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_wake));
        }
    }
}

/// Build display text from grid, inserting a block cursor character
fn build_display_text(
    grid: &[pterminal_core::terminal::GridLine],
    cursor: (u16, u16),  // (col, row)
    cursor_visible: bool,
) -> String {
    let mut text = String::with_capacity(grid.len() * 80);
    let (cursor_col, cursor_row) = cursor;

    for (row_idx, line) in grid.iter().enumerate() {
        if row_idx > 0 {
            text.push('\n');
        }
        for (col_idx, cell) in line.cells.iter().enumerate() {
            if cursor_visible
                && row_idx == cursor_row as usize
                && col_idx == cursor_col as usize
            {
                // Render cursor as block character (▊)
                text.push('█');
            } else {
                let c = cell.c;
                text.push(if c == '\0' || c == ' ' { ' ' } else { c });
            }
        }
        // Trim trailing spaces for cleaner rendering
        let trimmed_len = text.trim_end_matches(' ').len();
        // But keep at least up to the cursor column on cursor row
        if row_idx == cursor_row as usize && cursor_visible {
            let min_len = text.len() - line.cells.len() + cursor_col as usize + 1;
            text.truncate(trimmed_len.max(min_len));
        } else {
            text.truncate(trimmed_len);
        }
    }
    text
}

/// Convert winit key events to bytes for PTY input
fn key_to_bytes(event: &winit::event::KeyEvent) -> Option<Vec<u8>> {
    match &event.logical_key {
        Key::Named(named) => {
            let bytes: &[u8] = match named {
                NamedKey::Enter => b"\r",
                NamedKey::Backspace => b"\x7f",
                NamedKey::Tab => b"\t",
                NamedKey::Escape => b"\x1b",
                NamedKey::ArrowUp => b"\x1b[A",
                NamedKey::ArrowDown => b"\x1b[B",
                NamedKey::ArrowRight => b"\x1b[C",
                NamedKey::ArrowLeft => b"\x1b[D",
                NamedKey::Home => b"\x1b[H",
                NamedKey::End => b"\x1b[F",
                NamedKey::PageUp => b"\x1b[5~",
                NamedKey::PageDown => b"\x1b[6~",
                NamedKey::Delete => b"\x1b[3~",
                NamedKey::Insert => b"\x1b[2~",
                NamedKey::Space => b" ",
                _ => return None,
            };
            Some(bytes.to_vec())
        }
        Key::Character(_) => {
            // Use event.text for the actual input (respects Shift, Ctrl, etc.)
            if let Some(text) = &event.text {
                let s = text.as_str();
                if !s.is_empty() {
                    return Some(s.as_bytes().to_vec());
                }
            }
            None
        }
        _ => None,
    }
}
