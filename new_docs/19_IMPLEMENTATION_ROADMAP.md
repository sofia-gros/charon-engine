# v1.0.0までの実装工程

## この工程表の使い方

新設計の実装をE00〜E14で管理する。旧M工程の番号や合格は引き継がない。既存コードは再利用候補として要求と照合し、適合するコードと検証だけを新工程の証拠へ採用する。着手・検証範囲は[evidence/E01](evidence/E01/README.md)、[evidence/E02](evidence/E02/README.md)、[evidence/E03](evidence/E03/README.md)へ記録し、実装した基盤と未完了の後工程を区別する。工程全体の通過は各出口の証拠を揃えて判定する。

各Q作業は契約とfixture、実装、対象の単体/結合試験、単体レビュー、修正、再検証の順で進める。E単位の全体試験・総合試験・レビュー・PRは、そのEに属する全Qが完了した後にまとめて行う。重大な未達を残して次の依存工程を完了扱いにしない。作業を小さく分割しても、出口条件を小さく定義し直さない。

### Q作業とE工程の検証・提出手順

E工程内の作業は、該当するQの受け入れ条件を単位として進める。Qの作業ごとに次を行う。

1. Qの固定要件・出口条件と現在の実装証拠を照合し、真の未達だけを実装する。公開APIや進捗が変わる場合は、対応する設計文書・検証記録・ルートREADMEも更新する。
2. 変更箇所に絞った単体テストと、関連機能との結合テストを実行する。
3. 単体レビューを行い、実質的な指摘を修正する。修正後は影響範囲のテストを再実行し、Qの証拠へ結果を記録する。

E工程に属する全Q作業が完了した後、初めてE単位の検証・提出へ進む。

1. E単位のテスト、総合テストおよび必要な工程出口検査をまとめて実行し、結果・性能測定・未達を記録する。
2. E全体のコードレビューを行い、指摘を修正して必要な検査を再実行する。
3. E全体の変更をまとめてPRに提出し、対象範囲、検証結果、残る制約を記載する。PR後のCI・レビュー指摘へ対応し、追加修正があれば再検証・更新・再レビューを行う。
4. Eの固定された出口条件を満たした証拠が揃うまで、工程を完了扱いにしたり、依存する次工程へ進んだりしない。

小さなQ作業ごとにPRを作らない。必要な全体検査を各Qで重複実行せず、Eの全Q完了後に集約する。Eのマージは別途の明示的な指示に従う。工程ごとの完了条件を満たした後は、次工程への着手確認を繰り返し求めない。

設計の変更はP番号で理由、影響要件、データ/API/保存/性能への影響、移行、検証を記録する。Pが採用されたら重なる旧判断より優先し、関連する工程とfixtureを更新する。

## 工程と出口

| 工程 | 実装するもの | 通過に必要な成果 |
| --- | --- | --- |
| E00 要求と移行棚卸し | V1要件、既存コードの再利用判定、採用依存の固定、認定試験の準備 | 各要件へ実装先と試験を割り当て、旧設計を参照しない開発入口を確認 |
| E01 Coreとデータ | ID、schema、component store、codec、容量、エラー、transaction、乱数 | Q01/Q03の基礎。外部入力や容量失敗で部分変更がなく、allocator/状態を往復できる |
| E02 RuntimeとScene | 起動、plugin、設定、Scene lifecycle、scope、input、headless | タイトルSceneからの遷移、pause/resume、scope cleanup、表示Hzと固定更新の分離 |
| E03 資産と基本表示 | VFS、import、cache、PNG/音/font、sprite、camera、render adapter | 同じpackが各認定OSで読み込め、Scene共有・解放・差し替えを検証 |
| E04 世界と保存基盤 | 多layer、material寸法、block個体、chunk、streaming、生成、世代保存 | Q02/Q09基礎。大規模世界を部分生成し、lease・編集・保存・復帰が成立 |
| E05 セルと流体 | 基本挙動、mass交換、圧力、混合、物質粒子、dirty/sleep | Q04と参照比較。source/sink以外でトークンが増減せず、pool不足で消失しない |
| E06 熱と反応 | enthalpy、phase、燃焼・濡れ・腐食、出力予約、独自rule | Q05。断熱の保存、転移往復、反応満容量、保存中断と再開が一致 |
| E07 物体と破壊連成 | backend adapter、terrain shape、CCD、body local grid、reservoir、joint、破砕 | Q06。回転体の流体排除、熱破壊、粒子/地形への変換、backend完全保存 |
| E08 ゲーム共通サービス | controller、item、inventory、craft、combat、effect、AI、配線、環境と進行 | Q07。作者の独自挙動で主要経路を構成し、取引と継続状態を保存できる |
| E09 完成した表現と操作 | light/shadow、液体表現、post effect、animation、UI、IME、locale、audio | Q08。二つの作例の画面と操作を通常APIで構築し、device障害を回復 |
| E10 通信と独自拡張の統合 | server、replication、interest、予測、再接続、独自schema | Q10。4client、同時操作、遠隔更新、packet loss後の再同期が成立 |
| E11 制作ツールとSDK | CLI、inspector、map/prefab、asset pack、migrate/recover、tutorial | Q11/Q12。新規projectから配布までを別の作者・環境で再現 |
| E12 二つの検証作品 | T01〜T12、N01〜N12、共通の独自拡張作品を完成 | V1-01〜V1-32に公開APIで動く証拠があり、内部改変が不要 |
| E13 性能・堅牢化 | B01〜B05、fuzz、soak、fault、最適化、OS認定、migration fixtures | 18の性能・長時間・障害・配布条件を満たし、regression gateを固定 |
| E14 RCと1.0.0 | API/save互換凍結、release manifest、license/SBOM、配布、最終review | 全要件の証拠、P0/P1未解決なし、RC成果物を再検証してtagとartifactを生成 |

P001による追加の出口は以下とする。既存の工程表より弱い条件へ解釈しない。

| 工程 | 完全再現のために追加するもの |
| --- | --- |
| E00 | TargetManifestとFeatureLedger、TC01〜TC30/NC01〜NC26を対象buildの全内容へ展開する手順 |
| E01 | GridChannel、StateStore、provider state、候補tickとrollback、Player/World等のPersistenceScope |
| E02 | service slot、ScheduleProfile、barrier、SameTick/Deferred、clock、PRNG provider |
| E04 | 別解像度のchannel、液量、material remap/epoch、独自geometryと生成への接続 |
| E07 | BodyPhysics/Coupling/CollisionQueriesの置換、exact shape、custom hitbox、substep、teleport |
| E08 | 独自effect evaluator、命中免疫、Signals、全mode/進行の作者実装経路 |
| E09/E10 | render extraction/light/UI置換、独自stateの同期、OwnershipPolicy/Transport、対象人数profile |
| E11 | 各置換経路の日本語資料、独自providerの最小SDK作成手順と失敗処理 |
| E12 | [26](26_FULL_CLONE_ACCEPTANCE.md)の全ledger行を照合し、T/Nの代表経路だけで合格にしない |
| E13/E14 | 原作規模と全contentの負荷profile、一致比較、未対応の基盤能力ゼロ、公開経路の最終review |

依存順は表の順を基本とする。後工程の作者体験を検証するための契約テストや設計例は先に作れるが、未実装を仮の成功値で埋めて前工程の出口を通したことにはしない。

## 各工程で残す記録

`new_docs/evidence/E番号/`に、対象source revision、要件ID、変更点、実行コマンド、終了結果、profile、fixture、review finding、修正、残る制約を記録する。実行していないチェックは未実行と表示する。

commit後にreviewで重大な問題が出た場合は、修正・再検証・追加commitを行う。saveやAPIに変更があれば、利用例とmigration fixtureも同じ単位で更新する。

## 既存コードの扱い

E00で各moduleを、継続利用、adapterで分離、再実装、作例へ移動のいずれかに分類する。既存のPlayerやGameSessionが特定ゲームを前提にする場合は、汎用サービスと作例の内容へ分ける。既存のsave版をv1のschemaと混同しない。

実装を丸ごと破棄することも、既存動作を守るために新要件を削ることも、既定の方針にはしない。要求・責務・検証から判断する。

## RCの凍結

RC開始時に公開API、save schema、sim profile、認定依存とtoolchainを固定する。修正がこれらを変更する場合は、影響を記録して該当する試験をやり直す。RC中に未実装の主要機能を既知制約へ移して出荷しない。
