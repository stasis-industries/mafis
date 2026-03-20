use bevy::prelude::*;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SimState {
    #[default]
    Idle,
    Loading,
    Running,
    Paused,
    Replay,
    Finished,
}

// ---------------------------------------------------------------------------
// Loading progress tracking
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadingPhase {
    #[default]
    Setup,
    Obstacles,
    Agents,
    Baseline,
    Solving,
    Done,
}

#[derive(Resource, Default)]
pub struct LoadingProgress {
    pub phase: LoadingPhase,
    pub current: usize,
    pub total: usize,
}

/// Tracks previous SimState so OnEnter handlers can distinguish transitions.
#[derive(Resource, Default)]
pub struct PreviousSimState(pub Option<SimState>);

/// When true, the tick system will execute one tick then return to Paused.
#[derive(Resource, Default)]
pub struct StepMode {
    pub pending: bool,
}

/// When set, the simulation is fast-forwarding to a target tick after a
/// restart-from-replay. The tick loop runs but snapshot recording is suppressed.
/// Once `config.tick >= target`, the field is cleared and the sim pauses.
#[derive(Resource, Default)]
pub struct ResumeTarget {
    pub target_tick: Option<u64>,
}

#[derive(Resource, Debug)]
pub struct SimulationConfig {
    pub tick: u64,
    pub tick_hz: f64,
    pub max_ticks: Option<u64>,
    /// Mandatory simulation duration in ticks.
    pub duration: u64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            tick: 0,
            tick_hz: 10.0,
            max_ticks: None,
            duration: crate::constants::DEFAULT_DURATION,
        }
    }
}

fn track_previous_state(
    state: Res<State<SimState>>,
    mut prev: ResMut<PreviousSimState>,
) {
    prev.0 = Some(*state.get());
}

pub struct StatePlugin;

impl Plugin for StatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<SimState>()
            .init_resource::<SimulationConfig>()
            .init_resource::<PreviousSimState>()
            .init_resource::<StepMode>()
            .init_resource::<ResumeTarget>()
            .init_resource::<LoadingProgress>()
            .insert_resource(Time::<Fixed>::from_hz(10.0))
            .add_systems(Last, track_previous_state);
    }
}
