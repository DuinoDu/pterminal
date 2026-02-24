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
use pterminal_render::grid::grid_to_render_lines;

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

        let mut handler = AppHandler {
            app: self,
        };
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
            .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0));

        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let size = window.inner_size();

        // Create renderer (blocking on async)
        let renderer = pollster::block_on(Renderer::new(
            window.clone(),
            size.width.max(1),
            size.height.max(1),
        ))
        .expect("create renderer");

        // Calculate terminal grid size from window
        let font_size = self.app.config.font.size;
        let cell_width = (font_size * 0.6) as u16;
        let cell_height = (font_size * 1.3) as u16;
        let cols = (size.width as u16 / cell_width).max(1);
        let rows = (size.height as u16 / cell_height).max(1);

        // Create terminal emulator
        let emulator = TerminalEmulator::new(cols, rows);

        // Spawn PTY
        let shell = self.app.config.shell();
        let cwd = self.app.config.working_directory();

        let dirty = Arc::new(AtomicBool::new(true));

        let emulator_handle = emulator.clone_inner();
        let window_for_redraw = window.clone();
        let dirty_for_pty = Arc::clone(&dirty);

        let pty = PtyHandle::spawn(&shell, &cwd, cols, rows, move |data| {
            emulator_handle.process(data);
            dirty_for_pty.store(true, Ordering::Relaxed);
            window_for_redraw.request_redraw();
        })
        .expect("spawn PTY");

        info!(cols, rows, %shell, "Terminal started");

        self.app.state = Some(RunningState {
            window,
            renderer,
            emulator,
            pty,
            dirty,
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

            WindowEvent::Resized(new_size) => {
                state.renderer.resize(new_size.width, new_size.height);

                let font_size = self.app.config.font.size;
                let cell_width = (font_size * 0.6) as u16;
                let cell_height = (font_size * 1.3) as u16;
                let cols = (new_size.width as u16 / cell_width).max(1);
                let rows = (new_size.height as u16 / cell_height).max(1);

                state.emulator.resize(cols, rows);
                let _ = state.pty.resize(cols, rows);
                state.dirty.store(true, Ordering::Relaxed);
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed || event.state == ElementState::Released {
                    // Only handle press (and repeat, which winit sends as Pressed)
                    if event.state == ElementState::Pressed {
                        if let Some(bytes) = key_to_bytes(&event) {
                            let _ = state.pty.write(&bytes);
                            state.window.request_redraw();
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                let theme = &self.app.theme;
                let grid = state.emulator.extract_grid(theme);
                let lines = grid_to_render_lines(&grid);
                state.renderer.text_renderer.set_terminal_content(&lines);
                state.renderer.text_renderer.prepare(
                    &state.renderer.device,
                    &state.renderer.queue,
                    theme.colors.foreground,
                );

                let _ = state.renderer.render_frame(theme.colors.background, |_text| {
                    // Text already prepared above
                });
                state.dirty.store(false, Ordering::Relaxed);
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.app.state {
            // Poll terminal events
            let events = state.emulator.poll_events();
            if !events.is_empty() {
                state.dirty.store(true, Ordering::Relaxed);
            }

            if state.dirty.load(Ordering::Relaxed) {
                state.window.request_redraw();
            }

            // Wake up periodically to check for PTY output (16ms ≈ 60fps)
            event_loop.set_control_flow(
                winit::event_loop::ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(16),
                ),
            );
        }
    }
}

/// Convert winit key events to bytes for PTY input
fn key_to_bytes(event: &winit::event::KeyEvent) -> Option<Vec<u8>> {
    

    // Check if Ctrl is held (for Ctrl+C, Ctrl+D, etc.)
    // winit puts the text with modifiers applied in event.text
    // but Ctrl+key combos need special handling

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
        Key::Character(c) => {
            // Use event.text for the actual input (respects modifiers like Shift)
            if let Some(text) = &event.text {
                let s = text.as_str();
                if !s.is_empty() {
                    return Some(s.as_bytes().to_vec());
                }
            }
            // Fallback: handle Ctrl+key by checking if the character is a-z
            // and the text field is empty (macOS may not provide text for Ctrl combos)
            let ch = c.as_str().chars().next()?;
            if ch.is_ascii_lowercase() {
                // Could be Ctrl+key — but we can't tell without modifier state here.
                // Just pass the character through as-is.
                Some(c.as_str().as_bytes().to_vec())
            } else {
                Some(c.as_str().as_bytes().to_vec())
            }
        }
        _ => None,
    }
}
