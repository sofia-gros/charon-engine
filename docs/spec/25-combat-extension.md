# 仕様 25: 戦闘と装備・状態異常の拡張 (M31、第3段階)

2026-11-14。第2段階の保存可能な資源ループ (仕様 24) の次に、Terraria 型の一般的な戦闘・装備の仕組みと Noita 型のセル反応を接続するための **実装前の契約**。元ゲームの固有の数値・名称・アセット・コードは複製しない。M28 の実機比較は保留のまま。既存の [仕様 20](20-combat.md)、[仕様 19](19-items.md)、[仕様 10 §17](10-noita-like-game.md)、[仕様 03](03-reactions-heat.md)、[仕様 24](24-resource-loop.md) が優先する規則は明記して拡張し、既存の入力・保存の形式を無断で壊さない。

## 1. 順序と境界

| タスク | 変更 | 必須テスト |
|---|---|---|
| T31.1 被弾後の猶予 (§2) | 戦闘による被弾の連続適用を防ぐ。環境は従来どおり | `combat_immunity_blocks_repeat_hits_and_expires`、`combat_immunity_keeps_environment_damage`、`combat_immunity_is_hashed_and_cleared_on_respawn` |
| T31.2 装備の組合せ (§3) | 3 部位のセット効果とアクセサリーの能力を毎 tick 再計算する | `armor_set_requires_all_three_pieces`、`accessory_bonus_does_not_stack_on_refresh`、`equipment_bonuses_survive_item_save` |
| T31.3 武器ごとの使用 (§4) | 定義による使用間隔と攻撃属性・弾薬の原子的な消費 | `weapon_interval_and_damage_kind_follow_definition`、`ammunition_is_consumed_only_on_shot`、`held_weapon_input_is_deterministic` |
| T31.4 冷気と状態異常 (§5) | 冷気の攻撃と期限つきの減速を整数だけで進める | `cold_hit_applies_slow_once`、`slow_expires_and_movement_recovers`、`status_reaches_sleep_without_writes` |
| T31.5 戦闘とセルの通し (§6) | 命中→熱・反応→ダメージ→戦利品→保存後の再開を検証 | `combat_environment_chain_is_deterministic`、`combat_chain_keeps_cell_invariants`、`combat_loadout_survives_world_reload` |

各タスクごとに `scripts/check.sh` を通し、1 タスク 1 コミット。`docs/TESTING.md` に従い、場面テストは `testkit` と seed 1..=8 を使う。新しいセル規則には質量保存と最終的な休眠も必要。T31.2〜T31.5 の公開型や係数は着手前にこの仕様の該当節を具体化する。未確定の節から実装を先取りしない。Windows の表示が変わるタスクでは画像を確認・共有する。Android 実行は既存の免除に従い未検証と記す。

## 2. T31.1: 戦闘の被弾猶予

`Vitals` に `combat_immunity: u8` を加える。初期値 0、最大値は定数 `COMBAT_IMMUNITY_TICKS = 12`。`Vitals::damage_combat(amount, kind) -> i32` は `combat_immunity > 0`、死亡済み、または軽減後の実ダメージが 0 のとき、体力もカウンタも変えず 0 を返す。正のダメージを与えたときだけ `combat_immunity = 12` とし、実際に減った体力を返す。既存の `damage` / `damage_kind` は**変更しない**。防具の計算は仕様 20 §2 のまま。

`GameState::step` の入力検査に成功してから、装備の防御更新より前に全 `Vitals` を ID 昇順に 1 ずつ飽和減算する。直撃した tick に 12 を設定すると、続く 11 tick は防ぎ、12 tick 目に 0 となって再び被弾できる。同じ tick の複数攻撃では先に正のダメージを通した一発だけが体力を減らす。失敗した入力では時計を進めない。近接、弾の直撃、爆風、敵の接触だけが `damage_combat` を呼ぶ。命中が防がれても弾は既存の衝突規則どおり消え、爆発・着火による**セル**への作用は省略しない。免疫で実ダメージ 0 のときはノックバックを与えない。環境の熱、溺れ、燃焼・毒状態のダメージは従来どおり `damage_kind` / `damage` を通じて毎 tick 受ける (免疫を付与・消費しない)。

`VitalsTable::state_hash` には各キャラクターの `combat_immunity` を含める。ただし 0 のときは旧ハッシュとの互換性のため追加の印を流さない。死亡後の復活では 0 に戻す。カウンタは `game.ron` に保存しない (再開時は 0)。命中出来事の既存の形式・発生と `Projectile::Hit.damage` の意味 (弾の基本ダメージ) は維持する。近接出来事の `damage` は実ダメージのまま。弾が遮られたときに、命中イベントの基本ダメージを 0 に書き換えない。

### 必須テスト (`crates/charon-game/tests/combat.rs`)

| 名前 | 確認 |
|---|---|
| `combat_immunity_blocks_repeat_hits_and_expires` | 同 tick の複数近接/弾・接触で最初の一発だけ減る。11 tick は再被弾せず、12 tick 目に通る。免疫時のノックバックはない |
| `combat_immunity_keeps_environment_damage` | 免疫中も熱・溺れ・燃焼または毒が従来の式で体力を減らし、セルの反応は止めない |
| `combat_immunity_is_hashed_and_cleared_on_respawn` | 同じ seed/入力でハッシュが一致し、残り tick の差はハッシュに出る。復活で 0 に戻り、無効な入力では状態が変わらない |

## 3. T31.2: 防具セットとアクセサリー

3 部位の定義・同じ組合せの判定・能力値の合成を仕様 19 §2・§5 に追加する。装備を外した次の tick に効果は消える。状態を装備から再計算し、蓄積しない。

- `Armor` の省略可能な `set: Option<String>` (既定 `None`) は名前空間を含むセット ID。同じ層の非 `replace` 定義ではアイテムと同じ規則で `namespace:name` に正規化する。指定できる素の名前は ASCII 英数字と `_` の 1..=64 文字。`replace: true` で置き換えるときは ID を完全修飾名で指定する。`Accessory` に `set` は指定できない。
- `set_defense: u16` (0..=1000)、`set_fire`・`set_poison`・`set_cold: u8` (それぞれ 0..=100) は**頭装備のみ**に指定するセット補正。`set` が `None` の頭、胴、脚、アクセサリーに非 0 のセット補正を指定したら定義を拒否し、層は原子的に失敗する。省略値は 0。既定の木の 3 部位は `set: Some("wood")` で頭の `set_defense: 2`、石の 3 部位は `set: Some("stone")` で頭の `set_defense: 4, set_cold: 10`。セット名は材質名のハードコードではなく定義の文字列としてのみ扱う。
- `Equipment::stats` は武器→頭→胴→脚→アクセサリーの枠順に、各防具の `defense` と耐性を足す。`Head`・`Body`・`Legs` の 3 枠それぞれに、正しい枠の防具があり、`set` がすべて `Some` で**同じ完全修飾名**のときに限り、頭定義のセット補正を 1 回だけ足す。部分一致、別セット、欠損・未知の定義では加えない。アクセサリーの既存の `defense`・耐性はセット成立に関係なく通常の防具として 1 回足す。`defense` の最終上限は 5000、各耐性は既存どおり 100%。武器のダメージと種類、既存の `rating` と自動装備の順位は変えない。
- `GameState::step` は入力検査の後、既存の `refresh_armor` で**装備の定義から毎 tick 最初から**能力値を作り、`Vitals::armor` に上書きする。装備を外した直後の次 tick には補正が消え、同じ装備で何 tick 更新しても重複しない。同期ハッシュは装備 ID の既存ハッシュと定義の `content_hash` に従う。セットのある定義のみ `content_hash` に印 `0xA2`、セット ID と補正を追加し、旧定義のハッシュを保つ。
- MOD の `items.ron` は既存の追加・上書き規則でセットを定義する。既存の `Armor` 定義は `set: None`・補正 0 として読む。`game.ron` は引き続き 5 枠の**アイテム名だけ**を保存し、セットの発動や能力値は保存しない。読み込み時は既存の `apply_items` で装備を復元し、その次の `GameState::step` で再計算する。未知のアイテムは従来どおり読み飛ばす。`GAME_SAVE_VERSION` は 1 のまま。

## 4. T31.3: 武器と弾薬

近接・遠隔・杖の既存の入力と命中処理 (仕様 20) に、武器の定義で異なる間隔と属性を足す。遠隔の弾薬は不足時に射出も消費も起こさない。

- `Weapon` に `interval: u16` (0..=600、既定 0)、`kind: Physical|Fire|Poison|Cold` (既定 `Physical`)、`ammo: Option<String>` (既定 `None`) を追加する。近接・遠隔の `interval: 0` は仕様 20 の `MELEE_COOLDOWN = 20` / `RANGED_COOLDOWN = 24`。正の値はその武器の次の発射までの tick。杖は既存の `cast_delay` を使い、`interval != 0` / `kind != Physical` / `ammo.is_some()` の定義を拒否する。遠隔以外の `ammo` も拒否する。`kind` は近接と矢の**直撃だけ**に適用し、爆風の火属性と環境ダメージは変えない。`Hit.damage` は従来どおり基本ダメージ、近接イベントは実ダメージ。
- 弾薬は `items.ron` の `Misc` の定義アイテム (セル・ブロックは不可)。`ammo: Some("名前")` は定義が既に存在する `Misc` の名前を解決する。MOD では `namespace:name` を先に、なければ無修飾の名前を探す。同じ層で新規弾薬を参照するときは弾薬定義を**先に**書く。存在しないもの、または `Misc` 以外は層を原子的に拒否する。既定の `wood_bow` は弾薬不要のまま。既定の `wood_arrow` を `Misc` として追加し、MOD の遠隔武器が選べる弾薬にする。弾薬の自動補給・レシピ・新しい敵の装備はこのタスクでは足さない。
- 毎 tick の入力検査後、既存の順で待ち時間を 1 ずつ減らす。近接は待ち時間が 0 なら空振りでも攻撃し、その武器の間隔を設定する。矢は `aim != (0,0)`、発射者の実体があり、待ち時間 0、弾薬を指定した場合は持ち物があり 1 個以上あるときだけ発射を試みる。`ProjectileWorld::spawn` が成功して初めて弾薬を 1 個消費し、間隔を設定する。数が足りない、弾数上限、無効な入力・座標なら射出・個数・待ち時間・ID を変えない。弾薬のない既存の武器・杖・敵の挙動を維持する。敵に弾薬を要求する武器を持たせた場合、敵の持ち物に弾薬がなければ撃てない。
- `ProjectileWorld::spawn` は既存 API を保ち、直撃の属性を `Physical` とする。武器の矢だけ新しい `spawn_kind` で属性を持たせる。属性が `Physical` でない弾だけ projectile の同期ハッシュに印 `0xC3` と属性 ID を足し、既存の物理弾のハッシュを保つ。武器定義の `content_hash` は `interval`・`kind`・`ammo` が非既定のときだけ印 `0xA3` と各値を流す。武器ごとの待ち時間は既存の `GameState::state_hash` に含まれ、弾薬数は持ち物のハッシュに含まれる。新たな保存項目は追加せず、`game.ron` 版 1 の装備・弾薬の名前による保存を使う。

## 5. T31.4: 冷気と減速

`DamageKind::Cold` の**近接または弾の直撃**で、軽減・被弾猶予の後に実ダメージが正だった相手へ `StatusKind::Slow` を `SLOW_TICKS = 120` tick 付ける。0 ダメージ (耐性 100%、猶予、死亡済み)・壁への命中・爆風・環境の冷たさは付けない。`StatusTable::apply` の既存の上限 3600 と長い方へ延ばす規則を使い、同じ tick に複数回命中しても 1 種類の残り時間だけを保持する。冷気の状態は体力・ジャンプ・ノックバックを変えない。近接イベントと弾の `Hit.damage` は従来の意味を保つ。

`Slow` 中の歩く速さはプレイヤーも敵も `walk_speed / 2` (0 方向への整数除算)。`PLAYER_WALK_SPEED = 16` なら 8、敵の定義値も同じ式を使う。`EntityWorld::step_players` と `step_player` の従来の API は変えず、ゲームの入力経路 (App と Lockstep) だけが `GameState` の状態に応じた速さで歩かせる。移動は現行の順 (移動→`GameState::step`) のままなので、命中した tick の**次の移動**から減速する。`GameState::step` では入力検査の後、従来どおり環境ダメージ→`StatusTable::step` で燃焼・濡れ・毒とともに `Slow` の残りを 1 減らし、その後に攻撃・命中を処理する。残り 1 のときの移動は遅く、その直後の `step` で消え、次の移動は元に戻る。無効なゲーム入力は状態を減らさない。死亡時は他の状態と同じく消す。再開時に残り時間は保存せず、再命中で付け直す。

同期ハッシュには `Slow` がある ID の昇順で印 `0xC4`、ID (u64 LE)、残り (u16 LE) を流す。`Slow` がなければ従来の `GameState::state_hash` を変えない。セルへの冷気の直接書き込み・周囲の凍結は **T31.4 ではしない** (T31.5 の契約で決める)。水→氷などの相変化は仕様 03 §3 の `freeze.at` / `freeze.into` と既存の `convert` に任せ、材質名で分岐しない。状態の付与・経過・消滅ではセルに `set` / `swap` / `keep_awake` を呼ばない。変化しないセルは書かず、既存の熱交換・相変化が収束すれば `dirty = None` の休眠に達する。

### 必須テスト (`crates/charon-game/tests/status.rs`)

| 名前 | 確認 |
|---|---|
| `cold_hit_applies_slow_once` | 近接と弾の冷気直撃で正のダメージのときだけ 120 tick 付く。猶予・耐性による 0 ダメージでは付かず、二重に蓄積しない。残り時間の差はハッシュに出る |
| `slow_expires_and_movement_recovers` | プレイヤー・敵の歩幅が整数の半分になり、残り 1 で最後の減速、消えた次の移動で元に戻る。無効な入力では減らさない |
| `status_reaches_sleep_without_writes` | seed 1..=8 の静かな世界で Slow の付与・経過・消滅中にもセルの状態・質量が変わらず、最終的に休眠する。既存の凍結は相変化の規則に従う |

## 6. T31.5: Noita 型セルとの統合

既存の火の玉の `Payload::Ignite` (仕様 20 §4) と爆風 `Payload::Explode` (仕様 10 §4、セル→物理パーティクルへ移す) は変更しない。**冷気属性の弾** (`kind: Cold`、`Payload::None` に限る) がセルまたはキャラクターへ着弾したら、着弾セルが読み込み済みの非空・非境界・非剛体セルなら温度を `COLD_IMPACT_DELTA = 40` だけ `saturating_sub` で下げる。値が変わるときだけ `World::set` を呼ぶ。キャラクターへの直撃ダメージと減速は §4–5 のままで、着弾セルに対象がない場合も冷やす。周囲の範囲は 0 セル、近接武器・爆風・環境の冷気には新しいセル操作を足さない。たとえば既定温度 20 の水を冷やすと -20 となり、次にそのセルが更新される `World::step` で仕様 03 §3 の `freeze` と `convert` に従い氷になる (仕様 02 §3 の parity が一致してその tick を見送った場合は、さらに 1 tick 後)。材質名での分岐、冷気によるセル生成/消去、弾道の冷却はしない。

1 固定 tick の順序は、App/Lockstep の**移動** → `GameState::step` の入力検査 → 被弾猶予の減算・防具の再計算 → **環境ダメージと既存の状態更新** → 発射/近接/掘削 → 敵移動・攻撃 → 弾の ID 順の直撃ダメージ → 冷気弾の着弾セル冷却 → 既存の payload (着火/爆風) → ノックバック → 敵の死亡・戦利品 → ワールドのセル更新 (`World::step` は呼ぶ側) のまま。直撃のない爆風も既存の戦闘猶予を尊重し、セルへの作用は省略しない。冷えた水などの相変化や火の燃焼は次のセル更新で起こり、その結果の熱ダメージ/燃焼は**次の** `GameState::step` で読む。変化しないセルは書かず、材質の既存の遷移と休眠を変えない。新しい質量変化は足さない。爆風がセルを物理パーティクルへ移す既存の経路はワールドのセル数だけを見て損失と誤認しない。

`game.ron` の版は 1 のまま。保存するのは従来どおりプレイヤーの位置/体力/マナ・持ち物・ホットバー・装備の**名前**。被弾猶予・状態異常・飛んでいる弾・敵と戦利品は保存しない。世界のセルは既存のリージョン保存を使い、別の世界と `GameState` を作ってからロードして、冷却後のセル・装備した武器/防具・弾薬の数と攻撃可能性を確認する。未検証の実機表示やエフェクトは追加しない。

### 必須テスト (`crates/charon-game/tests/combat_chain.rs`)

| 名前 | 確認 |
|---|---|
| `combat_environment_chain_is_deterministic` | seed 1..=8 の同じ移動/攻撃/セル更新で、冷気による相変化、火の着火/熱ダメージ、爆風の命中・敵の撤去とドロップが同じ順・ハッシュになる |
| `combat_chain_keeps_cell_invariants` | 冷気弾は空・境界・剛体を変更せず、冷却だけではセルを増減させない。既存の反応・爆風のパーティクル転送の範囲を区別し、時間後に静かなセルが休眠する |
| `combat_loadout_survives_world_reload` | リージョンに冷却後のセルを保存して読み戻し、版 1 の `game.ron` のアイテム・装備名・弾薬数を別 `GameState` へ復元し、装備効果と攻撃が再開する。猶予・Slow・飛翔中の弾は持ち越さない |
