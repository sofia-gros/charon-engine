# 公開 API の互換性の方針

ゲームを作る人が、エンジンの更新で急に壊れないための決まり。

## 1. 安定の範囲

- **安定**: `charon_app::prelude` に載っている名前。ゲームはここだけを使えば足りる (仕様 14 §5)。外から見た使い方は `crates/charon-app/tests/prelude.rs` が毎回確かめる。
- **不安定**: それ以外の `pub` (内部のクレートの細部、`charon-render`・`charon-physics` の型など)。予告なしに変わることがある。使うなら、版を固定すること。
- RON のファイル (材質・反応・呪文・杖・敵・フック・掘る道具、`settings.ron`、`game.ron`) と、通信 (Lockstep) は、それぞれ「版」を持つ (下の表)。

## 2. 版の付け方

エンジンは `0.x` の間、次の約束にする (SemVer の 0 系の慣習に合わせる)。

| 変え方 | 版 | 例 |
|---|---|---|
| 互換のある追加 (新しい関数・フィールドにならない追加・新しい出来事) | パッチ | `GameEvent` の新しい種類 |
| 安定の範囲を壊す変更 | マイナー | `Game` トレイトのメソッドの変更 |
| 保存形式・通信の非互換な変更 | マイナー + 下の「版」を上げる | `PROTOCOL_VERSION` |

- 今の配布のタグ (`v0.0.N`) は、デモの版を 1 つずつ上げている。クレートの版 (`Cargo.toml`) は `0.1.0` のままで、公開するときに、この表に合わせて揃える。
- 壊す変更は、`CHANGELOG.md` に「壊れるもの」と「どう直すか」を書く。
- 安定の範囲の関数を消すときは、先に 1 つのマイナーの間 `#[deprecated]` にする。
- `1.0` に上げたら、通常の SemVer (壊す変更はメジャー) にする。

## 3. 将来の追加で壊れないための形

- 増えそうな `enum` は `#[non_exhaustive]` にしてある: `GameEvent`、`GameError`、`StatusKind`、`SoundEvent`、`AssetError`、`Command`。ゲームの `match` には `_ =>` が必要。
- 構造体は、ゲームが作るもの (`AppOptions`、`SpriteSpec`、`LevitationDef` など) は `Default` を持つ。`..Default::default()` で作れば、フィールドが増えても壊れない。
- 設定ファイルは、知らない項目を捨てて既定に戻す (壊れた `settings.ron` でも起動できる)。

## 4. 版の一覧

| もの | 今の版 | 非互換のとき |
|---|---|---|
| Lockstep の通信 (`PROTOCOL_VERSION`) | 4 | 上げる。違う版同士は接続を断る |
| `game.ron` (`GameSave::version`) | 1 | 上げる。読めない版は断る |
| `settings.ron` (`Settings::version`) | 1 | 上げる。読めなければ既定 |
| ワールドの保存 (`world.ron` のメタデータ) | [仕様 05](spec/05-world-streaming.md) | 材質は名前で保存するので、順序が変わっても読める |
| 定義のハッシュ (`Definitions::content_hash`) | 内容から計算 | 同期の相手と違えば接続しない |

## 5. 公開 (crates.io)

まだ公開していない (`publish = false`)。公開する前に決めること: 名前の確保、ライセンスの表記、`charon-app` の依存 (wgpu・winit・egui) をどう見せるか、`charon-core` だけを先に出すか。決めるのは、この文書ではなく、リポジトリの持ち主。
