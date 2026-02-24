use std::sync::Arc;

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
    needs_redraw: bool,
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
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

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

        let emulator_handle = emulator.clone_inner();
        let window_for_redraw = window.clone();

        let pty = PtyHandle::spawn(&shell, &cwd, cols, rows, move |data| {
            emulator_handle.process(data);
            window_for_redraw.request_redraw();
        })
        .expect("spawn PTY");

        info!(cols, rows, %shell, "Terminal started");

        self.app.state = Some(RunningState {
            window,
            renderer,
            emulator,
            pty,
            needs_redraw: true,
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
                state.needs_redraw = true;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    if let Some(text) = key_to_bytes(&event) {
                        let _ = state.pty.write(text.as_bytes());
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
                state.needs_redraw = false;
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.app.state {
            // Poll terminal events
            let events = state.emulator.poll_events();
            if !events.is_empty() || state.needs_redraw {
                state.window.request_redraw();
            }
        }
    }
}

/// Convert winit key events to bytes for PTY input
fn key_to_bytes(event: &winit::event::KeyEvent) -> Option<String> {
    match &event.logical_key {
        Key::Character(c) => Some(c.to_string()),
        Key::Named(named) => match named {
            NamedKey::Enter => Some("\r".to_string()),
            NamedKey::Backspace => Some("\x7f".to_string()),
            NamedKey::Tab => Some("\t".to_string()),
            NamedKey::Escape => Some("\x1b".to_string()),
            NamedKey::ArrowUp => Some("\x1b[A".to_string()),
            NamedKey::ArrowDown => Some("\x1b[B".to_string()),
            NamedKey::ArrowRight => Some("\x1b[C".to_string()),
            NamedKey::ArrowLeft => Some("\x1b[D".to_string()),
            NamedKey::Home => Some("\x1b[H".to_string()),
            NamedKey::End => Some("\x1b[F".to_string()),
            NamedKey::PageUp => Some("\x1b[5~".to_string()),
            NamedKey::PageDown => Some("\x1b[6~".to_string()),
            NamedKey::Delete => Some("\x1b[3~".to_string()),
            NamedKey::Insert => Some("\x1b[2~".to_string()),
            NamedKey::Space => Some(" ".to_string()),
            _ => None,
        },
        _ => None,
    }
}
