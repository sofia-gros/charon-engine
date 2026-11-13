//! 仕様 13: 通信。`Lockstep` (仕様 07 §7・10 §5) の上に、TCP (標準ライブラリだけ) で入力を配る。
//!
//! 形は**ホスト中継**: 参加者は全員が同じ世界を持ち、毎 tick の自分の入力をホストへ送る。ホストは、その tick の
//! 全員の入力が揃ったら**束** (`Bundle`) にして全員へ配る。全員が同じ束を `Lockstep::submit` して `advance` するので、
//! 世界は同じに進む (束の中身と順序は、ID の昇順で決まる)。接続の最初に、`ConnectionInfo` (プロトコル・材質・MOD・
//! 定義・seed・ビルド・feature・プラットフォーム) を照合して、違う相手は拒否する。ソケットは非ブロッキングで、
//! `poll` をフレームごとに呼ぶ。

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};

use crate::EntityId;
use crate::lockstep::{ConnectionInfo, TICK_INPUT_BYTES, TickInput};
use crate::mods::ModManifest;

/// 1 メッセージの大きさの上限 (バイト)。これより大きいものは、壊れているか悪意があるものとして接続を切る。
pub const MAX_MESSAGE: usize = 1 << 20;
/// 参加者の上限。
pub const MAX_PLAYERS: usize = 16;
/// 先読みしてためておく tick の上限 (これより先の入力は拒否する)。
pub const MAX_AHEAD: u64 = 600;

const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const REJECT: u8 = 3;
const INPUT: u8 = 4;
const BUNDLE: u8 = 5;
const START: u8 = 6;

#[derive(Debug)]
pub enum NetError {
    Io(io::Error),
    /// メッセージの形が正しくない。
    Protocol(&'static str),
    /// 接続情報が違う (相手を拒否した、または拒否された)。
    Rejected(String),
    /// 相手が切れた。
    Disconnected,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "通信に失敗: {e}"),
            Self::Protocol(why) => write!(f, "通信の形が正しくありません: {why}"),
            Self::Rejected(why) => write!(f, "接続を拒否しました: {why}"),
            Self::Disconnected => write!(f, "相手が切れました"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// ---- 符号化 ---------------------------------------------------------------

fn put_str(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let length = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
    out.extend(length.to_le_bytes());
    out.extend(&bytes[..usize::from(length)]);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NetError> {
        if self.0.len() < n {
            return Err(NetError::Protocol("途中で切れている"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn u16(&mut self) -> Result<u16, NetError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, NetError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, NetError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().map_err(|_| NetError::Protocol("長さ"))?))
    }

    fn string(&mut self) -> Result<String, NetError> {
        let length = usize::from(self.u16()?);
        String::from_utf8(self.take(length)?.to_vec()).map_err(|_| NetError::Protocol("文字列"))
    }
}

/// `ConnectionInfo` をバイト列にする (照合のために、内容を全部送る)。
pub fn encode_info(info: &ConnectionInfo) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(info.version.to_le_bytes());
    out.extend(info.definitions.to_le_bytes());
    out.extend(info.seed.to_le_bytes());
    out.extend(u32::try_from(info.materials.len()).unwrap_or(u32::MAX).to_le_bytes());
    for name in &info.materials {
        put_str(&mut out, name);
    }
    out.extend(u32::try_from(info.mods.len()).unwrap_or(u32::MAX).to_le_bytes());
    for m in &info.mods {
        put_str(&mut out, &m.id);
        out.extend(m.version.to_le_bytes());
    }
    put_str(&mut out, &info.binary);
    put_str(&mut out, &info.features);
    put_str(&mut out, &info.platform);
    out
}

pub fn decode_info(bytes: &[u8]) -> Result<ConnectionInfo, NetError> {
    let mut r = Reader(bytes);
    let version = r.u16()?;
    let definitions = r.u64()?;
    let seed = r.u64()?;
    let count = r.u32()? as usize;
    if count > 4096 {
        return Err(NetError::Protocol("材質が多すぎる"));
    }
    let materials = (0..count).map(|_| r.string()).collect::<Result<Vec<_>, _>>()?;
    let count = r.u32()? as usize;
    if count > 1024 {
        return Err(NetError::Protocol("MOD が多すぎる"));
    }
    let mods = (0..count)
        .map(|_| Ok(ModManifest { id: r.string()?, version: r.u32()? }))
        .collect::<Result<Vec<_>, NetError>>()?;
    let binary = r.string()?;
    let features = r.string()?;
    let platform = r.string()?;
    if !r.0.is_empty() {
        return Err(NetError::Protocol("余りがある"));
    }
    Ok(ConnectionInfo { version, materials, mods, definitions, seed, binary, features, platform })
}

fn encode_bundle(tick: u64, inputs: &[TickInput]) -> Vec<u8> {
    let mut out = tick.to_le_bytes().to_vec();
    out.push(u8::try_from(inputs.len()).unwrap_or(u8::MAX));
    for input in inputs {
        out.extend(input.encode());
    }
    out
}

fn decode_bundle(bytes: &[u8]) -> Result<(u64, Vec<TickInput>), NetError> {
    let mut r = Reader(bytes);
    let tick = r.u64()?;
    let count = usize::from(r.take(1)?[0]);
    if count > MAX_PLAYERS {
        return Err(NetError::Protocol("束の人数"));
    }
    let mut inputs = Vec::with_capacity(count);
    for _ in 0..count {
        let packet = r.take(TICK_INPUT_BYTES)?;
        let input = TickInput::decode(packet).map_err(|_| NetError::Protocol("入力"))?;
        if input.tick != tick {
            return Err(NetError::Protocol("束の中の tick が違う"));
        }
        inputs.push(input);
    }
    if !r.0.is_empty() {
        return Err(NetError::Protocol("余りがある"));
    }
    Ok((tick, inputs))
}

// ---- 接続 1 本 (メッセージの切り出し) ---------------------------------------

#[derive(Debug)]
struct Link {
    stream: TcpStream,
    read: Vec<u8>,
    write: Vec<u8>,
    closed: bool,
}

impl Link {
    fn new(stream: TcpStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        stream.set_nodelay(true)?;
        Ok(Self { stream, read: Vec::new(), write: Vec::new(), closed: false })
    }

    fn send(&mut self, kind: u8, payload: &[u8]) {
        let length = u32::try_from(payload.len() + 1).unwrap_or(u32::MAX);
        self.write.extend(length.to_le_bytes());
        self.write.push(kind);
        self.write.extend(payload);
    }

    /// 書けるだけ書き、読めるだけ読んで、揃ったメッセージ `(種類, 中身)` を返す。
    fn pump(&mut self) -> Result<Vec<(u8, Vec<u8>)>, NetError> {
        while !self.write.is_empty() {
            match self.stream.write(&self.write) {
                Ok(0) => break,
                Ok(n) => {
                    self.write.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.closed = true;
                    break;
                }
                Ok(n) => self.read.extend(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let mut messages = Vec::new();
        loop {
            if self.read.len() < 4 {
                break;
            }
            let length =
                u32::from_le_bytes([self.read[0], self.read[1], self.read[2], self.read[3]])
                    as usize;
            if length == 0 || length > MAX_MESSAGE {
                return Err(NetError::Protocol("メッセージの長さ"));
            }
            if self.read.len() < 4 + length {
                break;
            }
            let kind = self.read[4];
            let payload = self.read[5..4 + length].to_vec();
            self.read.drain(..4 + length);
            messages.push((kind, payload));
        }
        Ok(messages)
    }
}

// ---- ホスト -----------------------------------------------------------------

/// ホストから見た出来事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostEvent {
    /// `slot` (0 はホスト自身) の参加者が接続した。
    Joined(usize),
    /// 照合に失敗した相手を拒否した。
    Refused(String),
    /// `slot` の参加者が切れた。
    Left(usize),
}

#[derive(Debug)]
struct Peer {
    link: Link,
    /// 照合が済んで参加者の席 (1 以上) を持っているか。
    slot: Option<usize>,
}

/// 入力を集めて配るホスト。`players[0]` がホスト自身、`players[1..]` が接続してくる順の参加者。
#[derive(Debug)]
pub struct Host {
    listener: TcpListener,
    peers: Vec<Peer>,
    players: Vec<EntityId>,
    info: ConnectionInfo,
    started: bool,
    inputs: BTreeMap<u64, BTreeMap<EntityId, TickInput>>,
    next_tick: u64,
}

impl Host {
    /// `addr` で待ち受ける (`127.0.0.1:0` なら空いているポート)。`players` は参加者の全員 (ホスト自身を先頭に)、
    /// `start_tick` は最初に進める tick (`World::tick`)。
    pub fn listen(
        addr: impl ToSocketAddrs,
        players: Vec<EntityId>,
        info: ConnectionInfo,
        start_tick: u64,
    ) -> Result<Self, NetError> {
        if players.is_empty() || players.len() > MAX_PLAYERS {
            return Err(NetError::Protocol("参加者の数"));
        }
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            peers: Vec::new(),
            players,
            info,
            started: false,
            inputs: BTreeMap::new(),
            next_tick: start_tick,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// 全員が接続して、開始の合図を送ったか。
    pub fn started(&self) -> bool {
        self.started
    }

    /// 接続を受け付け、メッセージを読む。全員が揃ったら開始の合図を配る。
    pub fn poll(&mut self) -> Result<Vec<HostEvent>, NetError> {
        let mut events = Vec::new();
        while let Ok((stream, _)) = self.listener.accept() {
            self.peers.push(Peer { link: Link::new(stream)?, slot: None });
        }
        let mut dropped = Vec::new();
        for index in 0..self.peers.len() {
            let result = self.peers[index].link.pump();
            let messages = match result {
                Ok(messages) => messages,
                Err(_) => {
                    dropped.push(index);
                    continue;
                }
            };
            for (kind, payload) in messages {
                match kind {
                    HELLO if self.peers[index].slot.is_none() => {
                        self.on_hello(index, &payload, &mut events);
                    }
                    INPUT => {
                        if self.peers[index].slot.is_some() {
                            let accepted = self.on_input(index, &payload);
                            if !accepted {
                                dropped.push(index);
                            }
                        }
                    }
                    _ => dropped.push(index),
                }
            }
            if self.peers[index].link.closed && !dropped.contains(&index) {
                dropped.push(index);
            }
        }
        dropped.sort_unstable();
        dropped.dedup();
        for index in dropped.into_iter().rev() {
            let peer = self.peers.remove(index);
            if let Some(slot) = peer.slot {
                events.push(HostEvent::Left(slot));
            }
        }
        let joined = self.peers.iter().filter(|p| p.slot.is_some()).count() + 1;
        if !self.started && joined == self.players.len() {
            self.started = true;
            for peer in &mut self.peers {
                peer.link.send(START, &[]);
            }
        }
        for peer in &mut self.peers {
            // 送りたい分を送る (読み込みは上で済んでいる)。
            let _ = peer.link.pump();
        }
        Ok(events)
    }

    fn on_hello(&mut self, index: usize, payload: &[u8], events: &mut Vec<HostEvent>) {
        let verdict = decode_info(payload).and_then(|peer| {
            if peer == self.info {
                Ok(())
            } else {
                Err(NetError::Rejected("接続情報が一致しません".to_owned()))
            }
        });
        let taken: Vec<usize> = self.peers.iter().filter_map(|p| p.slot).collect();
        let free = (1..self.players.len()).find(|slot| !taken.contains(slot));
        match (verdict, free) {
            (Ok(()), Some(slot)) => {
                self.peers[index].slot = Some(slot);
                let mut welcome = vec![u8::try_from(slot).unwrap_or(0)];
                welcome.push(u8::try_from(self.players.len()).unwrap_or(0));
                welcome.extend(self.next_tick.to_le_bytes());
                self.peers[index].link.send(WELCOME, &welcome);
                events.push(HostEvent::Joined(slot));
            }
            (verdict, _) => {
                let reason = match verdict {
                    Err(e) => e.to_string(),
                    Ok(()) => "席がありません".to_owned(),
                };
                self.peers[index].link.send(REJECT, reason.as_bytes());
                events.push(HostEvent::Refused(reason));
            }
        }
    }

    /// 参加者から届いた入力を受け取る。本人の入力か・tick が範囲内かを調べる。受け取れなければ `false` (接続を切る)。
    fn on_input(&mut self, index: usize, payload: &[u8]) -> bool {
        let Ok(input) = TickInput::decode(payload) else { return false };
        let Some(slot) = self.peers[index].slot else { return false };
        if self.players.get(slot) != Some(&input.player) {
            return false; // 別の人のふりをした。
        }
        self.store(input)
    }

    fn store(&mut self, input: TickInput) -> bool {
        if input.tick < self.next_tick || input.tick > self.next_tick + MAX_AHEAD {
            return false;
        }
        let slot = self.inputs.entry(input.tick).or_default();
        if slot.contains_key(&input.player) {
            return false; // 同じ tick の入力が 2 度。
        }
        slot.insert(input.player, input);
        true
    }

    /// ホスト自身の入力を出す。
    pub fn submit_local(&mut self, input: TickInput) -> Result<(), NetError> {
        if input.player != self.players[0] {
            return Err(NetError::Protocol("ホストの入力ではない"));
        }
        if self.store(input) { Ok(()) } else { Err(NetError::Protocol("入力の tick")) }
    }

    /// 次の tick の全員の入力が揃っていれば、全員 (ID の昇順) の束を返し、接続している参加者へ配る。
    pub fn take_bundle(&mut self) -> Option<Vec<TickInput>> {
        if !self.started {
            return None;
        }
        let tick = self.next_tick;
        let have = self.inputs.get(&tick)?;
        if have.len() != self.players.len() {
            return None;
        }
        let bundle: Vec<TickInput> = have.values().copied().collect();
        let message = encode_bundle(tick, &bundle);
        for peer in &mut self.peers {
            if peer.slot.is_some() {
                peer.link.send(BUNDLE, &message);
            }
        }
        self.inputs.remove(&tick);
        self.next_tick += 1;
        for peer in &mut self.peers {
            let _ = peer.link.pump();
        }
        Some(bundle)
    }
}

// ---- クライアント -------------------------------------------------------------

/// ホストへ接続して、入力を送り、束を受け取る参加者。
#[derive(Debug)]
pub struct Client {
    link: Link,
    slot: Option<usize>,
    players: usize,
    start_tick: u64,
    started: bool,
    bundles: VecDeque<(u64, Vec<TickInput>)>,
    next_tick: u64,
}

impl Client {
    /// `addr` のホストへつなぎ、`info` を名乗る (照合の結果は `poll` のあとで `slot`・`rejected` に出る)。
    pub fn connect(addr: impl ToSocketAddrs, info: &ConnectionInfo) -> Result<Self, NetError> {
        let stream = TcpStream::connect(addr)?;
        let mut link = Link::new(stream)?;
        link.send(HELLO, &encode_info(info));
        let _ = link.pump();
        Ok(Self {
            link,
            slot: None,
            players: 0,
            start_tick: 0,
            started: false,
            bundles: VecDeque::new(),
            next_tick: 0,
        })
    }

    /// ホストが決めた席 (1 以上)。まだなら `None`。
    pub fn slot(&self) -> Option<usize> {
        self.slot
    }

    /// 参加者の数と、最初の tick (`Welcome` のあと)。
    pub fn session(&self) -> Option<(usize, u64)> {
        self.slot.map(|_| (self.players, self.start_tick))
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// 読む・書く。拒否されたときは `Err(Rejected)`、切れたときは `Err(Disconnected)`。
    pub fn poll(&mut self) -> Result<(), NetError> {
        let messages = self.link.pump()?;
        for (kind, payload) in messages {
            match kind {
                WELCOME => {
                    let mut r = Reader(&payload);
                    let slot = usize::from(r.take(1)?[0]);
                    self.players = usize::from(r.take(1)?[0]);
                    self.start_tick = r.u64()?;
                    self.next_tick = self.start_tick;
                    self.slot = Some(slot);
                }
                REJECT => {
                    return Err(NetError::Rejected(String::from_utf8_lossy(&payload).into_owned()));
                }
                START => self.started = true,
                BUNDLE => {
                    let (tick, inputs) = decode_bundle(&payload)?;
                    if tick != self.next_tick + self.bundles.len() as u64 {
                        return Err(NetError::Protocol("束の tick が順番どおりでない"));
                    }
                    self.bundles.push_back((tick, inputs));
                }
                _ => return Err(NetError::Protocol("知らないメッセージ")),
            }
        }
        if self.link.closed {
            return Err(NetError::Disconnected);
        }
        Ok(())
    }

    /// 自分の入力をホストへ送る。
    pub fn send_input(&mut self, input: TickInput) {
        self.link.send(INPUT, &input.encode());
        let _ = self.link.pump();
    }

    /// 次の tick の束 (全員の入力、ID の昇順)。まだ届いていなければ `None`。
    pub fn take_bundle(&mut self) -> Option<Vec<TickInput>> {
        let (tick, inputs) = self.bundles.pop_front()?;
        debug_assert_eq!(tick, self.next_tick);
        self.next_tick += 1;
        Some(inputs)
    }
}
