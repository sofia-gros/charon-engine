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

- デモでは、プレイヤーに体力・杖を持たせる (値と呪文はゲーム側の定義、§7.4)。**左ボタンを押している間、カーソルの方向へ撃つ** (T11.8 で、右ボタンはワイヤーフックの道具になった。§9.5)。狙いはプレイヤーの中心からカーソルのセルの中心への向き。杖は呪文の並びを順に撃つ。UI が押下を使ったときと、自由カメラ (`F`)・描画モードの間は撃たない。
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
| `demo_wand_fires_in_scene` | デモの世界で左ボタン相当の入力 (`fire`) が弾を生み、爆発でセルがパーティクルになる (画面なし) |
| `demo_renders_wand_panel` | RTX 4060 のオフスクリーン描画で `Wand` パネルと弾を含む画面を描ける |

ウィンドウでのボタン操作は、画面操作の許可がなければ未検証として記録する。

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

## 8. T11.4: 敵 (`charon-game::game`、`defs`)

敵の**種類**は §7 の `EnemyDef` (エンジンの既定とゲームの追加)、**生成**はゲームが API (`spawn_enemy`) で呼ぶ。動き・攻撃・接触ダメージ・撤去はエンジンの AI が決める。整数だけで計算し、乱数は `rng::hash` だけ。

### 8.1 `EnemyDef` の追加項目

- `contact_damage`: 省略できる (既定 0)。0..=1000。目標に当たり判定が重なっているとき、`CONTACT_INTERVAL = 30` tick ごとに与えるダメージ。`enemies.ron` に `contact_damage: 5` と書く。エンジンの既定の `walker` は 5、`shooter` は 0。内容ハッシュ (§7.2) に含める。

### 8.2 生成 API

- `EntityWorld::spawn_checked(x16, y16, width, height, world) -> Result<EntityId, EntityError>`: `spawn` の検査に加え、固体 (`collides`) と重なるなら `Occupied` で、ID を消費せず何も変えない。`spawn_player` はこれの 8×16 版にする。
- `GameState::spawn_enemy(&mut self, defs: &Definitions, entities: &mut EntityWorld, world: &World, name: &str, cell: (i32, i32)) -> Result<EntityId, GameError>`: `cell` は左上のセル。順に、(1) `defs.enemy(name)` がなければ `GameError::UnknownEnemy`、(2) `attack` の呪文が `self.spells` にあるか (なければ `GameError::UnknownSpell`)、(3) `spawn_checked` (失敗は `GameError::Entity`)、(4) `Vitals::new(max_hp)`、(5) 攻撃があれば、その呪文 1 つだけの杖 (`mana_max = 10000`、`mana_regen = 10000` で常に満タン) を作り、敵として登録する。**失敗したときは何も変えない**。敵は種類の定義の写しを持つ (あとで定義を上書きしても、動いている敵は変わらない)。

### 8.3 1 tick の AI (`GameState::step` の (4))

`GameState::step` は `entities: &mut EntityWorld` を受け取り、順序を **(1) 環境ダメージ → (2) 杖の回復 → (3) プレイヤーの発射 → (4) 敵 (ID 昇順) → (5) 弾の移動 → (6) 敵の撤去** にする。戻り値は `StepReport { hits: Vec<Hit>, defeated: Vec<EntityId> }`。

敵 1 体の処理 (体力 0 なら何もしない):

1. `attack_cooldown`、`contact_cooldown`、杖の待ち (`Wand::tick`) を 1 減らす (0 で止まる)。
2. **目標**: 杖を持つ生存中のキャラクター (`self.wands` の ID) のうち、中心のセル距離 `dx² + dy² <= sight²` で、**視線が通る**もの。最も近いもの (同距離なら ID が小さいもの)。視線: 敵の中心セルから目標の中心セルまでの整数の線分 (Bresenham) の途中のセル (両端を除く) が、`BOUNDARY`・`FLAG_BODY`・固体・粉体のどれでもないこと (液体・気体・空は通る)。
3. **移動の入力** (`dx` = 目標の中心 x − 自分の中心 x、セル):
   - 目標なし: 45 tick ごとに、`horizontal = hash(seed, tick / 45, id as i32, 0, SALT_WANDER) % 3 - 1` (`SALT_WANDER = 0x574E_4452`)。ジャンプなし。
   - 目標あり、攻撃なし (接近戦): 当たり判定が重なるまで近づく。`|dx| > max((自分の幅 + 目標の幅) / 2 - 2, 0)` のとき `sign(dx)`、そうでなければ 0 (中心の距離が幅の半分の和より 2 セル近いところで止まるので、箱が重なる)。
   - 目標あり、攻撃あり: `stand_off = sight / 2`。`|dx| > stand_off` なら `sign(dx)`、`|dx| < stand_off / 2` なら `-sign(dx)` (後退)、それ以外は 0。
   - ジャンプ: 接地していて、(a) `horizontal != 0` で、進む向きの 1 セル先で当たり判定が固体に重なる、または (b) 目標の上端が自分の上端より 4 セル以上上にあり `|dx| <= 4`。
4. **移動**: `step_entity(entity, PlayerInput { horizontal, jump }, walk_speed, jump_speed, world)` (§8.4)。
5. **攻撃** (`attack` があり、目標があり、`attack_cooldown == 0`): 狙いは**自分の中心から目標の中心への向き** (1/16 セル単位)。`Wand::cast` で撃ち、`Fired` なら弾の `grace` を自分にし、`attack_cooldown = attack_interval`。`Cooling` / `NoMana` / `InvalidAim` では何もしない (次の tick に再試行)。
6. **接触** (`contact_damage > 0`、`contact_cooldown == 0`): 当たり判定が重なる生存中のキャラクターのうち ID が最小のもの 1 体に `contact_damage` を与え、`contact_cooldown = CONTACT_INTERVAL`。

撤去: (5) のあと、体力 0 の敵を `entities`・`vitals`・敵の表から消し、`StepReport::defeated` に ID を昇順で入れる。ID は再利用しない。プレイヤーは撤去しない。敵の弾は敵にも当たる (チームなし)。

### 8.4 移動

`step_entity(entity, input, walk_speed, jump_speed, world)` は `step_player` の移動を、速さを引数にしたもの: `vx = horizontal * walk_speed`、接地中にジャンプなら `vy = -jump_speed`。`step_player` は `step_entity(.., 32, 96, ..)`。衝突の判定は仕様 07 §2.2 のまま。

### 8.5 ゲーム側から使う

`AppOptions::enemies: Vec<EnemySpawn>` (`EnemySpawn { name: String, cell: [i32; 2] }`) をゲームが渡す。エンジンの実行部分は、周囲のチャンクが読み込まれたら `spawn_enemy` を呼ぶ (プレイヤーの生成と同じ条件)。失敗は報告して、その敵だけ諦める。デモは `demo.rs` で部屋に `walker` と `shooter` を置く。`Wand` パネルに敵の数を出す。

| テスト | 確認 |
|---|---|
| `enemy_spawns_from_definition` | 定義の大きさ・体力で生成され、未定義の敵・呪文・固体との重なりは何も変えずにエラー。ID は単調増加 |
| `enemy_chases_target_in_sight` | 視界内の目標へ近づき、視界の外の敵は近づかない |
| `enemy_needs_line_of_sight` | 壁の向こうの目標は追わず撃たず、壁を外すと撃つ |
| `enemy_jumps_over_step` | 低い段差を跳び越えて目標に届く |
| `shooter_fires_and_hits_target` | 攻撃のある敵が間隔どおりに撃ち、目標の体力が減る。撃った tick に自分には当たらない |
| `enemy_contact_damage_repeats` | 接触ダメージが `CONTACT_INTERVAL` ごとに入る |
| `defeated_enemies_are_removed` | 体力 0 の敵は撤去され、`defeated` に ID が入り、ID は再利用されない。プレイヤーは残る |
| `enemy_ai_is_deterministic` | seed 1〜8 で、同じ入力から毎 tick の `state_hash` が一致する (徘徊を含む) |
| `step_entity_uses_given_speeds` | `walk_speed`・`jump_speed` どおりに動き、`step_player` の結果は変わらない |
| `demo_enemies_spawn_in_scene` | デモの世界にゲーム側の指定どおり敵が生まれ、決定的に動く (画面なし) |

## 9. T11.8: ワイヤーフックと装備の切り替え (`charon-physics::rope`、`charon-game::game`)

ワイヤーフックは**道具**で、**右手**に持つ。右クリックで、マウスの方向へ**物理挙動するワイヤー**を撃つ。**左手は武器** (杖) で、左クリックで撃つ。2 つは独立していて、同時に使える (手は操作の割り当ての概念で、見た目には反映しない)。ワイヤーは重力でしなり、地形に食い込まない。先端が地形に当たって止まったら、ワイヤーが張った分だけプレイヤーを引っ張る。右ボタンを押している間は巻き取り、離すと外れる。ワイヤーの物理は浮動小数点 (`f32`、位置はセル単位) で、剛体と同じく `charon-physics` に置く (CLAUDE.md の例外)。同一プラットフォーム・同一ビルドでは同じ入力から同じ結果になるが、異なる環境の一致は主張しない (仕様 07 §7 と同じ)。

### 9.1 ワイヤーの物理 (`charon_physics::rope`)

```rust
pub struct RopeParams { pub segments: usize, pub max_length: f32, pub min_length: f32,
    pub gravity: f32, pub iterations: usize, pub reel_speed: f32 }   // 長さはセル、速さはセル / tick
pub enum RopePhase { Flying, Anchored }
pub enum RopeEvent { None, Anchored, Lost }
pub struct Rope { /* 点列、前の点列、phase、rest (静止長)、先端の速度 */ }
```

- `Rope::launch(hand, direction, speed, &params) -> Option<Rope>`: `direction` が 0 または有限でなければ `None`。点は `segments + 1` 個 (4..=64 個のセグメント)。最初は全点が `hand` にあり、先端 (最後の点) の速度は `direction` を正規化して `speed` 倍にしたもの。`phase = Flying`。
- `Rope::step(&mut self, hand, reel, &params, solid) -> RopeEvent` を 1 tick に 1 回呼ぶ。`solid(point) -> bool` は点がいるセルが地形か (境界・剛体・固体・粉体)。
  1. **飛行 (`Flying`)**: 先端の速度に `gravity` を足し、位置を 0.5 セル以下の小刻みに進める。途中で `solid` が真になったら、その直前の位置に先端を置き、`phase = Anchored`、`rest = min(多角線の長さ, max_length)` (下限 `min_length`) とし、`RopeEvent::Anchored`。進み終えたときに手と先端の距離が `max_length` を超えたら `RopeEvent::Lost` (呼び出し側が外す)。
  2. **巻き取り (`Anchored`)**: `reel` が真なら `rest = max(min_length, rest - reel_speed)`。
  3. **しなり**: 点 0 は常に `hand`、最後の点 (先端) は動かさない。それ以外の点は Verlet 法 (`p' = p + (p - prev) * 0.995 + (0, gravity)`) で動かし、続いて `iterations` 回、隣り合う点の距離を `seg = rest / segments` (飛行中は `max(手と先端の距離, 0.01) * 1.03 / segments`) に近づける拘束を、点 0 から順に (端の点は動かさずに) 解く。
  4. **衝突**: 動かした各点が `solid` の中にいたら、動かす前の位置に戻し、その点の速度を 0 にする (地形に食い込まない)。
- 取り出し: `points()`、`phase()`、`rest_length()`、`polyline_length()`、`anchor() -> Option<[f32; 2]>` (`Anchored` のときの先端)、`state_hash()` (全点・phase・rest のビット列を FNV-1a 64-bit で流す)。
- `tether_velocity(rope, hand, velocity, reel, pull_accel, speed_cap) -> [f32; 2]`: プレイヤー (または任意の質点) の速度 (セル / tick) を、ワイヤーの拘束で直す。`Anchored` のときだけ働く (飛行中は `velocity` をそのまま返す)。(1) **張っている** (手とアンカーの**直線距離**が `rest_length() - 0.05` 以上。たるんで垂れ下がり、折れ線が伸びきったワイヤーを張っているとは見なさない) ときに `reel` が真なら、手から 3 セル以上離れた最初の点 (なければアンカー) への単位ベクトル `dir` を使って `dir * pull_accel` を足す。(2) 次の位置 `hand + velocity` がアンカーを中心とする半径 `rest_length()` の円の外に出るなら、**円の上に戻す** (ワイヤーは伸びない。振り子と、行き過ぎの修正を兼ねる)。(3) 結果の大きさを `speed_cap` 以下に収める。次の位置が円の中 (たるんでいる) なら (1) 以外は何も変えない。

### 9.2 エンティティの外力速度 (`charon-game`)

`Entity` に `ext_vx: i16` (1/16 セル / tick、既定 0) を足す。`step_entity` は `vx = horizontal * walk_speed + ext_vx` (各成分 ±512 に収める) で動き、x 方向に衝突したら `ext_vx = 0`。動いたあと、`ext_vx` を接地中は `* 3 / 4`、空中は `* 15 / 16` (0 方向へ丸める整数除算) にする。`ext_vx = 0` なら従来どおり (`step_player` の結果は変わらない)。`state_hash` に `ext_vx` を含める。

### 9.3 定義 (`hooks.ron`) とエンジンの既定

仕様 §7 の層・`replace`・名前空間の規則を `hooks.ron` にも使う (エンジンの既定は `assets/hooks.ron`、MOD に省略できる `hooks.ron`)。

```ron
[(name: "grapple", max_length: 200, min_length: 4, speed: 320, segments: 24, gravity: 6,
  iterations: 8, reel_speed: 24, pull_accel: 12, pull_max: 128, cooldown: 20)]
```

- `max_length` 4..=400 (セル)、`min_length` 2..=64 (セル、`max_length` 以下)、`speed` 16..=512、`gravity` 0..=64、`reel_speed` 0..=96、`pull_accel` 0..=64、`pull_max` 1..=192 (いずれも 1/16 セル / tick、`gravity`・`pull_accel` は 1/16 セル / tick²)、`segments` 4..=64、`iterations` 1..=32、`cooldown` 0..=600 (tick)。範囲外は名前を含む `DefsError::Invalid`。
- `Definitions::hook(name) -> Option<&HookDef>`、`hook_names()`。内容ハッシュ (§7.2) に含める。エンジンの既定は `grapple` 1 つ。

### 9.4 手の割り当てと 1 tick の処理 (`GameState`)

- **左手 = 武器 (杖)、右手 = 道具 (ワイヤーフック)**。`GameInput { fire, tool, aim }`: `fire` は左クリック (武器、§6.1 と同じ)、`tool` は右クリック (道具)、`aim` は 2 つで共通のマウスの方向。2 つは独立で、同じ tick に両方を使える。
- `GameState::equip_hook(id, &Definitions, name) -> Result<(), GameError>` でフックを持たせる。`name` が定義にないと `GameError::UnknownHook(name)`、`id` が杖を持つキャラクターでなければ `InvalidAction`。フックだけで杖のないキャラクターは、このタスクでは扱わない。
- `tool` の扱い (ワイヤーがないキャラクターでは何もしない):
  - `tool` が偽から真に変わった tick に、ワイヤーが出ておらず `cooldown == 0` なら、`aim` の向きに `Rope::launch` (手 = 当たり判定の中心のセル座標、`speed` = 定義の `speed / 16`)。狙いが `(0, 0)` なら撃たない。
  - ワイヤーが出ていて `tool` が偽 (離した)、またはキャラクターが倒れた、または `Lost` になったら、ワイヤーを外して `cooldown = 定義の cooldown`。
  - `Anchored` で `tool` が真なら巻き取り (`reel`)。
- 毎 tick、ワイヤーのあるキャラクターを ID 昇順に、プレイヤーの発射 (§6.1 の (3)) のあとに更新する: `Rope::step` → `Anchored` なら `tether_velocity` でプレイヤーの速度を直し、`vy` と `ext_vx` に書き戻す (`vy` は ±256、`ext_vx` は ±512 に収める)。速度は `(vx, vy + 4) / 16` (`vy` の `+ 4` は、次の tick に `step_entity` が足す重力を見込むため)、書き戻すときは `vy` から 4 を引く。`ext_vx` は、直した横の速度から、この tick の歩きの速度 (`vx - ext_vx`) を引いたもの。`solid` は仕様 §8.3 の「視線を遮るセル」と同じ判定。
- `StepReport` はそのまま。`GameState::rope(id) -> Option<&Rope>` で描画に渡す。`state_hash` にフックの状態 (`cooldown`・`tool` を押していたか・ワイヤーの `state_hash`) を含める。

### 9.5 `Lockstep` と デモ (ゲーム側)

- パケットを version 3 にする: 30 バイト (v2 の後ろに `tool: u8`、0 か 1)。範囲外は `InvalidPacket`。v2 以前は `Mismatch`。杖を持たない参加者が `tool` を送ったら、`fire` と同じく `InvalidAction` で拒否する。
- `PlayerLoadout` に `hook: Option<String>` (`Definitions::hook` の名前) を足す。`charon-app` は、プレイヤーの生成時にフックを `equip_hook` する。
- 入力: **左ボタン = 武器、右ボタン = 道具**。ブラシ (材質を描く、仕様 09) と左ボタンが重なるので、**描画モード** を設ける: `B` キー (または `Draw` パネルの `Paint` チェック) で切り替え、描画モードのあいだは左ボタンがブラシになり (武器は撃たない)、そうでないあいだ左ボタンは武器になる。デモは武器モードで始める。右ボタンは常に道具。UI が押下を使ったとき、自由カメラ (`F`) の間は、どちらも使わない。`Wand` パネルに道具 (`Tool: Hook`) と、ワイヤーの状態 (`flying` / `anchored`) を出す。
- 描画: ワイヤーは、点列の間を 1 セル間隔でうめた 1×1 の点と、先端の 2×2 の点で描く (エンティティの描画パス。色は固定)。

| テスト | 確認 |
|---|---|
| `rope_sags_under_gravity` | 両端を固定した、たるんだワイヤーが重力で垂れ下がり (中点が両端を結ぶ線より下)、長さは `rest` に近い |
| `rope_flight_anchors_on_solid` | 飛行する先端が地形の手前で止まって `Anchored` になる。速くても (1 tick に 20 セル) すり抜けない |
| `rope_reeling_shortens_and_stays_taut` | 巻き取りで `rest` が `min_length` まで減り、拘束後の多角線の長さは `rest` を大きく超えない |
| `rope_stays_out_of_solids` | 地形の角に当たっても、どの点も地形の中にいない |
| `rope_is_deterministic` | 同じ入力の列から、毎 tick の `state_hash` が一致する (1〜8 の初期条件) |
| `tether_removes_outward_velocity` | 張ったワイヤーは外向きの速度を除き、巻き取りで内向きに加速し、上限を守る。張っていなければ変えない |
| `entity_external_velocity_decays` | `ext_vx` が接地中・空中で減り、壁で 0 になる。`ext_vx = 0` のとき `step_player` と同じ |
| `hook_launches_and_anchors` | 右ボタン相当の入力で、狙った方向へワイヤーが飛び、壁に当たって `Anchored` になる |
| `hook_pulls_player_toward_anchor` | 着弾後、押している間はアンカーに近づき、床や壁を抜けず、`min_length` で止まる |
| `hook_swings_without_stretching` | 張ったワイヤーでぶら下がるプレイヤーが振り子のように揺れ、アンカーまでの距離が `rest` を大きく超えない |
| `hook_release_and_cooldown` | 離すとワイヤーが外れ、`cooldown` の間は撃てない。倒れても外れる |
| `hook_respects_max_length` | 最大の長さの外の壁には届かず、`Lost` で外れる |
| `weapon_and_tool_are_independent` | 武器 (`fire`) と道具 (`tool`) を同じ tick に使え、ワイヤーが出ていても杖が撃てる。杖のないキャラクターの `tool`・ワイヤーのない `tool` は何も起こさない |
| `hook_definitions_layer_and_replace` | `hooks.ron` の追加・`replace`・MOD・範囲検査・内容ハッシュ |
| `lockstep_packet_v3_roundtrip` | 30 バイトの符号化・復号が一致し、不正な `tool`・v2 を拒否する (v2 のテストを置き換える) |
| `demo_hook_in_scene` | デモの世界で、プレイヤーがフックを持って始まり、右ボタン相当の入力 (`tool`) でワイヤーが壁に着弾してプレイヤーが引かれ、左ボタン相当の入力 (`fire`) では杖が撃てる (画面なし) |
| `mouse_buttons_map_to_weapon_and_tool` | 左ボタン = 武器、右ボタン = 道具。描画モードでは左ボタンは武器にならない。UI が使った押下と自由カメラの間は何も使わない (`charon-app`) |
| `demo_renders_hook` | RTX 4060 のオフスクリーン描画で、ワイヤーと `Item: Hook` を含む画面を描ける |

## 10. 後続 (T11.5) の方針

- **T11.5 掘る道具・浮遊**: 掘るは `Payload` の一種 (半径内のセルをパーティクルへ移す)。浮遊は `PlayerInput` の拡張 (燃料を消費し、`vy` を上向きに足す)。
- いずれも着手前に、この文書へ表のテスト名と式を書く。

## 11. タッチとゲームパッドの操作 (M13、参照デモ)

Android (とタッチ画面のデスクトップ) で `charon-demo` を遊ぶための操作。ゲーム (`AppOptions::follow_player`) が有効なときだけ働く。エンジンの入力 `PlayerInput`・`GameInput` は変わらない。

### 11.1 仮想スティックとボタン (`charon-app::touch_pad`)

画面の大きさ `(w, h)` から `unit = max(min(w, h) × 0.07, 20)` ピクセルを決め、次を置く。

| 部品 | 位置 | 操作 |
|---|---|---|
| 移動スティック | 左下 (`x < 0.4w`、`y > 0.4h`)。半径 `3 × unit` | 横にずらすと歩く (ずれ > 半径の 0.3)。**上へスワイプ** (ずれ < −半径の 0.7) するとジャンプ |
| 狙いスティック | 右下 (`0.45w ≤ x < w − 3.6 unit`、`y > 0.4h`) | 触れている間は**武器を撃つ**。ずらした向き (ずれ > 半径の 0.25) へ狙う。向きは `(dx, dy)` を長さ 1000 にそろえた整数 |
| `JUMP` | 右端の下 `(w − 1.7 unit, h − 1.8 unit)` | 押している間ジャンプ |
| `HOOK` | その上 `(…, h − 4.5 unit)` | 押している間**右手の道具** (ワイヤーフック)。向きは最後の狙い (一度もなければ右上 `(1000, −400)`) |
| `PAINT` | さらに上 `(…, h − 7.2 unit)` | 押すたびに、左手 (武器) と描画 (ブラシ) を切り替える (`B` キーと同じ) |

スティックは、触れた場所を中心にして、指の位置に丸を描く。部品に触れた指は、2 本指のカメラ操作 (`TouchGestures`) とブラシに渡さない。部品の外のタッチは、これまでどおり (2 本指でカメラ、1 本指でブラシ・剛体のつかみ)。部品は、最初のタッチ (Android では最初から) で出す。半透明で描き、`MOVE`・`AIM`・`JUMP`・`HOOK`・`PAINT` の文字を添える。

### 11.2 ゲームパッド

- **Android:** ゲームパッドのボタンはキーとして届く (`KEYCODE_BUTTON_*`)。`A` = ジャンプ、`X`・`R1`・`R2` = 武器、`B`・`L1` = 道具、`L2` = 掘る道具、`Y` = 武器 / ブラシの切り替え。十字キーは矢印キー (歩く・ジャンプ) と同じ。撃つ向きは、狙いのスティックがないので、歩いている向きの少し上 `(±1000, −300)`。アナログスティックは winit が届けないため未対応。
- **デスクトップのゲームパッド (XInput など):** `gilrs` (`charon-app::gamepad`、Android・Web では使わない) が最初に見つけたゲームパッドを毎フレーム読む。左スティック・十字キーで歩く (上に倒す・十字キーの上・`A` でジャンプ)、`X`・`R1`・`R2` が武器、`B`・`L1` が道具、`L2` が掘る道具、右スティックで狙う (しきい値 0.3、スティックの上が画面の上)、`Y` が武器 / ブラシの切り替え。右スティックがないときの撃つ向きは、歩いている向きの少し上。タッチと重ねて使える。

### 11.3 テスト

| テスト | 確認 |
|---|---|
| `stick_walks_and_swipe_up_jumps` | 移動スティックで歩き、上へスワイプするとジャンプし、離すと止まる |
| `aim_stick_fires_toward_the_drag` | 狙いスティックに触れている間は撃ち、ずらした向きへ狙い、`HOOK` は最後の狙いの向きへ撃つ |
| `buttons_hold_and_paint_toggles_once` | `JUMP` は押している間、`PAINT` の切り替えは 1 度だけ |
| `touches_outside_the_pad_are_left_to_the_camera_and_brush` | 部品の外のタッチは部品のものにならない |
| `android_gamepad_buttons_map_to_actions` | Android のボタンが武器・道具・切り替えになり、知らないボタンは無視する |
| `sticks_and_buttons_map_to_actions` | デスクトップのゲームパッドのスティック・十字キー・ボタンが歩く・ジャンプ・武器・道具・切り替えになり、触れなければ何もしない |
| `right_stick_aims_with_screen_down_positive` | 右スティックの上が画面の上 (y 負) になり、小さな傾きでは狙いを変えない |
| `demo_renders_touch_pad` | RTX 4060 のオフスクリーン描画で、仮想パッドが重ね描きに出る ([`M13-touch-pad.png`](../screenshots/M13-touch-pad.png)) |

Android 実機での確認は未実施。

(M15 で、`DIG` ボタン (`HOOK` の上)、ゲームパッドの `L2`、キーボードの `E`、マウスの中ボタンが**掘る道具**になった。`L1` は `HOOK` だけ。§12。)

## 12. 掘る道具 (Starbound のレーザー型、M15、T11.5 の掘る部分)

狙った場所の**円**を、レーザーで削っていく。すべての固体・粉体に**硬さ**、道具に**力**があり、掘る時間は硬さと力で決まる。

### 12.1 硬さと力

- `Material::hardness` (`materials.ron` の `hardness: Some(..)`、仕様 01): 固体の既定は 300、粉体は 20。**液体・気体・空は掘れない** (0 に固定)。`boundary` (読み込まれていない所・世界の端) は `u16::MAX` で、掘れない。例: 石 600、氷 200、木 150、砂・土・灰は 20。
- `DigDef` (`digs.ron`、エンジンの既定 `assets/digs.ron`、MOD が追加・`replace` で上書き、内容ハッシュを同期の照合に含める): `power` (1 tick に円の中の各セルへ与えるダメージ、1..=10000)、`radius` (円の半径 (セル)、1..=24)、`range` (届く距離 (セル)、1..=400)、`recover` (当てていないセルのダメージが 1 tick に回復する量、0..=10000)。既定の `laser`: 力 6、半径 5、距離 48、回復 2。
- **掘る時間 = 切り上げ(硬さ / 力) tick**: 石 (600) を `laser` (6) で 100 tick、砂 (20) は 4 tick。力 100 の道具なら石は 6 tick、力 1 なら 600 tick。

### 12.2 動き (`charon-game::dig`、`GameState::step` の (3'') 、フックのあと・敵の前)

1. `GameInput::dig` を押している間 (倒れたキャラクターは使えない)、手 (当たり判定の中心) から `aim` の向きへ、**整数**で光線を進める。`aim` の長い方の軸が 1 セル (16/16) ずつ進み、最大 `range` セル。
2. 光線は、空・気体・液体を**素通り**する。固体・粉体に当たったらそこが当たった点。**剛体 (`FLAG_BODY`)・`boundary`・未読み込みのセルに当たったときは、光線がそこで止まるだけ**で、何も掘らない。
3. 当たった点を中心に、半径 `radius` の円 (`dx² + dy² <= radius²`) の中の掘れるセル (固体・粉体、剛体でなく、硬さが 0 でも `u16::MAX` でもない) へ、`power` のダメージを足す。溜まったダメージが硬さ以上のセルは、`World::set` で空のセルにして消す。
4. 当てていないセルのダメージは、1 tick に `recover` ずつ回復し、0 になったら表から消える。続けて当てれば続きから。
5. 消したセルは `StepReport::dug` (`Dug { pos, material }`) に出る。**集める・拾うのはゲームの仕事** (エンジンは数を減らすだけ)。
6. 描画のために、光線 (`Beam { from, to, hit }`) を `GameState::dig_beam(id)` が返す。デモは、手から当たった点まで 1 セル間隔の点で描く。

### 12.3 入力

`GameInput::dig` (Lockstep v4: `tools` バイトの bit 1。bit 0 は `tool`)。マウスの中ボタン、キーボードの `E`、タッチの `DIG`、ゲームパッドの `L2`。向きは、他の道具と同じ `aim` (マウスのカーソル、狙いのスティック、なければ歩く向きの少し上)。`PlayerLoadout::dig` が `Definitions::dig` に渡す名前 (デモは `laser`)。

### 12.4 テスト

| テスト | 確認 |
|---|---|
| `hardness_defaults_by_phase` | 硬さの既定 (固体 300・粉体 20・液体 0・`boundary` は最大)、`materials.ron` の石 600 |
| `dig_removes_cells_after_hardness_over_power_ticks` | 石を力 6 で掘ると、ちょうど 100 tick で消え、その前には 1 セルも消えない |
| `dig_time_depends_on_hardness_and_power` | 砂 4・石 100・力 100 の石 6・力 1 の石 600 tick |
| `dig_cuts_a_circle` | 円の中の壁が消え、円の外は残る |
| `dig_ignores_liquids_bodies_and_boundary` | 液体は掘れず素通り、剛体は掘れず光線が止まる |
| `dig_damage_recovers_when_released` | 離すと 1 tick に `recover` ずつ回復し、最後は表から消える |
| `dig_reports_dug_cells` | `StepReport::dug` の数と、消えたセルの数が一致する |
| `dig_beam_follows_the_aim` | 光線が狙いの向きへ進み、空へ向ければ当たらず、離せば消える |
| `dig_needs_an_equipped_tool_and_a_defined_name` | 道具がなければ掘れず、未定義の名前は拒否する |
| `dig_definitions_layer_and_replace` | 追加・`replace`・範囲外・重複の拒否・内容ハッシュ |
| `dig_is_deterministic` | 同じ入力で、世界とゲーム状態のハッシュが毎 tick 一致する |
| `lockstep_packet_v4_roundtrip` | `tools` バイト (tool = 1、dig = 2) の符号化・復号、bit 2 以上を拒否する |
| `demo_dig_in_scene` | デモのプレイヤーが掘る道具を持って始まり、左の石の壁を 100 tick で掘り抜く |

### 12.5 後続

掘れない固体 (`hardness: Some(65535)`) の見た目、レーザーの光・音、道具の持ち替え。浮遊は未着手 (T11.5 の残り)。掘ったものを集める持ち物は §13。

## 14. ゲーム用 UI・設定・キー割り当て (M15、`charon-app`)

- **ポーズのメニュー** (既定は `Esc`): 開いている間、世界は止まる。`Resume`・`Inventory`・`Quit`、`UI scale` (0.5〜3.0)・`Character scale` (0.25〜3.0)・光の品質 (`Off`/`Low`/`High`/`xHigh`)、キー割り当て (操作ごとに、押して次のキーを割り当てる。`Esc` で取り消し)、`Reset keys`。閉じたときに設定を書く。
- **持ち物の画面** (既定は `I`): 持っている材質を多い順に、名前と数で (§13)。持ち物がなければ `Empty`。
- **キー割り当て** (`settings::KeyBindings`): 操作 `Left`・`Right`・`Jump`・`Dig`・`FreeCamera`・`TogglePaint`・`Inventory`・`Pause` ごとに 1 つ以上のキー (名前は `winit::KeyCode` の `"KeyA"`・`"Space"` など。割り当てられるのは表のキーだけ)。**1 つのキーは 1 つの操作だけ**: 別の操作に割り当てると、前の操作から外れる。既定: 左 `A`/`←`、右 `D`/`→`、ジャンプ `W`/`↑`/`Space`/`C`、掘る `E`、自由カメラ `F`、武器 / ブラシ `B`、持ち物 `I`、ポーズ `Esc`。自由カメラの下 (`S`/`↓`) と `Shift` は割り当てなし。
- **設定ファイル** (`settings.ron`、`DemoConfig::settings`。Android は内部ストレージ): `Settings { version, ui_scale, character_scale, light, keys }`。読むときに、範囲外の値を収め (NaN は既定)、知らないキー名の除去・重複の解消・空の操作への既定の戻しを行う。版が違う・壊れたファイルは報告して既定で始める。64 KiB 超は拒否。起動時に読み、メニューを閉じたとき・世界の作り直し・終了で書く (一時ファイルへ書いて入れ替える)。Web では保存しない。
- テスト: `key_names_roundtrip_and_unknown_keys_are_refused`、`defaults_cover_every_action_without_overlap`、`binding_moves_a_key_from_the_old_action`、`settings_roundtrip_and_clamp`、`broken_key_tables_are_repaired`、`settings_file_io`、`custom_bindings_change_which_keys_work`、`pause_menu_and_inventory_draw`、`demo_renders_pause_menu` ([`M15-pause-menu.png`](../screenshots/M15-pause-menu.png))。

## 13. 持ち物とゲーム状態の保存 (M15)

### 13.1 持ち物 (`charon-game::inventory`)

- `Inventory`: 材質ごとの数 (`BTreeMap`)。`add(材質, 数) -> 入った数`、`take(材質, 数) -> bool` (足りなければ何も取らない)、`count`、`total`、`iter` (材質 ID の昇順)、`state_hash`。1 つの材質は `STACK_MAX = 99999` まで、種類は `KINDS_MAX = 256` まで (超えた分・新しい種類は入らない)。`empty`・`boundary` は持てない。
- `GameState::give_inventory(id)` で、杖を持つキャラクターに空の持ち物を持たせる (すでにあれば中身を残す)。持ち物のあるキャラクターが掘ったセルは、**材質ごとに 1 個ずつ**集まる (`step_digs`)。持ち物のないキャラクターが掘ったセルは、集まらずに消える。集めたものを使う・置く・捨てるのは、ゲームが `take` と `World::set` で決める。同期のハッシュ (`state_hash`) に含める。
- デモの `Wand` パネルに、多い順に 4 種類 (`Items (dig): stone 12 · sand 3`)。

### 13.2 ゲーム状態の保存 (`charon-game::save`)

- 世界の保存 (仕様 04) とは別の **`game.ron`** (世界の保存先のディレクトリ)。`GameSave { version, tick, player: Option<PlayerSave> }`、`PlayerSave { hp, breath, mana, x16, y16, inventory: Vec<(材質の名前, 数)> }`。材質は**名前**で保存する (表の順序が変わっても読める)。敵と弾は保存しない (読み込んだ世界で `AppOptions::enemies` から再び生まれる)。
- `GameSave::capture(game, entities, id, materials, tick)`、`to_ron`・`from_ron` (版が違うと `SaveError::Version`、持ち物の種類が `KINDS_MAX` を超えると `Invalid`)、`write(root)` (一時ファイルへ書いてから入れ替え)、`read(root)` (ファイルがなければ `Ok(None)`、1 MiB 超は拒否)。
- `GameSave::apply(game, entities, world, id)`: 体力は `max_hp` に収め、**0 (倒れていた) なら満タン**で戻す。息・マナは上限に収める。位置は、そこが固体と重ならないときだけ戻し (`Applied::moved`)、速度は 0 にする。持ち物は名前から材質を引いて足し、知らない名前は読み飛ばす。
- アプリ (`charon-app`): 世界を保存するとき (終了・`Singleplayer`・`Random Seed`) に、プレイヤーがいれば `game.ron` を書く。世界を開いたとき (起動・作り直し) に読み、**プレイヤーが生まれた最初のフレーム**で `apply` する。`game.ron` がなければ新しく始める。壊れていれば報告して新しく始める。Web では保存しない。

### 13.3 テスト

| テスト | 確認 |
|---|---|
| `add_take_and_count` / `stacks_and_kinds_are_limited` / `empty_and_boundary_are_never_kept` / `hash_tracks_contents` | 持ち物の追加・取り出し・上限・ハッシュ |
| `dig_collects_into_the_inventory` | 掘った数だけ材質が集まり、持ち物がなければ集めない |
| `inventory_requires_a_character_with_a_wand` | 杖のないキャラクターには持たせず、すでにあれば中身を残す |
| `game_state_roundtrips_through_ron` | 体力・息・マナ・位置・持ち物を RON に写して、新しいキャラクターへ戻せる |
| `apply_is_careful_with_dead_players_solid_positions_and_limits` | 倒れていたら満タン、固体の中へは戻さない、上限に収める、知らない材質を読み飛ばす |
| `bad_game_saves_are_rejected` | 版違い・壊れた RON・多すぎる種類を拒否する |
| `game_save_file_io` | ないときは `None`、壊れていればエラー、書き直しても一時ファイルは残らない |
| `demo_dig_in_scene` | デモで掘った石が持ち物に集まり、`game.ron` に写して読み戻せる |

## 15. 出来事 (イベント、T16.3)

ゲーム側が反応 (音・揺れ・得点・演出) を付けるための通知。シミュレーションの結果は変えない。

- `GameEvent::{Hit(Hit), Defeated(EntityId), Dug(Dug)}`。`GameState::step` が、その tick の `StepReport` と同じ内容を、起きた順に溜める。
- ゲームは `GameState::take_events()` で受け取る (受け取ると空になる)。受け取らずに溜まる数は `EVENTS_MAX` (4096) までで、超えた分は捨てる。
- デモでは、敵を倒す (`Defeated`) と大きく、敵や自分に弾が当たる (`Hit` で `entity` あり) と小さく画面が揺れる (仕様 06 §11)。

| テスト | 確認 |
|---|---|
| `dug_cells_arrive_as_events_once_and_the_queue_is_bounded` | 掘ったセルの数だけ `Dug` が届く、受け取ると空、溜まる数に上限がある |

## 16. 地形の崩落 (T16.4、`charon_game::collapse`)

掘って支えを失った固体が、ひとかたまりのまま落ちる。`GameState::collapse` に `Some(Collapse::new())` を入れたゲームだけで働く (デモは掘る道具があれば有効)。

- **支えの判定**: 掘ったセルの上下左右にある固体 (`Phase::Solid`、剛体のセルを除く) から、つながった固体を 4 近傍で集める。集めた数が `MAX_CLUSTER` (400) 以上、または読み込まれていない場所 (`BOUNDARY`) に触れていれば「地面とつながっている」とみなして落とさない。そうでなければ支えがない。材質の名前は使わない。
- **落ち方**: 同時に落とすのは `MAX_FALLING` (32) かたまりまで。毎 tick、下の行から順に 1 セル下と `swap` する。下が空・液体・気体なら進み (液体は押しのけられる)、固体・粉体・剛体・境界にふさがれたら止まって、そこで普通の固体に戻る。セルの数は変わらない。
- 落ちている途中のセルが固体でなくなったら (掘られた・燃えた)、そのかたまりは追わない。
- 順序: 掘る道具 (§12) の直後、敵・弾より前に、通知と 1 セルの移動を行う。

| テスト | 確認 |
|---|---|
| `unsupported_cluster_falls_as_one_piece_and_lands` | 板と柱の上半分が一緒に 5 セル落ちて着地する。セルの数は不変 |
| `supported_terrain_stays` | 床につながる部分は落ちない |
| `clusters_touching_the_boundary_are_supported` | 境界に触れるかたまりは落ちない |
| `huge_clusters_are_treated_as_ground` | 大きなかたまりは落ちない |
| `falling_displaces_liquid_and_stops_on_powder` | 水の数は不変、砂の上で止まる |
| `a_cluster_that_is_dug_stops_being_followed` | 落下中に掘られたら追わない |

## 17. 状態異常 (T17.4、`charon_game::status`)

`GameState::statuses` (`StatusTable`)。すべて整数で、決定的。

| 状態 | 効果 |
|---|---|
| `Burning` | `BURN_INTERVAL` (15) tick ごとに 1 ダメージ。長さ `BURN_TICKS` (180)。 |
| `Wet` | ダメージなし。長さ `WET_TICKS` (300)。濡れている間は燃えず、濡れると燃えているのが消える。 |
| `Poison` | `POISON_INTERVAL` (30) tick ごとに 1 ダメージ。API でだけ付ける。 |

- `apply(id, kind, ticks)`: 長さは `TICKS_MAX` (3600) に収め、すでになっていれば長い方に延ばす (縮めない)。0 は何もしない。付いたら真。
- ダメージは「残りの tick が間隔の倍数のとき」に当たる。残りは毎 tick 1 減り、0 で消える。
- 環境からの付与 (毎 tick、体の覆うセルを見る): 液体 (`Phase::Liquid`) があれば `Wet`、なければ、`FLAG_BURNING` または `HEAT_THRESHOLD` 以上の温度のセルがあれば `Burning`。材質の名前は使わない。
- 倒れたキャラクターの状態は消える。順序は、環境ダメージ (§6.1 の (1)) の直後。
- デモの画面 (HP の欄) に、いま付いている状態を出す。

| テスト | 確認 |
|---|---|
| `burning_and_poison_deal_damage_on_a_beat_and_expire` | 間隔ごとのダメージ、時間で消える、濡れるだけではダメージなし |
| `liquid_wets_and_extinguishes_fire_touches_ignite` | 火で燃える、液体で濡れて消える、濡れている間は燃えない、乾いたら燃える |
| `durations_are_clamped_and_extended_and_death_clears` | 上限、0、延長、倒れたら消える |
| `statuses_are_deterministic` | 同じ手順で同じ結果 |
