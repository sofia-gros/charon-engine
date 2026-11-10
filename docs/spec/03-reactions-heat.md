# 仕様 03: 反応・熱・燃焼・寿命

対象: `charon-core` の `sim` モジュール (マイルストーン M2)。先に仕様 01、02 を読むこと。
ここにある関数は仕様 02 §4 の `update_cell` から呼ばれる。

## 1. 反応 `step_reaction`

反応は「隣り合った 2 つの材質が、確率で別の 2 つの材質に変わる」規則である。

```ron
// assets/reactions.ron
[
    (a: "water", b: "lava", prob: 200, out_a: "steam", out_b: "stone"),
    (a: "acid",  b: "stone", prob: 20, out_a: "empty", out_b: "empty"),
]
```

```rust
pub struct Reaction { pub prob: u8, pub out_a: MaterialId, pub out_b: MaterialId }

pub struct ReactionTable {
    n: usize,              // 材質数
    index: Vec<u16>,       // 長さ n * n。index[a * n + b] = reactions の添字 + 1。0 は「反応なし」
    reactions: Vec<Reaction>,
    reactive: Vec<bool>,   // 長さ n。その材質が a として登場する反応が 1 つでもあるか
}
```

読み込みの規則: 各行を `(a, b)` と、入れ替えた `(b, a)` (out も入れ替える) の両方で登録する。
同じ組が 2 回定義されたらエラー。材質数が 1024 を超える場合はエラーにする (表が大きくなりすぎるため。
必要になったら `HashMap` に変える)。

```
step_reaction(nb, x, y):
    cell = nb.get(x, y)
    if !reactive[cell.material]: return
    DIRS = [(0,-1), (1,0), (0,1), (-1,0)]
    # 4 方向のどこかに反応の相手がいるか (いる間は眠らない)
    any = DIRS のどれかで、隣が FLAG_BODY でなく index[cell.material * n + 隣.material] != 0
    if !any: return
    nb.keep_awake(x, y)
    # 実際に試すのは 1 方向だけ
    (dx, dy) = DIRS[hash(.., SALT_REACT_DIR) & 3]
    other = nb.get(x + dx, y + dy)
    if other.flags に FLAG_BODY: return        # 剛体の加工は仕様 05 §7 で扱う
    r = index[cell.material * n + other.material]; if r == 0: return
    if !chance(hash(.., SALT_REACT), reactions[r - 1].prob): return
    nb.set(x, y, convert(cell, out_a)); nb.set(x + dx, y + dy, convert(other, out_b))
```

`convert(cell, new_material)`: 材質を変える共通関数。`temp` と `shade` は引き継ぐ。`vy = 0`、
`flags` は `FLAG_PARITY` だけ残して `FLAG_FALLING` を立てる。`life` は新しい材質の `lifetime` から
引き直す (仕様 01 §5 の `make_cell` と同じ式)。材質が変わる処理は必ずこの関数を通す。

## 2. 熱の移動 `step_heat`

```
step_heat(nb, x, y):                          # (x, y) は空でないセル
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)]:
        cell = nb.get(x, y); mat = materials.get(cell.material)
        other = nb.get(x + dx, y + dy); om = materials.get(other.material)
        if other.material == BOUNDARY または other.flags に FLAG_BODY: continue
        # 左と上は、相手が空のときだけ扱う (空でない相手は、相手自身の「右・下」で扱われる)
        if (dx, dy) が (-1, 0) か (0, -1) で、om.phase != Empty: continue
        k = min(mat.conductivity, om.conductivity) as i32
        d = (other.temp as i32 - cell.temp as i32) * k / 1024    # 0 方向へ切り捨て
        if d == 0: continue
        cell.temp += d; other.temp -= d        # 合計は変わらない。i16 の範囲に clamp する
        両方を nb.set (dirty が付くので、温度が動いている間は眠らない)
```

こうすると、隣り合う組のうち少なくとも片方が空でなければ、その組は 1 tick にちょうど 1 回処理される
(空のセルは `update_cell` が何もしないので、空でない側が受け持つ)。
乱数で 1 方向だけ選ぶ方式にしてはいけない。選ばれなかった方向に温度差が残ったまま眠ってしまう。

空と空の組は熱を交換しない (熱い空気は自分では広がらない)。簡略化であり、M2 では許容する。

`d` が 0 になれば書き込みが止まり、自然に休眠する (温度差が `1024 / k` 未満で止まる)。

空 (`empty`) も温度を持つ。`empty` の `conductivity` は 8 とする (空気はゆっくり熱を運ぶ)。
この値は `MaterialTable` が `empty` を作るときに設定する。

## 3. 相変化 (`step_heat` の最後に続けて行う)

```
mat = materials.get(cell.material)      # 温度更新後の cell で
if let Some(t) = mat.boil   かつ cell.temp >= t.at: cell = convert(cell, t.into)
else if let Some(t) = mat.melt   かつ cell.temp >= t.at: cell = convert(cell, t.into)
else if let Some(t) = mat.freeze かつ cell.temp <= t.at: cell = convert(cell, t.into)
変わったら nb.set
```

往復して振動しないよう、データ側で差を付ける (例: 水は 100 で蒸気に、蒸気は 80 で水に)。
読み込み時に「A が T1 以上で B、B が T2 以下で A、かつ T2 >= T1」を検出したらエラーにする。

## 4. 燃焼 `step_burning`

```
step_burning(nb, x, y):
    cell = nb.get(x, y); mat = materials.get(cell.material)
    if FLAG_BURNING なし:
        if let Some(t) = mat.ignite_at かつ cell.temp >= t:
            FLAG_BURNING を立て、cell.life = max(mat.burn_time, 1); nb.set
        return
    # 燃焼中
    nb.keep_awake(x, y)
    cell.temp を mat.burn_temp に向けて 1 tick あたり最大 50 近づける
    4 方向から 1 つ選び、そこが Empty なら chance(.., 64) で "fire" のセルを make_cell して置く
    if tick & 3 == 0: cell.life -= 1
    if cell.life == 0: cell = convert(cell, mat.burn_into) (FLAG_BURNING は下りる)
    nb.set
```

`fire` は普通の材質としてデータで定義する (特別扱いしない):
`(name: "fire", phase: Gas, density: 1, initial_temp: 900, conductivity: 255, lifetime: Some((4, 12)),
decay_into: "smoke", emission: (255, 160, 40))`。
火が隣の可燃物に移る仕組みは「火が熱い → 熱が伝わる → `ignite_at` を超える」だけで成立する。

`fire` という名前を引く処理は起動時に 1 回だけ行い、`MaterialTable` に `fire: Option<MaterialId>` として
保持する。定義されていなければ炎のセルを出さないだけで、エラーにはしない。

## 5. 寿命 `step_lifetime`

```
step_lifetime(nb, x, y):
    cell = nb.get(x, y)
    if cell.life == 0 または FLAG_BURNING あり: return     # 燃焼中の life は §4 が使う
    nb.keep_awake(x, y)
    if tick & 3 != 0: return
    cell.life -= 1
    if cell.life == 0: cell = convert(cell, mat.decay_into)
       # convert が新しい材質の lifetime から life を引き直す。lifetime がなければ 0 のまま
    nb.set
```

## 6. 必須テスト

| テスト | 内容 |
|---|---|
| `water_lava_makes_stone_and_steam` | 水と溶岩を隣に置くと、石と蒸気ができる |
| `reaction_is_symmetric` | 上の配置を左右逆にしても同じ結果の材質数になる |
| `heat_is_conserved` | 断熱の壁 (`conductivity: 0`) で囲んだ箱の中で、全セルの `temp` の合計が 1000 tick 変わらない |
| `heat_flows_hot_to_cold` | 熱い石と冷たい石を並べると温度差が縮まる |
| `water_boils_and_condenses` | 水を 100 度以上にすると蒸気になり、冷えると水に戻る |
| `wood_burns_to_ash` | 木に火を付けると燃え広がり、最後は灰と煙になる。木は残らない |
| `fire_dies_out` | 可燃物のない場所の火は寿命で消え、最終的に全チャンクが眠る |
| `phase_loop_is_rejected` | 振動する相変化の定義は読み込み時にエラーになる |
| `deterministic` | 反応と熱を含む世界で、同じ seed の `state_hash()` が毎 tick 一致する |
