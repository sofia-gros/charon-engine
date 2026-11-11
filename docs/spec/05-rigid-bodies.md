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
