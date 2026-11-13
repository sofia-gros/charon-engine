# Android ビルドと実機確認 (T4.8)

対象: API 26 以上の arm64-v8a 端末、または arm64 Android エミュレーター。
`apps/sandbox` はデスクトップ用 `[[bin]]` と Android NativeActivity 用 `cdylib` を持つ。
両方とも同じ初期ワールドと `charon_app::run` を使う。

## ツールの選択

`cargo-apk` を選ぶ。`xbuild` のリポジトリは [unmaintained と明記](https://github.com/rust-mobile/xbuild)されている。
[crates.io の cargo-apk 0.10.0](https://crates.io/crates/cargo-apk) の説明には xbuild を勧める古い注記が残るが、[cargo-apk のリポジトリ](https://github.com/rust-mobile/cargo-apk) は Rust 1.99 対応などを更新している。
そのためインストールはリポジトリの現行版から行う。リリース版と混同しないこと。

## 必要なもの

- Android SDK (platform-tools、Android 35 の platform、build-tools)
- Android NDK 27.2.12479018 と JDK 17 以上
- Rust ターゲット `aarch64-linux-android`
- Android 端末の USB デバッグ、または起動済みの arm64 エミュレーター

Android SDK をインストールし、`ANDROID_HOME` (または `ANDROID_SDK_ROOT`)、
`ANDROID_NDK_HOME` (SDK 内の `ndk/27.2.12479018`)、`JAVA_HOME` を設定する。
SDK のライセンスを承認した上で、次を実行する。

```bash
sdkmanager --install 'platform-tools' 'platforms;android-35' 'build-tools;35.0.0' 'ndk;27.2.12479018'
rustup target add aarch64-linux-android
cargo install --git https://github.com/rust-mobile/cargo-apk cargo-apk --locked
```

Windows では SDK の `platform-tools` を `PATH` に加える。環境変数はインストール先に合わせる。
Android 用のコードだけを型検査するなら、NDK リンカーなしで次を実行できる。

```bash
cargo check -p charon-render -p charon-app -p charon-sandbox --lib --target aarch64-linux-android
```

APK を生成・端末に入れて起動する:

```bash
cargo apk build -p charon-sandbox
adb devices
cargo apk run -p charon-sandbox
```

`apps/sandbox/Cargo.toml` は `aarch64-linux-android`、minSdk 26 と NativeActivity 用 `cdylib` を指定する。
デスクトップの `main.rs` は Android の APK に含めない。

## 参照デモの APK とリリース (M13)

画面の向きは**横画面に固定** (`apps/sandbox/Cargo.toml` の `orientation = "sensorLandscape"`: センサーで左右どちらの横向きにもなり、縦にはならない)。仮想スティックとボタンは横画面の配置 ([仕様 10 §11](spec/10-noita-like-game.md))。

APK は参照デモ (`charon-demo`) を動かす。`android_main` がデモの MOD (`assets/mods/fse_demo`、APK に埋め込み) を内部ストレージへ書き出して `charon_app::demo::run_demo_with` を呼ぶ。操作はタッチの仮想スティックとゲームパッドのボタン ([仕様 10 §11](spec/10-noita-like-game.md))。

リリースビルドの署名は、`apps/sandbox/Cargo.toml` の `[package.metadata.android.signing.release]` が `apps/sandbox/debug.keystore` (パスワード `android`、コミットしない) を指す。手元で `cargo apk build --release` するには、先に鍵を作る (配布用の本番の鍵ではない):

```bash
keytool -genkeypair -v -keystore apps/sandbox/debug.keystore -storepass android -alias androiddebugkey -keypass android -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US"
```

リリース用の APK は `cargo apk build --release -p charon-sandbox --lib` で作る (`--lib` がないと、デスクトップの bin と cdylib が混ざって cargo-apk が失敗する)。`v*` のタグを push すると、`.github/workflows/release.yml` が Windows 版 (`charon-demo-<タグ>-windows-x64.zip`、exe と `assets`) と Android 版 (`charon-demo-<タグ>-android-arm64.apk`) をビルドして GitHub Releases に載せる。

```bash
git tag v0.0.1
git push origin v0.0.1
```

## 実機またはエミュレーターの確認

1. 画面に砂・水と材質一覧が出ることを確認する。
2. タッチで砂を置き、消しゴムで消す。2 本指でカメラの移動・ズームを確認する。
3. ホーム画面へ移り、サンドボックスへ戻る。クラッシュせず世界を描画し続けることを確認する。
4. `adb logcat` でクラッシュを確認し、画面のスクリーンショットを記録する。

`charon-app` は `resumed` でウィンドウとサーフェスを作り、`suspended` でサーフェスを破棄し、
復帰時に新しいウィンドウからサーフェスを再生成して固定 tick の累積時間をリセットする。

## この環境での結果 (2026-11-11)

Windows 上で Android Rust ターゲットを追加し、上記 3 クレートとサンドボックスの Android ライブラリの
`cargo check` が通った。Android SDK・NDK・`adb`・`cargo-apk`、接続端末・エミュレーターは確認できず、
APK の生成と端末でのタッチ・ホーム復帰試験は**未実施**。2026-11-11 に依頼者が今後の Android 実機検証を省略し、T4.8 を完了扱いにして次のタスクへ進むよう指定した。未実施の試験を成功とみなさない。
