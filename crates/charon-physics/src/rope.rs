//! 仕様 10 §9.1: 重力でしなり、地形に食い込まないワイヤー。位置はセル単位の `f32`。
//!
//! 剛体と同じく、浮動小数点を使う `charon-physics` の物理。同一プラットフォーム・同一ビルドでは
//! 同じ入力から同じ結果になるが、異なる環境の一致は主張しない。

/// 1 tick に先端を進める最大の距離 (セル)。すり抜けないように小刻みに進める。
const SUBSTEP: f32 = 0.5;
/// 速度の減衰 (空気抵抗)。
const DAMPING: f32 = 0.995;
/// 張っているとみなす余裕 (セル)。
const TAUT_SLACK: f32 = 0.05;
/// 引く向きを決めるとき、手からこの距離 (セル) 以上離れた最初の点を使う。
const PULL_REACH: f32 = 3.0;

/// ワイヤーの設定。長さはセル、速さはセル / tick、重力はセル / tick²。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RopeParams {
    /// セグメントの数 (点は `segments + 1` 個)。
    pub segments: usize,
    pub max_length: f32,
    pub min_length: f32,
    pub gravity: f32,
    /// 拘束を解く回数。
    pub iterations: usize,
    /// 巻き取りで 1 tick に縮める長さ。
    pub reel_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RopePhase {
    /// 先端が飛んでいる。
    Flying,
    /// 先端が地形に着いて止まっている。
    Anchored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RopeEvent {
    None,
    /// この tick に先端が地形に着いた。
    Anchored,
    /// 最大の長さを超えた。呼び出し側がワイヤーを外す。
    Lost,
}

type Vec2 = [f32; 2];

fn sub(a: Vec2, b: Vec2) -> Vec2 {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Vec2, b: Vec2) -> Vec2 {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: Vec2, k: f32) -> Vec2 {
    [a[0] * k, a[1] * k]
}

fn length(a: Vec2) -> f32 {
    a[0].hypot(a[1])
}

#[derive(Clone, Debug)]
pub struct Rope {
    points: Vec<Vec2>,
    previous: Vec<Vec2>,
    phase: RopePhase,
    rest: f32,
    head_velocity: Vec2,
}

impl Rope {
    /// `hand` から `direction` の向きに、速さ `speed` (セル / tick) で撃つ。
    /// `direction` が 0 または有限でないとき、`speed` が正でないときは `None`。
    pub fn launch(hand: Vec2, direction: Vec2, speed: f32, params: &RopeParams) -> Option<Self> {
        let len = length(direction);
        let finite = hand.iter().chain(direction.iter()).all(|v| v.is_finite());
        if !finite || len <= 0.0 || !(speed.is_finite() && speed > 0.0) || params.segments < 1 {
            return None;
        }
        let count = params.segments + 1;
        Some(Self {
            points: vec![hand; count],
            previous: vec![hand; count],
            phase: RopePhase::Flying,
            rest: 0.0,
            head_velocity: scale(direction, speed / len),
        })
    }

    pub fn points(&self) -> &[[f32; 2]] {
        &self.points
    }

    pub fn phase(&self) -> RopePhase {
        self.phase
    }

    /// 静止長 (張ったときの長さ)。`Anchored` のときだけ意味がある。
    pub fn rest_length(&self) -> f32 {
        self.rest
    }

    /// 点を結ぶ折れ線の長さ。
    pub fn polyline_length(&self) -> f32 {
        self.points.windows(2).map(|w| length(sub(w[1], w[0]))).sum()
    }

    /// `Anchored` のときの先端。
    pub fn anchor(&self) -> Option<[f32; 2]> {
        (self.phase == RopePhase::Anchored).then(|| self.points[self.points.len() - 1])
    }

    /// 1 tick 進める (仕様 10 §9.1)。`solid(p)` は点 `p` のいるセルが地形か。
    pub fn step(
        &mut self,
        hand: Vec2,
        reel: bool,
        params: &RopeParams,
        solid: &mut dyn FnMut(Vec2) -> bool,
    ) -> RopeEvent {
        let last = self.points.len() - 1;
        let mut event = RopeEvent::None;

        match self.phase {
            RopePhase::Flying => {
                self.head_velocity[1] += params.gravity;
                let from = self.points[last];
                let v = self.head_velocity;
                let steps = (length(v) / SUBSTEP).ceil().max(1.0) as usize;
                let mut position = from;
                let mut hit = false;
                for s in 1..=steps {
                    let candidate = add(from, scale(v, s as f32 / steps as f32));
                    if solid(candidate) {
                        hit = true;
                        break;
                    }
                    position = candidate;
                }
                self.points[last] = position;
                self.previous[last] = position;
                self.points[0] = hand;
                self.previous[0] = hand;
                // 飛行中の途中の点は、手と先端の間に並べ直す (先端の動きに遅れてたまらない)。
                for i in 1..last {
                    let t = i as f32 / last as f32;
                    let p = add(hand, scale(sub(position, hand), t));
                    self.points[i] = p;
                    self.previous[i] = p;
                }
                if hit {
                    self.phase = RopePhase::Anchored;
                    self.rest = self.polyline_length().clamp(params.min_length, params.max_length);
                    event = RopeEvent::Anchored;
                } else if length(sub(hand, position)) > params.max_length {
                    return RopeEvent::Lost;
                }
            }
            RopePhase::Anchored => {
                if reel {
                    self.rest = (self.rest - params.reel_speed).max(params.min_length);
                }
            }
        }

        // 手は動かさない。先端は飛行中も着地後も、この tick では動かさない。
        self.points[0] = hand;
        self.previous[0] = hand;
        let segments = params.segments.max(1) as f32;
        let seg = match self.phase {
            RopePhase::Flying => length(sub(hand, self.points[last])).max(0.01) * 1.03 / segments,
            RopePhase::Anchored => self.rest / segments,
        };

        // Verlet 法で、手と先端以外の点を重力で動かす。
        for i in 1..last {
            let p = self.points[i];
            let velocity = scale(sub(p, self.previous[i]), DAMPING);
            let next = add(add(p, velocity), [0.0, params.gravity]);
            self.previous[i] = p;
            self.points[i] = if solid(next) { p } else { next };
        }

        // 伸びた分だけ縮める拘束 (縮んでいるときは押し返さない)。
        for _ in 0..params.iterations.max(1) {
            for i in 0..last {
                let (a, b) = (self.points[i], self.points[i + 1]);
                let d = sub(b, a);
                let len = length(d);
                if len <= seg || len < 1e-6 {
                    continue;
                }
                let (wa, wb) = (f32::from(i != 0), f32::from(i + 1 != last));
                if wa + wb == 0.0 {
                    continue;
                }
                let correction = scale(d, (len - seg) / len / (wa + wb));
                self.points[i] = add(a, scale(correction, wa));
                self.points[i + 1] = sub(b, scale(correction, wb));
            }
        }

        // 地形の中に入った点は、動かす前の位置に戻して止める。
        for i in 1..last {
            if solid(self.points[i]) {
                self.points[i] = self.previous[i];
            }
        }
        event
    }

    /// 全点・phase・静止長のビット列を FNV-1a 64-bit で流す。
    pub fn state_hash(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        let mut feed = |bytes: &[u8]| {
            for &byte in bytes {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        };
        feed(&[u8::from(self.phase == RopePhase::Anchored)]);
        feed(&self.rest.to_bits().to_le_bytes());
        for p in self.points.iter().chain(self.previous.iter()) {
            feed(&p[0].to_bits().to_le_bytes());
            feed(&p[1].to_bits().to_le_bytes());
        }
        feed(&self.head_velocity[0].to_bits().to_le_bytes());
        feed(&self.head_velocity[1].to_bits().to_le_bytes());
        hash
    }
}

/// ワイヤーでつながれた質点の速度 (セル / tick) を、拘束で直す (仕様 10 §9.1)。
///
/// `Anchored` のとき、(1) 張っていて (手とアンカーの直線距離が `rest_length() - 0.05` 以上)、
/// `reel` が真なら、引く向きへ `pull_accel` を足し、(2) 次の位置 `hand + velocity` が
/// アンカーを中心とする半径 `rest_length()` の円の外に出るなら、円の上に戻す (伸びない)。
/// そのあと、大きさを `speed_cap` 以下に収める。飛行中や、次の位置が円の中なら、`reel` による
/// 加速のほかは何も変えない。
pub fn tether_velocity(
    rope: &Rope,
    hand: [f32; 2],
    velocity: [f32; 2],
    reel: bool,
    pull_accel: f32,
    speed_cap: f32,
) -> [f32; 2] {
    if rope.phase != RopePhase::Anchored {
        return velocity;
    }
    let anchor = rope.points[rope.points.len() - 1];
    let chord = length(sub(anchor, hand));
    let mut v = velocity;
    if reel && chord >= rope.rest - TAUT_SLACK {
        // 引く向き: 手から 3 セル以上離れた最初の点へ (なければアンカーへ)。
        let target = rope.points[1..]
            .iter()
            .copied()
            .find(|&p| length(sub(p, hand)) >= PULL_REACH)
            .unwrap_or(anchor);
        let toward = sub(target, hand);
        let len = length(toward);
        if len > 1e-6 {
            v = add(v, scale(toward, pull_accel / len));
        }
    }
    // 次の位置が長さの円の外なら、円の上に戻す。
    let next = add(hand, v);
    let offset = sub(next, anchor);
    let distance = length(offset);
    if distance > rope.rest && distance > 1e-6 {
        let on_circle = add(anchor, scale(offset, rope.rest / distance));
        v = sub(on_circle, hand);
    }
    let speed = length(v);
    if speed > speed_cap && speed > 0.0 {
        v = scale(v, speed_cap / speed);
    }
    v
}
