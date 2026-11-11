//! 仕様 06 §7: 2 本指の移動と距離の変化をカメラ操作にする。

use std::collections::BTreeMap;

use winit::event::TouchPhase;

use crate::ui::Controls;

#[derive(Debug, Default)]
pub(super) struct TouchGestures {
    fingers: BTreeMap<u64, [f32; 2]>,
    pinch: Option<(f32, u32)>,
    active: bool,
}

impl TouchGestures {
    /// true の間はブラシ操作として扱わない。
    pub(super) fn update(
        &mut self,
        id: u64,
        phase: TouchPhase,
        pos: [f32; 2],
        controls: &mut Controls,
    ) -> bool {
        let previous = self.pair();
        match phase {
            TouchPhase::Started | TouchPhase::Moved => {
                self.fingers.insert(id, pos);
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.fingers.remove(&id);
            }
        }
        let next = self.pair();
        if let Some((a, b)) = next {
            self.active = true;
            let distance = distance(a, b);
            if self.pinch.is_none() {
                self.pinch = Some((distance, controls.zoom));
            }
            if let Some((old_a, old_b)) = previous {
                let prev_center = [(old_a[0] + old_b[0]) * 0.5, (old_a[1] + old_b[1]) * 0.5];
                let center = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                controls.origin[0] -= (center[0] - prev_center[0]) / controls.zoom as f32;
                controls.origin[1] -= (center[1] - prev_center[1]) / controls.zoom as f32;
            }
            if let Some((baseline, zoom)) = self.pinch
                && baseline > 0.0
            {
                controls.zoom = (zoom as f32 * distance / baseline).round().clamp(1.0, 8.0) as u32;
            }
        } else {
            self.pinch = None;
        }
        let consumed = self.active;
        if self.fingers.is_empty() {
            self.active = false;
        }
        consumed
    }

    fn pair(&self) -> Option<([f32; 2], [f32; 2])> {
        let mut values = self.fingers.values();
        Some((*values.next()?, *values.next()?))
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use charon_core::cell::MaterialId;

    #[test]
    fn two_finger_pan_and_zoom() {
        let mut gestures = TouchGestures::default();
        let mut controls = Controls::new(MaterialId::EMPTY);
        assert!(!gestures.update(1, TouchPhase::Started, [100.0, 100.0], &mut controls));
        assert!(gestures.update(2, TouchPhase::Started, [200.0, 100.0], &mut controls));
        assert!(gestures.update(2, TouchPhase::Moved, [300.0, 100.0], &mut controls));
        assert_eq!(controls.zoom, 8);
        assert!(controls.origin[0] < 0.0);
        assert!(gestures.update(2, TouchPhase::Ended, [300.0, 100.0], &mut controls));
        assert!(gestures.update(1, TouchPhase::Ended, [100.0, 100.0], &mut controls));
        assert!(!gestures.update(3, TouchPhase::Started, [100.0, 100.0], &mut controls));
    }
}
