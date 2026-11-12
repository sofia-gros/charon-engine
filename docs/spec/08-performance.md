# 仕様 08: M9 デスクトップのシミュレーション性能

## 1. 対象と不変条件

M9 は Windows 上の `charon-core` の性能を測り、実測で特定した箇所を最適化する。描画、Web、Android の実機動作は対象外であり、未検証の項目を達成済みと扱わない。

- 仕様 01〜03 の材質・乱数・走査順・休眠と、セルの `set` / `swap` / `keep_awake`、材質変更の `convert` を保つ。
- `unsafe`、シミュレーションに影響する時刻・非決定的な反復順、`charon-core` の新しい依存は追加しない。
- 最適化では毎 tick の `state_hash`、並列／逐次の結果、材質の個数、最終的な休眠と既存の基準ハッシュを変えない。
- 仕様 02 §11 のデスクトップ 100 万セル / 8 ms は長期目標。測定前に達成を約束したり、目標に届かなかった結果を隠したりしない。

## 2. T9.1: 再現可能な計測場面

`testkit::falling_million` を 100 万セル場面の唯一の生成関数とし、ベンチマークと場面テストの両方で使う。

1. seed 1 の `box_art(1024, 2048)` を作る。内側の `x = 1..=1024`、`y = 0, 2, ..., 2046` に砂を置く (計 1,048,576 個)。
2. `World::step()` を 1 回実行して tick 1 に進める。初期 parity と一致して更新されない tick 0 は計測しない。
3. Criterion の `iter_batched` では生成を測定対象から外し、生成済みの tick 1 の世界に対する **1 回の** `step()`、逐次側では `step_serial()` を測る。
4. `benchmark_fixture_updates_cells` は tick 1・砂の個数・空行への実際の移動とその後の砂の個数保存を検査する。`last_tick_cells` だけでは移動の証明としない。
5. `docs/BENCHMARKS.md` にコマンド、日付、CPU、OS、feature、ベンチ名、中央値を追記する。既存の測定値は上書きしない。1 回の測定で 8 ms 達成可否を一般化しない。

## 3. T9.2: ボトルネックの特定

T9.1 と同じ場面・同じマシンで、プロファイルまたは局所計測から、セル更新、熱・反応判定、チャンクの取り出し／復元などの内訳を調べる。計測方法・サンプル数・実測値・限界を `docs/BENCHMARKS.md` に追記する。最大の実測コストと変更するコードパスを特定し、**T9.3 に着手する前に**この節に処理の前後条件、変更を許す範囲、期待する計測結果を具体的に追記する。測定できなければ推測で方式を決めず、理由と不足している計測を報告する。

2026-11-12 の局所計測では、9 フェーズの更新処理が計測した tick のほぼ全時間を占め、粉体セルを 16 セル間隔で採取した 8,192 サンプルでは移動段階の累積時間が他の各段階より大きかった (値と計測上の制限は `docs/BENCHMARKS.md`)。よって T9.3 は **`sim/movement.rs` の `move_powder` の真下への移動部分のみ**を変更する。

現在は真下のセルを `below = nb.get(x, y + 1)` で読み `can_displace(materials, cell, below)` が真であると確かめ、液体への抵抗判定と `fall_steps` の後、`while` の最初の条件で同じ位置を再び `nb.get` / `can_displace` している。判定後から最初の `swap` まで `Neighborhood` を変える処理はない。**初回だけ**確認済みの真下へ `nb.swap(x, y, x, y + 1)` し、`cy = y + 1`、`moved = 1` として、残りの歩数だけ従来と同じ順序で `can_displace` と `swap` を繰り返す。液体への抵抗・乱数・`fall_steps`・移動後の `FLAG_FALLING` と `vy`・`wake_neighbors` と、斜め・停止時の処理は一切変えない。初回判定が偽のときの処理も変えない。

期待する計測結果は、同じ tick 1 の `falling_sand_1m` の中央値が T9.1 の 82.554 ms より短くなること。測定誤差だけの可能性があるため同じ CPU・コマンドで再計測し、改善が確認できなければこの変更を採用せず測定結果を残す。8 ms 達成は期待値に含めない。

## 4. T9.3: 測定箇所の最適化

§3 で確定したコードパスのみを変更する。変更の前後で同じ CPU と計測条件を使い、`optimization_matches_baseline` で seed 1〜8 の毎 tick の `state_hash` を比較する。場面は `world_from_ascii(&box_art(62, 64), seed)` に砂を `(12, 3)..=(20, 10)`、水を `(25, 0)..=(33, 8)` の矩形に `testkit::fill` で配置する。変更前のコードから tick 1〜16 のハッシュを生成し、`tests/golden/optimization.txt` に seed 昇順・tick 昇順で固定する。テストはこの全 128 個と毎 tick 比較し、最適化後に基準を作り直さない。`parallel_matches_serial`、`golden_hashes`、`mass_is_conserved` と休眠テストも通す。性能が改善しなければ最適化を採用せず、測定結果と原因を記録する。挙動を変える必要が判明したら `CLAUDE.md` の仕様不一致の手順に従う。

## 5. T9.4: 再計測と状態の確定

`falling_sand_1m`、`falling_sand_1m_serial`、`falling_sand_100k`、`falling_water_100k`、`reaction_heat_6k`、`sleeping_10k_chunks` を再計測する。測定環境・中央値・目標の達否・残るボトルネックを履歴に追記する。`scripts/check.sh` を通し、`docs/ROADMAP.md` と `README.md` の状態を実測に合わせる。

| テスト名 | 確認すること | 追加するタスク |
|---|---|---|
| `benchmark_fixture_updates_cells` | tick 1 からの 1 tick に実際の砂の移動と個数保存がある | T9.1 |
| `optimization_matches_baseline` | 複数 seed で最適化前後の毎 tick の状態ハッシュが一致する | T9.3 |

性能値は CI の合否条件にしない。毎回の速度差は同一環境の Criterion 計測で報告し、機能の合否は決定性と既存の自動テストで判定する。
