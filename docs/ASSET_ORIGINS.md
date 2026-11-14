# 同梱候補アセットの出所記録

2026-11-15 時点の `assets/` 全ファイル。Windows zip は `assets/` を再帰的に同梱し、Android APK はデモ MOD・文字列・フォント等をバイナリへ埋め込む。記録の存在は再配布の許可や最終的な権利確認を意味しない。ゲーム用の自作データについては、コードの MIT とは別に素材の配布条件を決める必要がある。[配布前の未検証事項](RELEASE_CHECK.md)を参照。

| 相対パス | 出所 | 権利の根拠・状態 |
|---|---|---|
| assets/digs.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/enemies.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/fixtures.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/fonts/noto-sans-jp/NotoSansJP.ttf | Google Fonts `ofl/notosansjp`、原本 https://github.com/google/fonts/blob/main/ofl/notosansjp/NotoSansJP%5Bwght%5D.ttf | SIL OFL 1.1。著作権表示と原文は `assets/fonts/noto-sans-jp/OFL.txt` |
| assets/fonts/noto-sans-jp/OFL.txt | Google Fonts `ofl/notosansjp`、原本 https://github.com/google/fonts/blob/main/ofl/notosansjp/OFL.txt | SIL OFL 1.1 の原文。フォントと共に保持 |
| assets/hooks.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/items.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/materials.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/mods/fse_demo/manifest.ron | このプロジェクトで作成したデモ MOD | 自作データ。素材の配布条件は未決 |
| assets/mods/fse_demo/materials.ron | このプロジェクトで作成したデモ MOD | 自作データ。素材の配布条件は未決 |
| assets/mods/fse_demo/reactions.ron | このプロジェクトで作成したデモ MOD | 自作データ。素材の配布条件は未決 |
| assets/mods/fse_demo/script.rhai | このプロジェクトで作成したデモ MOD | 自作スクリプト。素材の配布条件は未決 |
| assets/mods/fse_demo/spells.ron | このプロジェクトで作成したデモ MOD | 自作データ。素材の配布条件は未決 |
| assets/mods/fse_demo/wands.ron | このプロジェクトで作成したデモ MOD | 自作データ。素材の配布条件は未決 |
| assets/reactions.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/recipes.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/spells.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |
| assets/strings.ron | このプロジェクトで作成 | 自作翻訳。素材の配布条件は未決 |
| assets/wands.ron | このプロジェクトで作成 | 自作定義。素材の配布条件は未決 |

幾何アイコンと短い仮曲・効果音は実行時にコードで生成し、外部画像・音源ファイルを置いていない。フォント以外の外部ファイルを追加した場合は、原本と許諾文を確認してこの表に一行追加する。Rust 依存クレート・バイナリ内の同梱物はこの表の対象外であり、`Cargo.lock` と [配布前の検証記録](RELEASE_CHECK.md)を別に確認する。
