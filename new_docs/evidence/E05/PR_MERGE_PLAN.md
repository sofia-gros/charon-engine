# E05：最終マージ順の確定台帳（作業中）

**この文書は完了宣言ではない。** PRのマージとmainへの直接pushは実装担当が行わず、Codexが行う。E05最後のPR本文に確定した全PR番号列と確認条件を記載する。

## 現在のPR依存と重複

2026-10-18時点のGitHub base/headを確認した。PR #4のbaseはmain。基礎系列は次の直列。

**#4 → #5 → #6 → #7 → #8 → #9 → #10**

PR #10の上で2系列に分岐している。

| 系列 | 依存順 | 方針 |
| --- | --- | --- |
| 初期診断・Provider | #11 → #12 → #13 | #14以降との固有機能を差分監査し、重複して両方マージしない |
| 最新の全表現・Q04 | #14 → #15 → #16 → #17 → #18 → #19 → #20 → #21 → #22 | #10の直列子孫。収支、力学、Job、Sleep、Runtime、Q04 |
| 現在作業中 | Materials phase統合 | #22の子孫。E05の原子的処理と参照一致を検証中 |

2026-10-18の集約候補では、#11〜#13固有APIの移植に加え、PressureJob群の原子的公開、PressureJob・MaterialMotionJob・PressurePushSearchを一つのWorldEditへ載せる入口、新圧力snapshotから押出しを再探索する入口を追加した。対象`material_motion_job` 10件、`pressure_job` 27件は成功した。これは依存PRの統合・独立レビュー・全体品質検査・CI完了ではないため、採用順と#11〜#13の置換判定は未確定のままにする。

ローカルのgit祖先検証で、#10→#22、#18→#22、#20→#22を確認した。ただし祖先関係だけで全体のマージ安全性が証明されたとは扱わない。

### 初期診断分岐に残る固有契約（2026-10-18差分監査）

PR #11〜#13は**まだ安全に閉じられない**。#11のresident/archive内訳付き整数token/mass診断、#12のlayer単位の混合成分・粒子の内訳、#13の旧UniformMaterialForcesと一様gas浮力は、#14〜#17の全表現収支・標準接触浮力へ完全には引き継がれていない。2026-10-18に集約候補へAPIと回帰試験を移植し、material_inventory 6件・material_mixtures 21件・material_forces 28件のWindows対象実行は成功したが、移植差分の独立レビュー・CI・全体品質検査は未確認である。

全表現の有理数収支が旧診断を数学的に包含する部分は多いが、cell/particle・resident/archive別の内訳を必要とする作者利用や旧一様浮力の意味は異なる。固有の公開契約を移植しテストを再実行するか、正規設計書で旧契約を明示的に不採用とする判断が出るまでは、#11〜#13を「完全置換済み」と断言して閉じない。この状態では最終マージ順も未確定とする。

## PRを減らす推奨案

### 2026-10-18の実施済み整理と停止条件

PR #14のheadを、旧#17の成功済みcommit 280541dへ**fast-forward**し、#14が旧#14→#17の全コミットを含むようにした。PR #18のbaseを#14へ変更し、#16と#17をsupersededとしてclosedにした。GitHubは#15のcommitが#14のbase側へ入った時点で、#15を**merged=true**と記録した（mainにはマージしていない）。PRマージ操作は行っていないが、自動的なマージ判定を発生させた以上、ユーザーの「マージ判断はCodexに任せる」という条件を厳密に守るため、これ以上の同様のbranch集約・base変更・closeはCodexへ委ねる。

現時点で#11〜#13の固有公開診断API/一様力Providerは集約候補へ移植したが、独立レビューとCIを終えるまで置換判定しない。元PRは開いたまま保護する。#18〜#22の追加修正集約もレビュー・Q04工程出口確定までは未実行。集約未完了の現時点で番号列を最終確定と記載しない。

**最も安全でPR件数も少ない方式は、検証済みの最新E05 headからmain宛にE05全体の集約PRを1本作成すること。**

集約PRのbaseは最新のmain、headは検証済みの最新E05実装とする。集約PRに#4〜#10と#14〜#22の全コミットが含まれることを祖先検証し、単一PRとしてQ04全受け入れ・独立レビュー・CIを確認する。#11〜#13は固有実装を確認してから置換対象へ指定する。E05完成前は旧PRを維持し、レビュー可能な小単位の履歴を失わない。

**独立レビューで追加確認した差分：** #11のresident/archived別の素材数診断APIとゼロ件素材の列挙、#12のcell/particle別Mixture内訳API、#13の一様物性Force Provider APIは、#14〜#16の全表現監査や素材別Providerより狭い機能だが、**同じ公開API・数値結果ではない**。用途差が残るため集約候補へ移植し、対象テストを通過した。公開用途・実装の独立レビュー、全体品質検査、CIが未確認なので、#11〜#13の完全置換判定は保留する。

E05のQ04全受け入れ条件と独立レビュー、CIが通過するまでは旧PRをcloseしない。集約PRの受け入れ完了後、置換元PRの説明文へ集約先を残して重複PRを閉じる。元ブランチやコミットは削除しない。

全実装を包含し最終レビューに通過した場合の**推奨マージ順**は次のとおり。

**#（E05全体の集約PR番号）のみ**

この場合、#4〜#22の該当する既存PRは集約PRが代替するため、mainへ個別マージしない。番号を省略しているのではなく**マージ対象から除外した番号と理由**を最終PRにすべて明記する。旧ブランチ・コミットとレビュー経緯は残す。

既存の基礎PRを独立レビュー済みとして個別にマージする判断をCodexが行った場合、代替の**候補順序**は次のとおり。

**#4 → #5 → #6 → #7 → #8 → #9 → #10 → #（集約PR番号）**

レビュー上どうしても分割を維持する場合の**最後の予備候補順**は次のとおり。

**#4 → #5 → #6 → #7 → #8 → #9 → #10 → #14 → #15 → #16 → #17 → #18 → #19 → #20 → #21 → #22 → #（その後の最終PR）**

いずれも#11〜#13は自動的に同時マージしない。最終的には採用方式を**一つ**に決め、番号とhead/base SHAを再照合する。既存の積層PRを一つずつマージする方式では、GitHubの現在のbaseが前PRブランチなので、毎回baseをmainへ更新するまでマージしてはいけない。

## E05最後のPRの本文に必ず書く項目

1. 「**E05は PR #4、#5、…、#（最終） の順にマージしてください。**」と省略なしの番号列。含まないPRの番号・理由・置換先も明示。
2. 各PRのhead/base、採用merge方式。上流PRをmainへマージ後、次PRのbase更新とCI再実行。Squashで履歴が変わる場合は適切なrebaseと差分検証を済ませるまでは続行しない。
3. PR #4〜#10のAPI/保存仕様との互換性、全表現収支、Sleep/Wake、Runtime原子性、Q04の参照比較、レビュー指摘ゼロ、CIの確認結果。
4. 各マージ段階の検証ゲート。fmt、all-targets check、省メモリの対象テスト、GitHub Actionsなど。失敗したら後続マージを止める。

Q04工程出口が未達の現時点では候補を「絶対に壊れない」と保証しない。

## 2026-10-19：PR #22の実状態と検証済み依存順

以下は上の当時の推奨案を、実際に作成済みの集約PR #22に合わせて更新する。GitHub上で各PRのbase/head SHA、state、mergeableを読み直した。#4〜#10はすべてopen・mergeableで、各PRが直前のPR branchをbaseにする直列だった。2026-10-19の確認時点でPR #22はopen・mergeable、baseはPR #10のhead、headは`2330b7759a375eebd7535dde5a9b446924dab44d`だった。PR #22のheadに対するRust CI、E03、E04、E05 workflowはすべてsuccessとなった。PR #22はレビュー可能状態だが、独立したPR review submissionとinline threadはまだない。

**PR本文に記載した採用候補順：#4 → #5 → #6 → #7 → #8 → #9 → #10 → #22。** これはGitHubの現在のbase/headを反映した候補列であり、実際のマージは行っていない。各PRのbase/headは次のとおり。

| PR | base branch / SHA | head branch / SHA | 2026-10-19の状態 |
| --- | --- | --- | --- |
| #4 | `main` / `3ae60e20a34f7162f739988d25d332045bc52967` | `codex/e05-cell-and-fluid` / `f4c2b550f939d26a9f1c8eb4647037c5e4e8d56d` | open・mergeable |
| #5 | `codex/e05-cell-and-fluid` / `f4c2b550f939d26a9f1c8eb4647037c5e4e8d56d` | `codex/e05-motion-and-sweep` / `25d522f13888073473d47797431184b51704ad1d` | open・mergeable |
| #6 | `codex/e05-motion-and-sweep` / `25d522f13888073473d47797431184b51704ad1d` | `codex/e05-material-receipts` / `01ba6facf1bbe98cc7ce9cf5054e1c6db73ce4ce` | open・mergeable |
| #7 | `codex/e05-material-receipts` / `01ba6facf1bbe98cc7ce9cf5054e1c6db73ce4ce` | `codex/e05-wide-pressure` / `25e486db8e13396a40ec1cba5527d3f04b7b46e4` | open・mergeable |
| #8 | `codex/e05-wide-pressure` / `25e486db8e13396a40ec1cba5527d3f04b7b46e4` | `codex/e05-mixtures` / `6533710d977715970fa970651c3ba11f6e68b7a8` | open・mergeable |
| #9 | `codex/e05-mixtures` / `6533710d977715970fa970651c3ba11f6e68b7a8` | `codex/e05-push-resume` / `a9d7a4490b9976cc8962a4fb9b25de0fa23e1ccf` | open・mergeable |
| #10 | `codex/e05-push-resume` / `a9d7a4490b9976cc8962a4fb9b25de0fa23e1ccf` | `codex/e05-material-particles` / `5cfb0cf465e051be179c20f9817b268ca6d85214` | open・mergeable |
| #22 | `codex/e05-material-particles` / `5cfb0cf465e051be179c20f9817b268ca6d85214` | `codex/e05-q04-review-fixes` / `d8582af45a5216eb55d40a01f0826d44639da5e4` | open・mergeable・CI success・再レビュー依頼中 |

PR #11〜#13の個別マージは除外する。固有のresident/archive・混合内訳診断と旧一様力ProviderをPR #22へ移植したためである。PR #14〜#21も#22の集約差分へ含め、重複適用を避けるため個別マージ対象から除外する。PR #15の過去の`merged=true`はbranch base変更時にGitHubが付けた判定であり、mainへのマージではない。

ローカル出口検査と上記headのCIは成功済み。独立レビューでは正規粒子を無視するPressurePushSearchとMaterialMotionJobの二件のP1指摘があり、修正commit `d8582af`で解消した。MaterialMotionJobのinline threadはresolveし、PressurePushSearchのreview commentへ修正・検証結果を返信した。再レビューを依頼しており、その確認・必要な指摘対応を終えるまではE05完了としない。最終PR本文のマージ順・除外理由は現時点で上記候補と一致することを確認した。mainへのpushやPRマージは行っていない。

### 積み上げPRをmainへ順次取り込むときのbase更新

上記の番号順は、各PRをそのまま現base branchへ連続マージしてよいという意味ではない。#4をmainへ取り込んだ後、#5のbaseをmainへ変更し、差分に#4の変更が重複せず#5固有変更だけが残ること、PRが意図せずmerged/closed扱いにならないこと、更新後CIの成功を確認する。同じ手順を#10まで繰り返す。#10をmainへ取り込んだ後は#22のbaseもmainへ変更し、#4〜#10の変更が差分に再出現しないことを確認してから、PR #22のレビュー・CIを最終確認する。base変更で既存差分が再登場する、PRが自動merged扱いになる、または予期しない変更が見える場合はその場で停止し、rebase/merge方式と差分を再監査する。確認なしに後続PRを進めない。
