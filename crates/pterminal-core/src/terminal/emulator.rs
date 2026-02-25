use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use alacritty_terminal::event::{Event as AlacrittyEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term, TermDamage};
use alacritty_terminal::vte::ansi::{self, StdSyncHandler};

use crate::config::theme::{RgbColor, Theme};
use crate::event::TermEvent;
use crate::terminal::spsc;

const PARSER_CONTROL_QUEUE_DEPTH: usize = 512;
const PARSER_INPUT_QUEUE_DEPTH: usize = 2048;
const PARSER_IDLE_PARK_MS: u64 = 5;

/// Event listener that collects events
#[derive(Clone)]
struct Listener {
    sender: std::sync::mpsc::Sender<TermEvent>,
}

impl EventListener for Listener {
    fn send_event(&self, event: AlacrittyEvent) {
        match event {
            AlacrittyEvent::Title(title) => {
                let _ = self.sender.send(TermEvent::TitleChanged(title));
            }
            AlacrittyEvent::Bell => {
                let _ = self.sender.send(TermEvent::Bell);
            }
            _ => {}
        }
    }
}

/// Terminal parser state owned exclusively by the parser thread.
struct TermInner {
    term: Term<Listener>,
    processor: ansi::Processor<StdSyncHandler>,
}

/// Terminal emulator wrapping alacritty_terminal
pub struct TerminalEmulator {
    control_tx: spsc::Producer<ControlCommand>,
    input_tx: Option<spsc::Producer<Vec<u8>>>,
    event_rx: Receiver<TermEvent>,
    parser_waker: std::thread::Thread,
    parser_thread: Option<std::thread::JoinHandle<()>>,
}

/// Result of incrementally extracting the viewport grid.
#[derive(Debug, Default, Clone)]
pub struct GridDelta {
    pub full: bool,
    pub dirty_rows: Vec<usize>,
}

impl GridDelta {
    pub fn is_empty(&self) -> bool {
        !self.full && self.dirty_rows.is_empty()
    }
}

enum ControlCommand {
    Input(Vec<u8>),
    Resize(u16, u16),
    Scroll(i32),
    QuerySize(Sender<(u16, u16)>),
    QueryCursor(Sender<(u16, u16)>),
    QueryDisplayOffset(Sender<usize>),
    ExtractFull {
        theme: Arc<Theme>,
        reply: Sender<Vec<GridLine>>,
    },
    ExtractDelta {
        theme: Arc<Theme>,
        reply: Sender<DeltaExtractReply>,
    },
    Shutdown,
}

struct DeltaExtractReply {
    delta: GridDelta,
    rows: Vec<(usize, GridLine)>,
    cursor: (u16, u16),
}

impl TerminalEmulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let (control_tx, control_rx) = spsc::channel(PARSER_CONTROL_QUEUE_DEPTH);
        let (input_tx, input_rx) = spsc::channel::<Vec<u8>>(PARSER_INPUT_QUEUE_DEPTH);

        let parser_thread = std::thread::Builder::new()
            .name("term-parser".into())
            .spawn(move || {
                let listener = Listener { sender: event_tx };
                let size = TermSize::new(cols as usize, rows as usize);
                let term = Term::new(term::Config::default(), &size, listener);
                let processor = ansi::Processor::new();
                let mut inner = TermInner { term, processor };
                let mut render_cache: Vec<GridLine> = Vec::new();

                loop {
                    let mut did_work = false;

                    while let Some(data) = input_rx.try_pop() {
                        let TermInner {
                            ref mut term,
                            ref mut processor,
                        } = inner;
                        processor.advance(term, &data);
                        did_work = true;
                    }

                    while let Some(cmd) = control_rx.try_pop() {
                        did_work = true;
                        if handle_control_command(cmd, &mut inner, &mut render_cache) {
                            return;
                        }
                    }

                    if !did_work {
                        if input_rx.is_producer_closed() && control_rx.is_producer_closed() {
                            return;
                        }
                        std::thread::park_timeout(Duration::from_millis(PARSER_IDLE_PARK_MS));
                    }
                }
            })
            .expect("spawn terminal parser thread");
        let parser_waker = parser_thread.thread().clone();

        Self {
            control_tx,
            input_tx: Some(input_tx),
            event_rx,
            parser_waker,
            parser_thread: Some(parser_thread),
        }
    }

    /// Process raw bytes from PTY output (persistent VTE parser state)
    pub fn process(&self, data: &[u8]) {
        if let Some(input_tx) = &self.input_tx {
            let _ = enqueue_input_bytes(input_tx, &self.parser_waker, data);
            return;
        }
        let _ = send_control(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::Input(data.to_vec()),
        );
    }

    /// Drain pending events
    pub fn poll_events(&self) -> Vec<TermEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Get current dimensions
    pub fn size(&self) -> (u16, u16) {
        let (tx, rx) = mpsc::channel();
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::QuerySize(tx),
        );
        rx.recv().unwrap_or((0, 0))
    }

    /// Resize the terminal
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::Resize(cols, rows),
        );
    }

    /// Take the dedicated parser-input handle for the PTY reader thread.
    ///
    /// This is a one-time operation because the underlying queue is SPSC.
    pub fn take_parser_handle(&mut self) -> Option<TerminalEmulatorHandle> {
        self.input_tx.take().map(|input_tx| TerminalEmulatorHandle {
            input_tx,
            parser_waker: self.parser_waker.clone(),
        })
    }

    /// Get cursor position as (col, row)
    pub fn cursor_position(&self) -> (u16, u16) {
        let (tx, rx) = mpsc::channel();
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::QueryCursor(tx),
        );
        rx.recv().unwrap_or((0, 0))
    }

    /// Scroll the display by delta lines (positive = scroll up into history)
    pub fn scroll(&self, delta: i32) {
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::Scroll(delta),
        );
    }

    /// Get current display offset (0 = bottom, >0 = scrolled into history)
    pub fn display_offset(&self) -> usize {
        let (tx, rx) = mpsc::channel();
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::QueryDisplayOffset(tx),
        );
        rx.recv().unwrap_or(0)
    }

    /// Extract terminal grid content for rendering (respects display_offset for scrollback)
    pub fn extract_grid(&self, theme: &Arc<Theme>) -> Vec<GridLine> {
        let (tx, rx) = mpsc::channel();
        if send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::ExtractFull {
                theme: Arc::clone(theme),
                reply: tx,
            },
        )
        .is_err()
        {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }

    /// Incrementally update a cached grid snapshot using alacritty's damage tracking.
    ///
    /// This updates `out` in place and returns which viewport rows changed.
    pub fn extract_grid_delta_into(&self, theme: &Arc<Theme>, out: &mut Vec<GridLine>) -> GridDelta {
        self.extract_grid_delta_with_cursor_into(theme, out).0
    }

    /// Incrementally update a cached grid snapshot and return current cursor position
    /// from the same parser-thread snapshot to avoid an extra roundtrip.
    pub fn extract_grid_delta_with_cursor_into(
        &self,
        theme: &Arc<Theme>,
        out: &mut Vec<GridLine>,
    ) -> (GridDelta, (u16, u16)) {
        let (tx, rx) = mpsc::channel();
        if send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::ExtractDelta {
                theme: Arc::clone(theme),
                reply: tx,
            },
        )
        .is_err()
        {
            return (GridDelta::default(), (0, 0));
        }

        let Ok(reply) = rx.recv() else {
            return (GridDelta::default(), (0, 0));
        };

        let mut max_row = 0usize;
        for (row, _) in &reply.rows {
            max_row = max_row.max(*row + 1);
        }

        if reply.delta.full {
            out.clear();
        }
        if out.len() < max_row {
            out.resize_with(max_row, || GridLine { cells: Vec::new() });
        }

        for (row_idx, line) in reply.rows {
            if row_idx >= out.len() {
                out.resize_with(row_idx + 1, || GridLine { cells: Vec::new() });
            }
            out[row_idx] = line;
        }

        (reply.delta, reply.cursor)
    }
}

/// Handle for sharing emulator with PTY reader thread
pub struct TerminalEmulatorHandle {
    input_tx: spsc::Producer<Vec<u8>>,
    parser_waker: std::thread::Thread,
}

impl TerminalEmulatorHandle {
    pub fn process(&self, data: &[u8]) {
        let _ = enqueue_input_bytes(&self.input_tx, &self.parser_waker, data);
    }
}

impl Drop for TerminalEmulator {
    fn drop(&mut self) {
        let _ = send_control_blocking(
            &self.control_tx,
            &self.parser_waker,
            ControlCommand::Shutdown,
        );
        if let Some(handle) = self.parser_thread.take() {
            let _ = handle.join();
        }
    }
}

fn enqueue_input_bytes(
    input_tx: &spsc::Producer<Vec<u8>>,
    parser_waker: &std::thread::Thread,
    data: &[u8],
) -> Result<(), ()> {
    if data.is_empty() {
        return Ok(());
    }

    input_tx.push_blocking(data.to_vec()).map_err(|_| ())?;
    parser_waker.unpark();
    Ok(())
}

fn send_control(
    control_tx: &spsc::Producer<ControlCommand>,
    parser_waker: &std::thread::Thread,
    cmd: ControlCommand,
) -> Result<(), ControlCommand> {
    control_tx.push_blocking(cmd)?;
    parser_waker.unpark();
    Ok(())
}

fn send_control_blocking(
    control_tx: &spsc::Producer<ControlCommand>,
    parser_waker: &std::thread::Thread,
    cmd: ControlCommand,
) -> Result<(), ControlCommand> {
    send_control(control_tx, parser_waker, cmd)
}

fn handle_control_command(
    cmd: ControlCommand,
    inner: &mut TermInner,
    render_cache: &mut Vec<GridLine>,
) -> bool {
    match cmd {
        ControlCommand::Input(data) => {
            let TermInner {
                ref mut term,
                ref mut processor,
            } = inner;
            processor.advance(term, &data);
        }
        ControlCommand::Resize(cols, rows) => {
            inner
                .term
                .resize(TermSize::new(cols as usize, rows as usize));
        }
        ControlCommand::Scroll(delta) => {
            use alacritty_terminal::grid::Scroll;
            inner.term.grid_mut().scroll_display(Scroll::Delta(delta));
        }
        ControlCommand::QuerySize(reply) => {
            let _ = reply.send((
                inner.term.columns() as u16,
                inner.term.screen_lines() as u16,
            ));
        }
        ControlCommand::QueryCursor(reply) => {
            let cursor = inner.term.grid().cursor.point;
            let _ = reply.send((cursor.column.0 as u16, cursor.line.0 as u16));
        }
        ControlCommand::QueryDisplayOffset(reply) => {
            let _ = reply.send(inner.term.grid().display_offset());
        }
        ControlCommand::ExtractFull { theme, reply } => {
            let lines = extract_grid_full_from_term(&inner.term, &theme);
            let _ = reply.send(lines);
        }
        ControlCommand::ExtractDelta { theme, reply } => {
            let delta = extract_grid_delta_from_term(&mut inner.term, &theme, render_cache);
            let rows = if delta.full {
                render_cache.iter().cloned().enumerate().collect()
            } else {
                delta
                    .dirty_rows
                    .iter()
                    .filter_map(|&row| render_cache.get(row).cloned().map(|line| (row, line)))
                    .collect()
            };
            let cursor = inner.term.grid().cursor.point;
            let _ = reply.send(DeltaExtractReply {
                delta,
                rows,
                cursor: (cursor.column.0 as u16, cursor.line.0 as u16),
            });
        }
        ControlCommand::Shutdown => return true,
    }
    false
}

fn extract_grid_full_from_term(term: &Term<Listener>, theme: &Theme) -> Vec<GridLine> {
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;

    let grid = term.grid();
    let num_lines = grid.screen_lines();
    let num_cols = grid.columns();
    let display_offset = grid.display_offset();
    let mut lines = Vec::with_capacity(num_lines);

    for line_idx in 0..num_lines {
        let mut cells = Vec::with_capacity(num_cols);
        let actual_line = line_idx as i32 - display_offset as i32;
        for col_idx in 0..num_cols {
            let point = alacritty_terminal::index::Point::new(Line(actual_line), Column(col_idx));
            let cell = &grid[point];
            let fg = alacritty_color_to_rgb(&cell.fg, theme);
            let bg = alacritty_color_to_rgb(&cell.bg, theme);
            let flags = cell.flags;

            cells.push(GridCell {
                c: cell.c,
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.contains(Flags::UNDERLINE),
                wide_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
            });
        }
        lines.push(GridLine { cells });
    }

    lines
}

fn extract_grid_delta_from_term(
    term: &mut Term<Listener>,
    theme: &Theme,
    out: &mut Vec<GridLine>,
) -> GridDelta {
    use alacritty_terminal::index::{Column, Line};
    use alacritty_terminal::term::cell::Flags;

    let num_lines = term.grid().screen_lines();
    let num_cols = term.grid().columns();
    let display_offset = term.grid().display_offset();

    let shape_changed = out.len() != num_lines
        || out.first().is_some_and(|line| line.cells.len() != num_cols)
        || (out.len() > 1 && out.iter().any(|line| line.cells.len() != num_cols));

    let mut delta = GridDelta::default();

    match term.damage() {
        TermDamage::Full => delta.full = true,
        TermDamage::Partial(lines) => {
            delta
                .dirty_rows
                .extend(lines.filter_map(|d| (d.line < num_lines).then_some(d.line)));
        }
    }

    if shape_changed {
        delta.full = true;
        delta.dirty_rows.clear();
    }

    let grid = term.grid();

    if delta.full {
        // Resize line count but reuse existing cell Vec capacity.
        out.resize_with(num_lines, || GridLine { cells: Vec::with_capacity(num_cols) });
        out.truncate(num_lines);
        for line_idx in 0..num_lines {
            let cells = &mut out[line_idx].cells;
            cells.clear();
            let actual_line = line_idx as i32 - display_offset as i32;
            for col_idx in 0..num_cols {
                let point =
                    alacritty_terminal::index::Point::new(Line(actual_line), Column(col_idx));
                let cell = &grid[point];
                let fg = alacritty_color_to_rgb(&cell.fg, theme);
                let bg = alacritty_color_to_rgb(&cell.bg, theme);
                let flags = cell.flags;

                cells.push(GridCell {
                    c: cell.c,
                    fg,
                    bg,
                    bold: flags.contains(Flags::BOLD),
                    italic: flags.contains(Flags::ITALIC),
                    underline: flags.contains(Flags::UNDERLINE),
                    wide_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
                });
            }
        }
        delta.dirty_rows.extend(0..num_lines);
    } else {
        for &line_idx in &delta.dirty_rows {
            if line_idx >= out.len() {
                continue;
            }

            let cells = &mut out[line_idx].cells;
            cells.clear();

            let actual_line = line_idx as i32 - display_offset as i32;
            for col_idx in 0..num_cols {
                let point =
                    alacritty_terminal::index::Point::new(Line(actual_line), Column(col_idx));
                let cell = &grid[point];
                let fg = alacritty_color_to_rgb(&cell.fg, theme);
                let bg = alacritty_color_to_rgb(&cell.bg, theme);
                let flags = cell.flags;

                cells.push(GridCell {
                    c: cell.c,
                    fg,
                    bg,
                    bold: flags.contains(Flags::BOLD),
                    italic: flags.contains(Flags::ITALIC),
                    underline: flags.contains(Flags::UNDERLINE),
                    wide_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
                });
            }
        }
    }

    term.reset_damage();
    delta
}

/// A line of terminal cells
#[derive(Clone)]
pub struct GridLine {
    pub cells: Vec<GridCell>,
}

/// A single terminal cell extracted for rendering
#[derive(Clone, Copy)]
pub struct GridCell {
    pub c: char,
    pub fg: RgbColor,
    pub bg: RgbColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// True if this cell is a spacer for a preceding wide (CJK) character
    pub wide_spacer: bool,
}

/// Convert alacritty_terminal color to our RgbColor
pub fn alacritty_color_to_rgb(color: &ansi::Color, theme: &Theme) -> RgbColor {
    match color {
        ansi::Color::Named(named) => {
            use ansi::NamedColor;
            match named {
                NamedColor::Foreground | NamedColor::BrightForeground => theme.colors.foreground,
                NamedColor::Background => theme.colors.background,
                NamedColor::Cursor => theme.colors.cursor,
                NamedColor::DimBlack => dim_color(theme.colors.ansi[0]),
                NamedColor::DimRed => dim_color(theme.colors.ansi[1]),
                NamedColor::DimGreen => dim_color(theme.colors.ansi[2]),
                NamedColor::DimYellow => dim_color(theme.colors.ansi[3]),
                NamedColor::DimBlue => dim_color(theme.colors.ansi[4]),
                NamedColor::DimMagenta => dim_color(theme.colors.ansi[5]),
                NamedColor::DimCyan => dim_color(theme.colors.ansi[6]),
                NamedColor::DimWhite => dim_color(theme.colors.ansi[7]),
                _ => {
                    let idx = *named as usize;
                    if idx < 16 {
                        theme.colors.ansi[idx]
                    } else {
                        theme.colors.foreground
                    }
                }
            }
        }
        ansi::Color::Spec(rgb) => RgbColor::new(rgb.r, rgb.g, rgb.b),
        ansi::Color::Indexed(idx) => {
            if (*idx as usize) < 16 {
                theme.colors.ansi[*idx as usize]
            } else {
                // 256-color palette: compute from index
                index_256_to_rgb(*idx)
            }
        }
    }
}

/// Dim a color by reducing brightness ~33%
fn dim_color(c: RgbColor) -> RgbColor {
    RgbColor::new(
        (c.r as u16 * 2 / 3) as u8,
        (c.g as u16 * 2 / 3) as u8,
        (c.b as u16 * 2 / 3) as u8,
    )
}

fn index_256_to_rgb(idx: u8) -> RgbColor {
    if idx < 16 {
        // Should be handled by caller
        RgbColor::new(0, 0, 0)
    } else if idx < 232 {
        // 216 color cube: 6x6x6
        let idx = idx - 16;
        let r = (idx / 36) % 6;
        let g = (idx / 6) % 6;
        let b = idx % 6;
        let to_val = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        RgbColor::new(to_val(r), to_val(g), to_val(b))
    } else {
        // Grayscale ramp
        let v = 8 + 10 * (idx - 232);
        RgbColor::new(v, v, v)
    }
}
