# 仕様 05: 剛体・パーティクルとセルの相互作用

対象クレート: `charon-physics` (マイルストーン M6)。先に仕様 01、02 を読むこと。
物理エンジンは rapier2d を使う。`enhanced-determinism` feature を有効にする。

## 1. 目標とする挙動

この仕様は次の場面が正しく動くことを目標にする。**これが M6 の受け入れ条件である。**

> 「C」の形をした剛体の内側に砂や水をためる。C を回転させて開口部を下に向けると、
> 中の砂や水は重力に従って開口部から流れ落ちる。C を持ち上げれば中身も一緒に運ばれ、
> 中身が重いほど C は重く振る舞う。水に浮かべた船は沈まず、水を入れたコップは宙に浮かない。

実現の方針は 1 つだけである: **剛体を毎 tick セルの格子に「スタンプ」する。**
スタンプされたセルは、セルのシミュレーションから見ると動かない固体である。
だから容器の中の砂や水は、特別な処理なしに容器の壁で止まり、壁がなくなれば落ちる。

## 2. 1 tick の完全な順序

```
1. 入力の適用 (ブラシでの配置など)
2. セル → 剛体の力を計算して rapier に加える     (§5、現在のスタンプを使う)
3. アンスタンプ: 全剛体のスタンプを格子から消す    (§3)
4. rapier を 1 ステップ進める (dt = 1/60 秒)
5. スタンプ: 新しい位置で格子に書き込む。重なったセルは押しのける (§3、§4)
6. セルのシミュレーション (仕様 02 の 9 フェーズ)
7. パーティクルの更新 (§6)
8. 地形コライダーの再生成 (§8、1 tick あたりの上限つき)
9. tick += 1
```

眠っている (rapier の sleeping) 剛体は 3 と 5 を飛ばし、スタンプを残したままにする。

長さの換算: `CELLS_PER_METER = 16.0`。rapier の重力は `(0.0, 9.81 * 4.0)` (y 下向き。
ゲームとして見栄えのよい強さ。調整可)。

## 3. スタンプ

```rust
pub struct PixelBody {
    pub handle: rapier2d::dynamics::RigidBodyHandle,
    pub width: u16,
    pub height: u16,
    pub cells: Vec<Cell>,          // 長さ width * height。空の場所は material == EMPTY
    pub stamped: Vec<CellPos>,     // 現在スタンプしているワールド座標
    pub origin: [f32; 2],          // ボディ内座標での重心 (セル単位)
}
```

スタンプは **逆写像** で行う。ボディのセルを順に変換して書き込む方法 (順写像) は、
回転したとき壁に穴が空いて中身が漏れるので **使ってはいけない**。

```
stamp(body):
    T = rapier から得た位置と回転 (セル単位に換算)
    aabb = 4 隅を T で変換した範囲を整数に広げたもの
    for wy in aabb.y, for wx in aabb.x:
        p = T の逆変換を (wx + 0.5, wy + 0.5) に適用し、origin を足す     # ボディ内座標
        bx = floor(p.x); by = floor(p.y)
        if bx, by が範囲外: continue
        bc = body.cells[by * width + bx]; if bc.material == EMPTY: continue
        world = world.get(wx, wy)
        if world.material == BOUNDARY: continue
        if world.flags に FLAG_BODY: continue             # 他の剛体が先に書いた。重なりは rapier が解消する
        if world.material != EMPTY: displace(world, wx, wy, body)      # §4
        world.set(wx, wy, bc に FLAG_BODY を立てたもの)
        body.stamped.push((wx, wy))

unstamp(body):
    for pos in body.stamped.drain(..):
        c = world.get(pos); if c.flags に FLAG_BODY: world.set(pos, Cell::default())
```

剛体は `Vec` に入れ、常に添字の昇順で処理する (決定性のため)。

スタンプされたセルは `FLAG_BODY` を持つので、仕様 02 の `update_cell` は動かさず、
`can_displace` は false を返す。熱と反応も `FLAG_BODY` のセルを相手にしない (仕様 03)。
M6 の時点では剛体は温度を持たない。剛体に熱を伝える仕組みは M8 で検討する。

## 4. 押しのけ `displace`

剛体が動いて、砂や水のある場所に壁が入ってきたときの処理。**セルを消してはいけない。**

```
displace(cell, wx, wy, body):
    v = 剛体上の点 (wx, wy) の速度 (セル毎 tick)。 v = 重心速度 + 角速度 × (点 - 重心)
    dir = v を 8 方向のうち最も近い向きに丸めたもの (|v| < 0.05 なら (0, -1) = 上)
    # 1. 進行方向へ押す
    for i in 1..=4:
        q = (wx, wy) + dir * i
        if world.get(q).material == EMPTY かつ q はこの剛体がこれからスタンプする場所でない:
            world.set(q, cell に FLAG_FALLING を立て vy = 0); 力積を加える; return
    # 1b. 密に詰まった内容物を前方へ連鎖して押す
    # 上限は max(body.width, body.height) + 1 セル。新しいスタンプの空洞を優先する。
    # dir 上の連続した Powder / Liquid セルの先に、スタンプ予定でない EMPTY があれば:
    # 空きから手前へ world.swap を繰り返し、各移動セルに FLAG_FALLING を立て vy = 0 とする
    # 移動したセルそれぞれの密度で力積を加える; return
    # 途中に BOUNDARY、FLAG_BODY、Solid、スタンプ予定のセルがあれば中断する
    # 2. 近くの空きを探す (距離の近い順、同距離は上→右→下→左の順)
    半径 1..=3 の正方形の輪を順に調べ、最初の空きに置く; 力積を加える; return
    # 3. 置き場所がない
    パーティクルにする (§6)。初速は v + 上向き 1.0; 力積を加える
```

「これからスタンプする場所でない」の判定は、逆写像をもう 1 回計算して行う
(`body.cells` のその位置が `EMPTY` かどうか)。
速度 `v` はその剛体のスタンプ開始時の線速度・角速度から計算する。力積はスタンプの全セルを
処理した後に座標順で加える。途中の力積によって、既に移動した剛体の進行方向を同じスタンプ中に
反転させないためである。

力積: 押しのけたセル 1 個につき、剛体のその点に `-v * density * CELL_MASS_SCALE` の力積を加える
(`CELL_MASS_SCALE = 1.0 / (1000.0 * CELLS_PER_METER * CELLS_PER_METER)`)。
水や砂の中を動く剛体が減速するのはこの力積による。

C の形の例での動き: C の内側 (空洞) は `body.cells` が `EMPTY` なのでスタンプされず、中身には触れない。
C が動くと壁が中身に重なり、中身は壁の進行方向へ押される。密な列は 1b で空洞内の空きへ順にずらすため、後方の空きへ押し出されずに容器と一緒に運ばれる。
開口部が下を向けば、中身の下に壁がなくなり、仕様 02 の規則で普通に落ちる。

## 5. セル → 剛体の力

毎 tick、スタンプ中の各セルについて、上下左右の隣を調べる。隣が液体または止まっている粉体なら、
その面に力がかかる。**浮力・水の重さ・砂の重さを、この 1 つの仕組みで扱う。**

```
for pos in body.stamped:
    for (dx, dy) in [(0,-1), (1,0), (0,1), (-1,0)]:
        n = world.get(pos + (dx, dy)); nm = materials.get(n.material)
        if n.flags に FLAG_BODY: continue
        if nm.phase == Liquid:
            surface_y = n と 4 近傍でつながる液体領域の最上段のセルの y
            face_y = pos.y + 0.5 + dy * 0.5     # 上面は pos.y、下面は pos.y + 1
            depth = clamp(face_y - surface_y, 0, 48)
            p = nm.density * depth * PRESSURE_SCALE
            力 = (-dx, -dy) * p を、点 pos に加える          # 面を押す向き
        else if nm.phase == Powder かつ FLAG_FALLING なし かつ (dx, dy) == (0, -1):
            h = n から真上に粉体が何セル続くか (上限 48)
            力 = (0, +1) * nm.density * h * PRESSURE_SCALE を、点 pos に加える
```

`PRESSURE_SCALE = 重力加速度 * CELL_MASS_SCALE` (1 セル分の液柱の重さに相当)。
rapier の `add_force_at_point` は次 tick にも力が残る。各面の力は `dt = 1/60` を掛けた
`force * dt` をその tick に `apply_impulse_at_point` で 1 回だけ加え、入力で与えた力を消さず、
水の圧力を tick 間で蓄積させない。

これで次が成り立つ。

- **浮力**: 水中の剛体は、底面が上面より深いので、上向きの力のほうが大きい (アルキメデスの原理そのもの)。
- **船**: 外側の水が船底を押し上げ、内側は空気なので押し返さない。浮く。
- **水入りのコップ**: 中の水が底を押し下げる。側面の力は左右で打ち消す。水の分だけ重くなる。
- **砂入りの容器**: 砂の柱の高さに比例して底が重くなる。

液体領域は、材質 ID が違っても `phase == Liquid` で `FLAG_BODY` のないセルを上下左右に
たどって作る。未読み込みの `BOUNDARY`、固体、粉体、剛体は領域を分ける。密閉された領域も
最上段のセル上端を仮の水面とする。同じ領域のすべての面で `surface_y` を共有し、力の密度は
従来どおり面に接する材質の `nm.density` を使う。これにより底面の真上を剛体が塞いでも、
側方でつながる外水面から底面の深さを測れる。上面と底面の深さの差は剛体の高さになり、
同じ液体に全没した物体への正味の浮力は押しのけた液体の重さになる。領域探索は毎 tick の
現在のワールドについて行い、固定の上下左右順でたどる。計算済みの領域と `surface_y` は
その tick の力計算中だけ使い回す。粉体の高さは従来どおり真上の連続セルで数える。

剛体のセル数が多い場合の最適化として、スタンプを 1 つおきに間引いて力を 2 倍にしてもよいが、
最初は間引かずに実装し、ベンチマークを見てから判断する。

## 6. パーティクル

パーティクルは、格子から一時的に離れて飛んでいるセルである。押しのけと爆発に使う。

```rust
pub struct Particle { pub pos: [f32; 2], pub vel: [f32; 2], pub cell: Cell }
```

```
毎 tick、Vec の順に:
    vel.y += 0.25                          # 重力 (セル毎 tick²)
    pos から pos + vel まで、通過するセルを 1 つずつたどる (DDA)
        次のセルが EMPTY でなければ: 直前の空きセルに cell を置く (FLAG_FALLING を立てる)。パーティクルを消す
    最後まで進めたら pos を更新
    BOUNDARY に入ったら、直前の空きセルに置いて消す
```

1 tick の移動は 32 セルまでに制限する (`vel` の長さを clamp)。パーティクル数の上限は 50,000。
上限に達している場合、`displace` の 3 は半径を 16 まで広げて空きを探す (セルは消さない)。
パーティクルは逐次処理でよい (決定性を優先する)。

## 7. 剛体の生成・破壊

- **生成**: セルの 2 次元配列から作る。輪郭をマーチングスクエアで抽出し、
  Douglas-Peucker 法 (許容誤差 1.0 セル) で簡略化し、三角形分割して、三角形の集合 (compound) を
  コライダーにする。質量は各セルの `density * CELL_MASS_SCALE` の合計、重心も同様に求める。
  摩擦係数と反発係数は、含まれるセルの `rigid_friction`、`restitution` の平均。
- **加工**: 仕様 03 の反応・燃焼は `FLAG_BODY` のセルを対象にしない。剛体を削る・燃やす操作は
  `PixelBody::cells` を直接書き換える API (`damage(body, local_x, local_y)`) で行う。
- **分割**: `cells` が変わった剛体は、次の tick の冒頭で 4 近傍の連結成分に分ける。
  成分が 2 つ以上なら、それぞれを新しい剛体にする (位置・速度・角速度を引き継ぐ)。
  8 セル未満の成分は剛体にせず、パーティクルにする。

## 8. 地形のコライダー

剛体が地形に乗るために、チャンクごとに静的コライダーを作る。

- 対象のセル: `phase == Solid`、または `Powder` で `FLAG_FALLING` なし。`FLAG_BODY` は除く。
- マーチングスクエアで輪郭を取り、Douglas-Peucker 法 (許容誤差 1.0) で簡略化し、
  折れ線 (polyline) コライダーにする。
- 作り直すのは `collider_dirty` が立っていて、かつ起きている剛体の AABB から 1 チャンク以内のチャンクだけ。
  1 tick に最大 4 チャンクまで。近い順に処理する。
- 摩擦係数は、そのチャンクで最も多い対象材質の `rigid_friction`。

## 9. 摩擦と粘性のまとめ

| 何と何の間か | 決めるもの | 仕組み |
|---|---|---|
| 粉体の粒同士 | `Material::friction` | 仕様 02 §7 の `wake_neighbors` |
| 液体の流れやすさ | `Material::viscosity`、`dispersion` | 仕様 02 §8 の粘性チェック |
| 液体の中を沈む粉体 | 液体側の `viscosity` | 仕様 02 §5 の抵抗 |
| 剛体同士・剛体と地形 | `Material::rigid_friction`、`restitution` | rapier のコライダー設定 (§7、§8) |
| 剛体と周りの砂・液体 | 押しのけたセルの `density` | §4 の力積 |

## 10. 必須テスト

すべて描画なしで動かす。

| テスト | 内容 |
|---|---|
| `stamp_has_no_holes` | 幅 1 セルの輪の形の剛体を 0〜360 度まで 1 度刻みで回してスタンプし、どの角度でも輪の内外が 4 近傍でつながらない |
| `stamp_unstamp_restores_world` | スタンプしてアンスタンプすると、ワールドが元に戻る |
| `c_shape_holds_contents` | 開口部が上の C に砂 200 個を入れ、300 tick 後も砂がすべて C の内側にある |
| `c_shape_pours_when_rotated` | 上の状態から C を 180 度回す。600 tick 後、砂がすべて C より下にある。**砂の数は 200 のまま** |
| `c_shape_pours_water` | 同じことを水で行う |
| `carried_contents_move_with_body` | 砂入りの C を右へ 100 セル動かすと、砂の重心も右へ 80 セル以上動く |
| `displace_conserves_mass` | 砂で満たした箱の中で剛体を動かし続けても、砂のセル数 + パーティクル数が変わらない |
| `boat_floats` | 水面に置いた舟形の剛体が、600 tick 後も水面にある |
| `stone_sinks_wood_floats` | 密度が水より大きい塊は底に着き、小さい塊は浮く |
| `filled_cup_is_heavier` | 同じコップを 2 つ天秤に載せ、片方に水を入れると、そちらが下がる |
| `body_splits_when_cut` | 棒の中央を `damage` で切ると、剛体が 2 つになる |
| `deterministic` | 剛体・パーティクルを含む世界で、同じ seed の `state_hash()` が毎 tick 一致する |

## 11. 滑らかな剛体 (T10.9)

2026-11-12 の依頼。剛体の描画を、剛体ごとに **ドット** か **滑らか** から選べるようにする。世界の全体はドットのまま、家具などの一部だけを滑らかに見せる、といった使い方のための、エンジンの機能である。物理 (衝突・押しのけ・セルから受ける力・光の遮りと発光) は、どちらでも**同じ**で、変わるのは描き方だけ。

### 11.1 ドットと滑らか

| | ドット (既定) | 滑らか |
|---|---|---|
| 描き方 | スタンプしたセルを、セルの格子の上にそのまま描く (従来) | 画面の解像度で、剛体の正確な位置と角度で描く |
| 縁 | セル単位の階段 | アンチエイリアスした縁 |
| 色 | セルの色 | セルの色を滑らかに補間した色 |
| 光 | セルとして光る・遮る | 同じ (スタンプしたセルが光る・遮る)。さらに描画では光の明るさを掛ける |

### 11.2 `charon-physics` の API

```rust
pub enum BodyRender { Dots, Smooth }               // 既定は Dots
pub struct PixelBody { /* ... */ pub render: BodyRender }
pub struct BodyPose { pub center: [f32; 2], pub angle: f32 } // 重心のセル座標とラジアン

PhysicsEngine::set_render(index: usize, render: BodyRender) -> bool  // index がなければ false
PhysicsEngine::body_pose(index: usize) -> Option<BodyPose>
```

- `Smooth` の剛体は、スタンプするセルに `FLAG_SMOOTH` (`1 << 6`、仕様 01) を付ける。意味は「このセルは剛体が書いた。滑らかな描画のパスが描くので、セルの描画 (`CellPass`) は描かない」。シミュレーションは `FLAG_BODY` と同じに扱い、`FLAG_SMOOTH` は無視する。保存しない (スタンプは保存の前に外す)。
- `set_render` の変更は、次のスタンプから反映する (その tick のスタンプ済みのセルは、次の tick の再スタンプで入れ替わる)。描画の種類は保存しない。読み込んだあとゲームが指定し直す (`set_spin` と同じ)。
- `body_pose(index)` は、剛体の重心の位置 (セル座標、`translation × CELLS_PER_METER`) と角度 (`rotation().angle()`)。`index` がないか、物理の剛体がなければ `None`。

### 11.3 `charon-render` の描画

- `cell_to_texel`: `FLAG_SMOOTH` のセルは `[0, 0, 0, 0]` (空) にする。`CellPass` は何も描かない。光の計算はワールドのセルを直接読むので、発光と遮りは変わらない。
- `SmoothBodySprite<'a> { id: u64, width: u16, height: u16, cells: &'a [Cell], center: [f32; 2], origin: [f32; 2], angle: f32 }`: `center` は重心のワールド座標 (セル)、`origin` は重心の剛体のローカル座標 (`PixelBody::origin`)、`id` は剛体を見分ける番号 (描画のキャッシュのキー)。
- `SmoothBodyPass::new(ctx, &MaterialTable, ambient)`: 合成のあと (`DeepBackgroundPass` のあと)・後処理の前に置く。`PassList::render_with_bodies(ctx, world, background, entities, smooth, visible, output, origin)` で `SmoothBodySprite` の列を受け取る (`render_with_game` は `smooth` を空にして呼ぶ)。
- 色の作り方: 剛体のセルごとに、材質のパレットの色に `shade` の変化 (仕様 06 §3 の `variation` と同じ式。線形の色に掛ける) を掛け、アルファは材質の色のアルファ。空のセルは透明。これを、外側に 1 セルの透明な余白を足した `(width + 2) × (height + 2)` の `Rgba8UnormSrgb` (ストレートアルファ) のテクスチャにする。**透明なセルの色は、8 近傍の不透明なセルの色の平均で埋める** (バイリニア補間のとき、縁が黒く暗くならないように)。材質と `shade` の内容が変わったとき (ハッシュで検出) だけ作り直し、`id` でキャッシュする。使われなくなった剛体のテクスチャは捨てる。燃焼・温度の表示は付けない。
- 描き方: 剛体ごとに、剛体のローカル座標で `-1 .. width + 1`、`-1 .. height + 1` (余白を含む) の四角形を、`world = center + R (local - origin)` でワールド座標へ回して描く (頂点のテクスチャ座標は `(local + 1) / (size + 2)`)。位置と角度は浮動小数点のまま、カメラの原点とシーンの大きさから画面へ写す。各画素でバイリニア補間してサンプルし、縁は、補間したアルファ `a` を `smoothstep(0.35, 0.65, a)` に通して整える (低解像度のマスクから、滑らかで鋭い縁を作る)。色は補間した色をそのまま使う。光は合成と同じ式で掛ける: `色 × min(1, ambient + 光)`。光は `FrameContext::light[0]` を線形補間でサンプルする (品質が `Off` のときは掛けない)。
- 描画先は合成後の画面の解像度のテクスチャ。位置と角度は浮動小数点のまま、セルの格子にそろえない。
- `charon-app`: `build_passes_with` が `SmoothBodyPass` を常に足す。毎フレーム、`physics.bodies` のうち `render == Smooth` の剛体から `SmoothBodySprite` を作る (`smooth_sprites(&PhysicsEngine)`) 。

### 11.4 デモ (ゲーム側)

部屋に**滑らかな十字**を足す。ドットの十字 (中心 `(80, 250)`、3 rad/s) とは別に、もう 1 つの 16×16 の十字 (`ice`、線の太さ 5 セル) を中心 `(60, 226)` (看板・プレイヤー・ドットの十字・輪に触れない、上の方) に置き、`set_render(.., Smooth)` と、逆向きの回転 (`-3` rad/s) を指定する (`demo.rs` の `spin_crosses`)。剛体の順は、ドットの十字・輪・Z 字・滑らかな十字。2 つを並べて、違いが見える。

### 11.5 テスト

| テスト | 確認 |
|---|---|
| `flags_do_not_overlap` | `FLAG_SMOOTH` を含む全フラグのビットが重ならない (`charon-core`) |
| `smooth_flag_hides_cell_texels` | `FLAG_SMOOTH` のセルは空のテクセルになり、ほかのフラグ・温度の変換は変わらない (`charon-render`) |
| `smooth_body_stamps_flagged_cells` | `Smooth` の剛体のスタンプには `FLAG_SMOOTH` と `FLAG_BODY` が付き、`Dots` には `FLAG_SMOOTH` が付かない。切り替えると次のスタンプから変わり、スタンプしたセルの数は同じ |
| `smooth_flag_does_not_change_the_simulation` | ドットでも滑らかでも、剛体の位置・角度・スタンプの位置が毎 tick 同じ (物理は変わらない) |
| `body_pose_matches_the_rigid_body` | `body_pose` が重心の位置 (セル) と角度を返し、範囲外の `index` は `None` |
| `broken_smooth_pieces_stay_smooth` | 壊れた滑らかな剛体の破片は、滑らかな描き方を引き継ぐ |
| `sprite_texels_are_dilated_and_padded` | テクスチャに 1 セルの余白が付き、透明なセルの色は隣の色で埋まり (アルファは 0)、離れたセルは黒のまま |
| `sprite_color_follows_shade_variation` / `content_hash_changes_with_material_and_shade` / `uniform_layout_matches_the_shader` | `shade` の変化が色に出る。ハッシュは材質と `shade` で変わり、温度では変わらない。シェーダーの入力の並び |
| `demo_has_a_smooth_cross` | デモの 4 つ目の剛体 (2 つ目の十字) が `Smooth` で、逆向きに回り、スタンプに `FLAG_SMOOTH` が付く。ほかの剛体は `Dots` (画面なし) |
| `demo_renders_smooth_body` | RTX 4060 で、滑らかな剛体のスタンプしたセルはセルの描画では描かれず (スプライトを描かなければ、剛体のない画面とほぼ同じ)、滑らかな十字が描かれ、縁の細かさ (4×4 画素の明るさの幅の平均) がドットの十字の 1.25 倍より大きい |
| `smooth_body_turns_below_cell_resolution` | 角度を 0.02 ラジアン (約 1 度) 変えただけで、滑らかな十字の画素が変わる (セルの格子にそろえない) |
