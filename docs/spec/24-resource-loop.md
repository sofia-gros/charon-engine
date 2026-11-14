# 仕様 24: 資源・道具・製作・収納 (M30、第2段階)

2026-11-14 に第2段階の実装契約を定めた。**この文書は実装済みの報告ではない**。順序は T30.1 → T30.6。各タスクを個別のコミットにし、必須テストと `scripts/check.sh` を通す。M28 の Android 操作・GUI の実機比較は保留したままで、ここで 100% 達成とはしない。

既存の [仕様 17](17-blocks.md) (8×8 ブロックと原子的な設置・撤去)、[仕様 19](19-items.md) (種類別個数・名前による保存・レシピ)、[仕様 21](21-adventure-world.md) (森・鉱石・純粋なチャンク生成)、[仕様 10 §12–13](10-noita-like-game.md) (レーザー採掘・持ち物)、[仕様 04 §2–3](04-world-save.md) (読み込み・退避・保存) に従う。新しい機能は `charon-core` のセル数・反応・休眠規則を変更しない。数値は Charon 独自の初期値であり、Terraria の内部数値を推定したものではない。

## 1. 境界と順序

| タスク | 契約 | 場所・依存 | 完了条件のテスト |
|---|---|---|---|
| T30.1 森の木 | §2: 地表に収穫可能な木を決定的に生成 | `charon-world::terrain`、既存の `cells:wood` | `forest_trees_supply_wood_without_changing_other_biomes`、`trees_match_across_chunk_boundaries_and_orders` |
| T30.2 道具の力と採掘時間 | §3: 選択した道具で完全なブロックを時間をかけて撤去 | `Material::harvest_tool`、`ItemKind::Tool`、`charon-game`、App 入力 | `mining_progress_uses_hardness_and_tool_power`、`mining_interrupt_or_failure_changes_nothing`、`axe_prefers_wood_and_pick_mines_stone` |
| T30.3 機能ブロックの基盤 | §4: 作業台・かまど・箱の配置と名前による保存 | `charon-game::fixtures`、`assets/fixtures.ron`、App の世界保存/Web 保存 | `fixtures_place_only_on_complete_blocks`、`fixtures_survive_streaming_and_save`、`fixture_anchor_damage_keeps_metadata` |
| T30.4 製作と鉱石の使い道 | §5: 作業台・かまどの区別、木→鉄の道具の製作 | `Definitions::recipe`、`GameState::craft`、App の近接判定 | `craft_requires_the_correct_fixture_and_is_atomic`、`ore_processing_builds_an_iron_tool` |
| T30.5 箱の出し入れ | §6: 種類別個数の収納と破損時の保全 | T30.3 の機能ブロック、`Inventory`、`fixtures.ron` | `chest_transfer_is_atomic_and_bounded`、`chest_contents_survive_reopen_and_anchor_damage` |
| T30.6 画面と通しの復帰 | §7: カーソル/製作/箱 UI と保存後の資源ループ | `charon-app`、`charon-game`、`charon-world` | `resource_loop_survives_world_reload`、`fixture_ui_routes_craft_and_transfer` |

各テスト名は `docs/TESTING.md` の規則どおり。世界の場面テストは `testkit` を使う。同じ seed 1..=8 と、チャンク境界をまたぐ座標を含める。変更しないセルは書かず、静止した木/ブロックが最終的に休眠することを T30.1・T30.3 の場面テストでも確認する。M30 は Windows の実画面画像をタスクの表示変更ごとに確認・共有する。Android の実行確認は従来の免除に従い**未検証**と記す。画像が得られないときは自動テストと実機確認を混同しない。

## 2. T30.1: 森の木と木材

`AdventureGenerator::cell_at` とチャンク生成の両方で同一の座標関数を使う。森の 48 セル幅の帯 `r = x.div_euclid(48)` ごとに候補中心 `c = 48r + 24 + (rng::hash(seed, 0, r, 0, TREE_SALT) % 17) as i32 - 8` を一つ選ぶ。`TREE_SALT = 0x4244_001E` (既存 salt と異なる)。`r = -1, 0` は出発点の視界を守るため生成しない。中心 `c` と両端 `c±6` が森で、`surface_y(seed, c±1)` と中心との差がそれぞれ 2 セル以下であり、中心の地表セルが `grass`、地表直上の幹位置が家・池・立て坑ではないときだけ生やす。木の全候補セルに対し、家・ダンジョン・池がすでに内容を決める座標では木を生成せず、その内容を優先する。

幹は `c−1..=c+1`、`surface_y(seed,c)−19..=surface_y(seed,c)` の 3×20 セル、樹冠は `c−6..=c+6`、`surface_y(seed,c)−24..=surface_y(seed,c)−21` の 13×4 セル。重なりも `wood` の通常の固体セル (`FLAG_BLOCK` は立てない)。追加の葉材質、成長、自然落下、木そのものをアイテムとして拾う仕組みは入れない。既存のレーザー (仕様 10 §12) が木のセルを掘ると、1 セルは `cells:wood` 1 個として拾える。`Inventory` が満杯のケースは既存の採掘規則に従い、ここで無制限の回収を約束しない。地形の家の木ブロックと木の幹は別物であり、生成した幹を 8×8 の完全なブロックとして扱わない。

## 3. T30.2: 道具と完全なブロックの採掘

`Material` / `materials.ron` に `harvest_tool: Pick | Axe` (省略時 `Pick`) を足す。組み込み材質 `wood` だけ `Axe`、ほかは `Pick`。これは採掘の性質であり、シミュレーションの材質名による分岐にしない。`ItemKind::Tool { pick: u16, axe: u16 }` と `items.ron` の `Tool(pick, axe)` に拡張し、古い RON の省略 `axe` は 0。`Definitions` のハッシュに `axe` を含める。両方 0 の道具は何も採掘できない。既存の `wood_pickaxe` は `pick: 4` のまま、`wood_axe` は `axe: 8`。斧を持たない道具でも木の完全なブロックは `pick` の値で採掘できる。木に `axe > 0` なら斧を優先し、その他は `pick` を使う。セル単位のレーザー採掘 (`DigDef::power`) は変更しない。

新しい `GameState::mine_selected_tick(world, entities, id, defs, cursor, held)` は完全なブロックだけを対象にし、既存の `GameState::break_block_item` / `use_selected` の即時 API は互換のため残す。`held = true` かつ有効な選択道具・届く位置・完全な同材質の `FLAG_BLOCK` 64 セル・材質の `hardness` が 1..65534・持ち物にブロック 1 個を入れる空き、のとき、プレイヤーごとの `(BlockPos, ItemId, accumulated)` に `power` を 1 tick に 1 回加える。必要 tick は `ceil(hardness / power)`。`accumulated >= hardness` の瞬間に**一度だけ**既存の原子的な `break_block_item` を呼び、ブロック 1 個を持ち物へ加え、進捗を消す。条件を一つでも満たさない、指を離す、対象・道具を変える、または届かなくなると進捗を 0 に戻し、世界・個数は変えない。進捗は同期の状態ハッシュに含め、`game.ron` には保存せず再開時は 0。App は道具使用時だけこの API を**固定 tick ごと**に呼び、従来の 6 tick 間隔で進捗を間引かない。単発の「使う」で即座に掘る既存のテストを変更しない。

## 4. T30.3: 機能ブロック (作業台・かまど・箱)

`assets/fixtures.ron` を `Definitions` の層に加える。各定義は `(name: "workbench", item: "workbench", material: "wood", kind: Station)` の形で、`item` は `items.ron` の既存 `ItemKind::Misc`、`material` は既存のブロックにできる材質の名前。既定は `workbench` (木)、`furnace` (石)、`storage_box` (木、`kind: Chest`)。名前空間・`replace: true`・重複・不明な参照の扱いは仕様 19 §2 の定義と同じ。1 つの `item` は 1 つの fixture 定義にだけ属する。`station` に指定できるのは `kind: Station` のアイテムだけ。

`FixtureMap` は `BTreeMap<BlockPos, Fixture { item_name, material_name, contents }>` で、`charon-game` のゲーム状態に置く。通常のセルの旗を増やさない。配置操作は、アイテムを 1 個持つことと空の目的地を確認してから、仕様 17 の `place_block` で 8×8 の完全なブロックを置き、アイテム 1 個を引いて fixture を記録する。`place_block` が失敗したら個数も map も変えない。家具の追加データがある位置に、普通のブロックを重ねて置かない。完了済みの通常ブロックとは見た目が同じ仮表示で、独自の絵・ドア・松明・ベッド・看板・背景壁はこの段階に含めない。

`FixtureMap::active(world, pos)` は 64 セルが同じ材質の完全な `FLAG_BLOCK` のときだけ真。採掘・反応・熱などで一部が変わったら使用不可とするが、**metadata と中身は消さない**。同じ位置・同じ定義と材質で再配置されたときだけ再び有効になる。アクティブな fixture の撤去は空のものだけを許可し、持ち物に fixture アイテムを戻す余地を先に検査する。`GameState::break_block_item` は fixture のアンカーなら撤去を拒否し、専用の `remove_fixture` だけが fixture アイテムを戻す。§3 の時間採掘も fixture のアンカーを検出した場合は対象にせず、単なるブロック 1 個への変換を許さない。App ではアクティブな空の fixture を狙って道具を使ったときだけ、届く距離を確かめて `remove_fixture` を呼ぶ。中身がある箱の撤去は失敗し、状態を変えない。セルの温度変化などで予期せず壊れた箱も、中身は保全し、再配置で回復できる。傷んだ fixture の記録を黙って捨てない。

配置座標はブロック座標の昇順。保存は世界の root の **`fixtures.ron`** に名前と座標で行い、最大 4096 件、最大 16 MiB。ファイルがない古い世界は空 map。ネイティブは一時ファイルから置換、Web の snapshot は同じ RON の bytes を保持する。チャンクを退避しても map を削除せず、再ロード後にセルの完全性を確認する。世界・fixture は別ファイルなので全ファイルの同時原子的更新までは保証せず、再開時に anchor が合わない記録も保持して休眠させる。未知の fixture 名・材質名・壊れた RON は中身を捨てず、読み込みエラーとして報告する。保存失敗もユーザーに報告し、成功と表示しない。

## 5. T30.4: 作業台・かまどと鉱石の製作

近くの fixture は、プレイヤーの中心があるブロックから Chebyshev 距離 `<= 3` で、**読み込み済みでアクティブな** `Station` の item 名だけを `GameState::craft` の `nearby` (`ItemId`) に入れる。素の `block:wood` / `block:stone` は作業台・かまどではない。必要な材料・設備・空きの検査→材料を引く→成果を入れる順で、失敗は全状態を維持する (仕様 19 §4)。同名の既存レシピの入力や station を変更せず、古い保存の道具を無効にしない。

追加する既定定義とレシピ (いずれも名前で保存、結果は 1 個):

| レシピ | 入力 | 必要な fixture | 出力 |
|---|---|---|---|
| `workbench` | `cells:wood` 32 | なし | `workbench` (Misc) |
| `wood_axe` | `cells:wood` 24 | なし | `wood_axe` (Tool: pick 0, axe 8) |
| `furnace` | `cells:stone` 40 + `cells:wood` 8 | `workbench` | `furnace` (Misc) |
| `storage_box` | `cells:wood` 20 | `workbench` | `storage_box` (Misc) |
| `iron_bar` | `cells:iron_ore` 16 + `cells:coal` 4 | `furnace` | `iron_bar` (Misc) |
| `iron_pickaxe` | `iron_bar` 2 + `cells:wood` 20 | `workbench` | `iron_pickaxe` (Tool: pick 16, axe 0) |

精錬は 1 回のクラフトで完了し、稼働中の燃焼・時間経過・炉の温度は持たない。金の使い道・道具の追加ティア、代替材料、レシピ検索は後のタスク。既存の `iron_ore` の出現範囲は仕様 21 §4 を変更しない。

## 6. T30.5: 箱の出し入れ

`Chest` fixture の `contents` はアイテム名の昇順 `BTreeMap<String, u32>` (保存も名前)。1 箱に最大 40 種類、各種類 1..=99,999。`transfer_to_chest(id, pos, item, amount)` / `transfer_from_chest(id, pos, item, amount)` は 1..=99,999 の全量を動かす。箱がアクティブで、持ち物/箱に全量があり、移動先の種類・個数の上限を越えないことを**両方先に検査**してから変更する。失敗時は双方の個数・ホットバー・選択枠・装備・箱の状態が同じ。装備中の品は持ち物から動かせない。ホットバーの印は仕様 19 §3 に従って残り、個数 0 は空扱い。未知の名前は保存/再保存で保持し、定義がない間は出し入れできない。既存のセル/ブロック/定義の `ItemId` をすべて扱う。

空でない箱を手で撤去できない。外的な変化でアンカーが壊れた箱は §4 のとおり休眠して中身を保持する。クイックスタック・全スタックの物理的なスロット移動・捨てる操作は別タスク。`FixtureMap::state_hash` は座標、fixture 名、材質名、各 item 名と個数を昇順に含む。

## 7. T30.6: 入力・表示・保存の通し

デモの所持品画面で `Misc` の fixture を選んでブロック格子へ置く。届く距離・置ける場所の判定は仕様 18 §2 に従う。作業台/かまどの区別と製作可能・不可は §5 と同じ判定で表示し、箱は開く→中の種類と個数を表示→指定個数を出し入れ→閉じる。UI の全操作は `charon-app` のコマンドとして固定 tick で順に適用し、失敗時に見た目だけ先行させない。既存の 10×5 格子、クリックとドラッグによるホットバー印の操作は維持する。

通しのヘッドレス検査は seed 1..=8: 木を採集→作業台と炉を作る→鉱石と石炭を採掘→鉄の道具を製作→箱に入れる→ブロックで建築→チャンクを退避し保存→新しい App で同じ世界/プレイヤーを読み直す→箱と持ち物・建築を照合する。生成の都合で鉱石に届かない seed はテストの鉱石配置を `testkit` で固定し、生成の到達性は T30.1 の別テストに分離する。ネイティブでの実ウィンドウ描画を確認し、表示タスクの画像を共有する。Android ではビルド確認と実機未検証を明記する。

## 8. M30 の外

進行・自動保存・版移行・バックアップ (M29 第1段階の外)、床・壁の形/扉/看板/ベッド/松明・植物の成長、金の装備と上位の炉、連続採掘の演出、M28 の 100% 同等性、マルチプレイ、溶岩の熱の休眠は含まない。既存のレーザー/粉・液体/温度/反応の挙動を変えるときは、その仕様を先に更新し、質量・決定性・休眠のテストを追加する。
