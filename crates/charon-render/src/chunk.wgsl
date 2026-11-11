// 仕様 06 §3: 64×64 のチャンクを 1 インスタンスとして描画する。
struct Camera {
    transform: mat4x4<f32>,
};

@group(0) @binding(0) var cells: texture_2d_array<u32>;
@group(0) @binding(1) var palette: texture_2d<f32>;
@group(0) @binding(2) var palette_aux: texture_2d<f32>;
@group(0) @binding(3) var<uniform> camera: Camera;

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
    let texel_pos = vec2<i32>(clamp(vec2<i32>(floor(input.local)), vec2<i32>(0), vec2<i32>(63)));
    let cell = textureLoad(cells, texel_pos, i32(input.layer), 0);
    let id = cell.r | (cell.g << 8u);
    if id == 0u {
        discard;
    }
    let base = textureLoad(palette, vec2<i32>(i32(id), 0), 0);
    let aux = textureLoad(palette_aux, vec2<i32>(i32(id), 0), 0);
    let variation = 1.0 + (f32(cell.b) / 255.0 - 0.5) * aux.a * 8.0;
    var color = base.rgb * variation;
    if (cell.a & 1u) != 0u {
        color += vec3<f32>(0.32, 0.11, 0.0);
    }
    let heat = f32(cell.a >> 1u) / 127.0;
    color += vec3<f32>(heat * 0.18, heat * 0.025, 0.0);
    return vec4<f32>(color, base.a);
}
