// Draws every file as an instance of one sphere mesh, in a single draw call.
// Per instance: world position, scale, linear color, and emissive strength (in color.a).

import bevy_pbr::render::mesh_view_bindings::view;

struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) position_scale: vec4<f32>,
    @location(4) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    let world = vertex.position * vertex.position_scale.w + vertex.position_scale.xyz;
    var out: VertexOutput;
    out.clip_position = view.clip_from_world * vec4<f32>(world, 1.0);
    // Uniform scale and no rotation: the mesh normal is the world normal.
    out.normal = vertex.normal;
    out.color = vertex.color;
    return out;
}

// Matches the scene's directional light (shining from (1, 2, 1)) and ambient light, as the
// standard material rendered them: ambient + diffuse, plus the emissive glow that blooms.
const LIGHT_DIRECTION: vec3<f32> = vec3<f32>(0.408, 0.816, 0.408);
const AMBIENT: f32 = 0.08;
const DIFFUSE: f32 = 0.95;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let diffuse = max(dot(normalize(in.normal), LIGHT_DIRECTION), 0.0);
    let lit = in.color.rgb * (AMBIENT + DIFFUSE * diffuse + in.color.a);
    return vec4<f32>(lit, 1.0);
}
