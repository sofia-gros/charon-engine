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

## M9 Windows 実 GPU 描画の検証 (T9.5)

2026-11-12: `nvidia-smi` で NVIDIA GeForce RTX 4060 / ドライバー 617.42 を確認。
Windows 11 Pro 上で `CHARON_REQUIRED_GPU_ADAPTER=RTX 4060` を指定し、
`cargo test -p charon-render --lib light_pass_renders_offscreen -- --nocapture` を実行。
テストが取得した実際のアダプターは **NVIDIA GeForce RTX 4060 / DiscreteGpu / Vulkan / NVIDIA 617.42**。GPU が取得できない場合や、選択したアダプターが RTX 4060 の離散 GPU でない場合は、この指定でテストが失敗する (通常の GPU なし CI では従来どおりスキップ)。

`light_pass_renders_offscreen` は GPU 上で背景・セル・背景レイヤー・エンティティ・照明・合成・後処理を実行。光の品質 Off / Low / High の 3 通りを描画し、High の 256×256 出力を GPU から読み出して空と洞窟の明るさ、木と空の色差、エンティティと空の色差、砂セルと隣の空の色差を検査し、**1 件通過**した。High の読み出し結果を [`screenshots/T9.5-rtx4060-offscreen.png`](screenshots/T9.5-rtx4060-offscreen.png) に保存し、画像の内容を目視で確認。テストでは `CHARON_GPU_CAPTURE_PATH` を指定すると RGBA8 の GPU 読出し結果をそのパスに書き出す。記録画像はこのバイト列を PNG に変換したもの。

これは **実 GPU のオフスクリーン描画検証**であり、ウィンドウのサーフェスへの表示、実際の GUI 操作・最小化/復帰・ウィンドウのスクリーンショット、DX12 バックエンドや GPU フレーム時間を検証した結果ではない。CPU 側シミュレーションの 100 万セル / 8 ms 目標を GPU の存在だけで達成したことにもならない。

## M10 の追加: 滑らかな剛体 (T10.9)

2026-11-12: 剛体ごとにドットか滑らかかを選べるようにし、デモにドットの十字と並べて滑らかな十字を足した。RTX 4060 (Vulkan) のオフスクリーン描画 (`CHARON_REQUIRED_GPU_ADAPTER=RTX 4060`、`demo_renders_smooth_body`、`smooth_body_turns_below_cell_resolution`) で次を確認した: 滑らかな剛体のスタンプしたセルはセルの描画では描かれない (スプライトを描かないと、剛体のない画面と 12,544 画素中 12,522 画素が一致)。滑らかな十字は 7,158 画素を描き、縁の細かさ (4×4 画素の明るさの幅の平均) は滑らか 7.57 に対しドット 0.67。角度を 0.02 ラジアン変えただけで 441 画素が変わる。画像は [`screenshots/M10-smooth-cross.png`](screenshots/M10-smooth-cross.png) で、目視でも確認した。

**未検証:** ウィンドウで回り続ける様子、他の剛体・粒子・プレイヤーが滑らかな剛体に触れたときの見え方 (物理はドットと同じ)、大きな剛体 (一辺 510 セル超は描かない) と多数の滑らかな剛体の速さ。滑らかな剛体は合成のあとに重ねるので、プレイヤーなどのエンティティの手前に描かれる。

## M10 の追加: 背景のさらに奥で動き回る虹色 (T10.8)

2026-11-12: 前景の虹色の岩を取り除き、背景のさらに奥で動き回る虹色の塊を shader で描くようにした。RTX 4060 (Vulkan) のオフスクリーン描画 (`CHARON_REQUIRED_GPU_ADAPTER=RTX 4060`、`demo_renders_deep_background`、`deep_background_is_behind_cells`) で次を確認した: 空の画面に虹色の画素が 37,504 個、色相 10 種類。時間が進むと動き回る (240 tick で画素の約 5% が変わる)。カメラが 40 セル動くと塊は 20 セル動く (ずらして比べた一致率 100%)。前景のセルの内側は、塊の有無で画素が完全に同じ。画像は [`screenshots/M10-deep-rainbow.png`](screenshots/M10-deep-rainbow.png) で、目視でも確認した。

**未検証:** ウィンドウで動き回る様子 (動画としての見え方と速さの感じ)、参照の動画との動きの速さの一致。参照の塊の正確な形・大きさ・速さは、フレームから推定した値で、一致を主張しない。

## M10 の追加: 十字の剛体の回転 (T10.7)

2026-11-12: デモの部屋の十字 (X 字) を、床から浮かせてその場で 3 ラジアン / 秒で回すようにした。RTX 4060 (Vulkan) のオフスクリーン描画で、150 tick 後の十字が傾いて連続した形で空中にあることを確認した ([`screenshots/M10-spinning-cross.png`](screenshots/M10-spinning-cross.png))。自動テスト (`demo_cross_spins`) で、150 tick から 149 tick のあいだに約 7.5 ラジアン回り、位置は動かないことを確認した。ウィンドウで回り続ける様子 (動画としての見え方) と、回る十字に他の剛体・粒子・プレイヤーが触れたときの挙動は未検証。保存済みの旧デモ世界 (`saves/demo`) の十字は、保存した位置のまま回り始める (床に重なって保存されていた場合は、新しい seed で確かめること)。

## M11 デモの杖と弾 (T11.6)

2026-11-12: `charon-demo` に体力・杖・弾を組み込み、右ボタン (押している間) でカーソルの方向へ撃つようにした。`CHARON_REQUIRED_GPU_ADAPTER=RTX 4060` で `cargo test -p charon-app --lib demo_renders_wand_panel` を実行し、選択されたアダプターが **NVIDIA GeForce RTX 4060 / DiscreteGpu / Vulkan** であることを確認した。部屋の場面に体力 65/100 のプレイヤーと飛んでいる弾 3 つを置いて 1600×900 でオフスクリーンに描き、画面上端の `Wand` パネル (赤い体力バー、青い息・マナのバー、`Next: spark · Shots: 3`) と弾の点が画素に現れることを検査して通過した。画像は [`screenshots/M11-wand-panel.png`](screenshots/M11-wand-panel.png) で、目視でも確認した。

**敵 (T11.4):** 同じ RTX 4060 のオフスクリーン描画で、デモの部屋に敵 2 体 (`walker`・`shooter`) が生成され、`Wand` パネルに `Enemies: 2` と出ることを確認した ([`screenshots/M11-enemies.png`](screenshots/M11-enemies.png))。敵はプレイヤーと同じ色の箱で描かれ、区別できる色はまだない。画像の時点では、視線が剛体にさえぎられて目標がいないため、敵は徘徊で部屋の外 (扉の右) へ出ている。ウィンドウで敵と戦う操作は未検証。

**ワイヤーフック (T11.8):** 同じ RTX 4060 のオフスクリーン描画で、デモの部屋の天井にワイヤーが着き (`Anchored`)、点列が画素に現れること (変わった画素 1,152)、`Wand` パネルに `Tool (right): grapple · anchored` と `Weapon (left) next: spark`、`Draw` パネルに `Paint with left button (B)` が出ることを確認した ([`screenshots/M11-hook.png`](screenshots/M11-hook.png)、目視でも確認)。ワイヤーはエンティティと同じ固定色の点で、見分けやすい色はまだない。ウィンドウでの左右ボタンの操作 (`B` での切り替え、UI が押下を使った場合) は、画面操作の許可がなく**未検証**。ワイヤーの物理は浮動小数点で、同一ビルド・同一プラットフォームの決定性だけを自動テストで確認した。

**自動テストで確認したこと:** 環境ダメージ・息、弾の飛行と着弾と爆発ダメージ、杖のマナ・詠唱間隔・再充填、`GameState` の tick の順序、`Lockstep` のパケット version 2 とゲーム状態を含む同期ハッシュ、デモの世界で右ボタン相当の入力が弾とパーティクルを生むこと、狙いの計算。

**未検証 (成功とは扱わない):** ウィンドウでの実際の右ボタン操作と、UI が押下を使ったときに撃たないこと (画面操作の許可が得られなかった)。`Lockstep` は同一プロセス内のテストだけで、実際のネットワーク接続は持たない。Web・Android・Linux でのデモは未検証。

## M10 参照デモ (T10.1〜T10.6)

2026-11-12: 依頼者が提供した参照 (FallingSandEngine の `demo.mp4`、61.6 秒) を取得し、4 秒ごとのフレームで内容を確認して仕様 09 の観察表を作った。

**実 GPU で確認したこと:** `CHARON_REQUIRED_GPU_ADAPTER=RTX 4060` を指定して `cargo test -p charon-app demo_renders_on_gpu` を実行し、実際のアダプターが **NVIDIA GeForce RTX 4060 / DiscreteGpu / Vulkan / NVIDIA 617.42** であることを確認した。部屋、虹色の山、黄・緑・赤のバイオームの 5 場面を 1600×900 でオフスクリーンに描き、暗い洞窟であること、光る材質が見えること、場面ごとの色 (看板の青、11 色以上の虹色の縞、黄・緑・赤の壁) を画素で検査して通過した。UI (メニュー、`Draw`、計測グラフなど) を重ねた画像を目視し、参照のフレームと見比べて配置と雰囲気が近いことを確認した。画像は `screenshots/M10-*.png`。

**自動テストで確認したこと:** 生成器の純粋性と生成順への非依存、部屋と出入口の通路、苔・塊・文字・背景ラベル、キー入力と追従カメラ、虹色ブラシ、クリップボード、計測履歴、egui の 1 フレームの描画、デモの世界・プレイヤー・剛体の決定性 (seed 1〜8)。

**未検証 (成功とは扱わない):**

| 項目 | 状態 |
|---|---|
| ウィンドウでの実際のマウス・キー操作、`F` での自由カメラ、メニューのクリック | 画面操作の許可が得られず未検証。`charon-demo.exe` を起動して 12 秒動かし、プロセスの生存・ウィンドウ名 `Charon — demo`・標準エラーの空を確認しただけ |
| `Main Menu` の `Singleplayer` / `Random Seed` による世界の作り直し | 実装済みだが、ウィンドウからの操作は未検証 |
| Web・Android・Linux | デモはデスクトップ専用。未検証 |
| 参照との差 | CPU・メモリのグラフなし、`Z` / `X` キー未対応、参照の絵素材と簡略化した地形の折れ線・構造物の外形線は再現していない (仕様 09 §1) |

参照のフレームはこのリポジトリに入れていない。