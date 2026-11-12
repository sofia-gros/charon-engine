# 仕様 09: M10 参照デモ場面 (`charon-demo`)

対象: M10。仕様 01〜08 の材質・決定性・保存・描画・ゲーム層の規則は変えない。この仕様は既存の機能を組み合わせる **データ MOD、生成器、UI、起動ファイル** だけを追加する。`charon-core` のシミュレーションには手を入れない。

## 1. 参照と「同じ」の意味

参照は FallingSandEngine リポジトリ (PieKing1215/FallingSandEngine) の README が載せる `demo.mp4` (1920×1016、30 fps、61.6 秒) である。README は動画の内容を説明していないため、**動画を実際に取得して 4 秒ごとに抜き出したフレームの観察**を根拠にする。観察した内容は次のとおり。

| 観察 | 動画で見えたもの |
|---|---|
| 世界 | 地上のない暗い洞窟。縦横に長く続き、カメラが横断する。洞窟の壁は微かに発光し、バイオームごとに色が違う (黄・緑・赤)。 |
| 光 | 色付きの光。発光する材質のまわりだけが明るく、色が混ざる。 |
| 材質 | 虹色の砂を注ぐと山になり、位置によって色が変わる縦の縞ができる。洞窟には虹色の塊 (横方向に色が変わる)、緑・橙の鉱床が点在する。 |
| 文字 | 黄バイオームの洞窟に発光する橙の文字 `yellow thing` (上下逆さのものもある)。緑バイオームの背景に薄い `green biome`。 |
| 剛体 | X 字、輪、Z 字の剛体が部屋にある (X 字はその場で回っている)。`Tile Entity` と書かれた枠が部屋にある。 |
| プレイヤー | 小さな箱のプレイヤーが常に画面の中心近くにあり、カメラが追従する。 |
| 構造物 | 壁と通路で作られた部屋。 |
| UI | 左上に `Debug`、`Main Menu` (`Singleplayer`、`Random Seed`、`Quit`)、材質の見本が並ぶ `Draw`、`Clipboard` (`Nothing here...`)、`Registries`。右下に `mspf` / `fps` / `tick mspt` / `phys mspt` のグラフ。左下に `Development Build` とビルド情報。左上端に FPS。 |
| デバッグ表示 | 地形のコライダーの輪郭線、構造物の外形線、剛体の印 (`X`)。 |

**「同じこと」とは、この表の各項目を、Charon Engine の既存機能で同じ操作・同じ見え方になるよう実装することを指す。** ピクセル単位の一致は目標にしない。次は再現しない (または別の方法に置き換える)。

- 参照の絵素材 (`gamedir/assets/texture/` は CC BY-NC 4.0) と、参照の材質表・生成式のコピー。見本の色・形・文字は本エンジンで描き直す。
- CPU 使用率とメモリ使用量。標準ライブラリだけでは取れないので、新しい依存を足さずに **表示しない** (グラフの 4 系列は `mspf`、`fps`、`tick mspt`、`phys mspt`)。
- 参照の `Z` / `X` / `C` キーの正確な意味。README は組を書くだけで機能を書かない。移動・ジャンプ・自由カメラだけを実装し、残りは未対応と記録する。
- 動画に写る参照のデバッグ線の色分けの意味。地形コライダーの輪郭は、固体のセルのうち周囲に固体でないセルがあるものを緑の画素で描く (簡略化した折れ線にはしない)。構造物の外形線は描かない。
- 参照の材質見本の質感のある絵。見本は材質の `color` の単色の四角にする (虹色だけ 12 色の縞)。

## 2. データ MOD `fse_demo` (`assets/mods/fse_demo`)

仕様 07 §6 のデータ MOD として置く。`manifest.ron` は `(id: "fse_demo", version: 1)`、`reactions.ron` は `[]`、`script.ron` は置かない。名前は `fse_demo:` で修飾される。

| 名前 | 相 | 内容 |
|---|---|---|
| `rainbow_sand_0` … `rainbow_sand_11` | 粉体 | 色相 0°、30°、…、330° の砂 (`density: 1600`、`friction: 30`)。暗い洞窟で明るく見えるよう、色の 1/2 を `emission` にする。 |
| `rainbow_rock_0` … `rainbow_rock_11` | 固体 | 同じ 12 色の固体 (`density: 2600`)。色の 1/3 を `emission` にする。 |
| `glow_moss_yellow` / `glow_moss_green` / `glow_moss_red` | 固体 | 洞窟の壁。`emission` が弱い黄 (90, 70, 10)・緑 (10, 90, 20)・赤 (100, 15, 10)。 |
| `glow_orange` | 固体 | 文字用。`emission: (255, 140, 20)`。 |
| `sign_blue` | 固体 | `Tile Entity` の枠と文字。`emission: (40, 90, 110)`。 |
| `green_ore` / `orange_ore` | 固体 | 鉱床。緑 (60, 220, 90) と橙 (240, 140, 40)。弱く発光する。 |
| `bg_text_green` | 固体 | 背景レイヤーの `green biome` の文字。色は暗い緑。 |

虹色の 12 色は `(240,60,60)`、`(245,150,50)`、`(240,225,60)`、`(150,230,60)`、`(70,225,80)`、`(60,225,150)`、`(60,225,225)`、`(60,150,240)`、`(80,90,245)`、`(150,80,240)`、`(225,70,225)`、`(240,60,150)` の順で、アルファ 255。

**虹色の選び方 (位置で色が変わる):** `rainbow_index(x) = x.div_euclid(4).rem_euclid(12)`。虹色のブラシ・生成器・塊は、セルの x 座標からこの添字を求め、`rainbow_sand_0` / `rainbow_rock_0` の ID に加える。12 個は ID が連続する (定義順)。

## 3. `DemoGenerator` (`charon-world`)

`ChunkGenerator` と、背景用の `background_cell(seed, x, y) -> MaterialId` を実装する。他のチャンクを参照せず、仕様 04 §4 と同じ純粋性を守る。整数のみ、乱数は `rng::hash` だけ。材質の ID は生成器に渡す (名前は引数を作る呼び出し側が解決する)。

- **縦方向:** `y < 0` と `y >= 640` はすべて `stone`。
- **バイオーム:** `band = x.div_euclid(384)`。`band == 0` は平坦 (苔なし)。それ以外は `hash(seed, 0, band, 0, SALT_DEMO_BIOME) % 4` が 0 = 黄、1 = 緑、2 = 赤、3 = 平坦。
- **洞窟:** `cave(x, y) = (2 * octave(x, y, 48, S_A) + octave(x, y, 24, S_B)) / 3` (各 `octave` は仕様 04 §4 の値ノイズで 0..=255)。`cave < 112` を空洞とする。
- **通路:** 横に長く続く通路。中心線 `tunnel_center(x) = 254 + (octave(x, 0, 240, S_TUNNEL) - 128) * free / 240`、`free = clamp(|x - 188| - 20, 0, 240)`。`|y - tunnel_center(x)| <= 9` を空洞にする。出入口 (`x = 188`) の付近は部屋の床の高さに揃う。
- **壁の苔:** 空洞でないセルの上下左右 2 セル以内に空洞があり、バイオームが黄・緑・赤なら対応する `glow_moss_*`。それ以外は `stone`。
- **塊:** 空洞のセルで、`x.div_euclid(384) != 0` (出発点の帯には置かない) のとき、`hash(seed, 0, x.div_euclid(48), y.div_euclid(48), S_BLOB) & 3 == 0` の領域かつ `octave(x, y, 12, S_BLOB2) < 64` なら `rainbow_rock_{rainbow_index(x)}`。同様に `S_GREEN` で `green_ore`、`S_ORANGE` で `orange_ore` (領域の条件は `& 7 == 0`、値の閾値は 44、`octave` の間隔は 10)。
- **文字 `yellow thing`:** 幅 160 の帯 `t = x.div_euclid(160)` が黄バイオーム (帯の中心 `t*160 + 80` のバイオーム) で `hash(seed, 0, t, 0, S_TEXT) & 1 == 0` のとき、左端 `x0 = t*160 + 40 + hash % 40` に置く。幅は 71 セル。縦位置は `k = 0..24` を順に、`y0 = 16 + hash(seed, t, k, S_TEXT_Y) % 600` として `(x0, y0 + 3)` と `(x0 + 70, y0 + 3)` が空洞である最初の `y0`。見つからなければ置かない。`hash(...) & 2 != 0` のとき 180° 回転する。字は 5×7、送り幅 6、`glow_orange`。文字のドットが立つセルだけを上書きする。
- **部屋 (`x ∈ [4, 188)`, `y ∈ [196, 268)`):** 上の規則を使わず固定。外周 4 セルは `stone`、内側 `x ∈ [8, 184)`, `y ∈ [200, 264)` は空、床の行 `y = 264..268` は `stone`。右の壁の `x ∈ [184, 188)`, `y ∈ [245, 264)` は出入口 (空) で、通路につながる。部屋の `x ∈ [100, 166)`, `y ∈ [215, 222)` に `Tile Entity` を `sign_blue` で描き、`x ∈ [98, 168)`, `y ∈ [213, 224)` の外周 1 セルを枠にする。部屋は `band == 0` の中にある。
- **背景:** 緑バイオームの帯で、`hash(seed, 0, x.div_euclid(200), 0, S_BG) & 1 == 0` のとき、縦位置 `y0 = 40 + hash % 500`、左端 `x0 = (x.div_euclid(200)) * 200 + 20` に `green biome` を拡大率 2 で `bg_text_green` を描く。それ以外は `EMPTY`。

字形は 5×7 のビットマップで、`y e l o w t h i n g r b m T E` と空白を持つ。`text_cells` は `(文字列, 拡大率, 回転) → 立つセルの集合` を返す純粋関数にする。

## 4. プレイヤーと追従カメラ (`charon-app`)

- 入力は `PlayerInput` だけにする (仕様 07 §2.2)。`A` / `←` = 左、`D` / `→` = 右、`W` / `↑` / `Space` / `C` = ジャンプ。左右が同時なら中立。入力は固定 tick に 1 回、ID 順に適用する。
- 自由カメラ (`F` で切り替え): 押している間 `WASD` / 矢印でカメラを 1 フレーム 4 セル (`Shift` で 12 セル) 動かし、プレイヤーは中立入力のままにする。
- 追従 (既定): 毎フレーム、カメラの原点をプレイヤー中心がシーンの中央になる位置にする。整数に切り捨てる。
- `charon-app` の `AppOptions { follow_player: bool, demo: bool }` を足し、既存の `run*` の挙動は `AppOptions::default()` で変えない。

## 5. 参照画面の UI (`charon-app`、egui)

`demo` が真のときだけ参照の配置にする。既存の Sandbox パネルは `demo` が偽のとき従来どおり。

- 左上の細い帯: `Debug`、`Main Menu`、`Draw`、`Clipboard`、`Registries` の見出し。クリックで各パネルが開閉する。
- `Main Menu`: `Singleplayer` (最初の世界に戻る)、`Random Seed` (新しい seed で作り直す)、`Quit`。
- `Draw`: 全材質の色見本 (材質の `color`) の格子。選ぶと `selected` になる。虹色の材質は 1 つの見本 `Rainbow` にまとめ、描くときだけ x で色を変える。
- `Clipboard`: `Ctrl` + ドラッグで矩形を選び `Copy` で材質を保存し、`Paste` を押してからクリックした位置に置く。何もなければ `Nothing here...`。
- `Registries`: 材質名・相・密度・色の一覧 (読み取り専用)。
- 右下: 直近 120 フレームの `mspf`、`fps`、`tick mspt`、`phys mspt` の折れ線と現在値。
- 左下: `Development Build` とクレートの版。左上端: FPS。
- `Debug`: チャンク境界・休眠範囲・地形コライダーの輪郭・剛体の位置の印を切り替える。

## 6. デモの起動と場面 (`apps/sandbox`)

`charon-demo` (`[[bin]]`、デスクトップ専用) を足す。

- `ModPack::load` で `fse_demo` を読み、`DemoGenerator` を `Streamer` に渡す。保存先は `saves/demo`。
- プレイヤーは部屋の `x = 40`, `y = 248` (左上、セル) に `spawn_player`。
- 剛体は部屋に 3 つ: X 字 (`ice`、16×16、線の太さ 5 セル)、輪 (`wood`、外径 18)、Z 字 (`stone`、16×16)。x はそれぞれ 80、110、140 付近。輪と Z 字は床に転がる。**X 字は床から浮かせ (中心 `(80, 250)`)、その場でくるくる回り続ける** (T10.7): 角速度は 3 ラジアン / 秒 (約 2 秒で 1 周)。回転の半径 (対角線の半分、約 11.3 セル) が床に当たらない高さにする。回すのはゲーム側 (`demo.rs` の `spin_cross`) で、エンジンは `PhysicsEngine::set_spin(index, Some(角速度))` を提供する。
- 選択材質の既定は `Rainbow`。

## 7. 必須テスト

| テスト | 確認 |
|---|---|
| `demo_mod_loads` | `fse_demo` が読め、虹色 12 個ずつと他の名前が定義され、虹色の ID が連続し、同じ並びを何度読んでも同じ |
| `demo_generator_is_pure` | seed 1〜8 で、生成順を逆にしても同じチャンク。負座標を含む |
| `demo_room_is_open` | 部屋の内側は空、床は固体、プレイヤーが生成でき、剛体 3 つを重ならず置ける |
| `demo_moss_and_ores` | 黄・緑・赤の帯に対応する苔、鉱床が一定数ある。前景に虹色の岩はない (動く虹色は背景のさらに奥の shader、§9) |
| `demo_text_is_drawn` | `yellow thing` が黄バイオームの 1 つ以上の帯に描かれ、180° 回転した版もある。`Tile Entity` が部屋にある |
| `demo_background_label` | 緑バイオームの背景に `green biome` があり、前景や他バイオームの背景にはない |
| `rainbow_index_is_periodic` | `rainbow_index` が x で周期 12、負座標でも 0..=11 |
| `player_follows_camera` | 追従カメラが、プレイヤー中心をシーンの中央に置く |
| `keyboard_maps_to_input` | キー列が `PlayerInput` に正しく写る (左右同時は中立) |
| `demo_scene_is_deterministic` | デモの世界・剛体・プレイヤーを seed 1〜8 で 150 tick (チャンクの読み込み遅延 30 tick のあと) 進めた、世界・プレイヤー・剛体のハッシュが 2 回の実行で一致し、プレイヤーが 1 人生成されて接地し、剛体 3 つがスタンプされている。seed が違えばハッシュも違う |
| `demo_bodies_are_valid_and_inside_the_room` | 剛体 3 つが作れ、部屋の内側に収まる |
| `perf_history_is_bounded` | 計測履歴が直近 120 フレームに収まり、異常な値は 0 になる |
| `rainbow_swatches_are_grouped` | `Draw` の見本が虹色を砂・岩で 1 つずつにまとめ、`empty` と `boundary` を含まない |
| `clipboard_copy_paste_roundtrip` | コピーした矩形が貼り付けで同じ並びになり、空は何も置かず、未読み込み・大きすぎる矩形は拒否される |
| `terrain_edges_follow_solid_boundaries` | 地形の輪郭が固体の外周だけで、砂は輪郭にならず、上限で打ち切る |
| `random_seed_is_deterministic_and_changes` | `Random Seed` の seed が決定的で 0 にならず、呼ぶたびに変わる |
| `demo_ui_draws_a_frame` | 画面なしで egui の 3 フレームを出し、全パネルと重ね描きが図形を出す |
| `rainbow_brush_colors_by_x` | 虹色ブラシが各セルの x 座標で色を選ぶ (直径 25 セルで 6 色以上) |
| `demo_background_is_populated_once` | 背景の文字が 1 度だけ作られ、処理済み・既存の背景を上書きしない |
| `demo_renders_on_gpu` | GPU があるとき、5 つの場面 (部屋・虹色の山・黄・緑・赤のバイオーム) をオフスクリーンに描いて画素を検査する。どの場面も暗い洞窟 (平均の明るさが小さい) で発光する材質が見え、部屋は看板の青、虹色の山は 11 色以上の縞、黄・緑・赤はそれぞれの色の壁が現れる。GPU なしではスキップする。`CHARON_REQUIRED_GPU_ADAPTER` を指定すると、その名前を含む離散 GPU でなければ失敗する。`CHARON_DEMO_CAPTURE_DIR` を指定すると各場面を `<場面>_world.rgba` (UI なし) と `<場面>_ui.rgba` (UI あり) で書き出す |

## 8. 暗さの設定

参照は光の届かない空間がほぼ黒で、光る材質のまわりだけが見える。合成パスに環境光の設定 `CompositePass::with_ambient` を足し、既定 (`CompositePass::new`) は従来の 0.12 のまま変えない。デモは環境光 0.045、光の届かない空間の背景色 (0.012, 0.014, 0.02) を使う (`build_passes` の引数)。従来のサンドボックスの見た目は変わらない。

GPU・ウィンドウでの見え方は自動テストでは完全には確かめられない。実機の確認結果は `docs/PLATFORM_CHECK.md` に分けて記録し、確認していない項目を達成済みと書かない。

## 9. 背景のさらに奥の虹色の塊 (T10.8)

2026-11-12 の依頼で、観察を直した。参照のフレームで、暗い洞窟の奥に点在する虹色の塊は、**前景のセルではなく、背景のさらに奥で動き回るアニメーションの shader** である。T10.2 で前景の固体 (`rainbow_rock`) として置いていたのは誤りなので、**前景の虹色の岩を取り除く**。砂の虹色 (ブラシ、§2) は正しく、変えない。

### 9.1 見え方

- 色は塊の中の横位置で変わる (左が赤、右が紫。砂の虹色と同じ並び)。縁は有機的な形で、形も時間でゆらぐ。
- 塊は**自分で動き回る**。カメラが止まっていても、周期の違う波の組み合わせで、広い範囲を漂う (隣の区画の方へも行き来する)。カメラが動くと、視差の割合だけ動く (奥にあるのでゆっくり)。
- **光の影響を受けない** (自ら明るい)。前景のセルと背景のセルの**後ろに隠れ**、洞窟の空間 (シーンが空の画素) にだけ見える。ブルームで少し光る。

### 9.2 エンジンの API (`charon-render`)

`DeepBackgroundPass::new(&GpuContext, DeepBackgroundParams) -> (DeepBackgroundPass, DeepBackgroundClock)`。`PassList` の**合成のあと・ブルームの前**に置く (`charon-app` の `build_passes_with`)。

| 項目 | 意味 |
|---|---|
| `seed` | 乱数の種。同じ種なら同じ配置 |
| `parallax` (0..=1) | カメラの動きに対する塊の動きの割合。小さいほど奥 |
| `grid` | 格子の 1 区画の大きさ (セル)。1 区画に最大 1 つの塊 |
| `density` (0..=1) | 塊のある区画の割合 |
| `min_radius` / `max_radius` | 塊の半径 (セル) |
| `drift` | 動き回る速さ・形がゆらぐ速さ (ラジアン / tick) |
| `roam` (0..=0.45) | 動き回る範囲 (格子の大きさに対する割合) |
| `brightness` | 色の明るさの倍率 |
| `empty` | シーンの「空」の色 (線形)。この色の画素にだけ描く。`BackgroundPass` のクリア色と同じにする |

- 描き方: 画面の各画素について、シーン (光を掛ける前) の画素が `empty` なら、塊を描く。画素のセル座標 `q = カメラの左上 * parallax + 画面上のセル` を `grid` で区切り、隣り合う 3×3 区画の塊を調べる。区画ごとに整数のハッシュ (PCG) で、塊の有無・半径・基準の中心・位相・速さを決める。中心は基準のまわりを `roam * grid` の範囲で動き (`sin`・`cos` の 2 波の和)、半径は角度に応じた 2 つの正弦で `±34%` ゆらぐ。色は塊の中の横位置 `(dx / 縁) * 0.5 + 0.5` から虹色 (赤が 0、紫が約 0.8)、縁に近いほど 30% 暗い。画素はセルの格子にそろえる (ドット絵のまま)。
- 時刻: `DeepBackgroundClock::set_tick(world.tick())` を毎フレーム呼ぶ (2^32 で折り返す)。壁時計は使わない。同じ種・同じ時刻・同じカメラなら同じ画像になる。
- `charon-app`: `AppOptions::deep_background: Option<DeepBackgroundParams>` をゲームが渡す。`None` なら描かない。

### 9.3 デモ (ゲーム側)

`demo.rs` の `demo_deep_background(seed)`: `parallax 0.5`、`grid 90`、`density 0.55`、半径 6〜16 セル、`drift 0.02`、`roam 0.3`、`brightness 1.0`、`empty = DEMO_CLEAR`、種は世界の seed から。

### 9.4 テスト

| テスト | 確認 |
|---|---|
| `uniform_layout_matches_the_shader`、`uniform_values_are_clamped`、`clock_is_shared_and_wraps` | シェーダーの入力の並び、値の範囲、時計 (`charon-render`) |
| `demo_moss_and_ores` | 前景に虹色の岩がない (あれば失敗)。苔は対応するバイオームにだけあり、鉱床が一定数ある (`demo_moss_and_blobs` を置き換える) |
| `demo_renders_deep_background` | RTX 4060 で、空の画面に虹色の塊が見え (8,000 画素以上、色相 8 種類以上)、時間が進むと動き回り (240 tick で 4% 以上の画素が変わる、1 tick の変化はそれより小さい)、同じ入力で同じ画像になり、カメラが 40 セル動くと塊は 20 セル (80 画素) だけ動く |
| `deep_background_is_behind_cells` | 前景のセルの内側は、塊の有無で画素が完全に同じ (後ろに隠れる)。外側には塊がある |
