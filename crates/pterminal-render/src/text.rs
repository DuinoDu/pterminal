use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer as GlyphonTextRenderer,
    Viewport, Weight,
};

use pterminal_core::config::theme::RgbColor;
use pterminal_core::terminal::GridLine;

/// A colored span of text for rich-text rendering
struct RichSpan {
    text: String,
    fg: RgbColor,
    bold: bool,
    italic: bool,
}

/// Text rendering using glyphon (cosmic-text + wgpu)
pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    glyphon_renderer: GlyphonTextRenderer,
    viewport: Viewport,
    buffer: Buffer,
    width: u32,
    height: u32,
    scale_factor: f32,
    font_size: f32,
    line_height: f32,
    content_dirty: bool,
    /// Cached content hash for change detection
    content_hash: u64,
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
        let scaled_line_height = (font_size * 1.4) * scale;

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let glyphon_renderer =
            GlyphonTextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(device, &cache);

        let mut buffer = Buffer::new(
            &mut font_system,
            Metrics::new(scaled_font_size, scaled_line_height),
        );
        buffer.set_size(&mut font_system, Some(width as f32), Some(height as f32));

        Self {
            font_system,
            swash_cache,
            atlas,
            glyphon_renderer,
            viewport,
            buffer,
            width,
            height,
            scale_factor: scale,
            font_size: scaled_font_size,
            line_height: scaled_line_height,
            content_dirty: true,
            content_hash: 0,
        }
    }

    pub fn resize(&mut self, _queue: &wgpu::Queue, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.buffer.set_size(
            &mut self.font_system,
            Some(width as f32),
            Some(height as f32),
        );
        self.content_dirty = true;
    }

    pub fn update_scale_factor(&mut self, scale_factor: f64, font_size: f32) {
        let scale = scale_factor as f32;
        self.scale_factor = scale;
        self.font_size = font_size * scale;
        self.line_height = (font_size * 1.4) * scale;
        self.buffer.set_metrics(
            &mut self.font_system,
            Metrics::new(self.font_size, self.line_height),
        );
        self.content_dirty = true;
    }

    /// Set terminal content with per-cell ANSI colors.
    /// Builds rich text spans grouping consecutive cells with same attributes.
    pub fn set_terminal_content(
        &mut self,
        grid: &[GridLine],
        cursor_pos: (u16, u16),
        cursor_visible: bool,
        cursor_color: RgbColor,
    ) {
        // Build spans: group consecutive cells with same (fg, bold, italic)
        let spans = build_rich_spans(grid, cursor_pos, cursor_visible, cursor_color);

        // Compute a simple hash for change detection
        let hash = hash_spans(&spans);
        if hash == self.content_hash && !self.content_dirty {
            return;
        }
        self.content_hash = hash;

        // Build (text_slice, Attrs) pairs for set_rich_text
        let default_attrs = Attrs::new().family(Family::Monospace);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|span| {
                let mut attrs = default_attrs
                    .color(Color::rgb(span.fg.r, span.fg.g, span.fg.b));
                if span.bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                if span.italic {
                    attrs = attrs.style(Style::Italic);
                }
                (span.text.as_str(), attrs)
            })
            .collect();

        self.buffer.set_rich_text(
            &mut self.font_system,
            rich,
            default_attrs,
            Shaping::Advanced,
        );
        self.buffer
            .shape_until_scroll(&mut self.font_system, false);
        self.content_dirty = true;
    }

    /// Prepare and render text
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self
            .glyphon_renderer
            .render(&self.atlas, &self.viewport, pass);
    }

    /// Post-render cleanup: trim atlas
    pub fn post_render(&mut self) {
        self.atlas.trim();
        self.content_dirty = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.content_dirty
    }

    /// Prepare text for rendering (call before render_frame's render pass)
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        default_color: RgbColor,
    ) {
        let resolution = Resolution {
            width: self.width,
            height: self.height,
        };
        self.viewport.update(queue, resolution);

        let padding = 6.0 * self.scale_factor;

        let text_areas = [TextArea {
            buffer: &self.buffer,
            left: padding,
            top: padding,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: self.width as i32,
                bottom: self.height as i32,
            },
            default_color: Color::rgb(default_color.r, default_color.g, default_color.b),
            custom_glyphs: &[],
        }];

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

    /// Cell dimensions in physical pixels
    pub fn cell_size(&self) -> (f32, f32) {
        (self.font_size * 0.6, self.line_height)
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }
}

/// Build rich text spans from grid, grouping consecutive cells with same attributes.
fn build_rich_spans(
    grid: &[GridLine],
    cursor_pos: (u16, u16),
    cursor_visible: bool,
    cursor_color: RgbColor,
) -> Vec<RichSpan> {
    let (cursor_col, cursor_row) = cursor_pos;
    let mut spans: Vec<RichSpan> = Vec::with_capacity(grid.len() * 4);

    for (row_idx, line) in grid.iter().enumerate() {
        if row_idx > 0 {
            // Newline gets default color from previous span or new span
            if let Some(last) = spans.last_mut() {
                last.text.push('\n');
            } else {
                spans.push(RichSpan {
                    text: "\n".to_string(),
                    fg: RgbColor::new(255, 255, 255),
                    bold: false,
                    italic: false,
                });
            }
        }

        for (col_idx, cell) in line.cells.iter().enumerate() {
            let is_cursor =
                cursor_visible && row_idx == cursor_row as usize && col_idx == cursor_col as usize;
            let ch = if is_cursor {
                '█'
            } else {
                let c = cell.c;
                if c == '\0' { ' ' } else { c }
            };
            let fg = if is_cursor { cursor_color } else { cell.fg };
            let bold = if is_cursor { false } else { cell.bold };
            let italic = if is_cursor { false } else { cell.italic };

            // Try to extend the last span if attributes match
            if let Some(last) = spans.last_mut() {
                if last.fg == fg && last.bold == bold && last.italic == italic {
                    last.text.push(ch);
                    continue;
                }
            }

            // New span
            spans.push(RichSpan {
                text: String::from(ch),
                fg,
                bold,
                italic,
            });
        }
    }

    spans
}

/// Simple hash of spans for change detection (avoids full string comparison)
fn hash_spans(spans: &[RichSpan]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for span in spans {
        span.text.hash(&mut hasher);
        span.fg.r.hash(&mut hasher);
        span.fg.g.hash(&mut hasher);
        span.fg.b.hash(&mut hasher);
        span.bold.hash(&mut hasher);
        span.italic.hash(&mut hasher);
    }
    hasher.finish()
}
