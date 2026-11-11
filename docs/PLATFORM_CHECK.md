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

## M8 Web サンドボックス (T8.8)

2026-11-11: `charon-core` と `charon-sandbox --lib` の非並列 wasm32 ビルド・Clippy、
`web_streaming_tick_is_deterministic`、保存形式のネイティブテスト `web_save_roundtrip`、
`scripts/check.sh` は通過した。`cargo build -p charon-sandbox --lib --release
--target wasm32-unknown-unknown --no-default-features` も通過した。
これらはブラウザー上の実行・永続保存の成功を示さない。

| 確認対象 | 状態 | 未実施の操作 |
|---|---|---|
| WebGPU | 未検証 | ブラウザーで起動・描画、マウスとタッチでセルを配置、ポーズ・再開、保存・再読込 |
| WebGL2 | 未検証 | WebGPU を利用できない環境で同じ操作と復帰後の描画を確認 |
| `web_save_roundtrip`（ブラウザー） | 未検証 | 保存した材質名・前景・背景・剛体を再読込後に照合 |
| `web_input_and_resume`（ブラウザー） | 未検証 | ポインター操作と中断・復帰後の描画を確認 |
| Android 実機・エミュレーター | 依頼者の指示で省略 | Web の検証から Android 動作を推定しない |

`wasm-bindgen-cli` のインストールはツールの安全判定で拒否された。
`scripts/build-web.sh` は wasm の release ビルド後、6 行目で `wasm-bindgen: command not found`
（終了コード 127）となり、JavaScript glue は生成されていない。
2026-11-12: 依頼者は `wasm-bindgen-cli` のインストールを許可しなかったため、
別経路で導入しない。保存状態の UI 表示を修正し、`scripts/check.sh` と wasm release ビルドを再実行して通過。
JavaScript glue がないためサンドボックス本体の確認と撮影は未実施。
ブラウザーで HTML ホストだけを開くと `pkg/charon_sandbox.js` が 404 となることを確認した。
ホスト側は読み込み失敗を画面の状態欄に表示するよう修正し、ブラウザーで表示を確認した。
これは `web_input_and_resume` や保存の実機検証を代替しない。

2026-11-12: 依頼者は Windows 版の動作を当面の確認対象とし、その他の実機検証を後日に延期した。
Windows の `charon-sandbox.exe` をビルドし、既存セーブを変更しない別の作業ディレクトリで起動。
プロセスが生存し、`Charon — live world` というウィンドウタイトルが存在することを確認した。
操作ツールによるアプリ別の画面操作許可が得られなかったため、
**画面の目視、マウス配置、最小化・復帰、スクリーンショットは未検証**。
Windows の起動確認を WebGPU / WebGL2 の成功として扱わない。

2026-11-12: 依頼者の指示で画面操作なしの確認へ切り替えた。
`headless_pause_and_resume` は実際の描画ループと同じ固定 tick の停止・1 tick 送り・再開規則を、
`headless_windows_pointer_save_reload` はポインター配置・背景への建築・剛体スタンプを含む
Windows の保存処理とファイル再読込を、各 seed 1〜8 で検査する。
これらは入力と保存のロジックを確認するが、ウィンドウの描画・実際のマウス/タッチイベント・
OS の最小化と復帰を確認したことにはならない。
