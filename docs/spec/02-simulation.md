# 仕様 02: セルのシミュレーション

対象クレート: `charon-core` の `sim` モジュール。先に仕様 01 を読むこと。

## 1. 1 tick の流れ

`World::step()` は次を順に行う。

1. 全チャンクで `dirty = next_dirty.take()`。
2. フェーズ 0 から 8 まで順に、そのフェーズに属し `dirty` が `Some` のチャンクを処理する (§2)。
3. `tick += 1`。

剛体・パーティクルを含む完全な順序は仕様 05 §2 にある。

## 2. 9 フェーズ更新 (安全な並列化)

チャンクのフェーズは `phase = cx.rem_euclid(3) + 3 * cy.rem_euclid(3)` (0..9)。

同じフェーズのチャンク同士は 3 チャンク離れているため、それぞれの「自分 + 周囲 8 チャンク」は
1 つも重ならない。したがって各タスクに 9 チャンクの所有権を丸ごと渡せる。**`unsafe` は使わない。**

1 フェーズの手順:

1. そのフェーズで `dirty` が `Some` のチャンク位置を集め、`ChunkPos` の昇順に並べる。
2. 各位置について、自分と周囲 8 個の `Box<Chunk>` を `HashMap` から `remove` し、
   `Neighborhood` に入れる (存在しないチャンクは `None`)。
3. 各 `Neighborhood` を処理する (§3)。`parallel` feature が有効なら rayon の `par_iter_mut`、
   無効なら普通の `iter_mut`。**どちらも同じ関数を呼ぶ。**
4. 全チャンクを `HashMap` に戻す。

```rust
// crates/charon-core/src/sim/neighborhood.rs
pub struct Neighborhood {
    pub center: ChunkPos,
    chunks: [Option<Box<Chunk>>; 9], // 添字 = (dx + 1) + 3 * (dy + 1)
}
```

`Neighborhood` の座標は「中心チャンクの左上を (0, 0) とするローカル座標」で、有効範囲は
x, y とも `-64..128`。

| 関数 | 動作 |
|---|---|
| `get(&self, x: i32, y: i32) -> Cell` | チャンクが `None` または範囲外なら `BOUNDARY` のセル |
| `set(&mut self, x, y, cell)` | 書き込み + 仕様 01 §6 の処理 + `wake_watchers` (§9)。チャンクが `None` なら何もしない |
| `mark_parity(&mut self, x, y)` | `FLAG_PARITY` を現在の tick の値にする。dirty は付けない (§3) |
| `hash(&self, x, y, salt) -> u32` | ローカル座標をワールド座標に直して `rng::hash` を呼ぶ |
| `swap(&mut self, ax, ay, bx, by)` | 2 つのセルを入れ替える。両方に `set` と同じ処理を行い、両方の `FLAG_PARITY` を現在の tick の値にする |
| `keep_awake(&mut self, x, y)` | 仕様 01 §6 |

1 セルの更新が読み書きしてよい範囲は、そのセルから **32 セル以内** とする (`MAX_REACH = 32`)。
これにより中心チャンクのセルは必ず `-64..128` の内側だけを触る。`max_speed` の上限 127 は
1 tick に最大 8 セルの移動であり、この制限を満たす。

## 3. チャンク内の走査

`dirty` の矩形の中だけを走査する。

- 行は **下から上** (`y = max_y` から `min_y` へ)。
- 列の向きは行ごとに決める: `hash(seed, tick, cx, world_y, SALT_SCAN) & 1` が 0 なら左から右、1 なら右から左
  (`cx` は中心チャンクの x、`world_y` はその行のワールド y)。
- 各セルで `update_cell` (§4) を呼ぶ。

更新済みの判定: `cur_parity = (tick & 1)`。動かせる相 (粉体・液体・気体) のセルで
`FLAG_PARITY` が `cur_parity` と同じなら、既にこの tick で動いたセルなので飛ばす。

- 飛ばすときは必ず `keep_awake` を呼ぶ。眠っている間に parity が古くなったセルは、起こされた tick で
  たまたま `cur_parity` と一致して飛ばされることがある。`keep_awake` しないと、そのまま処理されずに
  再び眠ってしまう (支えを失った砂が宙に残る)。
- 処理するセルは、移動の前に `mark_parity` で `FLAG_PARITY` を `cur_parity` にする。
  `mark_parity` は dirty を付けない (parity は状態ではなく、`state_hash` にも含めない)。
- `swap` は両方のセルの parity を `cur_parity` にするので、動いた先でもう一度処理されることはない。
- 固体は動かないので parity を見ない。

`update_chunk` は処理の最初に中心チャンクの `dirty` を取り出して `None` にする。
したがって `step` の後、`awake_chunks()` は `next_dirty` が付いたチャンクだけを数える。

## 4. `update_cell`

```
update_cell(nb, x, y):
    cell = nb.get(x, y)
    if cell.flags に FLAG_BODY: return          # 剛体のセルは動かさない
    mat = materials.get(cell.material)
    if mat.phase == Empty: return
    if mat.phase が Powder / Liquid / Gas で、cell の parity が cur_parity:
        nb.keep_awake(x, y); return             # §3

    step_lifetime(nb, x, y)            # 仕様 03 §5。セルが別の材質に変わることがある
    step_heat(nb, x, y)                # 仕様 03 §2, §3
    step_reaction(nb, x, y)            # 仕様 03 §1
    step_burning(nb, x, y)             # 仕様 03 §4

    cell, mat を読み直す
    match mat.phase:
        Powder      => nb.mark_parity(x, y); move_powder(nb, x, y, cell, mat)
        Liquid, Gas => nb.mark_parity(x, y); move_fluid(nb, x, y, cell, mat)   # §8
        Solid, Empty => 何もしない
```

マイルストーン M1 では `step_*` の 4 つは何もしない関数として置き、M2 で実装する。

## 5. 入れ替わりの判定

```
can_displace(mover: Cell, target: Cell) -> bool:
    if target.flags に FLAG_BODY: return false
    tm = materials.get(target.material); mm = materials.get(mover.material)
    match tm.phase:
        Empty  => true
        Solid  => false
        Powder => false
        Liquid => mm.phase が Powder か Liquid で、mm.density > tm.density
        Gas    => mm.phase が Gas なら mm.density > tm.density (重い気体が下)、それ以外は true
```

気体が **上へ** 動くときの判定は別関数 `can_rise(mover, target)`: target が `Empty` なら true、
target が `Gas` で `mover.density < target.density` なら true、それ以外は false。

液体の中を沈むときの抵抗: 粉体が **真下** の液体へ沈もうとするとき、
`chance(hash(.., SALT_DRAG), 液体の viscosity / 2 + 64)` が true なら **今回は動かず**
`keep_awake` して終わる。斜めの移動と、液体同士の入れ替わりには抵抗をかけない。

## 6. 落下の歩数 (粉体・液体で共通)

```
fall_steps(cell, mat) -> (steps, new_vy):
    vy = min(cell.vy + mat.gravity, mat.max_speed)
    steps = vy / 16
    if (hash(.., SALT_FALL) & 15) < (vy % 16): steps += 1
    steps = max(steps, 1)
    return (steps, vy)
```

## 7. 粉体 `move_powder`

```
(steps, vy) = fall_steps(cell, mat)
moved = 0
for i in 0..steps:
    if can_displace(cell, nb.get(x, y + 1)):
        nb.swap(x, y, x, y + 1); y += 1; moved += 1
    else: break

if moved > 0:
    セルに FLAG_FALLING を立て、vy を書き込む (moved < steps なら vy = 0)
    wake_neighbors(元の x, 元の y)
    return

# 真下に進めなかった
# 摩擦 (1): 着地したセルは、確率 friction / 256 でその場に止まる
if cell.flags に FLAG_FALLING かつ !chance(hash(.., SALT_STICK), mat.friction):
    d = (hash(.., SALT_DIAG) & 1) なら +1、そうでなければ -1
    for dx in [d, -d]:
        if can_displace(cell, nb.get(x + dx, y + 1)) かつ can_displace(cell, nb.get(x + dx, y)):
            nb.swap(x, y, x + dx, y + 1)
            vy = 0 を書き込む; wake_neighbors(元の x, 元の y)
            return
# どこにも動けなかった
FLAG_FALLING を下ろし、vy = 0 を書き込む (値が変わる場合だけ set する)
```

```
wake_neighbors(x, y):       # (x, y) は今セルが出ていった場所
    for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1)]:
        n = nb.get(nx, ny); nm = materials.get(n.material)
        if nm.phase == Powder かつ FLAG_FALLING なし かつ FLAG_BODY なし:
            if chance(hash(.., SALT_WAKE), 255 - nm.friction):
                n に FLAG_FALLING を立てて nb.set
```

摩擦は 2 か所で効く。

1. **着地したとき** (上の「摩擦 (1)」): `friction` が大きいほど、斜めに滑らずその場に積もる。山が急になる。
2. **隣が動いたとき** (`wake_neighbors`): `friction` が大きいほど、止まっているセルが崩れにくい。

`friction: 10` の灰は約 45 度のなだらかな山に、`friction: 80` の土は切り立った山になる。
形は `cargo run -p charon-core --example piles` で確かめられる。

(1) がないと、注いだ粉体は `friction` に関係なく同じ 45 度の山になる。実装時のテストで見つかり、追加した。動いていないセルは書き込みをしないので、
チャンクは自然に休眠する。

## 8. 液体と気体 `move_fluid`

液体と気体は同じ関数で動かす。気体は「上下が逆の液体」である。

| | 液体 | 気体 |
|---|---|---|
| 縦の向き `dy` | +1 (下) | -1 (上) |
| 通れるかの判定 `pass(t)` | `can_displace(cell, t)` | `can_rise(cell, t)` |
| 見送る確率 `skip_p` | `mat.viscosity` | 128 |
| 縦に進む歩数 | `fall_steps` (§6) | 常に 1。`vy` は使わない |

以下、「縦に進む」とは液体なら落ちること、気体なら昇ることを指す。

### 止まるための仕組み

単純な規則 (空いていれば横へ動く) では、最上段が埋まりきっていない水面で端数のセルが永久に左右へ
動き続け、その周囲が休眠しない。これを防ぐため、横への移動を次の 2 つの場合に限る。

- **行き先がある**: 進む方向の `SCAN = 24` セル以内に「縦に進める場所」が見えている。
- **押されている**: 自分の真上 (気体なら真下) に、同じ相で自分以上に重い (気体なら自分以下に軽い)
  セルが載っている。横へ動けば、載っていたセルが必ず縦に進む。
- **勢いがある**: 直前に縦へ進んだ、または押されて動いた (`FLAG_FALLING` が立っている)。
  勢いは、壁に 2 回はね返されると失う (`FLAG_BOUNCED` で 1 回目を覚えておく)。
  同じ相のセルに塞がれた場合は、はね返らずに **何も書き込まず待つ** (勢いは保つ)。

この 3 つのどれにも当たらないセルは何も書き込まない。したがって休眠する。

必ず止まる理由: 横への移動が続くには、縦への移動が起き続ける必要がある (行き先へ向かう移動も、
押されての移動も、縦の移動を伴う。勢いはそれらでしか得られず、壁 2 回で尽きる)。縦への移動は重い流体を下げる (軽い気体を上げる) ので、
全体の位置エネルギーが必ず減る。位置エネルギーには下限があるので、縦への移動は有限回で終わる。

### 手順

```
move_fluid(nb, x, y, cell, mat):
    d = FLAG_FLOW_RIGHT が立っていれば +1、なければ -1

    # 1. 縦
    if pass(nb.get(x, y + dy)):
        if chance(hash(.., SALT_VISC), skip_p): nb.keep_awake(x, y); return
        歩数ぶん、pass である限り縦に進む (swap を繰り返す)
        セルに FLAG_FALLING を立て、FLAG_BOUNCED を下ろす。
        液体は vy を書き込む (歩数の途中で止まったら 0)
        return

    # 2. 斜め
    for dx in [d, -d]:
        if pass(nb.get(x + dx, y + dy)) かつ pass(nb.get(x + dx, y)):
            if chance(hash(.., SALT_VISC), skip_p): nb.keep_awake(x, y); return
            nb.swap(x, y, x + dx, y + dy)
            FLAG_FALLING を立て、FLAG_BOUNCED を下ろす。FLAG_FLOW_RIGHT を dx の向きにする。vy = 0
            return

    ここから先は縦に進めなかった場合。液体は vy が 0 でなければ 0 を書き込む

    # 3. 行き先を探す
    for dir in [d, -d]:
        open[dir] = 0; goal[dir] = なし
        for i in 1..=SCAN:
            if !pass(nb.get(x + dir * i, y)): break
            open[dir] = i
            if pass(nb.get(x + dir * i, y + dy)): goal[dir] = i; break
        if goal[dir] がある:
            if chance(hash(.., SALT_VISC), skip_p): nb.keep_awake(x, y); return
            n = min(goal[dir], reach)          # reach = min(max(mat.dispersion, 1), SCAN)
            nb.swap(x, y, x + dir * n, y)
            FLAG_FLOW_RIGHT を dir の向きにする
            return

    # 4. 押されている
    b = nb.get(x, y - dy); bm = materials.get(b.material)
    pressed = bm.phase == mat.phase かつ b に FLAG_BODY なし かつ
              (液体なら bm.density >= mat.density、気体なら bm.density <= mat.density)
    if pressed:
        for dir in [d, -d]:
            if open[dir] > 0:
                if chance(hash(.., SALT_VISC), skip_p): nb.keep_awake(x, y); return
                n = min(open[dir], reach)
                nb.swap(x, y, x + dir * n, y)
                FLAG_FALLING を立て、FLAG_BOUNCED を下ろし、FLAG_FLOW_RIGHT を dir の向きにする
                return
        return                                      # 両側とも塞がっている。何も書き込まない

    # 5. 勢い
    if FLAG_FALLING なし: return                    # 何も書き込まない。ここで休眠する
    if open[d] > 0:
        if chance(hash(.., SALT_VISC), skip_p): nb.keep_awake(x, y); return
        n = min(open[d], reach)
        nb.swap(x, y, x + d * n, y)
        return
    if materials.get(nb.get(x + d, y).material).phase == mat.phase:
        return                                      # 同じ相に塞がれた。待つ (何も書き込まない)
    if FLAG_BOUNCED なし:                           # 壁に 1 回目のはね返り
        FLAG_BOUNCED を立て、FLAG_FLOW_RIGHT を反転して nb.set
        return
    FLAG_FALLING と FLAG_BOUNCED を下ろして nb.set    # 2 回目。勢いを失う (書き込みはこの 1 回だけ)
```

`swap` の後にフラグや `vy` を変える場合は、移動先のセルを読んで書き換え、`nb.set` する。

`SCAN + 1 = 25` セルは `MAX_REACH = 32` の範囲内である。

粘性はこの `skip_p` で効く。`viscosity: 240` のスライムは動ける状況でも 16 回に 15 回は見送り、
`dispersion: 1` なので 1 回に 1 セルしか広がらない。その結果ゆっくり垂れて広がる。
水 (`viscosity: 0`、`dispersion: 5`) は毎 tick 最大 5 セル流れる。

見送るときに `keep_awake` を呼ぶのは「動ける場所がある」と確かめた後だけである。
動ける場所がないセルが `keep_awake` を呼ぶと休眠しなくなるので、この順序を守ること。

この規則は `tools/prototype/fluid.py` で試作して確かめてある (8 通りの seed で、幅 40 の容器に注いだ水が
完全に平らになり、すべての場面で休眠した)。Rust の実装で迷ったら試作の `fluid()` を正とする。

既知の性質:

- 行き先が見えるのは 24 セル先までなので、勢いのない広い水面には、幅 24 セル以上につき高さ 1 セルの
  緩い段が残ることがある。注がれた水や落ちてきた水は勢いを持つため、通常は平らになる。
- 液体は上へは動かない。U 字管の両側の水位は揃わない (圧力は横までしか伝わらない)。
  落下砂シミュレーションで一般的な制限であり、対応は M8 で検討する。

## 9. 休眠の規則

休眠は性能の中心である。休眠していないチャンクは、CPU の計算に加えて、GPU へのテクスチャ転送
(`render_dirty`) も毎フレーム発生させる。次の規則を全体で守る。

1. **もう変化しないセルは、何も書き込まない。** `set` / `swap` / `keep_awake` のどれも呼ばない。
2. `set` は、値が実際に変わるときだけ呼ぶ。同じ値を書き直さない。
3. `keep_awake` を呼んでよいのは、「今回は確率で見送ったが、動ける (変化できる) ことを確かめた」場合と、
   「寿命・燃焼・反応の相手がいる」など、時間で必ず終わる処理の最中だけ。
4. 新しい移動規則や `step_*` を足すときは、必ず「最終的に全チャンクが眠る」テストを一緒に足す
   (`testkit::assert_sleeps`、`docs/TESTING.md`)。
5. **遠くを見る規則には、遠くを起こす処理を対にする。** 書き込みが起こすのは周囲 2 セルだけだが、
   液体・気体は `SCAN = 24` セル先まで行き先を見る (§8)。対にしないと、離れた場所で水位が下がっても、
   眠っている水が気づかずに段が残る。そのため `Neighborhood::set` は、材質が変わって通れる場所
   (空・液体・気体) ができたとき `wake_watchers` を呼ぶ。

```
wake_watchers(x, y):                    # (x, y) が通れるようになった
    for (dy, watcher) in [(+1, Liquid), (-1, Gas)]:
        row = y - dy                    # 液体は 1 段上、気体は 1 段下から (x, y) を見る
        if !open_for(x, row, watcher): continue
        for dir in [-1, +1]:
            for k in 1..=SCAN:
                c = nb.get(x + dir * k, row)
                if c の相が watcher で FLAG_BODY なし: nb.keep_awake(x + dir * k, row); break
                if !open_for(x + dir * k, row, watcher): break     # 壁
                if open_for(x + dir * k, y, watcher): break        # 足場が途切れた。この先のセルは
                                                                   # もっと近い行き先を既に持っている

open_for(x, y, watcher): セルが FLAG_BODY でなく、相が Empty、または (watcher が Liquid で相が Gas)
```

`World::set` (ブラシなど外からの編集) は頻繁ではないので、相手を探さずに、材質が変わったら
上下 1 行・左右 `SCAN + 1` セルの帯をまとめて起こす。

この欠陥は試作 (`tools/prototype`) では見つからなかった。試作には休眠がないためである。
`breached_wall_drains` が検出した。

## 10. 必須テスト

テストはすべて `charon-core` の中に置き、描画なしで動かす。

| テスト | 内容 |
|---|---|
| `sand_falls` | 空の 1 チャンクの上端に砂を 1 個置き、200 tick 後に最下行にある |
| `sand_piles` | 1 点から砂を 500 個落とす。山の高さ < 幅 (崩れている) |
| `friction_makes_steeper_piles` | 同じ条件で砂 (`friction: 30`) と土 (`friction: 80`) の山を作り、土のほうが高い |
| `unsupported_sand_falls_after_sleeping` | 眠っている砂の支えを消すと落ちる (§3 の parity の規則) |
| `water_levels` | 幅 40 の容器の左端から水を 400 個注ぎ、1500 tick 後に各列の水面の高さの差が 1 以下 |
| `sand_sinks_in_water` | 水の上に砂を置くと、砂が底に着き水が上に来る |
| `oil_floats_on_water` | 油と水を混ぜて置くと、1000 tick 後に油がすべて水より上 |
| `slime_is_slower_than_water` | 同じ形の塊を床に置き、100 tick 後の横幅はスライムのほうが狭い |
| `gas_rises` | 蒸気を下に置くと天井に達する |
| `mass_is_conserved` | 乱数で材質を撒いた世界を 500 tick 進め、材質ごとのセル数が変わらない (M1 の時点。M2 以降は反応のない材質だけで行う) |
| `deterministic` | 同じ seed で 2 回実行し、毎 tick の `state_hash()` が一致する |
| `settles_to_sleep` | 砂と水を落として 3000 tick 以内に、全チャンクの `dirty` が `None` になる |
| `partial_layer_sleeps` | 幅 20 の容器に水を 20×5 + 7 個 (最上段が埋まりきらない数) 入れ、3000 tick 以内に全チャンクが眠る |
| `gas_under_ceiling_sleeps` | 密閉した箱に、寿命のない気体を天井を埋めきらない数だけ入れ、3000 tick 以内に全チャンクが眠る |
| `breached_wall_drains` | 眠っている池の壁を 1 セル消すと、水が流れ出て、壁の両側の水位の差が 1 以下になり、再び眠る |
| `water_flows_through_tunnel` | 池の底から長さ 30 の水平なトンネルを通って、水が反対側の部屋に届く |
| `mixed_liquids_sleep` | 水と油とスライムを混ぜて入れ、5000 tick 以内に全チャンクが眠る |
| `full_pool_sleeps` | 幅 20 の容器に水をちょうど 20×5 個入れ、2000 tick 後に全チャンクの `dirty` が `None` |
| `chunk_border` | 砂がチャンクの境界をまたいで落ち、消えも増えもしない |
| `parallel_matches_serial` (M3) | `parallel` feature の有無で、毎 tick の `state_hash()` が一致する |

## 11. 性能目標 (M3 で検証)

デスクトップ (8 コア) で、動いているセル 100 万個の 1 tick を 8 ms 以内。
モバイル (Android、高性能 4 コア) で、動いているセル 25 万個の 1 tick を 8 ms 以内。
休眠しているチャンクのコストはほぼ 0 であること。

これは目標値であり、M3 のベンチマークで実測して、届かなければこの文書と実装を見直す。
