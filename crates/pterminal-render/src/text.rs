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
    scale_factor: f32,
    font_size: f32,
    line_height: f32,
    content_dirty: bool,
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

    /// Set the text content — only reshapes if content actually changed
    pub fn set_terminal_content(&mut self, text: &str) {
        self.buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
        );
        self.buffer.shape_until_scroll(&mut self.font_system, false);
        self.content_dirty = true;
    }

    /// Prepare and render text
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self.glyphon_renderer.render(&self.atlas, &self.viewport, pass);
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
