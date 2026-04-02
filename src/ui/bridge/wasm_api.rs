use super::BRIDGE;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn get_simulation_state() -> String {
    BRIDGE.with(|b| {
        let inner = b.borrow();
        inner.outgoing.clone().unwrap_or_else(|| "{}".to_string())
    })
}

#[wasm_bindgen]
pub fn get_version() -> String {
    crate::constants::VERSION.to_string()
}

#[wasm_bindgen]
pub fn send_command(cmd: &str) {
    if let Some(parsed) = super::parse_command(cmd) {
        BRIDGE.with(|b| {
            let mut inner = b.borrow_mut();
            if inner.incoming.len() < crate::constants::MAX_COMMAND_QUEUE {
                inner.incoming.push(parsed);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Experiment API (WASM)
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub fn experiment_start() {
    crate::experiment::runner::wasm_experiment_start();
}

#[wasm_bindgen]
pub fn experiment_run_single(config_json: &str) -> String {
    crate::experiment::runner::wasm_experiment_run_single(config_json)
}

#[wasm_bindgen]
pub fn experiment_finish() -> String {
    crate::experiment::runner::wasm_experiment_finish()
}
