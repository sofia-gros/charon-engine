# Charon Engine ロードマップ

## 読み方

- 作業は **タスク** (T1.1 など) の単位で、上から順に行う。1 タスク = 1 コミット (または 1 プルリクエスト)。
- 各タスクには「読む仕様」「作るもの」「完了条件」がある。**仕様に書かれた名前・式・順序をそのまま実装する。**
- 作業の規則は [`CLAUDE.md`](../CLAUDE.md) にある。最初に読むこと。
- 仕様は `docs/spec/` にある。

| 文書 | 内容 |
|---|---|
| [01-core-data](spec/01-core-data.md) | 座標、セル、材質、乱数、チャンク、ワールド |
| [02-simulation](spec/02-simulation.md) | 粉体・液体・気体の移動、摩擦、粘性、並列化 |
| [03-reactions-heat](spec/03-reactions-heat.md) | 反応、熱、相変化、燃焼、寿命 |
| [04-world-save](spec/04-world-save.md) | チャンクの読み込み・退避、保存形式、生成 |
| [05-rigid-bodies](spec/05-rigid-bodies.md) | 剛体とセルの相互作用、浮力、パーティクル |
| [06-rendering-platform](spec/06-rendering-platform.md) | wgpu、シェーダー、Android、メインループ |

テストの書き方は [`docs/TESTING.md`](TESTING.md) にある。

## 方針

Noita 寄りの機能 (液体・気体・反応・熱) を先に固める。セルの表現とシミュレータの中心に関わり、
後から変えるコストが最も高いためである。Terraria 寄りの機能は、その土台の上に後から追加する。

## 設計原則

FallingSandEngine の調査で見つかった問題に、それぞれ対応させている。

| 原則 | 内容 | 対応する問題 |
|---|---|---|
| セルは小さな POD | 1 セル 8 バイトの `Copy` 型。性質は材質テーブルから引く | 約 32 バイトのピクセルを毎回 clone していた |
| 材質と反応はデータ | RON で定義し、コードを変えずに追加できる | 材質がハードコードで、性質を持つ場所がなかった |
| 決定的 | 乱数は seed・座標・tick から導出。結果はスレッド数に依存しない | 無シードの乱数を使っていた |
| `unsafe` なしの並列化 | 9 フェーズ更新で、各スレッドにチャンクの所有権を丸ごと渡す | `unsafe` 116 箇所、作者自身が未定義動作を認めていた |
| 核はヘッドレス | `charon-core` は描画・ウィンドウに依存しない | エンジンとゲームが分離していなかった |
| チャンクは 64×64 | 座標変換はシフトとマスク | `CHUNK_SIZE = 100` で除算が最内ループにあった |
| テストが先 | 各タスクの完了条件はテストで示す | 19,000 行にテスト 7 個 |
| どこでも動く描画 | wgpu、WGSL、コンピュートシェーダー不要 | OpenGL 4.3 必須で macOS・モバイル・Web が動かなかった |

## 全体の順序

| | 内容 | 状態 |
|---|---|---|
| M0 | 環境整備 | 完了 |
| M1 | シミュレーション核 (1 スレッド) | 完了 |
| M2 | 反応と熱 | 次に着手 |
| M3 | 並列化と性能 | |
| M4 | 描画とサンドボックス (Android を含む) | |
| M5 | 無限ワールドと保存 | |
| M6 | 剛体とパーティクル | |
| M7 | ライティングと後処理 | |
| M8 | ゲーム層 | 概要のみ。着手前に仕様を書く |

---

## M1 シミュレーション核

読む仕様: 01 の全部、02 の全部。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T1.1 | `coords.rs` (仕様 01 §1) | §1 の必須テストが通る |
| T1.2 | `cell.rs` (仕様 01 §2) | `size_of::<Cell>() == 8` のコンパイル時検査がある。`Cell::default()` が空 |
| T1.3 | `rng.rs` (仕様 01 §4) | 同じ入力で同じ値。`chance(h, 0)` は常に false。`p = 128` で 10,000 回試すと true が 4,500〜5,500 回 |
| T1.4 | `material.rs` (仕様 01 §3)。`assets/materials.ron` は用意済みなので書き換えない | `MaterialTable::builtin()` が 17 材質 (組み込み 2 + ファイル 15) を返す。未定義の参照・名前の重複・検査違反がエラーになる |
| T1.5 | `chunk.rs`、`world.rs` (仕様 01 §5、§6)。`step` は空実装 | `get` / `set` の往復。存在しないチャンクは `BOUNDARY`。`set` で `next_dirty` が 5×5 広がり、チャンク境界では隣にも付く。同じ値の `set` でも dirty は付く (呼ぶ側が避ける) |
| T1.6 | `sim/neighborhood.rs` (仕様 02 §2)。この時点では並列化せず、順に処理する | ローカル座標 `-64..128` の読み書きが正しいチャンクに届く。`swap` が両方の parity を更新する |
| T1.7 | `World::step` の骨格 (仕様 02 §1、§3、§4)。`step_*` と移動は空の関数 | 走査が下から上。parity で二重更新が起きない |
| T1.8 | `testkit` モジュール (`docs/TESTING.md` §2) | `world_from_ascii` と `to_ascii` の往復が一致。`count`、`column_heights` が正しい |
| T1.9 | `can_displace`、`can_rise`、`fall_steps`、`move_powder` (仕様 02 §5〜§7)。テストは `tests/powder.rs` | `sand_falls`、`sand_piles`、`friction_makes_steeper_piles`、`chunk_border` |
| T1.10 | `move_fluid` (仕様 02 §8)。液体と気体の両方。テストは `tests/fluid.rs` | `water_levels`、`sand_sinks_in_water`、`oil_floats_on_water`、`slime_is_slower_than_water`、`gas_rises`、`water_flows_through_tunnel`、`breached_wall_drains` |
| T1.11 | 休眠のテスト (仕様 02 §9)。`tests/sleep.rs` | `settles_to_sleep`、`full_pool_sleeps`、`partial_layer_sleeps`、`gas_under_ceiling_sleeps`、`mixed_liquids_sleep` |
| T1.12 | 全体のテスト。`tests/invariants.rs`、`tests/determinism.rs` | `mass_is_conserved`、`deterministic`、`golden_hashes` (`docs/TESTING.md` §4) |
| T1.13 | ベンチマーク (`benches/sim.rs`、criterion)。砂 10 万個の落下、水 10 万個の流れ、休眠した世界 | 実行でき、結果を `docs/BENCHMARKS.md` に記録する |

M1 の完了条件: 上の全テストが通り、`scripts/check.sh` が通る。

## M2 反応と熱

読む仕様: 03 の全部。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T2.1 | `convert` 関数 (仕様 03 §1 の末尾)。材質が変わる処理をここに集める | `temp` と `shade` を引き継ぎ、`life` を引き直す |
| T2.2 | `ReactionTable`、`step_reaction` (仕様 03 §1)。`assets/reactions.ron` は用意済み | `water_lava_makes_stone_and_steam`、`reaction_is_symmetric` |
| T2.3 | `step_heat` (仕様 03 §2) | `heat_is_conserved`、`heat_flows_hot_to_cold` |
| T2.4 | 相変化 (仕様 03 §3) と読み込み時の振動検査 | `water_boils_and_condenses`、`phase_loop_is_rejected` |
| T2.5 | `step_lifetime` (仕様 03 §5) | 寿命つきの材質が `decay_into` に変わる |
| T2.6 | `step_burning` (仕様 03 §4)。材質は `materials.ron` に定義済み | `wood_burns_to_ash`、`fire_dies_out`、`methane_explodes_into_fire` (メタンに火を付けると炎になって消える) |
| T2.7 | 全体のテストとベンチマークの追加 (反応と熱が動いている世界) | `deterministic`、`everything_sleeps` (全材質を乱数で撒いた世界が 20,000 tick 以内に眠る)。M1 のテストがすべて通ったまま。`golden_hashes` は `CHARON_BLESS=1` で更新する |

## M3 並列化と性能

読む仕様: 02 §2、§11。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T3.1 | `parallel` feature (既定で有効) と rayon。仕様 02 §2 の手順 3 を `par_iter_mut` にする | `parallel_matches_serial`: feature の有無で毎 tick の `state_hash` が一致。CI で両方を実行 |
| T3.2 | スレッド数を 1、2、4、8 に変えた決定性テスト | すべて同じ `state_hash` |
| T3.3 | プロファイルを取り、遅い箇所を直す。**`unsafe` は使わない** | 仕様 02 §11 の目標を満たす。満たせない場合は実測値と原因を `docs/BENCHMARKS.md` に書き、目標の見直しを提案する |
| T3.4 | 休眠の検証 | 休眠した 10,000 チャンクの世界で 1 tick が 0.2 ms 以内 |
| T3.5 | 休眠の計測をサンドボックス用に公開する: `World::awake_chunks()` と、直前の tick で処理したセル数 `World::last_tick_cells()` | 眠った世界で両方が 0 |

`unsafe` がどうしても必要だと判断した場合は、実装せずに作業を止め、理由と実測値を報告すること。

## M4 描画とサンドボックス

読む仕様: 06 の §1〜§4、§6〜§9。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T4.1 | `crates/charon-render` を作る。`GpuContext` (アダプター、デバイス、サーフェスの生成と作り直し) | 仕様 06 §2 の規則 3 のとおりに初期化する |
| T4.2 | セルの変換、層の割り当て、パレット生成 (GPU を使わない部分) | `cell_to_texel`、`layer_allocator`、`palette_bytes` |
| T4.3 | `chunk.wgsl` とセルの描画パス (仕様 06 §3) | `shaders_parse`。デスクトップで固定の世界が表示される |
| T4.4 | `RenderPass` の仕組みと背景パス、全画面パスの共通部品 (仕様 06 §4) | パスをリストに足すだけで描画順に入る |
| T4.5 | `crates/charon-app`: winit のイベント処理、固定 tick のループ (仕様 06 §6、§7)。`resumed` / `suspended` の手順を守る | `fixed_timestep`。ウィンドウを最小化して戻しても動く |
| T4.6 | 入力の正規化 (`PointerEvent`) | マウスで砂を置ける |
| T4.7 | `apps/sandbox` と egui の UI (仕様 06 §8)。ワークスペースの `members` に `apps/*` を足す | 仕様 06 §8 の全機能 |
| T4.8 | Android ビルド。`docs/BUILD_ANDROID.md` を書く | 実機かエミュレーターで起動し、タッチで砂を置ける。ホームに戻って復帰してもクラッシュしない |
| T4.9 | 各プラットフォームでの手動確認 | `docs/PLATFORM_CHECK.md` に結果を記録 |

## M5 無限ワールドと保存

読む仕様: 04 の全部。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T5.1 | `crates/charon-world`。チャンクのデータの書き出し・読み込み (仕様 04 §3) | `roundtrip`、`empty_chunk_is_small`、`material_reorder`、`unknown_material_becomes_empty` |
| T5.2 | リージョンファイル | `crash_safety`、`newer_version_is_rejected` |
| T5.3 | `ChunkGenerator`、`FlatGenerator`、`NoiseGenerator` (仕様 04 §4) | `generator_is_pure` |
| T5.4 | 読み込みと退避 (仕様 04 §2)。作業スレッドと固定の遅延 | `walk_away_and_back`、`streaming_is_deterministic` |
| T5.5 | サンドボックスに組み込む。カメラを動かすと世界が広がる | 遠くまで移動して戻っても状態が保たれる。終了して再起動しても同じ |

## M6 剛体とパーティクル

読む仕様: 05 の全部。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T6.1 | `crates/charon-physics`。rapier の初期化、`PixelBody`、輪郭抽出とコライダー生成 (仕様 05 §7 の「生成」) | 四角形と L 字のセル配列から、正しい質量と重心の剛体ができる |
| T6.2 | スタンプとアンスタンプ (仕様 05 §3)。押しのけは未実装 (重なったセルは無視) | `stamp_has_no_holes`、`stamp_unstamp_restores_world` |
| T6.3 | 地形のコライダー (仕様 05 §8) | 剛体が地面に乗って止まる。地面を掘ると落ちる |
| T6.4 | パーティクル (仕様 05 §6) | 投げたパーティクルが着地してセルに戻る。数が保存される |
| T6.5 | 押しのけ (仕様 05 §4) | `displace_conserves_mass`、`c_shape_holds_contents`、`c_shape_pours_when_rotated`、`c_shape_pours_water`、`carried_contents_move_with_body` |
| T6.6 | セル → 剛体の力 (仕様 05 §5) | `boat_floats`、`stone_sinks_wood_floats`、`filled_cup_is_heavier` |
| T6.7 | 加工と分割 (仕様 05 §7) | `body_splits_when_cut` |
| T6.8 | 1 tick の順序を仕様 05 §2 のとおりに統合。サンドボックスで剛体をつかんで動かせるようにする | `deterministic`。手で C 字の容器を回して中身が落ちることを確認 |
| T6.9 | 剛体の保存 (`bodies.bin`) | 保存して読み込むと、剛体の位置・形・速度が同じ |

## M7 ライティングと後処理

読む仕様: 06 §5。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T7.1 | 準備パスと伝播パス | 発光する材質の周りが明るくなる。洞窟の中が暗い |
| T7.2 | 合成パスと `LightQuality` | `Off` / `Low` / `High` を切り替えられる |
| T7.3 | 後処理の例を 1 つ (ブルーム) | WGSL ファイル 1 つとパスの登録 1 行で追加できている |
| T7.4 | モバイルでの計測 | Android の実機で 60 フレーム毎秒を保てる設定を既定にする。結果を `docs/BENCHMARKS.md` に記録 |

## M8 ゲーム層 (概要)

M8 はまだ仕様がない。**着手する前に、M1〜M7 と同じ粒度の仕様を `docs/spec/` に書くこと。**
対象は次のとおり。

- エンティティとプレイヤー (当たり判定はスタンプの仕組みを使える)
- バイオームと構造物の生成
- 背景レイヤー、建築
- 剛体の温度、爆発
- MOD (データの追加読み込み) とスクリプト
- マルチプレイ (決定性を前提にした入力同期)
- Web 対応

---

## 仕様の確かさについて

仕様 01〜06 の式と定数 (確率、係数、上限) は、実装前に決めた **初期値** である。
実装してテストやベンチマークで問題が見つかった場合は、仕様の文書を直してから実装を合わせる
(手順は `CLAUDE.md`)。テストの内容 (何が起きるべきか) は変えず、定数のほうを調整する。
