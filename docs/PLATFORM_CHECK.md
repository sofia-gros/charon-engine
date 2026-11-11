# M4 プラットフォーム確認 (T4.9)

記録日: 2026-11-11。各プラットフォームの実測と未実施を区別する。

| 環境 | 実機 GUI | 砂と水の配置 | 最小化・復帰 | ビルド / 型検査 | 備考 |
|---|---|---|---|---|---|
| Windows 11 Pro | 確認済み | 確認済み | 確認済み | `scripts/check.sh` 通過 | `charon-sandbox` を起動し、マウスで水と砂を置いてから最小化・復帰。画面は [T4.9-windows.png](screenshots/T4.9-windows.png) |
| Linux | 未実施 | 未実施 | 未実施 | 未実施 | このセッションに Linux 実機なし |
| macOS | 未実施 | 未実施 | 未実施 | 未実施 | このセッションに macOS 実機なし |
| Android (arm64-v8a) | **依頼者の指示で省略** | 省略 | 省略 | `charon-core`・`charon-render`・`charon-app`・`charon-sandbox` の `cargo check --target aarch64-linux-android` 通過 | 2026-11-11 以降、Android 実機・エミュレーター検証は行わずロードマップを進める。APK 生成・起動も未実施 |
| Web | 対象外 | 対象外 | 対象外 | `charon-core --target wasm32-unknown-unknown --no-default-features` 通過 | Web サンドボックスは M8 以降 |

Windows の確認は実際のサンドボックス画面に対するマウス操作とウィンドウの最小化・復帰で行った。
画面では選択中の砂と、追加した砂（黄）・水（青）を確認できる。
一時停止状態で撮影したため、置いたセルがその場に残っている。Linux と macOS の動作は未検証であり、
Windows の結果から他 OS の動作を推定しない。

Android の実機検証省略は**テスト成功を意味しない**。手順と未実施項目は
[BUILD_ANDROID.md](BUILD_ANDROID.md) に記録する。

## M6 剛体操作 (T6.8)

2026-11-11: Windows で `charon-sandbox` の起動を試みたが、画面操作ツールのアクセス許可が
得られなかったため、ドラッグ・回転・中身の排出を GUI 上で操作して確認できず、画像も撮影していない。
`grabbed_body_moves_and_releases`、`deterministic`、`c_shape_pours_when_rotated` の自動テストは通過した。
これらは GUI の実機確認を代替しない。依頼者の 2026-11-11 の判断により、
Windows 実機操作・撮影は未検証として免除し、後続タスクへ進む。
Android 実機確認も依頼者の指示どおり省略（未検証）。
