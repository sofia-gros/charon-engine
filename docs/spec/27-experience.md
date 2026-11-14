# 仕様 27: 第5段階の音・画面と配布前の確認 (M33)

2026-11-15。第4段階の生成資源→装備→ボス→保存 (仕様 26) の次に進める、[ROADMAP の第5段階](../ROADMAP.md) の実装前の契約。[PRODUCT_REVIEW §4.8–4.9・§4.14・§9](../PRODUCT_REVIEW.md)、[仕様 12](12-audio.md)、[仕様 15](15-assets.md)、[仕様 18](18-terraria-touch.md)、[仕様 23](23-polish.md) に従う。他社の音・絵・文字・名称を複製しない。実機基準がない M28 の 100% 比較は保留のままで、この段階の自動テストをもって代えない。溶岩の熱休眠は計測前に変更しない。マルチプレイは現時点で対象外とし T19.5 を先取りしない。

## 1. 依存順と必須テスト

| タスク | 境界 | 必須テスト |
|---|---|---|
| T33.0 第5段階の契約 | 依存順・実装と実機の境界を固定する。コードは変更しない | `scripts/check.sh` |
| T33.1 空間音 (§2) | 音源と聴取位置を使う効果音。従来の位置なし API は維持 | `spatial_sound_uses_position_and_distance`、`spatial_sound_follows_the_listener_without_changing_state`、`scripts/check.sh` |
| T33.2 BGM (§3) | 音楽系統の曲選択・切替時のフェードと欠損時の無音。オリジナル音源の用意と権利確認は別に記録 | `music_transitions_without_overlapping_forever`、`music_missing_asset_is_silent_and_sfx_continue`、`scripts/check.sh` |
| T33.3 日本語の UI (§4) | 文字列の切替と日本語を表示するフォント。ライセンスを確認できるファイルだけを同梱 | `ui_language_falls_back_for_missing_strings`、`japanese_glyphs_render_in_the_ui`、`scripts/check.sh` |
| T33.4 演出・UI (§5) | 第5段階のアイコン・説明・出来事の表示。位置とジェスチャーは仕様 18 を維持 | `item_tooltips_show_definition_values`、`combat_feedback_uses_real_events`、`scripts/check.sh` |
| T33.5 配布前の検証 (§6) | 自動の通し・性能記録・同梱物の権利一覧。署名と実機通しは持ち主の設備・判断が必要 | `experience_reload_retains_settings_and_progress`、`bundled_assets_have_a_recorded_origin`、`scripts/check.sh` |

各タスクに着手する直前に、該当節へ公開型・計算式・更新順・異常入力・保存との境界・テスト条件を具体化する。未確定の節の規則を先取りして実装しない。`docs/TESTING.md` に従い、ゲーム場面は seed 1..=8 を基本にし、1 タスクごとに全チェックを通し、1 コミットにする。描画が変わるタスクでは実際の表示を確認できる場合は画像を共有し、確認できなければ自動テストと実機未検証を区別する。Android の実行は既存の免除に従い未検証と記す。

## 2. T33.1: 空間音

既存の `charon-audio::Mixer` のパンと音量 (仕様 12) を使い、効果音だけに位置を付ける。音源・聴取者は世界の `CellPos`。`charon-app::audio::spatial_params(source, listener) -> Option<(f32, f32)>` は `dx = source.x - listener.x`、`dy = source.y - listener.y` を **i64** で計算し、`distance = max(abs(dx), abs(dy))` とする。`distance >= 128` なら `None` (鳴らさない)。そのほかは `pan = clamp(dx, -64, 64) / 64`、`gain = (128 - distance) / 128`。除算は音の演出に限り `f32` で行い、セル・ゲームのシミュレーションへ渡さない。音源と聴取者が同じときは中央、音量 1。距離 64 では音量 1/2、128 以上では無音。極端な座標でも桁あふれせず、常に有限の値になる。

`Audio::play_at(event, source, listener)` は、効果音の既存の基準音量に `gain` を掛け、`pan` を `PlayParams` に渡す。`Bus::Sfx`、ミキサーの 32 音上限、設定の master/sfx/music 音量、音声デバイス有無の挙動はそのまま。位置がない `Audio::play_events` / `Audio::update` は中央のままで、既存の再生順・音量を変えない。掘る音の開始/停止は従来どおり `update` でのみ処理し、`play_at` に渡されたら何もしない。ゲームロジックには `Context::play_at(SoundEvent, CellPos)` を加え、`Effects::spatial_sounds` に順序どおり蓄積する。既存 `Effects::sounds` と合わせて `SOUNDS_MAX = 64` を超えない。従来の `Context::play` / スクリプトは変更しない。描画の音処理では、既存 `update`、位置なしの `effects.sounds`、位置つきの `effects.spatial_sounds` の順に再生し、聴取者はそのフレームのプレイヤーの中心セルとする。プレイヤー不在なら位置つき音は再生しない。

デモの `GameEvent::BlockPlaced` / `BlockBroken` はブロックの左上セル (`BlockPos::origin`、桁あふれなら音なし)、`BossDefeated` はイベントのセル、`Melee` は対象エンティティの中心セルを音源として `Context::play_at` を呼ぶ。対象がすでに存在しなければ音は出さない。従来の揺れと GameEvent の内容・個数は維持し、他の出来事の音を増やさない。音だけの位置づけはゲームの状態ハッシュ・セル・同期入力に影響しない。`SoundEvent` は追加・変更しない。

| テスト (`charon-app`) | 確認 |
|---|---|
| `spatial_sound_uses_position_and_distance` (`audio.rs`) | 同じ位置は中央・元の音量、右左でパンが反転し、64 セルで半分、128 以上は無音。極端な i32 座標でも有限。2 チャンネルの出力を聴取位置と比較する |
| `spatial_sound_follows_the_listener_without_changing_state` (`api.rs`) | seed 1..=8。ロジックから位置つきの音を積み、順序・合計 64 の上限と位置なしの音を確認。聴取位置の違いだけで出力が変わり、世界・エンティティ・ゲームのハッシュが変わらない |

## 3. T33.2: BGM

着手前に具体化する。既存の `Music` bus と設定の `music_volume` を使う。既定の音源を無断で外部から取り込まない。仕様 15 の OGG 読み込み上限 (約 4 分) のままであれば**ストリーミング再生は未実装**と明記し、長時間再生のメモリを測るまで製品 BGM 完了としない。

## 4. T33.3: 日本語とフォント

着手前に具体化する。使用する字体の出典・ライセンスを確認してからファイルを同梱する。既存の英語画面と MOD の文字列を壊さず、利用できない文字列は安全に戻す。

## 5. T33.4: 演出と画面

着手前に具体化する。実際に記録されたゲームの出来事を使い、架空のダメージを表示しない。M28 の実機画像との差分・操作の完全一致は、この節のテストだけでは主張しない。

## 6. T33.5: 配布前の確認

着手前に具体化する。署名鍵の作成・保管、法的な判断、Windows/Android の実機通し、M28 の実機 Terraria 比較は自動テストで代替できないため、別の未検証事項として残す。テストの結果と権利の一覧は確認した範囲だけを記録する。
