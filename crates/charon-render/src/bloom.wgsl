// 仕様 06 §4、§5: 合成済みの画面に明部のにじみを足す後処理の例。
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;

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

fn bright(uv: vec2<f32>) -> vec3<f32> {
    return max(textureSample(source, nearest, uv).rgb - vec3<f32>(0.65), vec3<f32>(0.0));
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let center = textureSample(source, nearest, input.uv);
    let texel = 2.0 / vec2<f32>(textureDimensions(source));
    let nearby = bright(input.uv + vec2<f32>(texel.x, 0.0))
        + bright(input.uv + vec2<f32>(-texel.x, 0.0))
        + bright(input.uv + vec2<f32>(0.0, texel.y))
        + bright(input.uv + vec2<f32>(0.0, -texel.y));
    return vec4<f32>(min(center.rgb + nearby * 0.18, vec3<f32>(1.0)), center.a);
}
