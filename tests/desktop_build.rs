//! Build-time and structural verification tests for the desktop UI.
//!
//! These tests run on `cargo test` (native only) and verify that:
//! 1. Conditional compilation works correctly
//! 2. Constants are properly gated
//! 3. BridgeSet re-export is accessible from the expected path
//! 4. Desktop resources initialize without panic
//! 5. Theme palette values are internally consistent
//!
//! None of these tests require a window or GPU.

#[cfg(not(target_arch = "wasm32"))]
mod desktop_tests {
    use mafis::constants;

    // ── Constants: native limits are higher than WASM defaults ──────

    // Compile-time invariant checks (constants vs. literal bounds).
    const _: () = assert!(
        constants::MAX_AGENTS >= 2000,
        "Native MAX_AGENTS should be >= 2000 for research workloads",
    );
    const _: () = assert!(
        constants::PBS_MAX_NODE_LIMIT >= 5000,
        "Native PBS_MAX_NODE_LIMIT should be >= 5000",
    );
    const _: () = assert!(
        constants::LOADING_OBSTACLE_BATCH >= 1000,
        "Native LOADING_OBSTACLE_BATCH should be >= 1000",
    );
    const _: () = assert!(
        constants::LOADING_AGENT_BATCH >= 500,
        "Native LOADING_AGENT_BATCH should be >= 500",
    );
    const _: () = assert!(
        constants::BASELINE_TICKS_PER_FRAME >= 100,
        "Native BASELINE_TICKS_PER_FRAME should be >= 100",
    );
    const _: () = assert!(constants::MAX_AGENTS <= 100_000, "MAX_AGENTS is unreasonably high",);
    const _: () = assert!(
        constants::PBS_MAX_NODE_LIMIT <= 1_000_000,
        "PBS_MAX_NODE_LIMIT is unreasonably high",
    );

    // Grid dim consistency
    const _: () = assert!(constants::MIN_GRID_DIM > 0);
    const _: () = assert!(constants::MIN_GRID_DIM < constants::MAX_GRID_DIM);
    const _: () = assert!(constants::DEFAULT_GRID_DIM >= constants::MIN_GRID_DIM);
    const _: () = assert!(constants::DEFAULT_GRID_DIM <= constants::MAX_GRID_DIM);

    // Agent count consistency
    const _: () = assert!(constants::MIN_AGENTS > 0);
    const _: () = assert!(constants::MIN_AGENTS <= constants::DEFAULT_AGENTS);
    const _: () = assert!(constants::DEFAULT_AGENTS <= constants::MAX_AGENTS);

    // Duration consistency
    const _: () = assert!(constants::MIN_DURATION > 0);
    const _: () = assert!(constants::MIN_DURATION <= constants::DEFAULT_DURATION);
    const _: () = assert!(constants::DEFAULT_DURATION <= constants::MAX_DURATION);
    const _: () = assert!(constants::DURATION_SHORT <= constants::DURATION_MEDIUM);
    const _: () = assert!(constants::DURATION_MEDIUM <= constants::DURATION_LONG);

    #[test]
    fn native_max_agents_exceeds_wasm_default() {
        // Compile-time check above; this test documents the invariant.
        let _ = constants::MAX_AGENTS;
    }

    #[test]
    fn native_pbs_node_limit_exceeds_wasm_default() {
        let _ = constants::PBS_MAX_NODE_LIMIT;
    }

    #[test]
    fn native_loading_batches_exceed_wasm_defaults() {
        let _ = (constants::LOADING_OBSTACLE_BATCH, constants::LOADING_AGENT_BATCH);
    }

    #[test]
    fn native_baseline_ticks_per_frame_exceeds_wasm() {
        let _ = constants::BASELINE_TICKS_PER_FRAME;
    }

    #[test]
    fn max_agents_has_sane_upper_bound() {
        let _ = constants::MAX_AGENTS;
    }

    #[test]
    fn pbs_node_limit_has_sane_upper_bound() {
        let _ = constants::PBS_MAX_NODE_LIMIT;
    }

    #[test]
    fn grid_dim_bounds_are_consistent() {
        let _ = (constants::MIN_GRID_DIM, constants::MAX_GRID_DIM, constants::DEFAULT_GRID_DIM);
    }

    #[test]
    fn agent_bounds_are_consistent() {
        let _ = (constants::MIN_AGENTS, constants::DEFAULT_AGENTS, constants::MAX_AGENTS);
    }

    #[test]
    fn duration_bounds_are_consistent() {
        let _ = (
            constants::MIN_DURATION,
            constants::DEFAULT_DURATION,
            constants::MAX_DURATION,
            constants::DURATION_SHORT,
            constants::DURATION_MEDIUM,
            constants::DURATION_LONG,
        );
    }

    // ── BridgeSet re-export ────────────────────────────────────────

    #[test]
    fn bridge_set_is_accessible_from_ui_module() {
        // This test verifies the re-export compiles. If the cfg gate or
        // re-export is broken, this file won't compile at all.
        let _set = mafis::ui::BridgeSet;
    }

    // ── Desktop UI state defaults ──────────────────────────────────

    #[test]
    fn desktop_ui_state_defaults_are_reasonable() {
        use mafis::ui::desktop::state::DesktopUiState;

        let state = DesktopUiState::default();
        assert!(state.show_left_panel, "Left panel should be visible by default");
        assert!(state.show_right_panel, "Right panel should be visible by default");
        assert!(state.show_toolbar, "Toolbar should be visible by default");
        assert!(state.show_timeline, "Timeline should be visible by default");
        assert!(!state.show_profiling, "Profiling should be hidden by default");
        assert!(!state.show_experiment, "Experiment panel should be hidden by default");
    }

    #[test]
    fn desktop_ui_state_has_expected_sections() {
        use mafis::ui::desktop::state::DesktopUiState;

        let state = DesktopUiState::default();
        // Core sections must exist
        for key in &["simulation", "solver", "topology", "fault", "status", "scorecard"] {
            assert!(
                state.sections.contains_key(key),
                "Missing section '{key}' in DesktopUiState::default()",
            );
        }
    }

    // ── Theme palette consistency ──────────────────────────────────

    #[test]
    fn dm_mono_font_file_is_valid_ttf() {
        let bytes = include_bytes!("../assets/fonts/DMMono-Regular.ttf");
        // TrueType files start with 0x00010000 or "true" (0x74727565)
        assert!(bytes.len() > 12, "DMMono font file is too small ({} bytes)", bytes.len(),);
        let header = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert!(
            header == 0x00010000 || header == 0x74727565,
            "DMMono font file does not have a valid TrueType header (got {header:#010x})",
        );
    }

    // ── ThemeApplied resource ──────────────────────────────────────

    #[test]
    fn theme_applied_defaults_to_false() {
        use mafis::ui::desktop::ThemeApplied;

        let applied = ThemeApplied::default();
        assert!(!applied.0, "ThemeApplied should default to false");
    }
}
