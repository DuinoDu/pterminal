// Instanced background cell rect shader — one draw call for all cells
struct ScreenUniform {
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: ScreenUniform;

struct InstanceInput {
    @location(0) pos: vec2<f32>,    // x, y pixel coordinates
    @location(1) size: vec2<f32>,   // width, height in pixels
    @location(2) color: vec4<f32>,  // RGBA color
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_idx: u32,
    instance: InstanceInput,
) -> VertexOutput {
    // Generate quad corners from vertex index (6 vertices per quad, 2 triangles)
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_idx];

    // Calculate world position
    let world_pos = instance.pos + corner * instance.size;

    // Convert pixel coords to NDC: x: [0, width] → [-1, 1], y: [0, height] → [1, -1]
    let ndc_x = (world_pos.x / screen.size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (world_pos.y / screen.size.y) * 2.0;

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color;
}
