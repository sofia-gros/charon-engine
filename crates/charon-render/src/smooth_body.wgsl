// 仕様 05 §11: 滑らかな剛体。回転した四角形を画面の解像度で描き、セルの色をバイリニア補間でサンプルする。
//
// テクスチャは、剛体のセルの外側に 1 セルぶんの透明な縁を足したもの (色は隣のセルで埋めてある)。
// 縁は、補間したアルファを smoothstep で整えて、滑らかで鋭い縁にする。
@group(0) @binding(0) var body: texture_2d<f32>;
@group(0) @binding(1) var body_sampler: sampler;
@group(0) @binding(2) var light: texture_2d<f32>;
@group(0) @binding(3) var light_sampler: sampler;

struct Params {
    // x, y = カメラの左上 (セル)、z, w = シーンの大きさ (セル)。
    camera: vec4<f32>,
    // x, y = 重心のワールド座標 (セル)、z, w = 重心の剛体のローカル座標。
    center: vec4<f32>,
    // x = cos、y = sin、z = 幅、w = 高さ (セル)。
    rotation: vec4<f32>,
    // x = 環境光、y = 光を掛けるか (1 / 0)。
    light: vec4<f32>,
};
@group(0) @binding(4) var<uniform> params: Params;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) scene_uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> VertexOut {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex];
    let size = params.rotation.zw;
    // 外側に 1 セルの縁を足した範囲 (ローカル座標)。
    let local = mix(vec2<f32>(-1.0), size + vec2<f32>(1.0), corner);
    let d = local - params.center.zw;
    let c = params.rotation.x;
    let s = params.rotation.y;
    // 剛体のローカル座標 → ワールド座標 (仕様 05 §3 の逆写像の逆)。
    let world = params.center.xy + vec2<f32>(c * d.x - s * d.y, s * d.x + c * d.y);
    let scene_uv = (world - params.camera.xy) / params.camera.zw;
    var out: VertexOut;
    out.position = vec4<f32>(scene_uv.x * 2.0 - 1.0, 1.0 - scene_uv.y * 2.0, 0.0, 1.0);
    out.uv = (local + vec2<f32>(1.0)) / (size + vec2<f32>(2.0));
    out.scene_uv = scene_uv;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(body, body_sampler, input.uv);
    let alpha = smoothstep(0.35, 0.65, texel.a);
    if alpha <= 0.0 {
        discard;
    }
    var rgb = texel.rgb;
    if params.light.y > 0.5 {
        let illumination = textureSample(light, light_sampler, input.scene_uv).rgb;
        rgb = rgb * min(vec3<f32>(1.0), vec3<f32>(params.light.x) + illumination);
    }
    return vec4<f32>(rgb, alpha);
}
