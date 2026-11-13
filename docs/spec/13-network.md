# 仕様 13: 通信 (M15)

`Lockstep` (仕様 07 §7・10 §5。入力 30 バイト、全員の入力が揃うまで進まない、毎 tick の同期ハッシュ) の上に、TCP で入力を配る層。標準ライブラリだけ (`std::net`) で、依存クレートなし。`charon-game::net`。

## 1. 形: ホスト中継

- 参加者は全員が**同じ初期状態の世界**を持つ。毎 tick、自分の入力 (`TickInput`) をホストへ送る。ホストは、その tick の全員の入力が揃ったら**束** (`Bundle`) にして全員へ配る。全員が同じ束を `Lockstep::submit` して `advance` するので、世界は同じに進む (束は参加者 ID の昇順)。
- `players[0]` がホスト自身、`players[1..]` が接続してくる順の参加者 (席 1 以上)。全員が接続して照合が済んだら、ホストが**開始の合図**を配る。ホストが開始の合図を出すまで、束は作らない。
- ソケットは非ブロッキング。`Host::poll`・`Client::poll` をフレームごとに呼ぶ (読み書きとメッセージの切り出し)。

## 2. メッセージ (リトルエンディアン、`u32` の長さ (種類 1 バイト + 中身) のあとに種類と中身)

| 種類 | 向き | 中身 |
|---|---|---|
| 1 Hello | クライアント → ホスト | `ConnectionInfo` の全項目 (プロトコル・定義のハッシュ・seed・材質名・MOD・ビルド・feature・プラットフォーム) |
| 2 Welcome | ホスト → クライアント | 席 (1 バイト)・参加者の数 (1 バイト)・最初の tick (8 バイト) |
| 3 Reject | ホスト → クライアント | 理由 (UTF-8) |
| 4 Input | クライアント → ホスト | `TickInput` (30 バイト) |
| 5 Bundle | ホスト → クライアント | tick (8 バイト)・人数 (1 バイト)・`TickInput` × 人数 |
| 6 Start | ホスト → クライアント | なし |

- **照合**: ホストは、Hello の `ConnectionInfo` が自分のものと**完全に一致**しなければ Reject を返す (`HostEvent::Refused`)。席が全部埋まっていても Reject。
- **安全**: 1 メッセージは `MAX_MESSAGE` (1 MiB) まで、長さ 0・上限超過・知らない種類・形が壊れたものは**その接続を切る** (ホストは落ちず、待ち受けを続ける)。`ConnectionInfo` の材質・MOD の個数は上限 (4096・1024) を超えたら拒否 (個数だけで巨大なメモリを確保しない)。
- **入力の検査** (ホスト): 席の本人の ID の入力だけを受け取る (他人のふり → 切る)。tick は「次に進める tick 以上、`MAX_AHEAD` (600) 以内」。同じ tick・同じ参加者の 2 度目は切る。`Bundle` の tick は、クライアントが順番どおり (連続) のものだけ受け取り、中の入力の tick も束の tick と一致していること。

## 3. API

`Host::listen(addr, players, info, start_tick)`・`local_addr`・`poll() -> Vec<HostEvent>` (`Joined(席)`・`Refused(理由)`・`Left(席)`)・`started`・`submit_local(TickInput)`・`take_bundle() -> Option<Vec<TickInput>>`。`Client::connect(addr, &info)`・`poll() -> Result<(), NetError>` (`Rejected`・`Disconnected`)・`slot`・`session() -> Option<(人数, 最初の tick)>`・`started`・`send_input`・`take_bundle`。`encode_info`・`decode_info`。

## 4. テスト

| テスト | 確認 |
|---|---|
| `two_peers_stay_in_sync_over_tcp` | 別々の 2 つの世界 (seed 1〜3) が、TCP で配られた同じ束で、90 tick の間、毎 tick 同じ同期ハッシュになる。束は ID の昇順で、ホストとクライアントで同じ |
| `mismatched_peers_are_refused` | seed・ビルドが違う相手は断られる (`Refused`・`Rejected`) |
| `bad_inputs_drop_the_connection` | 他人のふり・同じ tick の 2 度目・先すぎる tick は接続を切られる。ホスト自身の入力の検査 |
| `malformed_frames_are_dropped` | 大きすぎる・長さ 0・知らない種類のメッセージは接続を切り、ホストは続けて正しい相手を受け付ける |
| `connection_info_roundtrips_and_rejects_garbage` | 接続情報の往復、切れた・余りのある・巨大な個数のものを拒否 |

## 5. 未実装

- **アプリ (`charon-app`) への組み込み** (メニューの `Host`・`Join`、ネットワーク越しのプレイヤーの生成・入力の遅延表示)。今はエンジンの層とテストだけ。
- 切断からの復帰・途中参加 (世界の送信)、遅延の補正・入力の先送り (入力遅延)、暗号化・認証、NAT 越え・マッチング、UDP。
- 同期のずれ (ハッシュの不一致) をホストへ報告して検出する仕組み (今は各自が `advance` の結果を比べる)。
