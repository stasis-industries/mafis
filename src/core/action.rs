use bevy::prelude::*;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Direction {
    North,
    South,
    East,
    West,
}

impl Direction {
    pub const ALL: [Direction; 4] =
        [Direction::North, Direction::South, Direction::East, Direction::West];

    pub fn offset(self) -> IVec2 {
        match self {
            Direction::North => IVec2::new(0, 1),
            Direction::South => IVec2::new(0, -1),
            Direction::East => IVec2::new(1, 0),
            Direction::West => IVec2::new(-1, 0),
        }
    }

    pub fn opposite(self) -> Direction {
        match self {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::East => Direction::West,
            Direction::West => Direction::East,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Action {
    Wait,
    Move(Direction),
}

impl Action {
    pub fn apply(self, pos: IVec2) -> IVec2 {
        match self {
            Action::Wait => pos,
            Action::Move(dir) => pos + dir.offset(),
        }
    }

    /// Encode as a single byte for compact snapshot storage.
    /// 0=Wait, 1=N, 2=S, 3=E, 4=W
    pub fn to_u8(self) -> u8 {
        match self {
            Action::Wait => 0,
            Action::Move(Direction::North) => 1,
            Action::Move(Direction::South) => 2,
            Action::Move(Direction::East) => 3,
            Action::Move(Direction::West) => 4,
        }
    }

    /// Decode from a snapshot byte. Unknown bytes become Wait.
    pub fn from_u8(b: u8) -> Self {
        match b {
            1 => Action::Move(Direction::North),
            2 => Action::Move(Direction::South),
            3 => Action::Move(Direction::East),
            4 => Action::Move(Direction::West),
            _ => Action::Wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Direction ────────────────────────────────────────────────────

    #[test]
    fn direction_offsets_are_cardinal() {
        assert_eq!(Direction::North.offset(), IVec2::new(0, 1));
        assert_eq!(Direction::South.offset(), IVec2::new(0, -1));
        assert_eq!(Direction::East.offset(), IVec2::new(1, 0));
        assert_eq!(Direction::West.offset(), IVec2::new(-1, 0));
    }

    #[test]
    fn direction_opposites_are_symmetric() {
        for dir in Direction::ALL {
            assert_eq!(dir.opposite().opposite(), dir);
        }
    }

    // ── Action ──────────────────────────────────────────────────────

    #[test]
    fn wait_preserves_position() {
        assert_eq!(Action::Wait.apply(IVec2::new(3, 5)), IVec2::new(3, 5));
    }

    #[test]
    fn move_applies_direction_offset() {
        let origin = IVec2::new(2, 2);
        assert_eq!(Action::Move(Direction::North).apply(origin), IVec2::new(2, 3));
        assert_eq!(Action::Move(Direction::South).apply(origin), IVec2::new(2, 1));
        assert_eq!(Action::Move(Direction::East).apply(origin), IVec2::new(3, 2));
        assert_eq!(Action::Move(Direction::West).apply(origin), IVec2::new(1, 2));
    }

    #[test]
    fn move_then_opposite_returns_to_start() {
        let start = IVec2::new(4, 4);
        for dir in Direction::ALL {
            let moved = Action::Move(dir).apply(start);
            let back = Action::Move(dir.opposite()).apply(moved);
            assert_eq!(back, start, "round-trip failed for {dir:?}");
        }
    }

    // ── Compact encoding (snapshot storage) ──────────────────────────

    #[test]
    fn u8_round_trip_preserves_all_variants() {
        let actions = [
            Action::Wait,
            Action::Move(Direction::North),
            Action::Move(Direction::South),
            Action::Move(Direction::East),
            Action::Move(Direction::West),
        ];
        for action in actions {
            assert_eq!(Action::from_u8(action.to_u8()), action, "round-trip failed for {action:?}");
        }
    }

    #[test]
    fn unknown_u8_decodes_to_wait() {
        assert_eq!(Action::from_u8(255), Action::Wait);
        assert_eq!(Action::from_u8(5), Action::Wait);
    }
}
