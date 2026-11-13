# 仕様 17: ブロックと建築

世界はすべてセルでできている (今までと同じ)。**建築物 (拠点・ダンジョン) だけが、8×8 セルの「ブロック」でできる**。ブロックは、セルの**上位**にある。決まりの全体は [製品版までの計画](../PRODUCT_PLAN.md) §2 に書いた【決定】のとおり。この文書は、実装のための細部を決める。

## 1. 用語と定数

- `BLOCK_SIZE` = 8 (セル)。ブロック座標 `(bx, by)` = `(x >> 3, y >> 3)` (算術右シフトで、負の座標も床に向けて丸める)。ブロック `(bx, by)` の 64 セルは、`x` が `bx*8 ..= bx*8+7`、`y` が `by*8 ..= by*8+7`。チャンク (64) は 8×8 個のブロックにきれいに分かれる。
- `BLOCK_CELLS` = 64。

## 2. セルの旗 `FLAG_BLOCK` (charon-core)

1. `FLAG_BLOCK` = `1 << 7` (`flags` の最後の 1 ビット)。**ブロックの種類 = 材質**。ブロックにできる材質は、`Phase::Solid` と `Phase::Powder` だけ (液体・気体・空・境界は不可)。
2. **`FLAG_BLOCK` のセルは動かない**: 粉でも落ちない・崩れない (`update_cell` で移動しない)。熱・寿命・反応・燃焼は、普通のセルと同じように進む。
3. **ブロックの中の崩れの規則**: `FLAG_BLOCK` のセルは、**同じ 8×8 の中**の上下左右の隣に、`FLAG_BLOCK` でないセル (空を含む) があれば、自分の旗を外す。外したセルは、普通のセルとして落下の旗を立てて、周りを起こす (粉は崩れ、固体はその場に残る)。8×8 の外の隣は見ない (隣のブロックには連鎖しない)。1 tick に 1 セルずつ隣へ広がるので、1 つのブロックの全体が崩れるまで最大 14 tick。座標だけで決まり、決定的。
4. **材質が変わる処理は旗を落とす**: `convert` (反応・相変化・燃焼・寿命) の結果は、`PARITY` と `FALLING` だけを引き継ぐので、旗が落ちる (燃えた木のブロックは、燃えたセルが消えて、残りが崩れの規則で普通の木のセルになる)。
5. 眠り (仕様 02 §9): 旗の付いたセルは、何も変わらなければ何も書かない。旗を外すときだけ書く。
6. **旗を見る場所** (「固体」として扱う): エンティティの衝突 (`collides`・`obstacle_height`)・崩落 (`collapse`)・物理の地形の当たり (`terrain`)・チャンクの `collider_dirty`・画面の「掘れる地形」の判定。`Phase::Solid` だけを見ていた場所を、`Cell::is_solid(&Material)` (固体 または `FLAG_BLOCK`) に置き換える。光・視線・raycast・ドロップは、すでに粉も固体と同じに扱うので、変更なし。
7. 掘る道具 (レーザー) は、ブロックのセルも (材質の硬さで) 掘れる。1 セルでも掘れたら、崩れの規則で、そのブロックは崩れる。

## 3. 置く (`charon-game::blocks`)

```rust
pub struct BlockPos { pub bx: i32, pub by: i32 }
pub fn place_block(world: &mut World, entities: &mut EntityWorld, at: BlockPos, material: MaterialId)
    -> Result<Placed, PlaceError>;
pub enum PlaceError { Unloaded, InvalidMaterial, Occupied, EntityStuck(EntityId), NoRoom }
```

**すべてか、何もしないか**: 失敗したら、世界もエンティティも変えない (先に計画を作って、全部できると分かってから書く)。

### 3.1 前提 (満たさなければ失敗)

- `Unloaded`: ブロックの 64 セルのチャンクが、読み込まれていない。
- `InvalidMaterial`: 材質が `Solid` でも `Powder` でもない、または空・境界。
- `Occupied`: 64 セルのどれかが、**押しのけられない**セル (`Phase::Solid` の地形、すでに `FLAG_BLOCK`、`FLAG_BODY`、境界)。先に掘る。空・粉・液体・気体だけが、押し出される。

### 3.2 エンティティ

ブロックと**重なるエンティティ** (楕円がブロックの 64 セルのどれかに重なる) を、ID の昇順に動かす。

1. 向き: エンティティの中心が、ブロックの中心より左にあれば**左**、右にあれば右 (同じなら右)。
2. その向きに、1/16 セル単位で動かして、世界とこのブロック (置いたあと) に**ぶつからない最初の位置**へ。見つからなければ (探す上限は 64 セル)、反対の向き。
3. 左右とも見つからなければ、上下 (中心がブロックの中心より上なら上、下なら下、同じなら上)。それも見つからなければ、反対の向き。
4. どこにも行けなければ `EntityStuck`。

先に動かしたエンティティの位置を、あとのエンティティの判定に反映する。

### 3.3 セルの押し出し

64 セルのうち、空でないセルを、**行 (y が小さい順)** に処理する。各行で、**左半分** (x が `bx*8 ..= bx*8+3`) の空でないセルは左へ、**右半分** (`bx*8+4 ..= bx*8+7`) は右へ押す。

**行を押す**: 左へ押すセルが k 個あるとき、ブロックの左端の左から、左へ向かって調べ、次のどちらかで止める。

- 空きの数が合計 k 個になった (位置 `E`)。
- 押せないセル (固体・`FLAG_BLOCK`・`FLAG_BODY`・境界・読み込まれていない場所) に当たった。または、探す距離の上限 (64 セル) に届いた。

空きが k 個に届いたら、`E` からブロックの左半分の右端までの**空でないセル** (順番を保って) を、**ブロックの左端の左側にぴったり詰めて並べ直す**。範囲のそれ以外のセルは空になる。色・温度・寿命などは、そのまま運ぶ。右へ押す場合は左右が逆。

**足りなかった分**は、次の順で処理する。

1. **列を押す**: 足りなかったセルを、その x の列の、上半分は上へ、下半分は下へ、同じやり方で押す。
2. **いちばん近い空き**: それでも足りなければ、残ったセルを、上下左右の 4 本の直線で、いちばん近い空きセルへ移す (距離が同じなら、左・右・上・下の順)。
3. 空きがなければ `NoRoom`。

**押し出しの処理の順序は決定的**: 行は上から、行の中は左半分のあと右半分、足りなかったセルは x が小さい順。

### 3.4 書き込み

- 押し出したセルは、`World::set` / `swap` / `keep_awake` だけで動かす (セルを直接書き換えない)。移したセルには落下の旗を立てて、周りを起こす。
- 64 セルを、`make_cell(material)` に `FLAG_BLOCK` を立てたセルで置く。
- 空きだったセルの上書きは、セルを消すことにはならない。**保存する量**: 置く前に世界にあった空でないセルは、置いたあとも (押し出された場所で) すべて残る。増えるのは、ブロックの 64 セルだけ。

## 4. 壊す・拾う・作る

- **`break_block(world, at) -> Result<MaterialId, BreakError>`**: ブロックの 64 セルがすべて `FLAG_BLOCK` で、同じ材質のとき、64 セルを空にして、その材質を返す (つるはしで壊す。アイテムは、ブロック 1 個)。そうでなければ `NotABlock`。
- **一部だけ壊れたブロック** (崩れの規則で旗が外れたもの) は、普通のセル。壊せるのは、**完全なブロック**だけ。残ったセルは、セルのアイテム (材質の粉・固体のかけら) として、今までどおり掘って拾う。
- **クラフト**: 材質のセルのアイテム 64 個 = その材質のブロック 1 個 (`Solid`・`Powder` の材質)。`Inventory::craft_block(material) -> bool` (M20 で、データ駆動のレシピに置き換える)。ブロックのアイテムは、仮に `Inventory` に別の数え方 (`block_count`・`add_block`・`take_block`) を足す。

## 5. 同期 (Lockstep)

置く・壊すは、入力のコマンドとして載せる (通信の版 4 → 5)。コマンドは、`Place { bx, by, material }` と `Break { bx, by }`。1 tick に 1 つまで。受け取った側は、検証して (届く範囲・持ち物・失敗する条件)、失敗したら何も変えない。M19 の最後に、通信を足す (M19.5)。

## 6. 描画

`FLAG_BLOCK` のセルは、材質の色のまま描く。ブロックの境目が見えるように、ブロックの縁をわずかに暗くする (任意。P1。壊れる様子が分かりやすくなる)。

## 7. テスト

実装済み (T19.1〜T19.4)。名前は、テストの名前のとおり。

| テスト | 場所 | 確認 |
|---|---|---|
| `block_flag_is_its_own_bit` | `charon-core/tests/blocks.rs` | 旗は 8 ビット目 (`0x80`) で、ほかの旗と重ならない |
| `block_flag_survives_a_chunk_roundtrip` | `charon-world` | チャンクの保存・読み込みで旗が残る |
| `convert_clears_the_block_flag` | core | `convert` で旗が落ち、隣は崩れの規則で旗を外す |
| `block_cells_do_not_move_even_when_powder` | core | 砂のブロックは、下が空いていても落ちない。眠る |
| `sand_block_crumbles_when_one_cell_is_removed` | core | 1 セル消すと、最大 14 tick で全体が旗を外して粉になり、崩れる。砂は消えない |
| `stone_block_stays_solid_after_a_cell_is_removed` | core | 石のブロックは、旗が外れても、その場に残る |
| `crumbling_does_not_cross_block_borders` | core | 隣 (右・下) のブロックには連鎖しない |
| `crumbling_is_deterministic` | core | 同じ手順で、毎 tick 同じハッシュ |
| `entities_stand_on_block_cells` | `charon-game/tests/blocks.rs` | 砂のブロックの上に、エンティティが立てる |
| `place_block_in_empty_space` | game | 空きに置くと、64 セルが旗つきで並ぶ。増えるのは 64 セルだけ |
| `place_block_pushes_a_sand_field_outward` | game | 砂場に置くと、左半分は左へ、右半分は右へ、順番を保って詰まる。砂場全体がずれる |
| `place_block_conserves_cells` | game | 砂・水・石の、材質ごとの数が残る |
| `place_block_falls_back_to_vertical` | game | 左右が壁なら、上半分は上へ、下半分は下へ |
| `place_block_uses_the_nearest_free_cell_as_a_last_resort` | game | 左右も上下も塞がっていれば、いちばん近い空きへ (同じ距離なら、左・右・上・下の順) |
| `place_block_without_any_room_fails_and_changes_nothing` | game | 行き先がなければ `NoRoom`、何も変わらない |
| `place_block_refuses_solid_or_unloaded_or_invalid` | game | 固体・ほかのブロック・読み込まれていない場所・不正な材質では、何も変えずに失敗 |
| `place_block_pushes_entities_sideways` | game | 敵・プレイヤーは、左右の近い方へ飛ばされる (壁を通り抜けない) |
| `place_block_fails_when_an_entity_cannot_move` | game | 動かせないエンティティがいれば `EntityStuck`、何も変わらない |
| `place_block_is_all_or_nothing_and_deterministic` | game | 同じ入力なら同じ結果 |
| `break_block_returns_the_material_or_refuses` | game | 完全なブロックは壊せて材質を返す。欠けたものは壊せない |
| `craft_block_needs_64_cells` | `charon-game` (`inventory.rs`) | 64 個で 1 ブロック。63 個では作れない。ハッシュに入る |
| `craft_place_and_break_roundtrip_through_the_inventory` | game | 粉 → ブロック → 置く → 壊す → 持ち物にもどる。出来事が出る |
| `placing_needs_a_block_item_and_pushing_keeps_the_cells_in_the_world` | game | ブロックがなければ置けない。押し出された砂は世界に残る |

## 8. 実装の注意 (実装で分かったこと)

- **エンティティを動かす探索**は、1/16 セルずつ進み、途中で地形にぶつかったら、その向きは行き止まりにする。目的地だけを調べると、壁を通り抜けて、壁の向こうへ飛ばしてしまう (テストで見つけた)。
- **`FLAG_FALLING` を、ブロックのセルには立てない**。立てると、静止した粉として、地形の当たり (`collider_dirty`・物理の地形) から外れる。`is_collidable` は、`FLAG_BLOCK` のセルを、旗の有無によらず地形に含める。
- ブロックの **`Cell::is_solid`** を使う場所: エンティティの衝突・段差の判定・崩落・画面の「掘れる地形」・物理の地形・`collider_dirty`。

## 9. まだ実装していないもの

- **通信 (Lockstep) のコマンド** (§5、T19.5): 通信の版を 4 → 5 にして、`Place`・`Break` を載せる。ネットワークがアプリに組み込まれるとき (または協力プレイの着手時) に実装する。
- **材質ごとのブロックの種類の定義** (見た目・硬さ・崩れ方のデータ化): M20 のアイテムのデータと一緒に行う。
- **ブロックの縁の描画** (§6)、**つるはしでの破壊の時間** (硬さ × 力)、**届く範囲の確認**: M22 (建築の操作) で行う。
- デモ (`charon-demo`) への組み込み: M22 の建築の操作と一緒に行う (今のデモにはブロックのボタンがない)。
