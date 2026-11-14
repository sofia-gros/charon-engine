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

3 部位の定義・同じ組合せの判定・能力値の合成を仕様 19 §2・§5 に追加する。装備を外した次の tick に効果は消える。状態を装備から再計算し、蓄積しない。セット ID、各補正の上限、MOD と保存の互換を **着手前に確定** する。

## 4. T31.3: 武器と弾薬

近接・遠隔・杖の既存の入力と命中処理 (仕様 20) に、武器の定義で異なる間隔と属性を足す。遠隔の弾薬は不足時に射出も消費も起こさない。弾薬の種類、対象、失敗時のロールバック、`GameState::state_hash` への影響を **着手前に確定** する。

## 5. T31.4: 冷気と減速

`DamageKind::Cold` の命中と `StatusTable` の期限つき効果をつなぎ、地形・移動に対する更新順、凍結材質の扱いと休眠を **着手前に確定** する。既存の燃焼・濡れ・毒の更新順は変更しない。

## 6. T31.5: Noita 型セルとの統合

火・冷気・爆風によるセル反応を既存の `convert` / `set` / `swap` だけで進め、命中/防御/ドロップ/復帰を headless に検証する。新規の質量変化は仕様 03 の反応・燃焼など既存の許された経路だけに限定する。戦闘によるセルの変更とキャラクターの更新順・保存の範囲は **着手前に確定** する。見た目の演出をゲーム状態やハッシュに混ぜない。
