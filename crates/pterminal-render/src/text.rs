use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer as GlyphonTextRenderer,
    Viewport, Weight,
};

use pterminal_core::config::theme::RgbColor;
use pterminal_core::split::PaneId;
use pterminal_core::terminal::GridLine;

/// A colored span referencing byte ranges in a shared String
struct RichSpan {
    start: usize,
    end: usize,
    fg: RgbColor,
    bold: bool,
    italic: bool,
}

/// Pixel rectangle for pane positioning (physical pixels)
pub struct PixelRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Per-line render buffer with change detection
struct LineBuffer {
    buffer: Buffer,
    content_hash: u64,
}

/// Per-pane collection of line buffers + background rects
struct PaneBuffer {
    lines: Vec<LineBuffer>,
    /// Background rects in cell-relative coords (col, row, color)
    bg_cells: Vec<BgCell>,
    /// Cursor position and color for vertical bar rendering
    cursor: Option<(u16, u16, [f32; 4])>, // (col, row, color)
}

/// A cell that needs a non-default background
struct BgCell {
    col: u16,
    row: u16,
    color: [f32; 4],
}

/// Text rendering using glyphon (cosmic-text + wgpu), supporting multiple panes.
/// Uses per-line Buffers so only changed lines are reshaped.
pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    glyphon_renderer: GlyphonTextRenderer,
    viewport: Viewport,
    pane_buffers: HashMap<PaneId, PaneBuffer>,
    width: u32,
    height: u32,
    scale_factor: f32,
    font_size: f32,
    line_height: f32,
}

impl TextRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale_factor: f64,
        font_size: f32,
    ) -> Self {
        let scale = scale_factor as f32;
        let scaled_font_size = font_size * scale;
        let scaled_line_height = (font_size * 1.22) * scale;

        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db.set_monospace_family("Menlo");
        db.set_sans_serif_family("PingFang SC");
        db.set_serif_family("PingFang SC");
        // Use zh locale so CJK fallback picks PingFang SC (黑体) not STSong (宋体)
        let font_system = FontSystem::new_with_locale_and_db("zh-Hans".to_string(), db);
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let glyphon_renderer =
            GlyphonTextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(device, &cache);

        Self {
            font_system,
            swash_cache,
            atlas,
            glyphon_renderer,
            viewport,
            pane_buffers: HashMap::new(),
            width,
            height,
            scale_factor: scale,
            font_size: scaled_font_size,
            line_height: scaled_line_height,
        }
    }

    pub fn resize(&mut self, _queue: &wgpu::Queue, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    pub fn update_scale_factor(&mut self, scale_factor: f64, font_size: f32) {
        let scale = scale_factor as f32;
        self.scale_factor = scale;
        self.font_size = font_size * scale;
        self.line_height = (font_size * 1.22) * scale;
        let metrics = Metrics::new(self.font_size, self.line_height);
        for pb in self.pane_buffers.values_mut() {
            for lb in &mut pb.lines {
                lb.buffer.set_metrics(&mut self.font_system, metrics);
            }
        }
    }

    /// Update a pane's line buffers. Only reshapes lines whose content changed.
    pub fn set_pane_content(
        &mut self,
        pane_id: PaneId,
        grid: &[GridLine],
        cursor_pos: (u16, u16),
        cursor_visible: bool,
        cursor_color: RgbColor,
        default_bg: RgbColor,
    ) {
        let metrics = Metrics::new(self.font_size, self.line_height);
        let pb = self.pane_buffers.entry(pane_id).or_insert_with(|| PaneBuffer {
            lines: Vec::new(),
            bg_cells: Vec::new(),
            cursor: None,
        });

        // Ensure correct number of line buffers
        while pb.lines.len() < grid.len() {
            pb.lines.push(LineBuffer {
                buffer: Buffer::new(&mut self.font_system, metrics),
                content_hash: 0,
            });
        }
        pb.lines.truncate(grid.len());

        // Collect background cells (rebuilt every time content changes)
        pb.bg_cells.clear();
        for (row_idx, line) in grid.iter().enumerate() {
            for (col_idx, cell) in line.cells.iter().enumerate() {
                if cell.bg != default_bg {
                    pb.bg_cells.push(BgCell {
                        col: col_idx as u16,
                        row: row_idx as u16,
                        color: [
                            cell.bg.r as f32 / 255.0,
                            cell.bg.g as f32 / 255.0,
                            cell.bg.b as f32 / 255.0,
                            1.0,
                        ],
                    });
                }
            }
        }

        // Store cursor for vertical bar rendering in collect_bg_rects
        let (cursor_col, cursor_row) = cursor_pos;
        if cursor_visible {
            pb.cursor = Some((cursor_col, cursor_row, [
                cursor_color.r as f32 / 255.0,
                cursor_color.g as f32 / 255.0,
                cursor_color.b as f32 / 255.0,
                1.0,
            ]));
        } else {
            pb.cursor = None;
        }

        let default_attrs = Attrs::new().family(Family::Monospace);

        for (row_idx, line) in grid.iter().enumerate() {
            let hash = hash_line(line);
            if hash == pb.lines[row_idx].content_hash {
                continue;
            }
            pb.lines[row_idx].content_hash = hash;

            let (text, spans) = build_line_rich_text(line);
            let rich: Vec<(&str, Attrs)> = spans
                .iter()
                .map(|span| {
                    let slice = &text[span.start..span.end];
                    let mut attrs =
                        default_attrs.color(Color::rgb(span.fg.r, span.fg.g, span.fg.b));
                    if span.bold {
                        attrs = attrs.weight(Weight::BOLD);
                    }
                    if span.italic {
                        attrs = attrs.style(Style::Italic);
                    }
                    (slice, attrs)
                })
                .collect();

            pb.lines[row_idx].buffer.set_rich_text(
                &mut self.font_system,
                rich,
                default_attrs,
                Shaping::Advanced,
            );
            pb.lines[row_idx]
                .buffer
                .shape_until_scroll(&mut self.font_system, false);
        }
    }

    /// Remove a pane's buffers (when the pane is closed).
    pub fn remove_pane(&mut self, pane_id: PaneId) {
        self.pane_buffers.remove(&pane_id);
    }

    /// Prepare all visible panes for rendering.
    pub fn prepare_panes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        panes: &[(PaneId, PixelRect)],
        default_color: RgbColor,
    ) {
        let resolution = Resolution {
            width: self.width,
            height: self.height,
        };
        self.viewport.update(queue, resolution);

        // Set width on each line buffer
        for (pane_id, rect) in panes {
            if let Some(pb) = self.pane_buffers.get_mut(pane_id) {
                for lb in &mut pb.lines {
                    lb.buffer.set_size(
                        &mut self.font_system,
                        Some(rect.w),
                        Some(self.line_height),
                    );
                }
            }
        }

        let default_glyphon_color = Color::rgb(default_color.r, default_color.g, default_color.b);
        let line_h = self.line_height;

        let text_areas: Vec<TextArea<'_>> = panes
            .iter()
            .filter_map(|(pane_id, rect)| {
                let pb = self.pane_buffers.get(pane_id)?;
                Some(
                    pb.lines
                        .iter()
                        .enumerate()
                        .map(move |(idx, lb)| TextArea {
                            buffer: &lb.buffer,
                            left: rect.x,
                            top: rect.y + idx as f32 * line_h,
                            scale: 1.0,
                            bounds: TextBounds {
                                left: rect.x as i32,
                                top: rect.y as i32,
                                right: (rect.x + rect.w) as i32,
                                bottom: (rect.y + rect.h) as i32,
                            },
                            default_color: default_glyphon_color,
                            custom_glyphs: &[],
                        }),
                )
            })
            .flatten()
            .collect();

        let _ = self.glyphon_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self
            .glyphon_renderer
            .render(&self.atlas, &self.viewport, pass);
    }

    pub fn post_render(&mut self) {
        self.atlas.trim();
    }

    /// Collect background rects for all visible panes (physical pixel coords)
    pub fn collect_bg_rects(&self, panes: &[(PaneId, PixelRect)]) -> Vec<crate::bg::BgRect> {
        let cell_w = self.font_size * 0.6;
        let cell_h = self.line_height;
        let cursor_bar_w = 2.0 * self.scale_factor; // 2px logical width vertical bar
        let mut rects = Vec::new();
        for (pane_id, rect) in panes {
            if let Some(pb) = self.pane_buffers.get(pane_id) {
                for bg in &pb.bg_cells {
                    rects.push(crate::bg::BgRect {
                        x: rect.x + bg.col as f32 * cell_w,
                        y: rect.y + bg.row as f32 * cell_h,
                        w: cell_w,
                        h: cell_h,
                        color: bg.color,
                    });
                }
                // Vertical bar cursor (iTerm2 style)
                if let Some((col, row, color)) = pb.cursor {
                    rects.push(crate::bg::BgRect {
                        x: rect.x + col as f32 * cell_w,
                        y: rect.y + row as f32 * cell_h,
                        w: cursor_bar_w,
                        h: cell_h,
                        color,
                    });
                }
            }
        }
        rects
    }

    pub fn cell_size(&self) -> (f32, f32) {
        (self.font_size * 0.6, self.line_height)
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }
}

/// Hash a single terminal line for change detection
fn hash_line(line: &GridLine) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in line.cells.iter() {
        cell.c.hash(&mut hasher);
        cell.fg.r.hash(&mut hasher);
        cell.fg.g.hash(&mut hasher);
        cell.fg.b.hash(&mut hasher);
        cell.bold.hash(&mut hasher);
        cell.italic.hash(&mut hasher);
        cell.wide_spacer.hash(&mut hasher);
    }
    hasher.finish()
}

/// Build rich text spans for a single terminal line
fn build_line_rich_text(
    line: &GridLine,
) -> (String, Vec<RichSpan>) {
    let mut text = String::with_capacity(line.cells.len());
    let mut spans: Vec<RichSpan> = Vec::with_capacity(8);

    let mut cur_fg = RgbColor::new(255, 255, 255);
    let mut cur_bold = false;
    let mut cur_italic = false;
    let mut span_start = 0;

    for (_col, cell) in line.cells.iter().enumerate() {
        // Skip spacer cells for wide (CJK) characters
        if cell.wide_spacer {
            continue;
        }

        let ch = if cell.c == '\0' { ' ' } else { cell.c };
        let fg = cell.fg;
        let bold = cell.bold;
        let italic = cell.italic;

        if fg != cur_fg || bold != cur_bold || italic != cur_italic {
            let cur_pos = text.len();
            if cur_pos > span_start {
                spans.push(RichSpan {
                    start: span_start,
                    end: cur_pos,
                    fg: cur_fg,
                    bold: cur_bold,
                    italic: cur_italic,
                });
            }
            span_start = cur_pos;
            cur_fg = fg;
            cur_bold = bold;
            cur_italic = italic;
        }

        text.push(ch);
    }

    if text.len() > span_start {
        spans.push(RichSpan {
            start: span_start,
            end: text.len(),
            fg: cur_fg,
            bold: cur_bold,
            italic: cur_italic,
        });
    }

    (text, spans)
}
