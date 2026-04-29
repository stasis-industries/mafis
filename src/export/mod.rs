pub mod config;
pub mod csv;
pub mod data;
pub mod gather;
pub mod io;
pub mod json;

use bevy::prelude::*;

use crate::analysis::AnalysisSet;
use crate::analysis::cascade::{CascadeState, DelayRecord};
use crate::analysis::fault_metrics::FaultMetrics;
use crate::analysis::heatmap::HeatmapState;
use crate::analysis::metrics::SimMetrics;
use crate::core::agent::{AgentActionStats, AgentRegistry, LogicalAgent};
use crate::core::grid::GridMap;
use crate::core::seed::SeededRng;
use crate::core::state::{SimState, SimulationConfig};
use crate::core::task::ActiveScheduler;
use crate::core::topology::ActiveTopology;
use crate::fault::FaultSet;
use crate::fault::breakdown::{Dead, FaultEvent};
use crate::fault::config::{FaultConfig, FaultType};
use crate::fault::heat::HeatState;
use crate::solver::ActiveSolver;
use crate::ui::controls::UiState;

use self::config::{ExportConfig, ExportRequest, ExportTrigger};

#[derive(Debug, Clone)]
pub struct FaultLogEntry {
    pub entity: Entity,
    pub fault_type: FaultType,
    pub tick: u64,
    pub position: IVec2,
}

#[derive(Resource, Debug, Default)]
pub struct FaultLog {
    pub entries: Vec<FaultLogEntry>,
    last_count: usize,
}

impl FaultLog {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_count = 0;
    }

    fn has_new_entries(&self) -> bool {
        self.entries.len() > self.last_count
    }

    fn mark_read(&mut self) {
        self.last_count = self.entries.len();
    }
}

pub struct ExportPlugin;

impl Plugin for ExportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExportConfig>()
            .init_resource::<FaultLog>()
            .add_message::<ExportRequest>()
            .add_systems(
                FixedUpdate,
                log_fault_events.after(FaultSet::Schedule).run_if(in_state(SimState::Running)),
            )
            .add_systems(
                FixedUpdate,
                (check_periodic_trigger, check_fault_trigger)
                    .after(AnalysisSet::Metrics)
                    .run_if(in_state(SimState::Running)),
            )
            .add_systems(OnEnter(SimState::Finished), check_finished_trigger)
            .add_systems(Update, process_export_requests)
            .add_systems(OnEnter(SimState::Idle), cleanup_export_state);
    }
}

fn log_fault_events(mut fault_events: MessageReader<FaultEvent>, mut fault_log: ResMut<FaultLog>) {
    for event in fault_events.read() {
        fault_log.entries.push(FaultLogEntry {
            entity: event.entity,
            fault_type: event.fault_type,
            tick: event.tick,
            position: event.position,
        });
    }
}

fn check_periodic_trigger(
    sim_config: Res<SimulationConfig>,
    mut export_config: ResMut<ExportConfig>,
    mut requests: MessageWriter<ExportRequest>,
) {
    if !export_config.periodic_enabled || export_config.periodic_interval == 0 {
        return;
    }

    let last = export_config.last_periodic_tick.unwrap_or(0);
    if sim_config.tick >= last + export_config.periodic_interval && sim_config.tick > 0 {
        export_config.last_periodic_tick = Some(sim_config.tick);
        requests.write(ExportRequest {
            trigger: ExportTrigger::Periodic(sim_config.tick),
            json: export_config.export_json,
            csv: export_config.export_csv,
        });
    }
}

fn check_fault_trigger(
    export_config: Res<ExportConfig>,
    sim_config: Res<SimulationConfig>,
    mut fault_log: ResMut<FaultLog>,
    mut requests: MessageWriter<ExportRequest>,
) {
    if !export_config.auto_on_fault {
        return;
    }

    if fault_log.has_new_entries() {
        fault_log.mark_read();
        requests.write(ExportRequest {
            trigger: ExportTrigger::FaultEvent(sim_config.tick),
            json: export_config.export_json,
            csv: export_config.export_csv,
        });
    }
}

fn check_finished_trigger(
    export_config: Res<ExportConfig>,
    mut requests: MessageWriter<ExportRequest>,
) {
    if !export_config.auto_on_finished {
        return;
    }
    requests.write(ExportRequest {
        trigger: ExportTrigger::Finished,
        json: export_config.export_json,
        csv: export_config.export_csv,
    });
}

#[allow(clippy::too_many_arguments)]
fn process_export_requests(
    mut requests: MessageReader<ExportRequest>,
    sim_config: Res<SimulationConfig>,
    grid: Res<GridMap>,
    rng: Res<SeededRng>,
    ui_state: Res<UiState>,
    fault_config: Res<FaultConfig>,
    cascade: Res<CascadeState>,
    metrics: Res<SimMetrics>,
    heatmap: Res<HeatmapState>,
    registry: Res<AgentRegistry>,
    fault_log: Res<FaultLog>,
    fault_metrics: Res<FaultMetrics>,
    solver: Res<ActiveSolver>,
    topology: Res<ActiveTopology>,
    scheduler: Res<ActiveScheduler>,
    agents: Query<(
        Entity,
        &LogicalAgent,
        Option<&HeatState>,
        Option<&DelayRecord>,
        Option<&AgentActionStats>,
        Has<Dead>,
    )>,
) {
    for request in requests.read() {
        let agent_data: Vec<_> = agents.iter().collect();

        let info = solver.solver().info();
        let snapshot = gather::gather_snapshot(
            &request.trigger,
            &sim_config,
            &grid,
            &rng,
            &ui_state,
            &fault_config,
            &cascade,
            &metrics,
            &heatmap,
            &registry,
            &fault_log,
            &fault_metrics,
            topology.name(),
            scheduler.name(),
            solver.name(),
            info.optimality.label(),
            info.scalability.label(),
            &agent_data,
        );

        let tick = sim_config.tick;
        let trigger_name = request.trigger.to_string();

        if request.json {
            match json::to_json(&snapshot) {
                Ok(content) => {
                    let filename = format!("mafis_t{tick}_{trigger_name}.json");
                    if let Err(e) = io::output_file(&filename, &content) {
                        error!("Export failed: {e}");
                    }
                }
                Err(e) => error!("JSON serialization failed: {e}"),
            }
        }

        if request.csv {
            match csv::to_csv_tables(&snapshot) {
                Ok(tables) => {
                    for (table_name, content) in &tables {
                        let filename = format!("mafis_t{tick}_{trigger_name}_{table_name}.csv");
                        if let Err(e) = io::output_file(&filename, content) {
                            error!("Export failed: {e}");
                        }
                    }
                }
                Err(e) => error!("CSV serialization failed: {e}"),
            }
        }
    }
}

fn cleanup_export_state(mut fault_log: ResMut<FaultLog>, mut export_config: ResMut<ExportConfig>) {
    fault_log.clear();
    export_config.last_periodic_tick = None;
}
