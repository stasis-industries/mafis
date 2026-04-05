pub mod baseline;
pub mod cascade;
pub mod dependency;
pub mod engine;
pub mod fault_metrics;
pub mod heatmap;
pub mod history;
pub mod metrics;
pub mod scorecard;

use bevy::prelude::*;

use crate::core::state::SimState;

// ---------------------------------------------------------------------------
// TimeSeriesAccessor — shared trait for AnalysisEngine and BaselineRecord
// ---------------------------------------------------------------------------

/// Uniform access to per-tick time-series data stored in `AnalysisEngine` and
/// `BaselineRecord`. Both structs record identical fields; this trait
/// eliminates the duplicated accessor implementations.
pub trait TimeSeriesAccessor {
    fn throughput_series(&self) -> &[f64];
    fn tasks_completed_series(&self) -> &[u64];
    fn idle_count_series(&self) -> &[usize];
    fn wait_ratio_series(&self) -> &[f32];
    fn position_snapshots(&self) -> &[Vec<IVec2>];

    /// Instantaneous throughput at a specific tick (1-based, clamped to bounds).
    /// Tick 1 = first recorded tick (index 0). Tick 0 returns 0.0.
    fn throughput_at(&self, tick: u64) -> f64 {
        let s = self.throughput_series();
        if s.is_empty() {
            return 0.0;
        }
        let idx = tick.saturating_sub(1) as usize; // tick is 1-based
        let idx = idx.min(s.len() - 1);
        s[idx]
    }

    /// Cumulative tasks at a specific tick (1-based, clamped to bounds).
    /// Tick 1 = first recorded tick (index 0). Tick 0 returns 0.
    fn tasks_at(&self, tick: u64) -> u64 {
        let s = self.tasks_completed_series();
        if s.is_empty() {
            return 0;
        }
        let idx = tick.saturating_sub(1) as usize; // tick is 1-based
        let idx = idx.min(s.len() - 1);
        s[idx]
    }

    /// Idle count at a specific tick (1-based, clamped to bounds).
    /// Tick 1 = first recorded tick (index 0). Tick 0 returns 0.
    fn idle_at(&self, tick: u64) -> usize {
        let s = self.idle_count_series();
        if s.is_empty() {
            return 0;
        }
        let idx = tick.saturating_sub(1) as usize; // tick is 1-based
        let idx = idx.min(s.len() - 1);
        s[idx]
    }

    /// Cumulative wait ratio at a specific tick (1-based, clamped to bounds).
    /// Tick 1 = first recorded tick (index 0). Tick 0 returns 0.0.
    fn wait_ratio_at(&self, tick: u64) -> f32 {
        let s = self.wait_ratio_series();
        if s.is_empty() {
            return 0.0;
        }
        let idx = tick.saturating_sub(1) as usize; // tick is 1-based
        let idx = idx.min(s.len() - 1);
        s[idx]
    }

    /// Agent positions at a specific tick (0-indexed: tick T -> index T-1).
    fn positions_at(&self, tick: u64) -> Option<&[IVec2]> {
        let idx = tick.saturating_sub(1) as usize;
        self.position_snapshots().get(idx).map(|v| v.as_slice())
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnalysisSet {
    BuildGraph,
    Cascade,
    Metrics,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct AnalysisConfig {
    pub heatmap_visible: bool,
    pub path_visible: bool,
}

/// Per-metric toggles — when disabled, the corresponding Rust computation is skipped.
#[derive(Resource, Debug, Clone)]
pub struct MetricsConfig {
    // Core
    pub aet: bool,
    pub makespan: bool,
    pub mttr: bool,
    // Cascade
    pub fault_count: bool,
    pub cascade_depth: bool,
    pub cascade_cost: bool,
    // Fault resilience
    pub fault_mttr: bool,
    pub recovery_rate: bool,
    pub cascade_spread: bool,
    pub throughput: bool,
    pub wait_ratio: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            aet: true,
            makespan: true,
            mttr: true,
            fault_count: true,
            cascade_depth: true,
            cascade_cost: true,
            fault_mttr: true,
            recovery_rate: true,
            cascade_spread: true,
            throughput: true,
            wait_ratio: true,
        }
    }
}

impl MetricsConfig {
    /// Any core metric (AET, makespan, MTTR) enabled?
    pub fn any_core(&self) -> bool {
        self.aet || self.makespan || self.mttr
    }

    /// Any cascade metric enabled?
    pub fn any_cascade(&self) -> bool {
        self.fault_count || self.cascade_depth || self.cascade_cost
    }

    /// Any fault-resilience metric enabled?
    pub fn any_fault(&self) -> bool {
        self.fault_mttr
            || self.recovery_rate
            || self.cascade_spread
            || self.throughput
            || self.wait_ratio
    }

    /// Any metric at all?
    pub fn any(&self) -> bool {
        self.any_core() || self.any_cascade() || self.any_fault()
    }
}

pub struct AnalysisPlugin;

impl Plugin for AnalysisPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnalysisConfig>()
            .init_resource::<MetricsConfig>()
            .init_resource::<baseline::BaselineStore>()
            .init_resource::<baseline::BaselineDiff>()
            .init_resource::<dependency::ActionDependencyGraph>()
            .init_resource::<dependency::AdgThrottle>()
            .init_resource::<dependency::BetweennessCriticality>()
            .init_resource::<cascade::CascadeState>()
            .init_resource::<metrics::SimMetrics>()
            .init_resource::<heatmap::HeatmapState>()
            .init_resource::<heatmap::HeatmapTilePool>()
            .init_resource::<fault_metrics::FaultMetrics>()
            .init_resource::<history::TickHistory>()
            .init_resource::<scorecard::ResilienceScorecard>()
            .init_resource::<scorecard::ScorecardState>()
            .configure_sets(
                FixedUpdate,
                (
                    AnalysisSet::BuildGraph.in_set(crate::core::CoreSet::PostTick),
                    AnalysisSet::Cascade
                        .in_set(crate::core::CoreSet::PostTick)
                        .after(AnalysisSet::BuildGraph),
                    AnalysisSet::Metrics
                        .in_set(crate::core::CoreSet::PostTick)
                        .after(AnalysisSet::Cascade),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    dependency::build_adg
                        .in_set(AnalysisSet::BuildGraph)
                        .run_if(in_state(SimState::Running))
                        .run_if(
                            |m: Res<MetricsConfig>,
                             config: Res<AnalysisConfig>,
                             hm: Res<heatmap::HeatmapState>| {
                                m.any_cascade()
                                    || m.any_fault()
                                    || (config.heatmap_visible
                                        && hm.mode == heatmap::HeatmapMode::Criticality)
                            },
                        ),
                    cascade::propagate_cascade
                        .in_set(AnalysisSet::Cascade)
                        .run_if(in_state(SimState::Running))
                        .run_if(|m: Res<MetricsConfig>| m.any_cascade() || m.any_fault()),
                    metrics::update_metrics
                        .in_set(AnalysisSet::Metrics)
                        .run_if(in_state(SimState::Running))
                        .run_if(|m: Res<MetricsConfig>| m.any_core()),
                    fault_metrics::register_fault_recovery
                        .in_set(AnalysisSet::Metrics)
                        .after(AnalysisSet::Cascade)
                        .run_if(in_state(SimState::Running))
                        .run_if(|m: Res<MetricsConfig>| m.any_fault()),
                    fault_metrics::update_fault_metrics
                        .in_set(AnalysisSet::Metrics)
                        .after(fault_metrics::register_fault_recovery)
                        .run_if(in_state(SimState::Running))
                        .run_if(|m: Res<MetricsConfig>| m.any_fault()),
                    scorecard::update_resilience_scorecard
                        .in_set(AnalysisSet::Metrics)
                        .after(fault_metrics::update_fault_metrics)
                        .run_if(in_state(SimState::Running))
                        .run_if(|m: Res<MetricsConfig>| m.any_fault()),
                    baseline::update_baseline_diff
                        .in_set(AnalysisSet::Metrics)
                        .run_if(in_state(SimState::Running)),
                    #[cfg(target_arch = "wasm32")]
                    baseline::check_position_parity
                        .in_set(AnalysisSet::Metrics)
                        .run_if(in_state(SimState::Running)),
                    history::record_tick_snapshot
                        .in_set(AnalysisSet::Metrics)
                        .after(fault_metrics::update_fault_metrics)
                        .run_if(in_state(SimState::Running)),
                    heatmap::accumulate_heatmap_density
                        .in_set(AnalysisSet::Metrics)
                        .run_if(in_state(SimState::Running))
                        .run_if(|config: Res<AnalysisConfig>| config.heatmap_visible),
                    heatmap::accumulate_heatmap_traffic
                        .in_set(AnalysisSet::Metrics)
                        .run_if(in_state(SimState::Running))
                        .run_if(|config: Res<AnalysisConfig>| config.heatmap_visible),
                    heatmap::accumulate_heatmap_criticality
                        .in_set(AnalysisSet::Metrics)
                        .after(AnalysisSet::BuildGraph)
                        .run_if(in_state(SimState::Running))
                        .run_if(|config: Res<AnalysisConfig>, hm: Res<heatmap::HeatmapState>| {
                            config.heatmap_visible && hm.mode == heatmap::HeatmapMode::Criticality
                        }),
                    dependency::compute_betweenness_criticality
                        .in_set(AnalysisSet::Metrics)
                        .after(AnalysisSet::BuildGraph)
                        .run_if(in_state(SimState::Running)),
                ),
            )
            .add_systems(
                Update,
                (
                    heatmap::hide_heatmap_tiles
                        .run_if(|config: Res<AnalysisConfig>| !config.heatmap_visible),
                    heatmap::replay_heatmap_density
                        .run_if(in_state(SimState::Replay))
                        .run_if(|config: Res<AnalysisConfig>| config.heatmap_visible),
                ),
            )
            .add_systems(
                OnEnter(SimState::Idle),
                (cleanup_analysis_state, heatmap::despawn_heatmap_tiles),
            );

        // Render-dependent systems are excluded in test and headless builds.
        // `update_heatmap_visuals` requires Assets<Mesh/Image/StandardMaterial> +
        // HeatmapTexture which are only available with the render plugin stack.
        #[cfg(not(any(test, feature = "headless")))]
        {
            app.add_systems(Startup, heatmap::setup_heatmap_palette)
                .add_systems(
                    Update,
                    heatmap::update_heatmap_visuals
                        .run_if(|config: Res<AnalysisConfig>| config.heatmap_visible),
                )
                .add_systems(OnEnter(SimState::Running), heatmap::resize_heatmap_texture);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_config_defaults_all_true() {
        let m = MetricsConfig::default();
        assert!(m.any());
        assert!(m.any_core());
        assert!(m.any_cascade());
        assert!(m.any_fault());
    }

    #[test]
    fn metrics_config_any_core() {
        let mut m = MetricsConfig::default();
        m.aet = false;
        m.makespan = false;
        m.mttr = false;
        assert!(!m.any_core());
        // but cascade/fault still on
        assert!(m.any());
    }

    #[test]
    fn metrics_config_any_cascade() {
        let mut m = MetricsConfig::default();
        m.fault_count = false;
        m.cascade_depth = false;
        m.cascade_cost = false;
        assert!(!m.any_cascade());
        assert!(m.any());
    }

    #[test]
    fn metrics_config_any_fault() {
        let mut m = MetricsConfig::default();
        m.fault_mttr = false;
        m.recovery_rate = false;
        m.cascade_spread = false;
        m.throughput = false;
        m.wait_ratio = false;
        assert!(!m.any_fault());
        assert!(m.any()); // core still on
    }

    #[test]
    fn metrics_config_none() {
        let m = MetricsConfig {
            aet: false,
            makespan: false,
            mttr: false,
            fault_count: false,
            cascade_depth: false,
            cascade_cost: false,
            fault_mttr: false,
            recovery_rate: false,
            cascade_spread: false,
            throughput: false,
            wait_ratio: false,
        };
        assert!(!m.any());
        assert!(!m.any_core());
        assert!(!m.any_cascade());
        assert!(!m.any_fault());
    }
}

fn cleanup_analysis_state(
    mut adg: ResMut<dependency::ActionDependencyGraph>,
    mut adg_throttle: ResMut<dependency::AdgThrottle>,
    mut betweenness: ResMut<dependency::BetweennessCriticality>,
    mut cascade: ResMut<cascade::CascadeState>,
    mut sim_metrics: ResMut<metrics::SimMetrics>,
    mut heatmap_state: ResMut<heatmap::HeatmapState>,
    mut fault_metrics_res: ResMut<fault_metrics::FaultMetrics>,
    mut tick_history: ResMut<history::TickHistory>,
    mut resilience_scorecard: ResMut<scorecard::ResilienceScorecard>,
    mut scorecard_state: ResMut<scorecard::ScorecardState>,
    mut baseline_diff: ResMut<baseline::BaselineDiff>,
) {
    adg.clear();
    *adg_throttle = dependency::AdgThrottle::default();
    betweenness.clear();
    cascade.clear();
    sim_metrics.clear();
    heatmap_state.clear();
    fault_metrics_res.clear();
    tick_history.clear();
    resilience_scorecard.clear();
    scorecard_state.clear();
    baseline_diff.clear();
}
