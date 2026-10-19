# E06：熱と反応

## 状態

E06/Q05の実装、出口検証、単体・結合・統合ローカルレビューを完了。設計上の出口は、断熱energy保存、熱の上限、相変化往復、反応出力満容量、reaction競合、保存中断と再開の一致であり、下記fixtureと全target結果で確認した。[実装ロードマップ](../../19_IMPLEMENTATION_ROADMAP.md)と[熱・反応契約](../../10_MATERIALS_AND_CELL_PHYSICS.md)を参照。

E05の差分から分離したローカルbranch `codex/e06-heat-reactions`で作業中。GitHubへのpush、PR提出、mergeはしていない。外部自動review jobを工程ゲートにしない。

## 現在の実装

- `src/world/thermal_reactions.rs`に連続検証付きenthalpy curve、顕熱・潜熱state変換、material相の選択、断熱熱移動、保存codecを追加。
- energyはmaterial追従channel、正方向edge residualは位置固定channelに分離。
- `WorldEdit::step_thermal_channel`を追加し、active rectangleの熱移動・相material更新を同一候補へまとめる。
- `ReactionRuleSet`が構築時に静的ruleと重複keyを検証し、対応するenthalpy familyを保持する。reaction job・燃料/濡れ/腐食/timer sidecarを追加し、cell・正規particle出力、rule別Deferred/SameTick schedule、出力先/particle slot不足の延期に対応する。
- `WorldEdit::step_reaction_channels`はenergy・数量・sidecar channelとWorld StateStore jobを同一候補で更新する。
- 極端なenthalpy補間の中間値overflowを検出し、panicでなくengine errorとして返す。

## 実行済み検証

| 範囲 | 結果 |
| --- | --- |
| `cargo test --locked --lib world::thermal_reactions::tests` | 18件成功（2026-10-19） |
| `cargo test --locked --test thermal_reactions` | 5件成功。WorldEdit熱更新、加熱/冷却相変化、pool満杯延期、particle出力payload、ReactionJob/World channel保存再開（2026-10-19） |
| `cargo test --locked --all-targets` | 成功。131 unit testを含む全test target成功。E06の手動performance benchmark 2件は通常実行からignored（2026-10-19） |
| `cargo check --all-targets --locked --jobs 1` | 成功（2026-10-19） |
| `cargo clippy --all-targets --locked --jobs 1 -- -D warnings` | 成功（2026-10-19） |
| `cargo doc --locked --lib --no-deps` | 成功（2026-10-19） |
| `cargo fmt --all -- --check`・`git diff --check` | 成功（2026-10-19） |
| 熱solver単体benchmark | Windows 10.0.26300、Ryzen 7 2700 (8 core / 16 logical), Rust/Cargo 1.98.1、test opt-level 3、debug 0、codegen-units 1024。100,000 cell/199,300 edge、warm-up 5、測定50回。P50 12.847ms / P95 12.993ms / P99 13.077ms、solver内allocation 0回 / 0 byte、追加peak 0 byte（2026-10-19） |
| 反応候補構築benchmark | 同環境、10,000候補、warm-up 5、測定50回。初期実装（候補全sort・逐次拡張）はP50 16.473ms / P95 16.881ms / P99 16.977ms、13 allocation/30.14MB/回、peak追加15.07MB。rule安定順の事前構築とscratch一括予約後はP50 11.196ms / P95 12.080ms / P99 12.290ms、1 allocation/9.2MB/回、peak追加9.2MB。条件を揃えた最終計測（2026-10-19）。WorldEdit込みreaction処理は未計測 |
| WorldEdit込み熱経路 | 未計測。全Worldの60Hz根拠とはしない |

テストは省メモリ設定（Cargo 1 job、test debug 0、codegen units 1024、incremental無効）で同一targetを使う。

## ローカルレビューで確認した点

Q05の単体レビューでは、enthalpy補間中間値overflowを検出し、反応ruleを不変RuleSetで一度だけ検証するよう修正した。tick順、read/write set、Unavailableとactive境界、容量不足のdefer、job decode後のauthoritative rule照合、output capacity確保、WorldEdit rollback、channel ownership、source/sink以外のenergy収支を確認した。粒子出力候補を「同じ座標に生成物があるか」で推測せず、実際にcommitしたcandidateから記録するよう修正し、競合candidate fixtureを追加した。rule順は構築時に確定し、tick毎の候補sortを除いた。熱solver10万cellと反応候補1万件の単体計測を記録した。

単体reviewでは補間overflow、登録時rule検証、候補sort/割当、reaction粒子出力のcommit候補識別、SameTick連鎖の保存可能な再構築通知を確認・修正した。結合reviewではchannel ownership、active/Unavailable境界、WorldEdit rollback、capacity延期、保存restore・job再開、phase往復、同一出力先競合を確認した。chain-pendingのcodec往復と新規job再構築をfixtureで検証し、最終差分の全target・check・clippy・docも成功した。WorldEdit込み性能は未計測であり、熱solver・reaction候補構築単体の計測を実World全体の60Hz根拠にしない。Block instance migrationはE07境界として記録し、E06では相変更candidate全体をrollbackしてinstance stateを保持する。
