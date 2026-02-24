use pterminal_core::terminal::GridLine;

use crate::text::{TerminalCell, TerminalLine};

/// Convert core GridLine to render TerminalLine
pub fn grid_to_render_lines(grid: &[GridLine]) -> Vec<TerminalLine> {
    grid.iter()
        .map(|line| TerminalLine {
            cells: line
                .cells
                .iter()
                .map(|cell| TerminalCell {
                    c: cell.c,
                    fg: cell.fg,
                    bg: cell.bg,
                    bold: cell.bold,
                    italic: cell.italic,
                    underline: cell.underline,
                })
                .collect(),
        })
        .collect()
}
