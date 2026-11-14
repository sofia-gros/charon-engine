# 仕様 20: 戦闘 (M23)

武器・防具・属性・ノックバック・火の玉を、データ駆動のアイテム (仕様 19) の上に作る。整数だけで、決定的。

## 1. 武器

武器は `Equipment` の `Weapon` 枠のアイテム (`ItemKind::Weapon { class, damage, wand }`) で決まる。`GameInput::fire` (左ボタン・使うボタン) で攻撃する。同期 (Lockstep) の入力は変えない。

| `class` | 攻撃 | 間隔 |
|---|---|---|
| `Melee` | 向き (左右) の前の箱を斬る。箱は、体の中心から前へ (体の半幅 + `MELEE_REACH` = 20 セル)、縦は中心の上下 `MELEE_HALF_HEIGHT` = 14 セル。箱に重なる相手全員に、`damage` の物理ダメージ + ノックバック。出来事 `GameEvent::Melee` | `MELEE_COOLDOWN` = 20 tick |
| `Ranged` | 重力のある矢 (速さ 160 = 10 セル / tick、重力 3、寿命 90) を、向きへ撃つ。ダメージは `damage` | `RANGED_COOLDOWN` = 24 tick |
| `Wand` | キャラクターの杖で撃つ (今までと同じ)。杖の武器 (`wand: Some("名前")`) を装備すると、そのキャラクターの杖が、その名前の杖に**入れ替わる** | 杖の `cast_delay` |

何も装備していないときも、杖で撃つ。近接は、相手が**敵ならプレイヤー、プレイヤーなら敵**にだけ当たる (同じ側には当たらない)。仕様 25 §4 で近接・遠隔に定義ごとの `interval`・`kind`、遠隔に任意の `Misc` 弾薬を追加する。省略時は上表の間隔・物理・弾薬不要のまま。杖の `cast_delay` は変更しない。

## 2. ダメージの種類と防具

`DamageKind`: `Physical`・`Fire`・`Poison`・`Cold`。`Vitals::armor` (`Armor { defense, fire, poison, cold }`) を、`GameState::step` の初めに、装備 (`Equipment::stats`) から写す。

- **物理**: `ダメージ − defense`。ただし、元のダメージの 1/4 (1 以上) までしか減らない。
- **火・毒・冷気**: `ダメージ × (100 − 耐性) ÷ 100` (四捨五入)。耐性 100 で無効。防御は効かない。

どの攻撃がどの種類か: 弾の直撃・近接・接触ダメージ = 物理。爆発・熱いセル・溶岩・燃えている状態 = 火。毒の状態 = 毒。溺れは防具で減らない。

## 3. ノックバック

当たった相手は、弾の向き (近接は前) へ弾き飛ばされる。`ext_vx` に `24 + min(ダメージ, 40)` (1/16 セル / tick) を足し、上へ `28` の速さで浮く。

## 4. 火の玉

呪文の `payload: Ignite(material: "fire", radius: 4)`:

- 飛んでいる間、通った空きセルに火のセルを置く (撃った人の体から出たあと)。火のセルは光るので、動く光になる。
- 当たった場所を中心に、半径の円の中で、空きセルの半分 (市松) に火を置き、`ignite_at` を持つ材質のセル (木・油など) は、発火する温度まで熱する。燃え広がりは、通常のセルの規則 (仕様 03) に任せる。
- 近くのキャラクターは、火のセルの熱で `Burning` の状態になり、火のダメージを受ける (耐性で減る)。

エンジンの既定: 呪文 `fireball`、杖 `fire_wand`、アイテム `fire_wand`・`bomb_wand`。

## 5. テスト (`crates/charon-game/tests/combat.rs`)

| テスト | 確認 |
|---|---|
| `damage_kinds_are_reduced_by_defense_and_resist` | 物理の軽減と下限、属性の耐性、0・負・体力を超える値 |
| `armor_is_copied_from_equipment_each_step` | 装備から防御・耐性が写り、外せば戻る |
| `melee_hits_in_front_with_cooldown_and_knockback` | 前だけ・間隔・ノックバック・向き |
| `ranged_weapon_fires_an_arrow_with_gravity` | 矢が出て、間隔が空く |
| `projectile_hits_knock_the_target_back` | 弾でも弾かれる |
| `a_fire_wand_swaps_the_wand_and_its_fireball_burns_wood` | 杖の入れ替え、火の玉で木が燃えて灰になる |
| `fire_resist_protects_from_lava` | 耐性 100% で溶岩のダメージが 0 |
| `combat_is_deterministic` | 同じ入力なら同じ結果 |

## 6. 実装の状態

自動テスト済み。見た目の効果 (炎の色・煙) は材質の `emission`・煙のセルに任せ、専用の粒の描画は足していない。敵の持ち物からの自動装備は M25。
