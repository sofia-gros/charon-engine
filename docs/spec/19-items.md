# 仕様 19: アイテム・持ち物・クラフト・装備 (M20)

持ち物を「材質の個数」から、**データ駆動のアイテム**に広げる。武器・防具・道具・消耗品を、同じ仕組みで持ち、作り、装備する。敵も同じ仕組みで、持ち物と装備を持つ (自動装備は §6)。整数だけで、決定的。

## 1. アイテム ID (`ItemId`)

`ItemId(u32)` は、実行中の識別子。**保存は名前で行う** (表の並びが変わっても読める)。

| 種類 | `ItemId` | 名前 | 意味 |
|---|---|---|---|
| セル | `材質 ID` (`0x0000_xxxx`) | `cells:<材質>` | 材質のセル (粉・固体のかけら)。掘ると集まる。64 個でブロック 1 個 |
| ブロック | `0x0001_xxxx` | `block:<材質>` | ブロックのアイテム (仕様 17)。置くと 64 セル |
| 定義 | `0x0002_xxxx` (下位が表の添字) | 定義の名前 (`wood_pickaxe`、MOD は `mod:name`) | `items.ron` で定義した、道具・武器・防具・消耗品・その他 |

## 2. 定義 (`items.ron`)

エンジンが `assets/items.ron` に既定を持ち、MOD が `items.ron` で足す・`replace: true` で上書きする (仕様 10 §7 と同じ層の仕組み。名前は ASCII 英数字と `_` の 1..=64 文字、MOD は `mod:` が付く)。

```ron
[
    (name: "wood_pickaxe", kind: Tool(pick: 4)),
    (name: "wood_sword",   kind: Weapon(class: Melee, damage: 6)),
    (name: "starter_wand", kind: Weapon(class: Wand, wand: Some("basic"))),
    (name: "wood_helmet",  kind: Armor(slot: Head, defense: 2)),
    (name: "fire_ring",    kind: Armor(slot: Accessory, fire: 20)),
    (name: "heal_potion",  kind: Consumable(heal: 50)),
]
```

| `kind` | 項目 (省略は 0 / `None`) | 範囲 |
|---|---|---|
| `Misc` | なし | |
| `Tool(pick)` | `pick`: つるはしの力 (ブロックを壊す速さ、M22) | 0..=1000 |
| `Weapon(class, damage, wand)` | `class`: `Melee`・`Ranged`・`Wand`。`damage`: 1 回の攻撃の基本のダメージ。`wand`: 杖の定義の名前 (`Wand` のとき) | `damage` 0..=10000。`wand` は定義済みの杖 |
| `Armor(slot, defense, fire, poison, cold)` | `slot`: `Head`・`Body`・`Legs`・`Accessory`。`defense`: 受けるダメージを減らす量。`fire`・`poison`・`cold`: その属性への耐性 (%) | `defense` 0..=1000、耐性 0..=100 |
| `Consumable(heal, mana)` | 使うと回復する体力・マナ (M23 で使い方をつなぐ) | 0..=10000 |

**強さの点数** (`rating`、自動装備・並べ替えに使う): `Weapon` は `damage`。`Armor` は `defense × 2 + fire + poison + cold`。`Tool` は `pick`。ほかは 0。

## 3. 持ち物 (`Inventory`)

- アイテムごとの個数 (`BTreeMap<ItemId, u32>`)。1 種類の上限 `STACK_MAX` = 99,999、種類の上限 `KINDS_MAX` = 256。今までの `add`・`take`・`count` (材質のセル)、`add_block`・`take_block`・`block_count` (ブロック) は、そのまま使える (`ItemId` へ写して動く)。新しく `add_item`・`take_item`・`item_count`・`item_iter`。
- **ホットバー**: 10 枠の `Option<ItemId>` と、選んでいる枠 (`selected`)。枠はアイテムを**指す**だけ (個数は持ち物にある)。持ち物から無くなった (0 個の) アイテムを指す枠は、空と同じ扱い。`pin`・`unpin`・`select`・`selected_item`。
- `state_hash`: セルだけの持ち物は、これまでと**同じ値**。ブロック・定義のアイテム・ホットバーがあるときだけ、印を付けて続きを流す。

## 4. クラフト (`recipes.ron`)

```ron
[
    (name: "wood_pickaxe", inputs: [("cells:wood", 40)], output: ("wood_pickaxe", 1)),
    (name: "stone_sword",  inputs: [("cells:stone", 40), ("cells:wood", 15)], output: ("stone_sword", 1), station: Some("block:wood")),
]
```

- 入力・出力は、アイテムの名前 (§1)。入力は 1..=8 種類、個数は 1..=99,999。出力の個数は 1..=99,999。
- `station` (任意): 近くにあるべきブロック (作業台など) の名前。**呼ぶ側が、近くにあるものの名前を渡す** (`GameState::craft(id, recipe, nearby)`)。M22 で、近くのブロックを調べて渡す。
- **組み込みのレシピ**: 材質のセル 64 個 → その材質のブロック 1 個 (`Solid`・`Powder` の材質、仕様 17 §4)。`recipes.ron` には書かない。
- 材料が足りない・持ち切れない (上限)・作業台がないときは、**何も変えずに失敗**する。

## 5. 装備 (`Equipment`)

5 つの枠: `Weapon`・`Head`・`Body`・`Legs`・`Accessory`。`GameState::equipment` に、キャラクターごとに持つ。

- `equip(item)`: 持ち物から 1 個を、そのアイテムの枠へ移す。枠に入っていたものは、持ち物へもどる。装備できない (`Weapon`・`Armor` 以外) ものは失敗。`unequip(slot)`: 持ち物へもどす (持ち切れなければ失敗)。
- **能力値** (`EquipStats`): 防具の `defense` の合計、耐性 (`fire`・`poison`・`cold`) の合計 (上限 100)、武器の `damage` と `class`。戦闘へのつなぎは M23。

## 6. 自動装備 (敵も使う)

`auto_equip_best(inventory, equipment, defs)`: 各枠について、持ち物と今の装備の中で、**強さの点数が最も高いもの**を装備する (同点は、`ItemId` の小さい方)。今より強いものがあるときだけ変える。変えた枠の数を返す。敵が装備を拾って戦う (M25) ための土台。

## 7. 保存

`PlayerSave` に、次の項目を足す (すべて `serde(default)`、古い保存は読める。`GAME_SAVE_VERSION` は 1 のまま): `items: Vec<(String, u32)>` (定義のアイテム)、`hotbar: Vec<Option<String>>` (10 枠、アイテムの名前)、`selected: u8`、`equipment: Vec<Option<String>>` (5 枠)。セルは `inventory`、ブロックは `blocks` (今までどおり)。知らない名前は読み飛ばす。

## 8. テスト

| テスト | 確認 |
|---|---|
| `items_load_layer_and_validate` | 既定の定義が読める。MOD の追加・`replace`・名前空間。不正な値・知らない杖・重複を拒否 |
| `item_names_roundtrip` | `cells:`・`block:`・定義の名前が、`ItemId` と相互に変わる。知らない名前は `None` |
| `inventory_holds_items_and_keeps_old_hashes` | アイテムの追加・取り出し・上限。セルだけの持ち物のハッシュが変わらない |
| `hotbar_pins_selects_and_ignores_missing_items` | 枠の指定・選択・持ち物にないものは空扱い |
| `recipes_craft_consume_and_fail_cleanly` | 材料を使って作る。足りない・上限・作業台なしは何も変えない |
| `block_recipe_is_built_in` | 64 セル → ブロック |
| `equip_swaps_and_unequip_returns_items` | 枠へ移す・入れ替え・外す |
| `equipment_stats_add_up_and_cap_resists` | 防御・耐性の合計、耐性の上限 100 |
| `auto_equip_picks_the_best_and_is_deterministic` | 最も強いものを装備、同点は ID 順、弱いものには替えない |
| `game_save_roundtrips_items_hotbar_and_equipment` | 保存・復帰。古い保存・知らない名前 |
