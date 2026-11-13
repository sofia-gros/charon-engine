# 仕様 15: 画像と音のアセット

`charon-app::assets`。ゲームが PNG・WAV・OGG を置き場から読む仕組み。

## 1. 置き場 `Assets`

- `add_root(dir)` で置き場を足す。**後に足したものが先に探される** (ゲームの MOD がエンジンの既定を上書きできる)。
- 名前は `dir/file.png` の相対パス。空・`..`・絶対パス・ドライブ名・`\` を含む名前は `AssetError::File` で断る (置き場の外へ出さない)。
- 1 ファイルは `MAX_FILE_BYTES` (32 MiB) まで。読んだ結果は名前で覚え、`clear_cache()` で忘れる。

## 2. 画像

- `decode_png(bytes)`: グレー・グレー+アルファ・RGB・RGBA・パレット・16 ビットを RGBA8 にする。一辺が 0 または `MAX_IMAGE_SIDE` (4096) を超える画像は、画素を作る前に断る。
- `sprite_from_png(bytes, &SpriteSpec)` / `Assets::sprite(name, &spec)`: `SpriteAsset` にする。`SpriteSpec { frame_size, pivot, scale, filter, clips }`。コマの大きさが 0 または画像より大きい、`scale` が正の有限でない場合は断る。基準点の既定は足元の中央。

## 3. 音

- `decode_sound(bytes)`: 先頭が `OggS` なら OGG Vorbis (`lewton`)、そうでなければ WAV (`charon-audio`)。ステレオは平均してモノラルにする。`MAX_SOUND_SAMPLES` (約 4 分) を超えると断る。
- `SoundBank::from_assets(&mut assets, rate)`: `sounds/{jump,shoot,anchor,pickup,dig}.wav|ogg` を読み、ないものは生成した音、壊れたものは報告して生成した音にする。`AppOptions::sounds` に渡す。
- デモは MOD の `assets/sounds/` を置き場にする。

## 4. 未実装

- フォント (`egui` へ TTF を渡す)、アトラス化、非同期の読み込み、Web での読み込み。
- OGG の読み込みは、壊れたファイルを断ることだけテストしている (読める OGG のテストデータがまだない)。

| テスト | 確認 |
|---|---|
| `png_color_types_become_rgba` | 4 種類の色形式が RGBA になる |
| `bad_and_huge_images_are_rejected` | 壊れた・空・大きすぎる・切れた PNG を断る |
| `sprites_take_frame_size_pivot_and_scale` | コマ・基準点・scale、不正な指定を断る |
| `sounds_are_picked_by_their_header` | WAV を読む、壊れた OGG・WAV・空を断る |
| `names_cannot_escape_the_roots` | 置き場の外へ出る名前を断る |
| `later_roots_override_earlier_ones_and_results_are_cached` | 上書きの順序とキャッシュ |
| `sound_bank_falls_back_to_generated_sounds` | 壊れた・ないファイルは生成した音 |
