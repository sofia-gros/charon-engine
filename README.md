# Charon Engine

Rust製の、汎用2Dサンドボックスゲームエンジンです。

Fabric APIのような機能へのアクセスと拡張性、Phaserのようなシーン・入力・資産・実行管理を、2Dサンドボックスのシミュレーションと統合します。クリエーターがRustでゲームのルール、素材、キャラクター、操作、進行を定義し、エンジンが制作と実行の基盤を提供します。

**現在はv1.0.0へ向けて開発中です。** E04「世界と保存基盤」までの新基盤を実装・検証しています。商用v1.0.0の完成や、Terraria・Noitaの完全クローン制作能力を実証済みという意味ではありません。

## 目指すもの

- ブロックとセル、粉体・液体・気体、熱と反応、破壊可能な回転物体を組み合わせられる世界
- シーン、入力、資産、描画、音響、UIを統合したゲーム制作API
- AI・経路探索、空間検索、画面外の活動制御、保存・リプレイ・通信
- 標準処理では表せない挙動も、独自データと公開API、置換可能な処理から構築できる拡張性

これらは**v1.0.0の設計目標**です。特定ゲームのルールをエンジンへ固定せず、TerrariaとNoitaの完全クローンを公開APIで作れるかどうかを受け入れ基準の一つとします。詳細は[製品設計](new_docs/README.md)と[完全クローンの受け入れ基準](new_docs/26_FULL_CLONE_ACCEPTANCE.md)を参照してください。

## 現在の実装と検証

| 工程 | 実装した範囲 | 根拠 |
| --- | --- | --- |
| E01：Coreとデータ | 型付きID、schema、component、独自状態、格子channel、codec、乱数、取引 | [実装・検証記録](new_docs/evidence/E01/README.md) |
| E02：RuntimeとScene | 起動、plugin、Scene遷移、scope、入力、固定更新、System順序、時計、headless | [実装・検証記録](new_docs/evidence/E02/README.md) |
| E03：資産と基本表示 | VFSとpack、PNG・WAV/Ogg・fontのimport、共有cache、解放・世代差し替え、sprite、camera、描画adapter | [実装・検証記録](new_docs/evidence/E03/README.md) |
| E04：世界と保存基盤 | 多layer・素材寸法・個体・部分生成・lease・独自channel・区域退避・世代保存 | [実装・検証記録](new_docs/evidence/E04/README.md) |
| E05：セルと流体（実装中） | 物質状態の一括輸送、局所移動、速度・端数と格子経路検査、cell変更の理由付きreceipt、混合・分離、圧力緩和、気体sourceと広域圧力job、保存可能な押出し探索、正規物質粒子の所有権移動・DDA・着地・区域退避。工程全体は未完了 | [契約と残作業](new_docs/30_MATERIAL_TRANSPORT.md)・[検証記録](new_docs/evidence/E05/README.md) |

E03の初回実装は全168件のテスト、整形、全ターゲットのコンパイル、警告拒否Clippy、Rustdocを通過しました。同じ資産packの読込とCPU参照描画はWindows・Ubuntu・macOSで、実ウィンドウのGPU表示はWindowsのVulkan / GeForce RTX 4060で確認しています。検証対象のコミットとCIへのリンクはE03の記録にあります。

E2・E3とE4のPR指摘対応を取り込んだE04は、旧変更を含まないツリーの全224件のテスト、整形、全target check、警告拒否Clippy、Rustdoc、保存復帰の作例を通過しています。

E05の輸送・圧力部分は[PR #4](https://github.com/sofia-gros/charon-engine/pull/4)のレビューで、空cellの出口圧、channel確保前の容量検査と圧力出力の容量再利用、探索frontierのstepごとの全複製を修正しました。PR #4時点ではLinux・Rust 1.98.1で全252テスト、整形、全target check、警告拒否Clippy・Rustdocを通過しました。その後の押出し探索の保存・復元修正は、後述する最新の全322テストで検証しています。E05全体とQ04は未完了です。詳細は[E05の検証記録](new_docs/evidence/E05/README.md)を参照してください。

次のE05単位として[速度・端数と格子経路検査](new_docs/31_MATERIAL_MOTION.md)を追加し、Linux・Rust 1.98.1で全264テストと品質チェック、headless作例を検証しました。一般形状の衝突、力の積分とprovider統合、混合・物質粒子などは残作業です。E05全体・Q04は引き続き未完了です。

cellの発行・消費・変換と[理由付きreceipt](new_docs/32_MATERIAL_RECEIPTS.md)を同時に保存する経路も追加し、全279テストと品質チェック、保存復元の作例をLinuxで検証しました。この経路は全編集の監視や全表現の総量台帳ではなく、粒子・剛体・退避領域などとの収支統合は残っています。

[気体sourceと広域圧力job](new_docs/33_WIDE_PRESSURE.md)も追加しました。温度・連結空間・素材別の係数とlambda・圧力上限・ポンプ境界を明示し、予算付きの計算後に全圧力を一括公開します。毎phaseの状態復元でも進捗を維持し、Linuxで全301テストと品質検査を通過しました。混合のprovider統合・粒子・全体収支・dirty/sleep・物性とprovider統合は残り、E05全体・Q04は未完了です。

[混合・分離](new_docs/34_MATERIAL_MIXTURES.md)では、整数比率の共有組成と有理数の成分量を使い、cell・全payload・組成参照と理由付きreceiptを同時に保存します。汎用書き戻しによる共有storeとjournalの履歴変更を拒否し、参照indexの再利用や分岐Worldからの履歴持込みを防ぎます。Linuxで全320テストと品質検査、保存再開の作例を通過しました。全表現の収支、物質粒子、dirty/sleep、provider統合は引き続きE05の残作業です。

**押出し探索修正時点の検証結果は全322テスト成功（失敗・無視なし）です。** [PR #9](https://github.com/sofia-gros/charon-engine/pull/9)の押出し探索修正（`5ba1c92c14757cee67e42dc5ae5145ebde31ca72`）では、毎phaseの保存・復元でも照合進捗を保持し、旧保存形式の読込みにも対応します。公開APIの回帰試験で有限回の完成と、連続実行・World保存再開との結果一致を確認しました。Linux / Rust 1.98.1 / `CARGO_BUILD_JOBS=2`で、`cargo fmt --all -- --check`、`cargo check --all-targets --locked`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked`、`RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`と`pressure_job`作例が成功しています。これは上記252〜320件の各段階を含む押出し修正の結果で、ローカルWindowsでの再実行結果ではありません。根拠は[E05の押出し探索保存・復元の検証記録](new_docs/evidence/E05/README.md#押出し探索の毎phase保存復元)を参照してください。E05全体とQ04は未完了です。

**最新のローカル検証は、正規物質粒子とレビュー修正を含む全354テスト成功（失敗・無視なし）です。** Windows/MSVC・Rust 1.98.1で、整形・全target check・警告拒否Clippy・Rustdocと粒子の保存再開作例を通過しました。[粒子API](new_docs/35_MATERIAL_PARTICLES.md)ではcellの全物質payloadを放出・DDA・着地まで保持し、所属regionと一緒に退避・復帰します。未処理粒子の開始位置も予約し、対向・追従・高速交差で重ならず、待機中もpayloadと速度を保持します。退避区域を復帰不能にする空気向け粒子remapは登録時に拒否します。Runtime用の保存上限は現在の確定状態を共通codecで計数し、lease・生成要求・Job・作者状態を含めます。状態変更後は再計算します。WorldはCHWORLD2、区域はCHREGN02を出力し、旧CHWORLD1・CHREGN01も粒子ゼロとして読めます。全表現の素材別収支、物性とproviderの統合、広域移動の継続Job、dirty/sleepとQ04は残作業です。詳細は[E05の粒子実装記録](new_docs/evidence/E05/README.md#正規物質粒子の実装単位)を参照してください。

E04では、素材別寸法・多layer・疎なchunk・個体・原子的編集・lease・生成baseと編集差分・独自channel・remap・Runtime・世代保存・区域の退避と復帰を接続しました。[公開APIと保存契約](new_docs/29_WORLD_AND_PERSISTENCE.md)、[検証記録](new_docs/evidence/E04/README.md)に、実行した検査と残る後工程を記載しています。新設計に沿った世界・マテリアル・物理・ゲーム共通サービス、完成した表現・音響、通信、制作ツールと商用認定は[工程表](new_docs/19_IMPLEMENTATION_ROADMAP.md)に従って実装・検証します。既存のシミュレーションやゲーム作例があることだけで、新工程を通過したとは判定しません。

## 動かす

CIではRust **1.98.1**を使用しています。開発ブランチはPRで検査し、mainへのpushと手動実行も検査します。同じ開発コミットでpushとPRの検査を重複させません。初回ビルドには依存crateを取得するネットワーク接続が必要です。ウィンドウ表示にはOSの表示環境と対応GPUが必要ですが、headlessの作例はウィンドウなしで動作します。

```sh
git clone https://github.com/sofia-gros/charon-engine.git
cd charon-engine
rustup toolchain install 1.98.1 --profile minimal --component rustfmt --component clippy
cargo +1.98.1 run --locked --example assets_scene
```

資産packから画像・音声・fontを読み込み、`target/e03-artifacts/gallery.png`へ画像を出力します。音声の再生と文字組みはE09の対象であり、この作例ではimportを確認します。

ウィンドウでスプライトとカメラを確認する場合：

```sh
cargo +1.98.1 run --locked --example assets_scene -- --window
```

その他の公開APIの作例：

```sh
cargo +1.98.1 run --locked --example foundation_data
cargo +1.98.1 run --locked --example runtime_scenes
cargo +1.98.1 run --locked --example sandbox_world
cargo +1.98.1 run --locked --example material_motion
cargo +1.98.1 run --locked --example material_receipts
cargo +1.98.1 run --locked --example pressure_job
cargo +1.98.1 run --locked --example material_mixtures
cargo +1.98.1 run --locked --example material_particles
```

| 作例 | 内容 |
| --- | --- |
| [foundation_data.rs](examples/foundation_data.rs) | 作者の状態・component・格子データの保存と取引 |
| [runtime_scenes.rs](examples/runtime_scenes.rs) | headlessで入力を注入し、Scene遷移と終了を実行 |
| [assets_scene.rs](examples/assets_scene.rs) | 共通packのimport、スプライト、複数カメラ、headless/GPU表示 |
| [sandbox_world.rs](examples/sandbox_world.rs) | 約43億cellの論理世界を部分生成し、家具・部分液量・遠隔lease・退避区域を保存復帰 |
| [material_motion.rs](examples/material_motion.rs) | 半cell/tickの端数蓄積、壁の直前での停止、速度・端数の保存復元 |
| [material_particles.rs](examples/material_particles.rs) | 全物質payloadを持つ粒子の放出、途中保存からの再開、壁の直前への着地 |
| [material_mixtures.rs](examples/material_mixtures.rs) | 異なる密度の素材の成分を丸めず混合・分離し、途中保存から再開 |
| [pressure_job.rs](examples/pressure_job.rs) | 気体の温度・連結容積から圧力を求め、分割計算の途中保存から再開 |
| [material_receipts.rs](examples/material_receipts.rs) | 作者の発行・変換・消費をreceiptへ記録し、地形と同時に保存復元 |

## ゲームの作り方

作者はSceneや独自SystemをRustの構造体とトレイトで実装し、型付きビルダーで生成時の設定を組み立て、実行中はメソッドとContextを通して操作します。Cargoが組み込む実装と依存を選び、YAMLがエンジンの実行設定を管理し、Rustコードがゲーム内容を定義します。

- [クリエーターの制作手順](new_docs/02_CREATOR_WORKFLOW.md)
- [APIの設計方針](new_docs/03_CREATOR_API_MODEL.md)
- [エンジン設定の責務](new_docs/05_ENGINE_CONFIGURATION.md)
- [実装済みの資産・基本描画API](new_docs/28_ASSET_AND_BASIC_RENDERING.md)

制作手順とv1のAPI設計には今後の実装も含まれます。現在使える具体的な経路は、各工程の検証記録とコンパイル対象の作例を参照してください。

## 開発に参加する

製品設計の入口は[new_docs/README.md](new_docs/README.md)、開発規約は[AGENTS.md](AGENTS.md)です。このREADMEも実装と検証の進捗に合わせて更新します。`codex/`と`gemini/`は履歴資料として保存しており、現在の開発仕様には使用しません。

リポジトリ直下で実行する品質チェック：

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo doc --lib --no-deps --locked
```

[全体Rust CI](.github/workflows/rust.yml)と[3 OSの資産・描画CI](.github/workflows/e03-assets.yml)、[3 OSの世界・保存CI](.github/workflows/e04-world.yml)を継続実行します。公開APIや保存形式の変更には、設計・利用例・テスト・互換性への影響を記録してください。

## ライセンス

エンジン本体は[MIT License](LICENSE)です。依存crateと同梱資産にはそれぞれのライセンスが適用されます。[E03の検査用資産](tests/fixtures/e03/README.md)には、生成した画像・音声と、OFLのfontの出典・使用許諾を記録しています。

PR #1のRuntime指摘も反映し、非活動Systemのphase境界、World横断recordの競合検出、plugin設定拒否時の解放対象を修正しています。根拠は[E02の記録](new_docs/evidence/E02/README.md)を参照してください。

PR #2のポインター指摘に対応し、HiDPI・resize後も論理frame座標を使用します。余白やwindow外へ出たpointerは無効化し、他の入力状態は維持します。詳細は[E03の記録](new_docs/evidence/E03/README.md)を参照してください。

E4のPRレビューで、次tickの生成採用・保存世代の列挙容量・blockの形状変更remapを修正し、回帰検査を追加しています。

E4の再レビューでは、一時保存世代の回収とRuntimeの地形時計の所有権も修正しました。保存中断・再起動と、pause中を含む時計不一致の回帰検査を追加しています。
