# Charon Engine — Codex guidance

## Source of truth

- Start with [`new_docs/README.md`](new_docs/README.md). The user's explicit requirements and the documents under `new_docs/` are the product design authority.
- The previous documentation under `codex/` and `gemini/` is retired and retained only as historical files. Do not read it to guide development or import its requirements, restrictions, milestones, or completion claims. Consult it only when the user explicitly requests a historical comparison. The root `README.md` is the maintained GitHub-facing project introduction; it summarizes `new_docs/` and verified implementation without becoming a separate design authority.
- Treat `src/`, `Cargo.toml`, and executed checks as evidence of current implementation, never as limits on the user's intended product.
- Read only the new design pages relevant to the task. Distinguish user requirements, proposed designs, unresolved decisions, and verified implementation. Do not silently present a planned feature as implemented.
- Charon Engine is an open-source, general-purpose 2D sandbox game engine. Creators own game rules, assets, entities, and progression. Specific games demonstrate the engine's capabilities and must not define mandatory gameplay in the engine core.
- The product combines Fabric API-style feature access and extensibility with Phaser-style scene, input, resource, and runtime management, integrated with 2D sandbox simulation. Interpret those reference experiences as Rust APIs: structs and traits for Scenes and systems, typed builders for construction, ordinary methods for runtime changes, and typed handles and scoped callbacks for access. Do not mechanically translate the user's illustrative JavaScript, class inheritance, dynamic options objects, prototype methods, or tentative spellings into the specification. Apply engineering judgment and explain design decisions.
- Cargo selects compiled implementations and dependencies; YAML configures engine runtime behavior and services; Rust code defines game content and registers Scenes, plugins, and custom behavior. Use those responsibilities to classify settings, rather than treating examples as an exhaustive list. YAML cannot enable absent compiled code or unsupported hardware. Block dimensions belong to material builders; player composition and controls belong to creator code and object APIs.
- Prioritize engine correctness, performance, extensibility, and maintainability. Human-readable API design is the default within those constraints. Expose and explain necessary batching, reservations, jobs, ownership, and synchronization costs instead of compromising the engine to hide complexity.
- Target commercial v1.0.0 using the requirements, architecture, data contracts, acceptance games, and release gates under `new_docs/`. Both Terraria-style and Noita-style games must be possible through public APIs, including destructible rotating bodies, material interaction, AI/navigation, spatial queries, and configurable offscreen activity. Do not downgrade required mechanisms to visual demonstrations or claim implementation completion from documentation alone.
- When implementation reveals a conflict, identify the affected user requirement and update the owning documents under `new_docs/`. Never narrow the user's goal to match existing code or make a milestone easier to pass.
- Apply P001 in `new_docs/25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md`. Standard solvers, next-tick deferral, numeric formats, randomness, and networking policies must not limit creator-authored behavior. Provide public provider, channel, state, schedule, collision, persistence, rendering, and networking integration routes. Judge Terraria and Noita reproduction by the full-clone acceptance ledger in `new_docs/26_FULL_CLONE_ACCEPTANCE.md`, never by genre similarity or representative mechanics alone. Distinguish design feasibility, verified SDK capability, and observed clone fidelity.
- Product changes are tracked as P (Plus) items. An explicitly adopted P takes precedence over overlapping earlier specifications and implementation. Record the affected requirements and implementation work; do not claim completion from documentation alone.

## Engineering rules

- Keep simulation state independent from windowing, rendering, audio, wall-clock time, and asynchronous completion order.
- Follow the agreed contracts under `new_docs/` for ordering, atomic state changes, capacity errors, and save/replay coverage. The retired documents do not establish these product contracts.
- Keep rendering data separate from simulation data. Prefer a correct scalar reference implementation before optimizing.
- Avoid new dependencies unless the task needs them. Explain their purpose and alternatives in the change.
- Make focused changes and keep public API, content, save format, and simulation version changes explicit.
- Explain ownership and borrowing in plain language when teaching Rust concepts.
- Prototypes are prohibited in almost all cases. Implement production-intent behavior that satisfies the applicable contracts and milestone exit criteria. Do not mark a milestone complete when only a prototype or partial implementation exists.
- If a prototype is genuinely necessary, notify the user before presenting or relying on it. Clearly identify the prototype's scope, missing behavior, limitations, and the work required to replace it. Keep that status visible in code and relevant documentation so readers can distinguish it from complete behavior without searching the entire repository.

## 必須：性能予算・安全性・FPSを同時に守る（全エージェント共通）

以下は新規実装、修正、最適化、独立レビュー、テスト、PR作成のすべてに適用する強制ルール。**安全性やテスト数を増やすために実行時の負荷を際限なく増やしてはならない**。同時に、FPSのために所有権・保存則・決定論・原子性を破壊してはならない。既存の受け入れ条件を引き下げないこと。

### 二つの独立した目標

- **モニターへの表示・プレゼンテーション：144 FPSを目標**（表示フレーム予算は約 **6.94 ms/frame**）。描画、入力反映、合成、フレームペーシングを含む。モニターが144 Hzで動作できる環境を想定した性能目標であり、全ハードウェアでの保証値ではない。
- **ゲームtickとゲーム状態のレンダリング更新：60 Hz / 60 FPSを目標**（固定シミュレーション予算は約 **16.67 ms/tick**）。物理・ゲーム状態の更新周期と、ゲーム状態に依存するレンダリング更新の基準。表示の144 FPSと同一クロックに固定しない。
- 144 FPS表示と60 Hzのゲーム状態更新を**混同しない**。レンダリングスナップショットと補間・フレームペーシングにより高頻度の表示を可能にする。描画フレームごとに物理tickを追加実行したり、可変描画deltaを正規シミュレーションへ混入したりしない。補間は表示専用で正規World・保存・replayを変更しない。
- 上記は**目標値**。達成判定には実機・GPU/CPU・解像度・シーン・対象セル数・活動chunk数・粒子数を明示した計測が必要。根拠なく「144 FPS達成」「60 FPS保証」と宣言しない。シミュレーション・描画が完成していない段階では未計測と記録する。

### 安全性は境界で確保し、ホットパスを軽量にする

- Content・Provider・Schema・設定の**登録時**、入力を受け付ける時、保存データの**復元時**、Worldの原子的**commit時**に不変条件を検証する。変更されていない静的不変条件を全セル・全粒子で毎tick再検証しない。
- ホットパスでは必要な動的な境界検査、世代付きhandle、所有権・予約・衝突検査、オーバーフロー対策は維持する。「速いから」という理由で未定義動作、データ競合、物質の増殖・消失、tick順序依存を認めない。
- 全World checkpoint、全表現の完全な質量再集計、保存byte比較、広域の全セル照合は**テスト・監査モード・必要な明示検証境界**へ寄せる。毎tickの必須処理にするときは差分・増分方式で必要性と計測結果を示す。通常の生産ホットパスへ無条件で追加しない。
- World編集の原子性は維持しつつ、実際に変更したchunk・channel・particleだけを扱う。不要な全World clone、繰り返しallocate、冗長なlock、全セル走査、毎tickのO(N²)再検証を避ける。差分journal、copy-on-write、世代付きcache、予約scratch、容量事前確保を既存契約に沿って検討する。
- Dirty/Sleep/Wakeは**参照実行と結果が一致する証明**がある処理に限定する。変更の依存範囲・halo・active region・外部編集・Provider・ロード/退避・保存復元・World分岐をwake対象として扱う。Sleep判定それ自体の費用が省略した処理の費用を超えないことを測る。推測だけで省略しない。
- 作者Providerやプラグインの拡張性を保つ。独自挙動が不明な場合は無理に高速化せず参照経路へfallbackする。安全性のために全作者callbackへ高価な全Worldコピーを強制する設計は避ける。

### 性能を仕様として測定・レビューする

- セル輸送、圧力、粒子、材料、熱・反応、Runtime、renderingの**高頻度経路を変更するPR**では、関連ベンチマークまたは既存の比較測定を使って、修正前後の**同じ条件**を比較する。計測を行えない場合は未計測と明示し、性能が改善したと主張しない。
- 最低限の代表ワークロードは、静止領域が多いWorld、大量の活動粉体/液体、正規粒子多数、複数chunkの境界と広域Job、作者Providerを使用した場合、描画負荷の高いシーン。全組合せを**毎小変更で**実行せず、影響があるワークロードを選び、工程出口で全体を確認する。
- 平均値だけでなく **P50/P95/P99** のtick・render時間、メモリピーク、allocation回数/量、処理したセル・粒子・chunk数、dirty/wake発生量、規模に対する伸びを記録する。FPSの分母に含める処理範囲と計測環境を明記する。
- 退行が見つかった場合は先にprofilingで原因を特定する。安全性を無差別に削るのではなく、重複検証・不要なallocate・全量コピー・広域走査の削減を優先する。性能問題を未解決のまま「完了」と判定しない。ただしユーザーが定めていない絶対的な粒子数・ハードウェア仕様を勝手に合否基準へ追加しない。
- 新機能を追加する際は **演算量・割当量・最悪時の上限・睡眠時のコスト**を簡潔に見積もる。明らかなホットパスのO(N²)、無制限job、毎フレームの永続IOなどは設計段階で拒否する。

### 開発時間・PR・レビューを最小化して工程を完成させる

- **設計書の固定された受け入れ条件 → 現在の実装と証拠 → 真の未達項目 → 最小の修正**の順で作業する。「より完璧にしたい」という理由で工程範囲・将来機能・公開APIを無制限に増やさない。E05/Q04とE06/Q05の境界を守る。
- 実装済みの機能や成功済みの検証を繰り返し最初から調査しない。影響範囲の対象テストを優先し、広域回帰・性能測定は統合時の必要なタイミングにまとめる。レビューで実質的な欠陥を指摘された場合は修正と再レビューを続ける。
- **単純な追加テスト・小修正・レビュー対応ごとにPRを増やさない**。可能なら既存の適切な機能PRを更新し、独立レビュー可能な大きさでまとまった機能単位へ整理する。PR数の削減自体が目的ではない。未継承の旧公開APIやテストを捨てない。
- GitHubでのcommit/push・PR作成更新は既に許可されている範囲で行う。**mainへの直接pushとPRのマージは行わない（Codexが担当）**。依存ブランチのbase変更・fast-forwardでGitHubがPRを自動merged扱いする場合もあるため、意図しないマージ判定が起きないか確認し、危険な操作は避ける。
- **E05の出口条件を全て実測・レビュー済みと判定した時点で**、最後のE05 PR本文に「E05はPR #4、#5、… の順でマージしてください。」という形で、最初から最後までの**採用PRだけの完全なマージ順**を書く。重複・除外PR、依存関係、各マージ後の確認事項も明示し、推測の順番を「壊れない確定順」と呼ばない。
- 無駄な再ビルドを避ける。既知のメモリ不足環境では `CARGO_BUILD_JOBS=1`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_PROFILE_TEST_CODEGEN_UNITS=1024`、`CARGO_INCREMENTAL=0` を基本とし、対象テストを分割して**重量級ビルドを同時に複数走らせない**。ビルド待機中は競合しない静的レビュー・次の実装の整理・必要な文書整備を行う。環境起因の未実行と実際のテスト失敗は厳密に区別する。
- ユーザーの**約3日でエンジン全体完成**は開発効率改善の挑戦的な計画目標とし、達成を保証する事実とは扱わない。安易に時間を引き延ばす見積もりも、根拠なく短縮した見積もりも禁止。残る工程・測定値・重大な不確実性を根拠に予測し直し、時間だけを理由に検証不足の機能を完成と宣言しない。

## E05作業で判明した遅延要因と再発防止（2026-10-19）

E05/Q04の作業では、実装そのものに必要な不具合修正に加えて、次の進め方の問題が開発時間を延ばした。以後の工程では、原因を記録するだけでなく以下の手順を実行する。

### 遅延要因

- **工程境界と日付を根拠なく決めた。** E05/Q04とE06/Q05の区分を読み違え、設計書に根拠のない2024年を設定した。ユーザーへの複数回の確認と訂正が発生した。
- **受け入れ条件と追加調査の境界を早く固定しなかった。** Q04の必須条件と未要求の連携候補を並行して扱い、設計書と実装の不一致も後段で発見したため、調査と記録のやり直しが生じた。
- **ビルド環境と検査の順序を十分整理しなかった。** 出力先やビルド設定の違いで重い再ビルドが発生し、メモリ不足でテストが起動しない試行の後、省メモリ設定で再実行した。工程完了前に広い検査を行う手順も、ユーザーのQ単位・E単位の進め方に沿っていなかった。
- **進捗台帳と文書の同期が遅れた。** 複数PRと累積記録を更新する間、完了済み・未完了の記述が併存した。CI成功後もPR本文・README・証拠記録に「実行中」「未確認」が残った。
- **完了時間を実測根拠なしに短く見積もった。** 残実装だけでなく、統合検査、ビルド待ち、CI、レビュー待ちを分けていなかった。

PressureJobの不要なrevision更新やArena候補複製の過剰割当は、検査で確認された正当な正確性・性能修正であり、不要な作業とは扱わない。遅延防止では、これらを対象テストと関連profileで早めに検出し、修正範囲を受け入れ条件内に保つ。

### 次工程から必ず行うこと

1. 着手時に`new_docs/README.md`と対象Qの設計ページだけを読み、Q/E境界、固定受け入れ条件、明示された未決事項を短い作業台帳に記録する。日付・範囲・APIを推測で補わない。
2. 各条件に対し、現在の実装、根拠となるテスト・計測・文書、真の未達を対応づけてから最小修正を始める。条件外の候補は実装せず、必要性が分かった場合だけ設計根拠を確認する。
3. Q作業中は影響範囲の単体テスト、連携テスト、単体レビューを行う。Qが揃ってからE全体の検査と総合レビューを一度実施する。成功済み検査は影響コードが変わらない限り再実行しない。
4. 既存の同じCargo target directoryと省メモリ設定を再利用し、重量級ビルドを同時に走らせない。失敗が環境起因かコード起因かを分け、未実行を失敗扱いしない。
5. 作業台帳を正としてREADME、対象設計書、evidence、PR本文を同じ状態へ更新する。ローカル検査、CI、独立レビューを別々の証拠として記録し、外部レビュー待ちを完了と表現しない。
6. 見積もりは未達項目、再利用できる検査、実測したビルド・CI時間、レビュー待ちを基に範囲または幅で示す。根拠のない確定時刻を伝えない。

## Useful commands

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
cargo clippy --all-targets --locked
```

Run the narrowest relevant checks while iterating, then the full set when implementation changes affect the engine. Never claim a check passed unless it was run successfully.

## Documentation

Keep product purpose, creator workflows, requirements, design decisions, and verified implementation records under `new_docs/`. Preserve retired documentation without treating it as an active specification. Use relative links that work on GitHub.

Update the root `README.md` whenever implementation, public APIs, setup or usage, validation status, or milestone progress changes its contents. Keep the GitHub introduction, runnable commands, verified capabilities, remaining work, and links aligned with `new_docs/` in the same change. Distinguish implemented behavior from v1.0.0 goals and link completion claims to verification records; never copy retired milestone claims into the README.

## Language and code documentation

- Always communicate with the user in Japanese.
- Code documentation and comments must be written in Japanese. English code documentation or comments are prohibited; translate existing English documentation or comments when modifying the relevant code.
- Write detailed, practical Japanese documentation so a human can understand the code by reading it. Explain purpose, assumptions, invariants, important control flow, failure behavior, and ownership or state changes where relevant. Avoid comments that merely restate the syntax.
- Keep public Rust API documentation (`///` and `//!`) in Japanese and generate/check it with [`cargo doc`](https://doc.rust-lang.org/cargo/commands/cargo-doc.html) when documentation changes warrant it. Generated documentation is placed in `target/doc`; a crate can be selected with `--lib` or `--bin` (for example, `cargo doc --lib`).
