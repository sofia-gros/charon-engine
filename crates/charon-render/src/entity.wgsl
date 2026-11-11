// 仕様 07 §2.2、§4: セルとは別のプレイヤー描画。材質 ID を参照しない。
struct Camera { origin_view: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
const PLAYER_COLOR: vec4<f32> = vec4<f32>(0.93, 0.66, 0.26, 1.0);

struct In {
    @builtin(vertex_index) vertex: u32,
    @location(0) pos16: vec2<f32>,
    @location(1) size: vec2<f32>,
};
struct Out { @builtin(position) position: vec4<f32> };

@vertex
fn vs_main(input: In) -> Out {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let cell = input.pos16 / 16.0 + corners[input.vertex] * input.size;
    let screen = (cell - camera.origin_view.xy) / camera.origin_view.zw;
    var out: Out;
    out.position = vec4<f32>(screen.x * 2.0 - 1.0, 1.0 - screen.y * 2.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return PLAYER_COLOR;
}
