//! 仕様 06 §7: マウスとタッチを同じポインターイベントに変換する。

use winit::event::{ElementState, MouseButton, TouchPhase, WindowEvent};

/// マウスの左ボタンは id=0。タッチは指ごとの id を保持する。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Down { id: u64, pos: [f32; 2] },
    Move { id: u64, pos: [f32; 2] },
    Up { id: u64, pos: [f32; 2] },
}

/// winit のイベントから入力種別に依存しないイベントへ変換する。
#[derive(Debug, Default)]
pub struct PointerInput {
    mouse_pos: [f32; 2],
    mouse_down: bool,
}

impl PointerInput {
    pub fn normalize(&mut self, event: &WindowEvent) -> Option<PointerEvent> {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_move([position.x as f32, position.y as f32])
            }
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                self.mouse_button(*state)
            }
            WindowEvent::Touch(touch) => {
                let id = touch.id;
                let pos = [touch.location.x as f32, touch.location.y as f32];
                Some(match touch.phase {
                    TouchPhase::Started => PointerEvent::Down { id, pos },
                    TouchPhase::Moved => PointerEvent::Move { id, pos },
                    TouchPhase::Ended | TouchPhase::Cancelled => PointerEvent::Up { id, pos },
                })
            }
            _ => None,
        }
    }

    fn mouse_move(&mut self, pos: [f32; 2]) -> Option<PointerEvent> {
        self.mouse_pos = pos;
        self.mouse_down.then_some(PointerEvent::Move { id: 0, pos })
    }

    fn mouse_button(&mut self, state: ElementState) -> Option<PointerEvent> {
        let down = state == ElementState::Pressed;
        if self.mouse_down == down {
            return None;
        }
        self.mouse_down = down;
        let pos = self.mouse_pos;
        Some(if down { PointerEvent::Down { id: 0, pos } } else { PointerEvent::Up { id: 0, pos } })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_down_move_up() {
        let mut input = PointerInput::default();
        assert_eq!(input.mouse_move([12.0, 16.0]), None);
        assert_eq!(
            input.mouse_button(ElementState::Pressed),
            Some(PointerEvent::Down { id: 0, pos: [12.0, 16.0] })
        );
        assert_eq!(
            input.mouse_move([20.0, 21.0]),
            Some(PointerEvent::Move { id: 0, pos: [20.0, 21.0] })
        );
        assert_eq!(
            input.mouse_button(ElementState::Released),
            Some(PointerEvent::Up { id: 0, pos: [20.0, 21.0] })
        );
        assert_eq!(input.mouse_move([22.0, 24.0]), None);
    }
}
