use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as AlacrittyEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, Term};
use alacritty_terminal::vte::ansi::{self, StdSyncHandler};

use crate::config::theme::{RgbColor, Theme};
use crate::event::TermEvent;

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

/// Shared terminal state protected by Mutex
struct TermInner {
    term: Term<Listener>,
    processor: ansi::Processor<StdSyncHandler>,
}

/// Terminal emulator wrapping alacritty_terminal
pub struct TerminalEmulator {
    inner: Arc<Mutex<TermInner>>,
    event_rx: std::sync::mpsc::Receiver<TermEvent>,
}

impl TerminalEmulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let listener = Listener { sender: tx };
        let size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(term::Config::default(), &size, listener);
        let processor = ansi::Processor::new();

        Self {
            inner: Arc::new(Mutex::new(TermInner { term, processor })),
            event_rx: rx,
        }
    }

    /// Process raw bytes from PTY output (persistent VTE parser state)
    pub fn process(&self, data: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        let TermInner {
            ref mut term,
            ref mut processor,
        } = *inner;
        processor.advance(term, data);
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
        let inner = self.inner.lock().unwrap();
        (
            inner.term.columns() as u16,
            inner.term.screen_lines() as u16,
        )
    }

    /// Resize the terminal
    pub fn resize(&self, cols: u16, rows: u16) {
        let mut inner = self.inner.lock().unwrap();
        let size = TermSize::new(cols as usize, rows as usize);
        inner.term.resize(size);
    }

    /// Clone the inner Arc for sharing with PTY reader thread
    pub fn clone_inner(&self) -> TerminalEmulatorHandle {
        TerminalEmulatorHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Get cursor position as (col, row)
    pub fn cursor_position(&self) -> (u16, u16) {
        let inner = self.inner.lock().unwrap();
        let cursor = inner.term.grid().cursor.point;
        (cursor.column.0 as u16, cursor.line.0 as u16)
    }

    /// Scroll the display by delta lines (positive = scroll up into history)
    pub fn scroll(&self, delta: i32) {
        use alacritty_terminal::grid::Scroll;
        let mut inner = self.inner.lock().unwrap();
        inner.term.grid_mut().scroll_display(Scroll::Delta(delta));
    }

    /// Get current display offset (0 = bottom, >0 = scrolled into history)
    pub fn display_offset(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.term.grid().display_offset()
    }

    /// Extract terminal grid content for rendering (respects display_offset for scrollback)
    pub fn extract_grid(&self, theme: &Theme) -> Vec<GridLine> {
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Flags;

        let inner = self.inner.lock().unwrap();
        let grid = inner.term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let display_offset = grid.display_offset();
        let mut lines = Vec::with_capacity(num_lines);

        for line_idx in 0..num_lines {
            let mut cells = Vec::with_capacity(num_cols);
            // display_offset shifts which line we render:
            // line_idx 0 with offset N → Line(-(N as i32))
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
            lines.push(GridLine { cells });
        }

        lines
    }
}

/// Handle for sharing emulator with PTY reader thread
pub struct TerminalEmulatorHandle {
    inner: Arc<Mutex<TermInner>>,
}

impl TerminalEmulatorHandle {
    pub fn process(&self, data: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        let TermInner {
            ref mut term,
            ref mut processor,
        } = *inner;
        processor.advance(term, data);
    }
}

/// A line of terminal cells
pub struct GridLine {
    pub cells: Vec<GridCell>,
}

/// A single terminal cell extracted for rendering
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
