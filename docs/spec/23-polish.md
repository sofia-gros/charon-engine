# 仕様 23: 仕上げ (M26)

製品版に向けて、すぐに遊べる状態にするための、小さな仕組みと確認。

## 1. 死亡と復活

`GameState::step_respawn(world, entities, spawn)` を毎 tick 呼ぶ (App の `Sim::tick` が呼ぶ)。倒れたプレイヤー (杖を持つ、敵でない) を `RESPAWN_TICKS` (180 tick = 3 秒) 数え、たったら `spawn` (左上のセル) で復活させる。

- 体力・息・マナが満タン。状態異常は消える。速度は 0。**持ち物・装備はそのまま**。
- 出発点が地形に埋まっていたら、上へ 1 セルずつ (最大 96) 探す。どこにも置けなければ、倒れたまま次の tick にまた試す。
- `GameEvent::Respawned`。`GameState::death_ticks` (描画用)。
- 倒れている間、画面に `You died` と `Respawning in N` を出す。敵は復活しない (撤去される)。

## 2. 最初のヒント

世界の始まりの 25 秒 (1500 tick) だけ、画面の下に操作を出す (タッチとデスクトップで文が違う)。

## 3. デスクトップの操作

左上にホットバー (10 枠、数字つき)。数字キー `1`〜`0` で枠を選ぶ。左ボタンで、選んだアイテムをカーソルの場所に使う (ブロックを置く・つるはしで壊す・消耗品を使う。武器を選んでいれば撃つ・斬る)。`I` で持ち物・クラフトの画面。

## 4. 音

効果音に、ブロックを置く (`Place`)・壊す (`Break`)・手応え (`Hit`、近接の命中とボスの撃破) を足した。置き場の `sounds/place.wav|ogg`・`break`・`hit` で差し替えられる (なければ生成した音)。デモのロジックが、出来事 (`BlockPlaced`・`BlockBroken`・`Melee`・`BossDefeated`) で鳴らす。

## 5. 性能の目安

冒険の世界の生成: 画面のまわりの 15×11 チャンク (165 個) で約 0.8 秒、1 チャンクあたり約 5 ms (開発用の最適化つきビルド、Ryzen 7 2700)。`crates/charon-world/tests/terrain.rs` の `generating_the_view_around_the_spawn_is_fast_enough` が、目安を出力し、極端に遅くないことだけを確かめる。100 万セル / 8 ms の目標は未達 (約 35〜41 ms) のまま。

## 6. テスト

| テスト | 場所 |
|---|---|
| `a_dead_player_respawns_at_the_spawn_point_with_full_vitals` ほか 3 件 | `crates/charon-game/tests/respawn.rs` |
| `hints_death_overlay_and_desktop_hotbar_draw` | `crates/charon-app/src/demo_ui.rs` |

## 7. まだやっていないこと (リリースの前に)

- **実機の確認**: Windows のウィンドウでの操作・見た目、Android 実機のタッチ (仕様 18)・移動 (M21)・音。自動テストと、描画のテスト (GPU) までで、人の目で通して遊んでいない。
- BGM、日本語の文字 (今は英語の表示)、昼夜、署名つきの配布、アセットの権利の確認、通しプレイ 1〜2 時間。
- 溶岩の熱が眠る (仕様 21 §9、持ち主の確認待ち)。
- ブロックの置く・壊すの通信 (T19.5)。
