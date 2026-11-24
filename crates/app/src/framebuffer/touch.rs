//! The path of one finger from going down to lifting, for the verbose log. A tap counts as a click only when it is
//! released on the item it pressed; the log shows how far a finger wandered before it lifted.

use slint::LogicalPosition;

#[derive(Debug, Default)]
pub struct Touch {
    down: LogicalPosition,
    last: LogicalPosition,
    moves: u32,
    farthest: f32,
}

impl Touch {
    pub fn down(at: LogicalPosition) -> Self {
        Self { down: at, last: at, ..Self::default() }
    }

    pub fn moved(&mut self, to: LogicalPosition) {
        self.moves += 1;
        self.last = to;
        self.farthest = self.farthest.max((to.x - self.down.x).hypot(to.y - self.down.y));
    }

    /// For the verbose log: where the finger lifted, how often it moved and how far it got from the press.
    pub fn describe(&self) -> String {
        let (x, y) = (self.last.x, self.last.y);
        format!("at {x:.0},{y:.0} after {} moves, at most {:.0} px from the press", self.moves, self.farthest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_keeps_the_last_place_and_the_farthest_distance() {
        let mut touch = Touch::down(LogicalPosition::new(60.0, 390.0));
        touch.moved(LogicalPosition::new(60.0, 420.0));
        touch.moved(LogicalPosition::new(60.0, 400.0));
        assert_eq!(touch.describe(), "at 60,400 after 2 moves, at most 30 px from the press");
    }

    #[test]
    fn a_touch_without_moves_lifts_where_it_went_down() {
        assert_eq!(
            Touch::down(LogicalPosition::new(10.0, 20.0)).describe(),
            "at 10,20 after 0 moves, at most 0 px from the press"
        );
    }
}
