// 仕様 06 §5: フラグメントシェーダーによる 4 近傍伝播。
@group(0) @binding(0) var sources: texture_2d<f32>;
@group(0) @binding(1) var previous: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(corners[vertex], 0.0, 1.0);
}

@fragment
fn fs_prepare(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(sources, vec2<i32>(position.xy), 0);
}

// xHigh: 解像度が 2 倍なので、1 歩が進む距離は半分。1 セルあたりの減衰を合わせるため、減衰は 0.92 の平方根 (約 0.959)。
@fragment
fn fs_propagate_fine(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let maximum = vec2<i32>(textureDimensions(previous)) - vec2<i32>(1);
    let source = textureLoad(sources, p, 0);
    let up = textureLoad(previous, clamp(p + vec2<i32>(0, -1), vec2<i32>(0), maximum), 0).rgb;
    let right = textureLoad(previous, clamp(p + vec2<i32>(1, 0), vec2<i32>(0), maximum), 0).rgb;
    let down = textureLoad(previous, clamp(p + vec2<i32>(0, 1), vec2<i32>(0), maximum), 0).rgb;
    let left = textureLoad(previous, clamp(p + vec2<i32>(-1, 0), vec2<i32>(0), maximum), 0).rgb;
    let nearby = max(max(up, right), max(down, left));
    return vec4<f32>(max(source.rgb, nearby * source.a * 0.959), source.a);
}

@fragment
fn fs_propagate(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let maximum = vec2<i32>(textureDimensions(previous)) - vec2<i32>(1);
    let source = textureLoad(sources, p, 0);
    let up = textureLoad(previous, clamp(p + vec2<i32>(0, -1), vec2<i32>(0), maximum), 0).rgb;
    let right = textureLoad(previous, clamp(p + vec2<i32>(1, 0), vec2<i32>(0), maximum), 0).rgb;
    let down = textureLoad(previous, clamp(p + vec2<i32>(0, 1), vec2<i32>(0), maximum), 0).rgb;
    let left = textureLoad(previous, clamp(p + vec2<i32>(-1, 0), vec2<i32>(0), maximum), 0).rgb;
    let nearby = max(max(up, right), max(down, left));
    return vec4<f32>(max(source.rgb, nearby * source.a * 0.92), source.a);
}
