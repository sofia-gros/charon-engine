# 変更履歴

互換性の方針は [docs/API_POLICY.md](docs/API_POLICY.md)。`0.x` の間は、安定の範囲 (`charon_app::prelude`) を壊す変更はマイナーの版で行い、ここに書く。

## 0.0.4 (予定)

### 追加

- **ゲームの入口**: `Game` トレイト (`on_start`・`on_tick`)、`Context`、`GameLogic`、`AppOptions::logic`。画面の揺れ・音は `Context::shake` / `play` で頼む (仕様 14)。
- **`charon_app::prelude`**: ゲームを作る人が使う名前をまとめた (安定の範囲)。
- **アセット**: PNG・WAV・OGG の読み込み、置き場の上書き、`SpriteSpec`、`SoundBank::from_assets` (仕様 15)。
- **スクリプト**: Rhai による MOD のスクリプト `script.rhai` (`on_start`・`on_event`・`on_tick`、命令の上限つき、仕様 16)。
- **状態異常**: 燃える・濡れる・毒 (仕様 10 §17)。
- **浮遊**: 燃料つきのジェット (仕様 10 §18)。
- **ドロップ**: 落ちているものと拾うこと、持ち物から落とす、掘ったものをドロップにする設定 (仕様 10 §19)。
- 音声デバイスへの出力 (`cpal`)、問い合わせ (raycast・視線・円の検索)、`GameEvent`、カメラのなめらかな追従と揺れ、地形の崩落。

### 壊れるもの

- `GameEvent::Defeated(EntityId)` は `Defeated(EntityId, CellPos)` になった (倒れた位置が付く)。
- `PlayerLoadout` に `levitation` が増えた。構造体を直接書いている場合は `levitation: None` を足す。
- 増えそうな `enum` を `#[non_exhaustive]` にした (`GameEvent`、`GameError`、`StatusKind`、`SoundEvent`、`AssetError`、`Command`)。`match` に `_ =>` が要る。

### 開発

- 開発用ビルドのデバッグ情報を行番号だけにした (Windows のリンカーの PDB の上限を避けるため)。

## 0.0.3

音声のミキサー、通信の層、ポーズのメニュー、持ち物の画面、設定とキー割り当て、掘る道具、アニメーション、Android の横画面固定。
