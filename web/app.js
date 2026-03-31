// MAFIS — App Controller
// Polls Bevy simulation state via wasm_bindgen bridge and updates the HTML UI.

import { get_simulation_state as wasmGetState, send_command as wasmSendCommand } from './mafis.js';
// Re-export for module-scoped usage
const get_simulation_state = wasmGetState;
window._getState = () => { try { return JSON.parse(wasmGetState()); } catch(e) { return null; } };

const POLL_INTERVAL = 100; // ms
const CHART_MAX_POINTS = 200;

// ---------------------------------------------------------------------------
// Security: HTML escaping helper
// ---------------------------------------------------------------------------

function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

// Performance warning thresholds
const WASM_WARN_AGENTS = 200;
const WASM_WARN_GRID_AREA = 128 * 128; // 16384

let pollTimer = null;
let selectedAgentId = null;
let activeTopologyId = null; // tracks the selected topology preset ID
let lastAgentCount = -1;
let _baselineLoggedForRun = false;
let prevMetricValues = {};
let chartInsts = {};
let chartData = {
    ticks: [],
    avgHeat: [],
    alive: [],
    dead: [],
    throughput: [],
    baselineThroughput: [],
    tasksCumulative: [],
    baselineTasksCumulative: [],
    idleRatio: [],
    baselineIdleRatio: [],
    cascadeSpread: [],
};

// Chart toggle state
let throughputChartMode = 'raw'; // 'raw' or 'gap'
let tasksChartMode = 'abs';      // 'abs' or 'norm'
let resultsThrouputMode = 'raw'; // 'raw' or 'gap' (results panel)
let resultsTasksMode = 'abs';    // 'abs' or 'norm' (results panel)
let _lastShowBaseline = false;   // cached from updateChartData for redrawChartsForMode
let lastChartTick = -1;
let resizeObserver = null;
let contextMenuOpen = false;

// Per-metric keys (match Rust MetricsConfig fields and data-metric attributes)
const METRIC_KEYS = [
    'throughput', 'tasks_completed',
    'fault_count', 'fault_mttr', 'fault_mtbf', 'propagation_rate', 'survival_rate',
];

// Compute composite resilience score from scorecard fields.
// Returns { composite, filledDots } where composite is 0-1 and filledDots is 0-5.
function computeCompositeScore(sc) {
    const ft = sc.fault_tolerance || 0;
    const nrrVal = sc.nrr != null ? sc.nrr : 0.5;
    const fur = sc.fleet_utilization || 0;
    const critNorm = Math.max(0, Math.min(1, 1.0 - (sc.critical_time || 0)));
    const composite = ft * 0.3 + nrrVal * 0.25 + fur * 0.25 + critNorm * 0.2;
    return { composite, filledDots: Math.round(composite * 5) };
}

// Benchmark import state
// (parsedMap/parsedScen removed — replaced by map maker JSON flow)

// Timeline state
let lastTimelineMarkerCount = -1;
let timelineDragging = false;

// ---------------------------------------------------------------------------
// Phase-Driven UI: SimState → UI phase mapping
// ---------------------------------------------------------------------------

let currentPhase = 'configure';
let lastSimState = 'idle';
let vizModeChangeTime = 0; // debounce for mode switch sync

function simStateToPhase(simState) {
    switch (simState) {
        case 'idle':
        case 'loading':
            return 'configure';
        case 'running':
            return 'observe';
        case 'paused':
        case 'replay':
        case 'finished':
            return 'analyze';
        default:
            return 'configure';
    }
}

// ---------------------------------------------------------------------------
// Init — called from index.html after WASM init() completes
// ---------------------------------------------------------------------------

export function initApp() {
    // Set initial phase
    document.body.dataset.phase = 'configure';
    document.getElementById('app-grid').dataset.phase = 'configure';

    initTheme();
    initCollapsible();

    initSettingsModal();
    initPerfWarning();
    detectKeyboardLayout();
    loadGraphicsSettings();
    bindControls();
    bindCustomMapImport();
    bindKeyboard();
    initContextMenu();
    initVizToolbar();
    initResultsPhase();
    initExperimentMode();
    initShareButton();

    // Auto-play demo: show on first visit (persisted in localStorage)
    if (!localStorage.getItem('mafis-demo-seen')) {
        demoController = new DemoController();
        // Enable "Watch Demo" button now that WASM is ready
        const watchBtn = document.getElementById('demo-watch-btn');
        const progressEl = document.getElementById('demo-splash-progress');
        if (watchBtn) {
            watchBtn.textContent = 'Watch Demo';
            watchBtn.disabled = false;
            if (progressEl) progressEl.style.display = 'none';
            watchBtn.addEventListener('click', () => {
                demoController?.startDemo();
            });
        }
    } else {
        // Returning user — hide splash
        const splash = document.getElementById('demo-splash');
        if (splash) splash.style.display = 'none';
    }

    // Skip button
    const skipBtn = document.getElementById('demo-skip');
    if (skipBtn) {
        skipBtn.addEventListener('click', () => {
            demoController?.skip();
        });
    }

    startPolling();

    // Check for shared URL state (auto-configures + starts simulation)
    checkSharedUrl();

    // Suppress right-click context menu on canvas (right-drag = pan)
    const canvas = document.getElementById('bevy-canvas');
    if (canvas) {
        canvas.addEventListener('contextmenu', e => e.preventDefault());
    }
}

// ---------------------------------------------------------------------------
// State polling
// ---------------------------------------------------------------------------

function startPolling() {
    pollTimer = setInterval(() => {
        try {
            const raw = get_simulation_state();
            if (raw && raw !== '{}') {
                const state = JSON.parse(raw);
                updateUI(state);
            }
        } catch (e) {
            // WASM not ready yet — ignore parse errors, but log real bugs
            if (e instanceof SyntaxError) return;
            console.error('[MAFIS poll]', e.message, e.stack);
        }
    }, POLL_INTERVAL);
}

// ---------------------------------------------------------------------------
// Send command to Bevy
// ---------------------------------------------------------------------------

function sendCommand(cmd) {
    try {
        wasmSendCommand(JSON.stringify(cmd));
    } catch (_) {
        // WASM not ready
    }
}
// Expose for debugging / automated testing
window._sendCommand = sendCommand;

// ---------------------------------------------------------------------------
// Loading overlay
// ---------------------------------------------------------------------------

const LOADING_MESSAGES = [
    "A robot slipped on oil, recalculating…",
    "Convincing agents to cooperate…",
    "Negotiating right-of-way disputes…",
    "Robots arguing about the shortest path…",
    "Optimizing procrastination subroutines…",
    "Teaching patience to impatient algorithms…",
    "Untangling a deadlock — the robots, not the code…",
    "Asking politely for more RAM…",
    "Bribing the path planner with extra cycles…",
    "Warming up the constraint solver's coffee…",
    "Counting grid cells… losing count… starting over…",
    "Herding robotic cats into formation…",
];

let loadingMsgIndex = 0;
let loadingMsgTimer = 0;
const LOADING_MSG_INTERVAL = 2500;

// ---------------------------------------------------------------------------
// Auto-Play Demo
// ---------------------------------------------------------------------------

let demoController = null;

const DEMO_CONFIG = {
    topology: 'compact-grid',
    agents: 20,
    solver: 'pibt',
    scheduler: 'random',
    seed: 42,
    tickHz: 12,
    duration: 300,
};

const DEMO_CUES = [
    { tick: 0,   text: '20 robots. Zero collisions.', phase: 'observe' },
    { tick: 50,  text: 'Now watch what happens when things go wrong.', commands: [{ type: 'set_camera_preset', value: 'side' }] },
    { tick: 100, text: 'One failure. Cascade spreading.' },
    { tick: 150, text: 'System adapting. Paths rerouting.' },
    { tick: 220, text: null }, // clear overlay, let metrics breathe
    { tick: 280, text: null, action: 'right_panel_tour' }, // pause sim, guided tour of right panel
];

class DemoController {
    constructor() {
        this.state = 'SPLASH'; // SPLASH | CONFIGURING | PLAYING | CTA | DONE
        this.cueIndex = 0;
        // Show splash (it's already visible in the DOM)
        const splash = document.getElementById('demo-splash');
        if (splash) splash.classList.remove('hidden');
        // Show skip button
        document.body.classList.add('demo-active');
    }

    isActive() {
        return this.state !== 'DONE';
    }

    startDemo() {
        // Hide splash
        const splash = document.getElementById('demo-splash');
        if (splash) splash.classList.add('hidden');

        this.state = 'CONFIGURING';
        this._tourTimers = [];

        // --- Spotlight + tooltip infrastructure ---
        let hole = document.querySelector('.demo-spotlight-hole');
        let tooltip = document.querySelector('.demo-tooltip');
        if (!hole) {
            hole = document.createElement('div');
            hole.className = 'demo-spotlight-hole';
            document.body.appendChild(hole);
        }
        if (!tooltip) {
            tooltip = document.createElement('div');
            tooltip.className = 'demo-tooltip';
            document.body.appendChild(tooltip);
        }
        this._spotHole = hole;
        this._spotTooltip = tooltip;

        const spotlight = async (elOrId, text, placement) => {
            const el = typeof elOrId === 'string' ? document.getElementById(elOrId) : elOrId;
            if (!el) { hole.style.display = 'none'; tooltip.classList.remove('visible'); return; }

            // 1. Scroll first, then wait for it to settle
            el.scrollIntoView({ behavior: 'smooth', block: 'center' });
            await delay(450);

            // 2. Now measure position after scroll
            const pad = 6;
            const r = el.getBoundingClientRect();
            hole.style.display = '';
            hole.style.top = (r.top - pad) + 'px';
            hole.style.left = (r.left - pad) + 'px';
            hole.style.width = (r.width + pad * 2) + 'px';
            hole.style.height = (r.height + pad * 2) + 'px';

            tooltip.innerHTML = text;
            tooltip.classList.add('visible');

            // Position tooltip near the element
            const gap = 12;
            const tw = 320;
            tooltip.style.maxWidth = tw + 'px';

            if (placement === 'right' || (!placement && r.right + tw + gap < window.innerWidth)) {
                tooltip.style.left = (r.right + gap) + 'px';
                tooltip.style.top = Math.max(8, r.top) + 'px';
                tooltip.style.removeProperty('right');
            } else if (placement === 'left' || (!placement && r.left - tw - gap > 0)) {
                tooltip.style.left = Math.max(8, r.left - tw - gap) + 'px';
                tooltip.style.top = Math.max(8, r.top) + 'px';
                tooltip.style.removeProperty('right');
            } else {
                // Below
                tooltip.style.left = Math.max(8, r.left) + 'px';
                tooltip.style.top = (r.bottom + gap) + 'px';
                tooltip.style.removeProperty('right');
            }
        };

        const clearSpotlight = () => {
            hole.style.display = 'none';
            tooltip.classList.remove('visible');
        };

        // Helper to expand a collapsed section
        const expandSection = (targetId) => {
            const content = document.getElementById(targetId);
            const toggle = document.querySelector(`[data-target="${targetId}"]`);
            if (content && content.classList.contains('collapsed')) {
                content.classList.remove('collapsed');
                if (toggle) {
                    const label = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
                    toggle.textContent = '\u25BE ' + label;
                }
            }
        };

        // Helper to collapse a section
        const collapseSection = (targetId) => {
            const content = document.getElementById(targetId);
            const toggle = document.querySelector(`[data-target="${targetId}"]`);
            if (content && !content.classList.contains('collapsed')) {
                content.classList.add('collapsed');
                if (toggle) {
                    const label = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
                    toggle.textContent = '\u25B8 ' + label;
                }
            }
        };

        const delay = (ms) => new Promise(r => {
            const t = setTimeout(r, ms);
            this._tourTimers.push(t);
        });

        // --- Guided tour sequence ---
        const runTour = async () => {
            const STEP = 3000;

            // 1. Topology
            expandSection('sim-content');
            const topo = loadedTopologies.find(t => t.id === DEMO_CONFIG.topology);
            if (topo) {
                sendCommand({ type: 'load_custom_map', ...topo.data });
            } else {
                sendCommand({ type: 'set_topology', value: DEMO_CONFIG.topology });
            }
            const presetBtns = document.querySelectorAll('#topology-presets .topo-preset-btn');
            presetBtns.forEach(b => {
                if (b.dataset.id === DEMO_CONFIG.topology) b.classList.add('active');
            });
            await spotlight('topology-presets', '<strong>Topology</strong> \u2014 Choose a warehouse layout. Selecting <strong>Compact Grid</strong>.');
            await delay(STEP);

            // 2. Algorithm
            expandSection('solver-sched-content');
            sendCommand({ type: 'set_solver', value: DEMO_CONFIG.solver });
            const solverSel = document.getElementById('input-solver');
            if (solverSel) solverSel.value = DEMO_CONFIG.solver;
            await spotlight('input-solver', '<strong>Algorithm</strong> \u2014 PIBT: reactive priority inheritance. Replans every tick.');
            await delay(STEP);

            // 3. Scheduler
            sendCommand({ type: 'set_scheduler', value: DEMO_CONFIG.scheduler });
            const schedSel = document.getElementById('input-scheduler');
            if (schedSel) schedSel.value = DEMO_CONFIG.scheduler;
            await spotlight('input-scheduler', '<strong>Scheduler</strong> \u2014 Random: assigns pickups and deliveries randomly.');
            await delay(STEP);

            // 4. Duration
            sendCommand({ type: 'set_seed', value: DEMO_CONFIG.seed });
            sendCommand({ type: 'set_tick_hz', value: DEMO_CONFIG.tickHz });
            sendCommand({ type: 'set_duration', value: DEMO_CONFIG.duration });
            const durInput = document.getElementById('input-duration');
            if (durInput) durInput.value = DEMO_CONFIG.duration;
            await spotlight('input-duration', '<strong>Duration</strong> \u2014 300 ticks. Enough time to observe fault recovery.');
            await delay(STEP);

            // 5. Fault Injection — add a burst fault via the list
            expandSection('fault-content');
            faultList = [{ id: 'f_demo', type: 'burst_failure', kill_percent: 20, at_tick: 100 }];
            renderFaultList();
            syncFaultListToRust();
            await spotlight('fault-config-panel', '<strong>Fault Injection</strong> \u2014 Burst failure: 20% of agents die at tick 100.');
            await delay(STEP);

            // 6. Launch
            clearSpotlight();
            const overlay = document.getElementById('demo-overlay');
            if (overlay) {
                overlay.textContent = 'Launching simulation\u2026';
                overlay.classList.add('visible');
            }
            sendCommand({ type: 'set_camera_preset', value: 'top' });
            sendCommand({ type: 'set_state', value: 'start' });
            await delay(1500);
            if (overlay) overlay.classList.remove('visible');
        };

        runTour().catch(() => {});
    }

    // Clean up tour elements
    _cleanupTour() {
        if (this._tourTimers) {
            this._tourTimers.forEach(t => clearTimeout(t));
            this._tourTimers = [];
        }
        const hole = document.querySelector('.demo-spotlight-hole');
        const tip = document.querySelector('.demo-tooltip');
        if (hole) hole.style.display = 'none';
        if (tip) tip.classList.remove('visible');
    }

    onPoll(s) {
        if (this.state === 'DONE') return;

        if (this.state === 'CONFIGURING') {
            if (s.state === 'running') {
                this.state = 'PLAYING';
                this.cueIndex = 0;
                // Fire first cue immediately
                this._fireCue(DEMO_CUES[0]);
                this.cueIndex = 1;
            }
            return;
        }

        if (this.state === 'PLAYING') {
            const tick = s.tick || 0;

            // Check if we've reached the next cue
            while (this.cueIndex < DEMO_CUES.length && tick >= DEMO_CUES[this.cueIndex].tick) {
                this._fireCue(DEMO_CUES[this.cueIndex]);
                this.cueIndex++;
            }

            // Also transition to finish if sim finishes early (before all cues)
            if (s.state === 'finished' && this.state === 'PLAYING') {
                this._finishTour();
            }
            return;
        }
    }

    _fireCue(cue) {
        const overlay = document.getElementById('demo-overlay');

        // Phase transition (e.g. configure → observe)
        if (cue.phase) {
            setPhase(cue.phase);
        }

        // Bridge commands (camera, etc.)
        if (cue.commands) {
            for (const cmd of cue.commands) {
                sendCommand(cmd);
            }
        }

        // Special action: pause sim and run guided tour of right panel
        if (cue.action === 'right_panel_tour') {
            sendCommand({ type: 'set_state', value: 'pause' });
            if (overlay) overlay.classList.remove('visible');
            setTimeout(() => {
                setPhase('analyze');
                this._rightPanelTour();
            }, 400);
            return;
        }

        // Legacy: pause sim and highlight VIEW RESULTS button
        if (cue.action === 'highlight_results') {
            sendCommand({ type: 'set_state', value: 'pause' });
            setTimeout(() => {
                setPhase('analyze');
                const resultsBtn = document.getElementById('btn-view-results');
                if (resultsBtn) resultsBtn.classList.add('demo-pulse');
                const panel = document.getElementById('panel-right');
                if (panel) panel.scrollTo({ top: panel.scrollHeight, behavior: 'smooth' });
                this._finishTour();
            }, 300);
            if (overlay) overlay.classList.remove('visible');
            return;
        }

        // Overlay text
        if (!overlay) return;
        if (cue.text) {
            overlay.textContent = cue.text;
            overlay.classList.remove('cta');
            overlay.classList.add('visible');
        } else {
            overlay.classList.remove('visible');
        }
    }

    async _rightPanelTour() {
        const STEP = 3000;
        this._tourTimers = this._tourTimers || [];
        const delay = (ms) => new Promise(r => {
            const t = setTimeout(r, ms);
            this._tourTimers.push(t);
        });

        // Reuse spotlight infrastructure from startDemo
        let hole = document.querySelector('.demo-spotlight-hole');
        let tooltip = document.querySelector('.demo-tooltip');
        if (!hole) {
            hole = document.createElement('div');
            hole.className = 'demo-spotlight-hole';
            document.body.appendChild(hole);
        }
        if (!tooltip) {
            tooltip = document.createElement('div');
            tooltip.className = 'demo-tooltip';
            document.body.appendChild(tooltip);
        }

        const spotlight = async (elOrId, text, placement) => {
            const el = typeof elOrId === 'string' ? document.getElementById(elOrId) : elOrId;
            if (!el) { hole.style.display = 'none'; tooltip.classList.remove('visible'); return; }

            // Scroll first, wait for settle, then measure
            el.scrollIntoView({ behavior: 'smooth', block: 'center' });
            await delay(450);

            const pad = 6;
            const r = el.getBoundingClientRect();
            hole.style.display = '';
            hole.style.top = (r.top - pad) + 'px';
            hole.style.left = (r.left - pad) + 'px';
            hole.style.width = (r.width + pad * 2) + 'px';
            hole.style.height = (r.height + pad * 2) + 'px';

            tooltip.innerHTML = text;
            tooltip.classList.add('visible');
            const gap = 12;
            const tw = 320;
            tooltip.style.maxWidth = tw + 'px';
            if (placement === 'left' || r.left - tw - gap > 0) {
                tooltip.style.left = Math.max(8, r.left - tw - gap) + 'px';
                tooltip.style.top = Math.max(8, r.top) + 'px';
            } else {
                tooltip.style.left = (r.right + gap) + 'px';
                tooltip.style.top = Math.max(8, r.top) + 'px';
            }
        };

        const clearSpotlight = () => {
            hole.style.display = 'none';
            tooltip.classList.remove('visible');
        };

        const expandSection = (targetId) => {
            const content = document.getElementById(targetId);
            const toggle = document.querySelector(`[data-target="${targetId}"]`);
            if (content && content.classList.contains('collapsed')) {
                content.classList.remove('collapsed');
                if (toggle) {
                    const label = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
                    toggle.textContent = '\u25BE ' + label;
                }
            }
        };

        const collapseSection = (targetId) => {
            const content = document.getElementById(targetId);
            const toggle = document.querySelector(`[data-target="${targetId}"]`);
            if (content && !content.classList.contains('collapsed')) {
                content.classList.add('collapsed');
                if (toggle) {
                    const label = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
                    toggle.textContent = '\u25B8 ' + label;
                }
            }
        };

        const panel = document.getElementById('panel-right');

        try {
            // 1. Status section — tick & task bar
            if (panel) panel.scrollTo({ top: 0, behavior: 'smooth' });
            expandSection('status-content');
            await spotlight('status-content', '<strong>Status</strong> \u2014 Current tick, simulation state, and fleet composition at a glance.', 'left');
            await delay(STEP);

            // 2. Task leg bar
            const taskBar = document.getElementById('task-leg-bar-row');
            if (taskBar) {
                await spotlight(taskBar, '<strong>Fleet Bar</strong> \u2014 Real-time breakdown: Delivering, Loading, Idle, and Dead agents.', 'left');
                await delay(STEP);
            }

            // 3. Resilience Scorecard
            expandSection('scorecard-content');
            await spotlight('scorecard-content', '<strong>Resilience Scorecard</strong> \u2014 Four metrics that grade how well the system handles faults: tolerance, recovery, utilization, critical time.', 'left');
            await delay(STEP);
            collapseSection('scorecard-content');

            // 4. System Performance — charts
            expandSection('system-perf-content');
            const throughputChart = document.getElementById('chart-throughput');
            if (throughputChart) {
                await spotlight(throughputChart, '<strong>Throughput Chart</strong> \u2014 Tasks completed per tick. Watch for the dip at tick 100 when agents die.', 'left');
                await delay(STEP);
            }

            // 5. Metrics cards
            const metricThroughput = document.getElementById('metric-throughput');
            if (metricThroughput) {
                const metricsRow = metricThroughput.closest('.metric-row') || metricThroughput.parentElement;
                await spotlight(metricsRow, '<strong>Metrics</strong> \u2014 Throughput, completed tasks, and idle ratio. Delta indicators show change from baseline.', 'left');
                await delay(STEP);
            }
            collapseSection('system-perf-content');

            // 6. Fault Response
            expandSection('fault-response-content');
            await spotlight('fault-response-content', '<strong>Fault Response</strong> \u2014 Live verdict, MTTR/MTBF, survival rate, and a timeline of every fault event.', 'left');
            await delay(STEP);
            collapseSection('fault-response-content');

            // 7. Fault Timeline
            expandSection('fault-events-content');
            await spotlight('fault-events-content', '<strong>Fault Timeline</strong> \u2014 Every fault event logged with tick, agent ID, and type.', 'left');
            await delay(STEP);
            collapseSection('fault-events-content');

            // 8. VIEW RESULTS button
            const resultsBtn = document.getElementById('btn-view-results');
            if (resultsBtn) {
                resultsBtn.classList.add('demo-pulse');
                await spotlight(resultsBtn, '<strong>View Results</strong> \u2014 Full analysis dashboard with exportable charts and data.', 'left');
                await delay(STEP);
            }

            // Done — end demo, let user interact
            clearSpotlight();
            this._finishTour();
        } catch (e) {
            // Tour was interrupted (skip pressed)
            clearSpotlight();
        }
    }

    _finishTour() {
        this.state = 'CTA';

        // Re-enable both panels so user can interact
        const rightPanel = document.getElementById('panel-right');
        if (rightPanel) rightPanel.style.pointerEvents = 'auto';
        const leftPanel = document.getElementById('panel-left');
        if (leftPanel) leftPanel.style.pointerEvents = 'auto';

        // Highlight VIEW RESULTS button
        const resultsBtn = document.getElementById('btn-view-results');
        if (resultsBtn) resultsBtn.classList.add('demo-pulse');

        // Clean up demo state after a short delay
        setTimeout(() => this._cleanup(), 500);
    }

    skip() {
        // Reset simulation, return to configure
        sendCommand({ type: 'set_state', value: 'reset' });
        this._cleanup();
        setPhase('configure');
    }

    _cleanup() {
        this.state = 'DONE';
        this._cleanupTour();
        localStorage.setItem('mafis-demo-seen', '1');
        document.body.classList.remove('demo-active');

        const splash = document.getElementById('demo-splash');
        if (splash) splash.classList.add('hidden');

        // Remove results button highlight
        const resultsBtn = document.getElementById('btn-view-results');
        if (resultsBtn) resultsBtn.classList.remove('demo-pulse');

        // Clear right panel pointer-events override
        const rightPanel = document.getElementById('panel-right');
        if (rightPanel) rightPanel.style.pointerEvents = '';

        const overlay = document.getElementById('demo-overlay');
        if (overlay) {
            overlay.classList.remove('visible', 'cta');
            overlay.textContent = '';
        }

        demoController = null;
    }
}

// ---------------------------------------------------------------------------
// Unified Timeline Bar
// ---------------------------------------------------------------------------

let timelineDuration = 1;

// Cached DOM refs for timeline (set once in initTimelineBar)
let _tlTrack = null, _tlPlayhead = null, _tlProgress = null, _tlTickLabel = null, _tlBuffer = null;
let _tlLastTick = -1, _tlLastDuration = -1, _tlLastMaxSnap = -1;
let _tlScheduledEls = []; // cached scheduled marker DOM elements
let _tlManualCount = 0;
let timelineMaxSnapshotTick = 0; // max tick with a recorded snapshot (for slider clamping)

function updateTimelineBar(s) {
    if (!_tlTrack) return;

    const duration = s.duration || 1;
    timelineDuration = duration;
    const isReplay = s.state === 'replay';
    const currentTick = (isReplay && s.replay) ? s.replay.cursor_tick : s.tick;

    // Max snapshotted tick — determines the buffer bar and slider clamp
    const maxSnap = (s.replay && s.replay.max_tick) ? s.replay.max_tick : 0;
    timelineMaxSnapshotTick = maxSnap;

    // Only update DOM when values actually changed
    const snapChanged = maxSnap !== _tlLastMaxSnap;
    if (currentTick !== _tlLastTick || duration !== _tlLastDuration || snapChanged) {
        _tlLastTick = currentTick;
        _tlLastDuration = duration;
        _tlLastMaxSnap = maxSnap;

        const pctStr = (Math.min(100, (currentTick / duration) * 100)).toFixed(2) + '%';
        _tlPlayhead.style.left = pctStr;
        _tlProgress.style.width = pctStr;
        _tlTickLabel.textContent = currentTick + ' / ' + duration;

        // Buffer bar: shows snapshotted range
        if (_tlBuffer) {
            const bufPct = (Math.min(100, (maxSnap / duration) * 100)).toFixed(2) + '%';
            _tlBuffer.style.width = bufPct;
        }
    }

    // Marker rendering — only rebuild when marker count changes
    const scheduledMarkers = s.fault_schedule_markers || [];
    const observedEvents = s.fault_events || [];
    let manualCount = 0;
    for (let i = 0; i < observedEvents.length; i++) {
        if (observedEvents[i].source === 'Manual') manualCount++;
    }

    const needsRebuild = scheduledMarkers.length !== _tlScheduledEls.length || manualCount !== _tlManualCount;

    if (needsRebuild) {
        // Remove old markers
        const existing = _tlTrack.querySelectorAll('.timeline-marker');
        for (let i = existing.length - 1; i >= 0; i--) existing[i].remove();
        _tlScheduledEls = [];
        _tlManualCount = manualCount;

        // Group all events by tick to handle stacking
        const byTick = new Map();

        // Scheduled markers
        for (let i = 0; i < scheduledMarkers.length; i++) {
            const m = scheduledMarkers[i];
            if (!byTick.has(m.tick)) byTick.set(m.tick, []);
            byTick.get(m.tick).push({
                type: 'scheduled', label: m.label, fired: m.fired, tick: m.tick
            });
        }

        // User-injected markers — only Manual source
        for (let i = 0; i < observedEvents.length; i++) {
            const fe = observedEvents[i];
            if (fe.source !== 'Manual') continue;
            if (!byTick.has(fe.tick)) byTick.set(fe.tick, []);
            byTick.get(fe.tick).push({
                type: 'user-injected', label: fe.fault_type + ' at T' + fe.tick,
                tick: fe.tick, affected: fe.agents_affected || 0, depth: fe.cascade_depth || 0
            });
        }

        // Render one marker per tick (stacked if multiple)
        for (const [tick, events] of byTick) {
            const el = document.createElement('div');
            const hasUser = events.some(e => e.type === 'user-injected');
            const allFired = events.every(e => e.type !== 'scheduled' || e.fired);
            el.className = hasUser ? 'timeline-marker user-injected' :
                (allFired ? 'timeline-marker scheduled fired' : 'timeline-marker scheduled');
            if (events.length > 1) el.classList.add('stacked');
            el.style.left = ((tick / duration) * 100) + '%';
            el.dataset.tick = tick;
            el.dataset.events = JSON.stringify(events);
            el.dataset.count = events.length;
            _tlTrack.appendChild(el);
            // Track scheduled markers for fired-state updates
            if (!hasUser) _tlScheduledEls.push(el);
        }
    } else {
        // Update fired state on cached grouped scheduled markers
        const firedTicks = new Set();
        for (let i = 0; i < scheduledMarkers.length; i++) {
            if (scheduledMarkers[i].fired) firedTicks.add(scheduledMarkers[i].tick);
        }
        for (const el of _tlScheduledEls) {
            const t = parseInt(el.dataset.tick);
            if (firedTicks.has(t) && !el.classList.contains('fired')) {
                el.classList.add('fired');
            }
        }
    }
}

function initTimelineBar() {
    _tlTrack = document.getElementById('timeline-track');
    _tlPlayhead = document.getElementById('timeline-playhead');
    _tlProgress = document.getElementById('timeline-progress');
    _tlBuffer = document.getElementById('timeline-buffer');
    _tlTickLabel = document.getElementById('timeline-tick-label');
    if (!_tlTrack) return;

    // Click + drag on the timeline track (skip if target is a marker)
    _tlTrack.addEventListener('mousedown', (e) => {
        if (e.target.classList.contains('timeline-marker')) return;
        timelineDragging = true;
        seekToClickPosition(e);
        e.preventDefault();
    });

    document.addEventListener('mousemove', (e) => {
        if (!timelineDragging) return;
        seekToClickPosition(e);
        e.preventDefault();
    });

    document.addEventListener('mouseup', () => {
        timelineDragging = false;
    });

    // Hover on fault markers shows popup
    _tlTrack.addEventListener('mouseover', (e) => {
        if (e.target.classList.contains('timeline-marker')) {
            showTimelinePopup(e.target);
        }
    });

    // Click on marker also shows popup (for touch/accessibility)
    _tlTrack.addEventListener('click', (e) => {
        if (e.target.classList.contains('timeline-marker')) {
            e.stopPropagation();
            showTimelinePopup(e.target);
        }
    });

    // Mouse leaves marker area — dismiss popup (unless hovering popup itself)
    _tlTrack.addEventListener('mouseout', (e) => {
        if (e.target.classList.contains('timeline-marker')) {
            const popup = document.getElementById('timeline-popup');
            // Small delay so user can move to popup
            setTimeout(() => {
                if (popup && !popup.matches(':hover')) {
                    hideTimelinePopup();
                }
            }, 200);
        }
    });

    // Click anywhere else to dismiss popup
    document.addEventListener('click', (e) => {
        const popup = document.getElementById('timeline-popup');
        if (popup && popup.style.display === 'block') {
            if (!popup.contains(e.target) && !e.target.classList.contains('timeline-marker')) {
                hideTimelinePopup();
            }
        }
    });
}

let _lastSeekTick = -1;
let _seekThrottleFrame = null;

function seekToClickPosition(e) {
    if (!_tlTrack) return;
    const rect = _tlTrack.getBoundingClientRect();
    const x = Math.max(0, Math.min(e.clientX - rect.left, rect.width));
    const pct = x / rect.width;
    const tick = Math.round(pct * timelineDuration);
    // Clamp to max recorded tick — use whichever is higher: max snapshot tick or current tick
    const maxSeek = Math.max(timelineMaxSnapshotTick, _tlLastTick, 1);
    const clamped = Math.max(0, Math.min(tick, maxSeek));
    if (clamped === _lastSeekTick) return;
    _lastSeekTick = clamped;

    // Immediate visual update (before bridge round-trip)
    const pctStr = ((clamped / timelineDuration) * 100).toFixed(2) + '%';
    _tlPlayhead.style.left = pctStr;
    _tlProgress.style.width = pctStr;
    _tlTickLabel.textContent = clamped + ' / ' + timelineDuration;

    // Throttle bridge commands to one per frame
    if (_seekThrottleFrame) cancelAnimationFrame(_seekThrottleFrame);
    _seekThrottleFrame = requestAnimationFrame(() => {
        sendCommand({ type: 'seek_to_tick', value: clamped });
        _seekThrottleFrame = null;
    });
}

function showTimelinePopup(marker) {
    const popup = document.getElementById('timeline-popup');
    if (!popup) return;

    const tick = marker.dataset.tick;
    let events = [];
    try { events = JSON.parse(marker.dataset.events || '[]'); } catch (_) {}

    let html = '';
    if (events.length === 0) {
        // Fallback for single legacy marker
        html = `<div class="popup-title">T${tick}</div>`;
    } else if (events.length === 1) {
        const ev = events[0];
        const titleClass = ev.type === 'scheduled' ? 'scheduled' : 'user-injected';
        html = `<div class="popup-title ${titleClass}">${ev.label} · T${tick}</div>`;
        if (ev.type === 'scheduled') {
            html += `<div class="popup-detail">${ev.fired ? 'Fired' : 'Scheduled'}</div>`;
        } else {
            html += `<div class="popup-detail">Affected: ${ev.affected || '?'} · Depth: ${ev.depth || '?'}</div>`;
        }
    } else {
        // Multiple faults at same tick — scrollable list
        html = `<div class="popup-title stacked-title">${events.length} faults at T${tick}</div>`;
        html += '<div class="popup-stacked-list">';
        events.forEach((ev, i) => {
            const cls = ev.type === 'scheduled' ? 'scheduled' : 'user-injected';
            html += `<div class="popup-stacked-item ${cls}">`;
            html += `<span class="popup-stacked-num">${i + 1}.</span> ${ev.label}`;
            if (ev.type === 'user-injected') {
                html += ` <span class="popup-stacked-info">· ${ev.affected} affected · depth ${ev.depth}</span>`;
            } else {
                html += ` <span class="popup-stacked-info">· ${ev.fired ? 'fired' : 'pending'}</span>`;
            }
            html += '</div>';
        });
        html += '</div>';
    }
    html += `<button class="popup-seek-btn" data-seek-tick="${tick}">SEEK</button>`;

    popup.innerHTML = html;
    const seekBtn = popup.querySelector('.popup-seek-btn');
    if (seekBtn) {
        seekBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            const t = parseInt(seekBtn.dataset.seekTick);
            sendCommand({ type: 'seek_to_tick', value: t });
            hideTimelinePopup();
        });
    }
    popup.style.display = 'block';

    // Position popup above the marker, centered
    const toolbarEl = document.getElementById('toolbar');
    const markerRect = marker.getBoundingClientRect();
    const toolbarRect = toolbarEl.getBoundingClientRect();

    // Measure popup after rendering
    const popupWidth = popup.offsetWidth;
    let left = markerRect.left - toolbarRect.left + (markerRect.width / 2) - (popupWidth / 2);
    // Clamp to toolbar bounds
    left = Math.max(8, Math.min(left, toolbarRect.width - popupWidth - 8));
    popup.style.left = left + 'px';
}

function hideTimelinePopup() {
    const popup = document.getElementById('timeline-popup');
    if (popup) popup.style.display = 'none';
}

function updateLoadingOverlay(s) {
    const overlay = document.getElementById('loading-overlay');
    if (s.state === 'loading' && s.loading) {
        overlay.classList.remove('hidden');
        const phaseLabels = {
            setup: 'PREPARING',
            obstacles: 'SPAWNING OBSTACLES',
            agents: 'DEPLOYING AGENTS',
            solving: 'COMPUTING PATHS',
            done: 'FINALIZING',
        };
        document.getElementById('loading-phase').textContent =
            phaseLabels[s.loading.phase] || s.loading.phase.toUpperCase();
        document.getElementById('loading-bar-fill').style.width =
            Math.min(s.loading.percent, 100).toFixed(1) + '%';
        document.getElementById('loading-percent').textContent =
            Math.round(s.loading.percent) + '%';

        const now = Date.now();
        if (now - loadingMsgTimer > LOADING_MSG_INTERVAL) {
            loadingMsgTimer = now;
            loadingMsgIndex = (loadingMsgIndex + 1) % LOADING_MESSAGES.length;
        }
        document.getElementById('loading-message').textContent =
            LOADING_MESSAGES[loadingMsgIndex];
    } else {
        overlay.classList.add('hidden');
    }
}

// ---------------------------------------------------------------------------
// UI update from Bevy state
// ---------------------------------------------------------------------------

function updateUI(s) {
    // Demo controller: tick-polled state machine
    if (demoController) demoController.onPoll(s);

    // Phase-Driven UI: set data-phase on body and app-grid
    // Don't override 'results', 'experiment' phases — they're user-triggered
    // Don't override phase while demo is active
    const newPhase = simStateToPhase(s.state);
    if (newPhase !== currentPhase && currentPhase !== 'results'
        && currentPhase !== 'experiment' && !demoController?.isActive()) {
        setPhase(newPhase);
        // Auto-scroll right panel to VIEW RESULTS when simulation finishes
        if (newPhase === 'analyze') {
            setTimeout(() => {
                const resultsBtn = document.getElementById('btn-view-results');
                const panel = document.getElementById('panel-right');
                if (resultsBtn && panel) {
                    panel.scrollTo({ top: panel.scrollHeight, behavior: 'smooth' });
                }
            }, 400);
        }
    }
    // Track sim phase while in experiment mode so we return to the right view
    if (currentPhase === 'experiment') {
        _phaseBeforeExp = newPhase;
    }
    // Reset phase override when sim goes back to idle
    if (s.state === 'idle' && currentPhase === 'results') {
        setPhase('configure');
    }
    lastSimState = s.state;

    // Header status badge
    const badge = document.getElementById('header-status');
    badge.textContent = s.state.toUpperCase();
    badge.dataset.state = s.state;

    // EXP button — always enabled, allow switching freely
    // (text is updated in setPhase)

    // Toolbar buttons
    const btnStart = document.getElementById('btn-start');
    const btnPause = document.getElementById('btn-pause');
    const btnStep = document.getElementById('btn-step');
    const btnReset = document.getElementById('btn-reset');

    const kSpace = '<kbd class="key-space">\u2423</kbd>';
    switch (s.state) {
        case 'idle': {
            btnStart.innerHTML = 'START ' + kSpace;
            // Disable START if no map is loaded (no preset selected AND no custom/imported scenario)
            const hasPreset = document.querySelector('#topology-presets .preset-btn.active');
            const hasScenario = !!s.scenario_loaded;
            btnStart.disabled = !hasPreset && !hasScenario;
            btnStart.classList.remove('ghost');
            btnPause.disabled = true;
            btnStep.disabled = true;
            btnReset.disabled = true;
            break;
        }
        case 'loading':
            btnStart.innerHTML = 'START ' + kSpace;
            btnStart.disabled = true;
            btnStart.classList.add('ghost');
            btnPause.disabled = true;
            btnStep.disabled = true;
            btnReset.disabled = false;
            break;
        case 'running':
            btnStart.innerHTML = 'START ' + kSpace;
            btnStart.disabled = true;
            btnStart.classList.add('ghost');
            btnPause.innerHTML = 'PAUSE ' + kSpace;
            btnPause.disabled = false;
            btnStep.disabled = true;
            btnReset.disabled = false;
            break;
        case 'paused':
            btnStart.innerHTML = 'START ' + kSpace;
            btnStart.disabled = true;
            btnStart.classList.add('ghost');
            btnPause.innerHTML = 'RESUME ' + kSpace;
            btnPause.disabled = false;
            btnStep.disabled = false;
            btnReset.disabled = false;
            break;
        case 'replay':
            btnStart.innerHTML = 'START ' + kSpace;
            btnStart.disabled = true;
            btnStart.classList.add('ghost');
            btnPause.innerHTML = 'RESUME ' + kSpace;
            btnPause.disabled = false;
            btnStep.disabled = false;
            btnReset.disabled = false;
            break;
        case 'finished':
            btnStart.innerHTML = 'RESTART ' + kSpace;
            btnStart.disabled = false;
            btnStart.classList.remove('ghost');
            btnPause.disabled = true;
            btnStep.disabled = true;
            btnReset.disabled = false;
            break;
    }

    // Loading overlay
    updateLoadingOverlay(s);

    // FPS in header
    if (s.fps !== undefined) {
        const fpsEl = document.getElementById('header-fps');
        if (fpsEl) fpsEl.textContent = Math.round(s.fps) + ' FPS';
    }

    // Update timeline bar
    updateTimelineBar(s);

    // Status panel
    const stateEl = document.getElementById('status-state');
    stateEl.textContent = s.state.toUpperCase();
    stateEl.dataset.state = s.state;

    animateMetric('status-tick', s.tick + ' / ' + s.duration);
    // Task leg stacked bar (includes dead agents)
    if (s.task_leg_counts && s.state !== 'idle') {
        const barRow = document.getElementById('task-leg-bar-row');
        if (barRow) {
            barRow.style.display = '';
            const idle = (s.task_leg_counts.free || 0) + (s.task_leg_counts.charging || 0);
            const loading = (s.task_leg_counts.travel_empty || 0) + (s.task_leg_counts.loading || 0);
            const delivering = (s.task_leg_counts.travel_to_queue || 0) + (s.task_leg_counts.queuing || 0) + (s.task_leg_counts.travel_loaded || 0) + (s.task_leg_counts.unloading || 0);
            const deadCount = s.dead_agents || 0;
            const total = idle + loading + delivering + deadCount;
            document.getElementById('task-leg-total').textContent = total;
            if (total > 0) {
                const delPct = (delivering / total * 100).toFixed(1);
                const loadPct = (loading / total * 100).toFixed(1);
                const idlePct = (idle / total * 100).toFixed(1);
                const deadPct = (deadCount / total * 100).toFixed(1);
                document.getElementById('task-bar-delivering').style.width = delPct + '%';
                document.getElementById('task-bar-loading').style.width = loadPct + '%';
                document.getElementById('task-bar-idle').style.width = idlePct + '%';
                document.getElementById('task-bar-dead-seg').style.width = deadPct + '%';
                const delLabel = document.getElementById('task-bar-delivering-label');
                const loadLabel = document.getElementById('task-bar-loading-label');
                const idleLabel = document.getElementById('task-bar-idle-label');
                const deadLabel = document.getElementById('task-bar-dead-label');
                if (delLabel) delLabel.textContent = delivering > 0 ? delivering : '';
                if (loadLabel) loadLabel.textContent = loading > 0 ? loading : '';
                if (idleLabel) idleLabel.textContent = idle > 0 ? idle : '';
                if (deadLabel) deadLabel.textContent = deadCount > 0 ? deadCount : '';
            }
        }
    } else {
        const barRow = document.getElementById('task-leg-bar-row');
        if (barRow) barRow.style.display = 'none';
    }

    // Header status with tick/duration progress
    if (s.state === 'running') {
        badge.textContent = `TICK ${s.tick}/${s.duration}`;
    }

    // (Baseline metrics group removed — now handled via cumulative gap row)

    // (Baseline indicator removed — now shown inline via chart legends and cumulative gap)

    // ═══ System Performance section (always visible when running) ═══
    if (s.metrics) {
        const m = s.metrics;
        const bd = s.baseline_diff;
        const hasBaseline = bd && bd.has_baseline;

        // Show baseline comparison when faults are active (either enabled or manually injected)
        const faultEnabled = s.fault_config && s.fault_config.enabled;
        // Wear/heat specific: only true when wear-based fault (Weibull) is active
        const wearEnabled = s.fault_config && s.fault_config.weibull_enabled;
        const hasFaults = m.fault_count > 0;
        const faultActive = faultEnabled || hasFaults;
        const showBaseline = hasBaseline && faultActive;

        // Throughput card — use MA(10) when available, fall back to instantaneous
        const tpDisplay = (chartData._tpMA && chartData._tpMA.length > 0)
            ? chartData._tpMA[chartData._tpMA.length - 1]
            : m.throughput;
        animateMetric('metric-throughput', tpDisplay != null ? tpDisplay.toFixed(2) : m.throughput.toFixed(2));
        if (showBaseline) {
            updateCtxMetric('throughput', m.throughput, bd.baseline_throughput, 1.0, false);
        } else {
            updateCtxBar('throughput', m.throughput, 1.0);
            clearCtxDelta('throughput');
            clearCtxGhost('throughput');
            clearCtxBaseline('throughput');
        }

        // Tasks Completed card
        const liveTasks = s.lifelong ? s.lifelong.tasks_completed : 0;
        animateMetric('metric-tasks-completed', liveTasks);
        if (showBaseline) {
            updateCtxMetric('tasks_completed', liveTasks, bd.baseline_tasks_at_tick, bd.baseline_total_tasks, false);
        } else {
            clearCtxDelta('tasks_completed');
            clearCtxGhost('tasks_completed');
            clearCtxBaseline('tasks_completed');
        }

        // Idle Ratio card
        animateMetric('metric-idle-ratio', m.idle_ratio.toFixed(2));
        if (showBaseline) {
            updateCtxMetric('idle_ratio', m.idle_ratio, bd.baseline_wait_ratio_at_tick, 1.0, true);
        } else {
            updateCtxBar('idle_ratio', m.idle_ratio, 1.0);
            clearCtxDelta('idle_ratio');
            clearCtxGhost('idle_ratio');
            clearCtxBaseline('idle_ratio');
        }

        // Impacted Area card
        const impactedAreaEl = document.getElementById('metric-impacted-area');
        if (impactedAreaEl && bd) {
            const ia = bd.impacted_area || 0;
            impactedAreaEl.textContent = ia.toFixed(1) + '%';
            impactedAreaEl.style.color = ia < 0 ? 'var(--state-fault)' : ia > 0 ? 'var(--state-moving)' : '';
        }

        // ═══ Fault Response section (show/hide based on fault state) ═══
        const faultSection = document.getElementById('fault-response-section');
        if (faultSection) {
            faultSection.classList.toggle('hidden', !faultActive);
        }

        // System heat chart — only show when wear-based (Weibull) fault is active
        const heatEl = document.getElementById('chart-heat');
        if (heatEl) heatEl.style.display = wearEnabled ? '' : 'none';

        // Fault Response metric cards
        animateMetric('metric-faults', m.fault_count);
        animateMetric('metric-mttr', m.fault_mttr != null ? m.fault_mttr.toFixed(1) : '\u2014');
        animateMetric('metric-mtbf', m.fault_mtbf != null ? m.fault_mtbf.toFixed(1) : '\u2014');
        animateMetric('metric-propagation-rate', m.propagation_rate != null ? m.propagation_rate.toFixed(2) : '0.00');

        // Survival rate
        const survivalRate = s.alive_agents != null && s.total_agents > 0
            ? ((s.alive_agents / s.total_agents) * 100).toFixed(1) + '%'
            : '100%';
        animateMetric('metric-survival-rate', survivalRate);

        // Live verdict + scorecard (compact, in Fault Response)
        if (s.scorecard && (faultEnabled || hasFaults)) {
            updateLiveVerdict(s.scorecard);
            updateLiveScorecard(s.scorecard);
        }
    }

    // Export Now button — enabled when not idle
    document.getElementById('btn-export-now').disabled = (s.state === 'idle' || s.state === 'loading');

    // Disable config inputs and presets when not idle
    const isIdle = s.state === 'idle';
    document.querySelectorAll('#topology-presets .preset-btn, #duration-presets .preset-btn').forEach(btn => {
        btn.disabled = !isIdle;
    });
    const configInputs = [
        'input-agents', 'input-seed', 'input-density',
        'input-grid-width', 'input-grid-height', 'input-solver',
        'input-scheduler',
        'input-duration',
        'input-fault-type',
    ];
    configInputs.forEach(id => {
        const el = document.getElementById(id);
        if (el) el.disabled = !isIdle;
    });

    // Solver dropdown sync and info card
    const solverEl = document.getElementById('input-solver');
    if (solverEl && s.solver && solverEl.value !== s.solver) {
        solverEl.value = s.solver;
    }
    const infoCard = document.getElementById('solver-info-card');
    if (infoCard && s.solver_info) {
        infoCard.style.display = 'block';
        document.getElementById('info-opt').textContent = s.solver_info.optimality;
        document.getElementById('info-scale').textContent = s.solver_info.scalability;
        document.getElementById('info-desc').textContent = s.solver_info.description;

        const warningEl = document.getElementById('info-warning');
        if (s.solver_info.recommended_max_agents && s.num_agents > s.solver_info.recommended_max_agents) {
            warningEl.textContent = `Warning: ${s.solver.toUpperCase()} is not recommended for \u003e${s.solver_info.recommended_max_agents} agents and may timeout.`;
            warningEl.style.display = 'block';
        } else {
            warningEl.style.display = 'none';
        }
    } else if (infoCard) {
        infoCard.style.display = 'none';
    }

    // Map capacity indicator
    const capEl = document.getElementById('map-capacity');
    if (capEl && s.map_capacity) {
        capEl.textContent = `max ${s.map_capacity}`;
    }

    // Fault params — disabled section styling
    const faultEnabled = s.fault_config && s.fault_config.enabled;
    const wearEnabled = s.fault_config && s.fault_config.weibull_enabled;
    const faultParamsEl = document.getElementById('fault-params');
    if (faultParamsEl) {
        faultParamsEl.classList.toggle('section-disabled', !faultEnabled);
    }
    // Also disable individual inputs for fault params when not idle
    const faultParams = [
        'input-heat-per-move', 'input-heat-per-wait', 'input-heat-dissipation',
        'input-congestion-radius', 'input-congestion-bonus',
        'input-overheat-threshold', 'input-breakdown-prob',
    ];
    faultParams.forEach(id => {
        const el = document.getElementById(id);
        if (el) el.disabled = !isIdle || !faultEnabled;
    });

    // (Show paths checkbox removed)

    // Heatmap mode toggle visibility + active state
    // Skip mode sync briefly after user clicks a mode button (prevents flicker)
    const modeDebounceActive = (Date.now() - vizModeChangeTime) < 500;
    const heatmapToggle = document.getElementById('heatmap-mode-toggle');
    if (heatmapToggle && s.analysis_config) {
        const hmVisible = s.analysis_config.heatmap_visible;
        heatmapToggle.style.display = hmVisible ? 'flex' : 'none';
        if (!modeDebounceActive && hmVisible && s.analysis_config.heatmap_mode) {
            const mode = s.analysis_config.heatmap_mode;
            document.getElementById('btn-heatmap-density').classList.toggle('active', mode === 'density');
            document.getElementById('btn-heatmap-traffic').classList.toggle('active', mode === 'traffic');
            document.getElementById('btn-heatmap-criticality').classList.toggle('active', mode === 'criticality');
            const densityRadiusGroup = document.getElementById('density-radius-group');
            if (densityRadiusGroup) {
                densityRadiusGroup.style.display = mode === 'density' ? '' : 'none';
            }
        }
    }

    // Sync floating viz toolbar with bridge state
    if (s.analysis_config) {
        const vizCb = document.getElementById('viz-heatmap-cb');
        if (vizCb) vizCb.checked = s.analysis_config.heatmap_visible;
        if (!modeDebounceActive && s.analysis_config.heatmap_mode) {
            const mode = s.analysis_config.heatmap_mode;
            const md = document.getElementById('viz-mode-d');
            const mt = document.getElementById('viz-mode-t');
            const mc2 = document.getElementById('viz-mode-c');
            if (md) md.classList.toggle('active', mode === 'density');
            if (mt) mt.classList.toggle('active', mode === 'traffic');
            if (mc2) mc2.classList.toggle('active', mode === 'criticality');
            const rg = document.getElementById('viz-radius-group');
            if (rg) rg.style.display = mode === 'density' ? '' : 'none';
        }
    }

    // Sync metric toggle checkboxes from bridge state
    const mc = s.metrics_config;
    if (mc) {
        METRIC_KEYS.forEach(key => {
            const el = document.getElementById('metric-toggle-' + key);
            if (el && el.checked !== mc[key]) el.checked = mc[key];
        });
    }

    // (Charts section visibility now handled per-section in System Performance / Fault Response)

    // Fault timeline — card-based display
    updateFaultTimeline(s);

    // Right-panel fault timeline (condensed)
    // (fault timeline handled by unified timeline bar)

    // Resilience scorecard — visible only during fault injection, with color zones
    const scorecardSection = document.getElementById('scorecard-section');
    if (scorecardSection) {
        if (s.scorecard) {
            scorecardSection.style.display = '';
            const sc = s.scorecard;

            // Fault Tolerance: 0-1, higher=better. Poor <0.5, Fair 0.5-0.8, Good >0.8
            document.getElementById('sc-ft').textContent = sc.fault_tolerance.toFixed(2);
            updateScZone('sc-ft', sc.fault_tolerance, [0.5, 0.8], false);

            // NRR: 0-1, higher=better. Poor <0.4, Fair 0.4-0.7, Good >0.7.
            // Null when < 2 fault events (MTBF undefined).
            const nrrEl = document.getElementById('sc-nrr');
            if (nrrEl) {
                if (sc.nrr != null) {
                    nrrEl.textContent = sc.nrr.toFixed(2);
                    updateScZone('sc-nrr', sc.nrr, [0.4, 0.7], false);
                } else {
                    nrrEl.textContent = 'N/A';
                    const zoneEl = document.getElementById('sc-nrr-zone');
                    if (zoneEl) { zoneEl.textContent = 'Requires 2+ fault events'; zoneEl.dataset.zone = ''; }
                }
            }

            // Adaptability: 0-1, higher=better. Poor <0.3, Fair 0.3-0.6, Good >0.6
            document.getElementById('sc-fleet-utilization').textContent = sc.fleet_utilization.toFixed(2);
            updateScZone('sc-fleet-utilization', sc.fleet_utilization, [0.3, 0.6], false);

            // Critical Time: 0-1, lower=better (inverted). Good <0.1, Fair 0.1-0.3, Poor >0.3
            document.getElementById('sc-crit').textContent = sc.critical_time.toFixed(2);
            updateScZone('sc-crit', sc.critical_time, [0.1, 0.3], true);
        } else {
            scorecardSection.style.display = 'none';
        }
    }

    // Manual fault section — visible when running or paused
    const manualFaultSection = document.getElementById('manual-fault-section');
    if (manualFaultSection) {
        const showManual = s.state === 'running' || s.state === 'paused';
        manualFaultSection.style.display = showManual ? '' : 'none';
    }

    // Conditional visibility: popover heat row
    const popoverHeatRow = document.getElementById('popover-heat-row');
    if (popoverHeatRow) popoverHeatRow.style.display = faultEnabled ? '' : 'none';

    // Periodic interval enabled only when periodic checkbox is checked
    const periodicEl = document.getElementById('input-periodic-interval');
    if (periodicEl) {
        periodicEl.disabled = !(s.export_config && s.export_config.periodic_enabled);
    }

    // Sync tick speed display
    document.getElementById('val-tick-hz').textContent = s.tick_hz;

    // 2D/3D view mode button sync
    const viewModeBtn = document.getElementById('btn-view-mode');
    const is2d = s.camera_mode === '2d';
    if (viewModeBtn && s.camera_mode) {
        viewModeBtn.textContent = is2d ? '2D' : '3D';
        viewModeBtn.style.display = s.state === 'idle' ? '' : 'none';
    }

    // Hide camera buttons in 2D mode (already top-down)
    const btnTop = document.getElementById('btn-view-top');
    if (btnTop) btnTop.style.display = is2d ? 'none' : '';


    // Agent list — aggregate summary when above threshold, per-agent otherwise
    if (s.agent_summary) {
        updateAgentSummary(s.agent_summary, wearEnabled);
        // Close popover in aggregate mode
        if (selectedAgentId !== null) {
            selectedAgentId = null;
            document.getElementById('agent-popover').classList.add('hidden');
        }
    } else {
        updateAgentList(s.agents || [], wearEnabled);
        // Update agent popover if one is selected
        if (selectedAgentId !== null && s.agents) {
            const agent = s.agents.find(a => a.id === selectedAgentId);
            if (agent) updatePopover(agent);
        }
    }

    // Viewport click selection → open context menu
    if (s.click_selection) {
        openContextMenu(s.click_selection);
    }

    // Phase-Driven UI overlays
    updateVerdictBanner(s);
    updateLiveStats(s);

    // Charts
    updateChartData(s);

    // Reset chart data on idle or loading (clears stale data on restart)
    if (s.state === 'idle' || s.state === 'loading') {
        resetChartData();
    }

    // Benchmark import UI sync
    updateCustomMapUI(s);
}

// ---------------------------------------------------------------------------
// Phase-Driven UI: Verdict Banner + Live Stats
// ---------------------------------------------------------------------------

function updateVerdictBanner(s) {
    const banner = document.getElementById('verdict-banner');
    if (!banner) return;

    const phase = currentPhase;
    const isVisible = phase === 'observe' || phase === 'analyze';
    banner.classList.toggle('visible', isVisible);
    if (!isVisible) return;

    const label = document.getElementById('verdict-label');
    const detail = document.getElementById('verdict-detail');
    const dots = document.querySelectorAll('#verdict-dots .verdict-dot');

    // Compute verdict from scorecard
    if (s.scorecard) {
        const { composite, filledDots } = computeCompositeScore(s.scorecard);
        let verdict, verdictLabel;
        if (composite >= 0.7) {
            verdict = 'resilient';
            verdictLabel = 'RESILIENT';
        } else if (composite >= 0.4) {
            verdict = 'degrading';
            verdictLabel = 'DEGRADING';
        } else {
            verdict = 'collapsing';
            verdictLabel = 'COLLAPSING';
        }

        banner.dataset.verdict = verdict;
        label.textContent = verdictLabel;
        detail.textContent = `Score ${composite.toFixed(2)}`;
        dots.forEach((d, i) => d.classList.toggle('filled', i < filledDots));
    } else {
        banner.dataset.verdict = 'running';
        label.textContent = s.state === 'running' ? `TICK ${s.tick}/${s.duration}` : s.state.toUpperCase();
        detail.textContent = '';
        dots.forEach(d => d.classList.remove('filled'));
    }
}

function updateLiveStats(s) {
    const panel = document.getElementById('live-stats');
    if (!panel) return;

    const isVisible = currentPhase === 'observe' || currentPhase === 'analyze';
    panel.classList.toggle('visible', isVisible);
    if (!isVisible) return;

    // Throughput
    const tp = document.getElementById('live-throughput');
    if (s.metrics) {
        tp.textContent = s.metrics.throughput.toFixed(2) + '/t';
    } else if (s.lifelong) {
        tp.textContent = s.lifelong.throughput.toFixed(2) + '/t';
    }

    // Impact (impacted area)
    const impact = document.getElementById('live-impact');
    if (impact && s.baseline_diff) {
        const ia = s.baseline_diff.impacted_area || 0;
        impact.textContent = ia.toFixed(1) + '%';
        impact.style.color = ia < 0 ? 'var(--state-fault)' : ia > 0 ? 'var(--state-moving)' : '';
    } else if (impact) {
        impact.textContent = '\u2014';
        impact.style.color = '';
    }

    // Faults
    const faults = document.getElementById('live-faults');
    if (s.metrics) {
        faults.textContent = s.metrics.fault_count;
    }

    // Fleet
    const fleet = document.getElementById('live-fleet');
    fleet.textContent = `${s.alive_agents}/${s.total_agents}`;
}

// ---------------------------------------------------------------------------
// Fault Timeline
// ---------------------------------------------------------------------------

let lastFaultEventCount = -1;

function updateFaultTimeline(s) {
    const section = document.getElementById('fault-events-section');
    if (!section) return;

    const events = s.fault_events || [];
    const list = document.getElementById('ft-list');
    const empty = document.getElementById('ft-empty');
    const summary = document.getElementById('ft-summary');

    if (events.length === 0) {
        section.style.display = 'none';
        return;
    }

    section.style.display = '';

    // Summary line
    const recovered = events.filter(e => e.recovered).length;
    const permanent = events.length - recovered;
    summary.textContent = `${events.length} events \u2014 ${recovered} recovered, ${permanent} permanent`;

    // Only rebuild DOM when event count changes
    if (events.length === lastFaultEventCount) return;
    lastFaultEventCount = events.length;

    empty.style.display = 'none';
    list.innerHTML = '';

    // Show most recent first, max 20 visible
    const visible = events.slice().reverse().slice(0, 20);
    visible.forEach(fe => {
        const card = document.createElement('div');
        card.className = 'ft-card';

        const recClass = fe.recovered ? 'recovered' : 'permanent';
        const recText = fe.recovered ? `recovered in ${fe.recovery_tick - fe.tick}t` : 'permanent';

        // Impact line: agents + optional throughput drop (only if meaningful)
        let impactHtml = `${fe.agents_affected} agent${fe.agents_affected !== 1 ? 's' : ''} affected`;
        if (fe.agents_affected > 0 && fe.throughput_before > 0 && Math.abs(fe.throughput_delta) > 0.01) {
            const dropPct = Math.abs((fe.throughput_delta / fe.throughput_before) * 100).toFixed(0);
            if (parseInt(dropPct) > 0) {
                const isNeg = fe.throughput_delta < 0;
                impactHtml += ` <span class="ft-card-delta ${isNeg ? 'negative' : 'positive'}">${isNeg ? '\u2212' : '+'}${dropPct}%</span>`;
            }
        }

        card.innerHTML = `
            <div class="ft-card-header">
                <span class="ft-card-type-badge ${fe.fault_type}">${fe.fault_type.toUpperCase()}</span>
                <span class="ft-card-tick">tick ${fe.tick}</span>
            </div>
            <div class="ft-card-impact">${impactHtml}</div>
            <div class="ft-card-status ${recClass}">${recText}</div>
            <div class="ft-card-actions">
                <button class="ft-btn" data-action="seek" data-tick="${fe.tick}">SEEK</button>
            </div>
        `;
        list.appendChild(card);
    });

    // Bind SEEK/COMPARE actions
    list.querySelectorAll('.ft-btn[data-action="seek"]').forEach(btn => {
        btn.addEventListener('click', () => {
            const tick = parseInt(btn.dataset.tick);
            sendCommand({ type: 'seek_to_tick', value: tick });
        });
    });

    list.querySelectorAll('.ft-btn[data-action="compare"]').forEach(btn => {
        btn.addEventListener('click', () => {
            const eventId = parseInt(btn.dataset.eventId);
            showCompareOverlay(eventId, s.fault_events);
        });
    });

}

// (Bottom fault timeline section removed — now handled by unified timeline bar)

function showCompareOverlay(eventId, events) {
    const fe = events.find(e => e.id === eventId);
    if (!fe) return;

    // Remove existing overlay
    const existing = document.getElementById('compare-overlay');
    if (existing) existing.remove();

    const beforeTick = fe.tick - 1;
    const afterTick = fe.recovered ? fe.recovery_tick : fe.tick + 20;
    const deltaPct = fe.throughput_before > 0
        ? ((fe.throughput_delta / fe.throughput_before) * 100).toFixed(0)
        : '0';

    const overlay = document.createElement('div');
    overlay.id = 'compare-overlay';
    overlay.className = 'compare-overlay';
    overlay.innerHTML = `
        <div class="compare-card">
            <div class="compare-header">
                <span class="compare-title">BEFORE / AFTER &mdash; Event #${fe.id}</span>
                <button class="compare-close" id="compare-close">&times;</button>
            </div>
            <div class="compare-cols">
                <div class="compare-col">
                    <div class="compare-col-label">BEFORE (T${beforeTick})</div>
                </div>
                <div class="compare-col">
                    <div class="compare-col-label">AFTER (T${afterTick})</div>
                </div>
            </div>
            <div class="compare-rows">
                <div class="compare-row">
                    <span class="compare-key">Throughput</span>
                    <span class="compare-val">${fe.throughput_before.toFixed(3)}</span>
                    <span class="compare-val">${fe.throughput_min.toFixed(3)}</span>
                    <span class="compare-delta ft-card-delta negative">\u25BC ${Math.abs(deltaPct)}%</span>
                </div>
                <div class="compare-row">
                    <span class="compare-key">Agents affected</span>
                    <span class="compare-val">0</span>
                    <span class="compare-val">${fe.agents_affected}</span>
                    <span class="compare-delta">\u25B2 ${fe.agents_affected}</span>
                </div>
                <div class="compare-row">
                    <span class="compare-key">Cascade</span>
                    <span class="compare-val">&mdash;</span>
                    <span class="compare-val">depth ${fe.cascade_depth}</span>
                    <span></span>
                </div>
                <div class="compare-row">
                    <span class="compare-key">Recovery</span>
                    <span class="compare-val">&mdash;</span>
                    <span class="compare-val">${fe.recovered ? (fe.recovery_tick - fe.tick) + ' ticks' : 'not recovered'}</span>
                    <span></span>
                </div>
            </div>
        </div>
    `;

    document.body.appendChild(overlay);
    document.getElementById('compare-close').addEventListener('click', () => overlay.remove());
    overlay.addEventListener('click', (e) => { if (e.target === overlay) overlay.remove(); });
}

// ---------------------------------------------------------------------------
// Contextual metric helpers
// ---------------------------------------------------------------------------

function updateCtxMetric(metricId, current, baseline, maxVal, invertDelta) {
    // Delta badge
    const deltaEl = document.getElementById('delta-' + metricId);
    if (deltaEl && baseline > 0) {
        const pct = ((current - baseline) / baseline) * 100;
        const absPct = Math.abs(pct).toFixed(0);
        if (Math.abs(pct) < 1) {
            deltaEl.textContent = '~0%';
            deltaEl.dataset.direction = 'neutral';
        } else {
            const arrow = pct > 0 ? '\u25B2' : '\u25BC';
            deltaEl.textContent = `${arrow} ${absPct}%`;
            // For idle ratio, "up" is bad; for throughput, "up" is good
            const isGood = invertDelta ? pct < 0 : pct > 0;
            deltaEl.dataset.direction = isGood ? 'up' : 'down';
        }
    }

    // Bar fill
    const barEl = document.getElementById('bar-' + metricId);
    if (barEl) {
        const pct = Math.min(100, (current / maxVal) * 100);
        barEl.style.width = pct + '%';
    }

    // Ghost (baseline marker)
    const ghostEl = document.getElementById('ghost-' + metricId);
    if (ghostEl && baseline > 0) {
        const pct = Math.min(100, (baseline / maxVal) * 100);
        ghostEl.style.left = pct + '%';
        ghostEl.style.display = '';
    }

    // Baseline text
    const baselineEl = document.getElementById('baseline-' + metricId);
    if (baselineEl && baseline > 0) {
        baselineEl.textContent = 'baseline: ' + baseline.toFixed(3);
    }
}

function updateCtxBar(metricId, current, maxVal) {
    const barEl = document.getElementById('bar-' + metricId);
    if (barEl) {
        const pct = Math.min(100, (current / maxVal) * 100);
        barEl.style.width = pct + '%';
    }
}

function clearCtxDelta(metricId) {
    const deltaEl = document.getElementById('delta-' + metricId);
    if (deltaEl) {
        deltaEl.textContent = '';
        deltaEl.dataset.direction = '';
    }
}

function clearCtxGhost(metricId) {
    const ghostEl = document.getElementById('ghost-' + metricId);
    if (ghostEl) ghostEl.style.display = 'none';
}

function clearCtxBaseline(metricId) {
    const baselineEl = document.getElementById('baseline-' + metricId);
    if (baselineEl) baselineEl.textContent = '';
}

// Scorecard zone helper
// thresholds = [poor/fair boundary, fair/good boundary]
// inverted: true when lower=better (e.g. critical_time)
function updateScZone(prefix, value, thresholds, inverted) {
    const zoneEl = document.getElementById(prefix + '-zone');
    const markerEl = document.getElementById(prefix + '-marker');

    let zone, label;
    if (inverted) {
        // lower = better: <threshold[0] = good, threshold[0]-threshold[1] = fair, >threshold[1] = poor
        if (value <= thresholds[0]) { zone = 'good'; label = 'GOOD'; }
        else if (value <= thresholds[1]) { zone = 'fair'; label = 'FAIR'; }
        else { zone = 'poor'; label = 'POOR'; }
    } else {
        // higher = better: <threshold[0] = poor, threshold[0]-threshold[1] = fair, >threshold[1] = good
        if (value < thresholds[0]) { zone = 'poor'; label = 'POOR'; }
        else if (value <= thresholds[1]) { zone = 'fair'; label = 'FAIR'; }
        else { zone = 'good'; label = 'GOOD'; }
    }

    if (zoneEl) {
        zoneEl.textContent = label;
        zoneEl.dataset.zone = zone;
    }

    // Position marker on the bar (0-100%)
    if (markerEl) {
        let pct;
        if (inverted) {
            // Map value to 0-100% where left=good, right=poor
            const maxVal = thresholds[1] * 1.5; // extend bar beyond "poor" threshold
            pct = Math.min(100, Math.max(0, (value / maxVal) * 100));
        } else {
            // For fault_tolerance/fleet_utilization/NRR (0-1 scale)
            if (thresholds[1] <= 1) {
                pct = Math.min(100, Math.max(0, value * 100));
            } else {
                // For metrics with non-unit scale
                const minVal = thresholds[0] * 2;
                const range = -minVal;
                pct = Math.min(100, Math.max(0, ((value - minVal) / range) * 100));
            }
        }
        markerEl.style.left = pct + '%';
    }
}

// ---------------------------------------------------------------------------
// Metric change animation
// ---------------------------------------------------------------------------

function animateMetric(id, value) {
    const el = document.getElementById(id);
    if (!el) return;
    const strVal = String(value);
    if (prevMetricValues[id] !== undefined && prevMetricValues[id] !== strVal) {
        el.classList.add('changed');
        setTimeout(() => el.classList.remove('changed'), 400);
    }
    el.textContent = strVal;
    prevMetricValues[id] = strVal;
}

// ---------------------------------------------------------------------------
// Agent list
// ---------------------------------------------------------------------------

let lastFaultState = null;

/** Returns the display label for a task leg, respecting simple/detailed mode. */
function taskLegDisplayLabel(leg) {
    const detailed = document.getElementById('setting-detailed-states');
    if (detailed && detailed.checked) return leg || 'free';
    return simplifyTaskLeg(leg);
}

function taskLegBadgeClass(leg) {
    if (!leg) return 'badge-free';
    const l = leg.toLowerCase();
    if (l === 'travel_loaded' || l === 'unloading') return 'badge-delivery';
    if (l === 'travel_empty' || l === 'loading') return 'badge-pickup';
    if (l === 'travel_to_queue') return 'badge-travel-to-queue';
    if (l === 'queuing') return 'badge-queuing';
    if (l === 'charging') return 'badge-charging';
    return 'badge-free';
}

/** Map a detailed task_leg label to a simplified display name. */
function simplifyTaskLeg(leg) {
    if (!leg) return 'idle';
    const l = leg.toLowerCase();
    if (l === 'free' || l === 'charging') return 'idle';
    if (l === 'travel_empty' || l === 'loading') return 'picking';
    return 'delivering';  // travel_to_queue, queuing, travel_loaded, unloading
}

function updateAgentList(agents, faultEnabled) {
    const container = document.getElementById('agent-list');
    if (!container) return;

    // Rebuild DOM when agent count changes OR fault toggle changes (heat bars appear/disappear)
    const needsRebuild = agents.length !== lastAgentCount || faultEnabled !== lastFaultState;
    if (needsRebuild) {
        container.innerHTML = '';
        agents.forEach(a => {
            const row = document.createElement('div');
            row.className = 'agent-row' + (a.is_dead ? ' dead' : '');
            row.dataset.agentId = a.id;
            let html = '<span class="agent-dot"></span>' +
                '<span class="agent-id">A' + a.id + '</span>';
            if (faultEnabled) {
                html += '<div class="agent-heat-bar"><div class="agent-heat-fill" style="width:' + (a.heat_normalized * 100) + '%"></div></div>';
            }
            const badgeClass = taskLegBadgeClass(a.task_leg);
            html += '<span class="agent-task-badge ' + badgeClass + '">' + taskLegDisplayLabel(a.task_leg) + '</span>';
            row.innerHTML = html;
            row.addEventListener('click', () => selectAgent(a.id));
            container.appendChild(row);
        });
        lastAgentCount = agents.length;
        lastFaultState = faultEnabled;
    } else {
        // Update in-place
        agents.forEach(a => {
            const row = container.querySelector('[data-agent-id="' + a.id + '"]');
            if (!row) return;
            row.className = 'agent-row' + (a.is_dead ? ' dead' : '') + (selectedAgentId === a.id ? ' selected' : '');
            if (faultEnabled) {
                const fill = row.querySelector('.agent-heat-fill');
                if (fill) fill.style.width = (a.heat_normalized * 100) + '%';
            }
            const badge = row.querySelector('.agent-task-badge');
            if (badge) {
                const leg = a.task_leg || 'free';
                badge.textContent = taskLegDisplayLabel(leg);
                badge.className = 'agent-task-badge ' + taskLegBadgeClass(leg);
            }
        });
    }
}

// Aggregate summary view for large agent counts (>AGGREGATE_THRESHOLD)
function updateAgentSummary(summary, faultEnabled) {
    const container = document.getElementById('agent-list');
    if (!container) return;

    // Only rebuild if switching to/from aggregate mode
    if (lastAgentCount !== -2) {
        container.innerHTML = '';
        lastAgentCount = -2; // sentinel for "aggregate mode"
    }

    // Build or update summary HTML
    let html = '<div class="agent-summary-panel">';
    html += '<div class="summary-title">AGGREGATE MODE (' + summary.total + ' agents)</div>';
    html += '<div class="summary-row"><span>Alive:</span><span class="mono">' + summary.alive + '</span></div>';
    html += '<div class="summary-row"><span>Dead:</span><span class="mono">' + summary.dead + '</span></div>';
    if (faultEnabled) {
        html += '<div class="summary-row"><span>Avg Wear:</span><span class="mono">' + (summary.avg_heat * 100).toFixed(1) + '%</span></div>';
        html += '<div class="summary-row"><span>Max Wear:</span><span class="mono">' + (summary.max_heat * 100).toFixed(1) + '%</span></div>';
    }
    html += '<div class="summary-row"><span>Avg Idle:</span><span class="mono">' + (summary.avg_idle_ratio * 100).toFixed(1) + '%</span></div>';

    // Mini heat histogram bar
    if (faultEnabled && summary.heat_histogram) {
        html += '<div class="summary-histogram-label">Heat Distribution</div>';
        html += '<div class="summary-histogram">';
        const maxBucket = Math.max(1, ...summary.heat_histogram);
        for (let i = 0; i < 10; i++) {
            const pct = (summary.heat_histogram[i] / maxBucket * 100).toFixed(0);
            const hue = 120 - i * 12; // green→red
            html += '<div class="histo-bar" style="height:' + pct + '%;background:hsl(' + hue + ',70%,50%)" title="' + (i * 10) + '-' + ((i + 1) * 10) + '%: ' + summary.heat_histogram[i] + '"></div>';
        }
        html += '</div>';
    }
    html += '</div>';

    container.innerHTML = html;
}

function selectAgent(id) {
    if (selectedAgentId === id) {
        // Deselect
        selectedAgentId = null;
        document.getElementById('agent-popover').classList.add('hidden');
        return;
    }
    selectedAgentId = id;
    document.getElementById('agent-popover').classList.remove('hidden');
}

function updatePopover(agent) {
    document.getElementById('popover-title').textContent = 'Agent ' + agent.id;
    document.getElementById('popover-pos').textContent = '(' + agent.pos[0] + ', ' + agent.pos[1] + ')';
    document.getElementById('popover-goal').textContent = '(' + agent.goal[0] + ', ' + agent.goal[1] + ')';
    document.getElementById('popover-status').textContent = agent.is_dead ? 'DEAD' : 'ALIVE';
    document.getElementById('popover-status').style.color = agent.is_dead ? 'var(--state-fault)' : 'var(--state-moving)';
    const taskLegEl = document.getElementById('popover-task-leg');
    if (taskLegEl) taskLegEl.textContent = agent.task_leg || 'Free';
    document.getElementById('popover-heat').textContent = agent.heat.toFixed(1);
    document.getElementById('popover-heat-fill').style.width = (agent.heat_normalized * 100) + '%';
    document.getElementById('popover-idle').textContent = (agent.idle_ratio * 100).toFixed(0) + '%';
    document.getElementById('popover-path').textContent = agent.distance_to_goal != null ? agent.distance_to_goal : agent.path_length;

    // Kill button — hidden if already dead
    const killBtn = document.getElementById('btn-kill-agent');
    if (killBtn) {
        killBtn.disabled = agent.is_dead;
        killBtn.style.display = agent.is_dead ? 'none' : '';
    }

    // Slow button — hidden if dead or already has latency
    const slowBtn = document.getElementById('btn-slow-agent');
    if (slowBtn) {
        slowBtn.disabled = agent.is_dead || agent.has_latency;
        slowBtn.style.display = agent.is_dead ? 'none' : '';
    }

    // Latency indicator
    const latencyRow = document.getElementById('popover-latency-row');
    if (latencyRow) {
        latencyRow.style.display = agent.has_latency ? '' : 'none';
    }
}

// ---------------------------------------------------------------------------
// Viewport Context Menu
// ---------------------------------------------------------------------------

let ctxMenuAgentId = null;
let ctxMenuCell = null;

// ---------------------------------------------------------------------------
// Results phase — transition and navigation
// ---------------------------------------------------------------------------

function setPhase(phase) {
    currentPhase = phase;
    document.body.dataset.phase = phase;
    document.getElementById('app-grid').dataset.phase = phase;
    // Sync EXP button active state and label
    const expBtn = document.getElementById('btn-exp-mode');
    if (expBtn) {
        expBtn.classList.toggle('active', phase === 'experiment');
        expBtn.textContent = phase === 'experiment' ? 'OBSERVATORY' : 'EXPERIMENTS';
    }
    // Reset right panel toggle override so phase CSS takes effect
    if (rightPanelHidden) {
        rightPanelHidden = false;
        const panel = document.getElementById('panel-right');
        const grid = document.getElementById('app-grid');
        if (panel) {
            panel.style.opacity = '';
            panel.style.pointerEvents = '';
            panel.style.overflow = '';
            panel.style.padding = '';
            panel.style.border = '';
            panel.style.width = '';
            panel.style.minWidth = '';
        }
        if (grid) grid.style.gridTemplateColumns = '';
    }
}

function initResultsPhase() {
    const btnResults = document.getElementById('btn-view-results');
    if (btnResults) {
        btnResults.addEventListener('click', () => {
            setPhase('results');
            populateResultsDashboard();
        });
    }

    const btnBackAnalysis = document.getElementById('btn-back-analysis');
    if (btnBackAnalysis) {
        btnBackAnalysis.addEventListener('click', () => {
            setPhase('analyze');
        });
    }

    const btnNewSim = document.getElementById('btn-new-sim');
    if (btnNewSim) {
        btnNewSim.addEventListener('click', () => {
            sendCommand({ type: 'set_state', value: 'reset' });
            setPhase('configure');
        });
    }

    // Export buttons
    const btnJson = document.getElementById('btn-export-json-results');
    if (btnJson) btnJson.addEventListener('click', () => sendCommand({ type: 'export_now' }));

    const btnCsv = document.getElementById('btn-export-csv-results');
    if (btnCsv) btnCsv.addEventListener('click', () => sendCommand({ type: 'export_now' }));
}

let resultsChartInsts = {};
let resultsResizeObserver = null;

function populateResultsDashboard() {
    try {
        const raw = get_simulation_state();
        if (!raw) return;
        const s = JSON.parse(raw);
        populateResultsFromState(s);
        // Short delay to let the DOM layout settle before creating charts
        const heatEnabled = s.fault_config && s.fault_config.weibull_enabled;
        setTimeout(() => renderResultsCharts(heatEnabled), 100);
    } catch (_e) {
        // State not available yet
    }
}

function renderResultsCharts(heatEnabled) {
    if (typeof uPlot === 'undefined') return;

    // Destroy previous results charts
    Object.values(resultsChartInsts).forEach(c => { if (c) c.destroy(); });
    resultsChartInsts = {};

    const root = document.documentElement;
    const colorMuted = getComputedStyle(root).getPropertyValue('--text-muted').trim() || '#888';
    const gridColor = 'rgba(128,128,128,0.1)';
    const font = "11px 'DM Mono', monospace";

    const colorPrimary = getComputedStyle(root).getPropertyValue('--text-primary').trim() || '#fff';

    const makeResultChart = (containerId, title, seriesDefs, data, legendHtml) => {
        const el = document.getElementById(containerId);
        if (!el || !data || !data[0] || data[0].length === 0) return null;

        el.innerHTML = '';
        el.style.position = 'relative';
        const width = el.offsetWidth || 250;

        // Title — rendered as own element in DOM order (before chart)
        const titleEl = document.createElement('div');
        titleEl.className = 'results-chart-title';
        titleEl.textContent = title;
        el.appendChild(titleEl);

        // Create tooltip
        const tooltip = document.createElement('div');
        tooltip.className = 'uplot-tooltip';
        tooltip.style.display = 'none';
        el.appendChild(tooltip);

        const opts = {
            width: width,
            height: 140,
            cursor: {
                lock: true,
                focus: { prox: 16 },
                points: { show: true, size: 6, fill: colorPrimary },
            },
            legend: { show: false },
            scales: { x: { time: false } },
            axes: [
                { grid: { show: false }, stroke: colorMuted, font, space: 40,
                  incrs: [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000],
                  values: (u, vals) => vals.map(v => Math.round(v)) },
                { grid: { stroke: gridColor, width: 1 }, stroke: colorMuted, font, size: 50, gap: 5 }
            ],
            series: [{ label: 'Tick' }, ...seriesDefs],
            hooks: {
                setCursor: [(u) => {
                    const idx = u.cursor.idx;
                    if (idx == null) { tooltip.style.display = 'none'; return; }
                    const xVal = u.data[0][idx];
                    let lines = ['T' + Math.round(xVal)];
                    for (let si = 1; si < u.series.length; si++) {
                        if (!u.series[si].show) continue;
                        const yVal = u.data[si][idx];
                        if (yVal != null) {
                            const label = u.series[si].label || '';
                            lines.push(label + ': ' + (yVal % 1 === 0 ? yVal : yVal.toFixed(2)));
                        }
                    }
                    tooltip.innerHTML = lines.join('<br>');
                    const left = u.valToPos(xVal, 'x');
                    tooltip.style.display = 'block';
                    tooltip.style.left = Math.max(0, Math.min(left - 30, el.offsetWidth - 120)) + 'px';
                }]
            }
        };

        const chart = new uPlot(opts, data, el);

        // Legend — appended last in DOM so it renders below the chart
        if (legendHtml) {
            const leg = document.createElement('div');
            leg.className = 'chart-legend';
            leg.innerHTML = legendHtml;
            el.appendChild(leg);
        }

        return { chart, titleEl };
    };

    const d = chartData;
    const hasBaseline = d.baselineThroughput && d.baselineThroughput.some(v => v !== null);
    const blStyle = { stroke: 'rgba(140,140,148,0.6)', width: 1, dash: [4, 4] };
    const blFaintStyle = { stroke: 'rgba(140,140,148,0.3)', width: 1, dash: [4, 4] };

    // --- Throughput chart (results) ---
    const _renderResultsThroughput = () => {
        const tpMA = d._tpMA && d._tpMA.length > 0 ? d._tpMA : d.throughput;
        const blTpMA = d._blTpMA && d._blTpMA.length > 0 ? d._blTpMA : null;

        let tpSeries, tpData, tpLegend;
        if (resultsThrouputMode === 'gap' && hasBaseline && blTpMA) {
            // GAP mode: single line = live_MA - baseline_MA + zero reference
            const gapData = tpMA.map((v, i) => {
                const bl = blTpMA[i];
                return (v != null && bl != null) ? v - bl : null;
            });
            tpSeries = [
                { label: 'Gap (MVA Live - Baseline)', stroke: 'rgb(143,58,222)', fill: 'rgba(143,58,222,0.1)', width: 2 },
                { label: 'Zero', stroke: 'rgba(140,140,148,0.4)', width: 1, dash: [4, 4] }
            ];
            tpData = [d.ticks, gapData, gapData.map(() => 0)];
            tpLegend = '<span style="color:rgb(143,58,222)">\u2501\u2501</span> Gap (MVA Live \u2212 MVA Baseline) &nbsp; <span style="color:rgba(140,140,148,0.4)">\u2508\u2508</span> Zero';
        } else {
            // RAW mode: 4 series — per-tick (faint) + MA (bold) + baseline per-tick (faint dashed) + baseline MA (dashed)
            tpSeries = [
                { label: hasBaseline ? 'Per-Tick (Live)' : 'Per-Tick', stroke: 'rgba(143,58,222,0.3)', width: 1 },
                { label: hasBaseline ? 'MVA (Live)' : 'MVA', stroke: 'rgb(143,58,222)', fill: 'rgba(143,58,222,0.1)', width: 2 }
            ];
            tpData = [d.ticks, d.throughput, tpMA];
            tpLegend = '<span style="color:rgba(143,58,222,0.3)">\u2501\u2501</span> Per-Tick &nbsp; <span style="color:rgb(143,58,222)">\u2501\u2501</span> MVA' + (hasBaseline ? ' (Live)' : '');
            if (hasBaseline) {
                tpSeries.push({ label: 'Per-Tick (BL)', ...blFaintStyle });
                tpSeries.push({ label: 'MVA (BL)', ...blStyle });
                tpData.push(d.baselineThroughput);
                tpData.push(blTpMA || d.baselineThroughput);
                tpLegend += ' &nbsp; <span style="color:rgba(140,140,148,0.3)">\u2508\u2508</span> Per-Tick (BL) &nbsp; <span style="color:rgba(140,140,148,0.6)">\u2508\u2508</span> MVA (Baseline)';
            }
        }
        const result = makeResultChart('results-chart-throughput', 'Throughput', tpSeries, tpData, tpLegend);
        if (result) {
            resultsChartInsts.throughput = result.chart;
            // Inject toggle button
            if (hasBaseline) {
                const btn = document.createElement('button');
                btn.className = 'chart-toggle-btn';
                btn.textContent = resultsThrouputMode === 'raw' ? '[RAW]' : '[GAP]';
                btn.title = 'Toggle gap mode (live MA - baseline MA)';
                btn.addEventListener('click', (e) => {
                    e.stopPropagation();
                    resultsThrouputMode = resultsThrouputMode === 'raw' ? 'gap' : 'raw';
                    // Capture width before destroy to prevent layout shift
                    const cell = document.getElementById('results-chart-throughput');
                    if (cell) cell.style.minWidth = cell.offsetWidth + 'px';
                    if (resultsChartInsts.throughput) { resultsChartInsts.throughput.destroy(); resultsChartInsts.throughput = null; }
                    _renderResultsThroughput();
                    if (cell) cell.style.minWidth = '';
                    if (cell && resultsResizeObserver) resultsResizeObserver.observe(cell);
                });
                result.titleEl.appendChild(btn);
            }
        }
    };

    // --- Tasks chart (results) ---
    const _renderResultsTasks = () => {
        let taskSeries, taskData, taskLegend;
        if (resultsTasksMode === 'norm' && hasBaseline && d.baselineTasksCumulative) {
            // NORM mode: ratio line + 1.0 reference
            const normData = d.tasksCumulative.map((v, i) => {
                const bl = d.baselineTasksCumulative[i];
                return (bl != null && bl > 0) ? v / bl : null;
            });
            taskSeries = [
                { label: 'Ratio (Live / BL)', stroke: 'rgb(45,160,0)', fill: 'rgba(45,160,0,0.1)', width: 2 },
                { label: '1.0', stroke: 'rgba(140,140,148,0.4)', width: 1, dash: [4, 4] }
            ];
            taskData = [d.ticks, normData, normData.map(() => 1.0)];
            taskLegend = '<span style="color:rgb(45,160,0)">\u2501\u2501</span> Ratio (Live / Baseline) &nbsp; <span style="color:rgba(140,140,148,0.4)">\u2508\u2508</span> 1.0';
        } else {
            // ABS mode
            taskSeries = [{ label: hasBaseline ? 'Live' : 'Tasks', stroke: 'rgb(45,160,0)', fill: 'rgba(45,160,0,0.1)', width: 2 }];
            taskData = [d.ticks, d.tasksCumulative];
            taskLegend = '<span style="color:rgb(45,160,0)">\u2501\u2501</span> ' + (hasBaseline ? 'Live' : 'Tasks');
            if (hasBaseline && d.baselineTasksCumulative) {
                taskSeries.push({ label: 'Baseline', ...blStyle });
                taskData.push(d.baselineTasksCumulative);
                taskLegend += ' &nbsp; <span style="color:rgba(140,140,148,0.6)">\u2508\u2508</span> Baseline';
            }
        }
        const result = makeResultChart('results-chart-tasks', 'Tasks Completed', taskSeries, taskData, taskLegend);
        if (result) {
            resultsChartInsts.tasks = result.chart;
            // Inject toggle button
            if (hasBaseline) {
                const btn = document.createElement('button');
                btn.className = 'chart-toggle-btn';
                btn.textContent = resultsTasksMode === 'abs' ? '[ABS]' : '[NORM]';
                btn.title = 'Toggle normalized mode (live / baseline)';
                btn.addEventListener('click', (e) => {
                    e.stopPropagation();
                    resultsTasksMode = resultsTasksMode === 'abs' ? 'norm' : 'abs';
                    // Capture width before destroy to prevent layout shift
                    const cell = document.getElementById('results-chart-tasks');
                    if (cell) cell.style.minWidth = cell.offsetWidth + 'px';
                    if (resultsChartInsts.tasks) { resultsChartInsts.tasks.destroy(); resultsChartInsts.tasks = null; }
                    _renderResultsTasks();
                    if (cell) cell.style.minWidth = '';
                    if (cell && resultsResizeObserver) resultsResizeObserver.observe(cell);
                });
                result.titleEl.appendChild(btn);
            }
        }
    };

    _renderResultsThroughput();
    _renderResultsTasks();

    // Span full row when heat is disabled (only 2 charts: throughput + tasks)
    const tpCell = document.getElementById('results-chart-throughput');
    const tasksCell = document.getElementById('results-chart-tasks');
    if (tpCell) tpCell.style.gridColumn = heatEnabled ? '' : 'span 2';
    if (tasksCell) tasksCell.style.gridColumn = heatEnabled ? '' : 'span 2';

    // Heat-related charts: hide when heat/faults disabled
    const heatCell = document.getElementById('results-chart-heat');
    const agentsCell = document.getElementById('results-chart-agents');
    const cascadeCell = document.getElementById('results-chart-cascade');
    if (heatCell) heatCell.style.display = heatEnabled ? '' : 'none';
    if (agentsCell) agentsCell.style.display = heatEnabled ? '' : 'none';
    if (cascadeCell) cascadeCell.style.display = heatEnabled ? '' : 'none';

    if (heatEnabled) {
        // Fault Response charts
        const heatResult = makeResultChart('results-chart-heat', 'Agent Wear',
            [{ label: 'Avg Wear', stroke: 'rgb(230,140,0)', fill: 'rgba(230,140,0,0.15)', width: 2 }],
            [d.ticks, d.avgHeat],
            '<span style="color:rgb(230,140,0)">\u2501\u2501</span> Avg Wear');
        if (heatResult) resultsChartInsts.heat = heatResult.chart;

        // Agent Status (Results only)
        const agentsResult = makeResultChart('results-chart-agents', 'Agent Status',
            [{ label: 'Alive', stroke: 'rgb(45,160,0)', fill: 'rgba(45,160,0,0.1)', width: 2 },
             { label: 'Dead', stroke: 'rgb(230,44,2)', fill: 'rgba(230,44,2,0.1)', width: 2 }],
            [d.ticks, d.alive, d.dead],
            '<span style="color:rgb(45,160,0)">\u2501\u2501</span> Alive &nbsp; <span style="color:rgb(230,44,2)">\u2501\u2501</span> Dead');
        if (agentsResult) resultsChartInsts.agents = agentsResult.chart;

        // Cascade Spread (Results only)
        const cascadeResult = makeResultChart('results-chart-cascade', 'Cascade Spread',
            [{ label: 'Spread', stroke: 'rgb(255,80,80)', width: 2, paths: () => null, points: { show: true, size: 5, fill: 'rgb(255,80,80)' } }],
            [d.ticks, d.cascadeSpread],
            '<span style="color:rgb(255,80,80)">&#x25CF;</span> Cascade Spread');
        if (cascadeResult) resultsChartInsts.cascade = cascadeResult.chart;
    }

    // Use ResizeObserver to resize charts when layout settles
    if (resultsResizeObserver) resultsResizeObserver.disconnect();
    resultsResizeObserver = new ResizeObserver((entries) => {
        for (const entry of entries) {
            const el = entry.target;
            const w = entry.contentRect.width;
            if (w <= 0) continue;
            // Find which chart lives in this cell
            const chart = Object.values(resultsChartInsts).find(c => c && c.root && c.root.parentElement === el);
            if (chart) {
                chart.setSize({ width: w, height: 140 });
            }
        }
    });
    // Observe all chart cells
    document.querySelectorAll('.results-chart-cell').forEach(cell => {
        resultsResizeObserver.observe(cell);
    });
}

function populateResultsFromState(s) {
    // Detect whether any faults actually occurred (scheduled or manual)
    const faultEvents = s.fault_events || [];
    const faultCount = s.metrics ? s.metrics.fault_count : 0;
    const hadFaults = faultEvents.length > 0 || faultCount > 0;

    // Verdict — only show resilience verdict when faults occurred
    const verdictLabel = document.getElementById('results-verdict-label');
    const verdictDots = document.getElementById('results-verdict-dots');
    const verdictRow = document.querySelector('.results-verdict-row');
    if (verdictRow) verdictRow.style.display = hadFaults ? '' : 'none';
    if (s.scorecard && verdictLabel && hadFaults) {
        const sc = s.scorecard;
        const { composite: avg, filledDots: filled } = computeCompositeScore(sc);
        let verdictText = 'Unknown';
        if (avg >= 0.8) verdictText = 'Resilient';
        else if (avg >= 0.6) verdictText = 'Moderate';
        else if (avg >= 0.4) verdictText = 'Degraded';
        else verdictText = 'Fragile';
        verdictLabel.textContent = verdictText;
        verdictDots.innerHTML = Array.from({length: 5}, (_, i) =>
            `<div class="dot${i < filled ? ' filled' : ''}"></div>`
        ).join('');
    }

    // Scorecard bars — only when faults occurred
    const scorecardBars = document.getElementById('results-scorecard-bars');
    if (s.scorecard && scorecardBars && hadFaults) {
        const sc = s.scorecard;
        const metrics = [
            { name: 'Fault Tolerance', value: sc.fault_tolerance || 0 },
            { name: 'NRR', value: sc.nrr != null ? sc.nrr : 0 },
            { name: 'Fleet Utilization', value: sc.fleet_utilization || 0 },
            { name: 'Critical Time', value: sc.critical_time || 0, inverted: true },
        ];
        scorecardBars.innerHTML = metrics.map(m => {
            const pct = (m.value * 100).toFixed(0);
            const color = m.inverted
                ? (m.value <= 0.1 ? 'var(--state-moving)' : m.value <= 0.3 ? 'var(--state-delayed)' : 'var(--state-dead)')
                : (m.value >= 0.7 ? 'var(--state-moving)' : m.value >= 0.4 ? 'var(--state-delayed)' : 'var(--state-dead)');
            return `<div style="display:flex;align-items:center;gap:8px;margin-bottom:4px;">
                <span style="width:90px;font-size:10px;color:var(--text-muted);text-transform:uppercase;">${m.name}</span>
                <div style="flex:1;height:8px;background:var(--bg-card);border:1px solid var(--border);">
                    <div style="width:${pct}%;height:100%;background:${color};"></div>
                </div>
                <span style="width:30px;font-size:10px;color:var(--text-secondary);text-align:right;">${pct}%</span>
            </div>`;
        }).join('');
    } else if (scorecardBars && !hadFaults) {
        scorecardBars.innerHTML = '';
    }

    // Summary table — adapt structure based on whether faults occurred
    const summaryBody = document.getElementById('results-summary-body');
    const summaryThead = summaryBody?.closest('table')?.querySelector('thead');
    if (s.metrics && summaryBody) {
        const m = s.metrics;
        const bd = s.baseline_diff || {};
        const liveTasks = s.lifelong ? s.lifelong.tasks_completed : null;
        const survivalRate = s.alive_agents != null && s.total_agents > 0
            ? (s.alive_agents / s.total_agents) : 1.0;

        if (hadFaults) {
            // Full baseline vs final comparison
            if (summaryThead) summaryThead.innerHTML = '<tr><th>Metric</th><th>Baseline</th><th>Final</th><th>Delta</th></tr>';
            const blThroughput = bd.has_baseline ? bd.baseline_avg_throughput : null;
            const blTasks = bd.has_baseline ? bd.baseline_total_tasks : null;
            const impactedArea = bd.impacted_area || 0;
            const blIdleRatio = bd.has_baseline && bd.baseline_wait_ratio_at_tick != null
                ? bd.baseline_wait_ratio_at_tick : null;
            const rows = [
                { name: 'Throughput (avg)', baseline: blThroughput, current: m.throughput },
                { name: 'Tasks Completed', baseline: blTasks, current: liveTasks, integer: true },
                { name: 'Idle Ratio (avg)', baseline: blIdleRatio, current: m.idle_ratio, inverted: true },
                { name: 'Impacted Area', baseline: null, current: impactedArea, noCompare: true, suffix: '%' },
                { name: 'Deficit', baseline: null, current: bd.deficit_integral || 0, integer: true, noCompare: true },
                { name: 'Surplus', baseline: null, current: bd.surplus_integral || 0, integer: true, noCompare: true },
                { name: 'MTTR', baseline: null, current: m.fault_mttr, noCompare: true },
                { name: 'MTBF', baseline: null, current: m.fault_mtbf, noCompare: true },
                { name: 'NRR', baseline: null, current: s.scorecard ? s.scorecard.nrr : null, noCompare: true },
                { name: 'Propagation Rate', baseline: null, current: m.propagation_rate, noCompare: true },
                { name: 'Survival Rate', baseline: null, current: survivalRate, noCompare: true },
            ];
            summaryBody.innerHTML = rows.map(r => {
                const fmt = v => r.integer ? String(Math.round(v)) : v.toFixed(3);
                const baseStr = r.baseline != null ? fmt(r.baseline) : '\u2014';
                const curStr = r.current != null ? (typeof r.current === 'number' ? fmt(r.current) : r.current) : '\u2014';
                let deltaStr = '';
                let deltaStyle = '';
                if (!r.noCompare && r.baseline != null && r.baseline > 0 && r.current != null) {
                    const delta = ((r.current - r.baseline) / r.baseline * 100).toFixed(1);
                    const invertedMetric = (r.name === 'Idle Ratio (avg)' || r.name === 'MTTR');
                    const badness = invertedMetric ? parseFloat(delta) : -parseFloat(delta);
                    const absBad = Math.abs(badness);
                    let color;
                    if (badness <= 0) color = 'rgb(45,160,0)';
                    else if (absBad <= 5) color = 'rgb(200,180,0)';
                    else if (absBad <= 20) {
                        const t = (absBad - 5) / 15;
                        color = `rgb(${Math.round(200 + t * 30)},${Math.round(180 - t * 136)},0)`;
                    } else color = 'rgb(230,44,2)';
                    deltaStr = (delta > 0 ? '\u25B2 +' : '\u25BC ') + delta + '%';
                    deltaStyle = `color:${color}`;
                }
                return `<tr><td>${r.name}</td><td>${baseStr}</td><td>${curStr}</td><td style="${deltaStyle}">${deltaStr}</td></tr>`;
            }).join('');
        } else {
            // No faults — simple single-value summary, no baseline/final/delta
            if (summaryThead) summaryThead.innerHTML = '<tr><th>Metric</th><th>Value</th></tr>';
            const rows = [
                { name: 'Throughput (avg)', value: m.throughput },
                { name: 'Tasks Completed', value: liveTasks, integer: true },
                { name: 'Idle Ratio (avg)', value: m.idle_ratio },
                { name: 'Survival Rate', value: survivalRate },
            ];
            summaryBody.innerHTML = rows.map(r => {
                const fmt = v => r.integer ? String(Math.round(v)) : v.toFixed(3);
                const valStr = r.value != null ? (typeof r.value === 'number' ? fmt(r.value) : r.value) : '\u2014';
                return `<tr><td>${r.name}</td><td>${valStr}</td></tr>`;
            }).join('');
        }
    }

    // Fault timeline table — hide entirely when no faults occurred
    const faultSection = document.querySelector('.results-fault-section');
    if (faultSection) faultSection.style.display = hadFaults ? '' : 'none';
    const faultBody = document.getElementById('results-fault-body');
    if (faultBody && hadFaults) {
        faultBody.innerHTML = faultEvents.map(fe => {
            const tpDrop = fe.throughput_delta > 0 ? `-${fe.throughput_delta.toFixed(3)}` : '\u2014';
            const tpClass = fe.throughput_delta > 0.05 ? ' class="delta-negative"' : '';
            const recStr = fe.recovered ? `T${fe.recovery_tick}` : (fe.throughput_delta > 0 ? 'pending' : '\u2014');
            return `<tr><td>T${fe.tick}</td><td>${fe.fault_type}</td><td>${fe.agents_affected}</td><td>${fe.cascade_depth}</td><td${tpClass}>${tpDrop}</td><td>${recStr}</td></tr>`;
        }).join('');
    }

    // Config summary — fix NaN% for fault label
    const configDl = document.getElementById('results-config');
    if (configDl) {
        const agentCount = s.total_agents || s.num_agents || (s.agents ? s.agents.length : 0) || '\u2014';
        const scheduler = (s.lifelong && s.lifelong.scheduler) || '\u2014';
        const sc = s.fault_scenario;
        const faultIsEnabled = s.fault_config && s.fault_config.enabled;
        let faultLabel = 'None';
        if (faultIsEnabled && sc && sc.scenario_type && sc.scenario_type !== 'none') {
            const intensity = sc.intensity;
            faultLabel = (intensity != null && !isNaN(intensity))
                ? `${sc.label} (${(intensity * 100).toFixed(0)}%)`
                : sc.label || 'Custom';
        } else if (faultIsEnabled) {
            faultLabel = 'Custom';
        }
        const items = [
            ['Topology', s.topology || '\u2014'],
            ['Agents', agentCount],
            ['Solver', s.solver || '\u2014'],
            ['Scheduler', scheduler],
            ['Faults', faultLabel],
            ['Duration', s.tick ? s.tick + ' ticks' : '\u2014'],
            ['Seed', s.seed != null ? s.seed : '\u2014'],
        ];
        configDl.innerHTML = items.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join('');
    }
}

// ---------------------------------------------------------------------------
// Floating visualization toolbar (observe + analyze phases)
// ---------------------------------------------------------------------------

function initVizToolbar() {
    const cb = document.getElementById('viz-heatmap-cb');
    const modeD = document.getElementById('viz-mode-d');
    const modeT = document.getElementById('viz-mode-t');
    const modeC = document.getElementById('viz-mode-c');
    const radius = document.getElementById('viz-radius');
    const opacity = document.getElementById('viz-opacity');
    const radiusGroup = document.getElementById('viz-radius-group');

    if (!cb) return;

    // Sync with left-panel heatmap checkbox
    cb.addEventListener('change', () => {
        sendCommand({ type: 'set_analysis_param', key: 'heatmap_visible', value: cb.checked });
        // Keep left-panel checkbox in sync
        const leftCb = document.getElementById('input-heatmap');
        if (leftCb) leftCb.checked = cb.checked;
    });

    function setMode(mode) {
        vizModeChangeTime = Date.now();
        sendCommand({ type: 'set_heatmap_mode', value: mode });
        [modeD, modeT, modeC].forEach(b => b.classList.remove('active'));
        if (mode === 'density') { modeD.classList.add('active'); radiusGroup.style.display = ''; }
        else if (mode === 'traffic') { modeT.classList.add('active'); radiusGroup.style.display = 'none'; }
        else { modeC.classList.add('active'); radiusGroup.style.display = 'none'; }
        // Also sync left-panel buttons
        const ld = document.getElementById('btn-heatmap-density');
        const lt = document.getElementById('btn-heatmap-traffic');
        const lc = document.getElementById('btn-heatmap-criticality');
        if (ld) ld.classList.toggle('active', mode === 'density');
        if (lt) lt.classList.toggle('active', mode === 'traffic');
        if (lc) lc.classList.toggle('active', mode === 'criticality');
        const lrg = document.getElementById('density-radius-group');
        if (lrg) lrg.style.display = mode === 'density' ? '' : 'none';
    }

    modeD.addEventListener('click', () => setMode('density'));
    modeT.addEventListener('click', () => setMode('traffic'));
    modeC.addEventListener('click', () => setMode('criticality'));

    radius.addEventListener('input', () => {
        sendCommand({ type: 'set_density_radius', value: parseInt(radius.value) });
    });

    opacity.addEventListener('input', () => {
        sendCommand({ type: 'set_robot_opacity', value: parseFloat(opacity.value) });
    });
}

// ---------------------------------------------------------------------------
// Context menu (viewport right-click / click-to-select)
// ---------------------------------------------------------------------------

function initContextMenu() {
    const menu = document.getElementById('viewport-context-menu');
    if (!menu) return;

    // Prevent mouse events from propagating to the Bevy canvas
    // (otherwise Bevy detects press+release as a new viewport click)
    menu.addEventListener('mousedown', e => e.stopPropagation());
    menu.addEventListener('mouseup', e => e.stopPropagation());
    menu.addEventListener('click', e => e.stopPropagation());

    // Action buttons
    document.getElementById('ctx-kill-agent')?.addEventListener('click', () => {
        if (ctxMenuAgentId !== null) {
            sendCommand({ type: 'kill_agent', value: ctxMenuAgentId });
        }
        closeContextMenu();
    });

    document.getElementById('ctx-slow-agent')?.addEventListener('click', () => {
        if (ctxMenuAgentId !== null) {
            sendCommand({ type: 'inject_latency', value: ctxMenuAgentId, duration: 30 });
        }
        closeContextMenu();
    });

    document.getElementById('ctx-place-wall')?.addEventListener('click', () => {
        if (ctxMenuCell) {
            sendCommand({ type: 'place_obstacle', x: ctxMenuCell[0], y: ctxMenuCell[1] });
        }
        closeContextMenu();
    });

    // Close on click outside
    document.addEventListener('mousedown', (e) => {
        if (contextMenuOpen && !menu.contains(e.target)) {
            closeContextMenu();
        }
    });

    // Close on Escape
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && contextMenuOpen) {
            closeContextMenu();
        }
    });
}

function openContextMenu(sel) {
    const menu = document.getElementById('viewport-context-menu');
    const container = document.getElementById('canvas-container');
    if (!menu || !container) return;

    ctxMenuCell = sel.cell;
    ctxMenuAgentId = sel.agent_index ?? null;

    // Header
    const header = document.getElementById('ctx-menu-header');
    if (header && sel.cell) {
        header.textContent = sel.agent_index !== null && sel.agent_index !== undefined
            ? `Agent ${sel.agent_index} · (${sel.cell[0]}, ${sel.cell[1]})`
            : `Cell (${sel.cell[0]}, ${sel.cell[1]})`;
    }

    // Show/hide agent actions
    const hasAgent = sel.agent_index !== null && sel.agent_index !== undefined;
    const agentActions = document.getElementById('ctx-menu-agent-actions');
    if (agentActions) {
        agentActions.style.display = hasAgent ? '' : 'none';
    }

    // Hide wall options when an agent is selected (can't place a wall on a robot)
    const wallBtn = document.getElementById('ctx-place-wall');
    const tempWallBtn = document.getElementById('ctx-place-temp-wall');
    if (wallBtn) wallBtn.style.display = hasAgent ? 'none' : '';
    if (tempWallBtn) tempWallBtn.style.display = hasAgent ? 'none' : '';

    // Position relative to canvas-container
    const rect = container.getBoundingClientRect();
    let left = sel.screen_x;
    let top = sel.screen_y;

    // Clamp to container bounds
    const menuW = 200; // approximate
    const menuH = 160;
    if (left + menuW > rect.width) left = rect.width - menuW;
    if (top + menuH > rect.height) top = rect.height - menuH;
    if (left < 0) left = 0;
    if (top < 0) top = 0;

    menu.style.left = left + 'px';
    menu.style.top = top + 'px';
    menu.classList.remove('hidden');
    contextMenuOpen = true;

    // Also select the agent in the right panel if applicable
    if (sel.agent_index !== null && sel.agent_index !== undefined) {
        selectedAgentId = sel.agent_index;
        sendCommand({ type: 'select_agent', value: sel.agent_index });
    }
}

function closeContextMenu() {
    const menu = document.getElementById('viewport-context-menu');
    if (menu) menu.classList.add('hidden');
    contextMenuOpen = false;
    ctxMenuAgentId = null;
    ctxMenuCell = null;
    sendCommand({ type: 'clear_selection' });
}

// ---------------------------------------------------------------------------
// Live verdict (compact, in Fault Response section)
// ---------------------------------------------------------------------------
function updateLiveVerdict(sc) {
    const label = document.getElementById('live-verdict-label');
    const dots = document.getElementById('live-verdict-dots');
    if (!label || !dots) return;

    const { composite: avg, filledDots: filled } = computeCompositeScore(sc);
    let text = 'Unknown';
    if (avg >= 0.8) text = 'Resilient';
    else if (avg >= 0.6) text = 'Moderate';
    else if (avg >= 0.4) text = 'Degraded';
    else text = 'Fragile';
    label.textContent = text;
    dots.innerHTML = Array.from({length: 5}, (_, i) =>
        `<div class="dot${i < filled ? ' filled' : ''}"></div>`
    ).join('');
}

// ---------------------------------------------------------------------------
// Live scorecard bars (compact, in Fault Response section)
// ---------------------------------------------------------------------------
function updateLiveScorecard(sc) {
    const container = document.getElementById('live-scorecard-bars');
    if (!container) return;

    const metrics = [
        { name: 'FT', value: sc.fault_tolerance || 0 },
        { name: 'NRR', value: sc.nrr != null ? sc.nrr : 0 },
        { name: 'FUR', value: sc.fleet_utilization || 0 },
    ];
    container.innerHTML = metrics.map(m => {
        const pct = (m.value * 100).toFixed(0);
        const color = m.value >= 0.7 ? 'var(--state-moving)' : m.value >= 0.4 ? 'var(--state-delayed)' : 'var(--state-fault)';
        return `<div style="display:flex;align-items:center;gap:4px;margin-bottom:2px;">
            <span style="width:28px;font-size:9px;color:var(--text-muted);text-transform:uppercase;">${m.name}</span>
            <div style="flex:1;height:4px;background:var(--bg-card);border:1px solid var(--border);">
                <div style="width:${pct}%;height:100%;background:${color};"></div>
            </div>
            <span style="width:24px;font-size:9px;color:var(--text-secondary);text-align:right;">${pct}%</span>
        </div>`;
    }).join('');
}

// ---------------------------------------------------------------------------
// Charts (uPlot)
// ---------------------------------------------------------------------------

function initCharts() {
    if (typeof uPlot === 'undefined') return;

    // Helper to get CSS variable values
    const root = document.documentElement;
    const colorPrimary = getComputedStyle(root).getPropertyValue('--text-primary').trim() || '#fff';
    const colorMuted = getComputedStyle(root).getPropertyValue('--text-muted').trim() || '#888';
    const gridColor = 'rgba(128,128,128,0.1)';
    const font = "11px 'DM Mono', monospace";

    const commonOpts = {
        width: 100, // Starting width, ResizeObserver will fix
        height: 140,
        cursor: {
            lock: true,
            focus: { prox: 16 },
            points: { show: true, size: 6, fill: colorPrimary },
        },
        legend: { show: false },
        scales: {
            x: { time: false }
        },
        axes: [
            {
                grid: { show: false },
                stroke: colorMuted,
                font: font,
                space: 40,
                incrs: [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000],
                values: (u, vals) => vals.map(v => Math.round(v))
            },
            {
                grid: { stroke: gridColor, width: 1 },
                stroke: colorMuted,
                font: font,
                size: 50,
                gap: 5,
            }
        ]
    };

    const makeUplot = (id, title, series) => {
        const el = document.getElementById(id);
        if (!el) return null;

        // Create tooltip element
        const tooltip = document.createElement('div');
        tooltip.className = 'uplot-tooltip';
        tooltip.style.display = 'none';
        el.style.position = 'relative';
        el.appendChild(tooltip);

        let opts = {
            ...commonOpts,
            title: title,
            series: [
                { label: 'Tick' }, // X-axis
                ...series
            ],
            hooks: {
                setCursor: [(u) => {
                    const idx = u.cursor.idx;
                    if (idx == null) {
                        tooltip.style.display = 'none';
                        return;
                    }
                    const xVal = u.data[0][idx];
                    let lines = ['T' + Math.round(xVal)];
                    for (let si = 1; si < u.series.length; si++) {
                        if (!u.series[si].show) continue;
                        const yVal = u.data[si][idx];
                        if (yVal != null) {
                            const label = u.series[si].label || '';
                            lines.push(label + ': ' + (yVal % 1 === 0 ? yVal : yVal.toFixed(2)));
                        }
                    }
                    tooltip.innerHTML = lines.join('<br>');
                    const left = u.valToPos(xVal, 'x');
                    tooltip.style.display = 'block';
                    tooltip.style.left = Math.max(0, Math.min(left - 30, el.offsetWidth - 120)) + 'px';
                }]
            }
        };

        const chart = new uPlot(opts, [[], ...series.map(() => [])], el);
        return chart;
    };

    // ═══ System Performance charts ═══
    // Throughput: raw (faint) + 10-tick MA (bold) + baseline MA (dotted)
    // In GAP mode: single line showing live_MA - baseline_MA
    chartInsts.throughput = makeUplot('chart-throughput', 'THROUGHPUT (GOALS/TICK)', [
        { label: 'Per-Tick', stroke: 'rgba(143,58,222,0.3)', width: 1 },
        { label: 'Moving Avg (10)', stroke: 'rgb(143,58,222)', fill: 'rgba(143,58,222,0.1)', width: 2 },
        { label: 'Baseline Moving Avg', stroke: 'rgba(140,140,148,0.6)', width: 1, dash: [4, 4] }
    ]);

    chartInsts.tasks = makeUplot('chart-tasks', 'TASKS COMPLETED (CUMULATIVE)', [
        { label: 'Tasks', stroke: 'rgb(45,160,0)', fill: 'rgba(45,160,0,0.1)', width: 2 },
        { label: 'Baseline', stroke: 'rgba(140,140,148,0.6)', width: 1, dash: [4, 4] }
    ]);

    // ═══ Fault Response charts ═══
    chartInsts.heat = makeUplot('chart-heat', 'Agent Wear', [
        { label: 'Avg Wear', stroke: 'rgb(230,140,0)', fill: 'rgba(230,140,0,0.15)', width: 2 }
    ]);

    // Setup ResizeObserver for responsive uPlot resizing
    if (!resizeObserver) {
        resizeObserver = new ResizeObserver(entries => {
            for (let entry of entries) {
                const width = entry.contentRect.width;
                // Update all charts to new width
                Object.values(chartInsts).forEach(chart => {
                    if (chart) chart.setSize({ width, height: 140 });
                });
            }
        });

        // Observe section containers to dictate width
        const perfSection = document.getElementById('system-perf-content');
        if (perfSection) resizeObserver.observe(perfSection);
        const faultSection = document.getElementById('fault-response-content');
        if (faultSection) resizeObserver.observe(faultSection);
    }

    // Inject chart toggle buttons into uPlot title elements
    injectChartToggleButtons();
}

function injectChartToggleButtons() {
    // Throughput chart: [RAW] / [GAP] toggle — label shows CURRENT mode
    const tpEl = document.getElementById('chart-throughput');
    if (tpEl) {
        const titleEl = tpEl.querySelector('.u-title');
        if (titleEl && !titleEl.querySelector('.chart-toggle-btn')) {
            const btn = document.createElement('button');
            btn.className = 'chart-toggle-btn';
            btn.textContent = throughputChartMode === 'raw' ? '[RAW]' : '[GAP]';
            btn.title = 'Toggle gap mode (live MA - baseline MA)';
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                throughputChartMode = throughputChartMode === 'raw' ? 'gap' : 'raw';
                btn.textContent = throughputChartMode === 'raw' ? '[RAW]' : '[GAP]';
                updateChartLegends();
                redrawChartsForMode();
            });
            titleEl.style.position = 'relative';
            titleEl.appendChild(btn);
        }
    }

    // Tasks chart: [ABS] / [NORM] toggle — label shows CURRENT mode
    const taskEl = document.getElementById('chart-tasks');
    if (taskEl) {
        const titleEl = taskEl.querySelector('.u-title');
        if (titleEl && !titleEl.querySelector('.chart-toggle-btn')) {
            const btn = document.createElement('button');
            btn.className = 'chart-toggle-btn';
            btn.textContent = tasksChartMode === 'abs' ? '[ABS]' : '[NORM]';
            btn.title = 'Toggle normalized mode (live / baseline)';
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                tasksChartMode = tasksChartMode === 'abs' ? 'norm' : 'abs';
                btn.textContent = tasksChartMode === 'abs' ? '[ABS]' : '[NORM]';
                updateChartLegends();
                redrawChartsForMode();
            });
            titleEl.style.position = 'relative';
            titleEl.appendChild(btn);
        }
    }

    // Set initial legends
    updateChartLegends();
}

// Update live chart legends based on current mode and whether faults are active
function updateChartLegends() {
    const hasFaults = _lastShowBaseline;
    const liveLabel = hasFaults ? 'Live' : 'MVA';
    const tpLegend = document.querySelector('#chart-throughput .chart-legend');
    if (tpLegend) {
        if (throughputChartMode === 'gap') {
            tpLegend.innerHTML = '<span style="color:rgb(143,58,222)">\u2501\u2501</span> Gap (MVA Live \u2212 MVA Baseline) &nbsp; <span style="color:rgba(140,140,148,0.4)">\u2508\u2508</span> Zero';
        } else if (hasFaults) {
            tpLegend.innerHTML = '<span style="color:rgba(143,58,222,0.3)">\u2501\u2501</span> Per-Tick &nbsp; <span style="color:rgb(143,58,222)">\u2501\u2501</span> MVA (Live) &nbsp; <span style="color:rgba(140,140,148,0.6)">\u2508\u2508</span> MVA (Baseline)';
        } else {
            tpLegend.innerHTML = '<span style="color:rgba(143,58,222,0.3)">\u2501\u2501</span> Per-Tick &nbsp; <span style="color:rgb(143,58,222)">\u2501\u2501</span> MVA';
        }
    }
    const taskLegend = document.querySelector('#chart-tasks .chart-legend');
    if (taskLegend) {
        if (tasksChartMode === 'norm') {
            taskLegend.innerHTML = '<span style="color:rgb(45,160,0)">\u2501\u2501</span> Ratio (Live / Baseline) &nbsp; <span style="color:rgba(140,140,148,0.4)">\u2508\u2508</span> 1.0';
        } else if (hasFaults) {
            taskLegend.innerHTML = '<span style="color:rgb(45,160,0)">\u2501\u2501</span> Live &nbsp; <span style="color:rgba(140,140,148,0.6)">\u2508\u2508</span> Baseline';
        } else {
            taskLegend.innerHTML = '<span style="color:rgb(45,160,0)">\u2501\u2501</span> Tasks';
        }
    }
}

// Redraw live charts immediately using current chartData (no new tick needed)
function redrawChartsForMode() {
    if (!chartInsts.throughput || chartData.ticks.length === 0) return;

    // Ensure MA arrays exist
    if (!chartData._tpMA) { chartData._tpMA = []; chartData._blTpMA = []; }
    const tpMA = chartData._tpMA;
    const blTpMA = chartData._blTpMA;

    if (chartInsts.throughput) {
        if (throughputChartMode === 'gap' && _lastShowBaseline) {
            const gapData = tpMA.map((v, i) => {
                const bl = blTpMA[i];
                return (v != null && bl != null) ? v - bl : null;
            });
            chartInsts.throughput.setData([chartData.ticks, gapData, gapData.map(() => null), gapData.map(() => 0)]);
        } else {
            chartInsts.throughput.setData([chartData.ticks, chartData.throughput, tpMA, blTpMA]);
        }
    }

    if (chartInsts.tasks) {
        if (tasksChartMode === 'norm' && _lastShowBaseline) {
            const normData = chartData.tasksCumulative.map((v, i) => {
                const bl = chartData.baselineTasksCumulative[i];
                return (bl != null && bl > 0) ? v / bl : null;
            });
            chartInsts.tasks.setData([chartData.ticks, normData, normData.map(() => 1.0)]);
        } else {
            chartInsts.tasks.setData([chartData.ticks, chartData.tasksCumulative, chartData.baselineTasksCumulative]);
        }
    }
}

function updateChartData(s) {
    if (!chartInsts.throughput && typeof uPlot !== 'undefined') {
        initCharts();
    }
    if (!chartInsts.throughput) return;
    if (s.state !== 'running' && s.state !== 'paused') return;
    if (s.tick === lastChartTick) return;
    if (s.tick === 0) return; // skip tick 0 — agents haven't been assigned tasks yet

    // Detect rewind: current tick is earlier than last charted tick.
    // Truncate chart arrays to remove stale future data before appending.
    if (s.tick < lastChartTick && chartData.ticks.length > 0) {
        const cutIndex = chartData.ticks.findIndex(t => t >= s.tick);
        if (cutIndex >= 0) {
            Object.values(chartData).forEach(arr => { arr.length = cutIndex; });
        }
    }

    lastChartTick = s.tick;

    // Compute avg heat — use aggregate summary when available
    let avgHeat = 0;
    if (s.agent_summary) {
        avgHeat = s.agent_summary.avg_heat;
    } else {
        let totalHeat = 0;
        const agents = s.agents || [];
        agents.forEach(a => { totalHeat += a.heat_normalized; });
        avgHeat = agents.length > 0 ? totalHeat / agents.length : 0;
    }

    // Extract metrics
    const m = s.metrics || {};
    const throughput = m.throughput || 0.0;

    // Baseline series — show only when faults actually exist (observed or scheduled).
    // After fault deletion, scenario.enabled stays true but all events are gone,
    // so we check for concrete faults rather than the config flag.
    const bd = s.baseline_diff;
    const _hasFaultEvents = (s.metrics && s.metrics.fault_count > 0);
    const _hasScheduledFaults = s.fault_schedule_markers && s.fault_schedule_markers.length > 0;
    const _showBaseline = bd && bd.has_baseline && (_hasFaultEvents || _hasScheduledFaults);
    // Update legends when fault state changes
    if (_showBaseline !== _lastShowBaseline) {
        _lastShowBaseline = _showBaseline;
        updateChartLegends();
    }
    _lastShowBaseline = _showBaseline; // cache for redrawChartsForMode
    const blThroughput = _showBaseline ? (bd.baseline_throughput != null ? bd.baseline_throughput : null) : null;
    const liveTasks = s.lifelong ? s.lifelong.tasks_completed : 0;
    const blTasks = _showBaseline ? (bd.baseline_tasks_at_tick != null ? bd.baseline_tasks_at_tick : null) : null;

    // Always show legends (they provide context even in baseline-only mode)
    document.querySelectorAll('#system-perf-content .chart-legend').forEach(el => {
        el.style.display = '';
    });

    // Show/hide chart toggle buttons based on baseline availability
    const tpToggle = document.querySelector('#chart-throughput .chart-toggle-btn');
    const taskToggle = document.querySelector('#chart-tasks .chart-toggle-btn');
    if (tpToggle) tpToggle.style.display = _showBaseline ? '' : 'none';
    if (taskToggle) taskToggle.style.display = _showBaseline ? '' : 'none';
    if (!_showBaseline) {
        if (throughputChartMode !== 'raw') throughputChartMode = 'raw';
        if (tasksChartMode !== 'abs') tasksChartMode = 'abs';
    }

    // One-time diagnostic log when baseline data first arrives
    if (_showBaseline && !_baselineLoggedForRun) {
        _baselineLoggedForRun = true;
        console.log('[BASELINE JS] total_tasks:', bd.baseline_total_tasks,
            'avg_tp:', bd.baseline_avg_throughput,
            'tasks_at_tick:', bd.baseline_tasks_at_tick,
            'tick:', s.tick);
    }

    chartData.ticks.push(s.tick);
    chartData.avgHeat.push(avgHeat);
    chartData.alive.push(s.alive_agents);
    chartData.dead.push(s.dead_agents);
    chartData.throughput.push(throughput);
    chartData.baselineThroughput.push(blThroughput);
    chartData.tasksCumulative.push(liveTasks);
    chartData.baselineTasksCumulative.push(blTasks);
    chartData.idleRatio.push(s.metrics ? s.metrics.idle_ratio : null);
    chartData.baselineIdleRatio.push(bd && bd.has_baseline ? bd.baseline_wait_ratio_at_tick : null);
    chartData.cascadeSpread.push(s.metrics ? s.metrics.avg_cascade_spread : null);

    // Sliding window
    if (chartData.ticks.length > CHART_MAX_POINTS) {
        Object.values(chartData).forEach(arr => arr.shift());
    }

    // Incrementally append moving-average values for throughput
    const MA_WINDOW = 10;
    if (!chartData._tpMA) { chartData._tpMA = []; chartData._blTpMA = []; }
    appendMA(chartData.throughput, chartData._tpMA, MA_WINDOW);
    appendMA(chartData.baselineThroughput, chartData._blTpMA, MA_WINDOW);
    // Trim MA arrays to match data after sliding-window shift
    while (chartData._tpMA.length > chartData.ticks.length) { chartData._tpMA.shift(); chartData._blTpMA.shift(); }
    const tpMA = chartData._tpMA;
    const blTpMA = chartData._blTpMA;

    // Pass data arrays to uPlot based on chart mode
    if (chartInsts.throughput) {
        if (throughputChartMode === 'gap' && _showBaseline) {
            // GAP mode: single line = live_MA - baseline_MA, with zero reference
            const gapData = tpMA.map((v, i) => {
                const bl = blTpMA[i];
                return (v != null && bl != null) ? v - bl : null;
            });
            chartInsts.throughput.setData([chartData.ticks, gapData, gapData.map(() => null), gapData.map(() => 0)]);
        } else {
            // RAW mode: raw (faint) + MA (bold) + baseline MA (dotted)
            chartInsts.throughput.setData([chartData.ticks, chartData.throughput, tpMA, blTpMA]);
        }
    }

    if (chartInsts.tasks) {
        if (tasksChartMode === 'norm' && _showBaseline) {
            // NORM mode: single line = live / baseline, reference at 1.0
            const normData = chartData.tasksCumulative.map((v, i) => {
                const bl = chartData.baselineTasksCumulative[i];
                return (bl != null && bl > 0) ? v / bl : null;
            });
            chartInsts.tasks.setData([chartData.ticks, normData, normData.map(() => 1.0)]);
        } else {
            chartInsts.tasks.setData([chartData.ticks, chartData.tasksCumulative, chartData.baselineTasksCumulative]);
        }
    }

    if (chartInsts.heat) chartInsts.heat.setData([chartData.ticks, chartData.avgHeat]);
}

// Append a single moving-average value to `maArr` for the latest point in `srcArr`.
// Only examines the last `window` elements of `srcArr` — O(window) not O(n).
function appendMA(srcArr, maArr, window) {
    const len = srcArr.length;
    let sum = 0, count = 0;
    for (let j = Math.max(0, len - window); j < len; j++) {
        if (srcArr[j] != null) { sum += srcArr[j]; count++; }
    }
    maArr.push(count > 0 ? sum / count : null);
}

// Full recompute (used after chart reset or mode switch).
function computeMA(arr, window) {
    const result = new Array(arr.length);
    for (let i = 0; i < arr.length; i++) {
        let sum = 0, count = 0;
        for (let j = Math.max(0, i - window + 1); j <= i; j++) {
            if (arr[j] != null) { sum += arr[j]; count++; }
        }
        result[i] = count > 0 ? sum / count : null;
    }
    return result;
}

function resetChartData() {
    chartData = { ticks: [], avgHeat: [], alive: [], dead: [], throughput: [], baselineThroughput: [], tasksCumulative: [], baselineTasksCumulative: [], idleRatio: [], baselineIdleRatio: [], cascadeSpread: [], _tpMA: [], _blTpMA: [] };
    throughputChartMode = 'raw';
    tasksChartMode = 'abs';
    resultsThrouputMode = 'raw';
    resultsTasksMode = 'abs';
    _lastShowBaseline = false;
    lastChartTick = -1;

    // Reset any results panel toggle buttons
    const resTpBtn = document.querySelector('#results-chart-throughput .chart-toggle-btn');
    if (resTpBtn) resTpBtn.textContent = '[RAW]';
    const resTaskBtn = document.querySelector('#results-chart-tasks .chart-toggle-btn');
    if (resTaskBtn) resTaskBtn.textContent = '[ABS]';
    _baselineLoggedForRun = false;

    lastTimelineMarkerCount = -1;
    lastFaultEventCount = -1;
    _tlScheduledEls = [];
    _tlManualCount = 0;
    _tlLastTick = -1;
    _tlLastDuration = -1;
    _tlLastMaxSnap = -1;
    _lastSeekTick = -1;

    // Clear all timeline marker DOM elements
    if (_tlTrack) {
        const existing = _tlTrack.querySelectorAll('.timeline-marker');
        for (let i = existing.length - 1; i >= 0; i--) existing[i].remove();
    }
    hideTimelinePopup();

    // Destroy existing chart instances to free DOM nodes and event listeners
    Object.values(chartInsts).forEach(chart => {
        if (chart) chart.destroy();
    });
    chartInsts = {};

    // Disconnect ResizeObserver to prevent accumulated callbacks
    if (resizeObserver) {
        resizeObserver.disconnect();
        resizeObserver = null;
    }
}

// ---------------------------------------------------------------------------
// Dark mode
// ---------------------------------------------------------------------------

function initTheme() {
    const saved = localStorage.getItem('mafis-theme');
    if (saved === 'dark') {
        document.documentElement.setAttribute('data-theme', 'dark');
        document.getElementById('btn-theme').textContent = '\u2600'; // sun
        // Sync Bevy ClearColor on next bridge poll
        setTimeout(() => sendCommand({ type: 'set_theme', value: 'dark' }), 500);
    }
}

function toggleTheme() {
    const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
    if (isDark) {
        document.documentElement.removeAttribute('data-theme');
        document.getElementById('btn-theme').textContent = '\u263D'; // moon
        localStorage.setItem('mafis-theme', 'light');
        sendCommand({ type: 'set_theme', value: 'light' });
    } else {
        document.documentElement.setAttribute('data-theme', 'dark');
        document.getElementById('btn-theme').textContent = '\u2600'; // sun
        localStorage.setItem('mafis-theme', 'dark');
        sendCommand({ type: 'set_theme', value: 'dark' });
    }
}

// ---------------------------------------------------------------------------
// Collapsible sections
// ---------------------------------------------------------------------------

const DEFAULT_OPEN_SECTIONS = ['sim-content', 'status-content', 'scorecard-content', 'system-perf-content'];

function initCollapsible() {
    document.querySelectorAll('.section-toggle').forEach(toggle => {
        const targetId = toggle.dataset.target;
        const label = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
        const startOpen = DEFAULT_OPEN_SECTIONS.includes(targetId);

        if (startOpen) {
            toggle.textContent = '\u25BE ' + label; // ▾ down = open
            toggle.setAttribute('aria-expanded', 'true');
        } else {
            toggle.textContent = '\u25B8 ' + label; // ▸ right = collapsed
            toggle.setAttribute('aria-expanded', 'false');
            const content = document.getElementById(targetId);
            if (content) content.classList.add('collapsed');
        }

        toggle.addEventListener('click', () => {
            const targetId = toggle.dataset.target;
            const content = document.getElementById(targetId);
            if (!content) return;

            const isCollapsed = content.classList.contains('collapsed');
            const text = toggle.textContent.replace(/^[▸▾►▼]\s*/, '').trim();
            if (isCollapsed) {
                content.classList.remove('collapsed');
                toggle.textContent = '\u25BE ' + text;
                toggle.setAttribute('aria-expanded', 'true');
            } else {
                content.classList.add('collapsed');
                toggle.textContent = '\u25B8 ' + text;
                toggle.setAttribute('aria-expanded', 'false');
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Graphics settings modal
// ---------------------------------------------------------------------------

const GRAPHICS_PRESETS = {
    low:    { shadows: false, msaa: false },
    medium: { shadows: false, msaa: true },
    high:   { shadows: true,  msaa: true },
};

let currentPreset = 'medium';

function initSettingsModal() {
    const overlay = document.getElementById('settings-overlay');
    const openBtn = document.getElementById('btn-settings');
    const closeBtn = document.getElementById('settings-close');
    if (!overlay || !openBtn) return;

    openBtn.addEventListener('click', () => overlay.classList.remove('hidden'));
    closeBtn.addEventListener('click', () => overlay.classList.add('hidden'));
    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) overlay.classList.add('hidden');
    });

    // Preset buttons
    document.querySelectorAll('.btn-preset').forEach(btn => {
        btn.addEventListener('click', () => {
            const preset = btn.dataset.preset;
            applyPreset(preset);
        });
    });

    // Individual toggles
    document.getElementById('setting-shadows').addEventListener('change', (e) => {
        sendCommand({ type: 'set_graphics', key: 'shadows', value: e.target.checked });
        updatePresetHighlight();
        updateRobotPreview();
        saveGraphicsSettings();
    });
    document.getElementById('setting-msaa').addEventListener('change', (e) => {
        sendCommand({ type: 'set_graphics', key: 'msaa', value: e.target.checked });
        updatePresetHighlight();
        saveGraphicsSettings();
    });
    document.getElementById('setting-colorblind').addEventListener('change', (e) => {
        sendCommand({ type: 'set_graphics', key: 'colorblind', value: e.target.checked });
        updatePresetHighlight();
        updateRobotPreview();
        saveGraphicsSettings();
    });
    document.getElementById('setting-detailed-states').addEventListener('change', (e) => {
        sendCommand({ type: 'set_graphics', key: 'detailed_states', value: e.target.checked });
        updateStateLegend(e.target.checked);
        saveGraphicsSettings();
    });

    // Keyboard layout override
    const kbSelect = document.getElementById('setting-keyboard');
    if (kbSelect) {
        kbSelect.addEventListener('change', () => {
            detectedLayout = kbSelect.value;
            localStorage.setItem('mafis-keyboard-layout', detectedLayout);
            syncKeyboardUI();
        });
    }
}

function applyPreset(name) {
    const preset = GRAPHICS_PRESETS[name];
    if (!preset) return;
    currentPreset = name;

    document.getElementById('setting-shadows').checked = preset.shadows;
    document.getElementById('setting-msaa').checked = preset.msaa;

    sendCommand({ type: 'set_graphics', key: 'shadows', value: preset.shadows });
    sendCommand({ type: 'set_graphics', key: 'msaa', value: preset.msaa });

    updatePresetHighlight();
    updateRobotPreview();
    saveGraphicsSettings();
}

function saveGraphicsSettings() {
    const settings = {
        shadows: document.getElementById('setting-shadows').checked,
        msaa: document.getElementById('setting-msaa').checked,
        colorblind: document.getElementById('setting-colorblind').checked,
        detailed_states: document.getElementById('setting-detailed-states').checked,
    };
    localStorage.setItem('mafis-graphics', JSON.stringify(settings));
}

function updateStateLegend(detailed) {
    const simple = document.getElementById('state-legend-simple');
    const det = document.getElementById('state-legend-detailed');
    if (simple) simple.style.display = detailed ? 'none' : '';
    if (det) det.style.display = detailed ? '' : 'none';
}

function loadGraphicsSettings() {
    const raw = localStorage.getItem('mafis-graphics');
    if (!raw) return;
    try {
        const settings = JSON.parse(raw);
        document.getElementById('setting-shadows').checked = !!settings.shadows;
        document.getElementById('setting-msaa').checked = settings.msaa !== false;
        document.getElementById('setting-colorblind').checked = !!settings.colorblind;
        document.getElementById('setting-detailed-states').checked = !!settings.detailed_states;

        sendCommand({ type: 'set_graphics', key: 'shadows', value: !!settings.shadows });
        sendCommand({ type: 'set_graphics', key: 'msaa', value: settings.msaa !== false });
        sendCommand({ type: 'set_graphics', key: 'colorblind', value: !!settings.colorblind });
        sendCommand({ type: 'set_graphics', key: 'detailed_states', value: !!settings.detailed_states });

        updateStateLegend(!!settings.detailed_states);
        updatePresetHighlight();
        updateRobotPreview();
    } catch (_) {}
}

function updatePresetHighlight() {
    const shadows = document.getElementById('setting-shadows').checked;
    const msaa = document.getElementById('setting-msaa').checked;

    let matched = null;
    for (const [name, p] of Object.entries(GRAPHICS_PRESETS)) {
        if (p.shadows === shadows && p.msaa === msaa) {
            matched = name;
            break;
        }
    }

    document.querySelectorAll('.btn-preset').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.preset === matched);
    });
}

function updateRobotPreview() {
    const model = document.getElementById('robot-model');
    if (!model) return;
    const shadows = document.getElementById('setting-shadows').checked;
    const colorblind = document.getElementById('setting-colorblind').checked;
    model.classList.toggle('shadows-on', shadows);
    model.classList.toggle('colorblind', colorblind);
}

// ---------------------------------------------------------------------------
// Performance warning modal
// ---------------------------------------------------------------------------

const PERF_DISMISS_KEY = 'mafis-perf-warning-dismissed';

function getConfigValues() {
    const agents = parseInt(document.getElementById('input-agents')?.value) || 5;
    const w = parseInt(document.getElementById('input-grid-width')?.value) || 16;
    const h = parseInt(document.getElementById('input-grid-height')?.value) || 16;
    return { agents, w, h, area: w * h };
}

function shouldWarnPerformance() {
    if (localStorage.getItem(PERF_DISMISS_KEY) === '1') return false;
    const { agents, area } = getConfigValues();
    return agents > WASM_WARN_AGENTS || area > WASM_WARN_GRID_AREA;
}

function estimateWasmFps(agents, area) {
    // Based on observed benchmarks from MEMORY.md
    if (agents > 400 || area > 40000) return '~5–15 FPS';
    if (agents > 300 || area > 25000) return '~15–30 FPS';
    if (agents > 200 || area > 16384) return '~30–50 FPS';
    return '~60+ FPS';
}

function estimateNativeFps(agents) {
    if (agents > 400) return '~60–90 FPS';
    if (agents > 200) return '~90–120 FPS';
    return '~120+ FPS';
}

function showPerfWarning() {
    const { agents, w, h, area } = getConfigValues();
    document.getElementById('perf-warn-agents').textContent = agents;
    document.getElementById('perf-warn-grid').textContent = w + '\u00d7' + h;
    document.getElementById('perf-warn-wasm-fps').textContent = estimateWasmFps(agents, area);
    document.getElementById('perf-warn-native-fps').textContent = estimateNativeFps(agents);
    document.getElementById('perf-warn-dismiss-check').checked = false;
    document.getElementById('perf-warning-overlay').classList.remove('hidden');
}

function hidePerfWarning() {
    document.getElementById('perf-warning-overlay').classList.add('hidden');
}

function initPerfWarning() {
    const overlay = document.getElementById('perf-warning-overlay');
    if (!overlay) return;

    document.getElementById('perf-warning-close').addEventListener('click', hidePerfWarning);
    document.getElementById('perf-warn-adjust').addEventListener('click', hidePerfWarning);

    document.getElementById('perf-warn-start').addEventListener('click', () => {
        if (document.getElementById('perf-warn-dismiss-check').checked) {
            localStorage.setItem(PERF_DISMISS_KEY, '1');
        }
        hidePerfWarning();
        actuallyStartSimulation();
    });

    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) hidePerfWarning();
    });
}

function actuallyStartSimulation() {
    sendCommand({ type: 'set_state', value: 'start' });
}

function tryStartSimulation() {
    const btnStart = document.getElementById('btn-start');
    if (!btnStart || btnStart.disabled) return;

    if (btnStart.textContent.includes('RESTART')) {
        // Restart flow — no warning needed
        sendCommand({ type: 'set_state', value: 'reset' });
        setTimeout(() => actuallyStartSimulation(), 150);
        return;
    }

    if (shouldWarnPerformance()) {
        showPerfWarning();
        return;
    }

    actuallyStartSimulation();
}

// ---------------------------------------------------------------------------
// Keyboard layout detection
// ---------------------------------------------------------------------------

let detectedLayout = 'qwerty';

async function detectKeyboardLayout() {
    // Check localStorage override first
    const saved = localStorage.getItem('mafis-keyboard-layout');
    if (saved) {
        detectedLayout = saved;
        syncKeyboardUI();
        return;
    }

    // Try navigator.keyboard API (Chrome/Edge)
    try {
        if (navigator.keyboard && navigator.keyboard.getLayoutMap) {
            const layoutMap = await navigator.keyboard.getLayoutMap();
            // On AZERTY, the 'KeyQ' physical key maps to 'a'
            const qKey = layoutMap.get('KeyQ');
            if (qKey === 'a') {
                detectedLayout = 'azerty';
            }
        }
    } catch (_) {
        // API not available — keep default
    }

    syncKeyboardUI();
}

function syncKeyboardUI() {
    const select = document.getElementById('setting-keyboard');
    if (select && select.value !== detectedLayout) {
        select.value = detectedLayout;
    }
}


// ---------------------------------------------------------------------------
// Keyboard shortcuts
// ---------------------------------------------------------------------------

function bindKeyboard() {
    document.addEventListener('keydown', (e) => {
        // Skip when focus is in input fields
        const tag = e.target.tagName.toLowerCase();
        if (tag === 'input' || tag === 'textarea' || tag === 'select') return;

        // Block shortcuts during demo (Escape skips the demo)
        if (demoController?.isActive()) {
            if (e.key === 'Escape') {
                e.preventDefault();
                demoController.skip();
            }
            return;
        }

        switch (e.key) {
            case ' ':
                e.preventDefault();
                handlePlayPauseRestart();
                break;
            case 'r':
            case 'R':
                sendCommand({ type: 'set_state', value: 'reset' });
                break;
            case 'n':
            case 'N':
                sendCommand({ type: 'step' });
                break;
            case 'ArrowLeft':
                e.preventDefault();
                sendCommand({ type: 'step_backward' });
                break;
            case 'ArrowRight':
                e.preventDefault();
                sendCommand({ type: 'step' });
                break;
            case 'Escape':
                // Close any overlay
                const compareOverlay = document.getElementById('compare-overlay');
                if (compareOverlay) compareOverlay.remove();
                break;
            case '1':
                sendCommand({ type: 'set_camera_preset', value: 'side' });
                break;
            case '2':
                sendCommand({ type: 'set_camera_preset', value: 'top' });
                break;
            case 'c':
            case 'C':
                sendCommand({ type: 'set_camera_preset', value: 'center' });
                break;
            case '+':
            case '=': {
                const slider = document.getElementById('input-tick-hz');
                const newVal = Math.min(30, parseFloat(slider.value) + 1);
                slider.value = newVal;
                document.getElementById('val-tick-hz').textContent = newVal;
                sendCommand({ type: 'set_tick_hz', value: newVal });
                break;
            }
            case '-': {
                const slider = document.getElementById('input-tick-hz');
                const newVal = Math.max(1, parseFloat(slider.value) - 1);
                slider.value = newVal;
                document.getElementById('val-tick-hz').textContent = newVal;
                sendCommand({ type: 'set_tick_hz', value: newVal });
                break;
            }
            case 'Tab':
                e.preventDefault();
                toggleRightPanel();
                break;
        }
    });
}

let rightPanelHidden = false;

function toggleRightPanel() {
    const grid = document.getElementById('app-grid');
    const panel = document.getElementById('panel-right');
    if (!grid || !panel) return;
    rightPanelHidden = !rightPanelHidden;
    if (rightPanelHidden) {
        panel.style.opacity = '0';
        panel.style.pointerEvents = 'none';
        panel.style.overflow = 'hidden';
        panel.style.padding = '0';
        panel.style.border = 'none';
        panel.style.width = '0';
        panel.style.minWidth = '0';
        // Override grid columns to give all space to viewport
        grid.style.gridTemplateColumns = '0 1fr 0';
    } else {
        panel.style.opacity = '';
        panel.style.pointerEvents = '';
        panel.style.overflow = '';
        panel.style.padding = '';
        panel.style.border = '';
        panel.style.width = '';
        panel.style.minWidth = '';
        // Remove override — let data-phase CSS rule take effect
        grid.style.gridTemplateColumns = '';
    }
}

function handlePlayPauseRestart() {
    const btnStart = document.getElementById('btn-start');
    const btnPause = document.getElementById('btn-pause');

    if (!btnStart.disabled) {
        tryStartSimulation();
    } else if (!btnPause.disabled) {
        if (btnPause.textContent.includes('RESUME') || btnPause.textContent.includes('LIVE')) {
            sendCommand({ type: 'set_state', value: 'resume' });
        } else {
            sendCommand({ type: 'set_state', value: 'pause' });
        }
    }
}

// ---------------------------------------------------------------------------
// Topology presets
// ---------------------------------------------------------------------------

// Topology registry — loaded dynamically from web/topologies/manifest.json
// Each entry: { filename, data (parsed JSON), name, id, imported (bool) }
let loadedTopologies = [];
// Separate list for experiment-only imported maps
let experimentImportedMaps = []; // [{ data, name, id }]

async function loadTopologyManifest() {
    try {
        const resp = await fetch('topologies/manifest.json');
        if (!resp.ok) return;
        const filenames = await resp.json();
        if (!Array.isArray(filenames) || filenames.length === 0) return;

        for (const filename of filenames) {
            try {
                const r = await fetch('topologies/' + filename);
                if (!r.ok) continue;
                const data = await r.json();
                const name = data.name || filename.replace(/\.json$/, '');
                const id = filename.replace(/\.json$/, '').replace(/[^a-zA-Z0-9_-]/g, '_');
                // Compute walkable_cells if not present
                if (!data.walkable_cells && data.cells) {
                    const walls = data.cells.filter(c => c.type === 'wall').length;
                    data.walkable_cells = (data.width * data.height) - walls;
                }
                loadedTopologies.push({ filename, data, name, id, imported: false });
            } catch (e) {
                console.warn('Failed to load topology:', filename, e);
            }
        }
    } catch (_) {
        // No manifest or network error — topologies will be empty
    }

    populateTopologyUI();
    populateExpTopologyUI();
}

function populateTopologyUI() {
    const group = document.getElementById('topology-presets');
    const emptyMsg = document.getElementById('topology-empty');
    if (!group) return;

    group.innerHTML = '';

    if (loadedTopologies.length === 0) {
        if (emptyMsg) emptyMsg.style.display = '';
        return;
    }
    if (emptyMsg) emptyMsg.style.display = 'none';

    for (let i = 0; i < loadedTopologies.length; i++) {
        const topo = loadedTopologies[i];
        const robots = topo.data.robots ? topo.data.robots.length : 0;
        const btn = document.createElement('button');
        btn.className = 'preset-btn' + (i === 0 ? ' active' : '');
        btn.dataset.topoIdx = i;
        btn.title = `${topo.name}: ${topo.data.width}x${topo.data.height}, ${robots} agents`;
        btn.innerHTML = `<span class="preset-name">${topo.name}</span>` +
            `<span class="preset-desc">${topo.data.width}&times;${topo.data.height} &middot; ${robots} agents</span>`;
        group.appendChild(btn);
    }

    // Apply first topology by default (unless loading custom map)
    const params = new URLSearchParams(window.location.search);
    if (params.get('source') !== 'custom' && !window._customMapPending && loadedTopologies.length > 0) {
        applyTopologyByIndex(0);
    }
    if (window._customMapPending) window._customMapPending = false;
}

// ---------------------------------------------------------------------------
// Experiment topology UI — dynamic checkboxes from loadedTopologies + imports
// ---------------------------------------------------------------------------

function getAllExpTopologies() {
    return [...loadedTopologies, ...experimentImportedMaps];
}

function getTopologyCapacity(topoId) {
    const all = getAllExpTopologies();
    const topo = all.find(t => t.id === topoId);
    if (!topo) return Infinity;
    return topo.data.walkable_cells || (topo.data.width * topo.data.height) || Infinity;
}

function populateExpTopologyUI() {
    const grid = document.getElementById('exp-topologies');
    const emptyMsg = document.getElementById('exp-topo-empty');
    if (!grid) return;

    grid.innerHTML = '';
    const all = getAllExpTopologies();

    if (all.length === 0) {
        if (emptyMsg) emptyMsg.style.display = '';
        return;
    }
    if (emptyMsg) emptyMsg.style.display = 'none';

    for (let i = 0; i < all.length; i++) {
        const topo = all[i];
        const cap = topo.data.walkable_cells || '?';
        const label = document.createElement('label');
        label.className = 'checkbox-label';

        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.value = topo.id;
        if (i === 0) cb.checked = true;
        cb.addEventListener('change', () => {
            document.querySelectorAll('.exp-preset-btn').forEach(b => b.classList.remove('active'));
            updateExpRunCount();
        });

        const span = document.createElement('span');
        let text = topo.name;
        text += ` <span class="capacity-hint">(${cap} cells)</span>`;
        if (topo.imported) {
            text += ` <button class="btn-remove-topo" data-topo-id="${topo.id}" title="Remove">&times;</button>`;
        }
        span.innerHTML = text;

        label.appendChild(cb);
        label.appendChild(span);
        grid.appendChild(label);
    }

    // Bind remove buttons
    grid.querySelectorAll('.btn-remove-topo').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.preventDefault();
            e.stopPropagation();
            const id = btn.dataset.topoId;
            experimentImportedMaps = experimentImportedMaps.filter(t => t.id !== id);
            populateExpTopologyUI();
            updateExpRunCount();
        });
    });
}

function importExpMap(file) {
    const reader = new FileReader();
    reader.onload = (e) => {
        try {
            const data = JSON.parse(e.target.result);
            if (!data.width || !data.height) {
                console.warn('Invalid map file: missing width/height');
                return;
            }
            const name = data.name || file.name.replace(/\.json$/, '');
            const id = 'imported_' + file.name.replace(/\.json$/, '').replace(/[^a-zA-Z0-9_-]/g, '_');

            // Avoid duplicates
            if (experimentImportedMaps.some(t => t.id === id)) {
                console.warn('Map already imported:', name);
                return;
            }

            // Compute walkable_cells if not present
            if (!data.walkable_cells && data.cells) {
                const walls = data.cells.filter(c => c.type === 'wall').length;
                data.walkable_cells = (data.width * data.height) - walls;
            }

            experimentImportedMaps.push({ data, name, id, imported: true });
            populateExpTopologyUI();
            updateExpRunCount();
        } catch (err) {
            console.error('Failed to parse map file:', err);
        }
    };
    reader.readAsText(file);
}

function initExpMapImport() {
    const btn = document.getElementById('exp-import-map-btn');
    const input = document.getElementById('exp-import-map-input');
    if (!btn || !input) return;

    btn.addEventListener('click', () => input.click());
    input.addEventListener('change', () => {
        for (const file of input.files) {
            importExpMap(file);
        }
        input.value = ''; // reset for re-import
    });
}

function applyTopologyByIndex(idx) {
    if (idx < 0 || idx >= loadedTopologies.length) return;
    const topo = loadedTopologies[idx];

    // Track selected topology ID for URL sharing
    activeTopologyId = topo.id;

    // Send the full map JSON as a custom map load
    sendCommand({ type: 'load_custom_map', ...topo.data });

    // Use suggested_agents if available, else count robots, else 0
    const agents = topo.data.suggested_agents || (topo.data.robots ? topo.data.robots.length : 0) || 0;
    const capacity = topo.data.walkable_cells || 0;
    setSlider('input-grid-width', topo.data.width, 'val-grid-width');
    setSlider('input-grid-height', topo.data.height, 'val-grid-height');
    setSlider('input-agents', agents, 'val-agents');

    // Update capacity indicator
    const capEl = document.getElementById('map-capacity');
    if (capEl) capEl.textContent = capacity > 0 ? `max ${capacity}` : '';

    const densityGroup = document.getElementById('density-group');
    if (densityGroup) densityGroup.style.display = 'none';
}

function getTopologyDataById(id) {
    return loadedTopologies.find(t => t.id === id) || null;
}

function setSlider(inputId, value, displayId) {
    const input = document.getElementById(inputId);
    const display = document.getElementById(displayId);
    if (input) input.value = value;
    if (display) display.textContent = value;
}

function bindTopologyPresets() {
    const group = document.getElementById('topology-presets');
    if (!group) return;

    // Load topologies from manifest (async — populates buttons when done)
    loadTopologyManifest();

    group.addEventListener('click', (e) => {
        const btn = e.target.closest('.preset-btn');
        if (!btn || btn.disabled) return;

        const idx = parseInt(btn.dataset.topoIdx, 10);
        if (isNaN(idx)) return;

        // Update active state
        group.querySelectorAll('.preset-btn').forEach(b => b.classList.remove('active'));
        btn.classList.add('active');

        // Hide customize controls (JSON presets have fixed dimensions)
        const customControls = document.getElementById('sim-customize-controls');
        if (customControls) customControls.style.display = 'none';

        applyTopologyByIndex(idx);
    });
}

// ---------------------------------------------------------------------------
// Fault scenario — per-scenario params
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Multi-fault list
// ---------------------------------------------------------------------------

let faultList = [];
let faultIdCounter = 0;

function getAgentCount() {
    return parseInt(document.getElementById('input-agents')?.value || 20);
}

function getDuration() {
    return parseInt(document.getElementById('input-duration')?.value || 500);
}

function showFaultParams(type) {
    document.querySelectorAll('.fl-params').forEach(el => el.style.display = 'none');
    const map = { burst_failure: 'fl-params-burst', wear_based: 'fl-params-wear', zone_outage: 'fl-params-zone', intermittent_fault: 'fl-params-intermittent', permanent_zone_outage: 'fl-params-perm-zone' };
    const panel = document.getElementById(map[type]);
    if (panel) panel.style.display = '';
}

function updateFlBurstAbs() {
    const pct = parseFloat(document.getElementById('fl-burst-pct')?.value || 20);
    const agents = getAgentCount();
    const abs = Math.max(1, Math.round(agents * pct / 100));
    const label = document.getElementById('fl-burst-abs');
    if (label) label.textContent = `(= ${abs} robot${abs !== 1 ? 's' : ''})`;
}

function clampFlTickSliders() {
    const duration = getDuration();
    ['fl-burst-tick', 'fl-zone-tick', 'fl-perm-zone-tick'].forEach(id => {
        const el = document.getElementById(id);
        if (el) {
            el.max = duration;
            if (parseInt(el.value) > duration) el.value = duration;
        }
    });
}

function buildFaultItemFromForm() {
    const type = document.getElementById('input-fault-type').value;
    const item = { type };
    switch (type) {
        case 'burst_failure':
            item.kill_percent = parseFloat(document.getElementById('fl-burst-pct').value);
            item.at_tick = parseInt(document.getElementById('fl-burst-tick').value);
            break;
        case 'wear_based': {
            const active = document.querySelector('#fl-wear-presets .preset-btn.active');
            item.heat_rate = active ? active.dataset.rate : 'medium';
            break;
        }
        case 'zone_outage':
            item.at_tick = parseInt(document.getElementById('fl-zone-tick').value);
            item.duration = parseInt(document.getElementById('fl-zone-dur').value);
            break;
        case 'intermittent_fault':
            item.mtbf = parseInt(document.getElementById('fl-inter-mtbf').value);
            item.recovery = parseInt(document.getElementById('fl-inter-rec').value);
            break;
        case 'permanent_zone_outage':
            item.at_tick = parseInt(document.getElementById('fl-perm-zone-tick').value);
            item.block_percent = parseFloat(document.getElementById('fl-perm-zone-pct').value);
            break;
    }
    return item;
}

function addFault() {
    const type = document.getElementById('input-fault-type').value;

    // Enforce: max 1 wear, max 1 intermittent
    if (type === 'wear_based' && faultList.some(f => f.type === 'wear_based')) return;
    if (type === 'intermittent_fault' && faultList.some(f => f.type === 'intermittent_fault')) return;
    if (type === 'permanent_zone_outage' && faultList.some(f => f.type === 'permanent_zone_outage')) return;

    const item = buildFaultItemFromForm();

    // Auto-merge: same type + same tick → combine instead of creating duplicate
    if (type === 'burst_failure') {
        const existing = faultList.find(f => f.type === 'burst_failure' && f.at_tick === item.at_tick);
        if (existing) {
            existing.kill_percent = Math.min(100, existing.kill_percent + item.kill_percent);
            renderFaultList();
            syncFaultListToRust();
            return;
        }
        // Cap: total burst % across all ticks can't meaningfully exceed 100% per tick
        item.kill_percent = Math.min(100, item.kill_percent);
    }
    if (type === 'zone_outage') {
        const existing = faultList.find(f => f.type === 'zone_outage' && f.at_tick === item.at_tick);
        if (existing) {
            existing.duration = Math.max(existing.duration, item.duration);
            renderFaultList();
            syncFaultListToRust();
            return;
        }
    }

    item.id = 'f_' + (++faultIdCounter);
    faultList.push(item);
    renderFaultList();
    syncFaultListToRust();
}

function removeFault(id) {
    faultList = faultList.filter(f => f.id !== id);
    renderFaultList();
    syncFaultListToRust();
}

function syncFaultListToRust() {
    sendCommand({ type: 'set_fault_list', value: JSON.stringify(faultList) });
}

function faultBadgeClass(type) {
    return { burst_failure: 'burst', wear_based: 'wear', zone_outage: 'zone', intermittent_fault: 'intermittent', permanent_zone_outage: 'perm-zone' }[type] || 'burst';
}

function faultBadgeLabel(type) {
    return { burst_failure: 'BURST', wear_based: 'WEAR', zone_outage: 'ZONE', intermittent_fault: 'INTER', permanent_zone_outage: 'PERM' }[type] || type;
}

function faultSummary(item) {
    switch (item.type) {
        case 'burst_failure': return `${item.kill_percent}% at t=${item.at_tick}`;
        case 'wear_based': return `${(item.heat_rate || 'medium')} intensity`;
        case 'zone_outage': return `${item.duration}t at t=${item.at_tick}`;
        case 'intermittent_fault': return `MTBF=${item.mtbf} rec=${item.recovery}t`;
        case 'permanent_zone_outage': return `${item.block_percent}% at t=${item.at_tick}`;
        default: return '';
    }
}

function renderFaultList() {
    const container = document.getElementById('fault-list-items');
    if (!container) return;
    container.innerHTML = '';

    if (faultList.length === 0) {
        container.innerHTML = '<div class="fault-list-empty">No faults configured</div>';
        updateFaultTypeOptions();
        return;
    }

    faultList.forEach(item => {
        const row = document.createElement('div');
        row.className = 'fault-item-row';
        row.innerHTML = `
            <span class="fault-type-badge ${faultBadgeClass(item.type)}">${faultBadgeLabel(item.type)}</span>
            <span class="fault-item-summary">${faultSummary(item)}</span>
            <button class="fault-item-remove" title="Remove">&times;</button>
        `;
        row.querySelector('.fault-item-remove').addEventListener('click', () => removeFault(item.id));
        container.appendChild(row);
    });

    updateFaultTypeOptions();
}

function updateFaultTypeOptions() {
    const sel = document.getElementById('input-fault-type');
    if (!sel) return;
    const hasWear = faultList.some(f => f.type === 'wear_based');
    const hasInter = faultList.some(f => f.type === 'intermittent_fault');
    const hasPermZone = faultList.some(f => f.type === 'permanent_zone_outage');
    for (const opt of sel.options) {
        if (opt.value === 'wear_based') opt.disabled = hasWear;
        if (opt.value === 'intermittent_fault') opt.disabled = hasInter;
        if (opt.value === 'permanent_zone_outage') opt.disabled = hasPermZone;
    }
    // If current selection is disabled, switch to burst
    if (sel.options[sel.selectedIndex]?.disabled) {
        sel.value = 'burst_failure';
        showFaultParams('burst_failure');
    }
}

function bindFaultList() {
    // Type dropdown — switch param forms
    const typeEl = document.getElementById('input-fault-type');
    if (typeEl) {
        typeEl.addEventListener('change', () => showFaultParams(typeEl.value));
    }

    // Burst params
    bindSlider('fl-burst-pct', 'fl-burst-pct-val', v => {
        document.getElementById('fl-burst-pct-val').textContent = v + '%';
        updateFlBurstAbs();
    });
    bindSlider('fl-burst-tick', 'fl-burst-tick-val', () => {});

    // Wear presets
    document.querySelectorAll('#fl-wear-presets .preset-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            document.querySelectorAll('#fl-wear-presets .preset-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
        });
    });

    // Zone params
    bindSlider('fl-zone-tick', 'fl-zone-tick-val', () => {});
    bindSlider('fl-zone-dur', 'fl-zone-dur-val', () => {});

    // Intermittent params
    bindSlider('fl-inter-mtbf', 'fl-inter-mtbf-val', () => {});
    bindSlider('fl-inter-rec', 'fl-inter-rec-val', () => {});

    // Permanent zone outage params
    bindSlider('fl-perm-zone-tick', 'fl-perm-zone-tick-val', () => {});
    bindSlider('fl-perm-zone-pct', 'fl-perm-zone-pct-val', v => {
        document.getElementById('fl-perm-zone-pct-val').textContent = v + '%';
    });

    // ADD button
    document.getElementById('btn-add-fault')?.addEventListener('click', addFault);

    // Update burst abs label and tick sliders on agent/duration change
    document.getElementById('input-agents')?.addEventListener('input', () => { updateFlBurstAbs(); clampFlTickSliders(); });
    document.getElementById('input-duration')?.addEventListener('input', clampFlTickSliders);

    // Initial state
    updateFlBurstAbs();
    clampFlTickSliders();
}

// ---------------------------------------------------------------------------
// Control bindings
// ---------------------------------------------------------------------------

function bindControls() {
    // Scenario presets
    bindTopologyPresets();
    bindFaultList();

    // Toolbar buttons
    document.getElementById('btn-start').addEventListener('click', () => {
        tryStartSimulation();
    });
    document.getElementById('btn-pause').addEventListener('click', () => {
        const text = document.getElementById('btn-pause').textContent;
        if (text.includes('RESUME')) {
            sendCommand({ type: 'set_state', value: 'resume' });
        } else {
            sendCommand({ type: 'set_state', value: 'pause' });
        }
    });
    document.getElementById('btn-step').addEventListener('click', () => {
        sendCommand({ type: 'step' });
    });
    document.getElementById('btn-reset').addEventListener('click', () => {
        sendCommand({ type: 'set_state', value: 'reset' });
    });

    // Timeline bar
    initTimelineBar();

    // Theme toggle
    document.getElementById('btn-theme').addEventListener('click', toggleTheme);

    // Agent popover close
    document.getElementById('popover-close').addEventListener('click', () => {
        selectedAgentId = null;
        document.getElementById('agent-popover').classList.add('hidden');
    });

    // Kill agent button (adversarial mode)
    document.getElementById('btn-kill-agent').addEventListener('click', () => {
        if (selectedAgentId !== null) {
            sendCommand({ type: 'kill_agent', value: selectedAgentId });
        }
    });

    // Slow agent button (latency injection)
    document.getElementById('btn-slow-agent').addEventListener('click', () => {
        if (selectedAgentId !== null) {
            sendCommand({ type: 'inject_latency', value: selectedAgentId, duration: 20 });
        }
    });

    // Manual obstacle placement
    document.getElementById('btn-place-obstacle').addEventListener('click', () => {
        const x = parseInt(document.getElementById('input-fault-x').value) || 0;
        const y = parseInt(document.getElementById('input-fault-y').value) || 0;
        sendCommand({ type: 'place_obstacle', x, y });
    });
    // Tick speed slider
    bindSlider('input-tick-hz', 'val-tick-hz', v => {
        sendCommand({ type: 'set_tick_hz', value: parseFloat(v) });
    });

    // Simulation config
    bindSlider('input-agents', 'val-agents', v => {
        sendCommand({ type: 'set_num_agents', value: parseInt(v) });
    });
    bindSlider('input-grid-width', 'val-grid-width', v => {
        sendCommand({ type: 'set_grid_width', value: parseInt(v) });
    });
    bindSlider('input-grid-height', 'val-grid-height', v => {
        sendCommand({ type: 'set_grid_height', value: parseInt(v) });
    });
    bindNumberInput('input-seed', v => {
        sendCommand({ type: 'set_seed', value: parseInt(v) });
    });
    bindSlider('input-density', 'val-density', v => {
        sendCommand({ type: 'set_obstacle_density', value: parseFloat(v) });
    });

    // Solver selection + RHCR params toggle
    const solverSelect = document.getElementById('input-solver');
    const rhcrParams = document.getElementById('rhcr-params');
    const rhcrHorizon = document.getElementById('input-rhcr-horizon');
    const rhcrReplan = document.getElementById('input-rhcr-replan');
    const rhcrFallback = document.getElementById('input-rhcr-fallback');
    const rhcrAutoHint = document.getElementById('rhcr-auto-hint');

    function updateRhcrVisibility() {
        const isRhcr = solverSelect && solverSelect.value.startsWith('rhcr_');
        if (rhcrParams) rhcrParams.style.display = isRhcr ? '' : 'none';
        if (isRhcr && rhcrAutoHint) {
            rhcrAutoHint.textContent = 'Auto-tuned for current grid and agent count.';
        }
    }

    if (solverSelect) {
        solverSelect.addEventListener('change', () => {
            sendCommand({ type: 'set_solver', value: solverSelect.value });
            updateRhcrVisibility();
        });
        updateRhcrVisibility();
    }

    if (rhcrHorizon) {
        rhcrHorizon.addEventListener('change', () => {
            sendCommand({ type: 'set_rhcr_horizon', value: parseInt(rhcrHorizon.value) || 10 });
        });
    }
    if (rhcrReplan) {
        rhcrReplan.addEventListener('change', () => {
            sendCommand({ type: 'set_rhcr_replan_interval', value: parseInt(rhcrReplan.value) || 5 });
        });
    }
    if (rhcrFallback) {
        rhcrFallback.addEventListener('change', () => {
            sendCommand({ type: 'set_rhcr_fallback', value: rhcrFallback.value });
        });
    }

    const schedulerSelect = document.getElementById('input-scheduler');
    if (schedulerSelect) {
        schedulerSelect.addEventListener('change', () => {
            sendCommand({ type: 'set_scheduler', value: schedulerSelect.value });
        });
    }

    // Duration presets + custom input
    document.querySelectorAll('#duration-presets .preset-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            const d = parseInt(btn.dataset.duration);
            document.getElementById('input-duration').value = d;
            document.querySelectorAll('#duration-presets .preset-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            sendCommand({ type: 'set_duration', value: d });
            clampFlTickSliders();
        });
    });
    bindNumberInput('input-duration', v => {
        const d = parseInt(v) || 500;
        document.querySelectorAll('#duration-presets .preset-btn').forEach(b => {
            b.classList.toggle('active', parseInt(b.dataset.duration) === d);
        });
        sendCommand({ type: 'set_duration', value: d });
        clampFlTickSliders();
    });

    // Visualization config
    // (Show paths checkbox removed)
    bindSlider('input-robot-opacity', 'val-robot-opacity', v => {
        sendCommand({ type: 'set_robot_opacity', value: parseFloat(v) });
    });
    bindCheckbox('input-heatmap', v => {
        sendCommand({ type: 'set_analysis_param', key: 'heatmap_visible', value: v });
    });
    document.getElementById('btn-heatmap-density').addEventListener('click', () => {
        sendCommand({ type: 'set_heatmap_mode', value: 'density' });
    });
    document.getElementById('btn-heatmap-traffic').addEventListener('click', () => {
        sendCommand({ type: 'set_heatmap_mode', value: 'traffic' });
    });
    document.getElementById('btn-heatmap-criticality').addEventListener('click', () => {
        sendCommand({ type: 'set_heatmap_mode', value: 'criticality' });
    });
    bindSlider('input-density-radius', 'val-density-radius', v => {
        sendCommand({ type: 'set_density_radius', value: parseInt(v) });
    });

    // Metric toggles
    METRIC_KEYS.forEach(key => {
        bindCheckbox('metric-toggle-' + key, v => {
            sendCommand({ type: 'set_metric', key, value: v });
        });
    });

    // Export config
    bindCheckbox('input-auto-finished', v => {
        sendCommand({ type: 'set_export_param', key: 'auto_on_finished', value: String(v) });
    });
    bindCheckbox('input-auto-fault', v => {
        sendCommand({ type: 'set_export_param', key: 'auto_on_fault', value: String(v) });
    });
    bindCheckbox('input-periodic', v => {
        sendCommand({ type: 'set_export_param', key: 'periodic_enabled', value: String(v) });
        document.getElementById('input-periodic-interval').disabled = !v;
    });
    bindNumberInput('input-periodic-interval', v => {
        sendCommand({ type: 'set_export_param', key: 'periodic_interval', value: String(v) });
    });
    bindCheckbox('input-export-json', v => {
        sendCommand({ type: 'set_export_param', key: 'export_json', value: String(v) });
    });
    bindCheckbox('input-export-csv', v => {
        sendCommand({ type: 'set_export_param', key: 'export_csv', value: String(v) });
    });

    // Export Now button
    document.getElementById('btn-export-now').addEventListener('click', () => {
        sendCommand({ type: 'export_now' });
    });

    // Camera preset buttons
    document.getElementById('btn-view-center').addEventListener('click', () => {
        sendCommand({ type: 'set_camera_preset', value: 'center' });
    });
    document.getElementById('btn-view-top').addEventListener('click', () => {
        sendCommand({ type: 'set_camera_preset', value: 'top' });
    });

    // 2D/3D view mode toggle
    const viewModeBtn = document.getElementById('btn-view-mode');
    if (viewModeBtn) {
        viewModeBtn.addEventListener('click', () => {
            const is3d = viewModeBtn.textContent.trim() === '3D';
            sendCommand({ type: 'set_camera_mode', value: is3d ? '2d' : '3d' });
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function bindSlider(sliderId, valueId, onChange) {
    const slider = document.getElementById(sliderId);
    const display = document.getElementById(valueId);
    if (!slider) return;

    const update = () => {
        const v = slider.value;
        if (display) {
            const num = parseFloat(v);
            if (Number.isInteger(num)) {
                display.textContent = num;
            } else {
                display.textContent = num.toFixed(Math.max(1, (v.split('.')[1] || '').length));
            }
        }
        onChange(v);
    };

    slider.addEventListener('input', update);
}

function bindNumberInput(inputId, onChange) {
    const input = document.getElementById(inputId);
    if (!input) return;

    input.addEventListener('change', () => {
        onChange(input.value);
    });
}

function bindCheckbox(inputId, onChange) {
    const input = document.getElementById(inputId);
    if (!input) return;

    input.addEventListener('change', () => {
        onChange(input.checked);
    });
}

// ---------------------------------------------------------------------------
// Custom Map — Load JSON + localStorage handoff from Map Maker
// ---------------------------------------------------------------------------

function bindCustomMapImport() {
    const jsonInput = document.getElementById('input-load-json');
    const statusEl = document.getElementById('custom-map-status');
    const clearBtn = document.getElementById('btn-clear-scenario');

    if (jsonInput) {
        jsonInput.addEventListener('change', () => {
            const file = jsonInput.files[0];
            if (!file) return;
            const reader = new FileReader();
            reader.onload = () => {
                try {
                    const data = JSON.parse(reader.result);
                    if (!data.width || !data.height) {
                        if (statusEl) statusEl.innerHTML = '<div class="import-err">Invalid JSON: missing width/height</div>';
                        return;
                    }
                    sendCommand({ type: 'load_custom_map', ...data });
                    // Deactivate topology preset buttons
                    document.querySelectorAll('#topology-presets .preset-btn').forEach(b => b.classList.remove('active'));
                    if (statusEl) {
                        const robotCount = data.robots ? data.robots.length : 0;
                        statusEl.innerHTML = '<div class="import-ok">Loaded: ' +
                            data.width + '×' + data.height + ' · ' +
                            robotCount + ' robots</div>';
                    }
                } catch (e) {
                    if (statusEl) statusEl.innerHTML = '<div class="import-err">Failed to parse JSON</div>';
                }
                jsonInput.value = '';
            };
            reader.readAsText(file);
        });
    }

    if (clearBtn) {
        clearBtn.addEventListener('click', () => {
            sendCommand({ type: 'clear_scenario' });
            if (statusEl) statusEl.innerHTML = '';
            // Re-select first topology preset and load it
            const topoPresets = document.getElementById('topology-presets');
            if (topoPresets) {
                topoPresets.querySelectorAll('.preset-btn').forEach(b => b.classList.remove('active'));
                const firstBtn = topoPresets.querySelector('.preset-btn');
                if (firstBtn) firstBtn.classList.add('active');
            }
            if (loadedTopologies.length > 0) applyTopologyByIndex(0);
        });
    }

    // Check for map maker handoff via localStorage
    const params = new URLSearchParams(window.location.search);
    if (params.get('source') === 'custom') {
        const stored = localStorage.getItem('mapfis_custom_map');
        if (stored) {
            try {
                const data = JSON.parse(stored);
                // Validate expected schema before sending
                const w = typeof data.width === 'number' ? data.width : 0;
                const h = typeof data.height === 'number' ? data.height : 0;
                if (w < 8 || w > 512 || h < 8 || h > 512) throw new Error('Invalid grid dimensions');
                const safeData = {
                    type: 'load_custom_map',
                    width: w,
                    height: h,
                    cells: Array.isArray(data.cells) ? data.cells : [],
                    robots: Array.isArray(data.robots) ? data.robots : [],
                    name: typeof data.name === 'string' ? data.name.slice(0, 100) : 'custom',
                };
                setTimeout(() => {
                    sendCommand(safeData);
                    document.querySelectorAll('#topology-presets .preset-btn').forEach(b => b.classList.remove('active'));
                }, 200);
                if (statusEl) {
                    const robotCount = safeData.robots.length;
                    statusEl.textContent = 'Loaded from Map Maker: ' +
                        w + '\u00d7' + h + ' \u00b7 ' + robotCount + ' robots';
                }
            } catch (e) {
                console.error('Failed to load custom map from localStorage:', e);
            }
            localStorage.removeItem('mapfis_custom_map');
        }
        // Clean URL — but keep a flag so topology loading doesn't override
        window._customMapPending = true;
        window.history.replaceState({}, '', window.location.pathname);
    }
}

function updateCustomMapUI(s) {
    const clearBtn = document.getElementById('btn-clear-scenario');
    const customControls = document.getElementById('sim-customize-controls');
    const scenarioInfo = document.getElementById('sim-scenario-info');
    const customMapSection = document.getElementById('custom-map-section');
    const topoPresets = document.getElementById('topology-presets');
    const topoLabel = topoPresets ? topoPresets.previousElementSibling : null;
    const seedGroup = document.getElementById('seed-group');

    const isIdle = s.state === 'idle';
    const loaded = !!s.scenario_loaded;

    // A preset topology button is active — treat as preset, not custom map
    const hasActivePreset = topoPresets && topoPresets.querySelector('.preset-btn.active');
    const isCustomMap = loaded && !hasActivePreset;

    // Hide the entire topology section when sim is running/paused (any loaded map).
    // Show it again only when idle (after reset/clear).
    const isSimActive = !isIdle;
    const hideTopology = isCustomMap || isSimActive;

    // When a user-uploaded custom map is loaded, hide topology presets and customize controls
    if (loaded && customControls) customControls.style.display = 'none';
    if (topoPresets) {
        topoPresets.style.display = hideTopology ? 'none' : '';
    }
    if (topoLabel && topoLabel.classList.contains('preset-label')) {
        topoLabel.style.display = hideTopology ? 'none' : '';
    }
    const topoEmpty = document.getElementById('topology-empty');
    if (topoEmpty && isSimActive) topoEmpty.style.display = 'none';

    if (scenarioInfo) {
        // Show scenario info for custom maps, or as a summary when sim is active with a preset
        const showInfo = isCustomMap || (isSimActive && hasActivePreset);
        scenarioInfo.style.display = showInfo ? '' : 'none';
        if (showInfo) {
            const nameEl = document.getElementById('scenario-info-name');
            const dimsEl = document.getElementById('scenario-info-dims');
            const agentsEl = document.getElementById('scenario-info-agents');
            if (nameEl) nameEl.textContent = s.scenario_name || s.topology || 'Map';
            if (dimsEl) dimsEl.textContent = s.grid_width + ' × ' + s.grid_height;
            if (agentsEl) agentsEl.textContent = s.num_agents;
        }
    }

    if (clearBtn) {
        clearBtn.disabled = !isIdle;
        // Hide clear button entirely when sim is active (only show for idle custom maps)
        clearBtn.style.display = isSimActive ? 'none' : '';
    }
    if (customMapSection) customMapSection.style.display = (isCustomMap || isSimActive) ? 'none' : '';

    // Hide seed when sim is active (topology section is collapsed)
    if (seedGroup) seedGroup.style.display = isSimActive ? 'none' : '';

    // Disable json input when not idle
    const jsonLabel = document.getElementById('label-load-json');
    if (jsonLabel) jsonLabel.classList.toggle('disabled', !isIdle);

    // Disable create map link when not idle
    const createBtn = document.getElementById('btn-create-map');
    if (createBtn) {
        if (!isIdle) {
            createBtn.classList.add('disabled');
            createBtn.removeAttribute('href');
        } else {
            createBtn.classList.remove('disabled');
            createBtn.setAttribute('href', 'mapmaker.html');
        }
    }
}

function syncSliderDisplay(sliderId, valId, value) {
    const slider = document.getElementById(sliderId);
    const display = document.getElementById(valId);
    if (slider) slider.value = value;
    if (display) display.textContent = value;
}

// ---------------------------------------------------------------------------
// Experiment Mode — Full-Page View
// ---------------------------------------------------------------------------

// Experiment Web Workers — persistent warm pool, work-stealing dispatch
let workerPool = [];          // warm workers, kept alive between runs
let experimentReject = null;  // reject function to cancel a running experiment
let cachedWasmModule = null;  // compiled module cached across runs (compile once, instantiate N times)

// State
let experimentData = null;       // { summaries: [...], runs?: [...] }
let _phaseBeforeExp = 'configure'; // remember phase to return to when leaving experiment mode
let experimentSortCol = 'fault_tolerance';
let experimentSortAsc = false;
let experimentRunning = false;
let experimentCancelled = false;
let experimentDrilldownIdx = -1;
let experimentLastExpStage = 'config'; // remember last stage for toggling back
let experimentTopoData = {};  // topology_name → topo.data (preserved from experiment configs)

const EXPERIMENT_METRICS = [
    { key: 'fault_tolerance', label: 'FT', decimals: 2 },
    { key: 'throughput', label: 'TP', decimals: 2 },
    { key: 'nrr', label: 'NRR', decimals: 2 },
    { key: 'critical_time', label: 'CT', decimals: 2 },
    { key: 'survival_rate', label: 'Surv', decimals: 2 },
    { key: 'mttr', label: 'MTTR', decimals: 1 },
    { key: 'propagation_rate', label: 'Prop', decimals: 2 },
    { key: 'idle_ratio', label: 'Idle', decimals: 2 },
    { key: 'impacted_area', label: 'Impact', decimals: 2 },
    { key: 'total_tasks', label: 'Tasks', decimals: 0 },
    { key: 'deficit_integral', label: 'Deficit', decimals: 0 },
    { key: 'solver_step_us', label: '\u00b5s', decimals: 1 },
    { key: 'wall_time_ms', label: 'ms', decimals: 0 },
];

// ---------------------------------------------------------------------------
// Statistical summary computation (mirrors Rust compute_stat_summary exactly)
// ---------------------------------------------------------------------------

// Two-tailed t-critical values for 95% CI, df 1..30 (same table as stats.rs)
const T_CRITICAL_95 = [
    12.706, 4.303, 3.182, 2.776, 2.571,  // df 1-5
    2.447, 2.365, 2.306, 2.262, 2.228,   // df 6-10
    2.201, 2.179, 2.160, 2.145, 2.131,   // df 11-15
    2.120, 2.110, 2.101, 2.093, 2.086,   // df 16-20
    2.080, 2.074, 2.069, 2.064, 2.060,   // df 21-25
    2.056, 2.052, 2.048, 2.045, 2.042,   // df 26-30
];

function computeStatSummaryJS(values) {
    const v = values.filter(x => x !== null && x !== undefined && !isNaN(x));
    const n = v.length;
    if (n === 0) return { n: 0, mean: 0, std: 0, ci95_lo: 0, ci95_hi: 0, min: 0, max: 0 };
    const mean = v.reduce((a, b) => a + b, 0) / n;
    const min = Math.min(...v);
    const max = Math.max(...v);
    const std = n > 1 ? Math.sqrt(v.reduce((a, b) => a + (b - mean) ** 2, 0) / (n - 1)) : 0;
    let ci95_lo, ci95_hi;
    if (n > 1) {
        const df = n - 1;
        const t = df <= 30 ? T_CRITICAL_95[df - 1] : 1.96;
        const margin = t * std / Math.sqrt(n);
        ci95_lo = mean - margin;
        ci95_hi = mean + margin;
    } else {
        ci95_lo = mean;
        ci95_hi = mean;
    }
    return { n, mean, std, ci95_lo, ci95_hi, min, max };
}

// Metric mapping: run JSON key -> summary JSON key
const METRIC_MAP = [
    ['avg_throughput', 'throughput'],
    ['total_tasks', 'total_tasks'],
    ['idle_ratio', 'idle_ratio'],
    ['fault_tolerance', 'fault_tolerance'],
    ['nrr', 'nrr'],
    ['critical_time', 'critical_time'],
    ['deficit_recovery', 'deficit_recovery'],
    ['throughput_recovery', 'throughput_recovery'],
    ['propagation_rate', 'propagation_rate'],
    ['survival_rate', 'survival_rate'],
    ['impacted_area', 'impacted_area'],
    ['deficit_integral', 'deficit_integral'],
    // Note: 'mttr' (EXPERIMENT_METRICS key) has no run-level JSON field —
    // the experiment export schema (write_metrics_json) does not include MTTR.
    // Per-seed tab correctly shows N/A for MTTR (null from runKeyForMetric).
    ['solver_step_avg_us', 'solver_step_us'],
    ['wall_time_ms', 'wall_time_ms'],
];

function computeSummariesJS(runs) {
    // Group by config key (excluding seed) -- mirrors Rust BTreeMap grouping
    const groups = {};
    for (const run of runs) {
        const c = run.config;
        const key = `${c.solver}|${c.topology}|${c.scenario}|${c.scheduler}|${c.num_agents}`;
        if (!groups[key]) groups[key] = [];
        groups[key].push(run);
    }
    // Sort keys for deterministic order (BTreeMap equivalent)
    return Object.keys(groups).sort().map(key => {
        const group = groups[key];
        const first = group[0].config;
        const summary = {
            solver: first.solver,
            topology: first.topology,
            scenario: first.scenario,
            scheduler: first.scheduler,
            num_agents: first.num_agents,
            num_seeds: group.length,
        };
        for (const [runKey, summaryKey] of METRIC_MAP) {
            summary[summaryKey] = computeStatSummaryJS(group.map(r => r.faulted[runKey]));
        }
        return summary;
    });
}

function mergeWorkerResults(workerResults) {
    const allRuns = [];
    for (const result of workerResults) {
        if (!result?.json) continue;
        try {
            const parsed = JSON.parse(result.json);
            if (Array.isArray(parsed.runs)) allRuns.push(...parsed.runs);
        } catch { /* skip unparseable partial results */ }
    }
    return allRuns;
}

const EXPERIMENT_SCENARIOS = {
    none: null,
    burst_20: { type: 'burst', kill_percent: 20, at_tick: 100 },
    burst_50: { type: 'burst', kill_percent: 50, at_tick: 100 },
    wear_low: { type: 'wear', rate: 'low', threshold: 80 },
    wear_medium: { type: 'wear', rate: 'medium', threshold: 80 },
    wear_high: { type: 'wear', rate: 'high', threshold: 60 },
    zone: { type: 'zone', at_tick: 100, duration: 50 },
    intermittent: { type: 'intermittent', mtbf: 80, recovery: 15 },
};

// User-defined custom scenarios (added via experiment panel)
let customScenarioCounter = 0;
const customScenarios = {};

function initCustomScenarioBuilder() {
    const addBtn = document.getElementById('btn-add-custom-scenario');
    const form = document.getElementById('exp-custom-form');
    const typeEl = document.getElementById('exp-custom-type');
    const saveBtn = document.getElementById('btn-save-custom-scenario');
    const cancelBtn = document.getElementById('btn-cancel-custom-scenario');
    if (!addBtn || !form) return;

    addBtn.addEventListener('click', () => {
        form.style.display = '';
        showCustomParams(typeEl.value);
    });

    typeEl.addEventListener('change', () => showCustomParams(typeEl.value));

    cancelBtn.addEventListener('click', () => { form.style.display = 'none'; });

    saveBtn.addEventListener('click', () => {
        const type = typeEl.value;
        let scenario, label;

        if (type === 'burst') {
            const pct = parseInt(document.getElementById('exp-custom-burst-pct').value) || 30;
            const tick = parseInt(document.getElementById('exp-custom-burst-tick').value) || 100;
            scenario = { type: 'burst', kill_percent: pct, at_tick: tick };
            label = `burst_${pct}pct_t${tick}`;
        } else if (type === 'wear') {
            const beta = parseFloat(document.getElementById('exp-custom-wear-beta').value) || 2.5;
            const eta = parseInt(document.getElementById('exp-custom-wear-eta').value) || 500;
            const thresh = parseInt(document.getElementById('exp-custom-wear-thresh').value) || 80;
            scenario = { type: 'wear', rate: 'custom', beta, eta, threshold: thresh };
            label = `wear_b${beta}_e${eta}`;
        } else if (type === 'zone') {
            const tick = parseInt(document.getElementById('exp-custom-zone-tick').value) || 100;
            const dur = parseInt(document.getElementById('exp-custom-zone-dur').value) || 50;
            scenario = { type: 'zone', at_tick: tick, duration: dur };
            label = `zone_t${tick}_d${dur}`;
        } else if (type === 'intermittent') {
            const mtbf = parseInt(document.getElementById('exp-custom-int-mtbf').value) || 80;
            const recovery = parseInt(document.getElementById('exp-custom-int-recovery').value) || 15;
            scenario = { type: 'intermittent', mtbf, recovery };
            label = `int_m${mtbf}_r${recovery}`;
        }

        if (scenario && label) {
            const key = `custom_${++customScenarioCounter}`;
            customScenarios[key] = scenario;
            EXPERIMENT_SCENARIOS[key] = scenario;

            // Add checkbox to scenario list
            const grid = document.getElementById('exp-scenarios');
            const lbl = document.createElement('label');
            lbl.className = 'checkbox-label';
            lbl.innerHTML = `<input type="checkbox" value="${key}" checked><span>${label}</span>`;
            lbl.querySelector('input').addEventListener('change', updateExpRunCount);
            grid.appendChild(lbl);

            // Add tag to custom list
            const list = document.getElementById('exp-custom-list');
            const tag = document.createElement('span');
            tag.className = 'exp-custom-tag';
            tag.innerHTML = `${label} <span class="remove-tag" data-key="${key}">\u00d7</span>`;
            tag.querySelector('.remove-tag').addEventListener('click', () => {
                delete customScenarios[key];
                delete EXPERIMENT_SCENARIOS[key];
                tag.remove();
                const cb = grid.querySelector(`input[value="${key}"]`);
                if (cb) cb.closest('label').remove();
                updateExpRunCount();
            });
            list.appendChild(tag);

            form.style.display = 'none';
            updateExpRunCount();
        }
    });
}

function showCustomParams(type) {
    ['burst', 'wear', 'zone', 'intermittent'].forEach(t => {
        const el = document.getElementById(`exp-custom-params-${t}`);
        if (el) el.style.display = t === type ? '' : 'none';
    });
}

const EXPERIMENT_PRESETS = {
    smoke: {
        solvers: ['pibt'], topologies: ['warehouse-medium'], schedulers: ['random'],
        scenarios: ['burst_20'], agents: '8', seeds: '42, 123', ticks: 50,
    },
    solver: {
        solvers: ['pibt', 'rhcr_pibt', 'rhcr_priority_astar'], topologies: ['warehouse-medium'],
        schedulers: ['random'], scenarios: ['none', 'burst_20', 'burst_50', 'wear_medium', 'wear_high', 'zone'],
        agents: '40', seeds: '42, 123, 456, 789, 1024', ticks: 500,
    },
    scale: {
        solvers: ['pibt'], topologies: ['warehouse-medium'], schedulers: ['random'],
        scenarios: ['none', 'burst_20', 'burst_50', 'wear_medium', 'wear_high', 'zone'],
        agents: '10, 20, 40, 80', seeds: '42, 123, 456, 789, 1024', ticks: 500,
    },
    scheduler: {
        solvers: ['pibt'], topologies: ['warehouse-medium'], schedulers: ['random', 'closest'],
        scenarios: ['none', 'burst_20', 'burst_50', 'wear_medium', 'wear_high', 'zone'],
        agents: '40', seeds: '42, 123, 456, 789, 1024', ticks: 500,
    },
};

// --- Helpers ---

function metricZoneClass(key, val) {
    switch (key) {
        case 'fault_tolerance': case 'nrr': case 'survival_rate':
            return val >= 0.7 ? 'zone-good' : val >= 0.4 ? 'zone-fair' : 'zone-poor';
        case 'critical_time': case 'propagation_rate':
            return val <= 0.2 ? 'zone-good' : val <= 0.5 ? 'zone-fair' : 'zone-poor';
        case 'impacted_area':
            // impacted_area: negative = deficit (bad), positive = surplus/Braess (good), near zero = neutral
            return val >= 0 ? 'zone-good' : val >= -10 ? 'zone-fair' : 'zone-poor';
        case 'mttr':
            return val <= 20 ? 'zone-good' : val <= 60 ? 'zone-fair' : 'zone-poor';
        case 'idle_ratio':
            return val <= 0.3 ? 'zone-good' : val <= 0.6 ? 'zone-fair' : 'zone-poor';
        default:
            return 'zone-neutral';
    }
}

function metricZoneHex(key, val) {
    switch (key) {
        case 'fault_tolerance': case 'nrr': case 'survival_rate':
            return val >= 0.7 ? '#78b478' : val >= 0.4 ? '#c8aa64' : '#b45050';
        case 'critical_time': case 'propagation_rate':
            return val <= 0.2 ? '#78b478' : val <= 0.5 ? '#c8aa64' : '#b45050';
        case 'impacted_area':
            // negative = deficit (bad), positive = surplus/Braess (good)
            return val >= 0 ? '#78b478' : val >= -10 ? '#c8aa64' : '#b45050';
        case 'mttr':
            return val <= 20 ? '#78b478' : val <= 60 ? '#c8aa64' : '#b45050';
        default:
            return '#6688aa';
    }
}

function getStat(summary, key) {
    return summary[key] || { mean: 0, std: 0, ci95_lo: 0, ci95_hi: 0, min: 0, max: 0, n: 0 };
}

function getCheckedValues(containerId) {
    const el = document.getElementById(containerId);
    if (!el) return [];
    return [...el.querySelectorAll('input[type="checkbox"]:checked')].map(cb => cb.value);
}

function setCheckedValues(containerId, values) {
    const el = document.getElementById(containerId);
    if (!el) return;
    el.querySelectorAll('input[type="checkbox"]').forEach(cb => {
        cb.checked = values.includes(cb.value);
    });
}

function downloadBlob(content, filename, mimeType) {
    const blob = new Blob([content], { type: mimeType });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = filename;
    a.click();
    URL.revokeObjectURL(a.href);
}

// --- Stage management ---

function setExpStage(stage) {
    const view = document.getElementById('experiment-view');
    if (view) view.dataset.expStage = stage;
    experimentLastExpStage = stage;
}

// --- Config form ---

function buildExperimentConfigs() {
    const solvers = getCheckedValues('exp-solvers');
    const topoIds = getCheckedValues('exp-topologies');
    const schedulers = getCheckedValues('exp-schedulers');
    const scenarioKeys = getCheckedValues('exp-scenarios');
    const agentsStr = document.getElementById('exp-agents')?.value || '20';
    const seedsStr = document.getElementById('exp-seeds')?.value || '42';
    const tickCount = parseInt(document.getElementById('exp-ticks')?.value || '500', 10);

    const agents = agentsStr.split(',').map(s => parseInt(s.trim(), 10)).filter(n => n > 0);
    const seeds = seedsStr.split(',').map(s => parseInt(s.trim(), 10)).filter(n => !isNaN(n));

    if (!solvers.length || !topoIds.length || !schedulers.length || !scenarioKeys.length || !agents.length || !seeds.length) {
        return [];
    }

    // Resolve topology entries from the registry
    const allTopos = getAllExpTopologies();
    const configs = [];
    const skipped = [];

    // Iteration order: topology → solver → scenario → scheduler → agents → seed
    // Outermost = slowest to change (padlock left column),
    // innermost = fastest (padlock right column).
    for (const topoId of topoIds) {
        const topo = allTopos.find(t => t.id === topoId);
        if (!topo) continue;

        const cap = getTopologyCapacity(topoId);

        for (const solver of solvers) {
            for (const scenarioKey of scenarioKeys) {
                for (const scheduler of schedulers) {
                    for (const numAgents of agents) {
                        if (numAgents > cap) {
                            skipped.push(`${topo.name} cannot hold ${numAgents} agents (max ${cap})`);
                            continue;
                        }
                        for (const seed of seeds) {
                            const cfg = {
                                solver,
                                topology: topo.name,
                                scheduler,
                                scenario_key: scenarioKey,
                                scenario: EXPERIMENT_SCENARIOS[scenarioKey] || null,
                                num_agents: numAgents, seed, tick_count: tickCount,
                                // Always send inline map data — makes experiments
                                // independent of ActiveTopology::from_name()
                                custom_map: topo.data,
                            };
                            configs.push(cfg);
                        }
                    }
                }
            }
        }
    }
    if (skipped.length > 0) {
        const unique = [...new Set(skipped)];
        console.warn('Skipped impossible configs:', unique);
    }
    return configs;
}

function updateExpRunCount() {
    const solvers = getCheckedValues('exp-solvers');
    const topoIds = getCheckedValues('exp-topologies');
    const schedulers = getCheckedValues('exp-schedulers');
    const scenarios = getCheckedValues('exp-scenarios');
    const agentsStr = document.getElementById('exp-agents')?.value || '';
    const seedsStr = document.getElementById('exp-seeds')?.value || '';
    const agents = agentsStr.split(',').map(s => parseInt(s.trim(), 10)).filter(n => n > 0);
    const seeds = seedsStr.split(',').map(s => parseInt(s.trim(), 10)).filter(n => !isNaN(n));

    // Count configs after filtering impossible topology/agent combinations
    const configs = buildExperimentConfigs();
    const cartesian = solvers.length * topoIds.length * schedulers.length * scenarios.length * agents.length * seeds.length;
    const skippedCount = cartesian - configs.length;

    const display = document.getElementById('exp-run-count-display');
    if (display) {
        let text = configs.length > 0 ? `${configs.length} runs` : '0 runs';
        if (skippedCount > 0) text += ` (${skippedCount} skipped: over capacity)`;
        display.textContent = text;
    }

    // Matrix breakdown
    const summary = document.getElementById('exp-matrix-summary');
    if (summary) {
        summary.innerHTML = [
            `${solvers.length} solver${solvers.length !== 1 ? 's' : ''}`,
            `${topoIds.length} topolog${topoIds.length !== 1 ? 'ies' : 'y'}`,
            `${schedulers.length} scheduler${schedulers.length !== 1 ? 's' : ''}`,
            `${scenarios.length} scenario${scenarios.length !== 1 ? 's' : ''}`,
            `${agents.length} agent count${agents.length !== 1 ? 's' : ''}`,
            `${seeds.length} seed${seeds.length !== 1 ? 's' : ''}`,
        ].join(' &times; ');
    }
}

// --- Init ---

function initExperimentMode() {
    const expView = document.getElementById('experiment-view');
    if (!expView) return;

    // Default stage
    setExpStage('config');

    // EXP header button toggle — allow switching freely at any time
    const expBtn = document.getElementById('btn-exp-mode');
    if (expBtn) {
        expBtn.addEventListener('click', () => {
            if (currentPhase === 'experiment') {
                // Return to whatever phase the sim is in
                setPhase(_phaseBeforeExp);
            } else {
                _phaseBeforeExp = currentPhase;
                setPhase('experiment');
                // If an experiment is actively running, always show the running stage
                // (don't be fooled by stale experimentData from a previous experiment)
                if (experimentRunning) {
                    setExpStage('running');
                } else {
                    setExpStage(experimentData?.summaries?.length ? 'results' : experimentLastExpStage);
                }
            }
        });
    }

    // Presets
    document.querySelectorAll('[data-exp-preset]').forEach(btn => {
        btn.addEventListener('click', () => {
            const key = btn.dataset.expPreset;
            const preset = EXPERIMENT_PRESETS[key];
            if (!preset) return;
            // Highlight active preset
            document.querySelectorAll('.exp-preset-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            setCheckedValues('exp-solvers', preset.solvers);
            setCheckedValues('exp-topologies', preset.topologies);
            setCheckedValues('exp-schedulers', preset.schedulers);
            setCheckedValues('exp-scenarios', preset.scenarios);
            const agentsEl = document.getElementById('exp-agents');
            if (agentsEl) agentsEl.value = preset.agents;
            const seedsEl = document.getElementById('exp-seeds');
            if (seedsEl) seedsEl.value = preset.seeds;
            const ticksEl = document.getElementById('exp-ticks');
            if (ticksEl) ticksEl.value = preset.ticks;
            updateExpRunCount();
        });
    });

    // Init map import for experiments
    initExpMapImport();

    // Init custom scenario builder
    initCustomScenarioBuilder();

    // Update run count on any config change
    // Use event delegation for #exp-topologies since checkboxes are dynamic
    document.querySelectorAll('#exp-solvers input, #exp-schedulers input, #exp-scenarios input').forEach(cb => {
        cb.addEventListener('change', () => {
            document.querySelectorAll('.exp-preset-btn').forEach(b => b.classList.remove('active'));
            updateExpRunCount();
        });
    });
    document.getElementById('exp-topologies')?.addEventListener('change', () => {
        document.querySelectorAll('.exp-preset-btn').forEach(b => b.classList.remove('active'));
        updateExpRunCount();
    });
    ['exp-agents', 'exp-seeds', 'exp-ticks'].forEach(id => {
        document.getElementById(id)?.addEventListener('input', () => {
            document.querySelectorAll('.exp-preset-btn').forEach(b => b.classList.remove('active'));
            updateExpRunCount();
        });
    });

    // Run button
    document.getElementById('btn-exp-run')?.addEventListener('click', () => {
        if (!experimentRunning) runExpAsync();
    });

    // Cancel button — stops dispatching new work; in-flight runOne finishes then workers go idle
    document.getElementById('btn-exp-cancel')?.addEventListener('click', () => {
        experimentCancelled = true;
        if (experimentReject) {
            experimentReject(new Error('Cancelled'));
            experimentReject = null;
        }
    });

    // Import JSON
    const fileInput = document.getElementById('input-import-experiment');
    if (fileInput) {
        fileInput.addEventListener('change', e => {
            const file = e.target.files[0];
            if (!file) return;
            const reader = new FileReader();
            reader.onload = ev => {
                try {
                    const json = JSON.parse(ev.target.result);
                    experimentData = json;

                    // Populate experimentTopoData from loaded topologies for simulateIn3D
                    experimentTopoData = {};
                    const allTopos = getAllExpTopologies();
                    if (json.summaries) {
                        for (const s of json.summaries) {
                            if (s.topology && !experimentTopoData[s.topology]) {
                                const match = allTopos.find(t => t.name === s.topology || t.id === s.topology);
                                if (match) experimentTopoData[s.topology] = match.data;
                            }
                        }
                    }

                    setExpStage('results');
                    renderExpResults();
                } catch (err) {
                    const status = document.getElementById('exp-status');
                    if (status) status.textContent = `Import error: ${err.message}`;
                }
            };
            reader.readAsText(file);
            fileInput.value = '';
        });
    }

    // Chart metric selector
    document.getElementById('exp-chart-metric')?.addEventListener('change', () => {
        renderExpChart();
    });

    // Export buttons
    document.getElementById('btn-exp-csv')?.addEventListener('click', () => exportExpCSV());
    document.getElementById('btn-exp-json')?.addEventListener('click', () => exportExpJSON());
    document.getElementById('btn-exp-latex')?.addEventListener('click', () => exportExpLatex());
    document.getElementById('btn-exp-typst')?.addEventListener('click', () => exportExpTypst());

    // New experiment button — cancel any running experiment first
    document.getElementById('btn-exp-new')?.addEventListener('click', () => {
        if (experimentRunning) {
            experimentCancelled = true;
            if (experimentReject) {
                experimentReject(new Error('Cancelled'));
                experimentReject = null;
            }
        }
        setExpStage('config');
    });

    // Drill-down close
    document.getElementById('btn-exp-drilldown-close')?.addEventListener('click', () => {
        document.getElementById('exp-drilldown').style.display = 'none';
        experimentDrilldownIdx = -1;
    });

    // Simulate in Observatory button
    document.getElementById('btn-exp-sim3d')?.addEventListener('click', () => {
        if (experimentDrilldownIdx >= 0 && experimentData?.summaries) {
            simulateIn3D(experimentDrilldownIdx);
        }
    });

    updateExpRunCount();

    // URL param: ?results=URL (validated: same-origin or relative only)
    const expParams = new URLSearchParams(window.location.search);
    const resultsUrl = expParams.get('results');
    if (resultsUrl) {
        let isSafeOrigin = false;
        try {
            const parsed = new URL(resultsUrl, window.location.origin);
            isSafeOrigin = parsed.origin === window.location.origin;
        } catch (_) {
            isSafeOrigin = !resultsUrl.includes('//');
        }
        if (isSafeOrigin) {
            fetch(resultsUrl)
                .then(r => r.json())
                .then(json => {
                    experimentData = json;
                    setPhase('experiment');
                    setExpStage('results');
                    renderExpResults();
                })
                .catch(err => console.error('Failed to load results:', err));
        } else {
            console.warn('[MAFIS] Blocked cross-origin results URL:', resultsUrl);
        }
    }
}

// ---------------------------------------------------------------------------
// Padlock matrix display
// ---------------------------------------------------------------------------

const PADLOCK_KEYS = ['topology', 'solver', 'scenario_key', 'scheduler', 'num_agents', 'seed'];
const PADLOCK_HEADERS = ['TOPOLOGY', 'ALGORITHM', 'SCENARIO', 'SCHEDULER', 'AGENTS', 'SEED'];
const PADLOCK_CELL_H = 30;
const PADLOCK_VISIBLE = 5;

let padlockState = null;

function padlockFormatValue(key, value) {
    const v = String(value);
    if (key === 'solver') {
        const map = { pibt: 'PIBT', rhcr_pibt: 'RHCR-PIBT', rhcr_priority_astar: 'RHCR-A*',
            token_passing: 'Token Pass', rhcr_pbs: 'RHCR-PBS' };
        return map[v] || v;
    }
    if (key === 'scenario_key') {
        const map = { none: 'none', burst_20: 'burst 20%', burst_50: 'burst 50%',
            wear_medium: 'wear med', wear_high: 'wear high', zone: 'zone' };
        return map[v] || v;
    }
    return v;
}

function padlockInit(configs) {
    const headersEl = document.getElementById('padlock-headers');
    headersEl.innerHTML = '';
    for (const label of PADLOCK_HEADERS) {
        const h = document.createElement('div');
        h.className = 'padlock-header';
        h.textContent = label;
        headersEl.appendChild(h);
    }

    const viewport = document.getElementById('padlock-viewport');
    viewport.innerHTML = '';

    const columns = [];
    for (let c = 0; c < PADLOCK_KEYS.length; c++) {
        const key = PADLOCK_KEYS[c];
        const values = configs.map(cfg => padlockFormatValue(key, cfg[key]));

        const colEl = document.createElement('div');
        colEl.className = 'padlock-col';

        const drum = document.createElement('div');
        drum.className = 'padlock-drum';

        // 6 cells: 5 visible + 1 buffer for roll animation
        const cells = [];
        for (let i = 0; i < PADLOCK_VISIBLE + 1; i++) {
            const cell = document.createElement('div');
            cell.className = 'padlock-cell';
            drum.appendChild(cell);
            cells.push(cell);
        }

        colEl.appendChild(drum);
        viewport.appendChild(colEl);
        columns.push({ drum, cells, values });
    }

    padlockState = { columns, currentIndex: -1, total: configs.length, pendingSnap: null };
    padlockSetIndex(0);
}

function padlockSetIndex(index) {
    if (!padlockState) return;
    const { columns, total } = padlockState;
    const half = Math.floor(PADLOCK_VISIBLE / 2);

    for (const col of columns) {
        for (let s = 0; s < PADLOCK_VISIBLE + 1; s++) {
            const cfgIdx = index - half + s;
            col.cells[s].textContent = (cfgIdx >= 0 && cfgIdx < total) ? col.values[cfgIdx] : '';
            col.cells[s].dataset.slot = String(s - half);
        }
        col.drum.classList.remove('rolling');
        col.drum.style.transform = 'translateY(0)';
    }

    padlockState.currentIndex = index;
}

function padlockAdvance() {
    if (!padlockState) return;

    // Finalize any pending animation from the previous advance
    if (padlockState.pendingSnap !== null) {
        padlockSetIndex(padlockState.pendingSnap);
        padlockState.pendingSnap = null;
    }

    const { columns, currentIndex, total } = padlockState;
    const nextIndex = currentIndex + 1;
    if (nextIndex >= total) return;

    // Force reflow so the browser commits the snap before starting the new animation
    void document.getElementById('padlock-viewport').offsetHeight;

    const half = Math.floor(PADLOCK_VISIBLE / 2);
    for (const col of columns) {
        const oldVal = (currentIndex >= 0 && currentIndex < total) ? col.values[currentIndex] : '';
        const newVal = (nextIndex >= 0 && nextIndex < total) ? col.values[nextIndex] : '';

        if (oldVal !== newVal) {
            // Set buffer cell to the value entering from below
            const bufIdx = nextIndex + half;
            col.cells[PADLOCK_VISIBLE].textContent = (bufIdx >= 0 && bufIdx < total) ? col.values[bufIdx] : '';

            // Animate: slide drum up by one cell height
            col.drum.classList.add('rolling');
            col.drum.style.transform = `translateY(-${PADLOCK_CELL_H}px)`;
        }
    }

    padlockState.pendingSnap = nextIndex;
}

function padlockFinalize() {
    if (padlockState?.pendingSnap !== null) {
        padlockSetIndex(padlockState.pendingSnap);
        padlockState.pendingSnap = null;
    }
}

// ---------------------------------------------------------------------------
// Warm worker pool management
// ---------------------------------------------------------------------------

// Initialize the pool once. Compiles WASM (if not cached) then spawns and
// inits one worker per logical CPU. Subsequent calls are instant no-ops.
async function ensureWorkerPool() {
    if (workerPool.length > 0) return;

    if (!cachedWasmModule) {
        const wasmUrl = new URL('mafis_bg.wasm', window.location.href).href;
        cachedWasmModule = await WebAssembly.compileStreaming(fetch(wasmUrl));
    }

    const size = navigator.hardwareConcurrency || 4;
    const workers = Array.from({ length: size }, () => new Worker('experiment-worker.js'));
    await Promise.all(workers.map(worker => new Promise((resolve, reject) => {
        worker.onerror = (ev) => { ev.preventDefault(); reject(new Error(ev.message || 'Worker init failed')); };
        worker.onmessage = (e) => {
            if (e.data.type === 'ready') resolve();
            else if (e.data.type === 'error') reject(new Error(e.data.message));
        };
        worker.postMessage({ type: 'init', module: cachedWasmModule });
    })));
    workerPool = workers;
}

// Terminate pool on page close to free OS threads
window.addEventListener('beforeunload', () => {
    for (const w of workerPool) w.terminate();
    workerPool = [];
});

// ---------------------------------------------------------------------------
// Async experiment runner
// ---------------------------------------------------------------------------

async function runExpAsync() {
    const configs = buildExperimentConfigs();
    if (configs.length === 0) {
        const status = document.getElementById('exp-status');
        if (status) status.textContent = 'No valid configurations';
        return;
    }

    // Preserve topology data for simulateIn3D (results JSON only stores the name)
    experimentTopoData = {};
    for (const cfg of configs) {
        if (cfg.custom_map && cfg.topology) {
            experimentTopoData[cfg.topology] = cfg.custom_map;
        }
    }

    experimentRunning = true;
    experimentCancelled = false;
    experimentData = null;
    setExpStage('running');
    const _expBtnRef = document.getElementById('btn-exp-mode');
    if (_expBtnRef) _expBtnRef.classList.add('exp-running');

    const total = configs.length;
    const startTime = performance.now();

    const fractionEl = document.getElementById('exp-progress-fraction');
    const pctEl = document.getElementById('exp-progress-pct');
    const fillEl = document.getElementById('exp-progress-fill');
    const timeEl = document.getElementById('exp-progress-time');

    padlockInit(configs);

    // Ensure worker pool is warm (no-op on second+ run — skips spawn + WASM init entirely)
    if (workerPool.length === 0) {
        if (fractionEl) fractionEl.textContent = cachedWasmModule ? 'Initializing workers...' : 'Compiling WASM...';
        await ensureWorkerPool();
    }

    const numWorkers = Math.min(workerPool.length, total);

    // Work-stealing queue cursor — each worker pulls the next available config index.
    // Unlike round-robin partitioning, fast workers keep running instead of sitting idle.
    let queueIdx = 0;
    let completedCount = 0;
    const allRuns = []; // populated incrementally so partial salvage works on cancel/error

    function updateProgressUI() {
        const pct = ((completedCount / total) * 100).toFixed(0);
        if (fractionEl) fractionEl.textContent = `${completedCount} / ${total}`;
        if (pctEl) pctEl.textContent = `${pct}%`;
        if (fillEl) fillEl.style.width = pct + '%';
        if (timeEl) {
            const elapsed = ((performance.now() - startTime) / 1000).toFixed(1);
            const eta = completedCount > 0
                ? (((performance.now() - startTime) / completedCount) * (total - completedCount) / 1000).toFixed(0)
                : '--';
            timeEl.textContent = `${elapsed}s elapsed \u2014 ~${eta}s remaining`;
        }
    }

    try {
        // Cancel token — rejecting this races against worker promises
        let cancelReject;
        const cancelPromise = new Promise((_, rej) => { cancelReject = rej; });
        experimentReject = cancelReject;

        if (fractionEl) fractionEl.textContent = `0 / ${total}`;

        // Work-stealing dispatch: each worker calls sendNext() when it finishes a job.
        // Workers are from the warm pool — no spawn or WASM init overhead here.
        await Promise.race([
            Promise.allSettled(workerPool.slice(0, numWorkers).map(worker => new Promise((resolve, reject) => {
                function sendNext() {
                    if (experimentCancelled || queueIdx >= total) { resolve(); return; }
                    const idx = queueIdx++;
                    worker.postMessage({ type: 'runOne', config: configs[idx], index: idx });
                }

                worker.onerror = (ev) => { ev.preventDefault(); reject(new Error(ev.message || 'Worker crashed')); };
                worker.onmessage = (e) => {
                    const msg = e.data;
                    if (msg.type === 'runOneDone') {
                        completedCount++;
                        updateProgressUI();
                        if (completedCount < total) padlockAdvance();
                        try {
                            const parsed = JSON.parse(msg.json);
                            if (Array.isArray(parsed.runs)) allRuns.push(...parsed.runs);
                        } catch {}
                        sendNext(); // pull next config from queue
                    } else if (msg.type === 'error') {
                        reject(new Error(msg.message));
                    }
                };

                sendNext(); // kick off first job for this worker
            }))),
            cancelPromise,
        ]);

        padlockFinalize();
        const elapsed = ((performance.now() - startTime) / 1000).toFixed(1);

        if (allRuns.length > 0) {
            experimentData = {
                total_runs: allRuns.length,
                wall_time_total_ms: Math.round(performance.now() - startTime),
                runs: allRuns,
                summaries: computeSummariesJS(allRuns),
            };
        }

        const threadStr = numWorkers > 1 ? ` (${numWorkers} threads)` : '';
        if (allRuns.length < total) {
            if (fractionEl) fractionEl.textContent = `${allRuns.length} / ${total}`;
            if (pctEl) pctEl.textContent = '';
            if (experimentData) { setExpStage('results'); renderExpResults(); }
            const status = document.getElementById('exp-status');
            if (status) status.textContent = `Partial: ${allRuns.length}/${total} runs in ${elapsed}s${threadStr}`;
        } else {
            if (fractionEl) fractionEl.textContent = `${total} / ${total}`;
            if (pctEl) pctEl.textContent = '100%';
            if (fillEl) fillEl.style.width = '100%';
            setExpStage('results');
            renderExpResults();
            const status = document.getElementById('exp-status');
            if (status) status.textContent = `Done: ${total} runs in ${elapsed}s${threadStr}`;
        }
    } catch (err) {
        padlockFinalize();

        // Salvage partial results collected so far (allRuns is populated incrementally)
        if (allRuns.length > 0) {
            experimentData = {
                total_runs: allRuns.length,
                wall_time_total_ms: Math.round(performance.now() - startTime),
                runs: allRuns,
                summaries: computeSummariesJS(allRuns),
            };
        }

        const status = document.getElementById('exp-status');
        if (experimentCancelled) {
            const elapsed = ((performance.now() - startTime) / 1000).toFixed(1);
            if (status) status.textContent = `Cancelled after ${elapsed}s`;
            if (fractionEl) fractionEl.textContent = 'Cancelled';
            if (pctEl) pctEl.textContent = '';
            if (experimentData) { setExpStage('results'); renderExpResults(); }
            else setExpStage('config');
        } else {
            if (status) status.textContent = `Error: ${err.message}`;
            setExpStage('config');
        }
    } finally {
        // Null out handlers on all pool workers. Workers stay alive (warm pool) but
        // stale runOneDone messages from a cancelled/errored run must not mix into
        // the next run's message stream.
        for (const w of workerPool) {
            w.onmessage = null;
            w.onerror = null;
        }
        experimentReject = null;
        experimentRunning = false;
        experimentCancelled = false;
        const _expBtnDone = document.getElementById('btn-exp-mode');
        if (_expBtnDone) _expBtnDone.classList.remove('exp-running');
    }
}

// ---------------------------------------------------------------------------
// Results rendering
// ---------------------------------------------------------------------------

function renderExpResults() {
    if (!experimentData || !Array.isArray(experimentData.summaries) || experimentData.summaries.length === 0) return;
    const summ = document.getElementById('exp-results-summary');
    if (summ) {
        const seeds = experimentData.summaries[0]?.num_seeds || '?';
        summ.textContent = `${experimentData.summaries.length} configurations \u2014 ${seeds} seeds each`;
    }
    renderExpTable();
    renderExpChart();
}

function getSortedIndices() {
    const summaries = experimentData.summaries;
    const indices = summaries.map((_, i) => i);
    const col = experimentSortCol;
    const asc = experimentSortAsc;

    indices.sort((a, b) => {
        let va, vb;
        if (['solver', 'topology', 'scenario', 'scheduler'].includes(col)) {
            va = summaries[a][col] || '';
            vb = summaries[b][col] || '';
            const cmp = va.localeCompare(vb);
            return asc ? cmp : -cmp;
        } else if (col === 'num_agents') {
            va = summaries[a].num_agents || 0;
            vb = summaries[b].num_agents || 0;
        } else {
            va = getStat(summaries[a], col).mean;
            vb = getStat(summaries[b], col).mean;
        }
        return asc ? va - vb : vb - va;
    });
    return indices;
}

function renderExpTable() {
    const thead = document.getElementById('exp-thead');
    const tbody = document.getElementById('exp-tbody');
    if (!thead || !tbody || !experimentData?.summaries) return;

    const summaries = experimentData.summaries;
    const sortedIndices = getSortedIndices();

    const configCols = [
        { key: 'solver', label: 'Solver' },
        { key: 'topology', label: 'Topology' },
        { key: 'scenario', label: 'Scenario' },
        { key: 'scheduler', label: 'Scheduler' },
        { key: 'num_agents', label: 'N' },
    ];

    let headerHtml = '<tr>';
    for (const c of configCols) {
        const arrow = experimentSortCol === c.key
            ? (experimentSortAsc ? ' \u25B2' : ' \u25BC') : '';
        headerHtml += `<th data-sort="${c.key}">${c.label}${arrow}</th>`;
    }
    for (const m of EXPERIMENT_METRICS) {
        const arrow = experimentSortCol === m.key
            ? (experimentSortAsc ? ' \u25B2' : ' \u25BC') : '';
        headerHtml += `<th data-sort="${m.key}">${m.label}${arrow}</th>`;
    }
    headerHtml += '</tr>';
    thead.innerHTML = headerHtml;

    // Bind sort clicks
    thead.querySelectorAll('th[data-sort]').forEach(th => {
        th.addEventListener('click', () => {
            const col = th.dataset.sort;
            if (experimentSortCol === col) {
                experimentSortAsc = !experimentSortAsc;
            } else {
                experimentSortCol = col;
                experimentSortAsc = ['solver', 'topology', 'scenario', 'scheduler'].includes(col);
            }
            renderExpTable();
            renderExpChart();
        });
    });

    // Body
    let bodyHtml = '';
    for (const idx of sortedIndices) {
        const s = summaries[idx];
        bodyHtml += `<tr data-exp-idx="${idx}">`;
        bodyHtml += `<td>${s.solver || ''}</td>`;
        bodyHtml += `<td>${s.topology || ''}</td>`;
        bodyHtml += `<td>${s.scenario || ''}</td>`;
        bodyHtml += `<td>${s.scheduler || ''}</td>`;
        bodyHtml += `<td>${s.num_agents || 0}</td>`;

        for (const m of EXPERIMENT_METRICS) {
            const stat = getStat(s, m.key);
            if (m.key === 'nrr' && stat.n === 0) {
                bodyHtml += `<td class="zone-neutral" title="NRR: N/A — requires ≥2 fault events per seed">—</td>`;
            } else {
                const cls = metricZoneClass(m.key, stat.mean);
                const val = stat.mean.toFixed(m.decimals);
                const tip = `${m.label}: ${stat.mean.toFixed(m.decimals)} \u00b1 ${stat.std.toFixed(m.decimals)}\n` +
                    `95% CI: [${stat.ci95_lo.toFixed(m.decimals)}, ${stat.ci95_hi.toFixed(m.decimals)}]\n` +
                    `Range: [${stat.min.toFixed(m.decimals)}, ${stat.max.toFixed(m.decimals)}]\nn = ${stat.n}`;
                bodyHtml += `<td class="${cls}" title="${tip}">${val}</td>`;
            }
        }
        bodyHtml += '</tr>';
    }
    tbody.innerHTML = bodyHtml;

    // Bind row click → drill-down
    tbody.querySelectorAll('tr').forEach(tr => {
        tr.addEventListener('click', (e) => {
            const idx = parseInt(tr.dataset.expIdx, 10);
            if (!isNaN(idx)) showExpDrilldown(idx);
        });
    });
}

function renderExpChart() {
    const container = document.getElementById('exp-chart');
    const metricSelect = document.getElementById('exp-chart-metric');
    if (!container || !experimentData?.summaries?.length) return;

    const metricKey = metricSelect?.value || 'fault_tolerance';
    const metricDef = EXPERIMENT_METRICS.find(m => m.key === metricKey) || EXPERIMENT_METRICS[0];
    const summaries = experimentData.summaries;
    const sortedIndices = getSortedIndices();

    const barH = 18;
    const gap = 2;
    const labelW = 100;
    const chartW = 150;
    const valueW = 42;
    const totalW = labelW + chartW + valueW;
    const totalH = (barH + gap) * sortedIndices.length + 20;

    let maxVal = 0;
    for (const idx of sortedIndices) {
        const v = getStat(summaries[idx], metricKey).mean;
        if (v > maxVal) maxVal = v;
    }
    if (maxVal < 1e-9) maxVal = 1;

    // NRR N/A note: if metric is NRR and all summaries have n===0, show explanation text
    const allNrrNA = metricKey === 'nrr' && sortedIndices.every(idx => getStat(summaries[idx], 'nrr').n === 0);

    let svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${totalW} ${totalH}" ` +
        `font-family="'DM Mono', monospace" font-size="8">`;

    svg += `<text x="${labelW}" y="12" font-size="9" font-weight="500" fill="currentColor" ` +
        `letter-spacing="0.5">${metricDef.label.toUpperCase()}</text>`;

    const yStart = 18;
    for (let row = 0; row < sortedIndices.length; row++) {
        const idx = sortedIndices[row];
        const s = summaries[idx];
        const stat = getStat(s, metricKey);
        const y = yStart + row * (barH + gap);
        const w = (stat.mean / maxVal) * chartW;
        const color = metricZoneHex(metricKey, stat.mean);

        const label = `${s.scenario || 'none'}/${s.num_agents || 0}`;
        const truncLabel = label.length > 18 ? label.slice(0, 16) + '\u2026' : label;
        svg += `<text x="4" y="${y + barH / 2}" text-anchor="start" ` +
            `dominant-baseline="middle" fill="currentColor">${truncLabel}</text>`;

        svg += `<rect class="exp-chart-bar" x="${labelW}" y="${y}" ` +
            `width="${w.toFixed(1)}" height="${barH}" fill="${color}"/>`;

        const ciLoX = labelW + (stat.ci95_lo / maxVal) * chartW;
        const ciHiX = labelW + (stat.ci95_hi / maxVal) * chartW;
        const midY = y + barH / 2;
        svg += `<line x1="${ciLoX.toFixed(1)}" y1="${midY}" x2="${ciHiX.toFixed(1)}" y2="${midY}" ` +
            `stroke="currentColor" stroke-width="1" opacity="0.5"/>`;

        svg += `<text x="${labelW + chartW + 4}" y="${y + barH / 2}" ` +
            `dominant-baseline="middle" fill="currentColor">${stat.mean.toFixed(metricDef.decimals)}</text>`;
    }

    svg += '</svg>';
    if (allNrrNA) {
        // Note: using a <p> below the SVG (not SVG <text>) for easier CSS styling.
        // This is a deliberate deviation from the spec which said "SVG text note."
        container.innerHTML = svg +
            '<p class="exp-chart-na-note">NRR not applicable — requires ≥2 fault events per run.</p>';
    } else {
        container.innerHTML = svg;
    }
}

/** Map a summary metric key back to its run-level faulted key via METRIC_MAP.
 *  Returns null if no mapping found — callers must treat null as N/A. */
function runKeyForMetric(summaryKey) {
    const entry = METRIC_MAP.find(([, sk]) => sk === summaryKey);
    return entry ? entry[0] : null;
}

// ---------------------------------------------------------------------------
// Drill-down
// ---------------------------------------------------------------------------

function showExpDrilldown(idx) {
    const card = document.getElementById('exp-drilldown');
    const titleEl = document.getElementById('exp-drilldown-title');
    const bodyEl = document.getElementById('exp-drilldown-body');
    if (!card || !titleEl || !bodyEl || !experimentData?.summaries) return;

    const s = experimentData.summaries[idx];
    if (!s) return;

    experimentDrilldownIdx = idx;

    // Highlight selected row
    document.querySelectorAll('#exp-tbody tr').forEach(tr => {
        tr.classList.toggle('selected-row', parseInt(tr.dataset.expIdx, 10) === idx);
    });

    titleEl.textContent = `${s.solver} / ${s.topology} / ${s.scenario || 'none'} / ${s.scheduler} / ${s.num_agents}a`;

    // Find matching runs for per-seed tab
    const seedRuns = experimentData.runs ? experimentData.runs.filter(r =>
        r.config.solver === s.solver &&
        r.config.topology === s.topology &&
        r.config.scenario === s.scenario &&
        r.config.scheduler === s.scheduler &&
        r.config.num_agents === s.num_agents
    ) : [];

    // Build tab bar: MEAN + one tab per seed
    let tabsHtml = '<div class="drilldown-tabs">';
    tabsHtml += '<button class="drilldown-tab active" data-tab="mean">MEAN</button>';
    for (const r of seedRuns) {
        tabsHtml += `<button class="drilldown-tab" data-tab="seed-${r.config.seed}">${r.config.seed}</button>`;
    }
    tabsHtml += '</div>';

    // Build MEAN table
    let meanHtml = '<div class="drilldown-tab-panel" data-panel="mean">';
    meanHtml += '<table><thead><tr><th>Metric</th><th>Mean</th><th>Std</th><th>CI 95%</th><th>Min</th><th>Max</th></tr></thead><tbody>';
    for (const m of EXPERIMENT_METRICS) {
        const stat = getStat(s, m.key);
        if (m.key === 'nrr' && stat.n === 0) {
            // Intentional: spec allows "a small inline note" — colspan merges data columns
            // with the N/A explanation, avoiding separate sub-row visual noise.
            meanHtml += `<tr>`;
            meanHtml += `<td>${m.label}</td>`;
            meanHtml += `<td colspan="5" style="color:var(--text-muted);font-style:italic;">N/A — requires ≥2 fault events</td>`;
            meanHtml += '</tr>';
        } else {
            const cls = metricZoneClass(m.key, stat.mean);
            meanHtml += `<tr>`;
            meanHtml += `<td>${m.label}</td>`;
            meanHtml += `<td class="${cls}">${stat.mean.toFixed(m.decimals)}</td>`;
            meanHtml += `<td>${stat.std.toFixed(m.decimals)}</td>`;
            meanHtml += `<td>[${stat.ci95_lo.toFixed(m.decimals)}, ${stat.ci95_hi.toFixed(m.decimals)}]</td>`;
            meanHtml += `<td>${stat.min.toFixed(m.decimals)}</td>`;
            meanHtml += `<td>${stat.max.toFixed(m.decimals)}</td>`;
            meanHtml += '</tr>';
        }
    }
    meanHtml += '</tbody></table>';
    meanHtml += '</div>';

    // Build per-seed panels
    let seedPanelsHtml = '';
    for (const r of seedRuns) {
        seedPanelsHtml += `<div class="drilldown-tab-panel" data-panel="seed-${r.config.seed}" style="display:none">`;
        seedPanelsHtml += '<table><thead><tr><th>Metric</th><th>Value</th></tr></thead><tbody>';
        for (const m of EXPERIMENT_METRICS) {
            const runKey = runKeyForMetric(m.key);
            // runKey is null when no METRIC_MAP entry exists — treat as N/A
            const rawVal = runKey != null ? r.faulted[runKey] : null;
            let display;
            if (rawVal == null) {
                display = '<span style="color:var(--text-muted);font-style:italic;">N/A</span>';
            } else {
                const cls = metricZoneClass(m.key, rawVal);
                display = `<span class="${cls}">${typeof rawVal === 'number' ? rawVal.toFixed(m.decimals) : rawVal}</span>`;
            }
            seedPanelsHtml += `<tr><td>${m.label}</td><td>${display}</td></tr>`;
        }
        seedPanelsHtml += '</tbody></table>';
        seedPanelsHtml += '</div>';
    }

    bodyEl.innerHTML = tabsHtml + meanHtml + seedPanelsHtml;

    // Wire tab clicks
    bodyEl.querySelectorAll('.drilldown-tab').forEach(btn => {
        btn.addEventListener('click', () => {
            const tab = btn.dataset.tab;
            bodyEl.querySelectorAll('.drilldown-tab').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            bodyEl.querySelectorAll('.drilldown-tab-panel').forEach(p => {
                p.style.display = p.dataset.panel === tab ? '' : 'none';
            });
        });
    });

    // Populate seed dropdown (reuse seedRuns computed above)
    const seedSelect = document.getElementById('exp-drilldown-seed-select');
    const seedLabel = document.getElementById('exp-drilldown-seed-label');
    if (seedSelect && seedLabel) {
        if (seedRuns.length <= 1) {
            seedSelect.style.display = 'none';
            seedLabel.textContent = seedRuns.length === 1
                ? `seed: ${seedRuns[0].config.seed}`
                : '';
        } else {
            seedLabel.textContent = '';
            seedSelect.style.display = '';
            seedSelect.innerHTML = seedRuns
                .map(r => `<option value="${r.config.seed}">seed ${r.config.seed}</option>`)
                .join('');
        }
    }

    card.style.display = '';
}

// ---------------------------------------------------------------------------
// Simulate in Observatory
// ---------------------------------------------------------------------------

function simulateIn3D(idx) {
    if (!experimentData?.summaries) return;
    const s = experimentData.summaries[idx];
    if (!s) return;

    // Reset first — Rust updates local state after reset so subsequent commands
    // in the same batch see SimState::Idle.
    sendCommand({ type: 'set_state', value: 'reset' });

    // Exit experiment mode → configure phase
    setPhase('configure');

    // Find the topology in our registry (presets + custom imports)
    const allTopos = getAllExpTopologies();
    const topoEntry = allTopos.find(t => t.name === s.topology || t.id === s.topology);

    let topoPresetIdx = -1;
    if (topoEntry) {
        // Use load_custom_map (without baked robot positions) so:
        // 1. The 3D preview map appears immediately via PreviewMap resource
        // 2. imported_scenario.agents is empty → begin_loading falls through
        //    to place_agents() matching the experiment runner's RNG path
        const { robots: _r, ...gridData } = topoEntry.data;
        sendCommand({ type: 'load_custom_map', ...gridData });

        // Track preset index for DOM button activation
        const presetIdx = loadedTopologies.findIndex(t => t.id === topoEntry.id);
        if (presetIdx >= 0) topoPresetIdx = presetIdx;
    } else if (experimentTopoData[s.topology]) {
        // Topology not in current manifest but was used in this experiment session
        const { robots: _r, ...gridData } = experimentTopoData[s.topology];
        sendCommand({ type: 'load_custom_map', ...gridData });
    } else if (s.topology) {
        // Last resort: registry ID only (no visual preview but sets internal state)
        sendCommand({ type: 'set_topology', value: s.topology });
    }

    if (s.solver) sendCommand({ type: 'set_solver', value: s.solver });
    if (s.scheduler) sendCommand({ type: 'set_scheduler', value: s.scheduler });

    // Parse fault scenario label and configure observatory fault settings
    configureFaultFromScenarioLabel(s.scenario);

    // Get seed from dropdown (if present) or fall back to first matching run
    // Note: visibility is toggled via inline style.display by showExpDrilldown (Task 3)
    const seedSelectEl = document.getElementById('exp-drilldown-seed-select');
    let selectedSeed = null;
    if (seedSelectEl && seedSelectEl.style.display !== 'none') {
        const v = parseInt(seedSelectEl.value, 10);
        if (!isNaN(v)) selectedSeed = v;
    }

    const matchingRun = experimentData.runs?.find(r =>
        r.config?.solver === s.solver &&
        r.config?.topology === s.topology &&
        r.config?.scenario === s.scenario &&
        r.config?.scheduler === s.scheduler &&
        r.config?.num_agents === s.num_agents &&
        (selectedSeed == null || r.config?.seed === selectedSeed)
    );

    if (s.num_agents) {
        sendCommand({ type: 'set_num_agents', value: s.num_agents });
    }

    const seed = matchingRun?.config?.seed;
    if (seed != null) {
        sendCommand({ type: 'set_seed', value: seed });
    }

    const tickCount = matchingRun?.config?.tick_count;
    if (tickCount) {
        sendCommand({ type: 'set_duration', value: tickCount });
    }

    // Auto-start — all config commands above are queued in the same batch,
    // Bevy processes them in order then begin_loading spawns agents.
    sendCommand({ type: 'set_state', value: 'start' });

    // Sync UI controls (DOM-only, after bridge commands are queued)
    setTimeout(() => {
        // Topology preset button
        const topoPresets = document.getElementById('topology-presets');
        if (topoPresets) {
            topoPresets.querySelectorAll('.preset-btn').forEach(b => b.classList.remove('active'));
            if (topoPresetIdx >= 0) {
                const btn = topoPresets.querySelector(`[data-topo-idx="${topoPresetIdx}"]`);
                if (btn) btn.classList.add('active');
            }
        }
        // Hide customize controls (preset topology — dimensions are fixed)
        const customControls = document.getElementById('sim-customize-controls');
        if (customControls && topoPresetIdx >= 0) customControls.style.display = 'none';

        const solverSelect = document.getElementById('input-solver');
        if (solverSelect && s.solver) solverSelect.value = s.solver;
        const schedSelect = document.getElementById('input-scheduler');
        if (schedSelect && s.scheduler) schedSelect.value = s.scheduler;
        if (s.num_agents) syncSliderDisplay('input-agents', 'val-agents', s.num_agents);
        if (seed != null) {
            const seedEl = document.getElementById('input-seed');
            if (seedEl) seedEl.value = seed;
        }
        if (tickCount) syncSliderDisplay('input-duration', 'val-duration', tickCount);
    }, 50);
}

// Parse experiment scenario label and configure the observatory fault settings.
// Labels follow Rust's scenario_label() format:
//   "none", "burst_XXpct", "wear_RATE", "zone_XXt", "intermittent_XXmYYr"
function configureFaultFromScenarioLabel(label) {
    if (!label || label === 'none') {
        // Disable faults
        faultList = [];
        renderFaultList();
        syncFaultListToRust();
        return;
    }

    // Build fault list from scenario label
    faultList = [];

    if (label.startsWith('burst_')) {
        const match = label.match(/^burst_(\d+)pct$/);
        const killPct = match ? parseInt(match[1]) : 20;
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'burst_failure', kill_percent: killPct, at_tick: 100 });
    } else if (label.startsWith('wear_')) {
        const rate = label.replace('wear_', '');
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'wear_based', heat_rate: rate || 'medium' });
    } else if (label.startsWith('zone_')) {
        const match = label.match(/^zone_(\d+)t$/);
        const duration = match ? parseInt(match[1]) : 50;
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'zone_outage', at_tick: 100, duration });
    } else if (label.startsWith('intermittent')) {
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'intermittent_fault', mtbf: 80, recovery: 15 });
    } else if (label.startsWith('perm_zone_')) {
        const match = label.match(/^perm_zone_(\d+)pct$/);
        const blockPct = match ? parseInt(match[1]) : 100;
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'permanent_zone_outage', at_tick: 100, block_percent: blockPct });
    } else {
        faultList.push({ id: 'f_' + (++faultIdCounter), type: 'burst_failure', kill_percent: 20, at_tick: 100 });
    }

    renderFaultList();
    syncFaultListToRust();
}

// ---------------------------------------------------------------------------
// Experiment exports
// ---------------------------------------------------------------------------

function exportExpJSON() {
    if (!experimentData) return;
    downloadBlob(JSON.stringify(experimentData, null, 2), 'experiment.json', 'application/json');
}

function exportExpCSV() {
    if (!experimentData?.summaries) return;
    const summaries = experimentData.summaries;
    const configCols = ['solver', 'topology', 'scenario', 'scheduler', 'num_agents', 'num_seeds'];
    const header = [...configCols, ...EXPERIMENT_METRICS.map(m => m.key + '_mean'), ...EXPERIMENT_METRICS.map(m => m.key + '_std')];
    let csv = header.join(',') + '\n';
    for (const s of summaries) {
        const row = [
            s.solver, s.topology, s.scenario, s.scheduler, s.num_agents, s.num_seeds,
            ...EXPERIMENT_METRICS.map(m => getStat(s, m.key).mean.toFixed(4)),
            ...EXPERIMENT_METRICS.map(m => getStat(s, m.key).std.toFixed(4)),
        ];
        csv += row.join(',') + '\n';
    }
    downloadBlob(csv, 'experiment_summary.csv', 'text/csv');
}

function exportExpLatex() {
    if (!experimentData?.summaries) return;
    const summaries = experimentData.summaries;
    const cols = EXPERIMENT_METRICS.slice(0, 6);
    let tex = `\\begin{tabular}{lllr${'r'.repeat(cols.length)}}\n\\toprule\n`;
    tex += `Solver & Topology & Scenario & Agents`;
    for (const c of cols) tex += ` & ${c.label}`;
    tex += ' \\\\\n\\midrule\n';
    for (const s of summaries) {
        tex += `${s.solver} & ${s.topology} & ${s.scenario} & ${s.num_agents}`;
        for (const c of cols) {
            const stat = getStat(s, c.key);
            tex += ` & $${stat.mean.toFixed(c.decimals)} \\pm ${stat.std.toFixed(c.decimals)}$`;
        }
        tex += ' \\\\\n';
    }
    tex += '\\bottomrule\n\\end{tabular}\n';
    downloadBlob(tex, 'experiment.tex', 'text/plain');
}

function exportExpTypst() {
    if (!experimentData?.summaries) return;
    const summaries = experimentData.summaries;
    const cols = EXPERIMENT_METRICS.slice(0, 6);
    const totalCols = 4 + cols.length;
    let typ = `#table(\n  columns: ${totalCols},\n`;
    typ += `  [*Solver*], [*Topology*], [*Scenario*], [*Agents*]`;
    for (const c of cols) typ += `, [*${c.label}*]`;
    typ += ',\n';
    for (const s of summaries) {
        typ += `  [${s.solver}], [${s.topology}], [${s.scenario}], [${s.num_agents}]`;
        for (const c of cols) {
            const stat = getStat(s, c.key);
            typ += `, [${stat.mean.toFixed(c.decimals)} \u00b1 ${stat.std.toFixed(c.decimals)}]`;
        }
        typ += ',\n';
    }
    typ += ')\n';
    downloadBlob(typ, 'experiment.typ', 'text/plain');
}

// ---------------------------------------------------------------------------
// URL Sharing System
// ---------------------------------------------------------------------------
// Encodes the current observatory state into a URL hash fragment.
// Format: #s=<base64url-encoded compressed JSON>
// Uses CompressionStream (native) with raw base64 fallback.

function getShareableState() {
    // Read the latest bridge state (for fields not available from DOM)
    let state;
    try {
        const raw = get_simulation_state();
        if (!raw || raw === '{}') return null;
        state = JSON.parse(raw);
    } catch { return null; }

    // DOM helpers — DOM controls are always current, even in configure phase
    const domStr = (id) => document.getElementById(id)?.value || undefined;
    const domInt = (id) => { const v = parseInt(document.getElementById(id)?.value, 10); return isNaN(v) ? undefined : v; };
    const domFloat = (id) => { const v = parseFloat(document.getElementById(id)?.value); return isNaN(v) ? undefined : v; };
    const domBool = (id) => document.getElementById(id)?.checked ?? false;

    // Topology: use activeTopologyId (JS-tracked) — reliable even when load_custom_map sets name="custom"
    const topoId = (activeTopologyId && activeTopologyId !== 'custom')
        ? activeTopologyId
        : (state.topology && state.topology !== 'custom' ? state.topology : undefined);

    const shared = {
        v: 1,
        t: topoId,
        s: domStr('input-solver') || state.solver || undefined,
        sc: domStr('input-scheduler') || state.lifelong?.scheduler || undefined,
        n: domInt('input-agents') || state.num_agents || undefined,
        // Seed: read bridge (always synced); handle seed=0 explicitly
        sd: state.seed != null ? state.seed : undefined,
        hz: domFloat('input-tick-hz') || state.tick_hz || undefined,
        d: domInt('input-duration') || state.duration || undefined,
    };

    // Fault list — use the in-memory fault list as source of truth
    if (faultList.length > 0) {
        shared.faults = faultList.map(f => {
            const item = { type: f.type };
            if (f.type === 'burst_failure') { item.kill = f.kill_percent; item.at = f.at_tick; }
            if (f.type === 'wear_based') { item.rate = f.heat_rate; }
            if (f.type === 'zone_outage') { item.at = f.at_tick; item.dur = f.duration; }
            if (f.type === 'intermittent_fault') { item.mtbf = f.mtbf; item.rec = f.recovery; }
            if (f.type === 'permanent_zone_outage') { item.at = f.at_tick; item.blk = f.block_percent; }
            return item;
        });
    }

    // Strip undefined values
    return JSON.parse(JSON.stringify(shared));
}

async function compressToBase64Url(jsonStr) {
    if (typeof CompressionStream !== 'undefined') {
        const stream = new Blob([jsonStr]).stream().pipeThrough(new CompressionStream('deflate-raw'));
        const buf = await new Response(stream).arrayBuffer();
        return arrayBufferToBase64Url(buf);
    }
    // Fallback: raw base64url (no compression)
    return btoa(jsonStr).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

async function decompressFromBase64Url(b64) {
    const buf = base64UrlToArrayBuffer(b64);
    if (typeof DecompressionStream !== 'undefined') {
        try {
            const stream = new Blob([buf]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
            return await new Response(stream).text();
        } catch {
            // Fallback: maybe it was stored uncompressed
        }
    }
    // Fallback: raw base64url decode
    try {
        return atob(b64.replace(/-/g, '+').replace(/_/g, '/'));
    } catch { return null; }
}

function arrayBufferToBase64Url(buf) {
    const bytes = new Uint8Array(buf);
    let binary = '';
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function base64UrlToArrayBuffer(b64) {
    const padded = b64.replace(/-/g, '+').replace(/_/g, '/');
    const binary = atob(padded);
    const buf = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) buf[i] = binary.charCodeAt(i);
    return buf.buffer;
}

async function generateShareUrl() {
    const state = getShareableState();
    if (!state) return null;
    const json = JSON.stringify(state);
    const encoded = await compressToBase64Url(json);
    const url = new URL(window.location.href);
    url.hash = '';
    url.search = '';
    return url.origin + url.pathname + '#s=' + encoded;
}

async function applySharedState(hash) {
    if (!hash || !hash.startsWith('#s=')) return false;
    const encoded = hash.slice(3);
    if (!encoded) return false;

    let json;
    try {
        json = await decompressFromBase64Url(encoded);
        if (!json) return false;
    } catch { return false; }

    let shared;
    try {
        shared = JSON.parse(json);
    } catch { return false; }

    if (!shared || shared.v !== 1) return false;

    // Reset and configure
    sendCommand({ type: 'set_state', value: 'reset' });
    setPhase('configure');

    // Hide demo splash for shared URLs
    const splash = document.getElementById('demo-splash');
    if (splash) splash.style.display = 'none';
    localStorage.setItem('mafis-demo-seen', '1');

    // Apply topology: load_custom_map is required on WASM (TopologyRegistry is empty there).
    // Track activeTopologyId so getShareableState() can read the correct ID later.
    if (shared.t) {
        activeTopologyId = shared.t;
        const topo = loadedTopologies.find(t => t.id === shared.t);
        if (topo) {
            sendCommand({ type: 'load_custom_map', ...topo.data });
        }
    }

    if (shared.s) sendCommand({ type: 'set_solver', value: shared.s });
    if (shared.sc) sendCommand({ type: 'set_scheduler', value: shared.sc });
    // set_num_agents after set_topology (set_topology resets num_agents to suggested_agents)
    if (shared.n) sendCommand({ type: 'set_num_agents', value: shared.n });
    if (shared.sd != null) sendCommand({ type: 'set_seed', value: shared.sd });
    if (shared.hz) sendCommand({ type: 'set_tick_hz', value: shared.hz });
    if (shared.d) sendCommand({ type: 'set_duration', value: shared.d });

    // Fault list — restore from shared config (supports both old `f` and new `faults` format)
    faultList = [];
    if (shared.faults && Array.isArray(shared.faults)) {
        shared.faults.forEach(f => {
            const item = { id: 'f_' + (++faultIdCounter), type: f.type };
            if (f.type === 'burst_failure') { item.kill_percent = f.kill || 20; item.at_tick = f.at || 100; }
            if (f.type === 'wear_based') { item.heat_rate = f.rate || 'medium'; }
            if (f.type === 'zone_outage') { item.at_tick = f.at || 100; item.duration = f.dur || 50; }
            if (f.type === 'intermittent_fault') { item.mtbf = f.mtbf || 80; item.recovery = f.rec || 15; }
            if (f.type === 'permanent_zone_outage') { item.at_tick = f.at || 100; item.block_percent = f.blk || 30; }
            faultList.push(item);
        });
    } else if (shared.f) {
        // Legacy single-fault format
        const rateNames = { 0: 'low', 1: 'medium', 2: 'high' };
        let faultLabel = 'none';
        if (shared.f.type === 'burst_failure') faultLabel = `burst_${shared.f.kill || 20}pct`;
        else if (shared.f.type === 'wear_based') {
            const rateName = typeof shared.f.rate === 'number' ? (rateNames[shared.f.rate] ?? 'medium') : (shared.f.rate || 'medium');
            faultLabel = `wear_${rateName}`;
        } else if (shared.f.type === 'zone_outage') faultLabel = `zone_${shared.f.dur || 50}t`;
        else faultLabel = shared.f.type || 'none';
        configureFaultFromScenarioLabel(faultLabel);
    }
    if (faultList.length > 0) {
        renderFaultList();
        syncFaultListToRust();
    }

    // Auto-start
    sendCommand({ type: 'set_state', value: 'start' });

    // Sync DOM controls
    setTimeout(() => {
        if (shared.s) {
            const el = document.getElementById('input-solver');
            if (el) el.value = shared.s;
        }
        if (shared.sc) {
            const el = document.getElementById('input-scheduler');
            if (el) el.value = shared.sc;
        }
        if (shared.d) {
            const el = document.getElementById('input-duration');
            if (el) el.value = shared.d;
        }
        if (shared.n) {
            syncSliderDisplay('input-agents', 'val-agents', shared.n);
        }
        if (shared.sd != null) {
            const el = document.getElementById('input-seed');
            if (el) el.value = shared.sd;
        }
        // Mark topology button active
        if (shared.t) {
            const presetBtns = document.querySelectorAll('#topology-presets .preset-btn');
            presetBtns.forEach(btn => {
                const idx = parseInt(btn.dataset.topoIdx, 10);
                const topo = loadedTopologies[idx];
                btn.classList.toggle('active', topo?.id === shared.t);
            });
        }
    }, 200);

    return true;
}

function initShareButton() {
    const btn = document.getElementById('btn-share');
    if (!btn) return;

    btn.addEventListener('click', async () => {
        const url = await generateShareUrl();
        if (!url) return;

        try {
            await navigator.clipboard.writeText(url);
        } catch {
            // Fallback for non-HTTPS or denied clipboard
            const ta = document.createElement('textarea');
            ta.value = url;
            ta.style.position = 'fixed';
            ta.style.opacity = '0';
            document.body.appendChild(ta);
            ta.select();
            document.execCommand('copy');
            document.body.removeChild(ta);
        }

        // Visual feedback
        const prev = btn.textContent;
        btn.textContent = 'COPIED';
        btn.classList.add('copied');
        setTimeout(() => {
            btn.textContent = prev;
            btn.classList.remove('copied');
        }, 1500);
    });
}

// Check for shared state on load (called from initApp after topologies are loaded)
async function checkSharedUrl() {
    if (!window.location.hash || !window.location.hash.startsWith('#s=')) return;

    // Wait for Bevy to be ready (first non-empty state from bridge).
    // The WASM binary is ~18MB; Bevy's event loop may take several seconds to start.
    const maxWait = 15000; // 15s max
    const start = Date.now();
    while (Date.now() - start < maxWait) {
        try {
            const raw = get_simulation_state();
            if (raw && raw !== '{}') break;
        } catch { /* WASM not ready */ }
        await new Promise(r => setTimeout(r, 200));
    }

    // Also wait for topologies to load (fetched from manifest.json)
    let topoWait = 0;
    while (loadedTopologies.length === 0 && topoWait < 5000) {
        await new Promise(r => setTimeout(r, 100));
        topoWait += 100;
    }

    const applied = await applySharedState(window.location.hash);
    if (applied) {
        history.replaceState(null, '', window.location.pathname + window.location.search);
    }
}
