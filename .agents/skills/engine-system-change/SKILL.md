---
name: engine-system-change
description: Implement or review Charon Engine simulation, materials, chunks, collision, gameplay, persistence, or renderer changes while keeping the product design and shared contracts consistent. Use when a task changes engine behavior or a cross-system boundary.
---

# Engine system change

1. Read `new_docs/README.md` and the relevant pages under `new_docs/`. The user's explicit requirements take precedence. Do not consult retired documentation in the root README, `codex/`, or `gemini/` unless the user explicitly requests a historical comparison.
2. Inspect the current implementation and its tests. Separate existing behavior from the user's design target before editing. Existing code and previous milestones do not limit the product's intended capabilities.
3. Trace the change through its read/write state, tick phase, coordinate and units, capacity limits, deterministic ordering, save/hash/replay coverage, and sleep or wake behavior.
4. For material or attachment changes, also trace transfer, reaction, inventory, schema version, content hash, and save restore. For chunk changes, distinguish visible, prefetched, active, frozen, and requested states.
5. Implement the smallest reviewable change. Add dependencies only when the benefit and alternatives are clear.
6. When the task includes verification or the change needs regression coverage, add or update targeted tests. Run focused checks, then the repository checks in `AGENTS.md` when the change warrants them. Report checks accurately.
7. Update the owning pages and any applicable requirement, decision, or implementation records under `new_docs/` when behavior, a contract, API, configuration, or supported workflow changes. Record product changes as P (Plus) items when applicable, with explicit precedence over conflicting earlier specifications. Do not introduce game-specific requirements into the engine core.
8. Finish with a short summary of behavior, validation, design or version impact, and unresolved risks.

## 開発効率と進捗の追跡

過去の工程では、QとEの完了単位を混同したこと、設計範囲を確かめる前に実装や将来要件まで広げたこと、成功済みの広域検査を細かな変更ごとに繰り返したこと、証拠READMEの所在と進捗が利用者に見えない状態で長時間作業したことが、見積もりと実際の進捗のずれにつながった。次の運用でこれらを防ぐ。

1. 作業開始時に `new_docs/19_IMPLEMENTATION_ROADMAP.md` と該当する設計ページ、`new_docs/evidence/<E>/README.md` を照合し、現在のcheckout/branchと証拠ファイルの所在を確認する。古いcheckoutや別worktreeの状態を現在のbranchの状態として報告しない。
2. EはQ単位で進める。各Qについて受け入れ条件、既存実装・成功済み証拠、真の未達項目を対応づけてから実装し、Qごとに対象unit test、integration test、ローカル単体レビューを行う。Qの完了とEの完了を同一視しない。
3. Qの途中では最も狭い関連testと差分reviewを使う。全target check、全体test、総合reviewはE内のQがすべて終わった後にまとめて一度実施する。変更のない対象に成功済み検査を再実行しない。重量級Cargo buildは一つずつ実行する。
4. 作業範囲は固定された受け入れ条件までとし、後続Q/Eの項目、追加の汎用API、重複するPRを先回りで作らない。Q/E境界や設計記述に矛盾がある場合は、勝手に工程を移動せず根拠を示して解決する。
5. Eの証拠READMEは開始時に作成・維持し、先頭に「完了・部分完了・未着手」の数、次に残作業と完了条件、末尾に検証記録を置く。細かな時系列ログは `HISTORY.md` へ分け、READMEの進捗表を正本にする。
6. 30分を超える作業では、長い無報告状態を作らず、作業単位の完了数、現在のブロッカー、次に閉じる条件を報告する。残時間は未達条件と実測した検査時間から見積もり、根拠がなければ幅を示す。
7. PRはE全体の検査とレビュー後に必要な採用PRだけを作る。GitHub PR操作が作業時間・利用枠を大きく消費する環境では、Q段階のレビューはローカルで完了し、PR往復を細かな修正のたびに行わない。
