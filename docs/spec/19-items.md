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
| `Weapon(class, damage, wand)` | `class`: `Melee`・`Ranged`・`Wand`。`damage`: 1 回の攻撃の基本のダメージ。`wand`: 杖の定義の名前 (`Wand` のとき)。`interval`・`kind`・`ammo` は [仕様 25 §4](25-combat-extension.md#4-t313-武器と弾薬) | `damage` 0..=10000、`interval` 0..=600。`wand` は定義済みの杖、`ammo` は定義済みの Misc |
| `Armor(slot, defense, fire, poison, cold)` | `slot`: `Head`・`Body`・`Legs`・`Accessory`。`defense`: 受けるダメージを減らす量。`fire`・`poison`・`cold`: その属性への耐性 (%)。省略可能な `set` と頭装備の `set_defense`・`set_fire`・`set_poison`・`set_cold` は [仕様 25 §3](25-combat-extension.md#3-t312-防具セットとアクセサリー) | `defense` と `set_defense` 0..=1000、耐性とセット耐性 0..=100 |
| `Consumable(heal, mana)` | 使うと回復する体力・マナ (M23 で使い方をつなぐ) | 0..=10000 |

**強さの点数** (`rating`、自動装備・並べ替えに使う): `Weapon` は `damage`。`Armor` は `defense × 2 + fire + poison + cold`。`Tool` は `pick`。ほかは 0。

## 3. 持ち物 (`Inventory`)

- アイテムごとの個数 (`BTreeMap<ItemId, u32>`)。1 種類の上限 `STACK_MAX` = 99,999、種類の上限 `KINDS_MAX` = 256。今までの `add`・`take`・`count` (材質のセル)、`add_block`・`take_block`・`block_count` (ブロック) は、そのまま使える (`ItemId` へ写して動く)。新しく `add_item`・`take_item`・`item_count`・`item_iter`。
- **ホットバー**: 10 枠の `Option<ItemId>` と、選んでいる枠 (`selected`)。枠はアイテムを**指す**だけ (個数は持ち物にある)。持ち物から無くなった (0 個の) アイテムを指す枠は、空と同じ扱い。`pin`・`unpin`・`select`・`selected_item`。
- `state_hash`: セルだけの持ち物は、これまでと**同じ値**。ブロック・定義のアイテム・ホットバーがあるときだけ、印を付けて続きを流す。

### 3.1 画面のスロット移動 (T29.2)

既存の §3 はアイテム**種類ごとの個数**であり、ホットバーは同じアイテムを指す印である。現段階ではスタックの個数をドラッグで分割せず、格子の 2–5 行目は `ItemId` 昇順の持ち物の**表示** (ホットバーにある種類は除く) とする。画面上で押して別の枠で離すと、下表の操作を 1 回だけ実行する。ドラッグを枠外で離す・同じ枠へ戻すと何も変えない。クリックによる選択・装備・ピン留め・解除は既存どおり使える。フィルタで非表示の種類にはドロップできない。

| 出発 → 到着 | 規則 |
|---|---|
| 持ち物の種類 → ホットバー | 持っている種類を到着枠に `pin`。以前そこにあった印は外す。個数は不変 |
| ホットバー → ホットバー | 二つの印を入れ替える (空枠も可)。選択枠と個数は不変。範囲外は失敗 |
| ホットバー → 持ち物の表示領域 | 出発枠の印を外す。個数は不変 |
| 持ち物の種類・ホットバー → 装備枠 | その枠に合うアイテムを 1 個 `equip`。すでにある装備は持ち物へ戻す。ホットバーの印は §3 のとおり残る (0 個になれば空扱い) |
| 装備枠 → 持ち物の表示領域 | `unequip`。持ち切れなければ何も変えない |
| 装備枠 → ホットバー | 1 個を `unequip` して、その種類を到着枠に `pin`。持ち切れない・枠の範囲外なら、装備も印も変えない |

出発の種類を持っていない、装備の種類が枠に合わない、持ち切れない、または表にない組合せでは**状態を一切変えない** (持ち物の個数・ホットバー 10 枠・選択枠・装備 5 枠)。判定と変更は 1 つの操作として原子的に行い、保存した名前の往復後も同じ配置になる。§3 の `state_hash` は従来どおり、印の配置を反映する。

[操作設定 Wiki のコントローラー設定](https://wikiwiki.jp/cs_terraria/%E6%93%8D%E4%BD%9C%E8%A8%AD%E5%AE%9A/%E3%82%B3%E3%83%B3%E3%83%88%E3%83%AD%E3%83%BC%E3%83%A9%E3%83%BC%E8%A8%AD%E5%AE%9A) の「拾う/置く」は**全スタックを持ち上げて任意の持ち物枠に置く**動作。現在の `BTreeMap` とピン留めは物理的な枠を持たないため、その完全再現、個数の分割、持ち物内の枠同士の入れ替えは **M28 の未解決差分**であり、T29.2 をもって 100% 達成としない。T29.2 は現行モデル内のスロット操作を完成させる。

## 4. クラフト (`recipes.ron`)

```ron
[
    (name: "wood_pickaxe", inputs: [("cells:wood", 40)], output: ("wood_pickaxe", 1)),
    (name: "stone_sword",  inputs: [("cells:stone", 40), ("cells:wood", 15)], output: ("stone_sword", 1), station: Some("workbench")),
]
```

- 入力・出力は、アイテムの名前 (§1)。入力は 1..=8 種類、個数は 1..=99,999。出力の個数は 1..=99,999。
- `station` (任意): 仕様 24 §4 以降は、`fixtures.ron` に `kind: Station` として登録された設備アイテムの名前だけを指定できる。普通の `block:wood` / `block:stone` や `kind: Chest` のアイテムは設備ではない。呼ぶ側は近くの読み込み済みで有効な Station 設備のアイテムだけを `GameState::craft(id, recipe, nearby)` に渡す (距離と有効性は仕様 24 §5)。旧来の普通のブロックを設備として渡す規則は使わない。
- **組み込みのレシピ**: 材質のセル 64 個 → その材質のブロック 1 個 (`Solid`・`Powder` の材質、仕様 17 §4)。`recipes.ron` には書かず、**作業台なしでも作れる**。`station` を指定しない道具レシピも作業台不要。
- 材料が足りない・持ち切れない (上限)・作業台がないときは、**何も変えずに失敗**する。

## 5. 装備 (`Equipment`)

5 つの枠: `Weapon`・`Head`・`Body`・`Legs`・`Accessory`。`GameState::equipment` に、キャラクターごとに持つ。

- `equip(item)`: 持ち物から 1 個を、そのアイテムの枠へ移す。枠に入っていたものは、持ち物へもどる。装備できない (`Weapon`・`Armor` 以外) ものは失敗。`unequip(slot)`: 持ち物へもどす (持ち切れなければ失敗)。
- **能力値** (`EquipStats`): 防具の `defense` の合計、耐性 (`fire`・`poison`・`cold`) の合計 (上限 100)、武器の `damage` と `class`。戦闘へのつなぎは M23。仕様 25 §3 からは、頭・胴・脚が同じセットのとき頭定義の補正を 1 回だけ加え、アクセサリーは常に通常の防具として加算する (防御の最終上限 5000、各耐性 100)。

## 6. 自動装備 (敵も使う)

`auto_equip_best(inventory, equipment, defs)`: 各枠について、持ち物と今の装備の中で、**強さの点数が最も高いもの**を装備する (同点は、`ItemId` の小さい方)。今より強いものがあるときだけ変える。変えた枠の数を返す。敵が装備を拾って戦う (M25) ための土台。

## 7. 保存

`PlayerSave` に、次の項目を足す (すべて `serde(default)`、古い保存は読める。`GAME_SAVE_VERSION` は 1 のまま): `items: Vec<(String, u32)>` (定義のアイテム)、`hotbar: Vec<Option<String>>` (10 枠、アイテムの名前)、`selected: u8`、`equipment: Vec<Option<String>>` (5 枠)。セルは `inventory`、ブロックは `blocks` (今までどおり)。知らない名前は読み飛ばす。

### 7.1 App での保存と復帰 (T29.1)

ネイティブ App の `save_world` は、`GameSave::capture` の直後に定義があれば `with_items` を呼び、同じ `game.ron` に書く。読み込み後、プレイヤーが生まれた最初のフレームで `apply` に続けて `apply_items` を呼ぶ。新しい保存 (`hotbar` が 10 枠、または `items` / `equipment` があるもの) は、起動時の初期持ち物を**保存内容に置き換えてから**復帰し、道具を複製しない。古い保存 (この 3 項目がすべて空) は、起動時の初期道具を残し、保存済みのセル・ブロックを追加する。知らないアイテム名は §7 のとおり読み飛ばす。`GAME_SAVE_VERSION` は 1 のまま、Web の保存経路は変更しない。

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
| `app_save_reloads_items_hotbar_and_equipment` | T29.1: App の保存経路から `game.ron` を書き、新しい App のプレイヤーへ復帰。初期アイテムは二重にせず、セル・ブロック・定義・装備・ホットバー・選択枠を保持 |
| `legacy_game_save_preserves_starter_items` | T29.1: 旧形式の保存は初期道具を消さず、セルを戻す。知らない名前は読み飛ばす |
| `inventory_slot_moves_swap_and_keep_counts` | T29.2: 持ち物・ホットバー・装備への移動と空枠を含む印の交換。個数と選択枠を守る |
| `inventory_slot_failures_are_atomic` | T29.2: 範囲外・未所持・種類違い・満杯の失敗は、持ち物・印・装備を一切変えない |
| `inventory_slot_ui_routes_drag` | T29.2: egui の押下→移動→離すで、UI のドラッグ元/先から 1 つの操作を発行する |
| `inventory_slot_drop_rejects_invalid_targets` | T29.2: 同じ枠・枠外・空の持ち物からは操作を発行しない |
| `inventory_slot_save_roundtrip` | T29.2: 入れ替えと装備のあとに名前で保存し、新しいキャラクターへ戻して印と装備を保つ |
