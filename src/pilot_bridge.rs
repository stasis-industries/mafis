//! Bridge between mafis and mapf-pilot.
//!
//! Builds snapshots lazily — only when the MCP server requests one via WebSocket.
//! When recording mode is active, also stores every tick for replay analysis.

use bevy::prelude::*;

use mapf_pilot_plugin::ring_buffer::SnapshotRingBuffer;
use mapf_pilot_plugin::snapshot::{build_agent_snapshot, build_snapshot};
use mapf_pilot_plugin::ws_server::{SnapshotChannel, SnapshotRequest};
use mapf_pilot_plugin::MapfPilotPlugin;

use crate::core::grid::GridMap;
use crate::core::live_sim::LiveSim;
use crate::core::topology::ZoneMap;
use crate::core::CoreSet;

pub struct PilotBridgePlugin;

impl Plugin for PilotBridgePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MapfPilotPlugin::default())
            .add_systems(
                FixedUpdate,
                handle_pilot_requests
                    .in_set(CoreSet::PostTick)
                    .run_if(resource_exists::<LiveSim>),
            );
    }
}

fn handle_pilot_requests(
    sim: Res<LiveSim>,
    grid: Res<GridMap>,
    zones: Res<ZoneMap>,
    channel: Res<SnapshotChannel>,
    mut ring: ResMut<SnapshotRingBuffer>,
) {
    // If recording, build a snapshot every tick for the ring buffer.
    if ring.is_recording() {
        let snapshot = build_current_snapshot(&sim, &grid, &zones);
        ring.push(snapshot);
    }

    // Drain all pending requests from the WS server.
    while let Ok(request) = channel.rx.try_recv() {
        match request {
            SnapshotRequest::Latest { reply } => {
                let snapshot = build_current_snapshot(&sim, &grid, &zones);
                let _ = reply.send(snapshot);
            }
            SnapshotRequest::History { n, reply } => {
                let history = ring.last_n(n);
                let _ = reply.send(history);
            }
            SnapshotRequest::StartRecording { capacity } => {
                ring.start_recording(capacity);
            }
            SnapshotRequest::StopRecording => {
                ring.stop_recording();
            }
        }
    }
}

fn build_current_snapshot(
    sim: &LiveSim,
    grid: &GridMap,
    zones: &ZoneMap,
) -> mapf_pilot_plugin::mapf_pilot_core::types::SimSnapshot {
    let runner = &sim.runner;

    let agents: Vec<_> = runner
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            build_agent_snapshot(
                i,
                (a.pos.x, a.pos.y),
                (a.goal.x, a.goal.y),
                a.planned_path.iter().map(|act| act.to_u8()).collect(),
                a.task_leg.label(),
                a.alive,
                a.heat,
                a.last_was_forced,
                a.latency_remaining,
                a.operational_age,
                a.last_action.to_u8(),
            )
        })
        .collect();

    let grid_snap = mapf_pilot_plugin::mapf_pilot_core::types::GridSnapshot {
        width: grid.width,
        height: grid.height,
        obstacles: grid.obstacles().iter().map(|p| [p.x, p.y]).collect(),
    };

    let zone_snap = mapf_pilot_plugin::mapf_pilot_core::types::ZoneSnapshot {
        pickup_cells: zones.pickup_cells.iter().map(|p| [p.x, p.y]).collect(),
        delivery_cells: zones.delivery_cells.iter().map(|p| [p.x, p.y]).collect(),
        corridor_cells: zones.corridor_cells.iter().map(|p| [p.x, p.y]).collect(),
    };

    let metrics = mapf_pilot_plugin::mapf_pilot_core::types::SimMetrics {
        tasks_completed: runner.tasks_completed,
        throughput: runner.throughput(runner.tick),
        alive_count: runner.agents.iter().filter(|a| a.alive).count(),
        dead_count: runner.agents.iter().filter(|a| !a.alive).count(),
        heat_avg: if runner.agents.is_empty() {
            0.0
        } else {
            runner.agents.iter().map(|a| a.heat).sum::<f32>() / runner.agents.len() as f32
        },
    };

    build_snapshot(
        runner.tick,
        agents,
        grid_snap,
        zone_snap,
        runner.solver().name(),
        vec![],
        metrics,
    )
}
