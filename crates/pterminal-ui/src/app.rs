use std::collections::HashMap;
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
use pterminal_core::split::{PaneId, SplitDirection};
use pterminal_core::terminal::{PtyHandle, TerminalEmulator};
use pterminal_core::workspace::WorkspaceManager;
use pterminal_core::Config;
use pterminal_render::Renderer;
use pterminal_render::text::PixelRect;

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

/// Per-pane terminal state
struct PaneState {
    emulator: TerminalEmulator,
    pty: PtyHandle,
    dirty: Arc<AtomicBool>,
    /// Last cursor visible state used in rendering (for blink-only updates)
    last_cursor_visible: bool,
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
    workspace_mgr: WorkspaceManager,
    pane_states: HashMap<PaneId, PaneState>,
    scale_factor: f64,
    modifiers: ModifiersState,
    clipboard: Option<Clipboard>,
    // Mouse selection
    selection: Option<Selection>,
    mouse_pressed: bool,
    last_mouse_pos: (f64, f64), // logical pixels
    // Performance monitoring
    frame_count: u64,
    fps_timer: Instant,
    debug_timing: bool,
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
    /// Convert logical pixel position to grid cell (col, row) for the active pane
    fn pixel_to_cell(state: &RunningState) -> (u16, u16) {
        let (cell_w, cell_h) = state.renderer.text_renderer.cell_size();
        let scale = state.scale_factor as f32;
        let padding = 6.0 * scale;
        let px = state.last_mouse_pos.0 as f32 * scale - padding;
        let py = state.last_mouse_pos.1 as f32 * scale - padding;
        let col = (px / cell_w).max(0.0) as u16;
        let row = (py / cell_h).max(0.0) as u16;

        let active_pane = state.workspace_mgr.active_workspace().active_pane();
        if let Some(ps) = state.pane_states.get(&active_pane) {
            let (grid_cols, grid_rows) = ps.emulator.size();
            (col.min(grid_cols.saturating_sub(1)), row.min(grid_rows.saturating_sub(1)))
        } else {
            (col, row)
        }
    }

    /// Extract selected text from the active pane's grid
    fn get_selected_text(state: &RunningState, theme: &Theme) -> Option<String> {
        let sel = state.selection?;
        let (start, end) = sel.normalized();

        let active_pane = state.workspace_mgr.active_workspace().active_pane();
        let ps = state.pane_states.get(&active_pane)?;
        let grid = ps.emulator.extract_grid(theme);

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
            let trimmed = text.trim_end_matches(' ').len();
            text.truncate(trimmed);
            if row < end.1 {
                text.push('\n');
            }
        }
        if text.is_empty() { None } else { Some(text) }
    }

    /// Spawn a new terminal pane and store its state
    fn spawn_pane(
        config: &Config,
        pane_id: PaneId,
        cols: u16,
        rows: u16,
        window: &Arc<Window>,
    ) -> PaneState {
        let shell = config.shell();
        let cwd = config.working_directory();
        let dirty = Arc::new(AtomicBool::new(true));

        let emulator = TerminalEmulator::new(cols, rows);
        let emulator_handle = emulator.clone_inner();
        let window_for_redraw = window.clone();
        let dirty_for_pty = Arc::clone(&dirty);

        let pty = PtyHandle::spawn(&shell, &cwd, cols, rows, move |data| {
            emulator_handle.process(data);
            dirty_for_pty.store(true, Ordering::Release);
            window_for_redraw.request_redraw();
        })
        .expect("spawn PTY");

        info!(pane_id, cols, rows, %shell, "Pane spawned");

        PaneState { emulator, pty, dirty, last_cursor_visible: true }
    }

    /// Calculate cols/rows from a physical-pixel pane rect
    fn rect_to_cols_rows(renderer: &Renderer, scale_factor: f64) -> (u16, u16) {
        let (cell_w, cell_h) = renderer.text_renderer.cell_size();
        let padding = (6.0 * scale_factor as f32) as u32;
        let w = renderer.width();
        let h = renderer.height();
        let cols = ((w - padding * 2) as f32 / cell_w).max(1.0) as u16;
        let rows = ((h - padding * 2) as f32 / cell_h).max(1.0) as u16;
        (cols, rows)
    }

    /// Calculate cols/rows for a specific pane pixel rect
    fn pixel_rect_to_cols_rows(rect: &PixelRect, renderer: &Renderer) -> (u16, u16) {
        let (cell_w, cell_h) = renderer.text_renderer.cell_size();
        let scale = renderer.text_renderer.scale_factor();
        let inner_padding = 6.0 * scale;
        let cols = ((rect.w - inner_padding * 2.0) / cell_w).max(1.0) as u16;
        let rows = ((rect.h - inner_padding * 2.0) / cell_h).max(1.0) as u16;
        (cols, rows)
    }

    /// Build PixelRect from normalized PaneRect
    fn pane_to_pixel_rect(
        pane_rect: &pterminal_core::split::PaneRect,
        window_w: u32,
        window_h: u32,
        scale: f32,
    ) -> PixelRect {
        let content_w = window_w as f32;
        let content_h = window_h as f32;
        let padding = 6.0 * scale;
        PixelRect {
            x: pane_rect.x * content_w + padding,
            y: pane_rect.y * content_h + padding,
            w: pane_rect.width * content_w - padding * 2.0,
            h: pane_rect.height * content_h - padding * 2.0,
        }
    }

    fn update_title(state: &RunningState) {
        let idx = state.workspace_mgr.active_index() + 1;
        let count = state.workspace_mgr.workspace_count();
        let pane_count = state.workspace_mgr.active_workspace().pane_ids().len();
        if pane_count > 1 {
            state.window.set_title(&format!("pterminal [tab {idx}/{count}, {pane_count} panes]"));
        } else {
            state.window.set_title(&format!("pterminal [tab {idx}/{count}]"));
        }
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

        let (cols, rows) = Self::rect_to_cols_rows(&renderer, scale_factor);

        // WorkspaceManager starts with workspace 0, pane 0
        let workspace_mgr = WorkspaceManager::new();
        let initial_pane_id: PaneId = 0;

        let ps = Self::spawn_pane(&self.app.config, initial_pane_id, cols, rows, &window);
        let mut pane_states = HashMap::new();
        pane_states.insert(initial_pane_id, ps);

        let clipboard = Clipboard::new().ok();
        let debug_timing = std::env::var("PTERMINAL_DEBUG").is_ok();
        info!(cols, rows, scale_factor, "Terminal started");

        let running = RunningState {
            window,
            renderer,
            workspace_mgr,
            pane_states,
            scale_factor,
            modifiers: ModifiersState::empty(),
            clipboard,
            selection: None,
            mouse_pressed: false,
            last_mouse_pos: (0.0, 0.0),
            frame_count: 0,
            fps_timer: Instant::now(),
            debug_timing,
        };

        Self::update_title(&running);
        self.app.state = Some(running);
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
                // Mark all panes dirty
                for ps in state.pane_states.values() {
                    ps.dirty.store(true, Ordering::Relaxed);
                }
            }

            WindowEvent::Resized(new_size) => {
                state.renderer.resize(new_size.width, new_size.height);

                let scale = state.scale_factor as f32;
                let w = new_size.width;
                let h = new_size.height;

                // Resize all panes in the active workspace based on their layout rects
                let layout = state.workspace_mgr.active_workspace().split_tree.layout();
                for (pane_id, pane_rect) in &layout {
                    let px_rect = Self::pane_to_pixel_rect(pane_rect, w, h, scale);
                    let (cols, rows) = Self::pixel_rect_to_cols_rows(&px_rect, &state.renderer);
                    if let Some(ps) = state.pane_states.get(pane_id) {
                        ps.emulator.resize(cols, rows);
                        let _ = ps.pty.resize(cols, rows);
                        ps.dirty.store(true, Ordering::Relaxed);
                    }
                }
            }

            // Mouse events for selection
            WindowEvent::MouseInput { state: btn_state, button: MouseButton::Left, .. } => {
                match btn_state {
                    ElementState::Pressed => {
                        state.mouse_pressed = true;
                        let cell = Self::pixel_to_cell(state);
                        state.selection = Some(Selection { start: cell, end: cell });
                        // Mark active pane dirty
                        let active = state.workspace_mgr.active_workspace().active_pane();
                        if let Some(ps) = state.pane_states.get(&active) {
                            ps.dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    ElementState::Released => {
                        state.mouse_pressed = false;
                        if let Some(sel) = &state.selection {
                            if sel.start == sel.end {
                                state.selection = None;
                                let active = state.workspace_mgr.active_workspace().active_pane();
                                if let Some(ps) = state.pane_states.get(&active) {
                                    ps.dirty.store(true, Ordering::Relaxed);
                                }
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
                            let active = state.workspace_mgr.active_workspace().active_pane();
                            if let Some(ps) = state.pane_states.get(&active) {
                                ps.dirty.store(true, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let super_key = state.modifiers.super_key();
                let shift = state.modifiers.shift_key();

                if super_key {
                    if let Key::Character(ref c) = event.logical_key {
                        match c.as_str() {
                            // Cmd+C: Copy selection
                            "c" => {
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
                            "v" => {
                                if let Some(clip) = &mut state.clipboard {
                                    if let Ok(text) = clip.get_text() {
                                        let active = state.workspace_mgr.active_workspace().active_pane();
                                        if let Some(ps) = state.pane_states.get(&active) {
                                            let _ = ps.pty.write(text.as_bytes());
                                        }
                                    }
                                }
                                return;
                            }
                            // Cmd+T: New workspace (tab)
                            "t" => {
                                let (_ws_id, pane_id) = state.workspace_mgr.add_workspace();
                                let (cols, rows) = Self::rect_to_cols_rows(&state.renderer, state.scale_factor);
                                let ps = Self::spawn_pane(&self.app.config, pane_id, cols, rows, &state.window);
                                state.pane_states.insert(pane_id, ps);
                                Self::update_title(state);
                                state.window.request_redraw();
                                return;
                            }
                            // Cmd+W: Close current workspace
                            "w" => {
                                if state.workspace_mgr.workspace_count() > 1 {
                                    let ws = state.workspace_mgr.active_workspace();
                                    let pane_ids = ws.pane_ids();
                                    let ws_id = ws.id;
                                    // Clean up all panes in this workspace
                                    for pid in &pane_ids {
                                        state.pane_states.remove(pid);
                                        state.renderer.text_renderer.remove_pane(*pid);
                                    }
                                    state.workspace_mgr.close_workspace(ws_id);
                                    Self::update_title(state);
                                    state.window.request_redraw();
                                }
                                return;
                            }
                            // Cmd+D: Split horizontally (Cmd+Shift+D: split vertically)
                            "d" | "D" => {
                                let direction = if shift {
                                    SplitDirection::Vertical
                                } else {
                                    SplitDirection::Horizontal
                                };
                                let active_pane = state.workspace_mgr.active_workspace().active_pane();
                                let new_pane_id = state.workspace_mgr.next_pane_id();
                                state.workspace_mgr.active_workspace_mut().split_tree.split(
                                    active_pane,
                                    direction,
                                    new_pane_id,
                                );

                                // Calculate size for new pane from its layout rect
                                let scale = state.scale_factor as f32;
                                let w = state.renderer.width();
                                let h = state.renderer.height();
                                let layout = state.workspace_mgr.active_workspace().split_tree.layout();
                                let (cols, rows) = if let Some((_, pr)) = layout.iter().find(|(id, _)| *id == new_pane_id) {
                                    let px = Self::pane_to_pixel_rect(pr, w, h, scale);
                                    Self::pixel_rect_to_cols_rows(&px, &state.renderer)
                                } else {
                                    Self::rect_to_cols_rows(&state.renderer, state.scale_factor)
                                };

                                let ps = Self::spawn_pane(&self.app.config, new_pane_id, cols, rows, &state.window);
                                state.pane_states.insert(new_pane_id, ps);

                                // Also resize the original pane since it shrunk
                                if let Some((_, pr)) = layout.iter().find(|(id, _)| *id == active_pane) {
                                    let px = Self::pane_to_pixel_rect(pr, w, h, scale);
                                    let (c, r) = Self::pixel_rect_to_cols_rows(&px, &state.renderer);
                                    if let Some(ops) = state.pane_states.get(&active_pane) {
                                        ops.emulator.resize(c, r);
                                        let _ = ops.pty.resize(c, r);
                                    }
                                }

                                state.workspace_mgr.active_workspace_mut().set_active_pane(new_pane_id);
                                Self::update_title(state);
                                state.window.request_redraw();
                                return;
                            }
                            // Cmd+]: Next pane
                            "]" => {
                                let ws = state.workspace_mgr.active_workspace();
                                let current = ws.active_pane();
                                if let Some(next) = ws.split_tree.next_pane(current) {
                                    state.workspace_mgr.active_workspace_mut().set_active_pane(next);
                                    state.window.request_redraw();
                                }
                                return;
                            }
                            // Cmd+[: Previous pane
                            "[" => {
                                let ws = state.workspace_mgr.active_workspace();
                                let current = ws.active_pane();
                                if let Some(prev) = ws.split_tree.prev_pane(current) {
                                    state.workspace_mgr.active_workspace_mut().set_active_pane(prev);
                                    state.window.request_redraw();
                                }
                                return;
                            }
                            // Cmd+1..9: Switch workspace
                            s if s.len() == 1 && s.as_bytes()[0] >= b'1' && s.as_bytes()[0] <= b'9' => {
                                let idx = (s.as_bytes()[0] - b'1') as usize;
                                state.workspace_mgr.select_workspace(idx);
                                Self::update_title(state);
                                state.window.request_redraw();
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                // Clear selection on any other key press
                if state.selection.is_some() {
                    state.selection = None;
                    let active = state.workspace_mgr.active_workspace().active_pane();
                    if let Some(ps) = state.pane_states.get(&active) {
                        ps.dirty.store(true, Ordering::Relaxed);
                    }
                }

                // Send keystrokes to the active pane's PTY
                // Handle Ctrl+letter → control character (0x01..0x1A)
                let ctrl = state.modifiers.control_key();
                let bytes = if ctrl {
                    if let Key::Character(ref c) = event.logical_key {
                        let ch = c.as_str().as_bytes();
                        if ch.len() == 1 && ch[0].is_ascii_alphabetic() {
                            Some(vec![ch[0].to_ascii_lowercase() - b'a' + 1])
                        } else {
                            key_to_bytes(&event)
                        }
                    } else {
                        key_to_bytes(&event)
                    }
                } else {
                    key_to_bytes(&event)
                };
                if let Some(bytes) = bytes {
                    let active = state.workspace_mgr.active_workspace().active_pane();
                    if let Some(ps) = state.pane_states.get(&active) {
                        let _ = ps.pty.write(&bytes);
                    }
                    state.window.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                let t_frame = Instant::now();
                let theme = &self.app.theme;
                let scale = state.scale_factor as f32;
                let w = state.renderer.width();
                let h = state.renderer.height();

                let layout = state.workspace_mgr.active_workspace().split_tree.layout();
                let active_pane = state.workspace_mgr.active_workspace().active_pane();

                let mut pane_rects: Vec<(PaneId, PixelRect)> = Vec::with_capacity(layout.len());
                let cursor_color = theme.colors.cursor;
                let mut any_updated = false;

                let t_grid = Instant::now();
                for (pane_id, pane_rect) in &layout {
                    let px_rect = Self::pane_to_pixel_rect(pane_rect, w, h, scale);

                    if let Some(ps) = state.pane_states.get_mut(pane_id) {
                        let show_cursor = *pane_id == active_pane;
                        let content_dirty = ps.dirty.load(Ordering::Acquire);
                        let cursor_changed = ps.last_cursor_visible != show_cursor;

                        if content_dirty || cursor_changed {
                            let grid = ps.emulator.extract_grid(theme);
                            let cursor_pos = ps.emulator.cursor_position();

                            state.renderer.text_renderer.set_pane_content(
                                *pane_id,
                                &grid,
                                cursor_pos,
                                show_cursor,
                                cursor_color,
                            );
                            ps.last_cursor_visible = show_cursor;
                            ps.dirty.store(false, Ordering::Relaxed);
                            any_updated = true;
                        }
                    }

                    pane_rects.push((*pane_id, px_rect));
                }
                let grid_dur = t_grid.elapsed();

                // Skip GPU work when nothing changed
                if any_updated {
                    let t_prep = Instant::now();
                    state.renderer.text_renderer.prepare_panes(
                        &state.renderer.device,
                        &state.renderer.queue,
                        &pane_rects,
                        theme.colors.foreground,
                    );
                    let prep_dur = t_prep.elapsed();

                    let t_render = Instant::now();
                    let _ = state
                        .renderer
                        .render_frame(theme.colors.background, |_| {});
                    let render_dur = t_render.elapsed();

                    if state.debug_timing {
                        let total = t_frame.elapsed();
                        eprintln!(
                            "[frame] total={:?} grid={:?} prepare={:?} render={:?}",
                            total, grid_dur, prep_dur, render_dur,
                        );
                    }
                }

                // FPS counter in title
                state.frame_count += 1;
                let fps_elapsed = state.fps_timer.elapsed();
                if fps_elapsed >= Duration::from_secs(1) {
                    let fps = state.frame_count as f32 / fps_elapsed.as_secs_f32();
                    state.frame_count = 0;
                    state.fps_timer = Instant::now();
                    let idx = state.workspace_mgr.active_index() + 1;
                    let count = state.workspace_mgr.workspace_count();
                    state.window.set_title(&format!(
                        "pterminal [tab {idx}/{count}] {fps:.0} fps"
                    ));
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.app.state {
            let active_panes = state.workspace_mgr.active_workspace().pane_ids();
            let any_dirty = active_panes.iter().any(|pid| {
                state.pane_states.get(pid).map_or(false, |ps| ps.dirty.load(Ordering::Relaxed))
            });
            if any_dirty {
                state.window.request_redraw();
            }
            // Short safety-net timeout: ensures rendering even if cross-thread
            // request_redraw doesn't immediately wake the macOS run loop.
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(4),
            ));
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
