use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer as GlyphonTextRenderer,
    Viewport,
};

use pterminal_core::config::theme::RgbColor;

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
}

impl TextRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let glyphon_renderer =
            GlyphonTextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(device, &cache);

        let mut buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 18.0));
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
    }

    /// Set font metrics (size and line height)
    pub fn set_font_metrics(&mut self, font_size: f32, line_height: f32) {
        self.buffer.set_metrics(
            &mut self.font_system,
            Metrics::new(font_size, line_height),
        );
    }

    /// Set the text content with per-character coloring
    pub fn set_terminal_content(&mut self, lines: &[TerminalLine]) {
        // Build rich text spans for each line
        let mut full_text = String::new();
        let mut attrs_list = Vec::new();

        for (line_idx, line) in lines.iter().enumerate() {
            if line_idx > 0 {
                full_text.push('\n');
            }
            for cell in &line.cells {
                let start = full_text.len();
                full_text.push(cell.c);
                let _end = full_text.len();
                attrs_list.push((start, cell.fg));
            }
        }

        // Simple approach: set text with default attrs, coloring will be handled
        // via per-line rich text in future iterations
        self.buffer.set_text(
            &mut self.font_system,
            &full_text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Prepare and render text
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self.glyphon_renderer.render(&self.atlas, &self.viewport, pass);
    }

    /// Post-render cleanup: trim atlas
    pub fn post_render(&mut self) {
        self.atlas.trim();
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

        let text_areas = [TextArea {
            buffer: &self.buffer,
            left: 4.0,
            top: 4.0,
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

    /// Access font system for metrics calculation
    pub fn font_system(&self) -> &FontSystem {
        &self.font_system
    }

    /// Calculate cell dimensions based on current font
    pub fn cell_size(&mut self) -> (f32, f32) {
        let metrics = self.buffer.metrics();
        // Approximate cell width from font metrics
        let cell_width = metrics.font_size * 0.6; // Monospace approximation
        let cell_height = metrics.line_height;
        (cell_width, cell_height)
    }
}

/// A line of terminal cells for rendering
pub struct TerminalLine {
    pub cells: Vec<TerminalCell>,
}

/// A single terminal cell
pub struct TerminalCell {
    pub c: char,
    pub fg: RgbColor,
    pub bg: RgbColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}
