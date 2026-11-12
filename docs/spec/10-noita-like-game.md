# 仕様 10: Noita 型ゲームの中核 (M11)

対象: M11。目標は、落下砂の世界の中で「歩く・掘る・撃つ・燃える・溺れる・爆発で地形が崩れる」遊びを成立させること。セル・材質・座標・決定性は仕様 01〜03、剛体と爆発は仕様 05・07 §5、エンティティは仕様 07 §2 に従う。ここで足すものはすべて `charon-game` に置き、`charon-core` のシミュレーションは変えない。

## 1. 既にあるもの・足すもの

| Noita の要素 | 状態 | タスク |
|---|---|---|
| 無限の洞窟・バイオーム、背景、保存、光 | 実装済み (M5・M7・M8・M10) | — |
| 液体・粉体・気体、熱、燃焼、反応、相変化 | 実装済み (M1〜M2) | — |
| 剛体、パーティクル、爆発 | 実装済み (M6、07 §5) | — |
| プレイヤーの移動・ジャンプ | 実装済み (07 §2) | — |
| 体力、熱・溺れによるダメージ | **未実装** | T11.1 |
| 飛び道具 (爆発弾・材質を撒く弾)、爆発のダメージ | **未実装** | T11.2 |
| 杖 (マナ・詠唱間隔・再充填) と呪文 (RON、MOD から追加) | 未実装 | T11.3 |
| 敵 (歩く・追う・撃つ) | 未実装 | T11.4 |
| 掘る道具、浮遊 (飛行) | 未実装 | T11.5 |
| サンドボックス・デモへの組み込み、ゲーム UI | 未実装 | T11.6 |

T11.1 と T11.2 を先に作り、T11.3 以降は T11.2 の `ProjectileWorld` を土台にする。仕様の追加は、そのタスクに着手する前にこの文書へ書く。

## 2. 共通の約束

- 整数だけで計算する。時計・`HashMap` の反復順・`rand` を使わない。乱数が必要なら `rng::hash`。
- 格納は `BTreeMap`、ID は 1 から単調増加で再利用しない (07 §2.1 と同じ)。
- セルは `World::set` / `make_cell` を通してだけ書き換え、材質名をコードに書かない。性質は `Material` のフィールドで判断する。
- 1 tick の順序は 07 §7 の `Lockstep` に今後組み込む。T11.1〜T11.2 の関数は、呼び出し側が ID 昇順で 1 回ずつ呼ぶ。

## 3. T11.1: 体力と環境ダメージ (`charon-game::vitals`)

```rust
pub const MAX_HP: i32 = 100;
pub const BREATH_MAX: u16 = 600;      // 10 秒
pub const HEAT_THRESHOLD: i16 = 150;  // この温度以上の接触でダメージ
pub const DROWN_INTERVAL: u8 = 8;     // 息が尽きたあと、この tick ごとに 1 ダメージ

pub struct Vitals { pub hp: i32, pub max_hp: i32, pub breath: u16, drown_clock: u8 }
pub struct VitalsTable { /* BTreeMap<EntityId, Vitals> */ }
```

- `Vitals::new(max_hp)`: `hp = max_hp`、`breath = BREATH_MAX`。`max_hp` は 1..=10000 でなければ `VitalsError::InvalidMaxHp`。
- `damage(amount)`: `amount <= 0` は何もしない。`hp` は 0 で止まる (負にならない)。`heal(amount)` は `max_hp` で止まる。`alive()` は `hp > 0`。
- `VitalsTable`: `insert(id, vitals)`、`get`、`get_mut`、`remove`、`iter` (ID 昇順)、`state_hash()` (FNV-1a 64-bit、07 §2.1 と同じ定数、各整数はリトルエンディアン、`drown_clock` を含む)。
- `step_environment(entity: &Entity, vitals: &mut Vitals, world: &World)` を 1 tick に 1 回呼ぶ。`hp == 0` なら何もしない。
  1. **熱**: エンティティの当たり判定が覆う全セル (07 §2.2 の範囲) のうち `FLAG_BODY` でなく `BOUNDARY` でもないものについて、`FLAG_BURNING` が立つか `temp >= HEAT_THRESHOLD` なら `hot = max(hot, heat_damage)`。`heat_damage = clamp((temp - 100) / 50, 1, 20)`、`FLAG_BURNING` のセルは 2 以上 (`max(2, …)`)。全セルの最大値を、その tick のダメージにする。
  2. **溺れ**: 当たり判定の最上段の行のセルのうち、材質の `phase == Liquid` のものが半数以上 (`2 * liquid >= width`) なら「頭が水没」。水没中は `breath` を 1 減らし (0 で止まる)、`breath == 0` の間は `drown_clock` を 1 進め、`DROWN_INTERVAL` に達したら 0 に戻して 1 ダメージ。水没していなければ `breath` を 4 回復 (`BREATH_MAX` で止まる) し、`drown_clock = 0`。
  3. 1 と 2 のダメージを合計して `damage` に渡す。
- この関数はセルも `Entity` も書き換えない。

| テスト | 確認 |
|---|---|
| `hot_cells_damage_entity` | 溶岩・燃えるセルに触れると毎 tick ダメージが入り、離れれば止まる |
| `drowning_needs_submerged_head` | 足だけ水に浸かっても息は減らず、頭が水没すると 600 tick で息が尽き、その後 `DROWN_INTERVAL` ごとに 1 ずつ減る |
| `breath_recovers_in_air` | 水から出ると息が回復し、`drown_clock` が 0 に戻る |
| `damage_and_heal_saturate` | `hp` は 0 と `max_hp` の外に出ない。0 以下のダメージは無視 |
| `vitals_hash_is_stable` | 同じ操作列で `state_hash` が等しく、`hp` か `breath` が違うと異なる |

## 4. T11.2: 飛び道具と爆発ダメージ (`charon-game::projectile`)

```rust
pub enum Payload {
    None,
    Explode { radius: u8 },        // 1..=16 (07 §5)
    Deposit { material: MaterialId }, // 着弾の直前の空きセルに 1 セル置く
}
pub struct Projectile {
    pub id: ProjectileId,
    pub x16: i64, pub y16: i64,    // 1/16 セル
    pub vx: i16, pub vy: i16,      // 1 tick あたり 1/16 セル
    pub gravity: i16,              // 毎 tick vy に足す。0 なら直進
    pub life: u16,                 // 残り tick。0 になったら payload なしで消える
    pub damage: i32,               // 直接当たったエンティティへのダメージ
    pub payload: Payload,
}
pub struct ProjectileWorld { /* BTreeMap<ProjectileId, Projectile>、next_id */ }
```

- `ProjectileWorld::spawn(..) -> Result<ProjectileId, ProjectileError>`: `life` は 1..=3600、`|vx|`・`|vy|` は 512 以下、`Explode` の半径は 1..=16、`Deposit` の材質は `world` の材質表にあること。違反は `ProjectileError::{InvalidLife, InvalidSpeed, InvalidRadius, UnknownMaterial}`。ID は `checked_add` で払い出す。同時に存在できるのは 1024 個まで (`TooMany`)。
- `step(&mut self, world, physics, entities, vitals) -> Vec<Hit>` を 1 tick に 1 回呼ぶ。弾を ID 昇順に処理する。
  1. `vy = clamp(vy + gravity, -512, 512)`。
  2. `n = max(|vx|, |vy|)` を 1/16 セルの歩数とし、`k = 1..=n` 歩目で位置を `start + round_toward_zero(v * k / n)` (整数除算) に置く。各歩で、弾の点が入るセルを調べる。
  3. **セルに当たる**: `BOUNDARY`、`FLAG_BODY`、または `phase` が `Solid`・`Powder`・`Liquid` のセル。気体・空は通る。
  4. **エンティティに当たる**: 弾の点が、`vitals` に登録されたエンティティの半開区間の当たり判定 (07 §2.2) に入る。複数ならエンティティ ID が最小のもの。`damage` を `Vitals::damage` に渡す。
  5. 当たったら、その 1 歩前の位置 (一度も進めていなければ出発点) を `last_free` とし、`payload` を実行して弾を消す。`life` は毎 tick 末尾に 1 減らし、0 になったら消える。
- `Payload::Explode`: `charon_physics::explosion::explode(world, physics, center = 当たったセル (エンティティに当たった場合は弾のセル), radius)` を呼ぶ。続けて `explosion_damage` を、当たり判定が爆風の円と交わる全エンティティ (ID 昇順) に適用する。
- `explosion_damage(radius, dist2, max_damage) = max_damage * (r*r - dist2) / (r*r)` (整数除算、`dist2 >= r*r` なら 0)。`dist2` は爆発の中心セルから当たり判定の矩形 (セル単位の閉区間) への最短距離の 2 乗。`max_damage` は `Explode` の弾では `damage * 2` とする。
- `Payload::Deposit`: `last_free` のセルが `EMPTY` で `FLAG_BODY` がないときだけ `world.make_cell(material, pos)` を `World::set` で置く。置けなければ何もしない (セルを増やさない・消さない)。
- `Hit { projectile, entity: Option<EntityId>, cell: CellPos }` を発生順に返す。
- `state_hash()`: ID 昇順に全フィールドを FNV-1a 64-bit (リトルエンディアン) で流し、`next_id` も含める。

| テスト | 確認 |
|---|---|
| `projectile_flies_and_falls` | `gravity > 0` の弾が放物線を描いて床に当たり、`gravity = 0` は直進する。結果は seed に依らない |
| `projectile_explodes_on_solid` | 壁に当たると半径内のセルが消えてパーティクルになり、前景のセル数とパーティクル数の合計が爆発の前後で等しい |
| `projectile_hits_entity` | エンティティに当たると `damage` だけ `hp` が減り、`Hit.entity` が入る。ID が小さい方が優先される |
| `explosion_damage_falls_off` | 中心に近いほどダメージが大きく、半径の外は 0。壁に隠れても爆風の距離だけで決まる (遮蔽はしない) |
| `projectile_expires_and_deposits` | `life` が尽きた弾は payload なしで消える。`Deposit` は空きセルにだけ 1 個置き、塞がっていれば何も置かない |
| `projectile_validation_and_limit` | 不正な引数と 1025 個目をエラーにし、状態は変わらない |

## 5. T11.3: 杖と呪文 (`charon-game::wand`)

### 5.1 呪文の表 (`SpellTable`)

`SpellTable::parse(text, materials, namespace) -> Result<SpellTable, SpellError>` は RON の配列を読む。`namespace` が `Some(id)` なら呪文名を `id:name` にする (MOD と同じ規則、仕様 07 §6)。

```ron
[
    (name: "spark", mana_cost: 5, cast_delay: 6, speed: 128, gravity: 0, life: 60,
     damage: 8, spread: 0, payload: None),
    (name: "bomb", mana_cost: 30, cast_delay: 30, speed: 96, gravity: 2, life: 120,
     damage: 20, spread: 8, payload: Explode(radius: 6)),
    (name: "sand_shot", mana_cost: 10, cast_delay: 10, speed: 96, gravity: 2, life: 90,
     damage: 0, spread: 16, payload: Deposit(material: "sand")),
]
```

- 検査 (違反は `SpellError::Invalid { spell, reason }`、検査は名前の昇順ではなく記載順で最初の違反): `name` は ASCII 英数字・`_` の 1..=64 文字、`mana_cost` 0..=10000、`cast_delay` 0..=600、`speed` 1..=512、`gravity` -64..=64、`life` 1..=3600、`damage` 0..=10000、`spread` 0..=128、`Explode` の半径 1..=16。呪文は 1..=256 個、名前の重複は `SpellError::Duplicate(name)`、`Deposit` の材質名が表になければ `SpellError::UnknownMaterial { spell, material }`。構文エラーは `SpellError::Parse`。
- 読み込んだ呪文は `SpellId(u16)` (記載順の添字) で参照する。`get(id)`、`id(name)`、`len()`、`iter()` を提供する。
- MOD のディレクトリから `spells.ron` を自動で読む処理は、仕様 07 §6 の変更になるため T11.3 では行わない。呼び出し側が `parse` に渡す。

### 5.2 杖 (`Wand`)

```rust
pub struct Wand {
    pub spells: Vec<SpellId>, // 1..=16
    pub mana: u32, pub mana_max: u32, // mana_max 1..=10000
    pub mana_regen: u16,      // 毎 tick の回復量
    pub recharge: u16,        // 最後の呪文のあと追加で待つ tick
    cursor: usize, cooldown: u16,
}
```

- `Wand::new(spells, mana_max, mana_regen, recharge, table) -> Result<Wand, WandError>`: 呪文が 1..=16 個、ID がすべて表にある、`mana_max` が 1..=10000、でなければエラー。`mana = mana_max`、`cursor = 0`、`cooldown = 0`。
- `Wand::tick(&mut self)`: `mana = min(mana_max, mana + mana_regen)`、`cooldown` を 1 減らす (0 で止まる)。1 tick に 1 回、`cast` より前に呼ぶ。
- `Wand::cast(&mut self, table, projectiles, world, muzzle: (i64, i64), aim: (i32, i32), tick, seed) -> Result<CastOutcome, CastError>`:
  1. `cooldown > 0` なら `Ok(Cooling)`。`mana < mana_cost` なら `Ok(NoMana)`。どちらも杖と弾を変えない。
  2. `aim` は 1/16 セル単位の向きのベクトル。`(0, 0)` や各成分の絶対値が 1,000,000 を超えるものは `CastError::InvalidAim`。`len = isqrt(ax² + ay²)` (`u64`、切り捨て、0 なら 1)。`dir_v = aim * speed / len` (各成分、0 方向へ丸める整数除算)。
  3. 散らし: `spread > 0` なら `off = (rng::hash(seed, tick, muzzle_x_cell, muzzle_y_cell, SALT_SPREAD) % (2*spread + 1)) - spread`。`perp = (-aim_y, aim_x)`、`v = dir_v + perp * speed * off / (256 * len)` (各成分、`i64`、0 方向へ丸め)。結果は `i16` の ±512 に `clamp`。
  4. `projectiles.spawn(world, muzzle.0, muzzle.1, vx, vy, gravity, life, damage, payload)` を呼ぶ。失敗したら `CastError::Projectile(e)` を返し、杖は何も変えない。
  5. 成功したら `mana -= mana_cost`、`cursor = (cursor + 1) % len`、`cooldown = cast_delay`。`cursor` が 0 に戻ったときは `cooldown += recharge` (`u16` の上限で止める)。`Ok(Fired(id))`。
- 乱数は `rng::hash` のみ (`SALT_SPREAD = 0x5350_5244`)。時刻・浮動小数点を使わない。`Wand::state_hash()` は全フィールドを FNV-1a 64-bit (リトルエンディアン) で流す。

| テスト | 確認 |
|---|---|
| `spell_table_parses_and_resolves` | 材質名が ID になり、`namespace` が名前に付き、記載順の `SpellId` で引ける |
| `spell_table_rejects_bad_data` | 重複・未定義の材質・範囲外の値・構文エラー・0 個と 257 個を拒否する |
| `wand_casts_in_order_and_recharges` | 呪文が順に撃たれ、`cast_delay` の間は `Cooling`、最後の呪文のあとは `recharge` が足される |
| `wand_mana_limits_casting` | マナ不足は `NoMana` で何も変わらず、回復すれば撃てる。`mana_max` を超えて回復しない |
| `aim_sets_velocity` | `(3, 4)` で速度 100 なら `(60, 80)`、負の向きも同じ式。`(0, 0)` は `InvalidAim` |
| `spread_is_deterministic` | 同じ入力で同じ速度。範囲内で、tick が違うと値が散る |
| `failed_cast_changes_nothing` | 弾の上限などで `spawn` が失敗すると、杖のマナ・カーソル・待ち時間が変わらない |

## 6. 後続 (T11.4〜T11.6) の方針

- **T11.4 敵**: `Vitals` を持つエンティティに、整数だけの AI (向かう・跳ぶ・間合いで撃つ) を足す。乱数は `rng::hash`。
- **T11.5 掘る道具・浮遊**: 掘るは `Payload` の一種 (半径内のセルをパーティクルへ移す)。浮遊は `PlayerInput` の拡張 (燃料を消費し、`vy` を上向きに足す)。
- **T11.6 組み込み**: `Lockstep` の tick 順序に vitals・projectile を入れ、サンドボックス・デモで操作できるようにする。
- いずれも着手前に、この文書へ表のテスト名と式を書く。
