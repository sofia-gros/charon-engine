# Charon Engine

Rust製の、汎用2Dサンドボックスゲームエンジンです。

Fabric APIのような機能へのアクセスと拡張性、Phaserのようなシーン・入力・資産・実行管理を、2Dサンドボックスのシミュレーションと統合します。クリエーターがRustでゲームのルール、素材、キャラクター、操作、進行を定義し、エンジンが制作と実行の基盤を提供します。

**現在はv1.0.0へ向けて開発中です。** E05「セルと流体」はQ01〜Q04のローカル実装・総合検査・ローカルレビューを完了し、PR #22の現head CI 10件も成功しました。PRはopenのままで、マージしていません。商用v1.0.0の完成や、Terraria・Noitaの完全クローン制作能力を実証済みという意味ではありません。

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
| E02：RuntimeとScene | 起動、plugin、Scene遷移、scope、入力、固定更新、System順序・明示独立worker、時計、headless | [実装・検証記録](new_docs/evidence/E02/README.md) |
| E03：資産と基本表示 | VFSとpack、PNG・WAV/Ogg・fontのimport、共有cache、解放・世代差し替え、sprite、camera、描画adapter | [実装・検証記録](new_docs/evidence/E03/README.md) |
| E04：世界と保存基盤 | 多layer・素材寸法・個体・部分生成・lease・独自channel・区域退避・世代保存 | [実装・検証記録](new_docs/evidence/E04/README.md) |
| E05：セルと流体（ローカル完了、PR open・未マージ） | 物質状態の一括輸送、局所移動、cell変更の理由付きreceipt、混合・分離、圧力・押出し、正規物質粒子、全表現監査、保存可能Job、保守的なsleep/wake、Runtime barrier、原子的Materials phase。Q04の全体テスト、E05総合検査、ローカル総合レビューが完了。2件のP1と2件のP2を修正し、最新head `87edd16`のGitHub CI 10件が成功 | [契約と残る制約](new_docs/30_MATERIAL_TRANSPORT.md)・[検証記録](new_docs/evidence/E05/README.md) |
| E06：熱と反応（ローカル完了、PR未提出） | 整数enthalpy曲線、断熱4近傍熱移動、cell/particle反応出力、Deferred/SameTick policy、保存可能なWorldEdit取引を実装。全target・check・clippy・docとローカル総合レビューが完了。WorldEdit込み性能は未計測 | [Q05設計](new_docs/45_HEAT_AND_REACTION_Q05.md)・[検証・レビュー証拠](new_docs/evidence/E06/README.md) |

### E05の現在地（2026-10-19）

E05のQ01〜Q04実装をまとめ、Q04の物質収支、粉体・液体・気体、圧力押出し、混合、sidecar追従、保存・復元、Runtime統合、参照経路とのtick比較を検証しています。E06/Q05の熱・反応は含みません。公開API、契約、残る制約は[new_docs/README.md](new_docs/README.md)と[E05検証記録](new_docs/evidence/E05/README.md)を参照してください。

Q04関連12 suiteの横断実行は296件成功しました。Arenaの候補複製で論理上限分の大きな予約を繰り返す問題を修正し、PressureJob・押出し探索・MaterialMotionJob・複数Provider・粒子を含む限定Runtime profileで、test profileの累積割当要求を約1.999GB/tickから約5.2〜6.0MB/tickへ減らしました。単一chunkのprofile測定であり、全World規模、動的multi-chunk、描画性能、144 FPS/60 Hz達成を示しません。

最終ローカルゲートでは`cargo test --locked --jobs 1`、`cargo check --all-targets --locked --jobs 1`、`cargo clippy --all-targets --locked --jobs 1 -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check`が成功しました。Cargoビルドは単一job・低メモリ設定です。保存可能な`MaterialParticleJob`とcell/scalar solverを同tickに共有予約へ合成するAPIは未提供で、[粒子Job契約](new_docs/35_MATERIAL_PARTICLES.md)に現状の制約を記録しています。

ローカルレビューで、保存済みPressurePushSearchの粒子索引がWorldと一致することを公開境界で検証しない欠陥を確認・修正しました。修正前の不正公開を再現する回帰と、修正後の単体6件・`pressure_job` 28件・対象Clippyが成功しています。E05/Q01〜Q04のローカル出口判定は完了しました。PR #22はopen・未マージで、レビュー依頼の有無は工程完了条件にしません。
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
cargo +1.98.1 run --locked --example material_accounting
cargo +1.98.1 run --locked --example material_forces
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
| [material_accounting.rs](examples/material_accounting.rs) | 混合・粒子・区域退避を含めた全表現の収支を理由付き台帳と照合し、世代保存と再開を検査 |
| [material_forces.rs](examples/material_forces.rs) | 素材ごとの重力Q16・DDAを2tick実行し、連続/途中保存再開の一致を確認 |

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
