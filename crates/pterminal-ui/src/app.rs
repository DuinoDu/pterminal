use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

use anyhow::Result;
use arboard::Clipboard;
use tracing::info;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use pterminal_core::config::theme::Theme;
use pterminal_core::terminal::{PtyHandle, TerminalEmulator};
use pterminal_core::Config;
use pterminal_render::Renderer;

/// Text selection range in grid coordinates
#[derive(Clone, Copy, PartialEq)]
struct Selection {
    start: (u16, u16), // (col, row)
    end: (u16, u16),
}

impl Selection {
    /// Normalize so start <= end (row-major order)
    fn normalized(&self) -> ((u16, u16), (u16, u16)) {
        if self.start.1 < self.end.1
            || (self.start.1 == self.end.1 && self.start.0 <= self.end.0)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

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
    modifiers: ModifiersState,
    clipboard: Option<Clipboard>,
    // Mouse selection
    selection: Option<Selection>,
    mouse_pressed: bool,
    last_mouse_pos: (f64, f64), // logical pixels
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

impl AppHandler {
    /// Convert logical pixel position to grid cell (col, row)
    fn pixel_to_cell(state: &RunningState) -> (u16, u16) {
        let (cell_w, cell_h) = state.renderer.text_renderer.cell_size();
        let scale = state.scale_factor as f32;
        let padding = 6.0 * scale;
        let px = state.last_mouse_pos.0 as f32 * scale - padding;
        let py = state.last_mouse_pos.1 as f32 * scale - padding;
        let col = (px / cell_w).max(0.0) as u16;
        let row = (py / cell_h).max(0.0) as u16;
        let (_, grid_rows) = state.emulator.size();
        let (grid_cols, _) = state.emulator.size();
        (col.min(grid_cols.saturating_sub(1)), row.min(grid_rows.saturating_sub(1)))
    }

    /// Extract selected text from the grid
    fn get_selected_text(state: &RunningState, theme: &Theme) -> Option<String> {
        let sel = state.selection?;
        let (start, end) = sel.normalized();
        let grid = state.emulator.extract_grid(theme);

        let mut text = String::new();
        for row in start.1..=end.1 {
            if row as usize >= grid.len() {
                break;
            }
            let line = &grid[row as usize];
            let col_start = if row == start.1 { start.0 as usize } else { 0 };
            let col_end = if row == end.1 {
                (end.0 as usize + 1).min(line.cells.len())
            } else {
                line.cells.len()
            };
            for col in col_start..col_end {
                let c = line.cells[col].c;
                text.push(if c == '\0' { ' ' } else { c });
            }
            // Trim trailing spaces on each line and add newline between rows
            let trimmed = text.trim_end_matches(' ').len();
            text.truncate(trimmed);
            if row < end.1 {
                text.push('\n');
            }
        }
        if text.is_empty() { None } else { Some(text) }
    }
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

        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
            scale_factor,
            font_size,
        ))
        .expect("create renderer");

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
        let clipboard = Clipboard::new().ok();
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
            modifiers: ModifiersState::empty(),
            clipboard,
            selection: None,
            mouse_pressed: false,
            last_mouse_pos: (0.0, 0.0),
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

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods.state();
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

            // Mouse events for selection
            WindowEvent::MouseInput { state: btn_state, button: MouseButton::Left, .. } => {
                match btn_state {
                    ElementState::Pressed => {
                        state.mouse_pressed = true;
                        let cell = Self::pixel_to_cell(state);
                        state.selection = Some(Selection { start: cell, end: cell });
                        state.dirty.store(true, Ordering::Relaxed);
                    }
                    ElementState::Released => {
                        state.mouse_pressed = false;
                        // If start == end, clear selection (it was just a click)
                        if let Some(sel) = &state.selection {
                            if sel.start == sel.end {
                                state.selection = None;
                                state.dirty.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                state.last_mouse_pos = (position.x, position.y);
                if state.mouse_pressed {
                    let cell = Self::pixel_to_cell(state);
                    if let Some(sel) = &mut state.selection {
                        if sel.end != cell {
                            sel.end = cell;
                            state.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let super_key = state.modifiers.super_key();

                // Cmd+C: Copy selection
                if super_key {
                    if let Key::Character(ref c) = event.logical_key {
                        if c.as_str() == "c" {
                            if let Some(text) =
                                Self::get_selected_text(state, &self.app.theme)
                            {
                                if let Some(clip) = &mut state.clipboard {
                                    let _ = clip.set_text(text);
                                }
                            }
                            return;
                        }
                        // Cmd+V: Paste
                        if c.as_str() == "v" {
                            if let Some(clip) = &mut state.clipboard {
                                if let Ok(text) = clip.get_text() {
                                    let _ = state.pty.write(text.as_bytes());
                                }
                            }
                            return;
                        }
                    }
                }

                // Clear selection on any other key press
                if state.selection.is_some() {
                    state.selection = None;
                    state.dirty.store(true, Ordering::Relaxed);
                }

                if let Some(bytes) = key_to_bytes(&event) {
                    let _ = state.pty.write(&bytes);
                    state.cursor_visible = true;
                    state.last_blink = Instant::now();
                }
            }

            WindowEvent::RedrawRequested => {
                let theme = &self.app.theme;

                let grid = state.emulator.extract_grid(theme);
                let cursor_pos = state.emulator.cursor_position();
                let cursor_color = theme.colors.cursor;

                state.renderer.text_renderer.set_terminal_content(
                    &grid,
                    cursor_pos,
                    state.cursor_visible,
                    cursor_color,
                );

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
            if self.app.config.cursor.blink
                && now.duration_since(state.last_blink) >= state.blink_interval
            {
                state.cursor_visible = !state.cursor_visible;
                state.last_blink = now;
                state.dirty.store(true, Ordering::Relaxed);
            }

            if state.dirty.load(Ordering::Acquire) {
                state.window.request_redraw();
            }

            let next_blink = state.last_blink + state.blink_interval;
            let next_wake = next_blink.min(now + Duration::from_millis(100));
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next_wake));
        }
    }
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
