use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use arboard::Clipboard;
use serde_json::{json, Value};
use tracing::{info, warn};

use pterminal_core::config::theme::Theme;
use pterminal_core::split::{PaneId, SplitDirection};
use pterminal_core::terminal::{PtyHandle, TerminalEmulator};
use pterminal_core::workspace::WorkspaceManager;
use pterminal_core::{Config, NotificationStore};
use pterminal_ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
use pterminal_render::text::PixelRect;
use pterminal_render::{BgRect, OffscreenRenderer};

slint::include_modules!();

// Re-import generated/private types needed in callback signatures
use slint::private_unstable_api::re_exports::{
    EventResult, KeyEvent, PointerEventButton, PointerEventKind,
};

// ---------------------------------------------------------------------------
// Display scale detection (Slint wgpu-28 reports sf=1 on macOS Retina)
// ---------------------------------------------------------------------------

/// On macOS, Slint's wgpu backend may report scale_factor=1 even on Retina
/// displays. Detect the real backing scale via CoreGraphics.
#[cfg(target_os = "macos")]
fn detect_display_scale() -> f64 {
    #[repr(C)]
    struct CGRect {
        origin_x: f64,
        origin_y: f64,
        size_width: f64,
        size_height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    unsafe {
        let display = CGMainDisplayID();
        let hw_pixels = CGDisplayPixelsWide(display) as f64;
        let bounds = CGDisplayBounds(display);
        if bounds.size_width > 0.0 {
            (hw_pixels / bounds.size_width).round().max(1.0)
        } else {
            1.0
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_display_scale() -> f64 {
    1.0
}

// ---------------------------------------------------------------------------
// Supporting types (mirrored from app.rs for the Slint backend)
// ---------------------------------------------------------------------------

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
    redraw_queued: Arc<AtomicBool>,
    render_grid: Vec<pterminal_core::terminal::GridLine>,
    render_dirty_rows: Vec<usize>,
    last_cursor_visible: bool,
}

struct IpcEnvelope {
    request: JsonRpcRequest,
    response_tx: Sender<JsonRpcResponse>,
}

// ---------------------------------------------------------------------------
// Shared mutable state accessible from Slint callbacks
// ---------------------------------------------------------------------------

struct TerminalState {
    renderer: Option<OffscreenRenderer>,
    workspace_mgr: WorkspaceManager,
    pane_states: HashMap<PaneId, PaneState>,
    config: Config,
    theme: Arc<Theme>,
    /// Effective display scale (real Retina factor, may differ from Slint sf).
    /// Used for font sizing, mouse mapping, padding.
    scale_factor: f64,
    /// Slint-reported scale factor. Used only for viewport resize math
    /// (converting Slint lengths → drawable pixels).
    slint_scale_factor: f64,
    clipboard: Option<Clipboard>,
    selection: Option<Selection>,
    mouse_pressed: bool,
    last_mouse_pos: (f64, f64),
    last_click_time: Instant,
    last_click_pos: (u16, u16),
    click_count: u8,
    notifications: NotificationStore,
    ipc_rx: Receiver<IpcEnvelope>,
    _ipc_server: Option<IpcServer>,
    ipc_socket_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub struct SlintApp {
    config: Config,
}

impl SlintApp {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn run(self) -> Result<()> {
        // 1. Select Slint wgpu-28 backend
        slint::BackendSelector::new()
            .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
            .select()
            .map_err(|e| anyhow::anyhow!("Slint backend: {e}"))?;

        // 2. Create AppWindow
        let app = AppWindow::new()?;
        let app_weak = app.as_weak();

        // 3. Shared state
        let theme = Arc::new(Theme::default());
        let workspace_mgr = WorkspaceManager::new();
        let clipboard = Clipboard::new().ok();

        let (ipc_tx, ipc_rx) = mpsc::channel::<IpcEnvelope>();
        let ipc_socket_path = Config::config_dir().join("pterminal.sock");
        let ipc_server = match IpcServer::start(
            &ipc_socket_path,
            Arc::new(move |request: JsonRpcRequest| {
                let req_id = request.id.clone();
                let (resp_tx, resp_rx) = mpsc::channel();
                if ipc_tx
                    .send(IpcEnvelope {
                        request,
                        response_tx: resp_tx,
                    })
                    .is_err()
                {
                    return JsonRpcResponse::internal_error(req_id, "application unavailable");
                }
                match resp_rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(resp) => resp,
                    Err(_) => JsonRpcResponse::internal_error(req_id, "request timed out"),
                }
            }),
        ) {
            Ok(server) => Some(server),
            Err(e) => {
                warn!("failed to start IPC server: {e}");
                None
            }
        };

        let slint_sf = app.window().scale_factor() as f64;
        let display_sf = detect_display_scale();
        let effective_sf = display_sf.max(slint_sf);
        info!(slint_sf, display_sf, effective_sf, "Scale factors");

        let state = Rc::new(RefCell::new(TerminalState {
            renderer: None,
            workspace_mgr,
            pane_states: HashMap::new(),
            config: self.config.clone(),
            theme: theme.clone(),
            scale_factor: effective_sf,
            slint_scale_factor: slint_sf,
            clipboard,
            selection: None,
            mouse_pressed: false,
            last_mouse_pos: (0.0, 0.0),
            last_click_time: Instant::now() - Duration::from_secs(10),
            last_click_pos: (0, 0),
            click_count: 0,
            notifications: NotificationStore::new(),
            ipc_rx,
            _ipc_server: ipc_server,
            ipc_socket_path,
        }));

        // 4. Rendering notifier ─ runs on RenderingSetup and BeforeRendering
        {
            let state = state.clone();
            let app_weak = app_weak.clone();
            let theme = theme.clone();
            let config = self.config.clone();
            app.window().set_rendering_notifier(move |rendering_state, graphics_api| {
                match rendering_state {
                    slint::RenderingState::RenderingSetup => {
                        let slint::GraphicsAPI::WGPU28 { device, queue, .. } = graphics_api
                        else {
                            return;
                        };
                        let mut s = state.borrow_mut();
                        // Initial texture uses the drawable size from Slint.
                        // get_terminal_width/Height may not be available yet,
                        // so use a reasonable default; BeforeRendering will resize.
                        let slint_sf = s.slint_scale_factor as f32;
                        let (init_w, init_h) = if let Some(app) = app_weak.upgrade() {
                            let tw = (app.get_terminal_width() * slint_sf) as u32;
                            let th = (app.get_terminal_height() * slint_sf) as u32;
                            if tw > 0 && th > 0 { (tw, th) } else { (1920, 1216) }
                        } else {
                            (1920, 1216)
                        };
                        let renderer = OffscreenRenderer::new(
                            device.clone(),
                            queue.clone(),
                            init_w,
                            init_h,
                            s.scale_factor, // effective display scale for font
                            config.font.size,
                        );
                        let (cols, rows) = calc_cols_rows(&renderer, s.scale_factor);
                        let ps = spawn_pane_slint(&config, 0, cols, rows);
                        s.pane_states.insert(0, ps);
                        s.renderer = Some(renderer);
                        info!(cols, rows, "Slint: initial pane spawned");
                    }
                    slint::RenderingState::BeforeRendering => {
                        let mut s = state.borrow_mut();
                        if let Some(app) = app_weak.upgrade() {
                            let sf = app.window().scale_factor() as f64;
                            // Update effective scale if Slint's sf changed
                            // (e.g., window moved to a different display, or sf
                            // was 1 at init and is now 2 after layout).
                            let new_effective = sf.max(detect_display_scale());
                            if (new_effective - s.scale_factor).abs() > 0.01 {
                                s.scale_factor = new_effective;
                                s.slint_scale_factor = sf;
                                if let Some(renderer) = &mut s.renderer {
                                    renderer
                                        .text_renderer
                                        .update_scale_factor(new_effective, config.font.size);
                                }
                            }
                            // Viewport resize — use Slint's sf for length→drawable
                            let slint_sf = sf as f32;
                            let tw = (app.get_terminal_width() * slint_sf) as u32;
                            let th = (app.get_terminal_height() * slint_sf) as u32;
                            if let Some(renderer) = &mut s.renderer {
                                if tw > 0
                                    && th > 0
                                    && (tw != renderer.width() || th != renderer.height())
                                {
                                    renderer.resize(tw, th);
                                    resize_active_workspace_panes(&mut s);
                                }
                            }
                        }
                        render_frame(&mut s, &theme, &app_weak);
                    }
                    _ => {}
                }
            })?;
        }

        // 5. Keyboard callback
        {
            let state = state.clone();
            let theme = theme.clone();
            let app_weak2 = app_weak.clone();
            app.on_terminal_key_pressed(move |event| {
                let mut s = state.borrow_mut();
                handle_key_event(&event, &mut s, &theme, &app_weak2);
                EventResult::Accept
            });
        }

        // 6. Tab callbacks
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_tab_clicked(move |idx| {
                let mut s = state.borrow_mut();
                s.workspace_mgr.select_workspace(idx as usize);
                for ps in s.pane_states.values() {
                    ps.dirty.store(true, Ordering::Relaxed);
                }
                update_tabs(&s, &app_weak2);
            });
        }
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_tab_close_clicked(move |idx| {
                let mut s = state.borrow_mut();
                if s.workspace_mgr.workspace_count() <= 1 {
                    return;
                }
                s.workspace_mgr.select_workspace(idx as usize);
                let ws = s.workspace_mgr.active_workspace();
                let pane_ids = ws.pane_ids();
                let ws_id = ws.id;
                for pid in &pane_ids {
                    s.pane_states.remove(pid);
                    if let Some(renderer) = &mut s.renderer {
                        renderer.text_renderer.remove_pane(*pid);
                    }
                }
                s.workspace_mgr.close_workspace(ws_id);
                for ps in s.pane_states.values() {
                    ps.dirty.store(true, Ordering::Relaxed);
                }
                update_tabs(&s, &app_weak2);
            });
        }
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_new_tab_clicked(move || {
                let mut s = state.borrow_mut();
                let (_ws_id, pane_id) = s.workspace_mgr.add_workspace();
                let (cols, rows) = if let Some(renderer) = &s.renderer {
                    calc_cols_rows(renderer, s.scale_factor)
                } else {
                    (80, 24)
                };
                let ps = spawn_pane_slint(&s.config, pane_id, cols, rows);
                s.pane_states.insert(pane_id, ps);
                update_tabs(&s, &app_weak2);
            });
        }

        // 7. Sidebar callback
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_sidebar_item_clicked(move |idx| {
                let mut s = state.borrow_mut();
                s.workspace_mgr.select_workspace(idx as usize);
                for ps in s.pane_states.values() {
                    ps.dirty.store(true, Ordering::Relaxed);
                }
                update_tabs(&s, &app_weak2);
            });
        }

        // 8. Mouse callbacks
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            let theme = theme.clone();
            app.on_terminal_pointer_event(move |event, x, y| {
                let mut s = state.borrow_mut();
                let sf = s.scale_factor as f32;
                let phys_x = x * sf;
                let phys_y = y * sf;
                s.last_mouse_pos = (phys_x as f64, phys_y as f64);

                let is_left_button = event.button == PointerEventButton::Left;
                if !is_left_button {
                    return;
                }

                match event.kind {
                    PointerEventKind::Down => {
                        // Determine which pane was clicked
                        if let Some(clicked_pane) = pane_at_pixel(&s, phys_x, phys_y) {
                            let prev_active = s.workspace_mgr.active_workspace().active_pane();
                            if prev_active != clicked_pane {
                                s.workspace_mgr
                                    .active_workspace_mut()
                                    .set_active_pane(clicked_pane);
                                for ps in s.pane_states.values() {
                                    ps.dirty.store(true, Ordering::Relaxed);
                                }
                            }
                        }

                        s.mouse_pressed = true;
                        let active = s.workspace_mgr.active_workspace().active_pane();
                        let cell = pixel_to_cell(&s, active);
                        let now = Instant::now();
                        let double_click_threshold = Duration::from_millis(400);

                        if now.duration_since(s.last_click_time) < double_click_threshold
                            && s.last_click_pos == cell
                        {
                            s.click_count = (s.click_count % 3) + 1;
                        } else {
                            s.click_count = 1;
                        }
                        s.last_click_time = now;
                        s.last_click_pos = cell;

                        match s.click_count {
                            2 => {
                                s.selection =
                                    Some(word_selection_at(&s, &theme, cell.0, cell.1));
                            }
                            3 => {
                                s.selection = Some(line_selection_at(&s, cell.1));
                            }
                            _ => {
                                s.selection = Some(Selection {
                                    start: cell,
                                    end: cell,
                                });
                            }
                        }
                        if let Some(ps) = s.pane_states.get(&active) {
                            ps.dirty.store(true, Ordering::Relaxed);
                        }
                        request_redraw(&app_weak2);
                    }
                    PointerEventKind::Up => {
                        s.mouse_pressed = false;
                        // Clear zero-length selection on single-click release
                        if s.click_count <= 1 {
                            if let Some(sel) = &s.selection {
                                if sel.start == sel.end {
                                    s.selection = None;
                                    let active =
                                        s.workspace_mgr.active_workspace().active_pane();
                                    if let Some(ps) = s.pane_states.get(&active) {
                                        ps.dirty.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                        request_redraw(&app_weak2);
                    }
                    _ => {}
                }
            });
        }
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_terminal_pointer_move(move |x, y| {
                let mut s = state.borrow_mut();
                let sf = s.scale_factor as f32;
                let phys_x = x * sf;
                let phys_y = y * sf;
                s.last_mouse_pos = (phys_x as f64, phys_y as f64);

                if s.mouse_pressed && s.click_count <= 1 {
                    let active = s.workspace_mgr.active_workspace().active_pane();
                    let cell = pixel_to_cell(&s, active);
                    if let Some(sel) = &mut s.selection {
                        if sel.end != cell {
                            sel.end = cell;
                            if let Some(ps) = s.pane_states.get(&active) {
                                ps.dirty.store(true, Ordering::Relaxed);
                            }
                            request_redraw(&app_weak2);
                        }
                    }
                }
            });
        }
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            app.on_terminal_scroll(move |_dx, dy| {
                let s = state.borrow_mut();
                let (_, cell_h) = if let Some(r) = &s.renderer {
                    r.text_renderer.cell_size()
                } else {
                    return;
                };
                let sf = s.scale_factor as f32;
                let lines = (dy * sf / cell_h).round() as i32;
                if lines != 0 {
                    let active = s.workspace_mgr.active_workspace().active_pane();
                    if let Some(ps) = s.pane_states.get(&active) {
                        ps.emulator.scroll(lines);
                        ps.dirty.store(true, Ordering::Relaxed);
                        request_redraw(&app_weak2);
                    }
                }
            });
        }

        // 9. Timer for polling dirty flags & dead panes
        let poll_timer = slint::Timer::default();
        {
            let state = state.clone();
            let app_weak2 = app_weak.clone();
            poll_timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(4),
                move || {
                    let s = state.borrow();
                    let active_panes = s.workspace_mgr.active_workspace().pane_ids();
                    let any_dirty = active_panes.iter().any(|pid| {
                        s.pane_states
                            .get(pid)
                            .map_or(false, |ps| ps.dirty.load(Ordering::Relaxed))
                    });
                    let any_dead = s.pane_states.values().any(|ps| !ps.pty.is_alive());
                    drop(s);

                    if any_dirty || any_dead {
                        if any_dead {
                            handle_dead_panes(&state, &app_weak2);
                        }
                        request_redraw(&app_weak2);
                    }

                    // Handle IPC requests
                    handle_ipc_requests(&state, &app_weak2);
                },
            );
        }

        // 10. Initial tab bar state
        update_tabs(&state.borrow(), &app_weak);

        // 11. Focus terminal and run
        app.invoke_focus_terminal();
        app.run()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn request_redraw(app_weak: &slint::Weak<AppWindow>) {
    if let Some(app) = app_weak.upgrade() {
        app.window().request_redraw();
    }
}

fn update_tabs(s: &TerminalState, app_weak: &slint::Weak<AppWindow>) {
    let Some(app) = app_weak.upgrade() else { return };
    let active_idx = s.workspace_mgr.active_index();
    let tabs: Vec<TabInfo> = (0..s.workspace_mgr.workspace_count())
        .map(|i| TabInfo {
            title: format!("Tab {}", i + 1).into(),
            active: i == active_idx,
        })
        .collect();
    let model = std::rc::Rc::new(slint::VecModel::from(tabs));
    app.set_tabs(slint::ModelRc::from(model));
}

fn spawn_pane_slint(config: &Config, pane_id: PaneId, cols: u16, rows: u16) -> PaneState {
    let shell = config.shell();
    let cwd = config.working_directory();
    let dirty = Arc::new(AtomicBool::new(true));
    let redraw_queued = Arc::new(AtomicBool::new(false));

    let mut emulator = TerminalEmulator::new(cols, rows);
    let parser_handle = emulator
        .take_parser_handle()
        .expect("terminal parser handle already taken");
    let dirty_for_pty = Arc::clone(&dirty);

    let pty = PtyHandle::spawn(
        &shell,
        &cwd,
        cols,
        rows,
        parser_handle,
        move || {
            dirty_for_pty.store(true, Ordering::Release);
        },
        || {},
    )
    .expect("spawn PTY");

    info!(pane_id, cols, rows, %shell, "Pane spawned (Slint)");

    PaneState {
        emulator,
        pty,
        dirty,
        redraw_queued,
        render_grid: Vec::new(),
        render_dirty_rows: Vec::new(),
        last_cursor_visible: true,
    }
}

fn calc_cols_rows(renderer: &OffscreenRenderer, _scale_factor: f64) -> (u16, u16) {
    let (cell_w, cell_h) = renderer.text_renderer.cell_size();
    let w = renderer.width().max(1);
    let h = renderer.height().max(1);
    let cols = (w as f32 / cell_w).max(1.0) as u16;
    let rows = (h as f32 / cell_h).max(1.0) as u16;
    (cols, rows)
}

fn pixel_rect_to_cols_rows(rect: &PixelRect, renderer: &OffscreenRenderer) -> (u16, u16) {
    let (cell_w, cell_h) = renderer.text_renderer.cell_size();
    let cols = (rect.w / cell_w).max(1.0) as u16;
    let rows = (rect.h / cell_h).max(1.0) as u16;
    (cols, rows)
}

/// Half-width of the divider gap between panes (physical pixels).
const DIVIDER_HALF: f32 = 1.0;
/// Color for pane divider lines (light gray, semi-transparent).
const DIVIDER_COLOR: [f32; 4] = [0.45, 0.45, 0.50, 1.0];

fn pane_to_pixel_rect(
    pane_rect: &pterminal_core::split::PaneRect,
    window_w: u32,
    window_h: u32,
    scale: f32,
    tab_bar_h: f32,
) -> PixelRect {
    let content_w = (window_w as f32).max(1.0);
    let content_h = window_h as f32 - tab_bar_h;
    // Only add gap on sides that border another pane (not window edges).
    let gap = DIVIDER_HALF * scale;
    let left = if pane_rect.x > 0.001 { gap } else { 0.0 };
    let top = if pane_rect.y > 0.001 { gap } else { 0.0 };
    let right = if pane_rect.x + pane_rect.width < 0.999 { gap } else { 0.0 };
    let bottom = if pane_rect.y + pane_rect.height < 0.999 { gap } else { 0.0 };
    PixelRect {
        x: pane_rect.x * content_w + left,
        y: pane_rect.y * content_h + top + tab_bar_h,
        w: pane_rect.width * content_w - left - right,
        h: pane_rect.height * content_h - top - bottom,
    }
}

fn pane_pixel_rect(s: &TerminalState, pane_id: PaneId) -> Option<PixelRect> {
    let renderer = s.renderer.as_ref()?;
    let scale = s.scale_factor as f32;
    let w = renderer.width();
    let h = renderer.height();
    s.workspace_mgr
        .active_workspace()
        .split_tree
        .layout()
        .into_iter()
        .find(|(id, _)| *id == pane_id)
        .map(|(_, rect)| pane_to_pixel_rect(&rect, w, h, scale, 0.0))
}

fn pane_at_pixel(s: &TerminalState, x: f32, y: f32) -> Option<PaneId> {
    let renderer = s.renderer.as_ref()?;
    let scale = s.scale_factor as f32;
    let w = renderer.width();
    let h = renderer.height();
    s.workspace_mgr
        .active_workspace()
        .split_tree
        .layout()
        .into_iter()
        .find_map(|(pane_id, pane_rect)| {
            let px = pane_to_pixel_rect(&pane_rect, w, h, scale, 0.0);
            let in_x = x >= px.x && x < px.x + px.w;
            let in_y = y >= px.y && y < px.y + px.h;
            if in_x && in_y {
                Some(pane_id)
            } else {
                None
            }
        })
}

fn pixel_to_cell(s: &TerminalState, pane_id: PaneId) -> (u16, u16) {
    let renderer = match s.renderer.as_ref() {
        Some(r) => r,
        None => return (0, 0),
    };
    let (cell_w, cell_h) = renderer.text_renderer.cell_size();
    let (mx, my) = (s.last_mouse_pos.0 as f32, s.last_mouse_pos.1 as f32);
    let (px, py) = if let Some(rect) = pane_pixel_rect(s, pane_id) {
        (mx - rect.x, my - rect.y)
    } else {
        (mx, my)
    };
    let col = (px / cell_w).max(0.0) as u16;
    let row = (py / cell_h).max(0.0) as u16;

    if let Some(ps) = s.pane_states.get(&pane_id) {
        let (grid_cols, grid_rows) = ps.emulator.size();
        (
            col.min(grid_cols.saturating_sub(1)),
            row.min(grid_rows.saturating_sub(1)),
        )
    } else {
        (col, row)
    }
}

fn get_selected_text(s: &TerminalState) -> Option<String> {
    let sel = s.selection?;
    let (start, end) = sel.normalized();
    let active_pane = s.workspace_mgr.active_workspace().active_pane();
    let ps = s.pane_states.get(&active_pane)?;
    let grid = ps.emulator.extract_grid(&s.theme);

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
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn word_selection_at(
    s: &TerminalState,
    theme: &Arc<Theme>,
    col: u16,
    row: u16,
) -> Selection {
    let active_pane = s.workspace_mgr.active_workspace().active_pane();
    if let Some(ps) = s.pane_states.get(&active_pane) {
        let grid = ps.emulator.extract_grid(theme);
        if (row as usize) < grid.len() {
            let line = &grid[row as usize];
            let cells = &line.cells;
            let c = col as usize;
            if c < cells.len() {
                let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
                let ch = cells[c].c;
                if is_word_char(ch) {
                    let mut start = c;
                    while start > 0 && is_word_char(cells[start - 1].c) {
                        start -= 1;
                    }
                    let mut end = c;
                    while end + 1 < cells.len() && is_word_char(cells[end + 1].c) {
                        end += 1;
                    }
                    return Selection {
                        start: (start as u16, row),
                        end: (end as u16, row),
                    };
                }
            }
        }
    }
    Selection {
        start: (col, row),
        end: (col, row),
    }
}

fn line_selection_at(s: &TerminalState, row: u16) -> Selection {
    let active_pane = s.workspace_mgr.active_workspace().active_pane();
    let max_col = if let Some(ps) = s.pane_states.get(&active_pane) {
        let (cols, _) = ps.emulator.size();
        cols.saturating_sub(1)
    } else {
        79
    };
    Selection {
        start: (0, row),
        end: (max_col, row),
    }
}

fn resize_active_workspace_panes(s: &mut TerminalState) {
    let Some(renderer) = &s.renderer else { return };
    let scale = s.scale_factor as f32;
    let w = renderer.width();
    let h = renderer.height();
    let layout = s.workspace_mgr.active_workspace().split_tree.layout();
    for (pane_id, pane_rect) in &layout {
        let px_rect = pane_to_pixel_rect(pane_rect, w, h, scale, 0.0);
        let (cols, rows) = pixel_rect_to_cols_rows(&px_rect, renderer);
        if let Some(ps) = s.pane_states.get(pane_id) {
            ps.emulator.resize(cols, rows);
            let _ = ps.pty.resize(cols, rows);
            ps.dirty.store(true, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

fn handle_key_event(
    event: &KeyEvent,
    s: &mut TerminalState,
    _theme: &Arc<Theme>,
    app_weak: &slint::Weak<AppWindow>,
) {
    let text = event.text.to_string();
    if text.is_empty() {
        return;
    }
    let ch = match text.chars().next() {
        Some(c) => c,
        None => return,
    };
    let raw_ctrl = event.modifiers.control;
    let raw_meta = event.modifiers.meta;
    let shift = event.modifiers.shift;

    // On macOS, Slint swaps Command and Control:
    //   Physical Command (⌘) → modifiers.control
    //   Physical Control (⌃) → modifiers.meta
    // Swap them so our logic matches physical keys.
    #[cfg(target_os = "macos")]
    let (ctrl, meta) = (raw_meta, raw_ctrl);
    #[cfg(not(target_os = "macos"))]
    let (ctrl, meta) = (raw_ctrl, raw_meta);

    // Modifier-only keys — ignore ONLY when no Ctrl/Meta modifier is held.
    // When Ctrl is pressed, chars like \u{0016} are Ctrl+V, not modifier-only.
    if !ctrl && !meta {
        match ch {
            '\u{0010}' | '\u{0011}' | '\u{0012}' | '\u{0013}' | '\u{0014}' | '\u{0015}'
            | '\u{0016}' | '\u{0017}' | '\u{0018}' => return,
            _ => {}
        }
    }

    // ── Cmd/Ctrl shortcuts ──
    // On macOS, Cmd (meta) is the primary modifier for UI actions.
    // Ctrl sends terminal control characters.
    let action_mod = meta || ctrl;

    if action_mod {
        // Determine the letter for matching.
        // Slint may send either the literal letter or a control character
        // depending on which modifier is active.
        let letter = if ch.is_ascii_alphabetic() {
            Some(ch.to_ascii_lowercase())
        } else if (ch as u32) >= 1 && (ch as u32) <= 26 {
            Some((b'a' + ch as u8 - 1) as char)
        } else {
            None
        };

        match letter {
            Some('c') => {
                // Copy if selection exists, otherwise send SIGINT (Ctrl+C)
                if s.selection.is_some() {
                    if let Some(txt) = get_selected_text(s) {
                        if let Some(clip) = &mut s.clipboard {
                            let _ = clip.set_text(txt);
                        }
                    }
                } else {
                    let active = s.workspace_mgr.active_workspace().active_pane();
                    if let Some(ps) = s.pane_states.get(&active) {
                        let _ = ps.pty.write(b"\x03");
                    }
                    request_redraw(app_weak);
                }
                return;
            }
            Some('v') => {
                if let Some(clip) = &mut s.clipboard {
                    if let Ok(txt) = clip.get_text() {
                        let active = s.workspace_mgr.active_workspace().active_pane();
                        if let Some(ps) = s.pane_states.get(&active) {
                            let _ = ps.pty.write(txt.as_bytes());
                        }
                    }
                }
                request_redraw(app_weak);
                return;
            }
            Some('t') if meta => {
                let (_ws_id, pane_id) = s.workspace_mgr.add_workspace();
                let (cols, rows) = if let Some(renderer) = &s.renderer {
                    calc_cols_rows(renderer, s.scale_factor)
                } else {
                    (80, 24)
                };
                let ps = spawn_pane_slint(&s.config, pane_id, cols, rows);
                s.pane_states.insert(pane_id, ps);
                update_tabs(s, app_weak);
                request_redraw(app_weak);
                return;
            }
            Some('w') if meta => {
                if s.workspace_mgr.workspace_count() > 1 {
                    let ws = s.workspace_mgr.active_workspace();
                    let pane_ids = ws.pane_ids();
                    let ws_id = ws.id;
                    for pid in &pane_ids {
                        s.pane_states.remove(pid);
                        if let Some(renderer) = &mut s.renderer {
                            renderer.text_renderer.remove_pane(*pid);
                        }
                    }
                    s.workspace_mgr.close_workspace(ws_id);
                    update_tabs(s, app_weak);
                    request_redraw(app_weak);
                }
                return;
            }
            Some('d') | Some('D') if meta => {
                let direction = if shift {
                    SplitDirection::Vertical
                } else {
                    SplitDirection::Horizontal
                };
                let active_pane = s.workspace_mgr.active_workspace().active_pane();
                let new_pane_id = s.workspace_mgr.next_pane_id();
                s.workspace_mgr
                    .active_workspace_mut()
                    .split_tree
                    .split(active_pane, direction, new_pane_id);

                let (cols, rows) = if let Some(renderer) = &s.renderer {
                    let scale = s.scale_factor as f32;
                    let w = renderer.width();
                    let h = renderer.height();
                    let layout = s.workspace_mgr.active_workspace().split_tree.layout();
                    if let Some((_, pr)) = layout.iter().find(|(id, _)| *id == new_pane_id) {
                        let px = pane_to_pixel_rect(pr, w, h, scale, 0.0);
                        pixel_rect_to_cols_rows(&px, renderer)
                    } else {
                        calc_cols_rows(renderer, s.scale_factor)
                    }
                } else {
                    (80, 24)
                };

                let ps = spawn_pane_slint(&s.config, new_pane_id, cols, rows);
                s.pane_states.insert(new_pane_id, ps);

                // Resize original pane
                if let Some(renderer) = &s.renderer {
                    let scale = s.scale_factor as f32;
                    let w = renderer.width();
                    let h = renderer.height();
                    let layout = s.workspace_mgr.active_workspace().split_tree.layout();
                    if let Some((_, pr)) = layout.iter().find(|(id, _)| *id == active_pane) {
                        let px = pane_to_pixel_rect(pr, w, h, scale, 0.0);
                        let (c, r) = pixel_rect_to_cols_rows(&px, renderer);
                        if let Some(ops) = s.pane_states.get(&active_pane) {
                            ops.emulator.resize(c, r);
                            let _ = ops.pty.resize(c, r);
                        }
                    }
                }

                s.workspace_mgr
                    .active_workspace_mut()
                    .set_active_pane(new_pane_id);
                request_redraw(app_weak);
                return;
            }
            Some(']') if meta => {
                let ws = s.workspace_mgr.active_workspace();
                let current = ws.active_pane();
                if let Some(next) = ws.split_tree.next_pane(current) {
                    s.workspace_mgr.active_workspace_mut().set_active_pane(next);
                    for ps in s.pane_states.values() {
                        ps.dirty.store(true, Ordering::Relaxed);
                    }
                    request_redraw(app_weak);
                }
                return;
            }
            Some('[') if meta => {
                let ws = s.workspace_mgr.active_workspace();
                let current = ws.active_pane();
                if let Some(prev) = ws.split_tree.prev_pane(current) {
                    s.workspace_mgr.active_workspace_mut().set_active_pane(prev);
                    for ps in s.pane_states.values() {
                        ps.dirty.store(true, Ordering::Relaxed);
                    }
                    request_redraw(app_weak);
                }
                return;
            }
            Some(c) if meta && c.is_ascii_digit() && c != '0' => {
                let idx = (c as u8 - b'1') as usize;
                if idx < s.workspace_mgr.workspace_count() {
                    s.workspace_mgr.select_workspace(idx);
                    for ps in s.pane_states.values() {
                        ps.dirty.store(true, Ordering::Relaxed);
                    }
                    update_tabs(s, app_weak);
                    request_redraw(app_weak);
                }
                return;
            }
            // Any other Cmd/Ctrl+letter → send control character to PTY
            // e.g. Cmd+L → \x0c (clear), Cmd+D → \x04 (EOF)
            Some(letter) if letter.is_ascii_alphabetic() => {
                let ctrl_byte = letter.to_ascii_lowercase() as u8 - b'a' + 1;
                let active = s.workspace_mgr.active_workspace().active_pane();
                if let Some(ps) = s.pane_states.get(&active) {
                    let _ = ps.pty.write(&[ctrl_byte]);
                }
                request_redraw(app_weak);
                return;
            }
            _ => return, // Unhandled Cmd/Ctrl combo — ignore
        }
    }

    // Clear selection on non-modifier key press
    if s.selection.is_some() {
        s.selection = None;
        let active = s.workspace_mgr.active_workspace().active_pane();
        if let Some(ps) = s.pane_states.get(&active) {
            ps.dirty.store(true, Ordering::Relaxed);
        }
    }

    // Convert key to bytes
    let bytes = slint_key_to_bytes(ch, ctrl, &text);
    if let Some(bytes) = bytes {
        let active = s.workspace_mgr.active_workspace().active_pane();
        if let Some(ps) = s.pane_states.get(&active) {
            let _ = ps.pty.write(&bytes);
        }
        request_redraw(app_weak);
    }
}

fn slint_key_to_bytes(ch: char, ctrl: bool, text: &str) -> Option<Vec<u8>> {
    // Special keys
    match ch {
        '\u{000a}' => return Some(b"\r".to_vec()),       // Return
        '\u{0008}' => return Some(b"\x7f".to_vec()),     // Backspace
        '\u{0009}' => return Some(b"\t".to_vec()),        // Tab
        '\u{001b}' => return Some(b"\x1b".to_vec()),     // Escape
        '\u{007f}' => return Some(b"\x1b[3~".to_vec()),  // Delete
        '\u{F700}' => return Some(b"\x1b[A".to_vec()),   // Up
        '\u{F701}' => return Some(b"\x1b[B".to_vec()),   // Down
        '\u{F702}' => return Some(b"\x1b[D".to_vec()),   // Left
        '\u{F703}' => return Some(b"\x1b[C".to_vec()),   // Right
        '\u{F729}' => return Some(b"\x1b[H".to_vec()),   // Home
        '\u{F72B}' => return Some(b"\x1b[F".to_vec()),   // End
        '\u{F72C}' => return Some(b"\x1b[5~".to_vec()),  // PageUp
        '\u{F72D}' => return Some(b"\x1b[6~".to_vec()),  // PageDown
        '\u{F727}' => return Some(b"\x1b[2~".to_vec()),  // Insert
        '\u{0020}' => return Some(b" ".to_vec()),         // Space
        _ => {}
    }

    // Ctrl+letter → control character
    if ctrl && ch.is_ascii_alphabetic() {
        return Some(vec![ch.to_ascii_lowercase() as u8 - b'a' + 1]);
    }

    // Regular text
    Some(text.as_bytes().to_vec())
}

// ---------------------------------------------------------------------------
// Pane divider lines
// ---------------------------------------------------------------------------

/// Build thin BgRect divider lines at split boundaries between panes.
fn build_divider_rects(
    layout: &[(PaneId, pterminal_core::split::PaneRect)],
    window_w: u32,
    window_h: u32,
    scale: f32,
    tab_bar_h: f32,
) -> Vec<BgRect> {
    if layout.len() <= 1 {
        return Vec::new();
    }
    let cw = (window_w as f32).max(1.0);
    let ch = window_h as f32 - tab_bar_h;
    let thickness = (1.0 * scale).max(1.0);
    let mut rects = Vec::new();
    let mut edges_seen: Vec<(f32, f32, f32, f32, bool)> = Vec::new(); // (pos, start, end, _, is_vertical)

    for (_, pr) in layout {
        // Right edge → vertical divider
        let right = pr.x + pr.width;
        if right > 0.001 && right < 0.999 {
            let px = right * cw;
            let py = pr.y * ch + tab_bar_h;
            let ph = pr.height * ch;
            let key = (px as i32, py as i32, ph as i32);
            if !edges_seen.iter().any(|(a, b, c, _, v)| {
                *v && (*a as i32 == key.0) && (*b as i32 == key.1) && (*c as i32 == key.2)
            }) {
                edges_seen.push((px, py, ph, 0.0, true));
                rects.push(BgRect {
                    x: px - thickness * 0.5,
                    y: py,
                    w: thickness,
                    h: ph,
                    color: DIVIDER_COLOR,
                });
            }
        }
        // Bottom edge → horizontal divider
        let bottom = pr.y + pr.height;
        if bottom > 0.001 && bottom < 0.999 {
            let py = bottom * ch + tab_bar_h;
            let px = pr.x * cw;
            let pw = pr.width * cw;
            let key = (px as i32, py as i32, pw as i32);
            if !edges_seen.iter().any(|(a, b, c, _, v)| {
                !*v && (*a as i32 == key.0) && (*b as i32 == key.1) && (*c as i32 == key.2)
            }) {
                edges_seen.push((px, py, pw, 0.0, false));
                rects.push(BgRect {
                    x: px,
                    y: py - thickness * 0.5,
                    w: pw,
                    h: thickness,
                    color: DIVIDER_COLOR,
                });
            }
        }
    }
    rects
}

// ---------------------------------------------------------------------------
// Render pipeline
// ---------------------------------------------------------------------------

fn render_frame(s: &mut TerminalState, theme: &Arc<Theme>, app_weak: &slint::Weak<AppWindow>) {
    let Some(renderer) = &mut s.renderer else {
        return;
    };
    let w = renderer.width();
    let h = renderer.height();

    let layout = s.workspace_mgr.active_workspace().split_tree.layout();
    let active_pane = s.workspace_mgr.active_workspace().active_pane();

    let mut pane_rects: Vec<(PaneId, PixelRect)> = Vec::with_capacity(layout.len());
    let cursor_color = theme.colors.cursor;
    let mut any_updated = false;

    for (pane_id, pane_rect) in &layout {
        let scale = s.scale_factor as f32;
        let px_rect = pane_to_pixel_rect(pane_rect, w, h, scale, 0.0);

        if let Some(ps) = s.pane_states.get_mut(pane_id) {
            ps.redraw_queued.store(false, Ordering::Release);
            let show_cursor = *pane_id == active_pane;
            let content_dirty = ps.dirty.load(Ordering::Acquire);
            let cursor_changed = ps.last_cursor_visible != show_cursor;
            let selection_active = *pane_id == active_pane && s.selection.is_some();

            if content_dirty || cursor_changed || selection_active {
                let cursor_pos;
                if content_dirty || ps.render_grid.is_empty() {
                    let (delta, cursor) = ps
                        .emulator
                        .extract_grid_delta_with_cursor_into(theme, &mut ps.render_grid);
                    cursor_pos = cursor;
                    ps.render_dirty_rows.clear();
                    if delta.full {
                        ps.render_dirty_rows.extend(0..ps.render_grid.len());
                    } else {
                        ps.render_dirty_rows.extend(delta.dirty_rows);
                    }
                } else {
                    cursor_pos = ps.emulator.cursor_position();
                }
                let sel = if *pane_id == active_pane {
                    s.selection.map(|sel| sel.normalized())
                } else {
                    None
                };

                renderer.text_renderer.set_pane_content(
                    *pane_id,
                    &ps.render_grid,
                    if content_dirty {
                        Some(&ps.render_dirty_rows)
                    } else {
                        Some(&[])
                    },
                    cursor_pos,
                    show_cursor,
                    cursor_color,
                    theme.colors.background,
                    sel,
                    theme.colors.selection_bg,
                );
                ps.last_cursor_visible = show_cursor;
                ps.dirty.store(false, Ordering::Relaxed);
                any_updated = true;
            }
        }

        pane_rects.push((*pane_id, px_rect));
    }

    if !any_updated {
        return;
    }

    let bg_rects = renderer.text_renderer.collect_bg_rects(&pane_rects);
    renderer
        .bg_renderer
        .prepare(&renderer.device, &renderer.queue, &bg_rects, w, h);

    // Draw divider lines between adjacent panes
    let divider_rects = build_divider_rects(&layout, w, h, s.scale_factor as f32, 0.0);
    renderer.overlay_bg_renderer.prepare(
        &renderer.device,
        &renderer.queue,
        &divider_rects,
        w,
        h,
    );

    renderer.text_renderer.prepare_panes(
        &renderer.device,
        &renderer.queue,
        &pane_rects,
        theme.colors.foreground,
    );

    let texture = renderer.render_to_texture(theme.colors.background);
    if let Some(app) = app_weak.upgrade() {
        if let Ok(img) = slint::Image::try_from(texture) {
            app.set_terminal_texture(img);
        }
    }
}

// ---------------------------------------------------------------------------
// Dead pane cleanup
// ---------------------------------------------------------------------------

fn handle_dead_panes(state: &Rc<RefCell<TerminalState>>, app_weak: &slint::Weak<AppWindow>) {
    let mut s = state.borrow_mut();
    let dead_panes: Vec<PaneId> = s
        .pane_states
        .iter()
        .filter(|(_, ps)| !ps.pty.is_alive())
        .map(|(id, _)| *id)
        .collect();

    if dead_panes.is_empty() {
        return;
    }

    for pid in &dead_panes {
        s.pane_states.remove(pid);
        if let Some(renderer) = &mut s.renderer {
            renderer.text_renderer.remove_pane(*pid);
        }
    }

    // Remove dead panes from split trees and fix active pane focus
    let ws_count = s.workspace_mgr.workspace_count();
    for i in 0..ws_count {
        s.workspace_mgr.select_workspace(i);
        {
            let ws = s.workspace_mgr.active_workspace_mut();
            for pid in &dead_panes {
                ws.split_tree.remove(*pid);
            }
        }
        let ws = s.workspace_mgr.active_workspace();
        let active = ws.active_pane();
        if dead_panes.contains(&active) {
            let live_ids: Vec<PaneId> = ws.pane_ids();
            if let Some(first_live) = live_ids.into_iter().find(|p| s.pane_states.contains_key(p)) {
                s.workspace_mgr.active_workspace_mut().set_active_pane(first_live);
            }
        }
    }

    // If all panes are gone, quit
    if s.pane_states.is_empty() {
        drop(s);
        let _ = slint::quit_event_loop();
        return;
    }

    // Clean up empty workspaces (a workspace is "empty" if none of its panes
    // still exist in pane_states — this handles the case where split_tree.remove()
    // can't remove the only leaf)
    let empty_ws_ids: Vec<_> = s
        .workspace_mgr
        .workspaces()
        .iter()
        .filter(|ws| ws.pane_ids().iter().all(|pid| !s.pane_states.contains_key(pid)))
        .map(|ws| ws.id)
        .collect();

    for ws_id in empty_ws_ids {
        if s.workspace_mgr.workspace_count() > 1 {
            s.workspace_mgr.close_workspace(ws_id);
        }
    }

    // Ensure active workspace index is valid after cleanup
    let max_idx = s.workspace_mgr.workspace_count().saturating_sub(1);
    if s.workspace_mgr.active_index() > max_idx {
        s.workspace_mgr.select_workspace(max_idx);
    }

    for ps in s.pane_states.values() {
        ps.dirty.store(true, Ordering::Relaxed);
    }
    // Re-layout surviving panes to fill the freed space
    resize_active_workspace_panes(&mut s);
    update_tabs(&s, app_weak);
}

// ---------------------------------------------------------------------------
// IPC handling
// ---------------------------------------------------------------------------

fn handle_ipc_requests(
    state: &Rc<RefCell<TerminalState>>,
    app_weak: &slint::Weak<AppWindow>,
) {
    let mut s = state.borrow_mut();
    while let Ok(msg) = s.ipc_rx.try_recv() {
        let response = handle_ipc_request(&mut s, msg.request, app_weak);
        let _ = msg.response_tx.send(response);
    }
}

fn handle_ipc_request(
    s: &mut TerminalState,
    request: JsonRpcRequest,
    app_weak: &slint::Weak<AppWindow>,
) -> JsonRpcResponse {
    if request.jsonrpc != "2.0" {
        return JsonRpcResponse::invalid_request(request.id);
    }

    let id = request.id.clone();
    let params = &request.params;

    match request.method.as_str() {
        "ping" | "system.ping" => JsonRpcResponse::success(id, json!({ "pong": true })),
        "capabilities" | "system.capabilities" => JsonRpcResponse::success(
            id,
            json!({
                "methods": [
                    "ping", "capabilities", "identify",
                    "workspace.list", "workspace.new", "workspace.close", "workspace.select",
                    "pane.list", "terminal.send", "pane.read_screen", "pane.capture",
                    "notification.send", "notification.list", "notification.clear"
                ]
            }),
        ),
        "identify" | "system.identify" => JsonRpcResponse::success(
            id,
            json!({
                "app": "pterminal",
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "platform": std::env::consts::OS,
                "socket": s.ipc_socket_path.to_string_lossy(),
            }),
        ),
        "workspace.list" | "list-workspaces" => {
            let active_idx = s.workspace_mgr.active_index();
            let workspaces: Vec<Value> = s
                .workspace_mgr
                .workspaces()
                .iter()
                .enumerate()
                .map(|(idx, ws)| {
                    json!({
                        "id": ws.id,
                        "index": idx,
                        "name": ws.name,
                        "active": idx == active_idx,
                        "pane_count": ws.pane_ids().len()
                    })
                })
                .collect();
            JsonRpcResponse::success(id, json!({ "workspaces": workspaces }))
        }
        "workspace.new" | "new-workspace" => {
            let (_ws_id, pane_id) = s.workspace_mgr.add_workspace();
            let (cols, rows) = if let Some(renderer) = &s.renderer {
                calc_cols_rows(renderer, s.scale_factor)
            } else {
                (80, 24)
            };
            let ps = spawn_pane_slint(&s.config, pane_id, cols, rows);
            s.pane_states.insert(pane_id, ps);
            update_tabs(s, app_weak);
            request_redraw(app_weak);
            JsonRpcResponse::success(id, json!({ "workspace_id": _ws_id, "pane_id": pane_id }))
        }
        "workspace.close" | "close-workspace" => {
            let target_ws = params
                .get("id")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| s.workspace_mgr.active_workspace().id);
            if s.workspace_mgr.workspace_count() <= 1 {
                return JsonRpcResponse::invalid_params(id, "cannot close last workspace");
            }
            let Some((ws_id, pane_ids)) = s
                .workspace_mgr
                .workspaces()
                .iter()
                .find(|ws| ws.id == target_ws)
                .map(|ws| (ws.id, ws.pane_ids()))
            else {
                return JsonRpcResponse::invalid_params(id, "workspace not found");
            };
            for pid in &pane_ids {
                s.pane_states.remove(pid);
                if let Some(renderer) = &mut s.renderer {
                    renderer.text_renderer.remove_pane(*pid);
                }
            }
            s.workspace_mgr.close_workspace(ws_id);
            update_tabs(s, app_weak);
            request_redraw(app_weak);
            JsonRpcResponse::success(id, json!({ "closed_workspace_id": ws_id }))
        }
        "workspace.select" | "select-workspace" => {
            let index = if let Some(ws_id) = params.get("id").and_then(Value::as_u64) {
                s.workspace_mgr
                    .workspaces()
                    .iter()
                    .position(|ws| ws.id == ws_id)
            } else {
                params
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
            };
            let Some(index) = index else {
                return JsonRpcResponse::invalid_params(id, "workspace id or index required");
            };
            if index >= s.workspace_mgr.workspace_count() {
                return JsonRpcResponse::invalid_params(id, "workspace index out of range");
            }
            s.workspace_mgr.select_workspace(index);
            update_tabs(s, app_weak);
            request_redraw(app_weak);
            JsonRpcResponse::success(
                id,
                json!({
                    "selected_index": index,
                    "workspace_id": s.workspace_mgr.active_workspace().id
                }),
            )
        }
        "pane.list" | "list-panes" => {
            let panes: Vec<Value> = s
                .workspace_mgr
                .active_workspace()
                .pane_ids()
                .into_iter()
                .map(|pane_id| {
                    json!({
                        "id": pane_id,
                        "active": pane_id == s.workspace_mgr.active_workspace().active_pane(),
                        "alive": s.pane_states.get(&pane_id).is_some_and(|ps| ps.pty.is_alive())
                    })
                })
                .collect();
            JsonRpcResponse::success(id, json!({ "panes": panes }))
        }
        "terminal.send" | "send" => {
            let Some(text) = params.get("text").and_then(Value::as_str) else {
                return JsonRpcResponse::invalid_params(id, "missing params.text");
            };
            let pane_id = params
                .get("pane_id")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| s.workspace_mgr.active_workspace().active_pane());
            let Some(ps) = s.pane_states.get(&pane_id) else {
                return JsonRpcResponse::invalid_params(id, "pane not found");
            };
            if let Err(e) = ps.pty.write(text.as_bytes()) {
                return JsonRpcResponse::internal_error(id, format!("pty write failed: {e}"));
            }
            request_redraw(app_weak);
            JsonRpcResponse::success(id, json!({ "pane_id": pane_id, "bytes": text.len() }))
        }
        "pane.read_screen" | "read-screen" | "pane.capture" | "capture-pane" => {
            let pane_id = params
                .get("pane_id")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| s.workspace_mgr.active_workspace().active_pane());
            let Some(ps) = s.pane_states.get(&pane_id) else {
                return JsonRpcResponse::invalid_params(id, "pane not found");
            };
            let grid = ps.emulator.extract_grid(&s.theme);
            let text = grid_to_text(&grid);
            JsonRpcResponse::success(id, json!({ "pane_id": pane_id, "text": text }))
        }
        "notification.send" | "notify" => {
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Notification");
            let body = params
                .get("body")
                .and_then(Value::as_str)
                .or_else(|| params.get("message").and_then(Value::as_str))
                .unwrap_or("");
            let item = s.notifications.push(title, body);
            request_redraw(app_weak);
            JsonRpcResponse::success(id, json!({ "notification": item }))
        }
        "notification.list" | "list-notifications" => {
            JsonRpcResponse::success(id, json!({ "notifications": s.notifications.list() }))
        }
        "notification.clear" | "clear-notifications" => {
            s.notifications.clear();
            request_redraw(app_weak);
            JsonRpcResponse::success(id, json!({ "cleared": true }))
        }
        _ => JsonRpcResponse::method_not_found(id, &request.method),
    }
}

fn grid_to_text(grid: &[pterminal_core::terminal::GridLine]) -> String {
    let mut out = String::new();
    for (row_idx, line) in grid.iter().enumerate() {
        let mut row = String::with_capacity(line.cells.len());
        for cell in &line.cells {
            let c = if cell.c == '\0' { ' ' } else { cell.c };
            row.push(c);
        }
        while row.ends_with(' ') {
            row.pop();
        }
        out.push_str(&row);
        if row_idx + 1 < grid.len() {
            out.push('\n');
        }
    }
    out
}
