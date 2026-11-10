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
| `get(&self, x: i32, y: i32) -> Cell` | チャンクが `None` なら `BOUNDARY` のセル。範囲外は `debug_assert!` で落とし、リリースでは `BOUNDARY` |
| `set(&mut self, x, y, cell)` | 書き込み + 仕様 01 §6 の処理。チャンクが `None` なら何もしない |
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

更新済みの判定: `cur_parity = (tick & 1) as u8`。セルの `flags & FLAG_PARITY == cur_parity` なら
既にこの tick で動いたセルなので飛ばす。`update_cell` は最後に必ずそのセル (移動した場合は移動先) の
`FLAG_PARITY` を `cur_parity` にする。

## 4. `update_cell`

```
update_cell(nb, x, y):
    cell = nb.get(x, y)
    if cell の parity が cur_parity: return
    if cell.flags に FLAG_BODY: return          # 剛体のセルは動かさない
    mat = materials.get(cell.material)
    if mat.phase == Empty: return

    step_lifetime(nb, x, y)            # 仕様 03 §5。セルが別の材質に変わることがある
    step_heat(nb, x, y)                # 仕様 03 §2, §3
    step_reaction(nb, x, y)            # 仕様 03 §1
    step_burning(nb, x, y)             # 仕様 03 §4

    cell, mat を読み直す
    match mat.phase:
        Powder => move_powder(nb, x, y)
        Liquid => move_liquid(nb, x, y)
        Gas    => move_gas(nb, x, y)
        Solid, Empty => parity だけ更新
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

液体の中を沈むときの抵抗: `can_displace` が true で target が `Liquid` のとき、
`chance(hash(.., SALT_DRAG), target の viscosity / 2 + 64)` が true なら **今回は動かず**
`keep_awake` して終わる。

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
if cell.flags に FLAG_FALLING:
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

これが摩擦の仕組みである。`friction` が小さい粉体は隣が動くとすぐ崩れ、なだらかな山になる。
`friction` が大きい粉体は動かず、急な斜面を保つ。動いていないセルは書き込みをしないので、
チャンクは自然に休眠する。

## 8. 液体 `move_liquid`

```
# 1. 落下
(steps, vy) = fall_steps(cell, mat)
粉体と同じループで真下へ進む
if moved > 0: vy を書き込み (moved < steps なら 0)、return

# 2. 斜め下
d = FLAG_FLOW_RIGHT が立っていれば +1、なければ -1
for dx in [d, -d]:
    if can_displace(cell, nb.get(x + dx, y + 1)) かつ can_displace(cell, nb.get(x + dx, y)):
        target = (x + dx, y + 1); goto 粘性チェック

# 3. 水平
n = 0; tx = x
while n < mat.dispersion かつ can_displace(cell, nb.get(tx + d, y)):
    tx += d; n += 1
    if can_displace(cell, nb.get(tx, y + 1)): break     # 下が空いたらそこで止まる
if n == 0:
    if can_displace(cell, nb.get(x - d, y)):
        FLAG_FLOW_RIGHT を反転して書き込む      # 次の tick に逆向きへ流れる
    # 左右とも塞がっているなら何も書き込まない (書き込むと dirty が付き、永久に眠れない)
    return
target = (tx, y)

粘性チェック:
    if chance(hash(.., SALT_VISC), mat.viscosity):
        nb.keep_awake(x, y); return        # 動けるが、今回は見送る
    nb.swap(x, y, target); vy = 0 を書き込む
```

粘性はここで効く。`viscosity: 240` のスライムは動ける状況でも 16 回に 15 回は見送り、
`dispersion: 1` なので 1 回に 1 セルしか広がらない。その結果ゆっくり垂れて広がる。
水 (`viscosity: 0`、`dispersion: 5`) は毎 tick 最大 5 セル流れる。

既知の制限: 最上段が埋まりきっていない水面では、端数のセルが左右に動き続け、その周囲だけ眠らない。
落下砂シミュレーションで一般的な挙動であり、M1 では許容する。

## 9. 気体 `move_gas`

気体は上へ動く。`vy` は使わない (常に 0)。

```
if chance(hash(.., SALT_GAS_SKIP), 128): nb.keep_awake(x, y); return   # 2 回に 1 回だけ動く
d = FLAG_FLOW_RIGHT が立っていれば +1、なければ -1
if can_rise(cell, nb.get(x, y - 1)):          swap して return
for dx in [d, -d]:
    if can_rise(cell, nb.get(x + dx, y - 1)): swap して return
水平: 液体の「3. 水平」と同じ。ただし判定は can_rise、
      「下が空いたら止まる」の代わりに「上が can_rise なら止まる」
動けなければ液体の `n == 0` と同じ処理 (判定は can_rise)
```

## 10. 必須テスト

テストはすべて `charon-core` の中に置き、描画なしで動かす。

| テスト | 内容 |
|---|---|
| `sand_falls` | 空の 1 チャンクの上端に砂を 1 個置き、200 tick 後に最下行にある |
| `sand_piles` | 1 点から砂を 500 個落とす。山の高さ < 幅 (崩れている) |
| `friction_makes_steeper_piles` | 同じ条件で `friction: 0` と `friction: 250` の山を作り、後者のほうが高い |
| `water_levels` | 容器に水を注ぎ、600 tick 後に各列の水面の高さの差が 1 以下 |
| `sand_sinks_in_water` | 水の上に砂を置くと、砂が底に着き水が上に来る |
| `oil_floats_on_water` | 油と水を混ぜて置くと、1000 tick 後に油がすべて水より上 |
| `slime_is_slower_than_water` | 同じ形の塊を床に置き、100 tick 後の横幅はスライムのほうが狭い |
| `gas_rises` | 蒸気を下に置くと天井に達する |
| `mass_is_conserved` | 乱数で材質を撒いた世界を 500 tick 進め、材質ごとのセル数が変わらない (M1 の時点。M2 以降は反応のない材質だけで行う) |
| `deterministic` | 同じ seed で 2 回実行し、毎 tick の `state_hash()` が一致する |
| `settles_to_sleep` | 砂だけを落として 2000 tick 後、全チャンクの `dirty` が `None` |
| `full_pool_sleeps` | 幅 20 の容器に水をちょうど 20×5 個入れ、2000 tick 後に全チャンクの `dirty` が `None` |
| `chunk_border` | 砂がチャンクの境界をまたいで落ち、消えも増えもしない |
| `parallel_matches_serial` (M3) | `parallel` feature の有無で、毎 tick の `state_hash()` が一致する |

## 11. 性能目標 (M3 で検証)

デスクトップ (8 コア) で、動いているセル 100 万個の 1 tick を 8 ms 以内。
モバイル (Android、高性能 4 コア) で、動いているセル 25 万個の 1 tick を 8 ms 以内。
休眠しているチャンクのコストはほぼ 0 であること。

これは目標値であり、M3 のベンチマークで実測して、届かなければこの文書と実装を見直す。
