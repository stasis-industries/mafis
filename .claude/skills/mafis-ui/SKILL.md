---
name: mafis-ui
description: >
  Design system for MAFIS web UI (WASM build only). Use this skill when building,
  modifying, or styling any HTML, CSS, or JS component in the web/ directory. Triggers: "add a
  button", "style the panel", "change the layout", "UI component", "CSS", "HTML structure",
  "design the dashboard", "make it look better", "fix the styling", "add a card", "toolbar",
  "sidebar", "metric display", "results page", "configure panel", or any web frontend work in
  the web/ directory. This skill defines the "Scientific Instrument" design language — no
  border-radius on buttons, no pure white backgrounds, DM Mono for numeric values, cream/sand
  palette. For desktop/egui UI work in src/ui/desktop/, use mafis-desktop-ui instead.
---

# MAFIS Web UI Design System — "Scientific Instrument"

**Applies to: `web/` directory only (WASM build).** Desktop native uses egui — see mafis-desktop-ui skill.

The aesthetic of a high-precision measurement instrument. Sober, technical, confident.
The 3D Bevy viewport is the absolute center of the experience — the UI serves it, never competes.

**If the UI draws more attention than the simulation, it has failed.**

Target audience: researchers, engineers, developers.
Visual references: Linear, Three.js Editor, Weights & Biases dashboard.

## Page Architecture

```
┌─────────────┬──────────────────────────────┬────────────┐
│ #panel-left │                              │#panel-right│
│ Parameters  │     #canvas-bevy             │ Real-time  │
│ Config      │     (Bevy WASM viewport)     │ Metrics    │
│ Scenario    │                              │ Agents     │
└─────────────┴──────────────────────────────┴────────────┘
│                  #toolbar-bottom                         │
│        Simulation controls · Timeline · Status           │
└──────────────────────────────────────────────────────────┘
```

Canvas takes **70-75%** width. Side panels are discreet tools.

## Colors

### Core Rule
Background is **never pure white**. Always slightly tinted cream/sand.

### Main Palette
| Role | CSS Value | Usage |
|------|-----------|-------|
| Body background | `rgb(246, 244, 240)` | Page, panels |
| Panel surface | `rgb(244, 241, 237)` | Sidebars, toolbar |
| Card surface | `rgb(247, 244, 240)` | Stat cards, sections |
| Input surface | `rgb(240, 237, 233)` | Inputs, slider tracks |
| Dark surface | `rgb(8, 8, 8)` | Header bar |
| Separator | `rgba(5, 5, 5, 0.08)` | Borders, dividers |

### Text Palette
| Role | CSS Value |
|------|-----------|
| Primary text | `rgb(5, 5, 5)` |
| Secondary text | `rgb(112, 112, 112)` |
| Muted text | `rgb(160, 160, 160)` |
| Text on dark | `rgb(244, 241, 237)` |

### State Colors (agents & simulation)
| State | Value | Usage |
|-------|-------|-------|
| IDLE | `rgb(160, 160, 160)` | Agent waiting |
| MOVING | `rgb(45, 160, 0)` | Agent moving |
| DELAYED | `rgb(230, 140, 0)` | Agent delayed (fault) |
| FAULT | `rgb(230, 44, 2)` | Agent broken |
| CRITICAL | `rgb(143, 58, 222)` | Agent critical |
| SUCCESS | `rgb(45, 160, 0)` | Simulation complete |

State colors are the **only saturated color** in the UI — they must pop on the cream background.

## Typography

| Font | Usage |
|------|-------|
| **Playfair Display** | Section titles, scenario names |
| **DM Mono** | Buttons, labels, ALL numeric values, badges |
| **Inter** | Body text, descriptions, stats |

### Type Scale
| Element | Font | Size | Weight | Notes |
|---------|------|------|--------|-------|
| Scenario title | Playfair | 28px | 400 | letter-spacing -0.5px |
| Panel title | DM Mono | 11px | 500 | UPPERCASE, ls 0.8px |
| Metric value | DM Mono | 32px | 400 | Real-time numbers |
| Metric label | Inter | 12px | 400 | Muted, below value |
| Body text | Inter | 14px | 300 | line-height 1.6 |
| Button | DM Mono | 13px | 500 | UPPERCASE, ls 0.4px |
| Badge | DM Mono | 11px | 500 | UPPERCASE |

### Typography Rules
1. Panel titles in monospace UPPERCASE — clear semantic separation
2. ALL numeric values in DM Mono — instrument consistency
3. Slight negative letter-spacing on serif titles (-0.5 to -1px)
4. Positive letter-spacing on all UPPERCASE elements (+0.4 to +0.8px)

## Layout

| Element | Size |
|---------|------|
| Left panel | 280px fixed |
| Right panel | 260px fixed |
| Bottom toolbar | 56px fixed |
| Header bar | 44px fixed |
| Bevy canvas | `calc(100vw - 540px)` |

Internal spacing: panel padding 16px, section gap 24px, label-value gap 4px, card padding 12px 16px, button padding 8px 16px.

## Component Patterns

### Primary Button
```css
.btn-primary {
  background: rgb(5, 5, 5);
  border: none;
  border-radius: 0px;          /* Square — style signature */
  padding: 10px 20px;
  font-family: "DM Mono", monospace;
  font-size: 13px;
  font-weight: 500;
  letter-spacing: 0.4px;
  text-transform: uppercase;
  color: rgb(244, 241, 237);
  cursor: pointer;
  transition: opacity 150ms ease;
}
.btn-primary:hover { opacity: 0.85; }
```

### Metric Card
```css
.metric-card {
  background: rgb(247, 244, 240);
  border: 1px solid rgba(5, 5, 5, 0.06);
  border-radius: 0px;
  padding: 12px 16px;
}
```

### Input / Slider
```css
.input-field {
  background: rgb(240, 237, 233);
  border: 1px solid rgba(5, 5, 5, 0.1);
  border-radius: 0px;
  padding: 7px 10px;
  font-family: "DM Mono", monospace;
  font-size: 13px;
}
```

## Hard Rules (never violate)

1. **`border-radius: 0px`** on all buttons, inputs, badges, cards
2. **Never pure white** — always cream/sand backgrounds
3. **All numeric values in DM Mono**
4. **UI labels always UPPERCASE + positive letter-spacing**
5. **Saturated colors only for agent states**
6. **No HTML overlays on the Bevy canvas** — panels are always lateral
7. **No box-shadow** — subtle borders only
8. **No decorative animations** — only state-change transitions

## Anti-Patterns

| Wrong | Right |
|-------|-------|
| `border-radius: 8px` | `border-radius: 0px` |
| `#ffffff` background | `rgb(246,244,240)` |
| Box shadows | `rgba(5,5,5,0.08)` borders |
| Accent colors everywhere | Colors only on agent states |
| Inter for numbers | DM Mono for all values |
| Overlay on canvas | Lateral panels only |
| 100px+ spacing | 16-24px functional spacing |
| Glassmorphism / blur | Opaque, crisp surfaces |
