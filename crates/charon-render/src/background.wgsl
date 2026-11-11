// 仕様 07 §4: 背景の材質 ID だけを使い、前景より暗く表示する。
struct Camera { transform: mat4x4<f32> };
@group(0) @binding(0) var cells: texture_2d_array<u32>;
@group(0) @binding(1) var palette: texture_2d<f32>;
@group(0) @binding(2) var<uniform> camera: Camera;

struct In {
    @builtin(vertex_index) vertex: u32,
    @location(0) chunk_pos: vec2<i32>,
    @location(1) layer: u32,
};
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
};
@vertex
fn vs_main(input: In) -> Out {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let local = corners[input.vertex] * 64.0;
    var out: Out;
    out.position = camera.transform * vec4<f32>(vec2<f32>(input.chunk_pos) * 64.0 + local, 0.0, 1.0);
    out.local = local;
    out.layer = input.layer;
    return out;
}
@fragment
fn fs_main(input: Out) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(clamp(vec2<i32>(floor(input.local)), vec2<i32>(0), vec2<i32>(63)));
    let packed = textureLoad(cells, xy, i32(input.layer), 0);
    let id = packed.r | (packed.g << 8u);
    if id == 0u { discard; }
    let color = textureLoad(palette, vec2<i32>(i32(id), 0), 0);
    return vec4<f32>(color.rgb * 0.45, color.a);
}
