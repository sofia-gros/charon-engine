//! 作者Signals providerへ接続する、保存可能な順序付きsignal queueです。
//!
//! queueはtick順、loop重複排除、容量とprovider stampを管理します。配線graph・信号payloadの意味・
//! SameTick/Deferredの伝播規則は`SignalsProvider`が所有し、他の標準規則を強制しません。

use crate::foundation::codec::{Decoder, Encoder, StateCodec};
use crate::foundation::{EngineError, EngineResult, ErrorCode, StableKey};
use std::collections::{BTreeMap, BTreeSet};

type SignalId = (StableKey, StableKey, u64, StableKey);

/// 配線上で受け渡す型付きpayloadの不透明な保存表現です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    /// graphやworld layerを識別する安定key。
    pub network: StableKey,
    /// sequenceを発行したsource。
    pub source: StableKey,
    /// source内で再利用しない単調sequence。
    pub sequence: u64,
    /// 信号を受け取るnode。
    pub target: StableKey,
    /// provider固有codecで検証されるpayload。
    pub payload: Vec<u8>,
}

impl Signal {
    fn id(&self) -> SignalId {
        (self.network, self.source, self.sequence, self.target)
    }
}

/// 次に配送する時間を作者が指定します。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalTiming {
    SameTick,
    Deferred,
}

/// providerが一つのsignalから作る配送先です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalDelivery {
    /// providerが発行する次の信号。
    pub signal: Signal,
    /// SameTickでは同じqueueをpollし、Deferredでは次tickへ保持します。
    pub timing: SignalTiming,
}

/// queueが保持する状態の上限です。config側で管理し、保存データから引き上げません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalLimits {
    /// 1tick内の処理済み・待機中と次tick待ちを合わせたsignal上限。
    pub signals_per_tick: u32,
    /// 現在・次tick queueに保持するpayload合計byte上限。
    pub queued_payload_bytes: u64,
    /// providerが一回に返すfanout上限。
    pub deliveries_per_signal: u32,
    /// 一つのsignal payload上限。
    pub payload_bytes: u32,
}

/// 1回のqueue pollの結果です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalProgress {
    ContinueSameTick,
    TickComplete,
}

/// 作者が登録する装置portの送受信方向です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WirePortDirection {
    Input,
    Output,
    Bidirectional,
}

impl WirePortDirection {
    fn can_send(self) -> bool {
        matches!(self, Self::Output | Self::Bidirectional)
    }

    fn can_receive(self) -> bool {
        matches!(self, Self::Input | Self::Bidirectional)
    }
}

/// World layerの一つのdevice port定義です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WirePort {
    /// graph内で一意なport key。
    pub key: StableKey,
    /// 所有するdeviceのstable key。
    pub device: StableKey,
    /// 更新対象を局所化するWorld region key。
    pub region: StableKey,
    /// このportが受け付けるpayload schema key。
    pub signal_type: StableKey,
    /// 送信、受信、または双方向の契約。
    pub direction: WirePortDirection,
}

/// 配線port間の有向配送経路です。送受信方向に応じて接続時に生成します。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WireConnection {
    pub source: StableKey,
    pub target: StableKey,
}

/// graph登録・局所接続・復元時に呼出側が与える容量です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireGraphLimits {
    pub ports: u32,
    pub directed_connections: u32,
    pub regions: u32,
}

/// portと有向接続を安定順に保持する作者向けwire graphです。
///
/// graph自体はsimulation stateとして保存できます。connectionsはregionごとのrevisionを持つため、
/// author solverは変更のないregionを再構築せずに済みます。graph更新は冷間の登録・編集APIで行い、
/// 信号poll時に全Worldや全graphを走査しません。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireGraph {
    network: StableKey,
    revision: u64,
    ports: Vec<WirePort>,
    connections: Vec<WireConnection>,
    regions: Vec<(StableKey, u64)>,
}

impl WireGraph {
    /// graph networkを指定して空のgraphを作ります。
    pub fn new(network: StableKey, limits: WireGraphLimits) -> Result<Self, SignalError> {
        validate_wire_limits(limits)?;
        Ok(Self {
            network,
            revision: 0,
            ports: Vec::new(),
            connections: Vec::new(),
            regions: Vec::new(),
        })
    }

    /// 読み込み・region再構築用にportと未方向link群からgraphを一括構築します。
    ///
    /// 入力を安定key順に整列して一度だけ各配列へ配置するため、逐次挿入で全体を構築する場合の
    /// 繰り返しshiftを避けます。linkはport方向から有向配送edgeへ変換し、重複と型不一致を拒否します。
    pub fn from_parts(
        network: StableKey,
        mut ports: Vec<WirePort>,
        mut links: Vec<(StableKey, StableKey)>,
        limits: WireGraphLimits,
    ) -> Result<Self, SignalError> {
        validate_wire_limits(limits)?;
        if ports.len() > limits.ports as usize {
            return Err(SignalError::CapacityExceeded);
        }
        ports.sort_unstable_by_key(|port| port.key);
        if ports.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(SignalError::InvalidPort);
        }
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(ports.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        regions.extend(ports.iter().map(|port| port.region));
        regions.sort_unstable();
        regions.dedup();
        if regions.len() > limits.regions as usize {
            return Err(SignalError::CapacityExceeded);
        }
        let mut region_revisions = Vec::new();
        region_revisions
            .try_reserve_exact(regions.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        region_revisions.extend(regions.into_iter().map(|region| (region, 1)));

        links.sort_unstable_by_key(|(first, second)| {
            if first <= second {
                (*first, *second)
            } else {
                (*second, *first)
            }
        });
        if links.windows(2).any(|pair| {
            let (a, b) = pair[0];
            let (c, d) = pair[1];
            (a.min(b), a.max(b)) == (c.min(d), c.max(d))
        }) {
            return Err(SignalError::DuplicateConnection);
        }
        let edge_capacity = links
            .len()
            .checked_mul(2)
            .ok_or(SignalError::CapacityExceeded)?;
        let mut connections = Vec::new();
        connections
            .try_reserve_exact(edge_capacity.min(limits.directed_connections as usize))
            .map_err(|_| SignalError::AllocationFailed)?;
        for (first, second) in links {
            if first == second {
                return Err(SignalError::InvalidPort);
            }
            let first_port = ports
                .binary_search_by_key(&first, |port| port.key)
                .ok()
                .map(|index| ports[index])
                .ok_or(SignalError::InvalidPort)?;
            let second_port = ports
                .binary_search_by_key(&second, |port| port.key)
                .ok()
                .map(|index| ports[index])
                .ok_or(SignalError::InvalidPort)?;
            if first_port.signal_type != second_port.signal_type {
                return Err(SignalError::SignalTypeMismatch);
            }
            let [forward, reverse] = directional_connections(first_port, second_port);
            if forward.is_none() && reverse.is_none() {
                return Err(SignalError::InvalidPort);
            }
            if let Some(connection) = forward {
                if connections.len() >= limits.directed_connections as usize {
                    return Err(SignalError::CapacityExceeded);
                }
                connections
                    .try_reserve(1)
                    .map_err(|_| SignalError::AllocationFailed)?;
                connections.push(connection);
            }
            if let Some(connection) = reverse {
                if connections.len() >= limits.directed_connections as usize {
                    return Err(SignalError::CapacityExceeded);
                }
                connections
                    .try_reserve(1)
                    .map_err(|_| SignalError::AllocationFailed)?;
                connections.push(connection);
            }
        }
        connections.sort_unstable();
        let graph = Self {
            network,
            revision: u64::from(!ports.is_empty() || !connections.is_empty()),
            ports,
            connections,
            regions: region_revisions,
        };
        graph.validate_limits(limits)?;
        Ok(graph)
    }

    /// graphの保存scopeです。
    pub fn network_key(&self) -> StableKey {
        self.network
    }

    /// いずれかのportまたは接続が変わった回数です。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 登録済みportをstable key順に読み取ります。
    pub fn ports(&self) -> &[WirePort] {
        &self.ports
    }

    /// 有向接続をsource、target順に読み取ります。
    pub fn connections(&self) -> &[WireConnection] {
        &self.connections
    }

    /// regionの局所graph revisionを返します。
    pub fn region_revision(&self, region: StableKey) -> Option<u64> {
        self.regions
            .binary_search_by_key(&region, |(key, _)| *key)
            .ok()
            .map(|index| self.regions[index].1)
    }

    /// 指定portから出る接続だけをstable target順で返します。
    pub fn connections_from(&self, source: StableKey) -> &[WireConnection] {
        let start = self
            .connections
            .partition_point(|connection| connection.source < source);
        let end = self
            .connections
            .partition_point(|connection| connection.source <= source);
        &self.connections[start..end]
    }

    /// 出力portから、接続先inputへ運ぶsignal候補を作ります。
    ///
    /// payloadのbyte解釈とSameTick/Deferredの選択は作者providerに残します。複製量はfanoutと
    /// payload byte数に比例し、`delivery_limit`を超えるfanoutは一部だけ返さず容量errorにします。
    pub fn deliveries(
        &self,
        signal: &Signal,
        timing: SignalTiming,
        delivery_limit: u32,
        payload_limit: u32,
    ) -> Result<Vec<SignalDelivery>, SignalError> {
        if signal.network != self.network || signal.payload.len() > payload_limit as usize {
            return Err(SignalError::InvalidSignal);
        }
        let port = self.port(signal.target)?;
        if !port.direction.can_send() {
            return if port.direction.can_receive() {
                Ok(Vec::new())
            } else {
                Err(SignalError::InvalidPort)
            };
        }
        let outgoing = self.connections_from(signal.target);
        if outgoing.len() > delivery_limit as usize {
            return Err(SignalError::CapacityExceeded);
        }
        let mut result = Vec::new();
        result
            .try_reserve_exact(outgoing.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        for connection in outgoing {
            let mut payload = Vec::new();
            payload
                .try_reserve_exact(signal.payload.len())
                .map_err(|_| SignalError::AllocationFailed)?;
            payload.extend_from_slice(&signal.payload);
            result.push(SignalDelivery {
                signal: Signal {
                    network: signal.network,
                    source: signal.source,
                    sequence: signal.sequence,
                    target: connection.target,
                    payload,
                },
                timing,
            });
        }
        Ok(result)
    }

    /// 一つのportを登録し、全体と所属regionのrevisionを進めます。
    pub fn register_port(
        &mut self,
        expected_revision: u64,
        port: WirePort,
        limits: WireGraphLimits,
    ) -> Result<u64, SignalError> {
        validate_wire_limits(limits)?;
        self.check_current_limits(limits)?;
        self.check_revision(expected_revision)?;
        if self
            .ports
            .binary_search_by_key(&port.key, |known| known.key)
            .is_ok()
        {
            return Err(SignalError::InvalidPort);
        }
        if self.ports.len() >= limits.ports as usize {
            return Err(SignalError::CapacityExceeded);
        }
        let next_revision = self.next_revision()?;
        let region_index = self
            .regions
            .binary_search_by_key(&port.region, |(key, _)| *key);
        let region_revision = match region_index {
            Ok(index) => self.regions[index]
                .1
                .checked_add(1)
                .ok_or(SignalError::RevisionOverflow)?,
            Err(_) => {
                if self.regions.len() >= limits.regions as usize {
                    return Err(SignalError::CapacityExceeded);
                }
                self.regions
                    .try_reserve(1)
                    .map_err(|_| SignalError::AllocationFailed)?;
                1
            }
        };
        self.ports
            .try_reserve(1)
            .map_err(|_| SignalError::AllocationFailed)?;
        let port_index = self
            .ports
            .binary_search_by_key(&port.key, |known| known.key)
            .unwrap_err();
        self.ports.insert(port_index, port);
        match region_index {
            Ok(index) => self.regions[index].1 = region_revision,
            Err(index) => self.regions.insert(index, (port.region, region_revision)),
        }
        self.revision = next_revision;
        Ok(self.revision)
    }

    /// 型と方向が合う二つのportを接続し、必要な向きの有向edgeを作ります。
    pub fn connect(
        &mut self,
        expected_revision: u64,
        first: StableKey,
        second: StableKey,
        limits: WireGraphLimits,
    ) -> Result<u64, SignalError> {
        validate_wire_limits(limits)?;
        self.check_current_limits(limits)?;
        self.check_revision(expected_revision)?;
        if first == second {
            return Err(SignalError::InvalidPort);
        }
        let first_port = self.port(first)?;
        let second_port = self.port(second)?;
        if first_port.signal_type != second_port.signal_type {
            return Err(SignalError::SignalTypeMismatch);
        }
        let additions = directional_connections(*first_port, *second_port);
        let addition_count = additions.iter().flatten().count();
        if addition_count == 0 {
            return Err(SignalError::InvalidPort);
        }
        if additions
            .iter()
            .flatten()
            .any(|connection| self.connections.binary_search(connection).is_ok())
        {
            return Err(SignalError::DuplicateConnection);
        }
        if self.connections.len().saturating_add(addition_count)
            > limits.directed_connections as usize
        {
            return Err(SignalError::CapacityExceeded);
        }
        let next_revision = self.next_revision()?;
        let first_region = self.region_next_revision(first_port.region)?;
        let second_region = if first_port.region == second_port.region {
            None
        } else {
            Some(self.region_next_revision(second_port.region)?)
        };
        self.connections
            .try_reserve(addition_count)
            .map_err(|_| SignalError::AllocationFailed)?;
        for connection in additions.into_iter().flatten() {
            let index = self.connections.binary_search(&connection).unwrap_err();
            self.connections.insert(index, connection);
        }
        self.set_region_revision(first_region.0, first_region.1);
        if let Some((region, revision)) = second_region {
            self.set_region_revision(region, revision);
        }
        self.revision = next_revision;
        Ok(self.revision)
    }

    /// 接続を外し、両端regionのrevisionを進めます。
    pub fn disconnect(
        &mut self,
        expected_revision: u64,
        first: StableKey,
        second: StableKey,
    ) -> Result<Option<u64>, SignalError> {
        self.check_revision(expected_revision)?;
        let first_port = self.port(first)?;
        let second_port = self.port(second)?;
        let forward = WireConnection {
            source: first,
            target: second,
        };
        let reverse = WireConnection {
            source: second,
            target: first,
        };
        let connected = self.connections.binary_search(&forward).is_ok()
            || self.connections.binary_search(&reverse).is_ok();
        if !connected {
            return Ok(None);
        }
        let next_revision = self.next_revision()?;
        let first_region = self.region_next_revision(first_port.region)?;
        let second_region = if first_port.region == second_port.region {
            None
        } else {
            Some(self.region_next_revision(second_port.region)?)
        };
        self.connections
            .retain(|edge| !(*edge == forward || *edge == reverse));
        self.set_region_revision(first_region.0, first_region.1);
        if let Some((region, revision)) = second_region {
            self.set_region_revision(region, revision);
        }
        self.revision = next_revision;
        Ok(Some(self.revision))
    }

    /// portをgraphから外し、接続先を含む全ての影響region revisionを進めます。
    pub fn unregister_port(
        &mut self,
        expected_revision: u64,
        key: StableKey,
    ) -> Result<u64, SignalError> {
        self.check_revision(expected_revision)?;
        let port_index = self
            .ports
            .binary_search_by_key(&key, |port| port.key)
            .map_err(|_| SignalError::InvalidPort)?;
        let next_revision = self.next_revision()?;
        let mut affected_regions = Vec::new();
        let affected_edge_count = self
            .connections
            .iter()
            .filter(|edge| edge.source == key || edge.target == key)
            .count();
        affected_regions
            .try_reserve_exact(affected_edge_count.saturating_add(1))
            .map_err(|_| SignalError::AllocationFailed)?;
        affected_regions.push(self.ports[port_index].region);
        for connection in self
            .connections
            .iter()
            .filter(|edge| edge.source == key || edge.target == key)
        {
            let other = if connection.source == key {
                connection.target
            } else {
                connection.source
            };
            let other_region = self.port(other)?.region;
            affected_regions.push(other_region);
        }
        affected_regions.sort_unstable();
        affected_regions.dedup();
        let mut next_regions = Vec::new();
        next_regions
            .try_reserve_exact(affected_regions.len())
            .map_err(|_| SignalError::AllocationFailed)?;
        for region in affected_regions {
            next_regions.push(self.region_next_revision(region)?);
        }

        self.connections
            .retain(|edge| edge.source != key && edge.target != key);
        self.ports.remove(port_index);
        for (region, revision) in next_regions {
            self.set_region_revision(region, revision);
        }
        self.revision = next_revision;
        Ok(self.revision)
    }

    /// 復元したgraphが登録時の容量設定に収まることを確認します。
    pub fn validate_limits(&self, limits: WireGraphLimits) -> Result<(), SignalError> {
        validate_wire_limits(limits)?;
        if self.ports.len() > limits.ports as usize
            || self.connections.len() > limits.directed_connections as usize
            || self.regions.len() > limits.regions as usize
            || (!self.ports.is_empty() && self.revision == 0)
        {
            return Err(SignalError::CapacityExceeded);
        }
        if self.ports.windows(2).any(|pair| pair[0].key >= pair[1].key)
            || self.connections.windows(2).any(|pair| pair[0] >= pair[1])
            || self.regions.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || self.ports.iter().any(|port| {
                self.regions
                    .binary_search_by_key(&port.region, |(region, _)| *region)
                    .is_err()
            })
            || self.connections.iter().any(|connection| {
                let (Ok(source), Ok(target)) = (
                    self.ports
                        .binary_search_by_key(&connection.source, |port| port.key),
                    self.ports
                        .binary_search_by_key(&connection.target, |port| port.key),
                ) else {
                    return true;
                };
                let source = self.ports[source];
                let target = self.ports[target];
                source.signal_type != target.signal_type
                    || !source.direction.can_send()
                    || !target.direction.can_receive()
            })
        {
            return Err(SignalError::InvalidState);
        }
        Ok(())
    }

    fn check_revision(&self, expected: u64) -> Result<(), SignalError> {
        if self.revision == expected {
            Ok(())
        } else {
            Err(SignalError::StaleRevision)
        }
    }

    fn check_current_limits(&self, limits: WireGraphLimits) -> Result<(), SignalError> {
        if self.ports.len() > limits.ports as usize
            || self.connections.len() > limits.directed_connections as usize
            || self.regions.len() > limits.regions as usize
        {
            Err(SignalError::CapacityExceeded)
        } else {
            Ok(())
        }
    }

    fn next_revision(&self) -> Result<u64, SignalError> {
        self.revision
            .checked_add(1)
            .ok_or(SignalError::RevisionOverflow)
    }

    fn port(&self, key: StableKey) -> Result<&WirePort, SignalError> {
        self.ports
            .binary_search_by_key(&key, |port| port.key)
            .map(|index| &self.ports[index])
            .map_err(|_| SignalError::InvalidPort)
    }

    fn region_next_revision(&self, region: StableKey) -> Result<(StableKey, u64), SignalError> {
        let index = self
            .regions
            .binary_search_by_key(&region, |(key, _)| *key)
            .map_err(|_| SignalError::InvalidState)?;
        Ok((
            region,
            self.regions[index]
                .1
                .checked_add(1)
                .ok_or(SignalError::RevisionOverflow)?,
        ))
    }

    fn set_region_revision(&mut self, region: StableKey, revision: u64) {
        let index = self
            .regions
            .binary_search_by_key(&region, |(key, _)| *key)
            .expect("登録済みportのregion revision");
        self.regions[index].1 = revision;
    }
}

fn directional_connections(first: WirePort, second: WirePort) -> [Option<WireConnection>; 2] {
    let first_to_second =
        (first.direction.can_send() && second.direction.can_receive()).then_some(WireConnection {
            source: first.key,
            target: second.key,
        });
    let second_to_first =
        (second.direction.can_send() && first.direction.can_receive()).then_some(WireConnection {
            source: second.key,
            target: first.key,
        });
    [first_to_second, second_to_first]
}

impl StateCodec for WireGraph {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHWIRE01")?;
        self.network.encode(out)?;
        out.u64(self.revision)?;
        out.u32(u32::try_from(self.ports.len()).map_err(|_| invalid_encoding())?)?;
        for port in &self.ports {
            port.key.encode(out)?;
            port.device.encode(out)?;
            port.region.encode(out)?;
            port.signal_type.encode(out)?;
            out.u8(match port.direction {
                WirePortDirection::Input => 0,
                WirePortDirection::Output => 1,
                WirePortDirection::Bidirectional => 2,
            })?;
        }
        out.u32(u32::try_from(self.connections.len()).map_err(|_| invalid_encoding())?)?;
        for connection in &self.connections {
            connection.source.encode(out)?;
            connection.target.encode(out)?;
        }
        out.u32(u32::try_from(self.regions.len()).map_err(|_| invalid_encoding())?)?;
        for (region, revision) in &self.regions {
            region.encode(out)?;
            out.u64(*revision)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHWIRE01" {
            return Err(invalid_encoding());
        }
        let network = StableKey::decode(input)?;
        let revision = input.u64()?;
        let port_count = input.u32()? as usize;
        if port_count > input.remaining() / 29 {
            return Err(invalid_encoding());
        }
        let mut ports = Vec::new();
        ports.try_reserve_exact(port_count).map_err(allocation)?;
        for _ in 0..port_count {
            let port = WirePort {
                key: StableKey::decode(input)?,
                device: StableKey::decode(input)?,
                region: StableKey::decode(input)?,
                signal_type: StableKey::decode(input)?,
                direction: match input.u8()? {
                    0 => WirePortDirection::Input,
                    1 => WirePortDirection::Output,
                    2 => WirePortDirection::Bidirectional,
                    _ => return Err(invalid_encoding()),
                },
            };
            if ports
                .last()
                .is_some_and(|previous: &WirePort| previous.key >= port.key)
            {
                return Err(invalid_encoding());
            }
            ports.push(port);
        }
        let connection_count = input.u32()? as usize;
        if connection_count > input.remaining() / 14 {
            return Err(invalid_encoding());
        }
        let mut connections = Vec::new();
        connections
            .try_reserve_exact(connection_count)
            .map_err(allocation)?;
        for _ in 0..connection_count {
            let connection = WireConnection {
                source: StableKey::decode(input)?,
                target: StableKey::decode(input)?,
            };
            if connections
                .last()
                .is_some_and(|previous| *previous >= connection)
            {
                return Err(invalid_encoding());
            }
            connections.push(connection);
        }
        let region_count = input.u32()? as usize;
        if region_count > input.remaining() / 15 {
            return Err(invalid_encoding());
        }
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(region_count)
            .map_err(allocation)?;
        for _ in 0..region_count {
            let region = (StableKey::decode(input)?, input.u64()?);
            if region.1 == 0
                || regions
                    .last()
                    .is_some_and(|previous: &(StableKey, u64)| previous.0 >= region.0)
            {
                return Err(invalid_encoding());
            }
            regions.push(region);
        }
        let graph = Self {
            network,
            revision,
            ports,
            connections,
            regions,
        };
        graph
            .validate_limits(WireGraphLimits {
                ports: u32::MAX,
                directed_connections: u32::MAX,
                regions: u32::MAX,
            })
            .map_err(|_| invalid_encoding())?;
        Ok(graph)
    }
}

/// 一つのwire device入力で使う作者指定容量です。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireDeviceLimits {
    /// 作者固有device stateの最大byte数です。
    pub state_bytes: u32,
    /// 一入力から作れる出力port emission数です。
    pub emissions_per_input: u32,
    /// graph fanout後にqueueへ渡すsignal総数です。
    pub deliveries_per_input: u32,
    /// 入出力payload一件の最大byte数です。
    pub payload_bytes: u32,
    /// duplicate配送抑止に保存する入力receipt数です。
    pub receipts: u32,
}

/// 作者handlerが作る、特定出力portからのsignal候補です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireEmission {
    /// deviceが所有する送信portです。
    pub port: StableKey,
    /// portのsignal typeに対応するopaque payloadです。
    pub payload: Vec<u8>,
    /// SameTickまたはDeferredの配送方針です。
    pub timing: SignalTiming,
}

/// device入力後の作者状態とport emission候補です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireDeviceResult {
    /// 次回入力から使う作者state codec bytesです。
    pub state: Vec<u8>,
    /// graphを通ってqueueへ向かうordered emissionです。
    pub emissions: Vec<WireEmission>,
}

/// 保存されるdevice状態schemaと入力処理を所有する作者handlerです。
///
/// 実行結果は入力signalとstate bytesだけで決まり、handler内部の可変状態や外部副作用に依存しては
/// いけません。外部更新は返された候補を使い、`WireDeviceCandidate::commit_with`のtransaction内で
/// 行います。これにより失敗時の再試行やcheckpoint復元でも同じ候補を再生成できます。
pub trait WireDeviceHandler {
    /// 保存stateへ紐付ける安定provider keyです。
    fn key(&self) -> StableKey;
    /// state schemaの意味版です。
    fn version(&self) -> u32;
    /// 初期・復元・次state bytesを作者codecで検証します。
    fn validate_state(&self, state: &[u8]) -> Result<(), SignalError>;
    /// immutable input/stateから次状態と出力候補を作ります。
    fn handle(
        &self,
        input_port: &WirePort,
        signal: &Signal,
        state: &[u8],
    ) -> Result<WireDeviceResult, SignalError>;
}

/// 一つのdeviceが所有する作者状態、provider identity、発行sequenceと入力receiptです。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireDevice {
    network: StableKey,
    key: StableKey,
    provider: StableKey,
    provider_version: u32,
    revision: u64,
    next_sequence: u64,
    state: Vec<u8>,
    receipts: Vec<SignalId>,
}

impl WireDevice {
    /// World network/device scopeとprovider stateを固定したdevice componentを作ります。
    pub fn new(
        network: StableKey,
        key: StableKey,
        handler: &impl WireDeviceHandler,
        initial_state: Vec<u8>,
        limits: WireDeviceLimits,
    ) -> Result<Self, SignalError> {
        validate_wire_device_limits(limits)?;
        validate_wire_device_handler(handler, &initial_state, limits)?;
        Ok(Self {
            network,
            key,
            provider: handler.key(),
            provider_version: handler.version(),
            revision: 0,
            next_sequence: 0,
            state: initial_state,
            receipts: Vec::new(),
        })
    }

    /// componentのstable keyです。
    pub fn key(&self) -> StableKey {
        self.key
    }

    /// 保存済み作者stateのrevisionです。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 次の出力emissionに割り当てるsequenceです。
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// handler identity、state codec、scopeと保存上限を照合します。
    pub fn validate(
        &self,
        expected_network: StableKey,
        handler: &impl WireDeviceHandler,
        limits: WireDeviceLimits,
    ) -> Result<(), SignalError> {
        validate_wire_device_limits(limits)?;
        if self.network != expected_network
            || self.provider != handler.key()
            || self.provider_version != handler.version()
            || self.state.len() > limits.state_bytes as usize
            || self.receipts.len() > limits.receipts as usize
            || self
                .receipts
                .iter()
                .any(|receipt| receipt.0 != self.network)
            || self.receipts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(SignalError::WrongProvider);
        }
        validate_wire_device_handler(handler, &self.state, limits)
    }

    /// 入力portをhandlerへ渡し、graph配送とdevice更新を一つのcommit候補へまとめます。
    ///
    /// duplicate receiptなら`None`を返します。handler error、provider/version不一致、容量超過、
    /// 不正port/graph、sequence overflowではdevice正規状態を変更しません。呼出側はcandidateの
    /// `commit_with` callbackでWorld/StateStore/SignalQueue候補を確定し、成功後だけdeviceを公開します。
    pub fn prepare<'a>(
        &'a mut self,
        graph: &WireGraph,
        signal: &Signal,
        handler: &impl WireDeviceHandler,
        device_limits: WireDeviceLimits,
        signal_limits: SignalLimits,
    ) -> Result<Option<WireDeviceCandidate<'a>>, SignalError> {
        self.validate(graph.network_key(), handler, device_limits)?;
        if graph.network_key() != self.network
            || signal.network != self.network
            || signal.payload.len() > device_limits.payload_bytes as usize
            || signal.payload.len() > signal_limits.payload_bytes as usize
        {
            return Err(SignalError::InvalidSignal);
        }
        let receipt = signal.id();
        if self.receipts.binary_search(&receipt).is_ok() {
            return Ok(None);
        }
        if self.receipts.len() >= device_limits.receipts as usize {
            return Err(SignalError::CapacityExceeded);
        }
        let input_port = graph.port(signal.target)?;
        if input_port.device != self.key || !input_port.direction.can_receive() {
            return Err(SignalError::InvalidPort);
        }
        let result = handler.handle(input_port, signal, &self.state)?;
        if result.state.len() > device_limits.state_bytes as usize
            || result.emissions.len() > device_limits.emissions_per_input as usize
        {
            return Err(SignalError::CapacityExceeded);
        }
        handler.validate_state(&result.state)?;

        let mut candidate = try_clone_wire_device(self)?;
        candidate.state = result.state;
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(SignalError::RevisionOverflow)?;
        let receipt_index = candidate.receipts.binary_search(&receipt).unwrap_err();
        candidate.receipts.insert(receipt_index, receipt);
        let mut deliveries = Vec::new();
        let mut next_sequence = candidate.next_sequence;
        for emission in result.emissions {
            if emission.payload.len() > device_limits.payload_bytes as usize {
                return Err(SignalError::CapacityExceeded);
            }
            let port = graph.port(emission.port)?;
            if port.device != self.key || !port.direction.can_send() {
                return Err(SignalError::InvalidPort);
            }
            let fanout = graph.connections_from(emission.port).len();
            let remaining = device_limits
                .deliveries_per_input
                .saturating_sub(deliveries.len() as u32);
            if fanout > signal_limits.deliveries_per_signal as usize || fanout > remaining as usize
            {
                return Err(SignalError::CapacityExceeded);
            }
            let sequence = next_sequence;
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or(SignalError::TickOverflow)?;
            let outgoing = graph.deliveries(
                &Signal {
                    network: self.network,
                    source: self.key,
                    sequence,
                    target: emission.port,
                    payload: emission.payload,
                },
                emission.timing,
                signal_limits.deliveries_per_signal,
                signal_limits.payload_bytes,
            )?;
            deliveries
                .try_reserve(outgoing.len())
                .map_err(|_| SignalError::AllocationFailed)?;
            deliveries.extend(outgoing);
        }
        candidate.next_sequence = next_sequence;
        Ok(Some(WireDeviceCandidate {
            target: self,
            candidate,
            deliveries,
        }))
    }
}

impl StateCodec for WireDevice {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        if self.provider_version == 0
            || self.state.len() > u32::MAX as usize
            || self.receipts.len() > u32::MAX as usize
            || self
                .receipts
                .iter()
                .any(|receipt| receipt.0 != self.network)
            || self.receipts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid_encoding());
        }
        out.raw(b"CHWDEV01")?;
        self.network.encode(out)?;
        self.key.encode(out)?;
        self.provider.encode(out)?;
        out.u32(self.provider_version)?;
        out.u64(self.revision)?;
        out.u64(self.next_sequence)?;
        out.bytes(&self.state)?;
        out.u32(u32::try_from(self.receipts.len()).map_err(|_| invalid_encoding())?)?;
        for (network, source, sequence, target) in &self.receipts {
            network.encode(out)?;
            source.encode(out)?;
            out.u64(*sequence)?;
            target.encode(out)?;
        }
        Ok(())
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHWDEV01" {
            return Err(invalid_encoding());
        }
        let network = StableKey::decode(input)?;
        let key = StableKey::decode(input)?;
        let provider = StableKey::decode(input)?;
        let provider_version = input.u32()?;
        let revision = input.u64()?;
        let next_sequence = input.u64()?;
        let state = copy_payload(input.bytes(u32::MAX as usize)?)?;
        let count = input.u32()? as usize;
        if count > input.remaining() / 8 {
            return Err(invalid_encoding());
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            receipts.push((
                StableKey::decode(input)?,
                StableKey::decode(input)?,
                input.u64()?,
                StableKey::decode(input)?,
            ));
        }
        let device = Self {
            network,
            key,
            provider,
            provider_version,
            revision,
            next_sequence,
            state,
            receipts,
        };
        if device.provider_version == 0
            || device
                .receipts
                .iter()
                .any(|receipt| receipt.0 != device.network)
            || device.receipts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid_encoding());
        }
        Ok(device)
    }
}

/// device処理後のgraph deliveriesとstateを先行更新するcandidateです。
pub struct WireDeviceCandidate<'a> {
    target: &'a mut WireDevice,
    candidate: WireDevice,
    deliveries: Vec<SignalDelivery>,
}

impl WireDeviceCandidate<'_> {
    /// 同一transactionへ追加するstable順signal deliveryです。
    pub fn deliveries(&self) -> &[SignalDelivery] {
        &self.deliveries
    }

    /// StateStoreやWorldへ保存するdevice候補です。
    pub fn device_state(&self) -> &WireDevice {
        &self.candidate
    }

    /// 外側候補の成功後だけcomponent stateを公開します。
    pub fn commit_with<R, E>(
        self,
        commit_external: impl FnOnce(&[SignalDelivery], &WireDevice) -> Result<R, E>,
    ) -> Result<R, E> {
        let output = commit_external(&self.deliveries, &self.candidate)?;
        *self.target = self.candidate;
        Ok(output)
    }
}

fn validate_wire_device_limits(limits: WireDeviceLimits) -> Result<(), SignalError> {
    if limits.receipts == 0 {
        Err(SignalError::InvalidState)
    } else {
        Ok(())
    }
}

fn validate_wire_device_handler(
    handler: &impl WireDeviceHandler,
    state: &[u8],
    limits: WireDeviceLimits,
) -> Result<(), SignalError> {
    if handler.version() == 0 || state.len() > limits.state_bytes as usize {
        return Err(SignalError::WrongProvider);
    }
    handler.validate_state(state)
}

fn try_clone_wire_device(device: &WireDevice) -> Result<WireDevice, SignalError> {
    let state = copy_payload(&device.state).map_err(|_| SignalError::AllocationFailed)?;
    let mut receipts = Vec::new();
    receipts
        .try_reserve_exact(device.receipts.len().saturating_add(1))
        .map_err(|_| SignalError::AllocationFailed)?;
    receipts.extend_from_slice(&device.receipts);
    Ok(WireDevice {
        network: device.network,
        key: device.key,
        provider: device.provider,
        provider_version: device.provider_version,
        revision: device.revision,
        next_sequence: device.next_sequence,
        state,
        receipts,
    })
}

fn copy_payload(payload: &[u8]) -> EngineResult<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(payload.len()).map_err(allocation)?;
    copy.extend_from_slice(payload);
    Ok(copy)
}

fn validate_wire_limits(limits: WireGraphLimits) -> Result<(), SignalError> {
    if limits.ports == 0 || limits.regions == 0 {
        Err(SignalError::CapacityExceeded)
    } else {
        Ok(())
    }
}

/// 信号の伝播とSameTick/Deferred方針を所有する、差替え可能なproviderです。
///
/// callbackは同じsignalとprovider stateで同じ順序付き配送候補を返します。永続的な作者stateは
/// StateStoreへ保存し、callback内の外部副作用でWorldやqueueを変更しません。
pub trait SignalsProvider {
    /// providerの安定key。
    fn key(&self) -> StableKey;
    /// save/loadで照合するprovider版。
    fn version(&self) -> u32;
    /// 現在のnetwork nodeで信号を解釈し、次配送を候補として返します。
    fn propagate(&self, signal: &Signal) -> Result<Vec<SignalDelivery>, SignalError>;
}

/// 信号処理で起きた失敗です。失敗時は未処理signalをqueueへ戻します。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalError {
    WrongProvider,
    InvalidSignal,
    DuplicateSequence,
    CapacityExceeded,
    InvalidState,
    TickOverflow,
    AllocationFailed,
    ProviderFailed,
    InvalidPort,
    DuplicateConnection,
    SignalTypeMismatch,
    StaleRevision,
    RevisionOverflow,
}
impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SignalError {}

/// 1network・1tickの安定順queue、次tick queue、loop検出集合です。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalQueue {
    network: StableKey,
    provider: StableKey,
    provider_version: u32,
    tick: u64,
    current: BTreeMap<SignalId, Signal>,
    deferred: BTreeMap<SignalId, Signal>,
    visited: BTreeSet<SignalId>,
    queued_payload_bytes: u64,
}

impl SignalQueue {
    /// 信号networkと差替えproviderのstampを固定した空queueを作ります。
    pub fn new(
        network: StableKey,
        provider: &impl SignalsProvider,
        tick: u64,
    ) -> Result<Self, SignalError> {
        if provider.version() == 0 {
            return Err(SignalError::WrongProvider);
        }
        Ok(Self {
            network,
            provider: provider.key(),
            provider_version: provider.version(),
            tick,
            current: BTreeMap::new(),
            deferred: BTreeMap::new(),
            visited: BTreeSet::new(),
            queued_payload_bytes: 0,
        })
    }

    /// 現在のsimulation tickです。
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// queueが所有するnetwork/world scopeです。
    pub fn network_key(&self) -> StableKey {
        self.network
    }
    /// 同tick queueと未処理次tick queueを合わせた数です。
    pub fn pending(&self) -> usize {
        self.current.len() + self.deferred.len()
    }

    /// 現在tickで処理可能なqueue件数です。次tick待ちのDeferred signalは含みません。
    pub fn ready_pending(&self) -> usize {
        self.current.len()
    }

    /// 外部inputを現在または次tick queueへ原子的に追加します。
    pub fn enqueue(
        &mut self,
        signal: Signal,
        timing: SignalTiming,
        limits: SignalLimits,
    ) -> Result<bool, SignalError> {
        if signal.network != self.network || signal.payload.len() > limits.payload_bytes as usize {
            return Err(SignalError::InvalidSignal);
        }
        let id = signal.id();
        if self.visited.contains(&id) {
            return Ok(false);
        }
        if self.current.contains_key(&id) || self.deferred.contains_key(&id) {
            return Err(SignalError::DuplicateSequence);
        }
        let count = self
            .visited
            .len()
            .saturating_add(self.current.len())
            .saturating_add(self.deferred.len());
        let bytes = self
            .queued_payload_bytes
            .checked_add(signal.payload.len() as u64)
            .ok_or(SignalError::CapacityExceeded)?;
        if count >= limits.signals_per_tick as usize || bytes > limits.queued_payload_bytes {
            return Err(SignalError::CapacityExceeded);
        }
        match timing {
            SignalTiming::SameTick => {
                self.current.insert(id, signal);
            }
            SignalTiming::Deferred => {
                self.deferred.insert(id, signal);
            }
        }
        self.queued_payload_bytes = bytes;
        Ok(true)
    }

    /// signal件数budget以内で配送候補を作ります。副作用の適用は呼出側transactionへ委ねます。
    pub fn poll(
        &mut self,
        provider: &impl SignalsProvider,
        limits: SignalLimits,
        budget: u32,
    ) -> Result<SignalProgress, SignalError> {
        if provider.key() != self.provider || provider.version() != self.provider_version {
            return Err(SignalError::WrongProvider);
        }
        if self
            .visited
            .len()
            .saturating_add(self.current.len())
            .saturating_add(self.deferred.len())
            > limits.signals_per_tick as usize
            || self.queued_payload_bytes > limits.queued_payload_bytes
        {
            return Err(SignalError::CapacityExceeded);
        }
        if budget == 0 {
            return Ok(if self.current.is_empty() {
                SignalProgress::TickComplete
            } else {
                SignalProgress::ContinueSameTick
            });
        }
        let mut processed = 0;
        while processed < budget {
            let Some((id, signal)) = self.current.pop_first() else {
                return Ok(SignalProgress::TickComplete);
            };
            let mut deliveries = match provider.propagate(&signal) {
                Ok(deliveries) => deliveries,
                Err(_) => {
                    self.current.insert(id, signal);
                    return Err(SignalError::ProviderFailed);
                }
            };
            if deliveries.len() > limits.deliveries_per_signal as usize
                || deliveries.iter().any(|delivery| {
                    delivery.signal.network != self.network
                        || delivery.signal.payload.len() > limits.payload_bytes as usize
                })
            {
                self.current.insert(id, signal);
                return Err(SignalError::CapacityExceeded);
            }
            deliveries.sort_unstable_by_key(|delivery| {
                let timing = match delivery.timing {
                    SignalTiming::SameTick => 0_u8,
                    SignalTiming::Deferred => 1_u8,
                };
                (delivery.signal.id(), timing)
            });
            let mut conflicting_duplicate = false;
            deliveries.dedup_by(|first, second| {
                if first.signal.id() != second.signal.id() {
                    return false;
                }
                if first.signal != second.signal || first.timing != second.timing {
                    conflicting_duplicate = true;
                }
                true
            });
            if conflicting_duplicate {
                self.current.insert(id, signal);
                return Err(SignalError::DuplicateSequence);
            }
            let mut candidate_current = Vec::new();
            let mut candidate_deferred = Vec::new();
            if candidate_current
                .try_reserve_exact(deliveries.len())
                .is_err()
                || candidate_deferred
                    .try_reserve_exact(deliveries.len())
                    .is_err()
            {
                self.current.insert(id, signal);
                return Err(SignalError::AllocationFailed);
            }
            for delivery in deliveries {
                let next_id = delivery.signal.id();
                if self.visited.contains(&next_id) || next_id == id {
                    continue;
                }
                let (same_timing, other_timing) = match delivery.timing {
                    SignalTiming::SameTick => {
                        (self.current.get(&next_id), self.deferred.get(&next_id))
                    }
                    SignalTiming::Deferred => {
                        (self.deferred.get(&next_id), self.current.get(&next_id))
                    }
                };
                if other_timing.is_some()
                    || same_timing.is_some_and(|known| known != &delivery.signal)
                {
                    self.current.insert(id, signal);
                    return Err(SignalError::DuplicateSequence);
                }
                if same_timing.is_some() {
                    continue;
                }
                match delivery.timing {
                    SignalTiming::SameTick => candidate_current.push((next_id, delivery.signal)),
                    SignalTiming::Deferred => candidate_deferred.push((next_id, delivery.signal)),
                }
            }
            let total = self
                .visited
                .len()
                .saturating_add(1)
                .saturating_add(self.current.len())
                .saturating_add(self.deferred.len())
                .saturating_add(candidate_current.len())
                .saturating_add(candidate_deferred.len());
            let remaining_bytes = self
                .queued_payload_bytes
                .checked_sub(signal.payload.len() as u64);
            let Some(remaining_bytes) = remaining_bytes else {
                self.current.insert(id, signal);
                return Err(SignalError::InvalidState);
            };
            let next_bytes = candidate_current
                .iter()
                .chain(candidate_deferred.iter())
                .try_fold(remaining_bytes, |sum, (_, candidate)| {
                    sum.checked_add(candidate.payload.len() as u64)
                        .ok_or(SignalError::CapacityExceeded)
                });
            let Ok(next_bytes) = next_bytes else {
                self.current.insert(id, signal);
                return Err(SignalError::CapacityExceeded);
            };
            if total > limits.signals_per_tick as usize || next_bytes > limits.queued_payload_bytes
            {
                self.current.insert(id, signal);
                return Err(SignalError::CapacityExceeded);
            }
            self.visited.insert(id);
            self.queued_payload_bytes = next_bytes;
            for (id, signal) in candidate_current {
                self.current.insert(id, signal);
            }
            for (id, signal) in candidate_deferred {
                self.deferred.insert(id, signal);
            }
            processed += 1;
        }
        Ok(if self.current.is_empty() {
            SignalProgress::TickComplete
        } else {
            SignalProgress::ContinueSameTick
        })
    }

    /// current queueの完了後にtickを進め、Deferred signalを同tick queueへ移します。
    pub fn begin_next_tick(&mut self) -> Result<(), SignalError> {
        if !self.current.is_empty() {
            return Err(SignalError::InvalidState);
        }
        self.tick = self.tick.checked_add(1).ok_or(SignalError::TickOverflow)?;
        self.current = std::mem::take(&mut self.deferred);
        self.visited.clear();
        Ok(())
    }

    /// 呼出側のconfig上限を保存状態へ適用します。
    ///
    /// checkpoint復元後とSignalLimitsを変更した境界で呼びます。queue全体を読むため、毎pollでは
    /// 実行せず、通常pollは件数・payload合計の増分counterと各signalの境界だけを確認します。
    pub fn validate_limits(&self, limits: SignalLimits) -> Result<(), SignalError> {
        let count = self
            .visited
            .len()
            .saturating_add(self.current.len())
            .saturating_add(self.deferred.len());
        if count > limits.signals_per_tick as usize
            || self.queued_payload_bytes > limits.queued_payload_bytes
            || self
                .current
                .values()
                .chain(self.deferred.values())
                .any(|signal| signal.payload.len() > limits.payload_bytes as usize)
        {
            return Err(SignalError::CapacityExceeded);
        }
        let bytes = self
            .current
            .values()
            .chain(self.deferred.values())
            .try_fold(0u64, |sum, signal| {
                sum.checked_add(signal.payload.len() as u64)
                    .ok_or(SignalError::CapacityExceeded)
            })?;
        if bytes != self.queued_payload_bytes {
            return Err(SignalError::InvalidState);
        }
        Ok(())
    }
}

impl StateCodec for SignalQueue {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        self.validate_state().map_err(|_| invalid_encoding())?;
        out.raw(b"CHRSIG01")?;
        self.network.encode(out)?;
        self.provider.encode(out)?;
        out.u32(self.provider_version)?;
        out.u64(self.tick)?;
        encode_signals(&self.current, out)?;
        encode_signals(&self.deferred, out)?;
        out.u32(u32::try_from(self.visited.len()).map_err(|_| invalid_encoding())?)?;
        for id in &self.visited {
            encode_id(*id, out)?;
        }
        out.u64(self.queued_payload_bytes)
    }

    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHRSIG01" {
            return Err(invalid_encoding());
        }
        let network = StableKey::decode(input)?;
        let provider = StableKey::decode(input)?;
        let provider_version = input.u32()?;
        let tick = input.u64()?;
        if provider_version == 0 {
            return Err(invalid_encoding());
        }
        let current = decode_signals(input, network)?;
        let deferred = decode_signals(input, network)?;
        let visited_count = input.u32()?;
        if visited_count as usize > input.remaining() / 23 {
            return Err(invalid_encoding());
        }
        let mut visited = BTreeSet::new();
        let mut previous_visited = None;
        for _ in 0..visited_count {
            let id = decode_id(input)?;
            if previous_visited.is_some_and(|previous| previous >= id) || !visited.insert(id) {
                return Err(invalid_encoding());
            }
            previous_visited = Some(id);
        }
        let queued_payload_bytes = input.u64()?;
        let queue = Self {
            network,
            provider,
            provider_version,
            tick,
            current,
            deferred,
            visited,
            queued_payload_bytes,
        };
        queue.validate_state().map_err(|_| invalid_encoding())?;
        Ok(queue)
    }
}

impl SignalQueue {
    fn validate_state(&self) -> Result<(), SignalError> {
        if self.provider_version == 0
            || self
                .current
                .iter()
                .chain(self.deferred.iter())
                .any(|(id, signal)| id != &signal.id() || signal.network != self.network)
            || self.visited.iter().any(|id| id.0 != self.network)
            || self.current.keys().any(|id| self.deferred.contains_key(id))
        {
            return Err(SignalError::InvalidState);
        }
        let bytes = self
            .current
            .values()
            .chain(self.deferred.values())
            .try_fold(0u64, |sum, signal| {
                sum.checked_add(signal.payload.len() as u64)
                    .ok_or(SignalError::CapacityExceeded)
            })?;
        if bytes != self.queued_payload_bytes {
            return Err(SignalError::InvalidState);
        }
        Ok(())
    }
}

fn encode_id(id: SignalId, out: &mut Encoder) -> EngineResult<()> {
    id.0.encode(out)?;
    id.1.encode(out)?;
    id.2.encode(out)?;
    id.3.encode(out)
}
fn decode_id(input: &mut Decoder<'_>) -> EngineResult<SignalId> {
    Ok((
        StableKey::decode(input)?,
        StableKey::decode(input)?,
        input.u64()?,
        StableKey::decode(input)?,
    ))
}
fn encode_signals(signals: &BTreeMap<SignalId, Signal>, out: &mut Encoder) -> EngineResult<()> {
    out.u32(u32::try_from(signals.len()).map_err(|_| invalid_encoding())?)?;
    for (id, signal) in signals {
        encode_id(*id, out)?;
        out.bytes(&signal.payload)?;
    }
    Ok(())
}
fn decode_signals(
    input: &mut Decoder<'_>,
    network: StableKey,
) -> EngineResult<BTreeMap<SignalId, Signal>> {
    let count = input.u32()?;
    if count as usize > input.remaining() / 27 {
        return Err(invalid_encoding());
    }
    let mut signals = BTreeMap::new();
    let mut previous = None;
    for _ in 0..count {
        let id = decode_id(input)?;
        if id.0 != network || previous.is_some_and(|old| old >= id) {
            return Err(invalid_encoding());
        }
        previous = Some(id);
        let payload = input.bytes(u32::MAX as usize)?;
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(payload.len())
            .map_err(allocation)?;
        copied.extend_from_slice(payload);
        signals.insert(
            id,
            Signal {
                network,
                source: id.1,
                sequence: id.2,
                target: id.3,
                payload: copied,
            },
        );
    }
    Ok(signals)
}
fn allocation(_: std::collections::TryReserveError) -> EngineError {
    EngineError::new(ErrorCode::AllocationFailed, "signal queue保存領域の確保")
}
fn invalid_encoding() -> EngineError {
    EngineError::new(
        ErrorCode::InvalidEncoding,
        "signal queueの保存形式が不正です",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::codec::StateCodec;

    fn key(value: &str) -> StableKey {
        StableKey::parse(value).unwrap()
    }
    fn limits() -> SignalLimits {
        SignalLimits {
            signals_per_tick: 32,
            queued_payload_bytes: 4096,
            deliveries_per_signal: 8,
            payload_bytes: 128,
        }
    }
    fn make_signal(source: &str, sequence: u64, target: &str) -> Signal {
        Signal {
            network: key("test:wires"),
            source: key(source),
            sequence,
            target: key(target),
            payload: vec![1],
        }
    }

    struct Ring;
    impl SignalsProvider for Ring {
        fn key(&self) -> StableKey {
            key("test:ring-provider")
        }
        fn version(&self) -> u32 {
            1
        }
        fn propagate(&self, signal: &Signal) -> Result<Vec<SignalDelivery>, SignalError> {
            let target = match signal.target.as_str() {
                "test:a" => "test:b",
                _ => "test:a",
            };
            Ok(vec![SignalDelivery {
                signal: make_signal(signal.source.as_str(), signal.sequence, target),
                timing: SignalTiming::SameTick,
            }])
        }
    }

    #[test]
    fn same_tick_loop_is_deduplicated_and_deferred_state_roundtrips() {
        let provider = Ring;
        let mut queue = SignalQueue::new(key("test:wires"), &provider, 4).unwrap();
        queue
            .enqueue(
                make_signal("test:source", 7, "test:a"),
                SignalTiming::SameTick,
                limits(),
            )
            .unwrap();
        assert_eq!(
            queue.poll(&provider, limits(), 1).unwrap(),
            SignalProgress::ContinueSameTick
        );
        let mut encoder = Encoder::new(8192);
        queue.encode(&mut encoder).unwrap();
        let encoded = encoder.finish();
        let mut restored = SignalQueue::decode(&mut Decoder::new(&encoded, 8192).unwrap()).unwrap();
        assert_eq!(
            restored.poll(&provider, limits(), 8).unwrap(),
            SignalProgress::TickComplete
        );
        assert_eq!(restored.pending(), 0);
        assert_eq!(
            restored.enqueue(
                make_signal("test:source", 7, "test:a"),
                SignalTiming::SameTick,
                limits()
            ),
            Ok(false)
        );
    }

    #[test]
    fn deferred_delivery_waits_for_next_logical_tick() {
        struct Delayed;
        impl SignalsProvider for Delayed {
            fn key(&self) -> StableKey {
                key("test:delayed-provider")
            }
            fn version(&self) -> u32 {
                1
            }
            fn propagate(&self, signal: &Signal) -> Result<Vec<SignalDelivery>, SignalError> {
                Ok(vec![SignalDelivery {
                    signal: make_signal(signal.source.as_str(), signal.sequence + 1, "test:next"),
                    timing: SignalTiming::Deferred,
                }])
            }
        }
        let provider = Delayed;
        let mut queue = SignalQueue::new(key("test:wires"), &provider, 12).unwrap();
        queue
            .enqueue(
                make_signal("test:source", 1, "test:start"),
                SignalTiming::SameTick,
                limits(),
            )
            .unwrap();
        assert_eq!(
            queue.poll(&provider, limits(), 1).unwrap(),
            SignalProgress::TickComplete
        );
        assert_eq!(queue.pending(), 1);
        queue.begin_next_tick().unwrap();
        assert_eq!(queue.tick(), 13);
        assert_eq!(
            queue.poll(&provider, limits(), 1).unwrap(),
            SignalProgress::TickComplete
        );
    }

    #[test]
    fn wire_graph_validates_ports_directions_types_and_region_revisions() {
        let limits = WireGraphLimits {
            ports: 4,
            directed_connections: 4,
            regions: 2,
        };
        let mut graph = WireGraph::new(key("test:wire-graph"), limits).unwrap();
        let output = WirePort {
            key: key("test:device-output"),
            device: key("test:source-device"),
            region: key("test:region-a"),
            signal_type: key("test:power-signal"),
            direction: WirePortDirection::Output,
        };
        let input = WirePort {
            key: key("test:device-input"),
            device: key("test:target-device"),
            region: key("test:region-b"),
            signal_type: key("test:power-signal"),
            direction: WirePortDirection::Input,
        };
        assert_eq!(graph.register_port(0, output, limits), Ok(1));
        assert_eq!(graph.register_port(1, input, limits), Ok(2));
        assert_eq!(graph.region_revision(output.region), Some(1));
        assert_eq!(graph.region_revision(input.region), Some(1));
        assert_eq!(graph.connect(2, output.key, input.key, limits), Ok(3));
        assert_eq!(
            graph.connections_from(output.key),
            &[WireConnection {
                source: output.key,
                target: input.key,
            }]
        );
        assert_eq!(graph.region_revision(output.region), Some(2));
        assert_eq!(graph.region_revision(input.region), Some(2));
        let signal = Signal {
            network: graph.network_key(),
            source: output.device,
            sequence: 1,
            target: output.key,
            payload: vec![5, 9],
        };
        let deliveries = graph
            .deliveries(&signal, SignalTiming::Deferred, 1, 64)
            .unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].signal.target, input.key);
        assert_eq!(deliveries[0].signal.payload, signal.payload);
        assert_eq!(deliveries[0].timing, SignalTiming::Deferred);
        assert_eq!(
            graph.deliveries(&signal, SignalTiming::Deferred, 0, 64),
            Err(SignalError::CapacityExceeded)
        );
        assert_eq!(
            graph.connect(2, output.key, input.key, limits),
            Err(SignalError::StaleRevision)
        );
        assert_eq!(graph.disconnect(3, output.key, input.key), Ok(Some(4)));
        assert!(graph.connections().is_empty());
        assert_eq!(graph.region_revision(output.region), Some(3));
        assert_eq!(graph.region_revision(input.region), Some(3));
        assert_eq!(graph.unregister_port(4, input.key), Ok(5));
        assert_eq!(graph.ports().len(), 1);
        assert!(graph.connections().is_empty());
        assert_eq!(graph.region_revision(input.region), Some(4));
    }

    #[test]
    fn wire_graph_codec_roundtrips_and_connection_rejects_wrong_signal_type() {
        let limits = WireGraphLimits {
            ports: 4,
            directed_connections: 4,
            regions: 1,
        };
        let mut graph = WireGraph::new(key("test:wire-graph"), limits).unwrap();
        let output = WirePort {
            key: key("test:output"),
            device: key("test:device-a"),
            region: key("test:region"),
            signal_type: key("test:power"),
            direction: WirePortDirection::Output,
        };
        let input = WirePort {
            key: key("test:input"),
            device: key("test:device-b"),
            region: key("test:region"),
            signal_type: key("test:logic"),
            direction: WirePortDirection::Input,
        };
        graph.register_port(0, output, limits).unwrap();
        graph.register_port(1, input, limits).unwrap();
        assert_eq!(
            graph.connect(2, output.key, input.key, limits),
            Err(SignalError::SignalTypeMismatch)
        );
        let mut encoded = Encoder::new(4096);
        graph.encode(&mut encoded).unwrap();
        let bytes = encoded.finish();
        let restored = WireGraph::decode(&mut Decoder::new(&bytes, 4096).unwrap()).unwrap();
        assert_eq!(restored, graph);
    }

    #[test]
    fn wire_graph_batch_build_is_stable_and_respects_directed_capacity() {
        let output = WirePort {
            key: key("test:z-output"),
            device: key("test:device-z"),
            region: key("test:region"),
            signal_type: key("test:signal"),
            direction: WirePortDirection::Output,
        };
        let input = WirePort {
            key: key("test:a-input"),
            device: key("test:device-a"),
            region: key("test:region"),
            signal_type: key("test:signal"),
            direction: WirePortDirection::Input,
        };
        let graph = WireGraph::from_parts(
            key("test:batch-wires"),
            vec![output, input],
            vec![(output.key, input.key)],
            WireGraphLimits {
                ports: 2,
                directed_connections: 1,
                regions: 1,
            },
        )
        .unwrap();
        assert_eq!(
            graph
                .ports()
                .iter()
                .map(|port| port.key)
                .collect::<Vec<_>>(),
            vec![input.key, output.key]
        );
        assert_eq!(graph.connections().len(), 1);
        assert_eq!(graph.connections()[0].source, output.key);
        assert_eq!(graph.connections()[0].target, input.key);
        assert_eq!(graph.revision(), 1);
    }

    struct CounterDevice {
        input: StableKey,
        output: StableKey,
    }

    impl WireDeviceHandler for CounterDevice {
        fn key(&self) -> StableKey {
            key("test:counter-device-handler")
        }

        fn version(&self) -> u32 {
            1
        }

        fn validate_state(&self, state: &[u8]) -> Result<(), SignalError> {
            if state.len() == 1 {
                Ok(())
            } else {
                Err(SignalError::InvalidState)
            }
        }

        fn handle(
            &self,
            input_port: &WirePort,
            _signal: &Signal,
            state: &[u8],
        ) -> Result<WireDeviceResult, SignalError> {
            if input_port.key != self.input {
                return Err(SignalError::InvalidPort);
            }
            let next = state[0].checked_add(1).ok_or(SignalError::TickOverflow)?;
            Ok(WireDeviceResult {
                state: vec![next],
                emissions: vec![WireEmission {
                    port: self.output,
                    payload: vec![next],
                    timing: SignalTiming::Deferred,
                }],
            })
        }
    }

    fn device_limits() -> WireDeviceLimits {
        WireDeviceLimits {
            state_bytes: 1,
            emissions_per_input: 2,
            deliveries_per_input: 2,
            payload_bytes: 8,
            receipts: 8,
        }
    }

    fn device_fixture() -> (WireGraph, CounterDevice, WireDevice, Signal) {
        let network = key("test:device-network");
        let device_key = key("test:counter-device");
        let input = WirePort {
            key: key("test:counter-input"),
            device: device_key,
            region: key("test:device-region"),
            signal_type: key("test:device-signal"),
            direction: WirePortDirection::Input,
        };
        let output = WirePort {
            key: key("test:counter-output"),
            device: device_key,
            region: key("test:device-region"),
            signal_type: key("test:device-signal"),
            direction: WirePortDirection::Output,
        };
        let target = WirePort {
            key: key("test:device-target"),
            device: key("test:target-device"),
            region: key("test:target-region"),
            signal_type: key("test:device-signal"),
            direction: WirePortDirection::Input,
        };
        let graph = WireGraph::from_parts(
            network,
            vec![input, output, target],
            vec![(output.key, target.key)],
            WireGraphLimits {
                ports: 3,
                directed_connections: 2,
                regions: 2,
            },
        )
        .unwrap();
        let handler = CounterDevice {
            input: input.key,
            output: output.key,
        };
        let device =
            WireDevice::new(network, device_key, &handler, vec![0], device_limits()).unwrap();
        let signal = Signal {
            network,
            source: key("test:upstream-device"),
            sequence: 7,
            target: input.key,
            payload: vec![9],
        };
        (graph, handler, device, signal)
    }

    #[test]
    fn wire_device_candidates_are_atomic_and_receipts_suppress_duplicates() {
        let (graph, handler, mut device, signal) = device_fixture();
        let mut undersized_queue_limits = limits();
        undersized_queue_limits.payload_bytes = 0;
        assert!(matches!(
            device.prepare(
                &graph,
                &signal,
                &handler,
                device_limits(),
                undersized_queue_limits
            ),
            Err(SignalError::InvalidSignal)
        ));
        assert_eq!(device.revision(), 0);
        let candidate = device
            .prepare(&graph, &signal, &handler, device_limits(), limits())
            .unwrap()
            .unwrap();
        assert_eq!(candidate.device_state().revision(), 1);
        assert_eq!(candidate.deliveries().len(), 1);
        assert_eq!(
            candidate.deliveries()[0].signal.target,
            key("test:device-target")
        );
        assert_eq!(candidate.deliveries()[0].signal.sequence, 0);
        assert_eq!(candidate.deliveries()[0].signal.payload, vec![1]);
        assert_eq!(candidate.deliveries()[0].timing, SignalTiming::Deferred);
        assert_eq!(
            candidate.commit_with::<(), _>(|_, _| Err("abort")),
            Err("abort")
        );
        assert_eq!(device.revision(), 0);
        assert_eq!(device.next_sequence(), 0);

        let candidate = device
            .prepare(&graph, &signal, &handler, device_limits(), limits())
            .unwrap()
            .unwrap();
        candidate.commit_with(|_, _| Ok::<_, ()>(())).unwrap();
        assert_eq!(device.revision(), 1);
        assert_eq!(device.next_sequence(), 1);
        assert!(
            device
                .prepare(&graph, &signal, &handler, device_limits(), limits())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn stateless_wire_device_allows_zero_state_byte_budget() {
        struct Stateless;
        impl WireDeviceHandler for Stateless {
            fn key(&self) -> StableKey {
                key("test:stateless-device")
            }
            fn version(&self) -> u32 {
                1
            }
            fn validate_state(&self, state: &[u8]) -> Result<(), SignalError> {
                if state.is_empty() {
                    Ok(())
                } else {
                    Err(SignalError::InvalidState)
                }
            }
            fn handle(
                &self,
                _input_port: &WirePort,
                _signal: &Signal,
                state: &[u8],
            ) -> Result<WireDeviceResult, SignalError> {
                self.validate_state(state)?;
                Ok(WireDeviceResult {
                    state: Vec::new(),
                    emissions: Vec::new(),
                })
            }
        }
        let mut limits = device_limits();
        limits.state_bytes = 0;
        let device = WireDevice::new(
            key("test:stateless-network"),
            key("test:stateless-instance"),
            &Stateless,
            Vec::new(),
            limits,
        );
        assert!(device.is_ok());
    }
}
