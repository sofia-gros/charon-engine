# 完全再現を妨げない拡張・衝突・実行契約

## P001：完全クローンを制作できる基盤

2026-10-15のユーザー指示を採用する。06のV1-31/V1-32を具体化し、標準solver、標準順序、標準codecを作者の表現能力の上限にしない。本書の契約は、07/10/11/12/15/16/22/23にある標準実装の固定的な説明より優先する。エンジン本体のfork、private API、作例だけの権限を必要とする経路は不合格とする。

「作者がRustで書ける」は、必要な入力、書込権限、実行位置、保存、描画、通信の接続が公開されて初めて根拠になる。本書はそれらを実装対象として定義する。ゲーム固有の規則は作者projectまたは追加crateへ置く。

## 拡張の段階

| 段階 | 公開する能力 | 選ぶ場面 |
| --- | --- | --- |
| 定義 | builder、schema、prefab、既定値の上書き | 標準処理で同じ挙動を作れる |
| 方針 | 型付きfilter、条件、数式、順序、clock、乱数provider | 処理の一部だけを変える |
| システム | 読書集合を持つSystem、batch、TransactionView | 独自のゲーム規則や計算 |
| サービス | 名前付きservice slotのprovider置換 | 衝突、物理、素材、配線等の標準アルゴリズムでは一致しない |
| 独自データ領域 | 型付きGridChannel、StateStore、独自空間索引 | 標準cellやEntityの構造では表せない |

同一slotに二つの排他的providerを登録したら起動時エラーとする。登録順で勝手に片方を選ばない。before/after、競合時のmerge、観測だけの拡張は区別する。各providerはkey/version、必要サービス、正規状態schema、能力、容量見積もり、保存・移行、再現profileを宣言する。

## 公開service slot

| slot | 標準provider | 置換providerが受け取り、返すもの |
| --- | --- | --- |
| `CollisionQueries` | cell/mask/convexのquery | 不変GeometryView、query/filter → 候補、接触、時刻、coverage |
| `BodyPhysics` | Rapier f64 adapter | 前のPhysicsState、geometry、力、dt → 候補状態、接触、占有と排除要求 |
| `CharacterMotion` | sweep/slide controller | 入力、状態、衝突query → 次の移動状態とcommand |
| `MaterialTransport` | 粉体・液体・気体の参照solver | channel view、neighbor、profile → 書込候補と台帳 |
| `ThermalReactions` | enthalpy/接触反応 | energy/物質/独自状態 → flux/変換候補 |
| `HitResolution` | 攻撃・免疫・damage処理 | hit候補、属性、履歴 → damage/状態変更 |
| `EffectExecution` | EffectProgram | 作者program/state、trigger → 出力と継続state |
| `Signals` | graph/遅延queue | port、graph、signal → 新状態とcommand |
| `Navigation` | region graph/local A* | profile、geometry、探索state → 経路・待機・進捗 |
| `Generation` / `ActivityPolicy` | region生成 / observerとlease | seed/region/履歴 → 候補chunk / 活動要求 |
| `RenderExtraction` / `Lighting` | 標準snapshot / 格子照明 | Worldの型付きView → 描画payload / light field |
| `InputPolicy` / `AudioMix` | action map / 標準mixer | timestamp付きdevice event / 音源・bus・制御 → 入力state / 出力buffer |
| `Replication` / `Transport` | server権威 / QUIC adapter | 検証済みcommandとstate / bounded bytes → 複製・受信候補 |

標準providerを無効化した対象へ、隠れた重力、摩擦、衝突、死亡、timer、despawnを適用しない。同じ個体を二つのBodyPhysicsが動かさない。複数の物理領域を同じWorldに置く場合は所属を宣言し、相互作用を明示bridgeで接続する。

providerの具体的なRust traitは、次の意味を満たす型として公開する。`prepare(&mut self, &ReadContext, &mut CandidateContext) -> StepStatus`、`snapshot/restore`、`validate`、`estimate`を持つ。戻り値はComplete、ContinueSameTick、WaitingForRegion、CapacityExceeded、Failedを区別する。ここでのselfの変更も候補状態に限定し、失敗した候補を正規状態へ漏らさない。保存が必要なselfを非公開のglobalへ隠さない。

登録後のlifecycleは、依存解決、Worldごとのstate作成、各候補stepのprepare/resume、validate、失敗しないcommitまたはdiscard、snapshot、restore、終了の順とする。World stateは型登録したstoreへ置き、同じproviderを複数Worldへ使っても状態を共有しない。能力が不足する組合せはWorld作成前に診断する。参照するgeometry、複製、描画extractorのbridgeが欠けたままproviderを有効にしない。

## 読書権限と候補状態

作者Systemはcomponentだけでなく登録済みresource、GridChannel、StateStoreへアクセスできる。読取Viewは最後のcommit、TransactionViewはその候補と先行barrierの確定結果を読む。書込は宣言した型・regionへの`WriteBatch<T>`または候補storeへの一時借用として許可する。

公開batchは隣接chunk、halo、stride、欠損coverageを返す。chunk-local表現のpaletteや内部pointerへ依存させない。channel領域の借用中は再配置・unloadを禁止し、競合する可変借用をschedulerが排除する。cellごとのdynamic callbackだけを高速実装の唯一の手段にしない。

候補の書込が完了したら、参照・所有権・数値・台帳・予約をvalidateし、Worldがcommitする。作者が任意の物質を発行するゲームも、理由付きsource/sinkとして記録できる。物質保存則の標準方針を、錬金術・複製・消滅等のゲーム規則の禁止に使わない。

## 時間、順序、同tickの再入

X0〜X7は標準ScheduleProfileである。作者はphase内へSystemとcommit barrierを挿入し、標準の処理を置換・無効化できる。入力取込、確定Snapshot公開などの境界を維持し、gameplay phaseの相対順序はmanifestへ記録する。循環依存と宣言なしの競合は起動時エラーにする。

二つの実行方針を提供する。

| 方針 | 意味 | 予算超過時 |
| --- | --- | --- |
| Deferred | 次tickの継続でよい処理 | 正規Jobへ保存し次tickで再開 |
| SameTick | 同tickの順序までゲーム仕様である処理 | World tickを進めず候補内のwork queueを継続 |

SameTickは再帰的なRust callback呼出しで実装せず、`logical_tick, barrier, substep, source, sequence`を持つqueueを安定順にdrainする。後続の処理は必要なbarrierの完了を待つ。壁時計の時間切れで残りを次tickに移し、成功扱いにしてはならない。無限ループや予約不能は診断停止または明示された作者の失敗規則となり、完了したtickを捏造しない。

同tickの作業が長い場合、描画とOS eventは前の確定Snapshotで継続できる。内部のcommit barrierは候補tick内の可視性を定める。tick全体の失敗では候補を破棄し、公開済みSnapshotを保つ。実装はcopy-on-writeまたはundo journalと候補backendを使い、すべてのセルを毎tickコピーすることを必須にしない。

投射物の追加更新は、位置、衝突、AI、timerのどれを何回更新するかを`SubstepPolicy`で定義する。時間はTickに加えてsubstep index、分母、clock keyを保存する。World時刻、天候、作物、装置、演出を異なるclock domainへ接続でき、停止・早送り・重力反転を作者が実装できる。

乱数は標準counter providerに加え、作者の状態付きPRNGと複数streamを登録できる。seed、内部state、呼出し順、versionを保存・hashする。処理順や乱数消費に依存する原作の挙動を、標準乱数への置換で同一とみなさない。

## データ表現を追加する契約

標準CellCoreは8byteのまま維持する。作者は`GridChannel<T>`をstable key、schema、座標変換、初期値、resident/active domain、codec、migration、dirty/invalidation、容量と共に登録する。型はu8等の整数、f32/f64、型付きrecord、paletteを選べる。

例えば、建築tileのsolid形状、wall、液体種とu8量、wire、actuator状態を独立channelとして同じ場所に重ねられる。液量0と255、中間量、移動順、濡れ判定を作者solverで保持し、満杯の物質cellだけへ丸めない。material tokenとの変換が必要なときだけbridgeを登録する。

gridの格子間隔とoriginはWorldのcellに対する有理数比として宣言できる。異なる解像度のauthor gridも、coverageとqueryを提供して衝突・描画へ接続する。大きなblock footprintを指定することを、別solverの解像度を強制する仕組みにしない。

StateStoreは非格子のgraph、soft bodyのnode/edge、履歴、独自配列を持てる。作者がf32演算、固定小数点、独自丸めを選んでも、共通Transformへのf64変換はquery/表示用の派生値に限定できる。毎tickその派生値から作者の正規状態へ逆変換しない。再現保証はproviderごとの認定profileで判定する。

## 当たり判定の契約

表示形状、移動を阻む形状、攻撃判定、被攻撃判定、sensor、素材接触、AI視線を別のchannelとする。例えば絵より小さい被弾範囲、壁を抜ける弾、曲線の鞭、長いbeamを同じsprite寸法から推測しない。

標準shapeはpoint、AABB/OBB、circle、capsule、segment/polyline、凸polygon、compound、cell/bit mask。穴のあるmaskは穴を保持する。concaveは正確な分割またはmask queryを使う。作者shapeは保守的なAABB、overlap、ray、必要ならsweepの実装を登録する。sweep非対応なら能力不足を返し、point rayで代用して成功にしない。

queryはWorld/layer/group、owner、ignore集合、one-way方向、trigger対象、開始時重なり、接触境界の含有、epsilon、tick/substep、最新候補を読むかを指定する。結果はHit/NoHit/Incomplete/Unsupportedを区別し、対象ID、shape/部位ID、material、接触点、normal、time of impact、geometry revision、coverageを返す。最初のhit、全hit、安定順、最大件数、truncateの有無を明示する。

broad phaseは全ての真の候補を含む必要がある。独自shapeと巨大なbeamのために、通常のEntity bucketを越えた登録を許可する。queryの範囲外候補を落として「作者が精密判定を書ける」と説明しない。

衝突filterは候補の除外、接触filterはnormal・摩擦・反発等の調整、responseは停止・slide・反射・貫通・独自解決を担当する。これらはWorldを再帰的に変更せず候補かcommandを返す。確定contact eventはenter/stay/exit、pre/post速度、impulse、接触相手、substepを持つ。除去・teleport・unloadによるexitも原因を区別する。

攻撃の免疫は、攻撃別、発射者別、対象別、同一projectile種共有などを作者が選ぶ。貫通回数、再命中間隔、遮蔽、部位共有HP、味方判定をHitResolutionで置き換えられる。既定の`AttackId × target × hit_window`だけへ固定しない。

## 剛体・連成の互換性

Rapierは標準providerであり、同じ係数を与えただけで他エンジンと同じ運動になる保証はない。`World::step_bodies_with_provider`は`BodyPhysicsStepProvider`を固定stepへ接続し、`BodyPhysicsStepContext`から固定dt、step番号、cell寸法、substep上限を渡す。候補BodyPhysics、BodyId順列挙、BodyState/pose/velocity、read-only World queryへアクセスできる。solver固有stateは作者`StateCodec`を使い、provider key/versionと共にBodyPhysics snapshotへ保存する。Engineはcallback後に有限値、resident swept region、標準fluid couplingと原子commitを検証する。fluid displacementがある時だけWorld候補を作り、その他のstepではWorld全体を複製しない。providerは自身のbody/body・terrain collision query/contact responseを実装し、`BodyPhysics::set_provider_contact_events`でstable順の候補eventを登録できる。Engineはeventの値・substep・body ID・resident terrain・capacityを検査し、fixed-step候補と一緒に公開する。

汎用queryは`CollisionQueriesProvider`と`World::query_body_collisions`で独立して差し替えられる。overlap/sweep/ray、layer/group/owner/ignore、one-way、sensor、開始重なり、境界、epsilon、tick/substep、最新候補、first/all、hit上限とstable orderをrequestに含める。任意形状のOverlap/Sweepは、作者形状を保持するprovider key/versionとowner/object/partの参照を渡し、queryごとに形状データを複製しない。結果はHit/NoHit/Incomplete/Unsupportedを区別し、対象、同一World Content内の`MaterialId`、接触点、単位normal、TOI、geometry revision、coverageを返す。custom target ownerはreportのprovider key/version内で安定する数値IDとする。Engineは入力と結果の有限値・範囲・上限、body/terrain/material参照を検証するが、custom broad phaseの完全性とshape fidelityはprovider側で保証する。このquery API単独ではRapierのfixed-step contact solverやcontact eventへ割り込まない。`BodyPhysicsStepProvider`からは`BodyPhysics::query_collisions`を使い、そのcallback内で変更した候補physicsを明示してqueryできる。providerはIncomplete/UnsupportedをHit/NoHitへ読み替えず、自身のresponseを決定する。

作者が置換fixed-stepから確定contact eventを発行する場合、`BodyContactTarget::Custom`でowner/object/partのstable identityを示せる。identityのscopeは現在有効な`BodyPhysicsStepProvider`のkey/versionであるため、targetごとに大きな固定長provider keyを複製しない。custom側にはBody velocityを割り当てず、不正eventは候補stepごとrollbackする。これはRapier標準solverに任意hookを追加する契約ではなく、fixed-step全体を置き換えたproviderがcontact solverとeventを一貫して所有する経路である。

標準の近似colliderと正確なmaskを使う場合、mask queryだけで接触検出を修正したことにしない。細部で異なるnormal/TOIがposeとimpulseへ反映される必要がある。標準providerは正確な占有分割と接触検証を提供し、近似形状を使うprofileは誤差を明示する。完全一致用profileで「1cell未満の誤差なら同じ」としない。

terrain/body/materialの相互作用は、占有・排除、力と反作用、熱、破壊を公開provider経路へ渡す。fluid応答は`CouplingProvider`が扱い、`fluid_contacts`で作者形状の接触cell、coverage、作用点を指定でき、`fluid_occupancy`でBodyStencilを置き換えるWorld占有cellを指定できる。custom body/world熱境界は`BodyThermalBoundaryProvider`が接触coverageを返し、標準のenergy conservation・phase/mass検証・WorldEdit atomic commitへ接続する。providerはshape geometryを決定的に算出し、Engineはlocal cell、面長、active/resident region、channelとmaterial整合、容量を検証する。Engineは占有cellの重複所有を拒否し、fluid移動・particle化・reservoir化をWorldEdit transactionへ組み込む。CollisionQueriesだけを差し替えて流体から不可視になる状態を許さない。one-way、無重力、局所力場、ragdoll、rope/soft body、stick/slime等はbody providerとStateStoreで実装できる。

標準substep幅は`BodyPhysicsSettings::max_substep_displacement_cells`でWorldごとに選べる。`World::teleport_body`は並進・回転を含むParry shape castで経路を検査するか、到着点だけを検査するか、到着時のsolid overlapを許可するかを明示する。失敗時はWorldとphysicsをrollbackし、経路・到着点がresidentであることを要求する。`World::teleport_body_with_provider`はteleport時の移動queryを`BodyTeleportQueryProvider`へ置換してprovider stampをreportする。汎用ray/sweep/query routeは前述の`CollisionQueriesProvider`で提供する。未読込regionのlease/load、body-local物質の移送、任意shapeのphysics contact、時間区間分割は標準APIでは代替せずproviderの責務に残る。標準移動幅やsubstep上限だけで原作の高速挙動を代替したとみなさない。

## 保存・通信・表示への接続

独自providerとchannelの正規状態、queue、PRNG、substep、material remap、探索中、世代参照を保存・hash・replayへ含める。PlayerProfile、WorldSave、AccountProgress、RunStateを別のPersistenceScopeとして登録できる。playerを別Worldへ移す際はcomponent/定義参照を検証し、全関連状態を取引で移す。

広域material変換はMaterialRemapTableとepochを保存し、residentと未読込chunkへ同じ規則を適用できる。定義そのものの破壊的変更とは区別する。見た目、反応、熱、container、body内、粒子のどこへ適用するかを作者policyにする。標準body local gridは`engine:body-local-grid-inventory` ownerとcell別適用epochを使い、mixed componentも同じowner/layer規則で変換する。明示的な移行policyのない空化や表現不能な結果は取引を拒否し、作者ownerは独自stateを自身のcodecで処理する。

描画は登録済みchannelとStateStoreのsnapshot extractor、custom vertex/index buffer、shader、render pass、UI canvasへ接続できる。標準sprite/light/UIで一致しない描画を置換できる。正規状態を必要とする光量・音刺激は表示cacheを逆読取せず、正規Systemとして登録する。

入力はactionへ統合する前のtimestamp付きdevice eventと座標変換を作者policyへ渡せる。edgeの合成、repeat、mouse sampling、同tickの複数eventの扱いを選び、入力記録には採用した方針と結果を残す。音は独自DSP/mix providerとbuffer出力を登録できる。音声threadは準備済みの不変データと制御queueだけを使い、正規Worldを書き換えない。標準の入力平滑化やmixerが原作の操作・音を再現できない場合も置換経路を持つ。

複数PersistenceScopeの同時更新や複数World間のtransferはRuntimeのcommit所有者が全対象をID順に予約し、共通の世代manifestを公開する。関連scopeの片方だけを新世代として採用しない。Worldのtick値が異なる場合はそれぞれの値とtransfer IDを記録し、同じtickと偽装しない。

通信は標準server権威を既定とし、OwnershipPolicy、検証、replication、transportのadapterを公開する。作者は対象ゲームの権限分担と参加人数を構成できる。4client試験は最小fixtureであり上限ではない。外部saveや既存protocolの入出力が必要な場合は、型付きimport/export codecとtransport adapterから接続できる。対応していない既存形式を標準機能として宣伝しない。

## 自由度の検証

標準cell transport、Rapier、ground controller、EffectProgram、wire queue、lighting、replicationを一つずつ無効化したprojectを作る。代替実装が読取、更新、barrier、query、描画、保存、headless、通信へ接続できることをQ01/Q03/Q06/Q07/Q09/Q10/Q12で確認する。標準providerが隠れて実行されないことも検証する。

unsafeなWorld pointerやエンジン内部の変更を使わず、上記の全経路を作れることをV1-31の出口とする。native pluginは信頼された作者コードであり、任意Rust処理の停止や有限時間をエンジンが保証するものではない。動作予算への協調と状態宣言を契約にする。
