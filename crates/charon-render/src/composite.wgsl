// 仕様 06 §5: セル解像度のシーンに、線形補間した光を掛ける。
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;
@group(0) @binding(2) var light: texture_2d<f32>;
@group(0) @binding(3) var linear: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let pos = corners[vertex];
    var out: VertexOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = vec2<f32>(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
    return out;
}

@fragment
fn fs_off(input: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(scene, nearest, input.uv);
}

@fragment
fn fs_lit(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(scene, nearest, input.uv);
    let illumination = textureSample(light, linear, input.uv).rgb;
    return vec4<f32>(color.rgb * min(vec3<f32>(1.0), vec3<f32>(0.12) + illumination), color.a);
}
