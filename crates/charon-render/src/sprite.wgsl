// 仕様 06 §9: スプライト。RGBA の画像を、ワールドの 1 点 (足元など) を基準に、拡大率 (1 画素あたりのセル数)・回転・左右反転を
// 掛けた四角形として、画面の解像度で描く。`lit` のとき、剛体と同じように光を掛ける。
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var light: texture_2d<f32>;
@group(0) @binding(3) var light_sampler: sampler;

struct Params {
    // x, y = カメラの左上 (セル)、z, w = シーンの大きさ (セル)。
    camera: vec4<f32>,
    // x, y = 基準点のワールド座標 (セル)、z, w = 基準点の画像上の位置 (画素)。
    place: vec4<f32>,
    // x, y = 画像の大きさ (画素)、z = 拡大率 (1 画素あたりのセル)、w = 左右反転 (1 / 0)。
    shape: vec4<f32>,
    // x = cos、y = sin、z = 環境光、w = 光を掛けるか (1 / 0)。
    turn: vec4<f32>,
    // x, y = 描くコマの左上 (画素)、z, w = 画像全体の大きさ (画素)。コマは `shape.xy` の大きさ。
    frame: vec4<f32>,
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
    // 基準点から見た位置 (画素)。左右反転は、基準点を中心に x を反転する。
    var rel = corner * params.shape.xy - params.place.zw;
    if params.shape.w > 0.5 {
        rel.x = -rel.x;
    }
    rel = rel * params.shape.z;
    let c = params.turn.x;
    let s = params.turn.y;
    let world = params.place.xy + vec2<f32>(c * rel.x - s * rel.y, s * rel.x + c * rel.y);
    let scene_uv = (world - params.camera.xy) / params.camera.zw;
    var out: VertexOut;
    out.position = vec4<f32>(scene_uv.x * 2.0 - 1.0, 1.0 - scene_uv.y * 2.0, 0.0, 1.0);
    out.uv = (params.frame.xy + corner * params.shape.xy) / params.frame.zw;
    out.scene_uv = scene_uv;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(image, image_sampler, input.uv);
    if texel.a <= 0.0 {
        discard;
    }
    var rgb = texel.rgb;
    if params.turn.w > 0.5 {
        let illumination = textureSample(light, light_sampler, input.scene_uv).rgb;
        rgb = rgb * min(vec3<f32>(1.0), vec3<f32>(params.turn.z) + illumination);
    }
    return vec4<f32>(rgb, texel.a);
}
