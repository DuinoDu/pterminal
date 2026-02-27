/// A colored rectangle to draw as cell background
#[derive(Clone, Copy)]
pub struct BgRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

/// Instance data for GPU instanced rendering (one per cell/rect)
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    pos: [f32; 2],   // x, y pixel coordinates
    size: [f32; 2],  // width, height in pixels
    color: [f32; 4], // RGBA color
}

/// Pre-allocated capacity for 65K cells to minimize buffer reallocations
const MAX_CELLS_PER_BATCH: usize = 65_536;

/// GPU renderer for cell background colors using instanced rendering
pub struct BgRenderer {
    pipeline: wgpu::RenderPipeline,
    instance_buffer: wgpu::Buffer,
    num_instances: u32,
    capacity: usize,
    screen_uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instance_scratch: Vec<CellInstance>,
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
            label: Some("bg_instanced_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bg_instanced.wgsl").into()),
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
            immediate_size: 0,
        });

        // Instanced rendering pipeline - no vertex buffer, just instance buffer
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg_instanced_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CellInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        // pos: vec2<f32>
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // size: vec2<f32>
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        // color: vec4<f32>
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
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
            multiview_mask: None,
            cache: None,
        });

        // Pre-allocate 65K cells for high-throughput scenarios
        let capacity = MAX_CELLS_PER_BATCH;
        let instance_buffer = Self::create_instance_buffer(device, capacity);

        let mut renderer = Self {
            pipeline,
            instance_buffer,
            num_instances: 0,
            capacity,
            screen_uniform,
            bind_group,
            instance_scratch: Vec::with_capacity(capacity),
            last_screen_size: (0, 0),
        };
        renderer.update_screen_size(queue, width, height);
        renderer
    }

    fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bg_instance_buffer"),
            size: (capacity * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
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

    fn ensure_capacity(&mut self, device: &wgpu::Device, needed: usize) {
        if needed <= self.capacity {
            return;
        }

        let new_capacity = needed.next_power_of_two().max(self.capacity.saturating_mul(2));
        self.instance_buffer = Self::create_instance_buffer(device, new_capacity);
        self.capacity = new_capacity;
        if self.instance_scratch.capacity() < new_capacity {
            self.instance_scratch.reserve(new_capacity - self.instance_scratch.capacity());
        }
    }

    /// Upload background rects to GPU using instanced rendering
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
            self.num_instances = 0;
            return;
        }

        self.ensure_capacity(device, rects.len());

        self.instance_scratch.clear();
        for rect in rects.iter() {
            self.instance_scratch.push(CellInstance {
                pos: [rect.x, rect.y],
                size: [rect.w, rect.h],
                color: rect.color,
            });
        }

        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&self.instance_scratch),
        );
        self.num_instances = rects.len() as u32;
    }

    /// Render background rects using instanced draw call
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        if self.num_instances == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        // 6 vertices per quad (2 triangles), num_instances quads
        pass.draw(0..6, 0..self.num_instances);
    }
}
