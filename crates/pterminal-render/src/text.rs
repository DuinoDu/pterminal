use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer as GlyphonTextRenderer, Viewport,
    Weight,
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
    text_hash: u64,
    bg_hash: u64,
    is_blank: bool,
}

/// Per-pane collection of line buffers + background rects
struct PaneBuffer {
    lines: Vec<LineBuffer>,
    /// Background spans from terminal content (cell-relative coords)
    content_bg_spans: Vec<BgSpan>,
    /// Selection highlight spans (cell-relative coords)
    selection_bg_spans: Vec<BgSpan>,
    /// Cursor position and color for vertical bar rendering
    cursor: Option<(u16, u16, [f32; 4])>, // (col, row, color)
    last_selection: Option<((u16, u16), (u16, u16))>,
    last_selection_bg: RgbColor,
    last_default_bg: RgbColor,
    last_line_layout_key: Option<(u32, u32)>,
    /// Reusable scratch buffers to avoid per-line allocation
    scratch_text: String,
    scratch_spans: Vec<RichSpan>,
}

/// A horizontal run of cells sharing the same background color
struct BgSpan {
    col: u16,
    row: u16,
    width: u16,
    color: [f32; 4],
}

/// Text rendering using glyphon (cosmic-text + wgpu), supporting multiple panes.
/// Uses per-line Buffers so only changed lines are reshaped.
pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    glyphon_renderer: GlyphonTextRenderer,
    /// Separate renderer for overlay text (context menu) — renders after overlay bg
    overlay_renderer: GlyphonTextRenderer,
    viewport: Viewport,
    pane_buffers: HashMap<PaneId, PaneBuffer>,
    width: u32,
    height: u32,
    scale_factor: f32,
    font_size: f32,
    line_height: f32,
    /// Tab bar label buffer (None = no tab bar)
    tab_bar: Option<TabBar>,
    /// Context menu overlay (None = hidden)
    context_menu: Option<ContextMenuOverlay>,
    atlas_trim_frames: u32,
}

/// Tab bar state
struct TabBar {
    /// Per-tab text buffers with their x-offset
    tab_buffers: Vec<(Buffer, f32)>, // (buffer, x_offset)
    height: f32, // physical pixels
    bg_rects: Vec<crate::bg::BgRect>,
    content_hash: u64,
}

/// Context menu overlay
struct ContextMenuOverlay {
    buffer: Buffer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    bg_rects: Vec<crate::bg::BgRect>,
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
        let overlay_renderer =
            GlyphonTextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(device, &cache);

        Self {
            font_system,
            swash_cache,
            atlas,
            glyphon_renderer,
            overlay_renderer,
            viewport,
            pane_buffers: HashMap::new(),
            width,
            height,
            scale_factor: scale,
            font_size: scaled_font_size,
            line_height: scaled_line_height,
            tab_bar: None,
            context_menu: None,
            atlas_trim_frames: 0,
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
            pb.last_line_layout_key = None;
        }
    }

    /// Update a pane's line buffers. Only reshapes lines whose content changed.
    pub fn set_pane_content(
        &mut self,
        pane_id: PaneId,
        grid: &[GridLine],
        dirty_rows: Option<&[usize]>,
        cursor_pos: (u16, u16),
        cursor_visible: bool,
        cursor_color: RgbColor,
        default_bg: RgbColor,
        selection: Option<((u16, u16), (u16, u16))>, // normalized (start, end) or None
        selection_bg: RgbColor,
    ) {
        let metrics = Metrics::new(self.font_size, self.line_height);
        let pb = self
            .pane_buffers
            .entry(pane_id)
            .or_insert_with(|| PaneBuffer {
                lines: Vec::new(),
                content_bg_spans: Vec::new(),
                selection_bg_spans: Vec::new(),
                cursor: None,
                last_selection: None,
                last_selection_bg: RgbColor::new(0, 0, 0),
                last_default_bg: RgbColor::new(0, 0, 0),
                last_line_layout_key: None,
                scratch_text: String::with_capacity(256),
                scratch_spans: Vec::with_capacity(16),
            });

        // Ensure correct number of line buffers
        let line_count_changed = pb.lines.len() != grid.len();
        while pb.lines.len() < grid.len() {
            pb.lines.push(LineBuffer {
                buffer: Buffer::new(&mut self.font_system, metrics),
                text_hash: u64::MAX,
                bg_hash: u64::MAX,
                is_blank: true,
            });
        }
        pb.lines.truncate(grid.len());

        // Store cursor for vertical bar rendering in collect_bg_rects
        let (cursor_col, cursor_row) = cursor_pos;
        if cursor_visible {
            pb.cursor = Some((
                cursor_col,
                cursor_row,
                [
                    cursor_color.r as f32 / 255.0,
                    cursor_color.g as f32 / 255.0,
                    cursor_color.b as f32 / 255.0,
                    1.0,
                ],
            ));
        } else {
            pb.cursor = None;
        }

        let default_attrs = Attrs::new().family(Family::Monospace);
        let bg_full_rebuild = line_count_changed || pb.last_default_bg != default_bg;
        let mut bg_dirty_rows: Vec<usize> = Vec::new();
        let mut any_bg_dirty = bg_full_rebuild;
        if line_count_changed {
            for (row_idx, line) in grid.iter().enumerate() {
                update_line_buffer(
                    &mut self.font_system,
                    pb,
                    row_idx,
                    line,
                    default_attrs,
                    &mut any_bg_dirty,
                    &mut bg_dirty_rows,
                );
            }
        } else if let Some(dirty_rows) = dirty_rows {
            for &row_idx in dirty_rows {
                if let Some(line) = grid.get(row_idx) {
                    update_line_buffer(
                        &mut self.font_system,
                        pb,
                        row_idx,
                        line,
                        default_attrs,
                        &mut any_bg_dirty,
                        &mut bg_dirty_rows,
                    );
                }
            }
        } else {
            for (row_idx, line) in grid.iter().enumerate() {
                update_line_buffer(
                    &mut self.font_system,
                    pb,
                    row_idx,
                    line,
                    default_attrs,
                    &mut any_bg_dirty,
                    &mut bg_dirty_rows,
                );
            }
        }

        if any_bg_dirty {
            if bg_full_rebuild || bg_dirty_rows.len() > grid.len() / 2 {
                // Full rebuild when > half the rows changed or grid resized.
                rebuild_content_bg_spans(&mut pb.content_bg_spans, grid, default_bg);
            } else {
                // Incremental: only update spans for dirty rows.
                incremental_update_bg_spans(
                    &mut pb.content_bg_spans,
                    grid,
                    default_bg,
                    &bg_dirty_rows,
                );
            }
            pb.last_default_bg = default_bg;
        }

        let selection_dirty =
            pb.last_selection != selection || pb.last_selection_bg != selection_bg;
        if selection_dirty {
            rebuild_selection_bg_spans(&mut pb.selection_bg_spans, grid, selection, selection_bg);
            pb.last_selection = selection;
            pb.last_selection_bg = selection_bg;
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
        let no_wrap_slack = (self.font_size * 0.6 * 2.0).max(2.0);

        // Set width on each line buffer only when pane width / line height changed.
        for (pane_id, rect) in panes {
            if let Some(pb) = self.pane_buffers.get_mut(pane_id) {
                let layout_key = Some((rect.w.to_bits(), self.line_height.to_bits()));
                if pb.last_line_layout_key != layout_key {
                    for lb in &mut pb.lines {
                        lb.buffer.set_size(
                            &mut self.font_system,
                            // Add a small slack so terminal rows don't soft-wrap due to
                            // glyph advance rounding differences vs our cell width estimate.
                            Some(rect.w + no_wrap_slack),
                            Some(self.line_height),
                        );
                    }
                    pb.last_line_layout_key = layout_key;
                }
            }
        }

        let default_glyphon_color = Color::rgb(default_color.r, default_color.g, default_color.b);
        let line_h = self.line_height;

        let mut text_areas: Vec<TextArea<'_>> = Vec::new();

        // Tab bar text (per-tab buffers)
        if let Some(ref tb) = self.tab_bar {
            for (buffer, x_offset) in &tb.tab_buffers {
                text_areas.push(TextArea {
                    buffer,
                    left: *x_offset,
                    top: 0.0,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: *x_offset as i32,
                        top: 0,
                        right: self.width as i32,
                        bottom: tb.height as i32,
                    },
                    default_color: default_glyphon_color,
                    custom_glyphs: &[],
                });
            }
        }

        // Pane text
        for (pane_id, rect) in panes {
            if let Some(pb) = self.pane_buffers.get(pane_id) {
                for (idx, lb) in pb.lines.iter().enumerate() {
                    if lb.is_blank {
                        continue;
                    }
                    text_areas.push(TextArea {
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
                    });
                }
            }
        }

        // Pane + tab bar text (NOT context menu — that's in overlay pass)
        let _ = self.glyphon_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );

        // Context menu text — separate prepare for overlay rendering
        let mut overlay_areas: Vec<TextArea<'_>> = Vec::new();
        if let Some(ref cm) = self.context_menu {
            let default_glyphon_color2 =
                Color::rgb(default_color.r, default_color.g, default_color.b);
            overlay_areas.push(TextArea {
                buffer: &cm.buffer,
                left: cm.x,
                top: cm.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: cm.x as i32,
                    top: cm.y as i32,
                    right: (cm.x + cm.w) as i32,
                    bottom: (cm.y + cm.h) as i32,
                },
                default_color: default_glyphon_color2,
                custom_glyphs: &[],
            });
        }
        let _ = self.overlay_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            overlay_areas,
            &mut self.swash_cache,
        );
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self
            .glyphon_renderer
            .render(&self.atlas, &self.viewport, pass);
    }

    /// Render overlay text (context menu) — call after overlay bg
    pub fn render_overlay<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        let _ = self
            .overlay_renderer
            .render(&self.atlas, &self.viewport, pass);
    }

    pub fn post_render(&mut self) {
        self.atlas_trim_frames = self.atlas_trim_frames.wrapping_add(1);
        // Trimming every frame causes avoidable CPU work and glyph churn.
        if self.atlas_trim_frames >= 120 {
            self.atlas.trim();
            self.atlas_trim_frames = 0;
        }
    }

    /// Collect background rects for all visible panes (physical pixel coords)
    pub fn collect_bg_rects(&self, panes: &[(PaneId, PixelRect)]) -> Vec<crate::bg::BgRect> {
        let cell_w = self.font_size * 0.6;
        let cell_h = self.line_height;
        let cursor_bar_w = 2.0 * self.scale_factor;
        let mut total_rects = self.tab_bar.as_ref().map_or(0, |tb| tb.bg_rects.len());
        for (pane_id, _) in panes {
            if let Some(pb) = self.pane_buffers.get(pane_id) {
                total_rects += pb.content_bg_spans.len();
                total_rects += pb.selection_bg_spans.len();
                total_rects += usize::from(pb.cursor.is_some());
            }
        }
        let mut rects = Vec::with_capacity(total_rects);

        // Tab bar bg rects
        if let Some(ref tb) = self.tab_bar {
            rects.extend_from_slice(&tb.bg_rects);
        }
        for (pane_id, rect) in panes {
            if let Some(pb) = self.pane_buffers.get(pane_id) {
                for bg in &pb.content_bg_spans {
                    rects.push(crate::bg::BgRect {
                        x: rect.x + bg.col as f32 * cell_w,
                        y: rect.y + bg.row as f32 * cell_h,
                        w: bg.width as f32 * cell_w,
                        h: cell_h,
                        color: bg.color,
                    });
                }
                for bg in &pb.selection_bg_spans {
                    rects.push(crate::bg::BgRect {
                        x: rect.x + bg.col as f32 * cell_w,
                        y: rect.y + bg.row as f32 * cell_h,
                        w: bg.width as f32 * cell_w,
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

    /// Collect overlay bg rects (context menu) — drawn AFTER text
    pub fn collect_overlay_bg_rects(&self) -> Vec<crate::bg::BgRect> {
        if let Some(ref cm) = self.context_menu {
            cm.bg_rects.clone()
        } else {
            Vec::new()
        }
    }

    pub fn cell_size(&self) -> (f32, f32) {
        (self.font_size * 0.6, self.line_height)
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// Returns tab bar height in physical pixels (0 if no tab bar)
    pub fn tab_bar_height(&self) -> f32 {
        self.tab_bar.as_ref().map_or(0.0, |tb| tb.height)
    }

    /// Update tab bar content. Pass empty slice to hide.
    pub fn set_tab_bar(
        &mut self,
        tabs: &[(String, bool)], // (label, is_active)
        bar_bg: RgbColor,
        active_bg: RgbColor,
        fg: RgbColor,
        active_fg: RgbColor,
    ) {
        if tabs.len() <= 1 {
            self.tab_bar = None;
            return;
        }

        // Hash to skip if unchanged
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (label, active) in tabs {
            label.hash(&mut hasher);
            active.hash(&mut hasher);
        }
        let hash = hasher.finish();

        let tab_font_size = self.font_size * 0.8; // slightly smaller than terminal
        let tab_height = tab_font_size * 1.6;
        let tab_width = self.width as f32 / tabs.len() as f32;

        if let Some(ref tb) = self.tab_bar {
            if tb.content_hash == hash {
                return;
            }
        }

        // Build bg rects for each tab
        let mut bg_rects = Vec::with_capacity(tabs.len() + 1);
        // Full bar background
        bg_rects.push(crate::bg::BgRect {
            x: 0.0,
            y: 0.0,
            w: self.width as f32,
            h: tab_height,
            color: [
                bar_bg.r as f32 / 255.0,
                bar_bg.g as f32 / 255.0,
                bar_bg.b as f32 / 255.0,
                1.0,
            ],
        });
        // Active tab highlight
        for (i, (_label, active)) in tabs.iter().enumerate() {
            if *active {
                bg_rects.push(crate::bg::BgRect {
                    x: i as f32 * tab_width,
                    y: 0.0,
                    w: tab_width,
                    h: tab_height,
                    color: [
                        active_bg.r as f32 / 255.0,
                        active_bg.g as f32 / 255.0,
                        active_bg.b as f32 / 255.0,
                        1.0,
                    ],
                });
            }
        }

        // Build per-tab text buffers, each positioned at its tab region
        // Each tab has a label buffer (left) and a close button buffer (right)
        let metrics = Metrics::new(tab_font_size, tab_height);
        let default_attrs = Attrs::new().family(Family::Monospace);
        let close_btn_w = tab_font_size * 2.0; // width reserved for ✕
        let mut tab_buffers = Vec::with_capacity(tabs.len() * 2);

        for (i, (label, active)) in tabs.iter().enumerate() {
            let x_offset = i as f32 * tab_width;
            let color = if *active { active_fg } else { fg };
            let attrs = default_attrs.color(Color::rgb(color.r, color.g, color.b));

            // Tab label (left-aligned)
            let mut label_buf = Buffer::new(&mut self.font_system, metrics);
            label_buf.set_size(
                &mut self.font_system,
                Some(tab_width - close_btn_w),
                Some(tab_height),
            );
            let label_text = format!("  {}", label);
            label_buf.set_rich_text(
                &mut self.font_system,
                [(&label_text as &str, attrs)],
                default_attrs,
                Shaping::Advanced,
            );
            label_buf.shape_until_scroll(&mut self.font_system, false);
            tab_buffers.push((label_buf, x_offset));

            // Close button ✕ (right side of tab, larger font)
            let close_font_size = tab_font_size * 1.3;
            let close_metrics = Metrics::new(close_font_size, tab_height);
            let mut close_buf = Buffer::new(&mut self.font_system, close_metrics);
            close_buf.set_size(&mut self.font_system, Some(close_btn_w), Some(tab_height));
            let dim_color = default_attrs.color(Color::rgb(fg.r, fg.g, fg.b));
            close_buf.set_rich_text(
                &mut self.font_system,
                [(" ✕", dim_color)],
                default_attrs,
                Shaping::Advanced,
            );
            close_buf.shape_until_scroll(&mut self.font_system, false);
            tab_buffers.push((close_buf, x_offset + tab_width - close_btn_w));
        }

        self.tab_bar = Some(TabBar {
            tab_buffers,
            height: tab_height,
            bg_rects,
            content_hash: hash,
        });
    }

    /// Show context menu at given position with given items
    pub fn set_context_menu(
        &mut self,
        x: f32,
        y: f32,
        items: &[(&str, bool)], // (label, enabled)
    ) {
        let scale = self.scale_factor;
        let item_h = 30.0 * scale;
        let menu_w = 160.0 * scale;
        let menu_h = items.len() as f32 * item_h + 4.0 * scale;
        let pad = 6.0 * scale;
        let font_size = self.font_size * 0.85;
        let border = 1.0 * scale;

        // Clamp to screen
        let mx = x.min(self.width as f32 - menu_w - pad);
        let my = y.min(self.height as f32 - menu_h - pad);

        let mut bg_rects = Vec::new();
        // Shadow (offset slightly)
        bg_rects.push(crate::bg::BgRect {
            x: mx + 2.0 * scale,
            y: my + 2.0 * scale,
            w: menu_w + border * 2.0,
            h: menu_h + border * 2.0,
            color: [0.0, 0.0, 0.0, 0.5],
        });
        // Border
        bg_rects.push(crate::bg::BgRect {
            x: mx - border,
            y: my - border,
            w: menu_w + border * 2.0,
            h: menu_h + border * 2.0,
            color: [0.55, 0.55, 0.58, 1.0],
        });
        // Solid opaque background — intentionally bright enough to stand out
        bg_rects.push(crate::bg::BgRect {
            x: mx,
            y: my,
            w: menu_w,
            h: menu_h,
            color: [0.22, 0.22, 0.26, 1.0],
        });
        // Per-item background strips for visual separation
        let y_pad = 2.0 * scale;
        for i in 0..items.len() {
            bg_rects.push(crate::bg::BgRect {
                x: mx + 2.0 * scale,
                y: my + y_pad + i as f32 * item_h,
                w: menu_w - 4.0 * scale,
                h: item_h,
                color: [0.28, 0.28, 0.32, 1.0],
            });
        }

        // Text buffer
        let metrics = Metrics::new(font_size, item_h);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(menu_w), Some(menu_h));

        let mut text = String::new();
        let mut spans = Vec::new();
        for (i, (label, _enabled)) in items.iter().enumerate() {
            let start = text.len();
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&format!("  {}", label));
            spans.push((start, text.len()));
        }

        let default_attrs = Attrs::new().family(Family::Monospace);
        let fg_color = Color::rgb(0xee, 0xee, 0xee);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|(s, e)| (&text[*s..*e], default_attrs.color(fg_color)))
            .collect();
        buffer.set_rich_text(
            &mut self.font_system,
            rich,
            default_attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        self.context_menu = Some(ContextMenuOverlay {
            buffer,
            x: mx,
            y: my + y_pad,
            w: menu_w,
            h: menu_h,
            bg_rects,
        });
    }

    /// Hide context menu
    pub fn clear_context_menu(&mut self) {
        self.context_menu = None;
    }
}

fn update_line_buffer(
    font_system: &mut FontSystem,
    pb: &mut PaneBuffer,
    row_idx: usize,
    line: &GridLine,
    default_attrs: Attrs<'static>,
    any_bg_dirty: &mut bool,
    bg_dirty_rows: &mut Vec<usize>,
) {
    let lb = &mut pb.lines[row_idx];

    // Single pass over cells for both text and bg hashes.
    let (text_hash, bg_hash) = hash_line_combined(line);
    if bg_hash != lb.bg_hash {
        lb.bg_hash = bg_hash;
        *any_bg_dirty = true;
        bg_dirty_rows.push(row_idx);
    }

    if text_hash == lb.text_hash {
        return;
    }
    lb.text_hash = text_hash;

    // Reuse pane-level scratch buffers to avoid per-line allocation.
    // build_line_rich_text_into also reports blank/ASCII status to avoid extra passes.
    let text = &mut pb.scratch_text;
    let spans = &mut pb.scratch_spans;
    let line_info = build_line_rich_text_into(line, text, spans);

    if line_info.is_blank {
        pb.lines[row_idx].is_blank = true;
        return;
    }

    let shaping = if line_info.all_ascii {
        Shaping::Basic
    } else {
        Shaping::Advanced
    };

    // Re-borrow lb after scratch buffer usage (scratch lives on pb).
    let lb = &mut pb.lines[row_idx];
    lb.is_blank = false;
    if spans.len() == 1 {
        let span = &spans[0];
        let mut attrs = default_attrs.color(Color::rgb(span.fg.r, span.fg.g, span.fg.b));
        if span.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if span.italic {
            attrs = attrs.style(Style::Italic);
        }
        let slice = &text[span.start..span.end];
        lb.buffer
            .set_rich_text(font_system, [(slice, attrs)], default_attrs, shaping);
    } else {
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|span| {
                let slice = &text[span.start..span.end];
                let mut attrs = default_attrs.color(Color::rgb(span.fg.r, span.fg.g, span.fg.b));
                if span.bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                if span.italic {
                    attrs = attrs.style(Style::Italic);
                }
                (slice, attrs)
            })
            .collect();
        lb.buffer
            .set_rich_text(font_system, rich, default_attrs, shaping);
    }
    lb.buffer.shape_until_scroll(font_system, false);
}

fn rgb_to_rgba(color: RgbColor) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        1.0,
    ]
}

fn rebuild_content_bg_spans(out: &mut Vec<BgSpan>, grid: &[GridLine], default_bg: RgbColor) {
    out.clear();
    for (row_idx, line) in grid.iter().enumerate() {
        emit_bg_spans_for_row(out, line, row_idx, default_bg);
    }
}

/// Incrementally update bg spans for a subset of dirty rows.
fn incremental_update_bg_spans(
    out: &mut Vec<BgSpan>,
    grid: &[GridLine],
    default_bg: RgbColor,
    dirty_rows: &[usize],
) {
    // Remove old spans for dirty rows.
    out.retain(|span| !dirty_rows.contains(&(span.row as usize)));
    // Add new spans for dirty rows.
    for &row_idx in dirty_rows {
        if let Some(line) = grid.get(row_idx) {
            emit_bg_spans_for_row(out, line, row_idx, default_bg);
        }
    }
}

fn emit_bg_spans_for_row(out: &mut Vec<BgSpan>, line: &GridLine, row_idx: usize, default_bg: RgbColor) {
    let mut col = 0usize;
    while col < line.cells.len() {
        let cell_bg = line.cells[col].bg;
        if cell_bg == default_bg {
            col += 1;
            continue;
        }

        let mut end = col + 1;
        while end < line.cells.len() && line.cells[end].bg == cell_bg {
            end += 1;
        }

        out.push(BgSpan {
            col: col as u16,
            row: row_idx as u16,
            width: (end - col) as u16,
            color: rgb_to_rgba(cell_bg),
        });
        col = end;
    }
}

fn rebuild_selection_bg_spans(
    out: &mut Vec<BgSpan>,
    grid: &[GridLine],
    selection: Option<((u16, u16), (u16, u16))>,
    selection_bg: RgbColor,
) {
    out.clear();
    let Some((start, end)) = selection else {
        return;
    };

    let color = rgb_to_rgba(selection_bg);
    for row in start.1..=end.1 {
        let Some(line) = grid.get(row as usize) else {
            break;
        };

        let col_start = if row == start.1 { start.0 } else { 0 };
        let col_end = if row == end.1 {
            end.0.saturating_add(1)
        } else {
            line.cells.len() as u16
        };
        if col_end <= col_start {
            continue;
        }

        let clamped_end = col_end.min(line.cells.len() as u16);
        if clamped_end <= col_start {
            continue;
        }

        out.push(BgSpan {
            col: col_start,
            row,
            width: clamped_end - col_start,
            color,
        });
    }
}

/// Compute text hash and bg hash in a single pass over cells.
fn hash_line_combined(line: &GridLine) -> (u64, u64) {
    use std::hash::{Hash, Hasher};
    let mut text_h = ahash::AHasher::default();
    let mut bg_h = ahash::AHasher::default();
    line.cells.len().hash(&mut bg_h);
    for cell in line.cells.iter() {
        cell.c.hash(&mut text_h);
        cell.fg.r.hash(&mut text_h);
        cell.fg.g.hash(&mut text_h);
        cell.fg.b.hash(&mut text_h);
        cell.bold.hash(&mut text_h);
        cell.italic.hash(&mut text_h);
        cell.wide_spacer.hash(&mut text_h);
        cell.bg.r.hash(&mut bg_h);
        cell.bg.g.hash(&mut bg_h);
        cell.bg.b.hash(&mut bg_h);
    }
    (text_h.finish(), bg_h.finish())
}

/// Info produced by build_line_rich_text_into alongside the text/spans.
struct LineInfo {
    is_blank: bool,
    all_ascii: bool,
}

/// Build rich text spans into caller-provided scratch buffers.
/// Also detects blank lines and ASCII-only content in the same pass
/// (replaces separate line_is_visually_blank and line_is_basic_shaping_friendly calls).
fn build_line_rich_text_into(
    line: &GridLine,
    text: &mut String,
    spans: &mut Vec<RichSpan>,
) -> LineInfo {
    text.clear();
    spans.clear();

    let mut cur_fg = RgbColor::new(255, 255, 255);
    let mut cur_bold = false;
    let mut cur_italic = false;
    let mut span_start = 0;
    let mut all_ascii = true;
    let mut is_blank = true;

    for cell in line.cells.iter() {
        if cell.wide_spacer {
            continue;
        }

        let ch = if cell.c == '\0' { ' ' } else { cell.c };

        if is_blank && ch != ' ' {
            is_blank = false;
        }
        if all_ascii && !ch.is_ascii() {
            all_ascii = false;
        }

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

    LineInfo { is_blank, all_ascii }
}
