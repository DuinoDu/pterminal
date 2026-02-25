/// A colored rectangle to draw as cell background
#[derive(Clone, Copy)]
pub struct BgRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BgVertex {
    position: [f32; 2],
    color: [f32; 4],
}

/// GPU renderer for cell background colors
pub struct BgRenderer {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    num_indices: u32,
    capacity: usize,
    screen_uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_scratch: Vec<BgVertex>,
    last_screen_size: (u32, u32),
}

impl BgRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bg_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bg.wgsl").into()),
        });

        // Screen size uniform
        let screen_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_screen_uniform"),
            size: 8, // 2x f32
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bg_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_uniform.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bg_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BgVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let capacity = 4096; // initial rect capacity (grows on demand)
        let (vertex_buffer, index_buffer) = Self::create_geometry_buffers(device, capacity);

        let mut renderer = Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            num_indices: 0,
            capacity,
            screen_uniform,
            bind_group,
            vertex_scratch: Vec::with_capacity(capacity * 4),
            last_screen_size: (0, 0),
        };
        renderer.upload_index_buffer(queue);
        renderer.update_screen_size(queue, width, height);
        renderer
    }

    fn create_geometry_buffers(
        device: &wgpu::Device,
        capacity: usize,
    ) -> (wgpu::Buffer, wgpu::Buffer) {
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_vertex"),
            size: (capacity * 4 * std::mem::size_of::<BgVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_index"),
            size: (capacity * 6 * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        (vertex_buffer, index_buffer)
    }

    fn update_screen_size(&mut self, queue: &wgpu::Queue, width: u32, height: u32) {
        if self.last_screen_size == (width, height) {
            return;
        }
        self.last_screen_size = (width, height);
        queue.write_buffer(
            &self.screen_uniform,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32]),
        );
    }

    fn upload_index_buffer(&mut self, queue: &wgpu::Queue) {
        let mut indices: Vec<u32> = Vec::with_capacity(self.capacity * 6);
        for i in 0..self.capacity {
            let base = (i * 4) as u32;
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&indices));
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, needed: usize) {
        if needed <= self.capacity {
            return;
        }

        let new_capacity = needed
            .next_power_of_two()
            .max(self.capacity.saturating_mul(2));
        let (vertex_buffer, index_buffer) = Self::create_geometry_buffers(device, new_capacity);
        self.vertex_buffer = vertex_buffer;
        self.index_buffer = index_buffer;
        self.capacity = new_capacity;
        self.upload_index_buffer(queue);
        if self.vertex_scratch.capacity() < new_capacity * 4 {
            let additional = new_capacity * 4 - self.vertex_scratch.capacity();
            self.vertex_scratch.reserve(additional);
        }
    }

    /// Upload background rects to GPU
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rects: &[BgRect],
        screen_w: u32,
        screen_h: u32,
    ) {
        self.update_screen_size(queue, screen_w, screen_h);

        if rects.is_empty() {
            self.num_indices = 0;
            return;
        }

        self.ensure_capacity(device, queue, rects.len());

        self.vertex_scratch.clear();
        if self.vertex_scratch.capacity() < rects.len() * 4 {
            let additional = rects.len() * 4 - self.vertex_scratch.capacity();
            self.vertex_scratch.reserve(additional);
        }

        for rect in rects.iter() {
            let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
            self.vertex_scratch.push(BgVertex {
                position: [x0, y0],
                color: rect.color,
            });
            self.vertex_scratch.push(BgVertex {
                position: [x1, y0],
                color: rect.color,
            });
            self.vertex_scratch.push(BgVertex {
                position: [x1, y1],
                color: rect.color,
            });
            self.vertex_scratch.push(BgVertex {
                position: [x0, y1],
                color: rect.color,
            });
        }

        queue.write_buffer(
            &self.vertex_buffer,
            0,
            bytemuck::cast_slice(&self.vertex_scratch),
        );
        self.num_indices = (rects.len() * 6) as u32;
    }

    /// Render background rects
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.num_indices == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.num_indices, 0, 0..1);
    }
}
