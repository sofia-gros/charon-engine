# Charon Engine

**Fabric APIのような機能の利用・拡張APIと、Phaserのようなシーン・入力・リソースの管理と統合を組み合わせた、OSSの2Dサンドボックスゲームエンジン。**

作者はRustでSceneとゲームコードを書き、エンジンが提供するAPIを使って画面、操作、世界、マテリアル、物理、効果、独自システムを組み立てます。Sceneは構造体とトレイト、生成時の設定は型付きのビルダー、実行中の操作はメソッド、独自機能はトレイトやイベント登録として表現します。

Cargoはビルドへ含める実装と依存を選び、YAMLはエンジンの実行設定を管理し、Rustコードはゲーム内容と独自機能を定義します。この役割から各設定の置き場所を判断します。ブロックサイズはマテリアルのビルダーで指定します。

## 文書の構成

| 文書 | 読むと分かること |
| --- | --- |
| [01_ENGINE_VISION.md](01_ENGINE_VISION.md) | 製品の目的、二つの参照モデルをどう組み合わせるか、作者とエンジンの役割 |
| [02_CREATOR_WORKFLOW.md](02_CREATOR_WORKFLOW.md) | Rustによる起動、Scene、素材、操作の利用例 |
| [03_CREATOR_API_MODEL.md](03_CREATOR_API_MODEL.md) | ビルダー、トレイト、型付きハンドル、イベント、独自機能の設計方針 |
| [04_RUNTIME_INTEGRATION.md](04_RUNTIME_INTEGRATION.md) | Scene、入力、リソース、ゲーム世界、独自システムを実行時にどうつなぐか |
| [05_ENGINE_CONFIGURATION.md](05_ENGINE_CONFIGURATION.md) | Cargo・YAML・Rustコードの役割、描画能力と実行設定、拡張機能の導入 |

## v1.0.0の実装仕様

| 文書 | 内容 |
| --- | --- |
| [06_V1_PRODUCT_REQUIREMENTS.md](06_V1_PRODUCT_REQUIREMENTS.md) | 商用制作向けの出荷範囲と32の必須要件 |
| [07_ARCHITECTURE_AND_EXECUTION.md](07_ARCHITECTURE_AND_EXECUTION.md) | 層、所有権、起動、tick順序、取引、並行処理、障害 |
| [08_DATA_MODEL_AND_FORMATS.md](08_DATA_MODEL_AND_FORMATS.md) | 座標、ID、セル、component、全データ分類、メモリ、形式と容量 |
| [09_WORLD_AND_STREAMING.md](09_WORLD_AND_STREAMING.md) | ブロックとcell、layer、生成、chunk状態、lease、sleep |
| [10_MATERIALS_AND_CELL_PHYSICS.md](10_MATERIALS_AND_CELL_PHYSICS.md) | 物性、粉体・液体・気体、圧力、熱、相変化、反応、物質粒子 |
| [11_RIGID_BODIES_AND_COUPLING.md](11_RIGID_BODIES_AND_COUPLING.md) | 回転・衝突・joint、破壊可能body、流体・熱連成と変換 |
| [12_GAMEPLAY_AND_EXTENSION_SERVICES.md](12_GAMEPLAY_AND_EXTENSION_SERVICES.md) | controller、inventory、craft、combat、効果、AI、配線、進行 |
| [13_PRESENTATION_INPUT_AND_ASSETS.md](13_PRESENTATION_INPUT_AND_ASSETS.md) | 資産pipeline、描画、light、入力、UI、文字、音 |
| [14_PERSISTENCE_AND_REPLAY.md](14_PERSISTENCE_AND_REPLAY.md) | 保存形式、世代公開、load、移行、replay、作者状態 |
| [15_NETWORKING.md](15_NETWORKING.md) | server権威、複製、interest、予測、再接続と検証 |
| [16_PUBLIC_API_CONTRACTS.md](16_PUBLIC_API_CONTRACTS.md) | Rust APIの契約、Context、builder、trait、event、上級API |
| [17_TOOLING_AND_COMMERCIAL_DELIVERY.md](17_TOOLING_AND_COMMERCIAL_DELIVERY.md) | 制作CLI、inspector、配布、チーム制作、ライセンス、保守 |
| [18_VALIDATION_AND_RELEASE.md](18_VALIDATION_AND_RELEASE.md) | 試験群、性能目標、障害、長時間、商用出荷条件 |
| [19_IMPLEMENTATION_ROADMAP.md](19_IMPLEMENTATION_ROADMAP.md) | E00〜E14の依存順、出口、検証・review・commitの進め方 |
| [20_REFERENCE_GAMES_AND_TRACEABILITY.md](20_REFERENCE_GAMES_AND_TRACEABILITY.md) | 完全再現に向けた統合fixtureと、全要件の対応表 |
| [21_ENGINEERING_DECISIONS.md](21_ENGINEERING_DECISIONS.md) | 採用する設計判断、依存、リスクと変更の管理 |
| [22_REFERENCE_ALGORITHMS_AND_TRANSACTIONS.md](22_REFERENCE_ALGORITHMS_AND_TRANSACTIONS.md) | 取引、熱、圧力、移動、破壊、衝突、状態変換の基準 |
| [23_CONTENT_AND_STATE_SCHEMAS.md](23_CONTENT_AND_STATE_SCHEMAS.md) | 定義と個体record、field、型、codec、検証 |
| [24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md](24_AI_NAVIGATION_AND_SPATIAL_ACTIVITY.md) | 知覚とAI、探索、chunk/画面判定、画面外の読込・更新・個体方針 |
| [25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md) | P001：標準処理の置換、独自データ、衝突、同tick順序と保存への接続 |
| [26_FULL_CLONE_ACCEPTANCE.md](26_FULL_CLONE_ACCEPTANCE.md) | Terraria・Noitaの全内容・全挙動を再現する能力の照合と出荷判定 |
| [28_ASSET_AND_BASIC_RENDERING.md](28_ASSET_AND_BASIC_RENDERING.md) | 実装したE03の資産、pack、scope、差し替え、sprite、camera、描画adapter |
| [29_WORLD_AND_PERSISTENCE.md](29_WORLD_AND_PERSISTENCE.md) | 実装したE04の世界・生成・活動域・独自状態・退避・保存API |
| [30_MATERIAL_TRANSPORT.md](30_MATERIAL_TRANSPORT.md) | 実装中のE05：物質状態の輸送、局所移動、圧力と押出し探索 |
| [31_MATERIAL_MOTION.md](31_MATERIAL_MOTION.md) | 実装中のE05：速度・端数の保存と整数格子の複数cell経路検査 |
| [35_MATERIAL_PARTICLES.md](35_MATERIAL_PARTICLES.md) | 実装中のE05：全payloadの粒子への移行、DDAと着地、所属regionとの退避・復帰 |
| [36_MATERIAL_ACCOUNTING.md](36_MATERIAL_ACCOUNTING.md) | 実装中のE05：全表現の物質収支・理由付き増減と監査付き保存 |
| [37_MATERIAL_FORCES.md](37_MATERIAL_FORCES.md) | 実装中のE05：Q16物性・標準/独自Providerの力積分・速度DDA統合 |
| [38_MATERIAL_MOTION_JOB.md](38_MATERIAL_MOTION_JOB.md) | 実装中のE05：広域単一tokenの保存再開できるDDA経路検索と原子的公開 |
| [39_MATERIAL_SLEEP.md](39_MATERIAL_SLEEP.md) | 実装中のE05：標準力学の静止証明・wake・逐tick参照一致 |
| [40_MATERIAL_RUNTIME.md](40_MATERIAL_RUNTIME.md) | 実装中のE05：Runtime Materials barrierと全表現収支監査付き編集 |
| [41_SCALAR_MATERIAL_SLEEP.md](41_SCALAR_MATERIAL_SLEEP.md) | 実装中のE05：粉液気scalarの静止証明・Wake・逐tick参照一致 |
| [42_Q04_CROSS_REPRESENTATION.md](42_Q04_CROSS_REPRESENTATION.md) | 実装中のQ04：混合有理量・粒子・archive・Runtimeの原子的な連鎖検証 |
| [43_MATERIAL_PHASE_PIPELINE.md](43_MATERIAL_PHASE_PIPELINE.md) | 実装中のE05：圧力・Q16力学・scalar・正規粒子の単一Materials原子取引 |
| [44_MATERIAL_LEGACY_DIAGNOSTICS.md](44_MATERIAL_LEGACY_DIAGNOSTICS.md) | 実装中のE05：PR #11〜#13の出所別診断APIと一様力Provider互換 |
| [45_HEAT_AND_REACTION_Q05.md](45_HEAT_AND_REACTION_Q05.md) | E06/Q05：整数enthalpy、material追従energy、位置固定辺端数、保存可能な反応transaction |
| [34_MATERIAL_MIXTURES.md](34_MATERIAL_MIXTURES.md) | 実装中のE05：整数比率の共有組成、正確な成分量、混合・分離とreceiptの取引 |
| [33_WIDE_PRESSURE.md](33_WIDE_PRESSURE.md) | 実装中のE05：気体の温度・連結容積と保存可能な広域圧力job |
| [32_MATERIAL_RECEIPTS.md](32_MATERIAL_RECEIPTS.md) | 実装中のE05：cell変更と理由付きreceiptの原子的保存 |
| [27_FULL_CLONE_DESIGN_REVIEW.md](27_FULL_CLONE_DESIGN_REVIEW.md) | 完全クローン基準のレビュー、18件の不足と修正、設計判断の根拠 |

## この文書群の位置付け

ユーザーの明示的な要求を基準にした、v1.0.0へ向けた製品設計です。概要、利用方法、内部構造、データ、アルゴリズム、API、検証、配布、工程を定義します。2026-10-15時点で、ここに記載する機能やコード例がすべて実装済みという意味ではありません。

エンジンの整合性・性能・保守性を優先し、その設計の上で人間に分かりやすいAPIを提供します。性能や正しさのために必要な予約、batch、Job、所有権を明示し、理由と利用方法を説明します。表面上の簡単さのために設計を崩しません。

01〜05は製品理解と利用例、06〜29は実装・検証の基準です。APIの正確な意味は16/25、共通データは08/23、標準更新順は07、標準solverの補足は22、完全再現の判定は26を参照します。P001により標準solverや次tick繰越を作者へ強制する解釈を撤回し、25の拡張契約を優先します。食い違いが見つかったら要件へ立ち戻って関係文書を同時に修正し、都合のよい説明だけを採用しません。

## 開発の基準

- ユーザーの明示的な指示を最優先とし、未決定事項をAIの判断だけで製品の制限へ変えません。
- 今後の開発では`new_docs/`を参照します。`codex/`、`gemini/`の文書は履歴として保持し、ユーザーが過去との比較を求めた場合だけ参照します。[ルートREADME](../README.md)はGitHub向けの案内として継続更新し、実装・使い方・工程の進捗をこの文書群と検証記録に合わせます。製品設計の正本は`new_docs/`です。
- 既存コードは現在の実装を示す資料です。既存の実装や旧工程の判定を理由に、目標を縮めたり、新しい要求を達成済みと扱ったりしません。
- 要求、採用する設計、未決定事項、検証済み実装を識別します。実装済みという表示には、対象のコードと検証の根拠が必要です。
- 機能や設計の変更はP（Plus）として管理し、採用されたPが以前の仕様や実装と重なる場合はPを優先します。元の目的の復元と、新機能の追加は区別します。
- コードのドキュメントとコメント、ユーザーへの説明は日本語で記述します。原則としてプロトタイプで要求の実装を済ませず、必要な場合は事前に通知し、未実装範囲を明示します。

## 完成の扱い

実装の着手・検証範囲は[E01](evidence/E01/README.md)、[E02](evidence/E02/README.md)、[E03](evidence/E03/README.md)へ記録しています。設計上の機能一覧と現在の実装を区別して参照してください。

すべての要件と二つの検証作品、配布物、性能、障害試験に根拠が揃うまでv1.0.0の完成とはしません。設計文書の完成と、エンジン実装の完成を別に記録します。工程は全て新しい要求に対して判定します。

TerrariaとNoitaは完全クローンの制作可能性を判定する対象です。同系統のゲームや主要機能の作例へ置き換えません。作者は全ゲーム内容をRustとデータで定義でき、標準機能で一致しない挙動は公開provider/channel/scheduleで実装できます。レビューの結論と、原作との一致を実証した結果は別に記録します。

## 現在の実装記録

- [E01：Coreとデータ](evidence/E01/README.md)
- [E02：RuntimeとScene](evidence/E02/README.md)
- [E03：資産と基本表示](evidence/E03/README.md)

実装済みの範囲と検証は各記録を参照してください。v1の設計・完全クローンの要件は、これらの基盤工程だけで達成したと扱いません。

- [E04：世界と保存基盤](evidence/E04/README.md)
- [E05：セルと流体（PR #4〜#10、#22 mainへマージ済み）](evidence/E05/README.md)
- [E06：熱と反応（PR #23 mainへマージ済み）](evidence/E06/README.md)
- [E07：物体と破壊連成（Q06実装中）](evidence/E07/README.md)
