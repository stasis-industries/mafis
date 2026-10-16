use bevy::prelude::*;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

pub const DEFAULT_SEED: u64 = 42;

#[derive(Resource, Clone)]
pub struct SeededRng {
    pub rng: ChaCha8Rng,
    seed: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self { rng: ChaCha8Rng::seed_from_u64(seed), seed }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn reseed(&mut self, seed: u64) {
        self.seed = seed;
        self.rng = ChaCha8Rng::seed_from_u64(seed);
    }
}

/// Counter-based stream for one purpose at one key (usually the tick).
///
/// Derived only from `(seed, salt, key)`, so its output does not depend on how
/// many draws any other consumer made — a fault perturbs only the decisions it
/// directly touches (common random numbers across the paired runs).
pub fn keyed_rng(seed: u64, salt: u64, key: u64) -> ChaCha8Rng {
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ salt);
    rng.set_stream(key);
    rng
}

impl Default for SeededRng {
    fn default() -> Self {
        Self::new(DEFAULT_SEED)
    }
}

pub struct SeedPlugin;

impl Plugin for SeedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SeededRng>();
    }
}
