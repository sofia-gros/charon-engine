// 仕様 06 §5: セル解像度のシーンに、線形補間した光を掛ける。
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;
@group(0) @binding(2) var light: texture_2d<f32>;
@group(0) @binding(3) var linear: sampler;
// x = 環境光。光が届かない場所の明るさの下限 (既定 0.12)。
@group(0) @binding(4) var<uniform> ambient: vec4<f32>;

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

// xHigh: 光を滑らかに補間し (双一次補間の菱形の筋を消す)、光の近くを濃く発色させ、彩度を上げ、白飛びを丸める。
@fragment
fn fs_lit_rich(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(scene, nearest, input.uv);
    let dims = vec2<f32>(textureDimensions(light));
    let p = input.uv * dims - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let smooth_f = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let illumination = textureSample(light, linear, (base + vec2<f32>(0.5) + smooth_f) / dims).rgb;
    var c = color.rgb * (vec3<f32>(ambient.x) + illumination);
    c = c + color.rgb * illumination * illumination * 0.5;
    let luma = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    c = mix(vec3<f32>(luma), c, 1.35);
    let over = max(c - vec3<f32>(0.8), vec3<f32>(0.0));
    c = min(c, vec3<f32>(0.8)) + over * 0.2 / (over + vec3<f32>(0.2));
    return vec4<f32>(max(c, vec3<f32>(0.0)), color.a);
}

@fragment
fn fs_lit(input: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(scene, nearest, input.uv);
    let illumination = textureSample(light, linear, input.uv).rgb;
    return vec4<f32>(color.rgb * min(vec3<f32>(1.0), vec3<f32>(ambient.x) + illumination), color.a);
}
