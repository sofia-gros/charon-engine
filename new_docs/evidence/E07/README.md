# E07/Q06：物体・剛体・破壊連成の実装証拠

## 状態

E07/Q06の実装・工程検証を完了し、P1（Rapier sensor overlapがcontact event列へ出ない）を修正したPR #25を2026-10-20にmainへマージしました。README専用PR #24は内容を包含したためクローズ済みです。レビュー指摘修正後の118件剛体統合test、event shape unit test、Clippy、rustdoc、format、差分検査も成功しています。Unavailable/lease境界、collision query、custom shape候補step、`BodyThermalBoundaryProvider`、`CouplingProvider::fluid_contacts`/`fluid_occupancy`、逐tick基準比較も受け入れtestで確認しています。性能・検査範囲とE13のB03最大負荷profileはこの記録の各節を参照してください。コードと証拠は`codex/e07-rigid-bodies`にあり、このREADMEを現行状態の正本、[HISTORY.md](HISTORY.md)を経過記録とします。

実装にはRapier f64 backend、World所有の`BodyPhysics`、回転可能なBodyStencilとcompound collider、joint、materialに基づく質量・重心・慣性、body-local grid、物質・流体・熱・反応・粒子連成、破砕・再固定、作者fixed-step/query/coupling provider、原子的なWorld編集とcheckpoint復元が含まれます。BodyPhysics snapshotはCHBODY12、World checkpointはCHWORLD3です。Q06の機能・検証を確認し、E07出口の測定・総合検査と区別します。

## レビュー指摘の修正

PR #25のP1レビュー指摘では、Rapierのsensor overlapがintersection graphに記録される一方、標準接触event収集がcontact graphだけを走査しており、sensorのEnter/Stay/Exitが公開されない点を指摘されました。intersection graphからsensorを含む交差pairを追加収集し、既存の安定順・遷移・event容量検査へ統合しました。sensor eventは中心間の幾何を使い、solver impulseをゼロに固定します。通常接触と同一Body pairに重複する場合は通常接触の観測を優先します。

修正後に`cargo test --locked --test rigid_bodies --jobs 1`が118件成功し、sensor Enter/Stay/Exitの追加testを含め失敗0件でした。event shape unit test 1件、`cargo clippy --locked --lib --test rigid_bodies --jobs 1 -- -D warnings`、`RUSTDOCFLAGS=-D warnings cargo doc --locked --lib --no-deps --jobs 1`、format、`git diff --check`も成功しました。各fixed substepでRapier intersection graph上のpairを一度走査するため追加計算量はO(I)です。sensorを含まないWorldでは有効交差のgeometry/event生成は発生しません。性能改善は主張しません。

## Q06の要求と現在の証拠

| 設計18/11/25の要求 | 現時点の証拠 | 判定 |
|---|---|---|
| 高速薄壁、回転bodyと流体の相互作用 | `world_fixed_step_does_not_tunnel_through_a_thin_terrain_wall_at_high_speed`、`body_step_exchanges_equal_and_opposite_fluid_drag_impulse`、`body_stencil_volume_contacts_preserve_exact_area_after_rotation`、統合fixture `q06_acceptance_rotating_body_fluid_heat_fracture_particle_checkpoint_chain` | 薄壁・流体反作用・回転面積の個別testと統合fixtureが成功 |
| kinematic moving platformとdynamic body | `moving_kinematic_platform_transfers_motion_to_dynamic_body`で8固定tickのsurface contactを検査 | 個別結合test成功 |
| 加熱・部分反応・破砕・粒子化・保存復帰後の継続 | `body_surface_heat_precedes_reaction_schedule_in_fixed_step`、`body_local_reaction_schedule_resumes_after_restore_and_skips_unchanged_grid`、`damage_particleization_and_fracture_preserve_total_momentum_atomically`、統合fixture | 境界熱fluxを反応scheduleより先に同じ固定step候補へ適用する個別testと、回転・流体・熱反応・破砕・保存復元の統合fixtureが成功。全sidecar/phase組合せを意味しない |
| joint移管、block個体と状態の移送、容量不足時rollback | `fracture_migrates_joint_to_the_component_owning_its_anchor`、`fracture_reports_and_removes_joint_whose_anchor_is_in_a_gap`、`body_damage_particleizes_block_attachment_state_and_keeps_remaining_block`、`fracture_capacity_failure_rolls_back_all_fragments` | 個別証拠あり |
| custom shape、one-way、開始時重なり、全hit/truncate、TOI/normal/geometry revision | `CollisionQueryShapeRef`はprovider key/versionとowner/object/partで作者形状を安定参照する。`custom_step_queries_its_updated_candidate_and_uses_provider_toi`はcandidate custom sweepからTOI 0.5を反映する。`world_collision_queries_provider_validates_and_stably_orders_hits`は標準sweepとcustom overlap、one-way/initial-overlap等のrequest delivery、stable ordering、truncate、geometry revisionを検査する。invalid version拒否も別testで検査 | 独立queryと候補stepへの代表接続testが成功 |
| Body/terrain/custom接触eventと固定step置換 | 標準contact event、sensor intersectionのEnter/Stay/Exit、作者provider event、`BodyContactTarget::Custom`、候補commit/rollback | 受け入れtest成功。sensor eventは中心から決めたpoint/normalを持ち、solver impulseはゼロ。Rapier内部hookは提供せず、作者solverはfixed-step全体を置換する。custom geometry fidelityはprovider責務で、Engineは作者shapeの幾何精度を保証しない |
| Unavailable region、region lease/load、移送 | `world_step_rejects_candidate_pose_that_enters_unavailable_chunk`、`newly_inserted_body_sweep_cannot_cross_an_unavailable_chunk`、`body_crosses_preleased_chunk_boundary_and_world_checkpoint_restores_both`、World保存/region archive復元test | 未resident chunkへの候補侵入はWorld/body/revision全体をrollbackし、事前lease下の境界通過・lease中の退避拒否・checkpoint/保存復帰を検査済み。callerがstep前に必要regionをleaseする契約のQ06証拠は完了 |
| 基準実装・並列候補評価・再開一致 | scalar geometry reference、旧snapshot復元、region/checkpoint結合、統合fixture | **完了**：設計11の再利用double-buffer基準経路とWorld所有候補stepでBodyId別pose/速度、contact event、step番号が8 tick一致。fluid Worldの候補をworker数1/2/4・遅延順変更で評価し、逐tick checkpointが逐次referenceと一致。設計18のscalar reference対象は別のセル・熱・反応・dirty/sleep試験群で照合済み。剛体へ別scalar solverは要求されていない |

## 今回の狭い検証

- `cargo check --locked --lib --jobs 1`：成功。
- `cargo test --locked --test rigid_bodies author_contact_event_can_name_a_stable_custom_target --jobs 1 -- --exact`：1件成功。
- `cargo test --locked --test rigid_bodies world_collision_queries_provider_validates_and_stably_orders_hits --jobs 1 -- --exact`：1件成功。one-way、開始重なり、境界、tick/substep指定がproviderへ届き、stable orderとhit上限時のtruncateを確認。独立queryからprovider所有shapeのOverlap参照も渡す。
- `cargo test --locked --test rigid_bodies world_collision_queries_provider_rejects_inconsistent_or_invalid_results --jobs 1 -- --exact`：1件成功。invalid custom shape version=0をprovider実行前に拒否する。
- `cargo test --locked --test rigid_bodies custom_step_queries_its_updated_candidate_and_uses_provider_toi --jobs 1 -- --exact`：1件成功。provider所有shapeのcandidate sweepからTOI 0.5でposeを確定し、query normal由来のlinear impulse、pre/post速度とCustom接触eventを同一step候補から公開する。
- `cargo test --locked --lib rectangle_batch_matches_scalar_cell_geometry_reference --jobs 1`：1件成功。BodyStencil rectangle batchingの占有cellをscalar polygon/cell参照経路と比較。
- `cargo test --locked --test rigid_bodies q06_acceptance_rotating_body_fluid_heat_fracture_particle_checkpoint_chain --jobs 1 -- --exact`：1件成功。回転body、fluid、heat/reaction、fracture、particleization、checkpoint復帰を一連で検査。
- `cargo test --locked --test rigid_bodies body_local_thermal_phase_change_preserves_an_exact_mass_mixture --jobs 1 -- --exact`：1件成功。潜熱境界でcarrierを切り替えた後も有理mixture成分、energy、body質量が保持されることを検査。
- `cargo test --locked --test rigid_bodies body_local_thermal_phase_change --jobs 1`：2件成功。質量が一致する混合carrier相変化と、混合質量不一致時のWorld checkpoint完全rollbackを検査。
- `cargo test --locked --test rigid_bodies body_local_reaction_ --jobs 1`：8件成功。部分消費ではflags/variation/opaque sidecarを残存sourceに保持し、出力へ複製しない。full-consumeの`MigrateToOutput`はauthor stateをbody cellまたはparticle channelへ移し、明示`Drop`はflags/variation/sidecarを破棄する。schedule復帰とparticle出力回帰も通過。
- `cargo test --locked --lib candidate_copy_shares_grid_chunk_until_candidate_write --jobs 1`：1件成功。候補は既存chunkを共有し、書込時にだけ独立配列へ分離される。
- `cargo test --locked --test foundation --jobs 1`：20件成功。GridChannel、transaction、codecの回帰を確認。
- `cargo test --locked --test world --jobs 1`：51件成功。chunk、channel、World編集、保存復元の回帰を確認。
- `cargo fmt --all -- --check`、`git diff --check`：成功。
- `RUSTDOCFLAGS=-D warnings cargo doc --locked --lib --no-deps --jobs 1`：公開GridChannelとrigid-body moduleの説明を含むlibrary API docsを警告なしで生成。
- `cargo test --locked --test rigid_bodies moving_kinematic_platform_transfers_motion_to_dynamic_body --jobs 1 -- --exact`：1件成功。resident World上でkinematic platformを8固定tick動かし、dynamic bodyへ接触運動が伝わることを検査。
- `cargo test --locked --test rigid_bodies body_crosses_preleased_chunk_boundary_and_world_checkpoint_restores_both --jobs 1 -- --exact`：1件成功。Rapier fixed stepでbodyを2 region間へ移動し、lease中の退避拒否とCHWORLD3 checkpoint復帰後の継続step一致を確認。
- `cargo test --locked --test world a_world_save_contains_all_archives_and_can_restore_them_into_a_fresh_paging_store --jobs 1 -- --exact`：1件成功。CHWORLD3のbody/backend stateと別保存region archiveをWorld::save/loadで復元し、region復帰後にphysics stepできることを確認。
- `cargo test --locked --test rigid_bodies --jobs 1`：現行114件成功。回転・流体・熱反応・破砕・粒子化・checkpoint chainを含むQ06統合targetを2026-10-20に一度実行。
- `cargo test --locked --test rigid_bodies --jobs 1`：比較fixture追加後のQ06 integration target 117件が全件成功。既存114件に加え、fluid連成worker schedulingとWorld候補/reference比較を実行。
- `cargo test --locked --test rigid_bodies body_world_thermal_boundary_conserves_energy_and_restores_fractional_flux --jobs 1 -- --exact`：今回1件成功。作者提供のcustom boundary contactで標準energy収支を確認し、不正local cell時のcheckpoint同一性によってrollbackを確認。
- `cargo test --locked --test rigid_bodies coupling_provider_custom_fluid_contact_geometry_is_validated_and_used --jobs 1 -- --exact`：今回1件成功。作者定義fluid cell/coverage/作用点と占有cellを使い、不正coverage・重複占有時にWorld checkpoint全体が変わらないことを確認。
- `cargo test --locked --test rigid_bodies parallel_world_body_candidates_match_the_sequential_checkpoint_timeline --jobs 1 -- --exact`：今回1件成功。同一checkpointから復元した1/2/4個の独立Worldを異なる遅延順で固定stepし、各tickの完全checkpointが単一Worldの逐次timelineと一致することを確認。これはWorld間のworker schedulingに対する決定論試験であり、Rapier solver内部の並列評価や別scalar/reference solverとの一致証明ではない。
- `cargo test --locked --test rigid_bodies world_candidate_matches_reusable_body_physics_reference_across_workers --jobs 1 -- --exact`：今回1件成功。World所有候補と設計11が基準実装とする再利用double-bufferの`BodyPhysics::step`で8 tickのBodyId別pose/速度、固定・dynamic body接触event、step番号が一致し、そのWorld候補をworker数1/2/4・遅延順変更で実行しても逐tick結果が基準経路と一致することを確認。workerは独立World候補の同時評価を指し、Rapier solver内部の並列化を意味しない。CHBODY snapshot byte列自体はWorld所有terrain revision metadataを含むため比較せず、正規body状態とeventを比較する。fluid/heat/particle連成まで含む別referenceは未実施。
- `cargo clippy --locked --lib --test rigid_bodies --test rigid_bodies_benchmark --jobs 1 -- -D warnings`：今回のWorld candidate/reference worker比較とsample別cost出力を含め、警告なし。
- `RUSTDOCFLAGS=-D warnings cargo doc --locked --lib --no-deps --jobs 1`：module説明と日本語公開API docsを警告なしで生成。
- `cargo test --release --locked --test rigid_bodies_benchmark world_multi_body_large_fluid_step_profile --jobs 1 -- --ignored --exact --nocapture`：Release profile、Windows/MSVC、16 bodies・64 chunks・active fluid row、warm-up 5/sample 50を3回測定。P50は136.6/169.2/140.7µs、P95は1.0781/1.1729/1.0840ms、P99は1.0870/1.2733/1.1065ms。各runのallocationは586.6/step、210,354B/step、追加peak 198,048Bで一定。最遅sampleは1.0998〜1.6302ms、230,240〜266,480Bで、step 0/4/5/9/10/14/15/19/20/25/35/40付近に現れた。debug test profileのP95 14〜15ms tailはReleaseで再現せず、このfixture単体ではコード修正を要する性能欠陥を示す証拠は得られなかった。16 bodyの測定であり、B03規模・全World tickの合格証明ではない。
- `cargo test --release --locked --test rigid_bodies_benchmark body_physics_step_profile world_body_step_profile --jobs 1 -- --ignored --exact --nocapture`（profile別に直列実行）：Rust 1.98.1、Windows/MSVC、AMD Ryzen 7 2700。256 dynamic circle/no contact、warm-up 5/sample 50。standalone `BodyPhysics::step`はP50/P95/P99=265.4/316.8/492.8µs、547.6 allocation・380,118B/step、追加peak 193,816B。World境界確認と原子候補stepは386.7/424.2/438.8µs、650.6 allocation・537,149B/step、追加peak 534,776B。比較対象の旧実装値なしの現行profileであり、World候補経路の差分コストを示す測定。接触・fluid・local grid・jointは含まず、B03全負荷の合格判定ではない。
- `cargo clippy --locked --lib --test rigid_bodies --jobs 1 -- -D warnings`：今回のcustom thermal/fluid contact/occupancy provider実装を含め警告なし。
- `RUSTDOCFLAGS=-D warnings cargo doc --locked --lib --no-deps --jobs 1`：日本語公開API文書を含め警告なしで生成。
- `cargo fmt --all -- --check`、`git diff --check`：custom thermal/fluid contact/occupancy provider変更後に成功。
- `cargo test --locked --lib world::rigid_bodies::tests --jobs 1`：16件成功。geometry/reference、CCD、snapshot復元、rollback関連を検査。
- `cargo clippy --locked --lib --test rigid_bodies --test world --jobs 1 -- -D warnings`：成功。
- `cargo clippy --locked --lib --test foundation --test rigid_bodies --test rigid_bodies_benchmark --jobs 1 -- -D warnings`：GridChannel共有、Q06 integration、手動profileを含む対象に警告なし。
- ローカルレビュー：Custom contact targetへ大きな`StableKey`を直接格納するとenumが約216 byteとなるため、provider key/versionを固定step scopeに置き、target自身はowner/object/partのみ保持する形に変更。Body/Terrain側のeventサイズを増やさず、custom側速度は`None`、snapshot変換と候補原子公開を維持。
- ローカルレビュー：混合carrier相変更はmixture tokenを変更しない。Content検証がcarrier材料と成分の厳密な有理質量一致を検査し、不一致時は外側WorldEdit候補ごと破棄する。反応部分消費の作者stateは残存sourceにのみ保ち、全消費時は保存済みReactionStateTransfer policyに従い移行または明示破棄する。body/particle出力のContent・schema・channel検証失敗時はWorldEdit全体をrollbackする。sidecar byte上限とContent検証を維持。
- ローカルレビュー：GridChannel候補複製は変更前にchunk配列を共有し、書込側のみ分離配列を確保する。確保に失敗したときは値・non-default count・revisionを変更しない。unit testで共有解除とsource保持を確認した。
- ローカルレビュー：custom thermal providerはcandidate WorldEdit内でgeometryを読み取り専用として供給し、Engineが接触件数、occupied local cell、有限面長、active/resident region、channel/material整合を検証してから標準整数fluxと同一transactionでcommitする。不正local cellによるrollbackを対象testで確認。
- ローカルレビュー：`CouplingProvider::fluid_contacts`はcandidate physicsと開始時Worldを読み取り専用で受け、Engineが件数・body handle・coverage・有限作用点・fluid material/channel・運動量を検証した後、stable順で応答を適用する。既存providerは`None`を返し従来BodyStencil contactを維持する。`fluid_occupancy`は全占有cellを一度preflightし、重複所有とunavailable regionを拒否する。custom occupancyとfluidの重なりがなく、他のcontact/reservoir workもなければWorld候補を作らず、重なりがある場合はそのowned cell一覧を移動・particle/reservoir fallbackへ再利用する。custom fixtureで占有外contactと占有内fluid displacement、duplicate owner rollbackを確認。手動profile（test・unoptimized、Windows/MSVC・Ryzen 7 2700、16 bodies/64 chunks/no fluid・warm-up 5/sample 50）では、標準geometryのP50/P95/P99=337.2/344.9/360.6µs、custom occupancy 256 cellsは640.1/655.9/678.2µs。customは+302.9µs P50、+1 allocation・+4,096B/step、追加peak +4,096B。occupancy前検査のみの差で、fluid処理あり・Release・全60Hz tickの性能値ではない。
- ローカルレビュー：`src/world/rigid_bodies.rs`のmodule説明に「custom shape fluid geometryは未実装」と残り、実装済みの`fluid_contacts`/`fluid_occupancy`および仕様書と食い違っていた。説明を現行契約へ修正し、設計11も作者fluid geometryと一般collision solver置換の境界を明記した。挙動変更はない。
- ローカルレビュー：`fracture_disconnected_local_grid`はordered cell seedから4近傍成分を安定列挙し、occupied-cell anchorを最小componentへ決定的に移す。fragmentのmaterial/sidecar/熱端数を保ち、質量中心・慣性から`v + ω×r`を算出し、pending force/torqueは比率配分後に最終fragmentへ丸め残差を置く。joint stable IDとsolver impulseを維持し、空隙anchorはreportして除去する。公開経路はWorldEdit候補内で実行される。`fracture_splits_local_components_and_conserves_linear_and_angular_momentum`、joint移管/除去test、容量失敗rollback、damage particleization運動量testが各契約に対応し、追加修正は不要とレビューした。
- ローカルレビュー：ReactionRuleはQ05どおり未混合整数token・単一output。部分消費はauthor stateを残るsourceへ保持し、全消費のみ`MigrateToOutput`で出力へ一度移すか明示policyで破棄する。出力Content/schema/channel/capacityが外側transaction内で検査され、失敗時は公開しない。混合reaction・複数output state分配はこの標準契約の不足ではない。
- 負荷見積り：author metadataを移す場合は、確定candidateのsource cellからflags/variation/sidecar payloadを最終出力へ一度だけ複製し、Content検証を行う。処理・scratch量は変更候補とsidecar bytesに比例し、`BodyLocalGridLimits::sidecar_bytes`と`job_bytes`で制限する。author metadataがない通常reactionは移行indexを確保しない。
- 旧単一channel chunk profile（COW変更前）：`body_world_thermal_boundary_profile`をWindows/MSVC、8×8 rotated body、44 contacts、warm-up 5・sample 50で3回実行。runごとのP50/P95/P99は2.5152/2.6729/2.6876ms、2.4847/2.6619/2.7133ms、2.5008/2.5574/2.5665ms。中央値は2.5008/2.6619/2.6876ms、791 allocations・189,288B/step、追加peak 143,875B。HISTORYの前回単発値と条件・コード履歴差があり、直接比較には使わない。profileはWorld編集transactionとboundary fluxを含み、全60Hz tickの測定ではない。
- 同条件multi-chunk profile：同じ8×8 body・44 contactsに16個のthermal channel chunkを配置し、うち15個はbody接触外で未変更の条件で共有前後各3 run。共有前のrun中央値はP50/P95/P99=7.5306/9.3036/10.3513ms、841 allocations・689,312B/step、追加peak 643,107B。共有後は7.2548/8.3256/8.4434ms、827 allocations・197,576B/step、追加peak 151,371B。P50/P95/P99は約3.7/10.5/18.4%減、割当byteは71.3%、追加peakは76.5%減。World編集transaction・channel validation・boundary fluxを含む限定profileで、全60Hz tickや別規模の保証ではない。
- 同条件profile：`body_local_reaction_profile`を32×8 local grid、128 source、256 candidate/cycle、warm-up 5・sample 50で整理前後各3 run。整理前中央値はcycle P50/P95/P99=21.3175/22.2129/22.2866ms、commit 650 alloc・1,198,839B/pass、1,410 alloc・3,154,432B/cycle、追加peak 871,123B。整理後は20.1928/20.8150/22.2975ms、commit 647 alloc・843,511B/pass、1,404 alloc・2,443,776B/cycle、追加peak 515,795B。P50/P95と割当量が改善し、P99は同程度。cycle時間はJob構築+commitを2回含む測定で、単一tickまたは全World処理の時間ではない。

## E07 Release性能証拠

| 計測 | 結果と範囲 |
| --- | --- |
| `world_body_joint_step_profile`、Release、3 run | Windows/MSVC、Ryzen 7 2700、256 dynamic circle・無重力・接触なし、50 samples/run。64 fixed jointなしのrun中央値はP50/P95/P99=296.7/348.8/364.5µs、751.3 allocation・426,680.5B/step、追加peak 478,180B。64 jointありは357.3/440.5/461.4µs、756.4 allocation・498,096.8B/step、追加peak 522,564B。joint追加差は中央値で+20.4/+26.3/+26.6%、+5.1 allocation、+71,416B/step、+44,384B peak。World境界確認と原子候補を含む。body local grid・fluid・terrain接触・粒子は含まず、全World tickや60Hz合格の主張には使わない（2026-10-20） |
| `world_custom_coupling_occupancy_preflight_profile`、Release、3 run | 同じ16 body・64 chunk条件、warm-up 5/sample 50。標準geometryのrun中央値P50/P95/P99=34.5/37.8/39.0µs、117 allocation・109,412.5B/step、追加peak 90,760B。custom 256 occupancy cellは50.3/56.6/66.2µs、118 allocation・113,508.5B/step、追加peak 94,856B。占有preflightの差はP50 +15.8µs、P95 +18.8µs、P99 +27.2µs、+1 allocation・+4,096B/step・+4,096B peak。fluidなしで、実流体応答のprofileではない（2026-10-20） |
| `body_stencil_sweep_profile`、Release、3 run | 16×16 stencil、306 sampled cells、50 samples/run。P50中央値46.7µs、P95 47.2µs、P99 52.3µs、0 allocation・0 bytes/step・0B追加peak。現行単体profileで、旧実装との比較ではない（2026-10-20） |
| `world_collision_query_validation_profile`、Release、3 run | 16 body・64 chunk・128 hits、50 query samples/run。stable orderのrun中央値P50/P95/P99=2.6/2.6/2.6µs、provider orderは1.7/1.8/1.8µs。両経路1 allocation・11,264B/query・11,264B peak。現行単体profileで、旧実装との比較ではない（2026-10-20） |

設計18ではB03の256 body・20万local cell・2万particle・65,536 cell破壊・64 jointの複合負荷をE13性能工程で独立計測します。E07ではQ06で変更したbody fixed-step/candidate、joint、fluid coupling、occupancy preflight、stencil sweep、collision query、body heat/reactionの影響経路を対象測定しました。body thermal chunk COWとlocal reaction差分適用には同条件before/after比較があります。初回実装前の物理経路値は存在しないため、256 body no-contactの`BodyPhysics::step`とWorld候補stepの同条件比較を現行cost referenceとして併記しています。Releaseの16-body fluid連成ではP95/P99 tailが1.1〜1.3ms、最遅sampleは1.63ms以下であり、以前debug profileに出た14〜15ms tailは同じfixtureのRelease実行では再現しません。これらは単一環境の限定workloadで、E13のB03測定、全World tickや60Hz達成を代替しません。

## E07出口の全体検査

| 検査 | 結果 |
| --- | --- |
| `cargo check --locked --jobs 1` | 成功（2026-10-20） |
| `cargo test --locked --jobs 1` | 全unit/integration/doc test targetが成功。手動性能profileは通常suite外のignored testで、上のRelease計測として別に実行（2026-10-20） |
| `cargo clippy --all-targets --locked --jobs 1 -- -D warnings` | 初回にGridChannel copy-on-write unit testの配置警告を検出。test moduleを公開impl群の後へ移し、全target再実行が成功（2026-10-20） |
| `cargo test --locked --lib candidate_copy_shares_grid_chunk_until_candidate_write --jobs 1` | test module移動後、候補copy時のchunk共有と初回書込時の分離を確認し1件成功（2026-10-20） |
| `RUSTDOCFLAGS=-D warnings cargo doc --locked --lib --no-deps --jobs 1` | 日本語library API docsを警告なしで生成（2026-10-20） |
| `cargo fmt --all -- --check`、`git diff --check`、`git diff --cached --check` | 成功（2026-10-20。GitのLF/CRLF変換警告のみ） |

## 残り作業と順序

| 順 | 作業単位 | 完了条件 | 状態 |
|---:|---|---|---|
| 1 | Unavailable境界とlease契約 | resident外へのbody候補stepが原子的に拒否されること、caller leaseの確保/解放とsave/restore復帰が受け入れtestでつながることを確認 | 完了：Unavailable拒否、事前lease境界越え、lease中の退避拒否、World/region復帰を個別・結合testで確認 |
| 2 | sidecar・mixture・phase移行 | terrain↔body↔particleの仕様対象変換で、対応可能なsidecar/有理token/phaseを保全し、未対応変換は全体rollbackする | 完了：terrain↔body↔particleのsidecar移送、rational mixture、mass-preserving phase change、reaction partial/full consumptionの保持・移行・明示破棄を対象testと114件targetで確認。標準ReactionRuleが未混合整数token・単一outputであることに従い、混合reactionと複数outputへの作者state分配はQ06出口要件に追加しない |
| 3 | custom shape/contactと標準fluid/heat連成 | provider owner shape、one-way、開始重なり、TOI/normal/impulse、geometry revisionと候補commitを対応づける。標準fluid/heat連成で任意shapeの面coverageをproviderから置換できること | 完了：query ordering/truncate/initial-overlap/one-way/revision、custom candidate sweep、custom thermal contact、fluid contact/occupancy、重複cell owner・不正coverage rollbackを対象testとlocal reviewで確認 |
| 4 | Q06出口レビュー | 上記残件を閉じた状態でQ06 acceptance matrixを再照合し、狭い関連suiteと一回のローカルレビューを記録 | 完了。sensor intersection漏れのP1をPR後レビューで検出し、118件の剛体suite、shape unit test、Clippy、rustdoc、format、差分検査を通過して修正済み。設計11/18の受け入れ行と各統合証拠を照合。E07性能出口は独立した作業単位5で追跡する |
| 5 | E07性能出口 | 影響workloadで同一条件profile/比較を行い、P50/P95/P99・allocation/bytes・tailを記録し、対象workloadの限界を判定 | 完了：body reaction 128 sourceと16 thermal chunk COWの同条件before/afterを記録。Releaseで256 body no-contactと64 joint有無を比較し、16 body fluid row、custom occupancy preflight、stencil sweep、collision queryを測定。profileごとの範囲・割当量・tail・限定事項を上節へ記録した。初回実装前の物理値がないためWorld候補と再利用double-bufferの現行cost比較を示し、全World tickや60Hz達成とは主張しない。B03全負荷は設計18どおりE13で計測する |
| 6 | Q06基準実装との並列一致 | 設計11/18のQ06候補処理について、worker数または実行順を変えた並列候補評価とscalar/reference経路の正規World状態を比較する | 完了：World所有候補と再利用double-buffer基準のbody state/event/step番号が8 tick一致。worker数1/2/4・遅延順変更時の候補結果も一致。fluid/heat/particle conservation、破砕linear/angular momentum、snapshot継続は各Q06 acceptance testで個別・統合検証済み。設計にない別scalar rigid-body solverは追加条件としない |
| 7 | E07工程出口 | Q06の全出口条件後、E07対象検査・総合レビューを一度だけ実施し、採用PRと依存順を証拠付きで確定 | 完了：PR #25をmainへmerge commit `e7384ed` としてマージ。sensor P1修正後に剛体統合118件、event shape unit 1件、Clippy、rustdoc、format、差分検査を再確認。PR #24は重複README差分としてクローズ。採用PRは#25の1件 |

**進捗数え方：** 7作業単位のうち完了7、部分完了0、未完了/未着手0です。混合phase 2件、反応8件、GridChannel unit 1件、ReactionJob unit 18件、BodyPhysics unit 16件、foundation 20件、World 51件の過去実行記録に加え、現行全体`cargo test`も成功しました。Q06受け入れ、並列/reference比較、性能出口、E07総合reviewと[PR #25](https://github.com/sofia-gros/charon-engine/pull/25)のマージを完了しました。E13のB03負荷profileは後続工程です。

## 開発効率の運用

- 完了済み個別testを工程完了へ読み替えない。未達は上表の7作業単位で追跡する。
- 各小変更は最狭の関連testと差分レビューで閉じ、既に成功したQ06検証を反復しない。
- 作業前に上のゲートと設計条件を対応付け、追加APIは契約上の不足が実証された場合だけ作る。
- 新しい証拠は該当行へ追記し、長い日次ログをREADMEへ積み上げない。履歴を参照する場合は、記録時点の根拠としてだけ使う。
