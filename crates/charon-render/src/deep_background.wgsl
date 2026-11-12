// 仕様 09 §9: 背景のさらに奥で、視差で動く虹色の塊。光の影響を受けず、シーンが空の画素にだけ描く。
//
// 描画先は合成後の画面解像度のテクスチャ。シーン (セル解像度、光を掛ける前) を読み、
// 画素が「空」(クリア色) のときだけ塊を描く。前景のセルや背景のセルの後ろに隠れる。
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var nearest: sampler;

struct Params {
    // x, y = カメラの左上 (セル)、z, w = シーンの大きさ (セル)。
    camera: vec4<f32>,
    // x = 視差 (奥ほど小さい)、y = 格子の大きさ、z = 塊のある格子の割合、w = 時間 (tick)。
    shape: vec4<f32>,
    // x = 最小の半径、y = 最大の半径、z = ゆらぎの速さ、w = 明るさ。
    size: vec4<f32>,
    // rgb = シーンの「空」の色、w = 許容誤差。
    empty: vec4<f32>,
    // x = 乱数の種。
    seed: vec4<u32>,
    // x = 動き回る範囲 (格子の大きさに対する割合)。
    extra: vec4<f32>,
};
@group(0) @binding(2) var<uniform> params: Params;

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

// PCG ハッシュ。整数だけで決まるので、フレームやプラットフォームによらない。
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn hash3(cell: vec2<i32>, salt: u32) -> u32 {
    return pcg(pcg(pcg(u32(cell.x) ^ params.seed.x) + u32(cell.y)) + salt);
}

fn unit(h: u32) -> f32 {
    return f32(h & 0xffffu) / 65535.0;
}

// 色相 (0..1、0 = 赤、約 0.8 = 紫) から虹色。
fn rainbow(t: f32) -> vec3<f32> {
    let h = clamp(t, 0.0, 1.0) * 0.8 * 6.0;
    let c = vec3<f32>(
        clamp(abs(h - 3.0) - 1.0, 0.0, 1.0),
        clamp(2.0 - abs(h - 2.0), 0.0, 1.0),
        clamp(2.0 - abs(h - 4.0), 0.0, 1.0),
    );
    return c;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    // シーンのセルの色。空 (クリア色) でなければ、前景か背景のセルが手前にある。
    let cell_color = textureSample(scene, nearest, input.uv).rgb;
    if any(abs(cell_color - params.empty.rgb) > vec3<f32>(params.empty.w)) {
        discard;
    }
    // 画素のセル座標。セルの格子にそろえる (ドット絵のまま)。
    let scene_cell = floor(input.uv * params.camera.zw);
    let parallax = params.shape.x;
    let grid = params.shape.y;
    let time = params.shape.w;
    // カメラが動いたとき、塊は視差の割合だけ動く (奥にあるほどゆっくり)。
    let q = params.camera.xy * parallax + scene_cell;
    let base = vec2<i32>(floor(q / grid));

    var color = vec3<f32>(0.0);
    var alpha = 0.0;
    // 隣の格子の塊が、この画素にかかることがある。
    for (var oy = -1; oy <= 1; oy = oy + 1) {
        for (var ox = -1; ox <= 1; ox = ox + 1) {
            let cell = base + vec2<i32>(ox, oy);
            let present = unit(hash3(cell, 1u));
            if present >= params.shape.z {
                continue;
            }
            let h2 = hash3(cell, 2u);
            let h3 = hash3(cell, 3u);
            let h4 = hash3(cell, 4u);
            let h5 = hash3(cell, 5u);
            let radius = mix(params.size.x, params.size.y, unit(h2));
            // 格子の中に中心の基準を置き、そのまわりを大きく動き回る (周期の違う 2 つの波の組み合わせ)。
            // 動く範囲は格子の大きさに対する割合 `roam` で決まり、隣の格子の塊とも行き来する。
            let base_center = (vec2<f32>(cell) + vec2<f32>(0.2) + vec2<f32>(unit(h3), unit(h4)) * 0.6) * grid;
            let phase = unit(h5) * 6.2831853;
            let speed = params.size.z * (0.6 + 0.8 * unit(h3 ^ h5));
            let roam = params.extra.x * grid;
            let center = base_center + roam * vec2<f32>(
                sin(time * speed + phase) + 0.5 * sin(time * speed * 2.3 + phase * 3.1),
                cos(time * speed * 0.87 + phase * 1.7) + 0.5 * cos(time * speed * 1.9 + phase * 2.3),
            ) * 0.67;
            let d = q - center;
            let dist = length(d);
            let angle = atan2(d.y, d.x);
            // 有機的な形: 角度に応じて半径をゆらす。ゆらぎも時間で動く。
            let wobble = 1.0
                + 0.22 * sin(3.0 * angle + phase + time * params.size.z * 0.5)
                + 0.12 * sin(5.0 * angle - phase * 2.0 + time * params.size.z * 0.7);
            let edge = radius * wobble;
            if dist < edge {
                // 色は塊の中の横位置で決まる (左が赤、右が紫。砂の虹色と同じ並び)。
                let u = clamp((d.x / edge) * 0.5 + 0.5, 0.0, 1.0);
                let shade = 1.0 - 0.3 * (dist / edge) * (dist / edge);
                let coverage = 1.0;
                color = rainbow(u) * shade * params.size.w;
                alpha = coverage;
            }
        }
    }
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(color, alpha);
}
