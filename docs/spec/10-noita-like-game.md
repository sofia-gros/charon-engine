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
- MOD のディレクトリから `spells.ron` を読む処理と、既定の定義・層・上書きは §7 (T11.7)。この節の `parse` は §7 の `layer` の 1 層分にあたる。

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

## 6. T11.6: ゲーム状態の組み込み (`charon-game::game`、`Lockstep`、デモ)

敵 (T11.4) より先に、体力・弾・杖を 1 つの tick の流れに入れ、画面で撃てるようにする。当たり判定の確認 (エンティティの当たり判定、弾の貫通、爆風) を、実際の描画と操作で行うため。

### 6.1 `GameState`

```rust
pub struct GameInput { pub fire: bool, pub aim: (i32, i32) } // aim は向きのベクトル (1/16 セル単位でも整数ならよい)
pub struct GameState {
    pub vitals: VitalsTable,
    pub projectiles: ProjectileWorld,
    pub wands: BTreeMap<EntityId, Wand>,
    pub spells: SpellTable,
}
```

- `GameState::new(spells)`、`add_character(id, vitals, wand)`。`wands` は `EntityId` の昇順。
- `GameState::step(&mut self, world, physics, entities, actions: &[(EntityId, GameInput)], tick, seed) -> Result<Vec<Hit>, GameError>` を 1 tick に 1 回呼ぶ。順序は固定で、(1) ID 昇順に `step_environment` (生存中のキャラクターだけ)、(2) ID 昇順に杖の `tick`、(3) `fire` が真で生存中なら ID 昇順に `Wand::cast` (発射位置 = エンティティの当たり判定の中心、`x16 + width*8`、`y16 + height*8`)、(4) `ProjectileWorld::step`。`actions` に存在しない ID、同じ ID の重複はエラー (`GameError::InvalidAction`)で、何も更新しない。杖が `Err(CastError::InvalidAim)` を返す (狙いが `(0, 0)`) ときは何も撃たずに続ける。他の `CastError` は `GameError::Cast` にして返す。
- 発射した弾の始点が自分の当たり判定の内側にあっても、自分の体力は減らさない: 弾は発射した ID を持たない代わりに、始点のエンティティは `ProjectileWorld::step` の「エンティティに当たる」判定から、発射した tick に限って除く。(`Projectile::grace: Option<EntityId>` を 1 tick 分だけ持つ。)
- `state_hash()`: `vitals.state_hash()`、`projectiles.state_hash()`、各杖の `state_hash()` (ID 昇順) を FNV-1a 64-bit で連結する。

### 6.2 `Lockstep`

- パケットを version 2 にする。`TickInput` に `action: GameInput` を足し、長さは 29 バイト (従来の 20 バイトの後ろに `fire: u8` (0 か 1)、`aim_x: i32` (LE)、`aim_y: i32` (LE))。`fire` が 0・1 以外、または `|aim|` が 1,000,000 を超えたら `InvalidPacket`。version 1 のパケットは `Mismatch`。
- `ConnectionInfo` に `spells: Vec<String>` (呪文名を記載順に) を足し、`with_spells(&SpellTable)` で設定する。`ConnectionInfo::new` は空で作る。呪文表が違う接続は `connect` で `Mismatch` になる。
- `advance(world, entities, physics, background, game)`。順序は **規則 → プレイヤー → `game.step` → 物理/セル**。同期ハッシュ (`synchronization_hash`) の末尾に `game.state_hash()` を足す。`game.wands` に登録されていない参加者には、中立の入力だけを許す。その参加者が `fire` や狙いを送ってきたら、何も更新せずに `LockstepError::Game(InvalidAction)` で拒否する。

### 6.3 デモ・サンドボックス

- デモでは、プレイヤーに体力・杖を持たせる (値と呪文はゲーム側の定義、§7.4)。**右ボタンを押している間、カーソルの方向へ撃つ** (狙いはプレイヤーの中心からカーソルのセルの中心への向き)。杖は呪文の並びを順に撃つ。UI が右ボタンの押下を使ったときは撃たない。自由カメラ (`F`) の間は撃たない。
- 画面の上端の中央に **体力・息・マナのバー、次の呪文の名前、飛んでいる弾の数** を出す (`Wand` パネル。体力 0 のときは `DEAD`)。体力 0 のプレイヤーは入力を受けない (歩けず、撃てない)。
- 弾はエンティティの描画パスの 2×2 セルの点として描く (色は呪文ごとではなく固定)。爆発はパーティクルが散る。
- 世界を作り直す (`Main Menu`) と、体力・杖・弾は初期状態に戻る。
- `Sim::tick` が `GameState` を借りて `step` を呼ぶ。画面なしのテストと描画ループは同じ手順を使う。

| テスト | 確認 |
|---|---|
| `game_step_order_is_fixed` | 環境ダメージ → 杖の tick → 発射 → 弾の移動の順で、同じ入力から毎 tick のハッシュが一致する (seed 1〜8) |
| `game_rejects_bad_actions_atomically` | 存在しない ID・同じ ID の重複はエラーで、何も変わらない。`aim = (0, 0)` は撃たずに続ける |
| `shooter_is_not_hit_by_own_shot` | 発射した tick に始点のエンティティへ当たらない。次の tick 以降は当たる |
| `dead_characters_do_not_fire` | 体力 0 のキャラクターは発射しない・環境ダメージを受けない |
| `lockstep_packet_v2_roundtrip` | 29 バイトの符号化・復号が一致し、不正な `fire`・大きすぎる `aim`・version 1 を拒否する |
| `lockstep_game_hash_is_order_independent` | 到着順が違っても `game` を含む毎 tick のハッシュが一致し、呪文表が違う接続は拒否される |
| `demo_wand_fires_in_scene` | デモの世界で右ボタン相当の入力が弾を生み、爆発でセルがパーティクルになる (画面なし) |
| `demo_renders_wand_panel` | RTX 4060 のオフスクリーン描画で `Wand` パネルと弾を含む画面を描ける |

ウィンドウでの右ボタン操作は、画面操作の許可がなければ未検証として記録する。

## 7. T11.7: エンジンの定義層とゲーム側の追加・上書き (`charon-game::defs`)

**原則**: このリポジトリは**エンジン**で、ゲームではない。エンジンは (1) 呪文・杖・敵の**既定の定義**を RON で持ち、(2) それを読み込み・追加・上書きする **API** を提供する。武器も敵も、ゲーム側がその API を呼んで使う。デモ (`charon-demo`) も「ゲーム」の 1 つであり、デモ固有の呪文・杖・体力はエンジンのコードに書かず、ゲーム側の RON と、`AppOptions` で渡す設定だけで表す。材質・反応の `assets/materials.ron`・`assets/reactions.ron` (組み込み) と MOD による追加 (仕様 07 §6) と同じ形にそろえる。

### 7.1 定義の種類と形式

| ファイル | 内容 | 既定 (エンジン、`assets/`) |
|---|---|---|
| `spells.ron` | 呪文 (§5.1 の形式) | `spark`、`bomb`、`sand_shot`、`water_shot` |
| `wands.ron` | 杖のひな形 | `basic` (`spark`)、`bomber` (`bomb`) |
| `enemies.ron` | 敵の種類 (データのみ。動きは T11.4) | `walker`、`shooter` |

```ron
// wands.ron
[(name: "basic", spells: ["spark"], mana_max: 100, mana_regen: 1, recharge: 0)]
// enemies.ron
[(name: "shooter", width: 8, height: 16, max_hp: 30, walk_speed: 16, jump_speed: 80,
  sight: 160, attack: Some("spark"), attack_interval: 60)]
```

- どの項目にも、省略できる `replace: bool` (既定は `false`) がある。
- 杖: `spells` は 1..=16 個の呪文名、`mana_max` 1..=10000、`mana_regen` 0..=65535、`recharge` 0..=65535 (省略時 0)。
- 敵: `width`・`height` 1..=32、`max_hp` 1..=10000、`walk_speed` 0..=96、`jump_speed` 0..=96 (1/16 セル / tick)、`sight` 0..=1024 (セル)、`attack` は呪文名か `None`、`attack_interval` 0..=3600 (`attack` があるときは 1 以上)。
- 範囲の外、構文エラーは、定義の名前を含むエラーにする (`DefsError`)。

### 7.2 層 (レイヤー) と上書き

`Definitions { spells, wands, enemies }` は、定義を**層で重ねる**。

- `Definitions::engine_defaults(&MaterialTable) -> Result<Definitions, DefsError>`: 埋め込んだ既定 (§7.1) を読む。
- `Definitions::layer(&mut self, files: DefinitionFiles, materials, namespace: Option<&str>) -> Result<(), DefsError>`: `DefinitionFiles { spells, wands, enemies }` (それぞれ RON 文字列の `Option`) を 1 層として重ねる。**失敗したときは `self` を変えない**。
  - `replace: false` (既定): 名前は `namespace` が `Some(id)` なら `id:name` になる。すでにある名前なら `DefsError::Duplicate`。新しい定義は末尾に足す。
  - `replace: true`: 名前は**そのまま** (名前空間を付けない) で、すでにある定義でなければ `DefsError::ReplaceMissing`。定義の中身を置き換えるが、**呪文は表の中の位置 (`SpellId`) を保つ**ので、すでに作った杖は壊れない。
  - 1 つの層の中で同じ名前を 2 回書いたら `Duplicate`。
- 杖と敵が参照する呪文名と、呪文の `Deposit` が参照する材質名は、まず `名前空間:名前`、次にそのままの名前の順で探す (仕様 07 §6 の材質と同じ)。層を重ねる時点で解決でき、見つからなければ `DefsError::UnknownSpell` / `UnknownMaterial`。同じ層で足す呪文を参照してよい。
- 取り出し: `Definitions::wand(&self, name) -> Result<Wand, DefsError>` は名前から杖を作る (`Wand::new`)。`enemy(&self, name) -> Option<&EnemyDef>`、`wand_names()`、`enemy_names()`。名前は重なった結果の名前 (`id:name` を含む)。
- `Definitions::content_hash() -> u64`: 全定義の中身を FNV-1a 64-bit で流す。上書きで中身が変わると、名前が同じでもハッシュが変わる。

### 7.3 MOD からの読み込み (仕様 07 §6 の変更)

データ MOD のディレクトリは、`manifest.ron`・`materials.ron`・`reactions.ron` に加えて、**省略できる** `spells.ron`・`wands.ron`・`enemies.ron` を読む。`ModPack::load` は `Definitions::engine_defaults` を作り、MOD を **MOD ID の昇順**で 1 層ずつ、`namespace = Some(MOD の ID)` で重ねる。結果は `ModPack::definitions`。ファイルがなければその種類の層は空。読み込みの失敗は材質と同じく起動全体のエラー (`ModError::Definitions`)。

### 7.4 ゲームへの渡し方 (`charon-app`・デモ)

- `GameState::from_definitions(&Definitions)` は呪文表を取り込む。
- `AppOptions` に `definitions: Option<Arc<Definitions>>` と `player: Option<PlayerLoadout>` を持たせる。`PlayerLoadout { max_hp: i32, wand: String }`。プレイヤーの生成時に、`definitions.wand(&loadout.wand)` で杖を作り、`Vitals::new(max_hp)` と合わせて `GameState::add_character` する。定義が足りないとき (杖の名前がない、など) は報告して、体力と杖なしで続ける。
- エンジンの実行部分 (`charon-app` の `lib.rs`・`sim.rs`・`ui.rs`) には、呪文名・杖の名前・体力・マナの値を**書かない**。デモは「ゲーム側」で、`assets/mods/fse_demo/` に `spells.ron`・`wands.ron` を置き、`PlayerLoadout` を `charon-app` の `demo.rs` (デモのエントリー) で組み立てる。
- `ConnectionInfo` は、呪文名の一覧の代わりに `definitions: u64` (`content_hash`) を持ち、`with_definitions(&Definitions)` で設定する。定義の中身が違う接続は `connect` で `Mismatch` になる。

| テスト | 確認 |
|---|---|
| `engine_defaults_load` | 既定の呪文・杖・敵がすべて読め、`wand` で杖が作れ、`SpellId` が記載順 |
| `layer_adds_and_replaces` | 名前空間付きで追加され、`replace: true` は中身だけ置き換えて `SpellId` を保つ。置き換え前に作った杖が新しい中身で撃てる |
| `layer_rejects_bad_data` | 重複・`replace` の対象なし・未定義の呪文/材質・範囲外・構文エラーを拒否し、失敗した層は何も変えない |
| `content_hash_tracks_overrides` | 上書きで名前が同じでもハッシュが変わり、同じ層を同じ順で重ねれば一致する |
| `mod_definitions_load_in_id_order` | MOD のファイルが ID 昇順に重なり、列挙順に依存しない。ファイルがなくても読める |
| `lockstep_rejects_different_definitions` | 定義の中身が違う接続を拒否する (`lockstep_game_hash_is_order_independent` を置き換える) |
| `demo_definitions_come_from_the_game_files` | デモの杖は `fse_demo` の RON から作られ、エンジン既定の `bomb` を上書きした結果が反映される |

## 8. 後続 (T11.4〜T11.5) の方針

- **T11.4 敵**: `EnemyDef` (§7.1) から、`Vitals` を持つエンティティを `spawn_enemy(defs, name, position)` の API で作り、整数だけの AI (向かう・跳ぶ・間合いで撃つ) で動かす。乱数は `rng::hash`。T11.6 の画面で当たり判定を確認してから着手する。
- **T11.5 掘る道具・浮遊**: 掘るは `Payload` の一種 (半径内のセルをパーティクルへ移す)。浮遊は `PlayerInput` の拡張 (燃料を消費し、`vy` を上向きに足す)。
- いずれも着手前に、この文書へ表のテスト名と式を書く。
