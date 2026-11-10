# 仕様 06: 描画とプラットフォーム

対象クレート: `charon-render`、`charon-app`、`apps/sandbox` (マイルストーン M4、M7)。

## 1. 対応プラットフォーム

| プラットフォーム | GPU バックエンド | 並列化 | 備考 |
|---|---|---|---|
| Windows | DX12 / Vulkan | rayon | |
| Linux | Vulkan (なければ GLES 3) | rayon | |
| macOS | Metal | rayon | |
| Android | Vulkan (なければ GLES 3) | rayon | API レベル 26 以上、arm64-v8a |
| Web | WebGPU (なければ WebGL2) | なし (1 スレッド) | M8 以降。ただし核は M1 から wasm でコンパイルできる状態を保つ |

バックエンドの選択は wgpu に任せる (`Backends::all()`)。自分で分岐を書かない。

## 2. 設計の規則

これらは「どこでも動く・速い・後から足せる」ための規則であり、破ってはいけない。

1. **シェーダーは WGSL だけ。** GLSL や HLSL を書かない。
2. **コンピュートシェーダーを必須にしない。** GLES 3.0 と WebGL2 にはコンピュートシェーダーがない。
   ライティングを含むすべての GPU 処理は、頂点 + フラグメントシェーダーだけで実装する。
3. **wgpu の `Features` は何も要求しない** (`Features::empty()`)。`Limits` は
   `Limits::downlevel_webgl2_defaults()` を基準にし、テクスチャの最大サイズだけアダプターの値を使う。
4. **`charon-core` は描画に依存しない。** 逆に `charon-render` は `charon-core` の公開 API だけを読む。
5. **シミュレーションと描画の速度を分ける** (§6)。
6. **GPU へ送るのは変わった部分だけ** (§3)。
7. 色空間: サーフェスは sRGB 形式を選ぶ。パレットのテクスチャは `Rgba8UnormSrgb`。

依存クレート (`wgpu`、`winit`、`egui` など) は、導入時点の最新安定版を `cargo add` で入れ、
`Cargo.lock` をコミットする。wgpu と egui-wgpu はバージョンの組み合わせに制約があるので、
egui-wgpu が対応している wgpu のバージョンに合わせる。

## 3. セルの描画

### データの流れ

```
Chunk::cells (CPU) ──変わった矩形だけ──▶ セルのテクスチャ配列 (GPU)
                                              │
MaterialTable ──起動時に 1 回──▶ パレットのテクスチャ      │
                                              ▼
                                   chunk.wgsl が色を決めて描く
```

### テクスチャ

| 名前 | 形式 | 大きさ | 内容 |
|---|---|---|---|
| セル配列 | `Rgba8Uint`、2D 配列 | 64×64、256 層 | 1 層 = 1 チャンク。R = 材質 ID の下位 8 ビット、G = 上位 8 ビット、B = `shade`、A = ビット 0 が `FLAG_BURNING`、ビット 1〜7 が温度の表示値 `clamp(temp / 16, 0, 127)` |
| パレット | `Rgba8UnormSrgb` | 幅 = 材質数 (最大 2048)、高さ 1 | 材質の `color` |
| パレット補助 | `Rgba8Unorm` | 同上 | RGB = `emission`、A = `color_variation` |

層の割り当て: `HashMap<ChunkPos, u32>` と空き層のスタックで管理する。画面から 2 チャンク以上離れた
チャンクの層は解放する。256 層で足りない画面サイズ (拡大率 1 で 4K など) の場合は、
起動時にアダプターの上限まで層数を増やす。

### 更新

毎フレーム、画面内の各チャンクについて `render_dirty.take()` を見る。`Some(rect)` なら、
その矩形のセルを 4 バイトに変換し、`queue.write_texture` で該当する層の該当する矩形にだけ書く。
新しく層を割り当てたチャンクは全体 (64×64) を書く。

### 描画

1 回のインスタンス描画で全チャンクを描く。インスタンスごとのデータは
`chunk_pos: vec2<i32>` と `layer: u32`。頂点シェーダーが四角形を作り、カメラ行列で変換する。
フラグメントシェーダー (`chunk.wgsl`) は次を行う。

```
cell = textureLoad(cells, 整数座標, layer, 0)
id = cell.r | (cell.g << 8)
if id == 0: discard                       # 空は背景を透かす
base = textureLoad(palette, (id, 0), 0)
aux  = textureLoad(palette_aux, (id, 0), 0)
明るさ = 1.0 + (cell.b / 255.0 - 0.5) * aux.a * 2.0 / 255.0 * 4.0
color = base.rgb * 明るさ
燃焼中なら橙色を加える。温度の表示値が高ければ赤熱の色を加える
```

拡大してもぼやけないよう、`textureLoad` (補間なし) を使う。`textureSample` は使わない。

## 4. 描画パスの構成 (拡張のしかた)

1 フレームは、順番に実行する「パス」のリストである。

```rust
pub trait RenderPass {
    fn name(&self) -> &str;
    fn resize(&mut self, ctx: &GpuContext, width: u32, height: u32);
    fn render(&mut self, ctx: &GpuContext, frame: &mut FrameContext);
}
```

| 順 | パス | 出力 | 追加時期 |
|---|---|---|---|
| 1 | 背景 | シーンのテクスチャ | M4 |
| 2 | セル (§3) | シーンのテクスチャ | M4 |
| 3 | パーティクル・エンティティ | シーンのテクスチャ | M6 |
| 4 | ライティング (§5) | ライトのテクスチャ | M7 |
| 5 | 合成 (シーン × ライト) | 後処理用のテクスチャ | M7 |
| 6 | 後処理 (0 個以上) | 画面 | M7 |
| 7 | デバッグ UI (egui) | 画面 | M4 |

新しい効果は、`RenderPass` を実装してリストに挿入するだけで足せる。既存のパスを書き換えない。
後処理は「入力テクスチャ 1 枚 → 全画面の三角形 1 枚 → 出力」という共通の形にし、
WGSL のフラグメントシェーダーのファイルを 1 つ足せば追加できるようにする
(`FullscreenPass::new(ctx, include_str!("bloom.wgsl"))`)。

シーンのテクスチャは、画面の解像度ではなく **セルの解像度** (画面 ÷ 拡大率) で持つ。
塗る面積が減り、モバイルで速い。最後の合成で画面の解像度に拡大する。

## 5. ライティング (M7)

コンピュートシェーダーを使わず、フラグメントシェーダーの往復 (ピンポン) で光を広げる。

1. **準備パス**: セルの解像度の 1/4 のテクスチャ 2 枚 (`Rgba16Float`、使えなければ `Rgba8Unorm`) を用意する。
   1 枚目に「光源」(材質の `emission`、燃焼中のセル、空が見える場所の空の色) を、
   A チャンネルに「遮り具合」(固体・粉体 = 0.6、液体 = 0.9、空・気体 = 1.0) を書く。
2. **伝播パス**: `out = max(自分の光源, 上下左右 4 点の光の最大値 × 自分の遮り具合 × 0.92)` を、
   2 枚を入れ替えながら N 回繰り返す。N はデスクトップ 16、モバイル 8 (設定で変えられる)。
3. **合成パス**: シーンの色 × (環境光 + ライト)。ライトは線形補間 (`textureSample`) で拡大する。

品質の設定 (`LightQuality { Off, Low, High }`) を持ち、`Off` では準備・伝播を飛ばして
ライト = 1.0 として合成する。性能が足りない端末の逃げ道である。

## 6. メインループ

シミュレーションは **固定 60 tick 毎秒**。描画は可能な限り (垂直同期)。

```
accumulator += 前フレームからの経過時間 (上限 0.1 秒)
steps = 0
while accumulator >= 1/60 かつ steps < 3:
    world.step(); accumulator -= 1/60; steps += 1
if steps == 3: accumulator = 0          # 追いつけないときは遅れを捨てる (スローになるが固まらない)
描画
```

シミュレーションする範囲は「カメラの中心から半径 R チャンク」に限る。R は設定値で、
デスクトップの既定は画面 + 2 チャンク、モバイルの既定は画面 + 1 チャンク。
範囲外のチャンクは `step` の対象から外す (M5 で読み込み・退避と統合する)。

## 7. アプリの構造と Android

```
crates/charon-app   ライブラリ。ウィンドウ、入力、メインループ、描画の初期化。全プラットフォーム共通
apps/sandbox        実行ファイルと Android 用ライブラリ。charon-app を呼ぶだけ
```

`apps/sandbox/Cargo.toml` は `[lib] crate-type = ["cdylib", "rlib"]` と `[[bin]]` の両方を持つ。
デスクトップは `main.rs` の `fn main()`、Android は `lib.rs` の
`#[unsafe(no_mangle)] fn android_main(app: AndroidApp)` から、同じ `charon_app::run(...)` を呼ぶ。
`android_main` のためだけに `unsafe` 属性が必要になる。`apps/sandbox` でこの 1 箇所に限り許可する。

ウィンドウは winit を使う (Android は `android-native-activity` feature)。

**Android で必ず守ること** (守らないと復帰時にクラッシュする):

1. winit の `resumed` イベントが来るまで、ウィンドウもサーフェスも作らない。
2. `suspended` イベントでサーフェスを破棄する (`Device` と `Queue`、テクスチャは保持する)。
3. 次の `resumed` で、新しいウィンドウからサーフェスを作り直す。
4. `suspended` の間は `world.step()` も描画も呼ばない。復帰時に `accumulator = 0` にする。
5. アプリが止められる前に保存できるよう、`suspended` で保存処理を呼ぶ (M5 以降)。

この手順はデスクトップでも同じコードで動く (デスクトップでも `resumed` は起動時に 1 回来る)。
**プラットフォームごとの分岐を作らず、全プラットフォームでこの流れにする。**

### 入力

入力は 1 つの型に正規化してからゲームに渡す。マウスとタッチを別々に扱うコードをゲーム側に書かない。

```rust
pub enum PointerEvent {
    Down { id: u64, pos: [f32; 2] },
    Move { id: u64, pos: [f32; 2] },
    Up   { id: u64, pos: [f32; 2] },
}
```

マウスの左ボタンは `id = 0` のポインター。タッチは指ごとの `id`。2 本指の距離の変化は拡大縮小、
2 本指の移動はカメラ移動としてサンドボックスが解釈する。

### ビルド

Android のビルドには `cargo-apk` か `xbuild` を使う。導入時にどちらが保守されているかを確認して選び、
選んだ理由と手順を `docs/BUILD_ANDROID.md` に書く。Android NDK が必要である。

CI では M0 から次を確認している (NDK 不要):

```bash
cargo check -p charon-core --target aarch64-linux-android
```

```bash
cargo check -p charon-core --target wasm32-unknown-unknown --no-default-features
```

M4 以降は、`charon-render` と `charon-app` にも Android の `cargo check` を追加する
(こちらは NDK が必要なので、CI に NDK のセットアップ手順を足す)。

## 8. サンドボックスの機能 (M4)

| 機能 | 内容 |
|---|---|
| 材質の選択 | egui の一覧。`MaterialTable` から自動で作る |
| ブラシ | 円形。大きさを変えられる。ポインターを押している間、`make_cell` で置く |
| 消しゴム | `EMPTY` を置く |
| 一時停止・1 tick 送り | |
| カメラ | 移動と拡大縮小 (整数倍) |
| デバッグ表示 | チャンクの境界、`dirty` の矩形、tick にかかった時間、動いているチャンクの数 |
| 状態の保存 | seed と操作の記録を書き出し、同じ結果を再生できる (決定性の確認) |

## 9. 必須テスト

GPU が必要なテストは CI で動かせないことがあるため、GPU を使わない部分を分けてテストする。

| テスト | 内容 |
|---|---|
| `cell_to_texel` | `Cell` から 4 バイトへの変換が §3 の表どおり |
| `layer_allocator` | 割り当て・解放・再利用が正しく、同じ層を 2 つのチャンクに渡さない |
| `fixed_timestep` | §6 のループに経過時間の列を与え、呼ばれる `step` の回数が期待どおり |
| `palette_bytes` | `MaterialTable` からパレットのバイト列が正しく作られる |
| `shaders_parse` | すべての `.wgsl` を naga で構文検査し、エラーがない |
| 手動確認 (M4 完了時) | Windows、Linux、macOS、Android の実機で起動し、砂と水を置けることを確認して結果を `docs/PLATFORM_CHECK.md` に記録する |
