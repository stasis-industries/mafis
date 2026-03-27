// ===========================================================================
// MAFIS Map Maker — standalone 2D grid editor
// ===========================================================================

// Cell types
const WALKABLE = 0;
const WALL = 1;
const PICKUP = 2;
const DELIVERY = 3;
const RECHARGING = 4;

const CELL_COLORS_DARK = {
    [WALKABLE]:   '#2a2a2e',
    [WALL]:       '#1a1a1c',
    [PICKUP]:     '#d98d26',
    [DELIVERY]:   '#40b873',
    [RECHARGING]: '#4d8cd9',
};

const CELL_COLORS_LIGHT = {
    [WALKABLE]:   '#d5d3cf',
    [WALL]:       '#9e9b96',
    [PICKUP]:     '#e6a84a',
    [DELIVERY]:   '#52c98a',
    [RECHARGING]: '#6ba3e0',
};

function isDarkTheme() {
    return document.documentElement.getAttribute('data-theme') === 'dark';
}

function getCellColors() {
    return isDarkTheme() ? CELL_COLORS_DARK : CELL_COLORS_LIGHT;
}

function getCanvasBg() {
    return isDarkTheme() ? '#111113' : '#eae7e2';
}

function getGridLineColor() {
    return isDarkTheme() ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.08)';
}

function getCoordColor() {
    return isDarkTheme() ? 'rgba(255,255,255,0.3)' : 'rgba(0,0,0,0.25)';
}

function getHoverStroke() {
    return isDarkTheme() ? 'rgba(255,255,255,0.4)' : 'rgba(0,0,0,0.35)';
}

function getArrowFill() {
    return isDarkTheme() ? 'rgba(255, 255, 255, 0.85)' : 'rgba(0, 0, 0, 0.6)';
}

function getQueueGhostFill() {
    return isDarkTheme() ? 'rgba(64, 184, 115, 0.12)' : 'rgba(40, 160, 90, 0.15)';
}

const CELL_TYPE_NAMES = {
    [WALKABLE]: 'Walkable',
    [WALL]: 'Wall',
    [PICKUP]: 'Pickup',
    [DELIVERY]: 'Delivery',
    [RECHARGING]: 'Recharging',
};

const TOOL_TO_CELL = {
    wall: WALL,
    walkable: WALKABLE,
    pickup: PICKUP,
    delivery: DELIVERY,
    recharging: RECHARGING,
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let gridW = 32;
let gridH = 32;
let seed = 42;
let cells = [];     // cells[y][x] = cell type
let robots = [];    // [{x, y}, ...]

let activeTool = 'wall';
let drawMode = 'click';

// Viewport
let vpOffsetX = 0;
let vpOffsetY = 0;
let cellSize = 20;
const MIN_CELL_SIZE = 3;
const MAX_CELL_SIZE = 60;

// Undo / redo
let undoStack = [];
let redoStack = [];

// Interaction state
let isPainting = false;
let isErasing = false;
let isPanning = false;
let panStartX = 0;
let panStartY = 0;
let panStartOffsetX = 0;
let panStartOffsetY = 0;
let spaceDown = false;

// Rectangle mode
let rectStartCell = null;
let rectPreview = null;

// Hover
let hoverCell = null;

// Queue directions for delivery cells: "x,y" → "north"|"south"|"east"|"west"
let deliveryDirections = {};
let dirPickCells = [];        // delivery cells awaiting direction selection (batch)
let paintDirDiffs = [];       // direction diffs accumulated during a stroke

// Canvas
let canvas, ctx;

// Direction helpers — offsets match Rust Direction enum
const DIR_VISUAL_OFFSET = {
    east:  { dx: 1, dy: 0 },
    west:  { dx: -1, dy: 0 },
    north: { dx: 0, dy: 1 },   // +y in grid = down on screen
    south: { dx: 0, dy: -1 },  // -y in grid = up on screen
};

function directionFromDelta(dx, dy) {
    if (dx === 1 && dy === 0) return 'east';
    if (dx === -1 && dy === 0) return 'west';
    if (dx === 0 && dy === 1) return 'north';
    if (dx === 0 && dy === -1) return 'south';
    return null;
}

// Relaxed version: any click direction relative to cell → cardinal direction
function directionFromRelative(dx, dy) {
    if (dx === 0 && dy === 0) return null;
    if (Math.abs(dx) >= Math.abs(dy)) {
        return dx > 0 ? 'east' : 'west';
    } else {
        return dy > 0 ? 'north' : 'south';
    }
}

// Track hovered direction during direction pick
let dirPickPreviewDir = null;

// Compute center of dirPickCells (fractional coordinates)
function dirPickCenter() {
    if (dirPickCells.length === 0) return null;
    let sx = 0, sy = 0;
    for (const c of dirPickCells) { sx += c.x; sy += c.y; }
    return { x: sx / dirPickCells.length, y: sy / dirPickCells.length };
}

function setDeliveryDirection(x, y, dir) {
    const key = `${x},${y}`;
    const oldDir = deliveryDirections[key] || null;
    if (oldDir === dir) return;
    deliveryDirections[key] = dir;
    pushUndo({
        cellDiffs: [],
        dirDiffs: [{ key, before: oldDir, after: dir }],
    });
    updateValidation();
    render();
}

function cancelDirPick() {
    // Revert cells to walkable and remove from undo
    const cellDiffs = [];
    for (const { x, y } of dirPickCells) {
        if (cells[y][x] === DELIVERY) {
            cellDiffs.push({ x, y, before: DELIVERY, after: WALKABLE });
            cells[y][x] = WALKABLE;
            delete deliveryDirections[`${x},${y}`];
        }
    }
    if (cellDiffs.length > 0) {
        pushUndo({ cellDiffs, dirDiffs: [] });
    }
    dirPickCells = [];
    dirPickPreviewDir = null;
    showToast('Delivery cancelled — cells removed', 'error');
    updateValidation();
    render();
}

function setBatchDeliveryDirection(cellList, dir) {
    const dirDiffs = [];
    for (const { x, y } of cellList) {
        const key = `${x},${y}`;
        const oldDir = deliveryDirections[key] || null;
        if (oldDir === dir) continue;
        deliveryDirections[key] = dir;
        dirDiffs.push({ key, before: oldDir, after: dir });
    }
    if (dirDiffs.length > 0) {
        pushUndo({ cellDiffs: [], dirDiffs });
    }
    updateValidation();
    render();
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

function initGrid(w, h) {
    gridW = w;
    gridH = h;
    cells = [];
    for (let y = 0; y < h; y++) {
        cells[y] = new Array(w).fill(WALKABLE);
    }
    robots = [];
    deliveryDirections = {};
    dirPickCells = [];
    undoStack = [];
    redoStack = [];
}

function init() {
    canvas = document.getElementById('mm-canvas');
    ctx = canvas.getContext('2d');

    initTheme();
    initGrid(gridW, gridH);
    resizeCanvas();
    centerView();

    bindTools();
    bindModes();
    bindCanvas();
    bindProperties();
    bindRobotGen();
    bindImport();
    bindExport();
    bindHistory();
    bindViewControls();
    bindKeyboard();

    updateValidation();
    updateStatusBar();
    render();

    window.addEventListener('resize', () => {
        resizeCanvas();
        render();
    });
}

function resizeCanvas() {
    const area = canvas.parentElement;
    canvas.width = area.clientWidth;
    canvas.height = area.clientHeight;
}

function centerView() {
    const area = canvas.parentElement;
    const totalW = gridW * cellSize;
    const totalH = gridH * cellSize;
    vpOffsetX = (area.clientWidth - totalW) / 2;
    vpOffsetY = (area.clientHeight - totalH) / 2;
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function render() {
    const w = canvas.width;
    const h = canvas.height;

    const cellColors = getCellColors();
    ctx.fillStyle = getCanvasBg();
    ctx.fillRect(0, 0, w, h);

    const startCol = Math.max(0, Math.floor(-vpOffsetX / cellSize));
    const endCol = Math.min(gridW, Math.ceil((w - vpOffsetX) / cellSize));
    const startRow = Math.max(0, Math.floor(-vpOffsetY / cellSize));
    const endRow = Math.min(gridH, Math.ceil((h - vpOffsetY) / cellSize));

    // Draw cells
    for (let y = startRow; y < endRow; y++) {
        for (let x = startCol; x < endCol; x++) {
            const px = vpOffsetX + x * cellSize;
            const py = vpOffsetY + y * cellSize;
            ctx.fillStyle = cellColors[cells[y][x]];
            ctx.fillRect(px, py, cellSize, cellSize);

            // Grid lines
            if (cellSize >= 8) {
                ctx.strokeStyle = getGridLineColor();
                ctx.lineWidth = 1;
                ctx.strokeRect(px + 0.5, py + 0.5, cellSize - 1, cellSize - 1);
            }
        }
    }

    // Draw queue line ghost previews
    if (cellSize >= 6) {
        for (const [key, dir] of Object.entries(deliveryDirections)) {
            const [cx, cy] = key.split(',').map(Number);
            if (cells[cy]?.[cx] !== DELIVERY) continue;
            const off = DIR_VISUAL_OFFSET[dir];
            if (!off) continue;
            let qx = cx + off.dx, qy = cy + off.dy;
            while (inBounds(qx, qy) && cells[qy][qx] !== WALL) {
                const px = vpOffsetX + qx * cellSize;
                const py = vpOffsetY + qy * cellSize;
                ctx.fillStyle = getQueueGhostFill();
                ctx.fillRect(px + 1, py + 1, cellSize - 2, cellSize - 2);
                qx += off.dx;
                qy += off.dy;
            }
        }
    }

    // Draw queue direction arrows on delivery cells
    if (cellSize >= 10) {
        for (const [key, dir] of Object.entries(deliveryDirections)) {
            const [cx, cy] = key.split(',').map(Number);
            if (cx < startCol || cx >= endCol || cy < startRow || cy >= endRow) continue;
            if (cells[cy]?.[cx] !== DELIVERY) continue;
            drawDirectionArrow(cx, cy, dir);
        }
    }

    // Draw direction pick indicator (supports batch) with live preview
    if (dirPickCells.length > 0) {
        // Dashed border on all cells awaiting direction
        for (const dpc of dirPickCells) {
            const dpx = vpOffsetX + dpc.x * cellSize;
            const dpy = vpOffsetY + dpc.y * cellSize;
            ctx.strokeStyle = isDarkTheme() ? '#ffffff' : '#333333';
            ctx.lineWidth = 2;
            ctx.setLineDash([3, 3]);
            ctx.strokeRect(dpx + 1, dpy + 1, cellSize - 2, cellSize - 2);
            ctx.setLineDash([]);
        }
        // If hovering gives a preview direction, draw arrows and queue line preview
        if (dirPickPreviewDir) {
            const off = DIR_VISUAL_OFFSET[dirPickPreviewDir];
            if (off) {
                // Draw queue line preview: 3 cells extending from each delivery cell
                const PREVIEW_OPACITIES = [0.4, 0.25, 0.1];
                for (const dpc of dirPickCells) {
                    for (let i = 0; i < 3; i++) {
                        const qx = dpc.x + off.dx * (i + 1);
                        const qy = dpc.y + off.dy * (i + 1);
                        if (!inBounds(qx, qy) || cells[qy][qx] === WALL) break;
                        const qpx = vpOffsetX + qx * cellSize;
                        const qpy = vpOffsetY + qy * cellSize;
                        ctx.fillStyle = `rgba(64, 184, 115, ${PREVIEW_OPACITIES[i]})`;
                        ctx.fillRect(qpx + 1, qpy + 1, cellSize - 2, cellSize - 2);
                    }
                }
                // Draw direction arrows on all cells
                if (cellSize >= 10) {
                    for (const dpc of dirPickCells) {
                        drawDirectionArrow(dpc.x, dpc.y, dirPickPreviewDir);
                    }
                }
            }
        }
    }

    // Draw rectangle preview
    if (rectPreview && rectStartCell) {
        const x0 = Math.min(rectStartCell.x, rectPreview.x);
        const y0 = Math.min(rectStartCell.y, rectPreview.y);
        const x1 = Math.max(rectStartCell.x, rectPreview.x);
        const y1 = Math.max(rectStartCell.y, rectPreview.y);
        const px = vpOffsetX + x0 * cellSize;
        const py = vpOffsetY + y0 * cellSize;
        const pw = (x1 - x0 + 1) * cellSize;
        const ph = (y1 - y0 + 1) * cellSize;
        ctx.strokeStyle = isErasing ? 'rgba(255,100,100,0.7)' : 'rgba(255,255,255,0.7)';
        ctx.lineWidth = 2;
        ctx.setLineDash([4, 4]);
        ctx.strokeRect(px, py, pw, ph);
        ctx.setLineDash([]);
        ctx.fillStyle = isErasing ? 'rgba(255,100,100,0.1)' : 'rgba(255,255,255,0.1)';
        ctx.fillRect(px, py, pw, ph);
    }

    // Draw robots
    const robotRadius = Math.max(2, cellSize * 0.3);
    for (const r of robots) {
        if (r.x < startCol || r.x >= endCol || r.y < startRow || r.y >= endRow) continue;
        const cx = vpOffsetX + r.x * cellSize + cellSize / 2;
        const cy = vpOffsetY + r.y * cellSize + cellSize / 2;
        ctx.beginPath();
        ctx.arc(cx, cy, robotRadius, 0, Math.PI * 2);
        ctx.fillStyle = '#ffffff';
        ctx.fill();
        ctx.strokeStyle = '#000000';
        ctx.lineWidth = 1;
        ctx.stroke();
    }

    // Draw hover highlight
    if (hoverCell && hoverCell.x >= 0 && hoverCell.x < gridW && hoverCell.y >= 0 && hoverCell.y < gridH) {
        const px = vpOffsetX + hoverCell.x * cellSize;
        const py = vpOffsetY + hoverCell.y * cellSize;
        ctx.strokeStyle = getHoverStroke();
        ctx.lineWidth = 2;
        ctx.strokeRect(px + 1, py + 1, cellSize - 2, cellSize - 2);
    }

    // Draw coordinates
    drawCoordinates(startCol, endCol, startRow, endRow);
}

function drawDirectionArrow(cx, cy, dir) {
    const off = DIR_VISUAL_OFFSET[dir];
    if (!off) return;
    const px = vpOffsetX + cx * cellSize + cellSize / 2;
    const py = vpOffsetY + cy * cellSize + cellSize / 2;
    const sz = cellSize * 0.3;
    ctx.save();
    ctx.translate(px, py);
    ctx.rotate(Math.atan2(off.dy, off.dx));
    ctx.beginPath();
    ctx.moveTo(sz, 0);
    ctx.lineTo(-sz * 0.3, -sz * 0.6);
    ctx.lineTo(-sz * 0.3, sz * 0.6);
    ctx.closePath();
    ctx.fillStyle = getArrowFill();
    ctx.fill();
    ctx.restore();
}

function drawCoordinates(startCol, endCol, startRow, endRow) {
    if (cellSize < 12) return; // too small to read

    const step = cellSize >= 30 ? 1 : cellSize >= 15 ? 5 : 10;

    ctx.font = `${Math.min(10, cellSize * 0.4)}px "DM Mono", monospace`;
    ctx.fillStyle = getCoordColor();
    ctx.textAlign = 'center';
    ctx.textBaseline = 'bottom';

    // Column numbers along top
    for (let x = startCol; x < endCol; x++) {
        if (x % step !== 0) continue;
        const px = vpOffsetX + x * cellSize + cellSize / 2;
        const py = vpOffsetY - 3;
        if (py > 0 && py < canvas.height) {
            ctx.fillText(x.toString(), px, py);
        }
    }

    // Row numbers along left
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    for (let y = startRow; y < endRow; y++) {
        if (y % step !== 0) continue;
        const px = vpOffsetX - 4;
        const py = vpOffsetY + y * cellSize + cellSize / 2;
        if (px > 0 && px < canvas.width) {
            ctx.fillText(y.toString(), px, py);
        }
    }
}

// ---------------------------------------------------------------------------
// Cell ↔ screen conversion
// ---------------------------------------------------------------------------

function screenToCell(sx, sy) {
    const x = Math.floor((sx - vpOffsetX) / cellSize);
    const y = Math.floor((sy - vpOffsetY) / cellSize);
    return { x, y };
}

function inBounds(x, y) {
    return x >= 0 && x < gridW && y >= 0 && y < gridH;
}

// ---------------------------------------------------------------------------
// Tool / Mode binding
// ---------------------------------------------------------------------------

function bindTools() {
    const btns = document.querySelectorAll('.mm-tool-btn');
    btns.forEach(btn => {
        btn.addEventListener('click', () => {
            setTool(btn.dataset.tool);
        });
    });
}

function bindModes() {
    const btns = document.querySelectorAll('.mm-mode-btn');
    btns.forEach(btn => {
        btn.addEventListener('click', () => {
            setMode(btn.dataset.mode);
        });
    });
}

function setTool(name) {
    activeTool = name;
    dirPickCells = [];
    // Auto-switch paint → click when selecting delivery
    if (name === 'delivery' && drawMode === 'paint') {
        setMode('click');
    }
    document.querySelectorAll('.mm-tool-btn').forEach(b => {
        b.classList.toggle('active', b.dataset.tool === name);
    });
    render();
}

function setMode(name) {
    // Block paint mode for delivery tool
    if (name === 'paint' && activeTool === 'delivery') {
        showToast('Paint mode disabled for delivery — use Click or Rect');
        return;
    }
    drawMode = name;
    dirPickCells = [];
    document.querySelectorAll('.mm-mode-btn').forEach(b => {
        b.classList.toggle('active', b.dataset.mode === name);
    });
    render();
}

// ---------------------------------------------------------------------------
// Undo / Redo
// ---------------------------------------------------------------------------

function pushUndo(action) {
    undoStack.push(action);
    redoStack = [];
    updateHistoryButtons();
}

function undo() {
    if (undoStack.length === 0) return;
    const action = undoStack.pop();
    applyDiff(action, true);
    redoStack.push(action);
    updateHistoryButtons();
    updateValidation();
    updateStatusBar();
    render();
}

function redo() {
    if (redoStack.length === 0) return;
    const action = redoStack.pop();
    applyDiff(action, false);
    undoStack.push(action);
    updateHistoryButtons();
    updateValidation();
    updateStatusBar();
    render();
}

function applyDiff(action, isUndo) {
    for (const d of action.cellDiffs) {
        cells[d.y][d.x] = isUndo ? d.before : d.after;
    }
    if (action.robotsBefore !== undefined) {
        robots = JSON.parse(JSON.stringify(isUndo ? action.robotsBefore : action.robotsAfter));
    }
    if (action.dirDiffs) {
        for (const d of action.dirDiffs) {
            const val = isUndo ? d.before : d.after;
            if (val) {
                deliveryDirections[d.key] = val;
            } else {
                delete deliveryDirections[d.key];
            }
        }
    }
}

function updateHistoryButtons() {
    document.getElementById('btn-undo').disabled = undoStack.length === 0;
    document.getElementById('btn-redo').disabled = redoStack.length === 0;
}

function bindHistory() {
    document.getElementById('btn-undo').addEventListener('click', undo);
    document.getElementById('btn-redo').addEventListener('click', redo);
}

// ---------------------------------------------------------------------------
// Canvas interaction
// ---------------------------------------------------------------------------

let paintDiffs = [];
let paintRobotsBefore = null;

function bindCanvas() {
    canvas.addEventListener('mousedown', onMouseDown);
    canvas.addEventListener('mousemove', onMouseMove);
    canvas.addEventListener('mouseup', onMouseUp);
    canvas.addEventListener('mouseleave', onMouseLeave);
    canvas.addEventListener('wheel', onWheel, { passive: false });
    canvas.addEventListener('contextmenu', e => e.preventDefault());
}

function onMouseDown(e) {
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;

    // Direction pick mode: resolve on click (single or batch)
    if (dirPickCells.length > 0 && e.button === 0) {
        const cell = screenToCell(sx, sy);
        const center = dirPickCenter();
        const dx = cell.x - center.x;
        const dy = cell.y - center.y;
        const dir = directionFromRelative(dx, dy);
        if (dir) {
            if (dirPickCells.length === 1) {
                setDeliveryDirection(dirPickCells[0].x, dirPickCells[0].y, dir);
            } else {
                setBatchDeliveryDirection(dirPickCells, dir);
            }
            showToast(`Queue direction: ${dir} (${dirPickCells.length} cell${dirPickCells.length > 1 ? 's' : ''})`);
        } else {
            // Clicked on the cell itself (dx=0,dy=0) — ignore, keep waiting
            e.preventDefault();
            return;
        }
        dirPickCells = [];
        dirPickPreviewDir = null;
        render();
        e.preventDefault();
        return;
    }

    // Middle button or Space+left → pan
    if (e.button === 1 || (e.button === 0 && spaceDown)) {
        isPanning = true;
        panStartX = e.clientX;
        panStartY = e.clientY;
        panStartOffsetX = vpOffsetX;
        panStartOffsetY = vpOffsetY;
        canvas.style.cursor = 'grabbing';
        e.preventDefault();
        return;
    }

    if (e.button !== 0 && e.button !== 2) return;

    const erase = e.button === 2;
    isErasing = erase;
    const cell = screenToCell(sx, sy);

    if (drawMode === 'rect') {
        rectStartCell = cell;
        rectPreview = cell;
        render();
        return;
    }

    // Click or Paint start
    isPainting = true;
    paintDiffs = [];
    paintDirDiffs = [];
    paintRobotsBefore = JSON.parse(JSON.stringify(robots));

    if (inBounds(cell.x, cell.y)) {
        applyToolAt(cell.x, cell.y, erase);
    }
    render();
}

function onMouseMove(e) {
    const rect = canvas.getBoundingClientRect();
    const sx = e.clientX - rect.left;
    const sy = e.clientY - rect.top;

    // Update hover
    hoverCell = screenToCell(sx, sy);
    updateStatusBar();

    // Direction pick preview: compute direction from mouse relative to center of batch
    if (dirPickCells.length > 0) {
        const center = dirPickCenter();
        const dx = hoverCell.x - center.x;
        const dy = hoverCell.y - center.y;
        const newDir = directionFromRelative(dx, dy);
        if (newDir !== dirPickPreviewDir) {
            dirPickPreviewDir = newDir;
            render();
        }
        return; // no other interaction during direction pick
    }

    // Pan
    if (isPanning) {
        vpOffsetX = panStartOffsetX + (e.clientX - panStartX);
        vpOffsetY = panStartOffsetY + (e.clientY - panStartY);
        render();
        return;
    }

    // Rectangle preview — only redraw if the preview cell actually changed
    if (rectStartCell) {
        const newPreview = screenToCell(sx, sy);
        if (!rectPreview || newPreview.x !== rectPreview.x || newPreview.y !== rectPreview.y) {
            rectPreview = newPreview;
            render();
        }
        return;
    }

    // Paint mode continuous painting
    if (isPainting && drawMode === 'paint') {
        const cell = screenToCell(sx, sy);
        if (inBounds(cell.x, cell.y)) {
            applyToolAt(cell.x, cell.y, isErasing);
            render();
        }
    }
}

function onMouseUp(e) {
    if (isPanning) {
        isPanning = false;
        canvas.style.cursor = '';
        return;
    }

    // Rectangle mode — fill
    if (rectStartCell && rectPreview) {
        const erase = isErasing;
        const x0 = Math.min(rectStartCell.x, rectPreview.x);
        const y0 = Math.min(rectStartCell.y, rectPreview.y);
        const x1 = Math.max(rectStartCell.x, rectPreview.x);
        const y1 = Math.max(rectStartCell.y, rectPreview.y);

        paintDiffs = [];
        paintDirDiffs = [];
        paintRobotsBefore = JSON.parse(JSON.stringify(robots));

        for (let y = y0; y <= y1; y++) {
            for (let x = x0; x <= x1; x++) {
                if (inBounds(x, y)) {
                    applyToolAt(x, y, erase);
                }
            }
        }

        if (paintDiffs.length > 0 || paintDirDiffs.length > 0 || JSON.stringify(robots) !== JSON.stringify(paintRobotsBefore)) {
            pushUndo({
                cellDiffs: paintDiffs,
                dirDiffs: paintDirDiffs.length > 0 ? paintDirDiffs : undefined,
                robotsBefore: paintRobotsBefore,
                robotsAfter: JSON.parse(JSON.stringify(robots)),
            });
        }

        // Enter batch direction pick for delivery cells placed via rect
        if (activeTool === 'delivery' && !erase) {
            const placedDelivery = paintDiffs.filter(d => d.after === DELIVERY);
            if (placedDelivery.length > 0) {
                dirPickCells = placedDelivery.map(d => ({ x: d.x, y: d.y }));
            }
        }

        rectStartCell = null;
        rectPreview = null;
        paintDiffs = [];
        paintDirDiffs = [];
        paintRobotsBefore = null;
        updateValidation();
        updateStatusBar();
        if (dirPickCells.length > 0) {
            showToast(`Set direction for ${dirPickCells.length} delivery cell${dirPickCells.length > 1 ? 's' : ''} — click adjacent or press arrow`);
        }
        render();
        return;
    }

    // End click/paint stroke
    if (isPainting) {
        isPainting = false;
        if (paintDiffs.length > 0 || paintDirDiffs.length > 0 || JSON.stringify(robots) !== JSON.stringify(paintRobotsBefore)) {
            pushUndo({
                cellDiffs: paintDiffs,
                dirDiffs: paintDirDiffs.length > 0 ? paintDirDiffs : undefined,
                robotsBefore: paintRobotsBefore,
                robotsAfter: JSON.parse(JSON.stringify(robots)),
            });
        }

        // Enter direction pick for delivery placement in click mode
        if (drawMode === 'click' && activeTool === 'delivery' && !isErasing) {
            const placed = paintDiffs.find(d => d.after === DELIVERY);
            if (placed) {
                dirPickCells = [{ x: placed.x, y: placed.y }];
            } else {
                // Clicked existing delivery cell → re-enter direction pick
                const rect = canvas.getBoundingClientRect();
                const sx = e.clientX - rect.left;
                const sy = e.clientY - rect.top;
                const cell = screenToCell(sx, sy);
                if (inBounds(cell.x, cell.y) && cells[cell.y][cell.x] === DELIVERY) {
                    dirPickCells = [{ x: cell.x, y: cell.y }];
                }
            }
        }

        paintDiffs = [];
        paintDirDiffs = [];
        paintRobotsBefore = null;
        updateValidation();
        updateStatusBar();
        if (dirPickCells.length > 0) {
            showToast('Click adjacent cell or press arrow key to set queue direction');
        }
        render();
    }

    isErasing = false;
}

function onMouseLeave() {
    hoverCell = null;
    updateStatusBar();
    render();
}

function onWheel(e) {
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;

    const oldSize = cellSize;
    const delta = e.deltaY > 0 ? -1 : 1;
    let newSize = cellSize + delta * Math.max(1, Math.floor(cellSize * 0.1));
    newSize = Math.max(MIN_CELL_SIZE, Math.min(MAX_CELL_SIZE, newSize));

    if (newSize === oldSize) return;

    // Zoom centered on cursor
    const cellX = (mx - vpOffsetX) / oldSize;
    const cellY = (my - vpOffsetY) / oldSize;
    cellSize = newSize;
    vpOffsetX = mx - cellX * newSize;
    vpOffsetY = my - cellY * newSize;

    render();
}

// ---------------------------------------------------------------------------
// Apply tool at a cell
// ---------------------------------------------------------------------------

function applyToolAt(x, y, erase) {
    if (activeTool === 'robot' && !erase) {
        // Place robot (only on walkable cells, no duplicates)
        if (cells[y][x] === WALL) return;
        if (robots.some(r => r.x === x && r.y === y)) return;
        robots.push({ x, y });
        return;
    }

    if (activeTool === 'robot' && erase) {
        // Remove robot
        robots = robots.filter(r => !(r.x === x && r.y === y));
        return;
    }

    const newType = erase ? WALKABLE : TOOL_TO_CELL[activeTool];
    if (newType === undefined) return;

    const oldType = cells[y][x];
    if (oldType === newType) return;

    // Check if a diff for this cell already exists in this stroke
    const existing = paintDiffs.find(d => d.x === x && d.y === y);
    if (existing) {
        existing.after = newType;
    } else {
        paintDiffs.push({ x, y, before: oldType, after: newType });
    }

    cells[y][x] = newType;

    // Remove queue direction if cell is no longer delivery
    if (oldType === DELIVERY && newType !== DELIVERY) {
        const key = `${x},${y}`;
        const oldDir = deliveryDirections[key] || null;
        if (oldDir) {
            paintDirDiffs.push({ key, before: oldDir, after: null });
            delete deliveryDirections[key];
        }
    }

    // Auto-remove robots on walls
    if (newType === WALL) {
        robots = robots.filter(r => !(r.x === x && r.y === y));
    }
}

// ---------------------------------------------------------------------------
// Grid resize (center-anchored)
// ---------------------------------------------------------------------------

function resizeGrid(newW, newH) {
    newW = Math.max(8, Math.min(512, newW));
    newH = Math.max(8, Math.min(512, newH));
    if (newW === gridW && newH === gridH) return;

    const diffW = newW - gridW;
    const diffH = newH - gridH;
    const padLeft = Math.floor(diffW / 2);
    const padTop = Math.floor(diffH / 2);

    const oldCells = cells;
    const oldRobots = robots;
    const oldDirs = deliveryDirections;

    const newCells = [];
    for (let y = 0; y < newH; y++) {
        newCells[y] = new Array(newW).fill(WALKABLE);
        for (let x = 0; x < newW; x++) {
            const srcX = x - padLeft;
            const srcY = y - padTop;
            if (srcX >= 0 && srcX < gridW && srcY >= 0 && srcY < gridH) {
                newCells[y][x] = oldCells[srcY][srcX];
            }
        }
    }

    const newRobots = [];
    for (const r of oldRobots) {
        const nx = r.x + padLeft;
        const ny = r.y + padTop;
        if (nx >= 0 && nx < newW && ny >= 0 && ny < newH && newCells[ny][nx] !== WALL) {
            newRobots.push({ x: nx, y: ny });
        }
    }

    // Build undo action (full snapshot since resize is complex)
    const cellDiffs = [];
    // Record all old cells as "before"
    for (let y = 0; y < gridH; y++) {
        for (let x = 0; x < gridW; x++) {
            if (oldCells[y][x] !== WALKABLE) {
                cellDiffs.push({ x, y, before: oldCells[y][x], after: WALKABLE });
            }
        }
    }

    cells = newCells;
    robots = newRobots;
    gridW = newW;
    gridH = newH;

    // Remap delivery directions to new coordinates
    deliveryDirections = {};
    for (const [key, dir] of Object.entries(oldDirs)) {
        const [ox, oy] = key.split(',').map(Number);
        const nx = ox + padLeft;
        const ny = oy + padTop;
        if (nx >= 0 && nx < newW && ny >= 0 && ny < newH && newCells[ny][nx] === DELIVERY) {
            deliveryDirections[`${nx},${ny}`] = dir;
        }
    }
    dirPickCells = [];

    // Clear undo stack on resize (too complex to reverse reliably)
    undoStack = [];
    redoStack = [];
    updateHistoryButtons();

    centerView();
    updateValidation();
    updateStatusBar();
    render();

    if (newRobots.length < oldRobots.length) {
        showToast(`${oldRobots.length - newRobots.length} robot(s) removed (out of bounds)`);
    }
}

function bindProperties() {
    const wInput = document.getElementById('input-grid-w');
    const hInput = document.getElementById('input-grid-h');
    const seedInput = document.getElementById('input-seed');

    wInput.value = gridW;
    hInput.value = gridH;
    seedInput.value = seed;

    wInput.addEventListener('change', () => {
        resizeGrid(parseInt(wInput.value, 10) || gridW, gridH);
        wInput.value = gridW;
    });

    hInput.addEventListener('change', () => {
        resizeGrid(gridW, parseInt(hInput.value, 10) || gridH);
        hInput.value = gridH;
    });

    seedInput.addEventListener('change', () => {
        seed = Math.max(0, Math.min(9999, parseInt(seedInput.value, 10) || 0));
        seedInput.value = seed;
    });
}

// ---------------------------------------------------------------------------
// Robot generation
// ---------------------------------------------------------------------------

function bindRobotGen() {
    document.getElementById('btn-gen-replace').addEventListener('click', () => generateRobots(true));
    document.getElementById('btn-gen-add').addEventListener('click', () => generateRobots(false));
}

function generateRobots(replace) {
    const count = parseInt(document.getElementById('input-robot-count').value, 10) || 0;
    if (count <= 0) return;

    // Collect walkable cells
    const walkable = [];
    const occupied = new Set();
    if (!replace) {
        for (const r of robots) occupied.add(`${r.x},${r.y}`);
    }

    for (let y = 0; y < gridH; y++) {
        for (let x = 0; x < gridW; x++) {
            if (cells[y][x] !== WALL && !occupied.has(`${x},${y}`)) {
                walkable.push({ x, y });
            }
        }
    }

    const available = walkable.length;
    const toPlace = Math.min(count, available);

    if (toPlace === 0) {
        showToast('No available walkable cells');
        return;
    }
    if (toPlace < count) {
        showToast(`Only ${available} cells available — placed ${toPlace} robots`);
    }

    const robotsBefore = JSON.parse(JSON.stringify(robots));

    if (replace) {
        robots = [];
    }

    // Simple seeded shuffle using seed field
    const rng = mulberry32(seed);
    shuffle(walkable, rng);

    for (let i = 0; i < toPlace; i++) {
        robots.push(walkable[i]);
    }

    pushUndo({
        cellDiffs: [],
        robotsBefore,
        robotsAfter: JSON.parse(JSON.stringify(robots)),
    });

    updateValidation();
    updateStatusBar();
    render();
}

// Simple PRNG (mulberry32)
function mulberry32(a) {
    return function() {
        a |= 0; a = a + 0x6D2B79F5 | 0;
        let t = Math.imul(a ^ a >>> 15, 1 | a);
        t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
        return ((t ^ t >>> 14) >>> 0) / 4294967296;
    };
}

function shuffle(arr, rng) {
    for (let i = arr.length - 1; i > 0; i--) {
        const j = Math.floor(rng() * (i + 1));
        [arr[i], arr[j]] = [arr[j], arr[i]];
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

function bindImport() {
    document.getElementById('input-import-map').addEventListener('change', importMapFile);
    document.getElementById('input-import-scen').addEventListener('change', importScenFile);
    document.getElementById('input-import-json').addEventListener('change', importJsonFile);
}

function importMapFile(e) {
    const file = e.target.files[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
        const parsed = parseMapFile(reader.result);
        if (!parsed) {
            showToast('Failed to parse .map file');
            return;
        }
        loadMapData(parsed.width, parsed.height, parsed.obstacles);
        showToast(`Imported map: ${parsed.width}x${parsed.height}`);
    };
    reader.readAsText(file);
    e.target.value = '';
}

function importScenFile(e) {
    const file = e.target.files[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
        const agents = parseScenFile(reader.result);
        if (!agents || agents.length === 0) {
            showToast('No agents found in .scen file');
            return;
        }
        loadScenData(agents);
    };
    reader.readAsText(file);
    e.target.value = '';
}

function importJsonFile(e) {
    const file = e.target.files[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
        try {
            const data = JSON.parse(reader.result);
            if (loadJsonData(data) !== false) {
                showToast(`Imported JSON: ${data.width}x${data.height}`);
            }
        } catch (err) {
            showToast('Failed to parse JSON file');
        }
    };
    reader.readAsText(file);
    e.target.value = '';
}

function parseMapFile(text) {
    const lines = text.split('\n');
    let width = 0, height = 0, mapStart = -1;

    for (let i = 0; i < lines.length; i++) {
        const line = lines[i].trim();
        if (line.startsWith('height')) height = parseInt(line.split(/\s+/)[1], 10);
        if (line.startsWith('width')) width = parseInt(line.split(/\s+/)[1], 10);
        if (line === 'map') { mapStart = i + 1; break; }
    }

    if (width <= 0 || height <= 0 || mapStart < 0) return null;

    const obstacles = [];
    for (let y = 0; y < height && (mapStart + y) < lines.length; y++) {
        const row = lines[mapStart + y];
        for (let x = 0; x < width && x < row.length; x++) {
            const ch = row[x];
            if (ch !== '.' && ch !== 'G') {
                obstacles.push({ x, y });
            }
        }
    }

    return { width, height, obstacles };
}

function parseScenFile(text) {
    const lines = text.split('\n');
    const agents = [];

    for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('version')) continue;
        const parts = trimmed.split(/\s+/);
        if (parts.length >= 9) {
            const sx = parseInt(parts[4], 10);
            const sy = parseInt(parts[5], 10);
            if (!isNaN(sx) && !isNaN(sy)) {
                agents.push({ x: sx, y: sy });
            }
        }
    }

    return agents;
}

function loadMapData(w, h, obstacles) {
    gridW = Math.min(512, Math.max(8, w));
    gridH = Math.min(512, Math.max(8, h));
    cells = [];
    for (let y = 0; y < gridH; y++) {
        cells[y] = new Array(gridW).fill(WALKABLE);
    }
    for (const ob of obstacles) {
        if (ob.x >= 0 && ob.x < gridW && ob.y >= 0 && ob.y < gridH) {
            cells[ob.y][ob.x] = WALL;
        }
    }

    // Auto-classify zones (JS port of Rust heuristic)
    classifyZones();

    robots = [];
    deliveryDirections = {};
    dirPickCells = [];
    undoStack = [];
    redoStack = [];
    updateHistoryButtons();

    document.getElementById('input-grid-w').value = gridW;
    document.getElementById('input-grid-h').value = gridH;

    centerView();
    updateValidation();
    updateStatusBar();
    render();
}

function loadScenData(agents) {
    // If no grid loaded yet, auto-size
    if (gridW <= 8 && gridH <= 8) {
        let maxX = 0, maxY = 0;
        for (const a of agents) {
            if (a.x > maxX) maxX = a.x;
            if (a.y > maxY) maxY = a.y;
        }
        resizeGrid(Math.max(8, maxX + 2), Math.max(8, maxY + 2));
    }

    const robotsBefore = JSON.parse(JSON.stringify(robots));
    let skipped = 0;
    const placed = new Set(robots.map(r => `${r.x},${r.y}`));

    for (const a of agents) {
        if (!inBounds(a.x, a.y)) { skipped++; continue; }
        if (cells[a.y][a.x] === WALL) { skipped++; continue; }
        if (placed.has(`${a.x},${a.y}`)) { skipped++; continue; }
        robots.push({ x: a.x, y: a.y });
        placed.add(`${a.x},${a.y}`);
    }

    if (robots.length > robotsBefore.length) {
        pushUndo({
            cellDiffs: [],
            robotsBefore,
            robotsAfter: JSON.parse(JSON.stringify(robots)),
        });
    }

    const total = agents.length;
    const imported = total - skipped;
    showToast(`Imported ${imported}/${total} robots` + (skipped > 0 ? ` (${skipped} skipped)` : ''));

    updateValidation();
    updateStatusBar();
    render();
}

function loadJsonData(data) {
    // --- Strict validation ---------------------------------------------------
    const validDirs = ['north', 'south', 'east', 'west'];
    const validCellTypes = ['wall', 'pickup', 'delivery', 'recharging', 'walkable'];

    if (typeof data.width !== 'number' || typeof data.height !== 'number') {
        showToast('Invalid JSON: "width" and "height" must be numbers', 'error');
        return false;
    }
    if (data.width < 8 || data.width > 512 || data.height < 8 || data.height > 512) {
        showToast(`Invalid JSON: dimensions must be 8–512 (got ${data.width}x${data.height})`, 'error');
        return false;
    }
    if (!Array.isArray(data.cells) || data.cells.length === 0) {
        showToast('Invalid JSON: "cells" array is required and must not be empty', 'error');
        return false;
    }
    for (let i = 0; i < data.cells.length; i++) {
        const c = data.cells[i];
        if (typeof c.x !== 'number' || typeof c.y !== 'number' || typeof c.type !== 'string') {
            showToast(`Invalid JSON: cell[${i}] must have numeric "x", "y" and string "type"`, 'error');
            return false;
        }
        if (!validCellTypes.includes(c.type)) {
            showToast(`Invalid JSON: cell[${i}] has unknown type "${c.type}"`, 'error');
            return false;
        }
        if (c.type === 'delivery') {
            if (!c.queue_direction || !validDirs.includes(c.queue_direction)) {
                showToast(
                    `Invalid JSON: delivery cell[${i}] at (${c.x},${c.y}) must have "queue_direction" (north|south|east|west)`,
                    'error'
                );
                return false;
            }
        }
    }
    // --- End validation ------------------------------------------------------

    gridW = data.width;
    gridH = data.height;
    seed = data.seed || 42;

    cells = [];
    for (let y = 0; y < gridH; y++) {
        cells[y] = new Array(gridW).fill(WALKABLE);
    }

    deliveryDirections = {};
    for (const c of data.cells) {
        if (c.x >= 0 && c.x < gridW && c.y >= 0 && c.y < gridH) {
            const t = { wall: WALL, pickup: PICKUP, delivery: DELIVERY, recharging: RECHARGING }[c.type];
            if (t !== undefined) cells[c.y][c.x] = t;
            if (c.type === 'delivery') {
                deliveryDirections[`${c.x},${c.y}`] = c.queue_direction;
            }
        }
    }

    robots = [];
    if (data.robots) {
        for (const r of data.robots) {
            if (inBounds(r.x, r.y) && cells[r.y][r.x] !== WALL) {
                robots.push({ x: r.x, y: r.y });
            }
        }
    }

    undoStack = [];
    redoStack = [];
    updateHistoryButtons();

    document.getElementById('input-grid-w').value = gridW;
    document.getElementById('input-grid-h').value = gridH;
    document.getElementById('input-seed').value = seed;
    const nameEl = document.getElementById('input-map-name');
    if (nameEl) nameEl.value = data.name || '';

    centerView();
    updateValidation();
    updateStatusBar();
    render();
    return true;
}

// ---------------------------------------------------------------------------
// Zone auto-classification (JS port of Rust classify_imported_zones)
// ---------------------------------------------------------------------------

function classifyZones() {
    // Detect warehouse structure: columns containing obstacle runs >= 5
    const hasStorageBlock = new Array(gridW).fill(false);

    for (let y = 0; y < gridH; y++) {
        let run = 0, runStart = 0;
        for (let x = 0; x < gridW; x++) {
            if (cells[y][x] === WALL) {
                if (run === 0) runStart = x;
                run++;
            } else {
                if (run >= 5) {
                    for (let c = runStart; c < runStart + run; c++) hasStorageBlock[c] = true;
                }
                run = 0;
            }
        }
        if (run >= 5) {
            for (let c = runStart; c < runStart + run; c++) hasStorageBlock[c] = true;
        }
    }

    const sl = hasStorageBlock.indexOf(true);
    const sr = hasStorageBlock.lastIndexOf(true);

    if (sl === -1) return; // No warehouse structure

    for (let y = 0; y < gridH; y++) {
        for (let x = 0; x < gridW; x++) {
            if (cells[y][x] !== WALKABLE) continue;

            if (x < sl || x > sr) {
                cells[y][x] = DELIVERY;
            } else {
                // Check adjacency to obstacles
                const adj = [
                    [x-1,y],[x+1,y],[x,y-1],[x,y+1]
                ].some(([nx,ny]) => inBounds(nx,ny) && cells[ny][nx] === WALL);

                if (adj) {
                    cells[y][x] = PICKUP;
                }
                // Corridors stay as WALKABLE
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

function bindExport() {
    document.getElementById('btn-save-json').addEventListener('click', exportJson);
    document.getElementById('btn-save-scen').addEventListener('click', exportScen);
    document.getElementById('btn-load-sim').addEventListener('click', loadIntoSimulator);
}

function buildJsonData() {
    const cellsData = [];
    for (let y = 0; y < gridH; y++) {
        for (let x = 0; x < gridW; x++) {
            const t = cells[y][x];
            if (t === WALKABLE) continue;
            const typeName = { [WALL]: 'wall', [PICKUP]: 'pickup', [DELIVERY]: 'delivery', [RECHARGING]: 'recharging' }[t];
            if (typeName) {
                const entry = { x, y, type: typeName };
                if (typeName === 'delivery') {
                    const dir = deliveryDirections[`${x},${y}`];
                    if (dir) entry.queue_direction = dir;
                }
                cellsData.push(entry);
            }
        }
    }

    const nameEl = document.getElementById('input-map-name');
    const name = nameEl?.value?.trim() || `${gridW}x${gridH}`;

    return {
        version: 1,
        name,
        width: gridW,
        height: gridH,
        seed,
        cells: cellsData,
        robots: robots.map(r => ({ x: r.x, y: r.y })),
    };
}

function exportJson() {
    const data = buildJsonData();
    const json = JSON.stringify(data, null, 2);
    // Use map name for filename (sanitized)
    const safeName = data.name.replace(/[^a-zA-Z0-9_-]/g, '_').toLowerCase();
    downloadFile(`${safeName}.json`, json, 'application/json');
}

function exportScen() {
    if (robots.length === 0) return;
    let lines = ['version 1'];
    for (let i = 0; i < robots.length; i++) {
        const r = robots[i];
        lines.push(`${i}\tcustom\t${gridW}\t${gridH}\t${r.x}\t${r.y}\t-1\t-1\t-1`);
    }
    downloadFile(`scenario_${robots.length}agents.scen`, lines.join('\n'), 'text/plain');
}

function loadIntoSimulator() {
    const data = buildJsonData();
    localStorage.setItem('mapfis_custom_map', JSON.stringify(data));
    window.location.href = 'index.html?source=custom';
}

function downloadFile(filename, content, mime) {
    const blob = new Blob([content], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

function countCellType(type) {
    let n = 0;
    for (let y = 0; y < gridH; y++) {
        for (let x = 0; x < gridW; x++) {
            if (cells[y][x] === type) n++;
        }
    }
    return n;
}

function updateValidation() {
    const pickupCount = countCellType(PICKUP);
    const deliveryCount = countCellType(DELIVERY);
    const rechargingCount = countCellType(RECHARGING);
    const robotCount = robots.length;

    const dirCount = Object.keys(deliveryDirections).length;

    const rules = [
        { label: 'Pickup zone', ok: pickupCount > 0, text: pickupCount > 0 ? `${pickupCount} cells` : 'None' },
        { label: 'Delivery zone', ok: deliveryCount > 0, text: deliveryCount > 0 ? `${deliveryCount} cells` : 'None' },
        { label: 'Queue dirs', info: true, text: dirCount > 0 ? `${dirCount}/${deliveryCount} set` : 'None (optional)' },
        { label: 'Robots', ok: robotCount > 0, text: robotCount > 0 ? `${robotCount} placed` : 'None' },
        { label: 'Recharging', info: true, text: rechargingCount > 0 ? `${rechargingCount} cells` : 'Absent (optional)' },
    ];

    const el = document.getElementById('mm-validation');
    el.innerHTML = rules.map(r => {
        const cls = r.info ? 'mm-val-info' : (r.ok ? 'mm-val-ok' : 'mm-val-fail');
        return `<div class="mm-val-row"><span class="${cls}"></span><span class="mm-val-label">${r.label}</span><span class="mm-val-text">${r.text}</span></div>`;
    }).join('');

    const valid = pickupCount > 0 && deliveryCount > 0 && robotCount > 0;
    document.getElementById('btn-save-json').disabled = !valid;
    document.getElementById('btn-load-sim').disabled = !valid;
    document.getElementById('btn-save-scen').disabled = robotCount === 0;
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

function updateStatusBar() {
    const wallCount = countCellType(WALL);
    const robotCount = robots.length;
    let hoverText = '';
    if (hoverCell && inBounds(hoverCell.x, hoverCell.y)) {
        const ct = cells[hoverCell.y][hoverCell.x];
        const hasRobot = robots.some(r => r.x === hoverCell.x && r.y === hoverCell.y);
        const dir = deliveryDirections[`${hoverCell.x},${hoverCell.y}`];
        const dirLabel = dir ? ` [queue: ${dir}]` : '';
        hoverText = `(${hoverCell.x}, ${hoverCell.y}) ${CELL_TYPE_NAMES[ct]}${dirLabel}${hasRobot ? ' [Robot]' : ''}`;
    } else {
        hoverText = '\u2014';
    }

    document.getElementById('mm-status-bar').textContent =
        `${gridW} \u00d7 ${gridH} | ${wallCount} walls | ${robotCount} robots | ${hoverText}`;
}

// ---------------------------------------------------------------------------
// Toast notifications
// ---------------------------------------------------------------------------

let activeToastEl = null;
let activeToastTimer = null;

function showToast(msg, variant) {
    // Remove previous toast immediately
    if (activeToastEl) {
        activeToastEl.remove();
        clearTimeout(activeToastTimer);
        activeToastEl = null;
    }
    const container = document.getElementById('mm-toasts');
    const el = document.createElement('div');
    el.className = 'mm-toast' + (variant === 'error' ? ' mm-toast-error' : '');
    el.textContent = msg;
    container.appendChild(el);
    activeToastEl = el;

    activeToastTimer = setTimeout(() => {
        el.classList.add('mm-toast-fade');
        setTimeout(() => {
            el.remove();
            if (activeToastEl === el) activeToastEl = null;
        }, 300);
    }, 3000);
}

// ---------------------------------------------------------------------------
// Keyboard shortcuts
// ---------------------------------------------------------------------------

function bindKeyboard() {
    document.addEventListener('keydown', e => {
        // Space for pan mode
        if (e.code === 'Space' && !e.repeat) {
            spaceDown = true;
            canvas.style.cursor = 'grab';
            e.preventDefault();
            return;
        }

        // Ctrl+Z / Ctrl+Shift+Z
        if ((e.ctrlKey || e.metaKey) && e.key === 'z') {
            e.preventDefault();
            if (e.shiftKey) { redo(); } else { undo(); }
            return;
        }

        // Direction pick: arrow keys or Escape (single or batch)
        if (dirPickCells.length > 0) {
            if (e.key === 'Escape') {
                cancelDirPick();
                e.preventDefault();
                return;
            }
            const dirMap = { ArrowRight: 'east', ArrowLeft: 'west', ArrowDown: 'north', ArrowUp: 'south' };
            const dir = dirMap[e.key];
            if (dir) {
                if (dirPickCells.length === 1) {
                    setDeliveryDirection(dirPickCells[0].x, dirPickCells[0].y, dir);
                } else {
                    setBatchDeliveryDirection(dirPickCells, dir);
                    showToast(`Queue direction: ${dir} (${dirPickCells.length} cells)`);
                }
                dirPickCells = [];
                dirPickPreviewDir = null;
                render();
                e.preventDefault();
                return;
            }
        }

        // Tool shortcuts
        if (!e.ctrlKey && !e.metaKey && !e.altKey) {
            switch (e.key) {
                case '1': setTool('wall'); break;
                case '2': setTool('walkable'); break;
                case '3': setTool('pickup'); break;
                case '4': setTool('delivery'); break;
                // case '5': recharging not yet modelled — shortcut disabled
                case '6': setTool('robot'); break;
                case 'q': case 'Q': setMode('click'); break;
                case 'w': case 'W': setMode('paint'); break;
                case 'e': case 'E': setMode('rect'); break;
                case 'c': case 'C': centerView(); render(); break;
            }
        }
    });

    document.addEventListener('keyup', e => {
        if (e.code === 'Space') {
            spaceDown = false;
            if (!isPanning) canvas.style.cursor = '';
        }
    });
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

function initTheme() {
    const saved = localStorage.getItem('mafis-theme');
    if (saved === 'dark') {
        document.documentElement.setAttribute('data-theme', 'dark');
        const btn = document.getElementById('btn-theme');
        if (btn) btn.textContent = '\u2600'; // sun
    }
    const btn = document.getElementById('btn-theme');
    if (btn) btn.addEventListener('click', toggleTheme);
}

function toggleTheme() {
    const wasDark = document.documentElement.getAttribute('data-theme') === 'dark';
    if (wasDark) {
        document.documentElement.removeAttribute('data-theme');
        document.getElementById('btn-theme').textContent = '\u263D'; // moon
        localStorage.setItem('mafis-theme', 'light');
    } else {
        document.documentElement.setAttribute('data-theme', 'dark');
        document.getElementById('btn-theme').textContent = '\u2600'; // sun
        localStorage.setItem('mafis-theme', 'dark');
    }
    render(); // re-draw canvas with new theme colors
}

// ---------------------------------------------------------------------------
// View controls
// ---------------------------------------------------------------------------

function bindViewControls() {
    document.getElementById('btn-center').addEventListener('click', () => {
        centerView();
        render();
    });
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', init);
