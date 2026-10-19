//! 気体の連結空間と温度を含む、保存可能な広域圧力の参照計算。
//!
//! 入力採取、配列確保、保存、復元と最終公開は一括処理です。stepの件数予算は
//! 隣接構築・連結探索・連結量の反映・Jacobiの各cell処理を分割します。

use super::{
    CellQuery, ChannelQuery, ChunkCoord, LayerId, MaterialBase, MaterialId, PressureRelaxation,
    PressureRelaxationReport, World, WorldEdit, conflict, invalid,
};
use crate::foundation::{
    EngineError, EngineResult, ErrorCode, StableKey,
    codec::{Decoder, Encoder, StateCodec},
    error::allocation,
    grid::CellPoint,
    id::WorldId,
};

/// 標準気体proxyの式。実在気体の状態方程式の精度を保証するものではありません。
#[derive(Clone, Copy, Debug)]
pub struct GasPressureSource {
    /// 気体cellの絶対温度Q16を保存したi64の1対1 channel。負値は拒否します。
    /// 位置固定・同layerの物質追従のどちらも使えます。
    pub temperature: StableKey,
}

/// 広域計算の入力。設定はjobとともに保存され、途中で暗黙に変更されません。
#[derive(Clone, Copy, Debug)]
pub struct PressureJobConfig {
    /// 出力圧力・重力・反復数・scratchのcell上限。
    pub relaxation: PressureRelaxation,
    /// Noneでは従来の液体・気体共通緩和と同じ式です。
    /// Someでは液体と気体の連結を分け、気体は空cellを容積へ含めます。
    pub gas: Option<GasPressureSource>,
    /// 任意の位置固定i64 Q16 channel。非負値をその位置の最低圧として課します。
    /// ポンプの仕事量や物質発行を表すものではなく、作者が設定する圧力境界です。
    pub pump: Option<StableKey>,
}

/// 計算段階。完成前には圧力channelを書き換えません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressureJobStatus {
    /// 隣接、連結、または反復の途中です。
    Computing,
    /// 全反復が完成し、一括公開できます。
    Ready,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Input {
    point: CellPoint,
    material: MaterialId,
    // 0=空間、1=液体（従来profileでは気体も含む）、2=気体。
    kind: u8,
    mass: u32,
    pressure: i64,
    temperature: i64,
    pump: i64,
    surface: bool,
    coefficient: u32,
    weight: u32,
    maximum: i64,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Node {
    input: Input,
    edges: [Option<u32>; 4],
    seen: bool,
    fixed: Option<i64>,
    source: Option<i64>,
    old: i64,
    next: i64,
}

#[derive(Clone, Copy, Default)]
struct PressureSummary {
    pressure: u128,
    gas: u128,
    open: bool,
    has_gas: bool,
}
impl PressureSummary {
    fn target(self, kind: u8, count: usize) -> EngineResult<i64> {
        let value = if kind == 1 {
            if self.open {
                0
            } else {
                self.pressure / count as u128
            }
        } else {
            self.gas / (count as u128 * 65536)
        };
        i64::try_from(value).map_err(|_| invalid("保存した成分targetの上限"))
    }
}

/// Worldに所属する広域圧力job。StateCodecをschemaへ登録してWorld状態に保存できます。
///
/// 入力は全活動chunkのrevisionで保守的に失効させます。入力採取は全活動地形の走査、
/// 各stepには別途活動集合の照合費用があります。stepの予算をwall-clock上限とはしません。
/// 完成したjobの公開は全入力と最終容量を再検査する一括取引です。
pub struct PressureJob {
    world: WorldId,
    content: [u8; 32],
    epoch: u64,
    layer: StableKey,
    config: PressureJobConfig,
    chunks: Vec<(ChunkCoord, u64)>,
    nodes: Vec<Node>,
    // 0=隣接、1=連結root選択、2=BFS、3=連結反映、4=Jacobi、5=完成。
    stage: u8,
    cursor: usize,
    root: usize,
    queue: Vec<u32>,
    queue_cursor: usize,
    total_pressure: u128,
    total_gas: u128,
    open: bool,
    has_gas: bool,
    target: i64,
    iteration: u16,
    work: u64,
    report: PressureRelaxationReport,
}

impl PressureJob {
    /// 全入力を採取します。cell上限には、気体有効時の連結空間用の空cellも含みます。
    /// この採取・事前確保は分割stepの予算外であり、巨大な活動域では別途費用があります。
    pub fn new(world: &World, layer: LayerId, config: PressureJobConfig) -> EngineResult<Self> {
        if config.relaxation.iterations == 0
            || !world.content.layer(layer)?.simulated
            || config.relaxation.cell_limit > u32::MAX as usize
        {
            return Err(invalid("広域圧力の反復数・cell上限・layer"));
        }
        check_channel(world, config.relaxation.channel, None)?;
        if let Some(gas) = config.gas {
            check_channel(world, gas.temperature, Some(layer))?;
            if gas.temperature == config.relaxation.channel {
                return Err(invalid("温度と出力圧力は別channelが必要"));
            }
        }
        if let Some(pump) = config.pump {
            check_channel(world, pump, None)?;
            if pump == config.relaxation.channel {
                return Err(invalid("ポンプと出力圧力は別channelが必要"));
            }
        }
        let mut chunks = Vec::new();
        let mut nodes = Vec::new();
        for (coord, chunk) in &world.chunks {
            if !chunk.active {
                continue;
            }
            chunks.try_reserve(1).map_err(allocation)?;
            chunks.push((*coord, chunk.revision));
            let origin = coord.origin()?;
            let page = &chunk.layers[layer.0 as usize];
            for index in 0..4096 {
                if page
                    .blocks
                    .as_ref()
                    .is_some_and(|blocks| blocks[index].is_some())
                {
                    continue;
                }
                let material = page.cells[index].material;
                let definition = world.content.material(material)?;
                let kind = match definition.map(|material| material.base()) {
                    Some(MaterialBase::Liquid) => 1,
                    Some(MaterialBase::Gas) => {
                        if config.gas.is_some() {
                            2
                        } else {
                            1
                        }
                    }
                    None if config.gas.is_some() => 0,
                    _ => continue,
                };
                let point = CellPoint {
                    x: origin.x + (index % 64) as i32,
                    y: origin.y + (index / 64) as i32,
                };
                if !world.bounds.contains(point) {
                    continue;
                }
                if nodes.len() == config.relaxation.cell_limit {
                    return Err(EngineError::capacity(
                        "広域圧力のcell・空間数",
                        nodes.len() as u64 + 1,
                        config.relaxation.cell_limit as u64,
                    ));
                }

                let mut surface = false;
                if kind == 1 {
                    for (dx, dy) in NEIGHBORS {
                        if let Some(other) = offset(point, dx, dy) {
                            surface |= active_empty(world, layer, other)?;
                        }
                    }
                }
                let properties = definition
                    .and_then(|material| material.gas_pressure())
                    .unwrap_or_default();
                let input = Input {
                    point,
                    material,
                    kind,
                    mass: definition.map_or(0, |material| material.mass_per_cell()),
                    pressure: if kind == 0 {
                        0
                    } else {
                        read(world, config.relaxation.channel, point)?
                    },
                    temperature: if kind == 2 {
                        read(world, config.gas.expect("気体profile").temperature, point)?
                    } else {
                        0
                    },
                    pump: if kind == 0 {
                        0
                    } else {
                        config
                            .pump
                            .map(|key| read(world, key, point))
                            .transpose()?
                            .unwrap_or(0)
                    },
                    surface,
                    coefficient: if kind == 2 {
                        properties.coefficient_q16
                    } else {
                        0
                    },
                    weight: if kind == 2 {
                        properties.relaxation_weight
                    } else {
                        0
                    },
                    maximum: if kind == 2 {
                        properties.maximum_q16
                    } else {
                        i64::MAX
                    },
                };
                if input.pressure > input.maximum {
                    return Err(invalid("気体の保存圧力が素材上限を超過"));
                }
                nodes.try_reserve(1).map_err(allocation)?;
                nodes.push(Node {
                    input,
                    edges: [None; 4],
                    seen: false,
                    fixed: None,
                    source: None,
                    old: input.pressure,
                    next: 0,
                });
            }
        }
        nodes.sort_unstable_by_key(|node| (node.input.point.x, node.input.point.y));
        let mut queue = Vec::new();
        // 探索中の確保失敗で訪問bitだけが更新されないよう、最大queueを先に予約します。
        queue.try_reserve_exact(nodes.len()).map_err(allocation)?;
        let report = PressureRelaxationReport {
            cells: nodes.iter().filter(|node| node.input.kind != 0).count(),
            ..Default::default()
        };
        Ok(Self {
            world: world.id,
            content: world.content.fingerprint(),
            epoch: world.remaps.epoch(),
            layer: world.content.layer(layer)?.key,
            config,
            chunks,
            nodes,
            stage: 0,
            cursor: 0,
            root: 0,
            queue,
            queue_cursor: 0,
            total_pressure: 0,
            total_gas: 0,
            open: false,
            has_gas: false,
            target: 0,
            iteration: 0,
            work: 0,
            report,
        })
    }

    /// 完成前はComputing、全反復後はReadyです。
    pub fn status(&self) -> PressureJobStatus {
        if self.stage == 5 {
            PressureJobStatus::Ready
        } else {
            PressureJobStatus::Computing
        }
    }
    /// 完了したcell単位の処理数。保存を挟んでも増分が維持されます。
    pub fn completed_work(&self) -> u64 {
        self.work
    }
    /// 完了したJacobi反復数。
    pub fn completed_iterations(&self) -> u16 {
        self.iteration
    }
    /// 流体数と連結領域数。計算途中では連結領域の集計も途中です。
    pub fn report(&self) -> PressureRelaxationReport {
        self.report
    }
    /// scratchへ保持したcell数。気体profileでは空間を含みます。
    pub fn sample_count(&self) -> usize {
        self.nodes.len()
    }

    /// 最大budget件の隣接・探索・反映・反復処理を進めます。Worldを変更しません。
    /// 失敗時は、そのcell処理の直前で止まります。先に完了した処理はjobに残ります。
    /// 容量はnewで予約済みです。入力失効時には進めず、新しいjobが必要です。
    pub fn step(&mut self, world: &World, budget: usize) -> EngineResult<PressureJobStatus> {
        self.validate_context(world)?;
        for _ in 0..budget {
            if self.stage == 5 {
                break;
            }
            self.unit()?;
            self.work = self
                .work
                .checked_add(1)
                .ok_or_else(|| invalid("広域圧力の仕事量上限"))?;
        }
        Ok(self.status())
    }
    // 復元時に連結成分を一度だけ構築し、保存した探索状態を検査します。
    // Jacobiの過去の反復は再実行せず、保存した数値状態からの進捗を維持します。
    fn validate_progress(&self) -> EngineResult<()> {
        let n = self.nodes.len();
        if self.stage <= 3 && self.iteration != 0 || self.stage == 5 && self.cursor != 0 {
            return Err(invalid("広域圧力の段階と反復数"));
        }
        if self.stage == 0 {
            if self.root != 0
                || !self.queue.is_empty()
                || self.queue_cursor != 0
                || self.nodes.iter().any(|node| {
                    node.seen
                        || node.fixed.is_some()
                        || node.source.is_some()
                        || node.old != node.input.pressure
                        || node.next != 0
                })
                || self.total_pressure != 0
                || self.total_gas != 0
                || self.open
                || self.has_gas
                || self.target != 0
                || self.report.open_components != 0
                || self.report.closed_components != 0
            {
                return Err(invalid("広域圧力の初期探索状態"));
            }
            return Ok(());
        }
        if matches!(self.stage, 2 | 3) && self.cursor != self.root {
            return Err(invalid("広域圧力の探索root"));
        }
        let mut partition = Vec::new();
        partition.try_reserve_exact(n).map_err(allocation)?;
        partition.resize(n, false);
        let mut bfs = Vec::new();
        bfs.try_reserve_exact(n).map_err(allocation)?;
        let mut last_completed = None;
        let mut previous_target = 0;
        let mut selected = false;
        let mut report = PressureRelaxationReport {
            cells: self.report.cells,
            ..Default::default()
        };
        for root in 0..n {
            if partition[root] {
                continue;
            }
            bfs.clear();
            bfs.push(root as u32);
            partition[root] = true;
            let current = matches!(self.stage, 2 | 3) && root == self.root;
            let mut discovered = 1;
            let mut cursor = 0;
            while cursor < bfs.len() {
                if current && self.stage == 2 && cursor == self.queue_cursor {
                    discovered = bfs.len();
                }
                let node = &self.nodes[bfs[cursor] as usize];
                for other in node.edges.into_iter().flatten() {
                    if !partition[other as usize] {
                        partition[other as usize] = true;
                        bfs.push(other);
                    }
                }
                cursor += 1;
            }
            if current && self.stage == 2 && cursor == self.queue_cursor {
                discovered = bfs.len();
            }
            let completed = match self.stage {
                1 => root < self.cursor,
                2 | 3 => root < self.root,
                4..=6 => true,
                _ => false,
            };
            let reflected = completed || current && self.stage == 3;
            let summary = if reflected {
                self.summary(&bfs)?
            } else {
                PressureSummary::default()
            };
            let target = if reflected {
                summary.target(self.nodes[root].input.kind, bfs.len())?
            } else {
                0
            };
            if reflected {
                if self.nodes[root].input.kind == 1 {
                    if summary.open {
                        report.open_components += 1;
                    } else {
                        report.closed_components += 1;
                    }
                } else if summary.has_gas {
                    report.closed_components += 1;
                }
            }
            for (position, index) in bfs.iter().enumerate() {
                let node = &self.nodes[*index as usize];
                let seen = completed || current && (self.stage == 3 || position < discovered);
                let assigned =
                    completed || current && self.stage == 3 && position < self.queue_cursor;
                let fixed = if !assigned {
                    None
                } else if node.input.kind == 0 {
                    Some(0)
                } else if node.input.surface
                    || (node.input.kind == 1 && *index as usize == root && !summary.open)
                {
                    Some(target)
                } else {
                    None
                };
                let source = if assigned && node.input.kind == 2 {
                    Some(target)
                } else {
                    None
                };
                if node.seen != seen || node.fixed != fixed || node.source != source {
                    return Err(invalid("広域圧力の成分と訪問・source状態"));
                }
                if self.stage <= 3 && (node.old != node.input.pressure || node.next != 0) {
                    return Err(invalid("広域圧力の反復前の数値状態"));
                }
            }
            let retained = (self.stage == 1 || self.stage >= 4) && completed && root == self.root;
            if current || retained {
                selected = true;
                let expected = if current && self.stage == 2 {
                    if self.queue_cursor > bfs.len() {
                        return Err(invalid("広域圧力の探索cursor"));
                    }
                    &bfs[..discovered]
                } else {
                    bfs.as_slice()
                };
                if self.queue != expected
                    || (!current || self.stage != 2)
                        && (self.stage != 3 && self.queue_cursor != bfs.len())
                    || self.stage == 3 && self.queue_cursor > bfs.len()
                {
                    return Err(invalid("広域圧力の成分とqueue"));
                }
                let expected_summary = if current && self.stage == 2 {
                    self.summary(&bfs[..self.queue_cursor])?
                } else {
                    summary
                };
                let expected_target = if current && self.stage == 2 {
                    previous_target
                } else {
                    target
                };
                if self.total_pressure != expected_summary.pressure
                    || self.total_gas != expected_summary.gas
                    || self.open != expected_summary.open
                    || self.has_gas != expected_summary.has_gas
                    || self.target != expected_target
                {
                    return Err(invalid("広域圧力の探索集計とtarget"));
                }
            }
            if completed {
                last_completed = Some(root);
                previous_target = target;
            }
        }
        if report != self.report {
            return Err(invalid("広域圧力の成分と診断件数"));
        }
        if matches!(self.stage, 2 | 3) {
            if !selected {
                return Err(invalid("広域圧力の成分rootが存在しない"));
            }
        } else if let Some(last) = last_completed {
            if self.root != last || !selected {
                return Err(invalid("広域圧力の最後の成分"));
            }
        } else if self.root != 0
            || !self.queue.is_empty()
            || self.queue_cursor != 0
            || self.total_pressure != 0
            || self.total_gas != 0
            || self.open
            || self.has_gas
            || self.target != 0
        {
            return Err(invalid("広域圧力の未探索状態"));
        }
        // 現反復のprefixだけを検査します。stage 6ではコピーした旧値は失われるため、
        // コピー済みprefixのold==nextと、初回の未コピーsuffixの初期値を照合します。
        for (index, node) in self.nodes.iter().enumerate() {
            match self.stage {
                4 => {
                    let expected = if index < self.cursor {
                        self.next_pressure(index)?
                    } else if self.iteration == 0 {
                        0
                    } else {
                        node.old
                    };
                    if node.next != expected
                        || self.iteration == 0 && node.old != node.input.pressure
                    {
                        return Err(invalid("保存したJacobiの数値prefix"));
                    }
                }
                6 => {
                    if index < self.cursor && node.old != node.next
                        || self.iteration == 0
                            && index >= self.cursor
                            && node.old != node.input.pressure
                        || self.cursor == 0 && node.next != self.next_pressure(index)?
                    {
                        return Err(invalid("保存した圧力buffer切替のprefix"));
                    }
                }
                5 if node.old != node.next => return Err(invalid("完成した圧力bufferの不一致")),
                _ => (),
            }
        }
        Ok(())
    }
    fn summary(&self, queue: &[u32]) -> EngineResult<PressureSummary> {
        let mut result = PressureSummary::default();
        for index in queue {
            let input = self.nodes[*index as usize].input;
            result.pressure = result
                .pressure
                .checked_add(input.pressure as u128)
                .ok_or_else(|| invalid("圧力集計上限"))?;
            result.gas = result
                .gas
                .checked_add(
                    u128::from(input.mass)
                        * input.temperature as u128
                        * u128::from(input.coefficient),
                )
                .ok_or_else(|| invalid("気体集計上限"))?;
            result.open |= input.surface;
            result.has_gas |= input.kind == 2;
        }
        Ok(result)
    }
    fn validate_context(&self, world: &World) -> EngineResult<()> {
        if self.world != world.id {
            return Err(EngineError::new(ErrorCode::WrongScope, "広域圧力のWorld"));
        }
        if self.content != world.content.fingerprint()
            || self.epoch != world.remaps.epoch()
            || self.chunks.len() != world.chunks.values().filter(|chunk| chunk.active).count()
            || self.chunks.iter().any(|(coord, revision)| {
                !world
                    .chunks
                    .get(coord)
                    .is_some_and(|chunk| chunk.active && chunk.revision == *revision)
            })
        {
            return Err(conflict("広域圧力の入力失効"));
        }
        Ok(())
    }
    /// 一つのJacobi値。実行と保存prefix検査で同じ式を使い、丸め規則を分岐させません。
    fn next_pressure(&self, index: usize) -> EngineResult<i64> {
        let node = self.nodes[index];
        let pressure = if let Some(fixed) = node.fixed {
            fixed
        } else {
            let mut sum = 0i128;
            let mut count = 0;
            for other in node.edges.into_iter().flatten() {
                let other = self.nodes[other as usize];
                let density = (i128::from(node.input.mass) + i128::from(other.input.mass)) / 2;
                sum += i128::from(other.old)
                    + density
                        * i128::from(self.config.relaxation.gravity_q16)
                        * (i128::from(node.input.point.y) - i128::from(other.input.point.y));
                count += 1;
            }
            if let Some(source) = node.source {
                let weight = i128::from(node.input.weight);
                sum += weight * i128::from(source);
                count += weight;
            }
            if count == 0 {
                node.old
            } else {
                i64::try_from((sum / count).max(0)).map_err(|_| invalid("広域圧力のi64上限"))?
            }
        };
        let value = pressure.max(node.input.pump);
        if value > node.input.maximum {
            return Err(invalid("素材の圧力上限"));
        }
        Ok(value)
    }
    // 一回の処理は最大四辺だけを触ります。stageの切替自体も一件として数えます。
    fn unit(&mut self) -> EngineResult<()> {
        let n = self.nodes.len();
        match self.stage {
            0 if self.cursor < n => {
                let input = self.nodes[self.cursor].input;
                let mut edges = [None; 4];
                for (edge, (dx, dy)) in NEIGHBORS.into_iter().enumerate() {
                    if let Some(point) = offset(input.point, dx, dy)
                        && let Ok(other) = self
                            .nodes
                            .binary_search_by_key(&(point.x, point.y), |node| {
                                (node.input.point.x, node.input.point.y)
                            })
                        && ((input.kind == 1 && self.nodes[other].input.kind == 1)
                            || (input.kind != 1 && self.nodes[other].input.kind != 1))
                    {
                        edges[edge] = Some(other as u32);
                    }
                }
                self.nodes[self.cursor].edges = edges;
                self.cursor += 1;
            }
            0 => {
                self.stage = 1;
                self.cursor = 0;
            }
            1 if self.cursor == n => {
                self.stage = 4;
                self.cursor = 0;
            }
            1 if self.nodes[self.cursor].seen => {
                self.cursor += 1;
            }
            1 => {
                self.root = self.cursor;
                self.queue.clear();
                self.queue.push(self.root as u32);
                self.nodes[self.root].seen = true;
                self.queue_cursor = 0;
                self.total_pressure = 0;
                self.total_gas = 0;
                self.open = false;
                self.has_gas = false;
                self.stage = 2;
            }
            2 if self.queue_cursor < self.queue.len() => {
                let index = self.queue[self.queue_cursor] as usize;
                let node = self.nodes[index];
                // u32質量×非負i64温度×u32係数は127bit以内です。領域全体の合計は別に検査します。
                let gas = self
                    .total_gas
                    .checked_add(
                        u128::from(node.input.mass)
                            * node.input.temperature as u128
                            * u128::from(node.input.coefficient),
                    )
                    .ok_or_else(|| invalid("気体温度質量の積算上限"))?;
                let pressure = self
                    .total_pressure
                    .checked_add(node.input.pressure as u128)
                    .ok_or_else(|| invalid("広域圧力の合計上限"))?;
                self.total_pressure = pressure;
                self.has_gas |= node.input.kind == 2;
                self.total_gas = gas;
                self.open |= node.input.surface;
                for other in node.edges.into_iter().flatten() {
                    if !self.nodes[other as usize].seen {
                        self.queue.push(other);
                        self.nodes[other as usize].seen = true;
                    }
                }
                self.queue_cursor += 1;
            }
            2 => {
                let target = if self.nodes[self.root].input.kind == 1 {
                    if self.open {
                        0
                    } else {
                        i64::try_from(self.total_pressure / self.queue.len() as u128)
                            .map_err(|_| invalid("閉領域圧力上限"))?
                    }
                } else {
                    i64::try_from(self.total_gas / (self.queue.len() as u128 * 65536))
                        .map_err(|_| invalid("気体sourceのi64上限"))?
                };
                self.target = target;
                if self.nodes[self.root].input.kind == 1 {
                    if self.open {
                        self.report.open_components += 1;
                    } else {
                        self.report.closed_components += 1;
                    }
                } else if self.has_gas {
                    // 気体空間は活動境界でno-flowです。空だけの空間を流体領域として数えません。
                    self.report.closed_components += 1;
                }
                self.stage = 3;
                self.queue_cursor = 0;
            }
            3 if self.queue_cursor < self.queue.len() => {
                let index = self.queue[self.queue_cursor] as usize;
                let node = &mut self.nodes[index];
                node.fixed = if node.input.kind == 0 {
                    // 空間は相対圧0です。targetを近傍境界から注入するとlambda=0を迂回します。
                    Some(0)
                } else if node.input.surface
                    || (node.input.kind == 1 && index == self.root && !self.open)
                {
                    Some(self.target)
                } else {
                    None
                };
                node.source = if node.input.kind == 2 {
                    Some(self.target)
                } else {
                    None
                };
                self.queue_cursor += 1;
            }
            3 => {
                self.stage = 1;
                self.cursor = self.root + 1;
            }
            4 if self.cursor < n => {
                let value = self.next_pressure(self.cursor)?;
                self.nodes[self.cursor].next = value;
                self.cursor += 1;
            }
            4 => {
                // 二重bufferをnode内に持つためswapは全nodeを走査せず、次stageで一件ずつ行います。
                self.stage = 6;
                self.cursor = 0;
            }
            6 if self.cursor < n => {
                self.nodes[self.cursor].old = self.nodes[self.cursor].next;
                self.cursor += 1;
            }
            6 => {
                self.iteration += 1;
                self.cursor = 0;
                self.stage = if self.iteration == self.config.relaxation.iterations {
                    5
                } else {
                    4
                };
            }
            5 => (),
            _ => return Err(invalid("広域圧力の計算段階")),
        }
        Ok(())
    }
}

impl WorldEdit<'_> {
    /// 完成したjobを、入力・最終容量の再検査後に一括公開します。
    /// 失敗を無視しても外側の編集全体が破棄されます。公開後のrevisionで二重公開も拒否します。
    pub fn publish_pressure_job(
        &mut self,
        job: &PressureJob,
    ) -> EngineResult<PressureRelaxationReport> {
        self.run(|editor| {
            job.validate_context(editor.world)?;
            if job.status() != PressureJobStatus::Ready {
                return Err(conflict("未完成の広域圧力"));
            }
            let layer = editor.world.content.layer_id(job.layer)?;
            // 復元を含め入力の完全性を再検査します。保存された数値状態は正規状態として
            // 継続し、過去の全反復をやり直しません。保存buffer自体の整合は保存層が扱います。
            let checked = PressureJob::new(editor.world, layer, job.config)?;
            if checked.nodes.len() != job.nodes.len()
                || checked
                    .nodes
                    .iter()
                    .zip(&job.nodes)
                    .any(|(a, b)| a.input != b.input)
            {
                return Err(conflict("広域圧力の保存入力が不一致"));
            }
            let channel = job.config.relaxation.channel;
            let origin = editor.world.channel_layout::<i64>(channel)?.origin;
            let mut updates = Vec::new();
            updates
                .try_reserve_exact(job.report.cells)
                .map_err(allocation)?;
            for node in &job.nodes {
                if node.input.kind != 0 {
                    updates.push((channel_point(node.input.point, origin)?, node.old));
                }
            }
            let default = editor.world.validate_channel_updates(channel, &updates)?;
            for clearing in [true, false] {
                for &(point, value) in &updates {
                    if (value == default) == clearing {
                        editor.set_channel(channel, point, value)?;
                    }
                }
            }
            Ok(job.report)
        })
    }
}

const NEIGHBORS: [(i32, i32); 4] = [(0, -1), (-1, 0), (1, 0), (0, 1)];
fn offset(point: CellPoint, dx: i32, dy: i32) -> Option<CellPoint> {
    Some(CellPoint {
        x: point.x.checked_add(dx)?,
        y: point.y.checked_add(dy)?,
    })
}
fn channel_point(point: CellPoint, origin: CellPoint) -> EngineResult<CellPoint> {
    Ok(CellPoint {
        x: point
            .x
            .checked_sub(origin.x)
            .ok_or_else(|| invalid("圧力channelのx座標"))?,
        y: point
            .y
            .checked_sub(origin.y)
            .ok_or_else(|| invalid("圧力channelのy座標"))?,
    })
}
fn check_channel(
    world: &World,
    key: StableKey,
    material_layer: Option<LayerId>,
) -> EngineResult<()> {
    let layout = world.channel_layout::<i64>(key)?;
    let owner = world.channel_material_layer::<i64>(key)?;
    if layout.x_scale.ratio() != (1, 1)
        || layout.y_scale.ratio() != (1, 1)
        || (owner.is_some()
            && owner
                != material_layer
                    .map(|layer| world.content.layer(layer).map(|definition| definition.key))
                    .transpose()?)
    {
        return Err(invalid("広域圧力のchannel格子・所有layer"));
    }
    Ok(())
}
fn read(world: &World, key: StableKey, point: CellPoint) -> EngineResult<i64> {
    let point = channel_point(point, world.channel_layout::<i64>(key)?.origin)?;
    match world.channel_value::<i64>(key, point)? {
        ChannelQuery::Loaded(value) if value >= 0 => Ok(value),
        _ => Err(invalid("広域圧力の負値・coverage")),
    }
}
fn active_empty(world: &World, layer: LayerId, point: CellPoint) -> EngineResult<bool> {
    Ok(world
        .chunks
        .get(&ChunkCoord::at(point))
        .is_some_and(|chunk| chunk.active)
        && matches!(world.cell(layer, point)?, CellQuery::Loaded(cell) if cell.block.is_none() && cell.core.material == MaterialId::AIR))
}

impl StateCodec for PressureJob {
    fn encode(&self, out: &mut Encoder) -> EngineResult<()> {
        out.raw(b"CHPRJOB3")?;
        self.world.encode(out)?;
        out.raw(&self.content)?;
        out.u64(self.epoch)?;
        self.layer.encode(out)?;
        self.config.relaxation.channel.encode(out)?;
        out.u16(self.config.relaxation.iterations)?;
        out.u32(self.config.relaxation.gravity_q16)?;
        out.u32(self.config.relaxation.cell_limit as u32)?;
        out.u8(u8::from(self.config.gas.is_some()))?;
        if let Some(gas) = self.config.gas {
            gas.temperature.encode(out)?;
        }
        self.config.pump.encode(out)?;
        out.u32(self.chunks.len() as u32)?;
        for (coord, revision) in &self.chunks {
            coord.encode(out)?;
            out.u64(*revision)?;
        }
        out.u32(self.nodes.len() as u32)?;
        for node in &self.nodes {
            let input = node.input;
            input.point.encode(out)?;
            input.material.encode(out)?;
            out.u8(input.kind)?;
            out.u32(input.mass)?;
            out.i64(input.pressure)?;
            out.i64(input.temperature)?;
            out.i64(input.pump)?;
            out.u8(u8::from(input.surface))?;
            out.u32(input.coefficient)?;
            out.u32(input.weight)?;
            out.i64(input.maximum)?;
            for edge in node.edges {
                edge.encode(out)?;
            }
            out.u8(u8::from(node.seen))?;
            node.fixed.encode(out)?;
            node.source.encode(out)?;
            out.i64(node.old)?;
            out.i64(node.next)?;
        }
        out.u8(self.stage)?;
        out.u32(self.cursor as u32)?;
        out.u32(self.root as u32)?;
        out.u32(self.queue.len() as u32)?;
        for index in &self.queue {
            out.u32(*index)?;
        }
        out.u32(self.queue_cursor as u32)?;
        out.raw(&self.total_pressure.to_le_bytes())?;
        out.raw(&self.total_gas.to_le_bytes())?;
        out.u8(u8::from(self.open))?;
        out.u8(u8::from(self.has_gas))?;
        out.i64(self.target)?;
        out.u16(self.iteration)?;
        out.u64(self.work)?;
        out.u32(self.report.cells as u32)?;
        out.u32(self.report.open_components as u32)?;
        out.u32(self.report.closed_components as u32)
    }
    fn decode(input: &mut Decoder<'_>) -> EngineResult<Self> {
        if input.raw(8)? != b"CHPRJOB3" {
            return Err(invalid("広域圧力recordのmagic"));
        }
        let world = WorldId::decode(input)?;
        let content = input
            .raw(32)?
            .try_into()
            .map_err(|_| invalid("圧力Content"))?;
        let epoch = input.u64()?;
        let layer = StableKey::decode(input)?;
        let relaxation = PressureRelaxation {
            channel: StableKey::decode(input)?,
            iterations: input.u16()?,
            gravity_q16: input.u32()?,
            cell_limit: input.u32()? as usize,
        };
        let gas = if boolean(input)? {
            Some(GasPressureSource {
                temperature: StableKey::decode(input)?,
            })
        } else {
            None
        };
        let pump = Option::<StableKey>::decode(input)?;
        let count = record_count(input, 16)?;
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            chunks.push((ChunkCoord::decode(input)?, input.u64()?));
        }
        let count = record_count(input, 56)?;
        if count > relaxation.cell_limit {
            return Err(invalid("広域圧力の保存cell上限"));
        }
        let mut nodes = Vec::new();
        nodes.try_reserve_exact(count).map_err(allocation)?;
        for _ in 0..count {
            let point = CellPoint::decode(input)?;
            let material = MaterialId::decode(input)?;
            let kind = input.u8()?;
            let mass = input.u32()?;
            let pressure = input.i64()?;
            let temperature = input.i64()?;
            let pump = input.i64()?;
            let surface = boolean(input)?;
            let coefficient = input.u32()?;
            let weight = input.u32()?;
            let maximum = input.i64()?;
            let mut edges = [None; 4];
            for edge in &mut edges {
                *edge = Option::<u32>::decode(input)?;
            }
            let seen = boolean(input)?;
            let fixed = Option::<i64>::decode(input)?;
            let source = Option::<i64>::decode(input)?;
            let old = input.i64()?;
            let next = input.i64()?;
            if kind > 2
                || [pressure, temperature, pump, old, next]
                    .iter()
                    .any(|value| *value < 0)
                || fixed.is_some_and(|value| value < 0)
                || source.is_some_and(|value| value < 0)
                || maximum < 0
                || (kind == 2
                    && (pressure > maximum || old > maximum || next > maximum || fixed.is_some()))
                || (kind != 2
                    && (coefficient != 0 || weight != 0 || maximum != i64::MAX || source.is_some()))
                || (kind != 2 && temperature != 0)
                || (kind == 0
                    && (pressure != 0
                        || old != 0
                        || next != 0
                        || pump != 0
                        || fixed.is_some_and(|value| value != 0)))
                || edges.iter().flatten().any(|index| *index as usize >= count)
                || (kind == 0 && (material != MaterialId::AIR || mass != 0))
                || (kind != 0 && (material == MaterialId::AIR || mass == 0))
            {
                return Err(invalid("広域圧力の保存node"));
            }
            nodes.push(Node {
                input: Input {
                    point,
                    material,
                    kind,
                    mass,
                    pressure,
                    temperature,
                    pump,
                    surface,
                    coefficient,
                    weight,
                    maximum,
                },
                edges,
                seen,
                fixed,
                source,
                old,
                next,
            });
        }
        let stage = input.u8()?;
        let cursor = input.u32()? as usize;
        let root = input.u32()? as usize;
        let size = count_queue(input, nodes.len())?;
        let mut queue = Vec::new();
        queue.try_reserve_exact(nodes.len()).map_err(allocation)?;
        for _ in 0..size {
            let index = input.u32()?;
            if index as usize >= nodes.len() {
                return Err(invalid("広域圧力queue index"));
            }
            queue.push(index);
        }
        let queue_cursor = input.u32()? as usize;
        let total_pressure =
            u128::from_le_bytes(input.raw(16)?.try_into().map_err(|_| invalid("圧力合計"))?);
        let total_gas =
            u128::from_le_bytes(input.raw(16)?.try_into().map_err(|_| invalid("気体合計"))?);
        let open = boolean(input)?;
        let has_gas = boolean(input)?;
        let target = input.i64()?;
        let iteration = input.u16()?;
        let work = input.u64()?;
        let report = PressureRelaxationReport {
            cells: input.u32()? as usize,
            open_components: input.u32()? as usize,
            closed_components: input.u32()? as usize,
        };
        if stage > 6
            || cursor > nodes.len()
            || root > nodes.len()
            || (root == nodes.len() && !nodes.is_empty())
            || queue_cursor > queue.len()
            || target < 0
            || relaxation.iterations == 0
            || work > (2 * u64::from(relaxation.iterations) + 8) * (nodes.len() as u64 + 1) + 5
            || iteration > relaxation.iterations
            || (stage != 5 && iteration == relaxation.iterations)
            || nodes.windows(2).any(|pair| {
                (pair[0].input.point.x, pair[0].input.point.y)
                    >= (pair[1].input.point.x, pair[1].input.point.y)
            })
            || chunks.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || report.cells != nodes.iter().filter(|node| node.input.kind != 0).count()
            || report
                .open_components
                .checked_add(report.closed_components)
                .is_none_or(|total| total > report.cells)
            || (gas.is_none() && nodes.iter().any(|node| node.input.kind != 1))
            || (stage == 5 && iteration != relaxation.iterations)
            || ((stage == 2 || stage == 3) && queue.is_empty())
        {
            return Err(invalid("広域圧力の保存進捗"));
        }
        // 保存された隣接索引も座標と分類から照合し、非隣接・欠落した辺を再開しません。
        for (index, node) in nodes.iter().enumerate() {
            let mut expected = [None; 4];
            if stage != 0 || index < cursor {
                for (edge, (dx, dy)) in NEIGHBORS.into_iter().enumerate() {
                    if let Some(point) = offset(node.input.point, dx, dy)
                        && let Ok(other) = nodes.binary_search_by_key(&(point.x, point.y), |node| {
                            (node.input.point.x, node.input.point.y)
                        })
                        && ((node.input.kind == 1) == (nodes[other].input.kind == 1))
                    {
                        expected[edge] = Some(other as u32);
                    }
                }
            }
            if node.edges != expected {
                return Err(invalid("広域圧力の保存隣接索引"));
            }
        }
        // queue内の重複は探索費用と合計を壊すため、確保済みのbit列で一度検査します。
        let mut seen = Vec::new();
        seen.try_reserve_exact(nodes.len()).map_err(allocation)?;
        seen.resize(nodes.len(), false);
        for index in &queue {
            if seen[*index as usize] {
                return Err(invalid("広域圧力queueの重複"));
            }
            seen[*index as usize] = true;
            if !nodes[*index as usize].seen {
                return Err(invalid("広域圧力queueの訪問状態"));
            }
        }
        let job = Self {
            world,
            content,
            epoch,
            layer,
            config: PressureJobConfig {
                relaxation,
                gas,
                pump,
            },
            chunks,
            nodes,
            stage,
            cursor,
            root,
            queue,
            queue_cursor,
            total_pressure,
            total_gas,
            open,
            has_gas,
            target,
            iteration,
            work,
            report,
        };
        job.validate_progress()?;
        Ok(job)
    }
}
fn boolean(input: &mut Decoder<'_>) -> EngineResult<bool> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid("広域圧力bool")),
    }
}
fn record_count(input: &mut Decoder<'_>, minimum: usize) -> EngineResult<usize> {
    let count = input.u32()? as usize;
    if count > input.remaining() / minimum {
        return Err(invalid("広域圧力record件数"));
    }
    Ok(count)
}
fn count_queue(input: &mut Decoder<'_>, limit: usize) -> EngineResult<usize> {
    let count = record_count(input, 4)?;
    if count > limit {
        return Err(invalid("広域圧力queue件数"));
    }
    Ok(count)
}
