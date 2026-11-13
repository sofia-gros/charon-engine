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
| [07-game-layer](spec/07-game-layer.md) | エンティティ、生成、建築、剛体熱、MOD、同期、Web |
| [08-performance](spec/08-performance.md) | デスクトップ性能の再測定・分析・決定性を保つ最適化 |
| [09-demo-scene](spec/09-demo-scene.md) | FallingSandEngine の参照動画に合わせたデモ場面 |

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
| M2 | 反応と熱 | 完了 |
| M3 | 並列化と性能 | 完了 (100 万セルの性能目標は未達、実測と見直し案を BENCHMARKS.md に記録) |
| M4 | 描画とサンドボックス (Android を含む) | 完了（Android 実機確認は依頼者の指示で免除、Linux/macOS は未検証） |
| M5 | 無限ワールドと保存 | 完了（Windows 実機で移動・復帰・再起動を確認、Android 実機は依頼者の指示で免除） |
| M6 | 剛体とパーティクル | 完了（Windows の T6.8 実機操作・撮影は依頼者の指示で免除、未検証。Android 実機も免除、未検証） |
| M7 | ライティングと後処理 | 実装・自動テスト完了（T7.4 の Android 60 fps 実測は依頼者の指示で免除、未検証。Windows GUI 目視・撮影も未実施） |
| M8 | ゲーム層 | T8.1〜T8.8 の実装と自動テストを完了。T8.8 の実ブラウザー動作・保存は依頼者の指示で後日検証（未検証）。Windows GUI の操作・撮影も未検証 |
| M9 | デスクトップ性能の計測と改善 | T9.1〜T9.4 の計測と T9.5 の RTX 4060 オフスクリーン描画検証を完了。最適化案は不採用。100 万セル / 8 ms は未達 (T9.6 で約 35〜41 ms まで短縮)、休眠 10,000 チャンク / 0.2 ms は達成。Windows GUI 操作は未検証 |
| M10 | 参照デモ場面 (`charon-demo`) | T10.1〜T10.6 の実装と自動テストを完了。RTX 4060 (Vulkan) のオフスクリーン描画で 5 場面を確認。ウィンドウでのマウス・キー操作の目視は未検証 (操作許可が得られなかった)。Web・Android・Linux は未検証 |
| M11 | Noita 型ゲームの中核 (体力・飛び道具・杖・敵・掘る) | T11.1〜T11.4、T11.6〜T11.8 (体力・環境ダメージ・飛び道具・爆発ダメージ・杖と呪文・ゲーム状態の組み込み・定義層・敵・ワイヤーフック) の実装と自動テストを完了。デモは左クリックで杖、右クリックでワイヤーフックを使え、部屋に敵がいる (ウィンドウ操作は未検証)。T11.5 (掘る・浮遊) は未着手 |

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

M4 の実機結果は `docs/PLATFORM_CHECK.md` に記録した。2026-11-11 の依頼者指示により、
T4.8 と T4.9 の Android 実機・エミュレーター検証は免除する（成功とみなすものではない）。
Linux と macOS の実機は未検証で、結果を明記した。

## M5 無限ワールドと保存

読む仕様: 04 の全部。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T5.1 | `crates/charon-world`。チャンクのデータの書き出し・読み込み (仕様 04 §3) | `roundtrip`、`empty_chunk_is_small`、`material_reorder`、`unknown_material_becomes_empty` |
| T5.2 | リージョンファイル | `crash_safety`、`newer_version_is_rejected` |
| T5.3 | `ChunkGenerator`、`FlatGenerator`、`NoiseGenerator` (仕様 04 §4) | `generator_is_pure` |
| T5.4 | 読み込みと退避 (仕様 04 §2)。作業スレッドと固定の遅延 | `walk_away_and_back`、`streaming_is_deterministic` |
| T5.5 | サンドボックスに組み込む。カメラを動かすと世界が広がる | 遠くまで移動して戻っても状態が保たれる。終了して再起動しても同じ |

M5 の Windows 実機確認: カメラ移動後に生成地形が表示され、元の位置へ戻ると元のエリアが保持された。
通常終了後に `saves/default/world.ron` とリージョンファイルを作成し、再起動して同じ地形を復元した。
画像は `docs/screenshots/T5.5-initial.png`、`T5.5-pan.png`、`T5.5-return.png`、`T5.5-restart.png`。
Android 実機確認は依頼者の 2026-11-11 の指示により免除し、未検証として扱う。

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

T6.8 の Windows 実機操作・スクリーンショットは、2026-11-11 の依頼者判断で未検証のまま
免除して後続タスクへ進む。自動テストは通過したが実機動作の成功とはみなさない。
Android 実機検証も従来の依頼者指示で免除し、未検証として扱う。
詳細は [`PLATFORM_CHECK.md`](PLATFORM_CHECK.md) に記録した。

## M7 ライティングと後処理

読む仕様: 06 §5。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T7.1 | 準備パスと伝播パス | 発光する材質の周りが明るくなる。洞窟の中が暗い |
| T7.2 | 合成パスと `LightQuality` | `Off` / `Low` / `High` を切り替えられる |
| T7.3 | 後処理の例を 1 つ (ブルーム) | WGSL ファイル 1 つとパスの登録 1 行で追加できている |
| T7.4 | モバイルでの計測 | Android の実機で 60 フレーム毎秒を保てる設定を既定にする。結果を `docs/BENCHMARKS.md` に記録 |

T7.4 の Android 実機・エミュレーター計測は依頼者の継続指示により未検証として免除した。
60 fps 達成も、実機測定に基づく既定値の最適化も主張しない。
Windows の新しいライティング画面も操作ツールのアクセスが拒否されたため目視・撮影は未実施。
GPU オフスクリーンテストでは、空の見える地点が洞窟内より明るいことを確認した。

## M8 ゲーム層

読む仕様: [07 の全部](spec/07-game-layer.md)。既存機能との接点では、各節の先頭に記載した先行仕様も読む。仕様 07 を書くことが実装の前提であり、完了条件は下表のテスト名で確認する。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T8.1 | `crates/charon-game`、`EntityId`、`Entity`、`EntityWorld` (07 §2.1) | `entity_ids_are_monotonic`、`entity_hash_is_stable` |
| T8.2 | `PlayerInput` とプレイヤーの整数移動・スタンプとの衝突 (07 §2.2) | `player_lands_on_solid`、`player_hits_body_stamp`、`player_jump_and_negative_coords`、`invalid_input_is_atomic` |
| T8.3 | バイオームと境界をまたぐ構造物生成 (07 §3) | `biome_boundary_is_pure`、`structure_crosses_chunk_border`、`biome_materials_are_named` |
| T8.4 | 背景レイヤー、建築、保存、描画順 (07 §4) | `building_does_not_overwrite`、`background_does_not_affect_simulation`、`background_roundtrip` |
| T8.5 | 剛体と前景の温度交換、爆発 (07 §5) | `body_heat_is_conserved`、`body_heat_roundtrip`、`explosion_conserves_cells`、`explosion_damages_body` |
| T8.6 | MOD の材質・反応追加と安全な規則データ (07 §6) | `mods_load_in_sorted_order`、`duplicate_mod_material_is_rejected`、`script_rules_are_deterministic`、`script_rule_limit_is_enforced` |
| T8.7 | 同一ビルドの決定的入力同期 (07 §7) | `lockstep_waits_for_all_inputs`、`lockstep_order_is_stable`、`lockstep_rejects_mismatch` |
| T8.8 | Web ビルド、同期チャンク生成、実ブラウザーでの保存と操作 (07 §8) | `web_core_compiles`、`web_streaming_tick_is_deterministic`、`web_save_roundtrip`、`web_input_and_resume` |

Web の実機結果と異種 OS の入力同期は、自動テストだけから成功を推定しない。Android 実機検証は依頼者の継続指示により免除し、未検証と記録する。
2026-11-12 の依頼者指示で、T8.8 の Web 実機確認は後日実施に変更。Windows 版の起動を当面の確認範囲とし、
Web の `web_save_roundtrip`（ブラウザー）と `web_input_and_resume` は未検証のままにする。
これらの完了を主張せず、未検証項目は [`PLATFORM_CHECK.md`](PLATFORM_CHECK.md) に残す。

## M9 デスクトップのシミュレーション性能

読む仕様: [08 の全部](spec/08-performance.md)、02 §1〜§4・§9〜§11、01 §5〜§6、03 §1〜§5。実装で変更する規則は先行仕様の本文を再確認する。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T9.1 | 100 万セル場面の生成を `testkit::falling_million` に一本化し、更新 tick の測定条件と変更前の基準値を固定する (08 §2) | `benchmark_fixture_updates_cells`。同一環境の Criterion 中央値と条件を `BENCHMARKS.md` に追記 |
| T9.2 | CPU の処理内訳を測り、最大の実測ボトルネックと T9.3 の変更範囲を仕様 08 §3 に確定する | 計測方法・内訳・再現条件を `BENCHMARKS.md` に追記。推測を測定結果と扱わない |
| T9.3 | 08 §3 で確定した箇所だけ最適化し、同一環境で変更前後を比較する | `optimization_matches_baseline`、`parallel_matches_serial`、`golden_hashes`、`mass_is_conserved`、休眠テストが通過。性能が改善しなければ採用しない |
| T9.4 | 100 万・10 万セル、反応・熱、休眠 10,000 チャンクを再計測し、達否を記録する (08 §5) | `BENCHMARKS.md` に履歴を追記し `scripts/check.sh` が通過。ロードマップと README の状態を実測と一致させる |
| T9.5 | Windows の実 GPU で既存のオフスクリーン描画を検証する (06 §5・§9)。ソフトウェアアダプターへの置換や GPU 未取得時のスキップを許さない | `light_pass_renders_offscreen` が RTX 4060 を選択して通過。選択したバックエンドと画像を `PLATFORM_CHECK.md` に記録し、GUI の実機確認とは区別する |
| T9.6 | 性能改善プランを立て、空セル書き込み時の監視走査 (`wake_watchers`) を省く (08 §6) | 既存の決定性テストと `phase_counts_follow_every_write` が通過。変更前後の Criterion を `BENCHMARKS.md` に追記 |

T9.1〜T9.4 の計測と自動検証は完了。T9.3 の粉体の初回判定を省く案は改善が確認できず不採用とし、実装を戻した。T9.4 の並列 100 万セルは 84.509 ms/tick で 8 ms 目標未達、休眠 10,000 チャンクは 43.415 µs/tick で 0.2 ms 目標を達成。測定の条件と他の場面は [`BENCHMARKS.md`](BENCHMARKS.md) に記録した。
T9.5 では RTX 4060 (Vulkan) 上でオフスクリーン描画・GPU からの画素読出しと検査を実行し、画像を保存した。Windows GUI の操作・サーフェス描画は引き続き未検証。
T9.6 は実験で測った上限 (走査を外すと約 82 → 34 ms) をもとに、挙動を変えずに走査を省いた。砂の 100 万セルは約 35〜41 ms/tick (8 ms は未達)、逐次は 592 → 227 ms。ばらつきと水の場面の差は `BENCHMARKS.md`。
Android・Web・Windows GUI の延期中の実機項目は M9 の完了条件に含めず、未検証として残す。

## M10 参照デモ場面

読む仕様: [09 の全部](spec/09-demo-scene.md)、06 §3〜§8、07 §2〜§4。2026-11-12 の依頼で、FallingSandEngine の `demo.mp4` と同じことができる動作デモを作る。「同じ」の範囲と再現しないものは仕様 09 §1 に固定した。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T10.1 | 仕様 09、データ MOD `assets/mods/fse_demo`、`rainbow_index` | `demo_mod_loads`、`rainbow_index_is_periodic` |
| T10.2 | `DemoGenerator` (洞窟・バイオーム・苔・塊・文字・部屋・背景) と `text_cells` | `demo_generator_is_pure`、`demo_room_is_open`、`demo_moss_and_ores`、`demo_text_is_drawn`、`demo_background_label` |
| T10.3 | キーボード入力、プレイヤー操作、追従・自由カメラ、虹色ブラシ | `player_follows_camera`、`keyboard_maps_to_input` |
| T10.4 | 参照画面の UI (メニュー、`Draw`、`Clipboard`、`Registries`、計測グラフ、`Debug` 表示) | `perf_history_is_bounded`、`rainbow_swatches_are_grouped`、`clipboard_copy_paste_roundtrip`、`terrain_edges_follow_solid_boundaries`、`random_seed_is_deterministic_and_changes`、`demo_ui_draws_a_frame` |
| T10.5 | `charon-demo`、剛体 3 つ、プレイヤーの配置、`Tile Entity`。1 tick の更新を `Sim` に分離 | `demo_scene_is_deterministic`、`demo_bodies_are_valid_and_inside_the_room` |
| T10.6 | 実 GPU のオフスクリーン描画、背景の文字、環境光の設定、参照フレームとの比較、記録 | `demo_renders_on_gpu`、`demo_background_is_populated_once`、`PLATFORM_CHECK.md` に結果と未検証項目 |

T10.7 (2026-11-12 の依頼): 部屋の十字 (X 字) の剛体を、その場でくるくる回す。エンジンに `PhysicsEngine::set_spin` (運動学的な剛体として、位置を保ったまま一定の角速度で回す。`None` で動的に戻す) を足し、デモ (ゲーム側) が十字を床から浮かせて指定する。確認は `spinning_body_turns_in_place`、`spin_can_be_stopped_and_rejected`、`spinning_is_deterministic`、`demo_cross_spins` ([`M10-spinning-cross.png`](screenshots/M10-spinning-cross.png))。

T10.8 (2026-11-12 の依頼): 参照の「動く虹色」は、前景のセルではなく**背景のさらに奥で動き回るアニメーションの shader** だった。前景の虹色の岩 (T10.2) を取り除き、エンジンに `DeepBackgroundPass` (視差で動き、自ら動き回り、光の影響を受けず、前景・背景のセルの後ろに隠れる) を足し、デモ (ゲーム側) が `AppOptions::deep_background` で渡す。砂の虹色 (ブラシ) は変えない。確認は `demo_moss_and_ores`、`demo_renders_deep_background`、`deep_background_is_behind_cells` ほか ([`M10-deep-rainbow.png`](screenshots/M10-deep-rainbow.png))。

T10.9 (2026-11-12 の依頼): 剛体の描画を、剛体ごとに**ドット**か**滑らか**から選べるようにする (世界の全体はドットのまま、家具などだけを滑らかに見せる、といった使い方のためのエンジンの機能)。`charon-physics` に `BodyRender`・`set_render`・`body_pose`、滑らかな剛体のスタンプに付く `FLAG_SMOOTH` (`charon-core`、セルの描画では描かない)、`charon-render` に `SmoothBodyPass` (画面の解像度・正確な角度・アンチエイリアスした縁・光を掛ける、`PassList::render_with_bodies`)。デモ (ゲーム側) は、ドットの十字と並べて滑らかな十字を足す。確認は `smooth_body_stamps_flagged_cells`、`demo_has_a_smooth_cross`、`demo_renders_smooth_body` ほか ([`M10-smooth-cross.png`](screenshots/M10-smooth-cross.png))。

T10.6 では RTX 4060 (Vulkan) で 5 場面をオフスクリーンに描き、参照動画のフレームと見比べた。見え方は [`docs/screenshots/M10-*.png`](screenshots/) にある。参照のフレームはこのリポジトリに入れない。ウィンドウでの操作の目視、Web・Android・Linux の確認は未検証で、成功とは扱わない。

## M11 Noita 型ゲームの中核

読む仕様: [10 の全部](spec/10-noita-like-game.md)、07 §2・§5、02 §1〜§4。2026-11-12 の依頼「noita のようなゲームを作るための機能をそろえる」への対応。既にある世界・材質・熱・燃焼・剛体・爆発・プレイヤー移動の上に、遊びとして必要な層を `charon-game` に足す。実装済みと未実装の一覧は仕様 10 §1。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T11.1 | 体力・息と、熱・溺れによる環境ダメージ (10 §3) | `hot_cells_damage_entity`、`drowning_needs_submerged_head`、`breath_recovers_in_air`、`damage_and_heal_saturate`、`vitals_hash_is_stable` |
| T11.2 | 飛び道具 (爆発・材質を置く)、爆発のダメージ (10 §4) | `projectile_flies_and_falls`、`projectile_explodes_on_solid`、`projectile_hits_entity`、`explosion_damage_falls_off`、`projectile_expires_and_deposits`、`projectile_validation_and_limit` |
| T11.3 | 杖 (マナ・詠唱間隔・再充填) と呪文の表 (RON、名前空間付き) (10 §5) | `spell_table_parses_and_resolves`、`spell_table_rejects_bad_data`、`wand_casts_in_order_and_recharges`、`wand_mana_limits_casting`、`aim_sets_velocity`、`spread_is_deterministic`、`failed_cast_changes_nothing` |
| T11.7 | エンジンの定義層: 呪文・杖・敵の既定 (`assets/*.ron`)、層による追加と `replace` での上書き、MOD からの読み込み、定義の内容ハッシュを同期の照合に使う。デモ固有の定義をエンジンのコードから外す (10 §7)。T11.4 の前に行う | `engine_defaults_load`、`layer_adds_and_replaces`、`layer_rejects_bad_data`、`content_hash_tracks_overrides`、`mod_definitions_load_in_id_order`、`mod_definition_errors_stop_loading`、`lockstep_rejects_different_definitions`、`demo_definitions_come_from_the_game_files` |
| T11.8 | ワイヤーフック (右手の道具)。右クリックでマウスの方向へ物理挙動するワイヤーを撃ち、着弾でプレイヤーを引っ張る (重力でしなる鎖、振り子、巻き取り)。左手 = 武器 (左クリック)・右手 = 道具 (右クリック) の割り当て。`hooks.ron` の定義層、`Lockstep` v3 (10 §9) | `rope_sags_under_gravity`、`rope_flight_anchors_on_solid`、`rope_reeling_shortens_and_stays_taut`、`rope_stays_out_of_solids`、`rope_is_deterministic`、`tether_removes_outward_velocity`、`entity_external_velocity_decays`、`hook_launches_and_anchors`、`hook_pulls_player_toward_anchor`、`hook_swings_without_stretching`、`hook_release_and_cooldown`、`hook_respects_max_length`、`weapon_and_tool_are_independent`、`hook_definitions_layer_and_replace`、`mod_hooks_load_after_engine_defaults`、`hook_is_deterministic`、`lockstep_packet_v3_roundtrip`、`mouse_buttons_map_to_weapon_and_tool`、`demo_hook_in_scene`、`demo_renders_hook` |
| T11.4 | 敵 (視界と視線で目標を選び、追う・跳ぶ・撃つ・接触ダメージ・撤去)。種類は T11.7 の `EnemyDef`、生成はゲームが呼ぶ API `spawn_enemy`、ゲームは `AppOptions::enemies` で配置を渡す (10 §8) | `enemy_spawns_from_definition`、`enemy_chases_target_in_sight`、`enemy_needs_line_of_sight`、`enemy_jumps_over_step`、`shooter_fires_and_hits_target`、`enemy_contact_damage_repeats`、`defeated_enemies_are_removed`、`enemy_ai_is_deterministic`、`step_entity_uses_given_speeds`、`demo_enemies_spawn_in_scene` |
| T11.5 | 掘る道具 (Starbound 型のレーザー、材質の硬さと道具の力で掘る時間が変わる、仕様 10 §12) | `dig_removes_cells_after_hardness_over_power_ticks` ほか [仕様 10 §12.4](spec/10-noita-like-game.md) の表、`hardness_defaults_by_phase`、`lockstep_packet_v4_roundtrip`、`demo_dig_in_scene`。**浮遊は未着手** |
| T11.6 | `GameState` と `Lockstep` (パケット version 2) への組み込み、デモの右ボタン発射・`Wand` パネル・弾の描画 (10 §6)。T11.4 の前に行い、当たり判定を画面で確かめられるようにする | `game_step_order_is_fixed`、`game_rejects_bad_actions_atomically`、`shooter_is_not_hit_by_own_shot`、`dead_characters_do_not_fire`、`lockstep_packet_v2_roundtrip`、`lockstep_game_hash_is_order_independent`、`demo_wand_fires_in_scene`、`demo_renders_wand_panel`、`aim_points_from_player_to_cursor` |

T11.1〜T11.4、T11.6〜T11.8 は自動テストまで完了 (`charon-game` の `vitals`・`projectile`・`wand`・`game` モジュールと `Lockstep`、`charon-app` のデモ)。デモの `Wand` パネルと弾は RTX 4060 (Vulkan) のオフスクリーン描画で確認した ([`M11-wand-panel.png`](screenshots/M11-wand-panel.png))。ウィンドウでの右ボタン操作は、画面操作の許可が得られておらず**未検証**。MOD ディレクトリから `spells.ron` を自動で読む処理は仕様 07 §6 の変更になるため行っておらず、呪文の表は呼び出し側が `SpellTable::parse` に渡す。サンドボックス本体 (`charon-sandbox`) には組み込んでおらず、`charon-demo` だけで撃てる。ワイヤーフックの物理 (`charon_physics::rope`) は浮動小数点で、同一ビルド・同一プラットフォームの決定性だけを確認している (他の環境の一致は主張しない)。ワイヤーは地形の角に巻き付かず、引く向きはアンカーの方向 (手から 3 セル以上離れた最初の点) で、壁にめり込む向きは衝突で止まる。T11.5 は未着手。敵は描画で自分と同じ色の箱に見える (エンティティごとの色はまだない)。完了したタスクだけを完了として扱う。

## M12 遊びやすさと見た目の改善

2026-11-12〜13 の依頼 (実機で試した所見と、見た目・操作の改善)。読む仕様: 07 §2、06 §5、09 §3。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T9.7 | 更新範囲 (`World::set_active_region`): カメラのまわりだけ砂や水を動かし、遠くは保留する (Terraria と同じ)。光源準備のチャンク引きを省き、光の CPU 時間を約半分に | `outside_active_region_is_frozen`、`light_pass_renders_offscreen` |
| T12.1 | プレイヤーを 5×10 に小さくし、当たり判定を箱に内接する楕円にし、楕円に重なった砂を押し出す (`push_out_powder`) (07 §2.2) | `player_collision_is_elliptical`、`player_pushes_sand_out` ほか既存テスト |
| T12.2 | 光の品質 `XHigh` (2 倍の解像度、滑らかな補間、濃い発色) (06 §5) | `demo_renders_xhigh_light` ([`M12-light-xhigh.png`](screenshots/M12-light-xhigh.png)) |
| T12.3 | Noita のような洞窟 (歪めたノイズの大広間・坑道、窪みの水と溶岩) (09 §3) | `demo_caves_are_connected_and_roomy` ([`M12-caves.png`](screenshots/M12-caves.png)) |

プレイ動画の自動撮影は、2026-11-13 の指示で中止した (コードは残していない)。更新範囲の限定 (T9.7) と xHigh・洞窟・プレイヤーの変更は、実際のウィンドウでの操作は未検証 (自動テストと RTX 4060 のオフスクリーン描画で確認)。

## M13 配布とタッチ・ゲームパッド操作

2026-11-13 の依頼。読む仕様: [10 §11](spec/10-noita-like-game.md)。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T13.1 | 仮想スティック (移動・上へスワイプでジャンプ・狙って撃つ) とボタン (`JUMP`・`HOOK`・`PAINT`)、Android のゲームパッドのボタン | `stick_walks_and_swipe_up_jumps`、`aim_stick_fires_toward_the_drag`、`buttons_hold_and_paint_toggles_once`、`touches_outside_the_pad_are_left_to_the_camera_and_brush`、`android_gamepad_buttons_map_to_actions`、`demo_renders_touch_pad` |
| T13.2 | Android の入口を参照デモにする (MOD を埋め込み、内部ストレージへ書き出す)、`run_demo_with` | `cargo check --target aarch64-linux-android` |
| T13.3 | `v*` のタグで Windows (zip)・Android (apk) をビルドして Releases に載せる GitHub Actions (`release.yml`)。最初のタグ `v0.0.1` | タグの push で Release に 2 つの成果物が付く |

デスクトップのゲームパッド (XInput など) は `gilrs` (2026-11-13 に追加を承認) で対応した (`sticks_and_buttons_map_to_actions`、`right_stick_aims_with_screen_down_positive`)。実際のゲームパッドをつないだ確認と Android 実機での確認は未実施。

## M14 画像と UI の大きさ (倍率)

2026-11-13 の依頼「スプライトや画像系はスケール値を設定できるように。キャラクターは 32×64 で作って scale で小さくできる、GUI もサイズ変更可能に」。読む仕様: [06 §10](spec/06-rendering-platform.md)。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T14.1 | `SpritePass`・`Sprite` (RGBA の画像、`scale` = 1 画素あたりのセル数、基準点・回転・反転・補間・光)、`PassList::render_with_sprites`、`AppOptions::player_sprite` (`SpriteAsset`)、デモの 32×64 の魔法使い | `scale_sets_the_size_in_cells`、`a_32_by_64_character_scales_to_5_by_10_cells`、`invalid_sprites_are_rejected`、`content_hash_tracks_pixels_and_size`、`demo_renders_sprite_with_scale` |
| T14.2 | `UI scale` (egui の `zoom_factor`、仮想パッドも)、`Character scale` (`Debug` パネル) | `scale_resizes_the_pad` |

アニメーション (スプライトシート) は T15.1 で入った。アイテムへの画像の割り当ては未着手。ウィンドウでの操作確認は未実施 (RTX 4060 のオフスクリーン描画で確認)。

## M15 アニメーション・掘る道具・持ち物

2026-11-13 の依頼 (優先順に止まらず進める)。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T15.1 | スプライトシートのアニメーション (立つ・歩く・跳ぶ・落ちる)、敵の種類ごとの画像 ([06 §10.4](spec/06-rendering-platform.md)) | `clips_advance_and_loop`、`pose_follows_the_entity`、`frames_pick_the_row_and_column_from_the_sheet`、`sprites_are_assigned_to_the_player_and_to_enemy_kinds`、`facing_flips_when_walking_left_and_is_remembered`、`frames_select_part_of_the_sheet` |
| T15.2 (= T11.5 の掘る部分) | 掘る道具 (Starbound 型のレーザー、硬さ × 力) ([10 §12](spec/10-noita-like-game.md)) | `dig_*`、`hardness_defaults_by_phase`、`lockstep_packet_v4_roundtrip`、`demo_dig_in_scene` |
| T15.4 | ポーズのメニュー・持ち物の画面・設定 `settings.ron`・キー割り当て ([10 §14](spec/10-noita-like-game.md))。Android を横画面に固定 | `settings_roundtrip_and_clamp`、`binding_moves_a_key_from_the_old_action`、`broken_key_tables_are_repaired`、`settings_file_io`、`custom_bindings_change_which_keys_work`、`pause_menu_and_inventory_draw`、`demo_renders_pause_menu` |
| T15.5 | 音: `charon-audio` (依存なしのミキサー・効果音の生成・WAV)、出来事から効果音、音量の設定 ([仕様 12](spec/12-audio.md))。**デバイスへの出力は未実装** (依存クレートの許可待ち) | `charon-audio` の 10 個のテスト、`events_follow_state_changes` ほか、`volumes_are_clamped` |
| T15.6 | 通信: Lockstep の上のホスト中継 TCP `charon-game::net` ([仕様 13](spec/13-network.md))。**アプリへの組み込みは未実装** | `two_peers_stay_in_sync_over_tcp`、`mismatched_peers_are_refused`、`bad_inputs_drop_the_connection`、`malformed_frames_are_dropped`、`connection_info_roundtrips_and_rejects_garbage` |
| T15.3 | 持ち物とゲーム状態の保存 `game.ron` ([10 §13](spec/10-noita-like-game.md)) | `add_take_and_count` ほか、`game_state_roundtrips_through_ron`、`game_save_file_io`、`dig_collects_into_the_inventory` |
| T15.7 | 音声デバイスへの出力 (`cpal`、デバイスのレートに合わせる、開けなければ無音) ([12 §4](spec/12-audio.md))。聞こえるかは未確認 | `channels_are_arranged_for_the_device`、`the_audio_device_opens_or_reports_why_not`、`changing_the_rate_keeps_sound_speed` |

## M16 エンジン API の拡充

2026-11-13 の依頼 (順序は実装者が決め、止まらず進める。確かめ方はデモに足すこと)。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T16.1 | 問い合わせ: raycast・視線・円の検索 ([01 §7](spec/01-core-data.md)) | `raycast_*`、`line_of_sight_and_the_outside_of_the_world`、`circle_search_is_ordered_and_counts` |
| T16.2 | カメラの演出: なめらかな追従・揺れ ([06 §11](spec/06-rendering-platform.md))。デモで撃つ・倒すと揺れる | `following_converges_without_overshooting`、`following_is_frame_rate_independent`、`far_targets_snap_and_smoothing_can_be_off`、`shake_moves_the_view_and_dies_out` |
| T16.3 | 出来事 `GameEvent` ([10 §15](spec/10-noita-like-game.md)) | `dug_cells_arrive_as_events_once_and_the_queue_is_bounded` |
| T16.4 | 地形の崩落 ([10 §16](spec/10-noita-like-game.md))。デモで有効 | `unsupported_cluster_falls_as_one_piece_and_lands` ほか 5 個 (`tests/collapse.rs`) |

## M17 エンジンとして使える入口・アセット・スクリプト・機能の穴

2026-11-13 の依頼 (順序は実装者が決め、止まらず進める)。エディタは作らない。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T17.1 | ゲームの入口 `Game` トレイト・`Context`・`GameLogic` ([14](spec/14-game-api.md))。デモを載せ替え | `game_logic_is_started_once_and_ticked_with_events`、`effects_are_clamped_and_bounded` |
| T17.2 | アセット: PNG・WAV・OGG、置き場の上書き、`SpriteSpec`、`SoundBank::from_assets` ([15](spec/15-assets.md))。OGG は壊れたファイルを断ることだけテスト | `tests` in `assets.rs` 6 個、`sound_bank_falls_back_to_generated_sounds` |
| T17.3 | MOD のスクリプト (Rhai、命令の上限) ([16](spec/16-scripting.md))。デモ MOD の `script.rhai` | `script/tests.rs` 5 個 |
| T17.4 | 状態異常: 燃える・濡れる・毒 ([10 §17](spec/10-noita-like-game.md)) | `tests/status.rs` 4 個 |
| T17.5 | 浮遊: 燃料つきのジェット ([10 §18](spec/10-noita-like-game.md)) | `tests/levitation.rs` 4 個 |
| T17.6 | ドロップ: 落ちる・拾う・落とす・掘ったものをドロップに ([10 §19](spec/10-noita-like-game.md)) | `tests/drops.rs` 7 個 |
| T17.7 | API の安定: `prelude`、`non_exhaustive`、[API_POLICY](API_POLICY.md)、[CHANGELOG](../CHANGELOG.md) | `tests/prelude.rs` 2 個 |
| T17.8 | 性能: 液体・気体の有無の確認を 1 回にする ([08 §7](spec/08-performance.md))。砂の場面が約 5〜9% 速い。8 ms 目標は未達 | 既存のテスト (`golden_hashes` ほか) が同じ値で通る、[BENCHMARKS](BENCHMARKS.md) |

## M18 スマホのタッチ操作の作り直し

2026-11-13 の依頼。Terraria と同じ操作 (移動パッド・ポインター・ボタンでポインターの方へ使う)。

| タスク | 作るもの | 完了条件 |
|---|---|---|
| T18.1 | 左下 1/4 の浮動の移動パッド、1 本指のポインター (プレイヤーの周りの円)、右下の `USE`・`HOOK`・`DIG` ボタン、滑らかなズーム (拡大率を `f32` に。2 本指のスワイプ・ピンチは T18.2 で廃止) ([10 §11](spec/10-noita-like-game.md)) | `touch_pad` の 8 個、`gesture` の 3 個、`zoom_can_be_fractional_and_is_cleaned`、`demo_renders_touch_pad` ([`M18-touch-controls.png`](screenshots/M18-touch-controls.png)) |

| T18.2 | 実機 (Android) の確認からの修正: ピンチ・スワイプのカメラを廃止してズームのボタン (`+` `-`) とカメラの遅れて追う追従に、ジャンプは短押し = ジャンプ・長押し = ホバー (`LevitationDef::delay`)、HUD は右上の小さなバーだけ、パネルは左端のタブに格納、**3 本ほどの指で固まる不具合の修正** (離すイベントは UI が消費しても必ず部品に渡す、同じ部品に触れ直すと古い指を捨てる) ([10 §11・§18](spec/10-noita-like-game.md)) | `zoom_buttons_hold_and_release`、`lost_release_events_do_not_leave_buttons_or_the_stick_stuck`、`panels_dock_to_the_left_and_open_by_touch`、`a_short_press_is_a_plain_jump`、`holding_past_the_delay_hovers_until_the_fuel_runs_out` |

| T18.3 | 更新距離の設定 (コールドフリーズの距離、0 = 自動): `Streamer::set_sim_distance` / `active_radius`、`Controls::sim_distance`、`settings.ron` の `sim_distance`、Debug とポーズのスライダー ([02 §9.4](spec/02-simulation.md))。溶岩が眠らないことの実測 | `active_radius_follows_the_setting_and_clamps`、`chunks_outside_the_sim_distance_are_frozen_and_resume_in_range`、`sim_distance_roundtrips_clamps_and_is_optional_in_old_files`、`smoke_decays_by_itself_and_the_world_falls_asleep` |

| T18.4 | 移動の操作性: 段差の自動乗り越え (`STEP_UP_CELLS` = 4、壁の高さで判定)、コヨーテタイム (6 tick)、ジャンプの先行入力 (5 tick) ([10 §8.4.1](spec/10-noita-like-game.md)) | `tests/movement.rs` の 9 個 |

Android 実機での再確認はこれから (T18.2 の修正は実機で確かめていない)。

## M19〜M26 製品版に向けて

[製品版までの計画](PRODUCT_PLAN.md) の段階。仕様は [17 ブロック](spec/17-blocks.md)・[18 Terraria 型の操作](spec/18-terraria-touch.md)。M19 から着手する (2026-11-13)。

| マイルストーン | 内容 | 状態 |
|---|---|---|
| **M19 ブロック** | `FLAG_BLOCK`・ブロック内の崩れ (T19.1)、「固体」の判定の置き換え (T19.2)、置く・押し出し (T19.3)、壊す・クラフト (T19.4) は**実装・自動テスト済み**。通信のコマンド (T19.5) は、ネットワークの組み込みと一緒に行う ([仕様 17 §9](spec/17-blocks.md))。デモへの組み込みは M22 | 🟡 実装済み (T19.5 とデモは未) |
| **M20 アイテム** | データ駆動のアイテム (`items.ron`)、持ち物の作り直し (`ItemId`)、ホットバー、クラフト (`recipes.ron`・作業台)、装備 5 枠・能力値、自動装備、保存 (名前で保存、古い保存も読める)。[仕様 19](spec/19-items.md)。自動テスト済み。デモの画面は M22 | 🟡 実装済み (画面は未) |
| **M21 体の大きさと移動** (自動テスト済み、実機は未) | プレイヤー・敵を 16×24 に、速度・重力・ジャンプ・乗り越え (1 ブロック) の調整。[仕様 10 §8.4](spec/10-noita-like-game.md) | 🟡 実装済み (実機は未) |
| **M22 建築の操作** | Terraria 型の操作 (仕様 18): 固定スティック・使うボタンのカーソル・スマートカーソル・フックのスティック・ホットバー、インベントリ・クラフト・装備の画面、ブロックを置く・壊す。[仕様 18 §7](spec/18-terraria-touch.md)。チュートリアルは M26 | 🟡 実装済み (自動テスト済み、実機は未) |
| **M23 戦闘** | 近接・遠距離・杖の武器、防具の防御・耐性、ダメージの種類 (物理・火・毒・冷気)、ノックバック、火の玉 (火の筋・発火・燃え広がり)。[仕様 20](spec/20-combat.md) | 🟡 実装済み (自動テスト済み、実機は未) |
| **M24 ワールド** | 冒険の世界の地形生成 (表面・バイオーム・池・洞窟・立て坑・鉱石・地下の水と溶岩・ダンジョンと家・底)。[仕様 21](spec/21-adventure-world.md)。溶岩の熱が眠るのは、シミュレーションの結果が変わるので、持ち主の確認待ち | 🟡 実装済み (熱が眠るのは未、実機は未) |
| **M25 敵とボス** | 武器・防具を持つ敵 (自動装備)・骸骨・弓兵・魔術師・ゴーレム (ボス)・戦利品・体力のバー、自動のスポーンと削除 (深さの規則・上限・距離)。[仕様 22](spec/22-enemies-bosses.md)。経路探索・飛ぶ敵は未 | 🟡 実装済み (自動テスト済み、実機は未) |
| **M27 範囲の外の液体の落下** | コールドフリーズ中のチャンクでも、液体を大雑把に下へ落とす (リリース前に追加)。[仕様 02 §9.5](spec/02-simulation.md) | 🟡 実装済み (自動テスト済み) |
| **M26 仕上げと配布** | 死亡と復活、最初のヒント、デスクトップのホットバー (数字キー)、ブロック・命中の効果音、生成の速さの目安。[仕様 23](spec/23-polish.md)。BGM・日本語・署名つきの配布・権利の確認・実機の通しプレイは未 | 🟡 一部 (実機は未) |

未着手 (次の候補): 装備・アイテムの種類、ネットワークのアプリへの組み込み、BGM・空間音、フォント、経路探索、性能の次の候補 (08 §6 の (b)(c))。

---

## 仕様の確かさについて

仕様 01〜06 の式と定数 (確率、係数、上限) は、実装前に決めた **初期値** である。
実装してテストやベンチマークで問題が見つかった場合は、仕様の文書を直してから実装を合わせる
(手順は `CLAUDE.md`)。テストの内容 (何が起きるべきか) は変えず、定数のほうを調整する。
