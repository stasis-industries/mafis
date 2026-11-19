//! Render on demand. Bevy draws every display frame by default, even when nothing on
//! screen changes; the observatory mostly sits still (configuring, paused, reading
//! results). It keeps drawing every frame while the simulation runs or someone
//! interacts, then sleeps until the next canvas input or JS command.

use std::time::Duration;

use bevy::prelude::*;
use bevy::window::WindowEvent;
use bevy::winit::{EventLoopProxyWrapper, UpdateMode, WinitSettings};

use super::BRIDGE;
use crate::constants::{IDLE_RENDER_GRACE_SECS, IDLE_RENDER_WAIT_SECS};
use crate::core::state::SimState;
use crate::render::orbit_camera::OrbitCamera;

/// Let `send_command` wake the event loop, so a command sent while asleep runs at once.
pub(super) fn store_waker(proxy: Res<EventLoopProxyWrapper>) {
    let waker = (**proxy).clone();
    BRIDGE.with(|b| b.borrow_mut().waker = Some(waker));
}

pub(super) fn pace_rendering(
    state: Res<State<SimState>>,
    orbit: Res<OrbitCamera>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut window_events: MessageReader<WindowEvent>,
    real_time: Res<Time<Real>>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut settings: ResMut<WinitSettings>,
    mut last_active: Local<f32>,
    mut was_asleep: Local<bool>,
) {
    let command_received = BRIDGE.with(|b| std::mem::take(&mut b.borrow_mut().command_received));
    // Raw mouse motion comes from the whole page, not just the canvas.
    let canvas_input =
        window_events.read().filter(|e| !matches!(e, WindowEvent::MouseMotion(_))).count() > 0;
    let busy = matches!(state.get(), SimState::Loading | SimState::Running)
        || orbit.animating
        || keys.get_pressed().next().is_some()
        || mouse.get_pressed().next().is_some()
        || canvas_input
        || command_received;

    let now = real_time.elapsed_secs();
    if busy {
        *last_active = now;
    }
    let asleep = now - *last_active > IDLE_RENDER_GRACE_SECS;

    let mode = if asleep {
        UpdateMode::reactive_low_power(Duration::from_secs_f32(IDLE_RENDER_WAIT_SECS))
    } else {
        UpdateMode::Continuous
    };
    if settings.focused_mode != mode || settings.unfocused_mode != mode {
        settings.focused_mode = mode;
        settings.unfocused_mode = mode;
    }

    // Stop the clock while asleep, or the sleep (capped at 250 ms) lands as one frame's delta:
    // fixed ticks replay (a STEP runs several) and camera easing / key panning snap. Real time
    // lags a frame (the render world reports when its frame ended), so the frame after the
    // wake-up still spans the sleep: restart only once a whole frame has run awake.
    let paused = asleep || *was_asleep;
    if paused != virtual_time.is_paused() {
        if paused {
            virtual_time.pause();
        } else {
            virtual_time.unpause();
        }
    }
    *was_asleep = asleep;
}
