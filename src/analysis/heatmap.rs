//! Texture-based heatmap overlay.
//!
//! Data accumulation (density/traffic) runs per-tick in FixedUpdate.
//! Rendering uses a single textured quad — 1 entity, 1 draw call.
//! The CPU writes pixel colors into a Bevy `Image` which the GPU samples.
//!
//! Density and traffic data use flat `Vec` arrays indexed by `y * width + x`
//! for cache-friendly O(1) access (no HashMap hashing overhead).

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::analysis::dependency::ActionDependencyGraph;
use crate::analysis::history::TickHistory;
use crate::core::agent::LogicalAgent;
use crate::core::grid::GridMap;
use crate::fault::breakdown::Dead;

// ── Constants ───────────────────────────────────────────────────────

const DENSITY_DECAY: f32 = 0.85;
const DENSITY_EPSILON: f32 = 0.005;

// ── Mode enum ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeatmapMode {
    #[default]
    Density,
    Traffic,
    Criticality,
}

// ── Resources ───────────────────────────────────────────────────────

/// Minimum raw density to show a pixel (filters out single-robot cells).
const DENSITY_MIN_THRESHOLD: f32 = 1.8;

/// Data-only heatmap state. Flat arrays indexed by `y * grid_width + x`.
#[derive(Resource, Debug)]
pub struct HeatmapState {
    pub mode: HeatmapMode,
    pub density_radius: i32,
    /// Flat density grid — length = grid_width * grid_height.
    pub density: Vec<f32>,
    pub max_density: f32,
    /// Flat traffic grid — length = grid_width * grid_height.
    pub traffic: Vec<u32>,
    pub max_traffic: u32,
    /// Flat criticality grid (in-degree + optional betweenness blend).
    pub criticality: Vec<f32>,
    pub max_criticality: f32,
    pub dirty: bool,
    /// Cached grid dimensions for flat indexing.
    pub grid_w: i32,
    pub grid_h: i32,
}

impl Default for HeatmapState {
    fn default() -> Self {
        Self {
            mode: HeatmapMode::default(),
            density_radius: 2,
            density: Vec::new(),
            max_density: 0.0,
            traffic: Vec::new(),
            max_traffic: 0,
            criticality: Vec::new(),
            max_criticality: 0.0,
            dirty: false,
            grid_w: 0,
            grid_h: 0,
        }
    }
}

impl HeatmapState {
    pub fn clear(&mut self) {
        self.density.clear();
        self.max_density = 0.0;
        self.traffic.clear();
        self.max_traffic = 0;
        self.criticality.clear();
        self.max_criticality = 0.0;
        self.dirty = false;
        self.grid_w = 0;
        self.grid_h = 0;
    }

    /// Ensure flat arrays match grid dimensions. Resets data on resize.
    fn ensure_size(&mut self, w: i32, h: i32) {
        if self.grid_w != w || self.grid_h != h {
            let n = (w * h) as usize;
            self.density.clear();
            self.density.resize(n, 0.0);
            self.traffic.clear();
            self.traffic.resize(n, 0);
            self.criticality.clear();
            self.criticality.resize(n, 0.0);
            self.max_density = 0.0;
            self.max_traffic = 0;
            self.max_criticality = 0.0;
            self.grid_w = w;
            self.grid_h = h;
        }
    }

    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.grid_w + x) as usize
    }
}

/// Marker for the single heatmap quad entity.
#[derive(Component)]
pub struct HeatmapQuad;

/// Handle to the heatmap texture image and material.
#[derive(Resource)]
pub struct HeatmapTexture {
    pub image_handle: Handle<Image>,
    pub material_handle: Handle<StandardMaterial>,
    pub width: u32,
    pub height: u32,
}

// Keep HeatmapTilePool as an empty resource for cleanup compatibility
#[derive(Resource, Default)]
pub struct HeatmapTilePool;

impl HeatmapTilePool {
    pub fn clear(&mut self) {}
    pub fn has_active(&self) -> bool {
        false
    }
}

// ── Setup (Startup) ─────────────────────────────────────────────────

pub fn setup_heatmap_palette(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Create a 1x1 placeholder texture — resized to grid dims on OnEnter(Running)
    let mut image = Image::new(
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0u8; 4],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::nearest();

    let image_handle = images.add(image);

    let material_handle = materials.add(StandardMaterial {
        base_color_texture: Some(image_handle.clone()),
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.insert_resource(HeatmapTexture { image_handle, material_handle, width: 0, height: 0 });
}

// ── Early resize (Loading) ──────────────────────────────────────────

/// Resize the heatmap texture to match grid dimensions on `OnEnter(Running)`.
pub fn resize_heatmap_texture(
    mut images: ResMut<Assets<Image>>,
    mut heatmap_tex: ResMut<HeatmapTexture>,
    grid: Res<GridMap>,
) {
    let w = grid.width as u32;
    let h = grid.height as u32;
    if w == 0 || h == 0 || (heatmap_tex.width == w && heatmap_tex.height == h) {
        return;
    }
    if let Some(image) = images.get_mut(&heatmap_tex.image_handle) {
        *image = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0u8; (w * h * 4) as usize],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
    }
    heatmap_tex.width = w;
    heatmap_tex.height = h;
}

// ── Accumulation (FixedUpdate) ──────────────────────────────────────

pub fn accumulate_heatmap_density(
    agents: Query<&LogicalAgent, Without<Dead>>,
    mut heatmap: ResMut<HeatmapState>,
    grid: Res<GridMap>,
) {
    heatmap.ensure_size(grid.width, grid.height);

    // Decay pass — vectorizable flat loop
    let mut max_d: f32 = 0.0;
    for v in heatmap.density.iter_mut() {
        *v *= DENSITY_DECAY;
        if *v < DENSITY_EPSILON {
            *v = 0.0;
        } else if *v > max_d {
            max_d = *v;
        }
    }

    let radius = heatmap.density_radius.max(1);
    let w = heatmap.grid_w;
    let h = heatmap.grid_h;
    for agent in &agents {
        let pos = agent.current_pos;
        for dx in -radius..=radius {
            let cx = pos.x + dx;
            if cx < 0 || cx >= w {
                continue;
            }
            for dy in -radius..=radius {
                let cy = pos.y + dy;
                if cy < 0 || cy >= h {
                    continue;
                }
                let dist = (dx.abs() + dy.abs()) as f32;
                let weight = 1.0 / (1.0 + dist * 0.5);
                let idx = (cy * w + cx) as usize;
                heatmap.density[idx] += weight;
                if heatmap.density[idx] > max_d {
                    max_d = heatmap.density[idx];
                }
            }
        }
    }

    heatmap.max_density = max_d.max(0.01);

    if heatmap.mode == HeatmapMode::Density {
        heatmap.dirty = true;
    }
}

pub fn accumulate_heatmap_traffic(
    agents: Query<&LogicalAgent, Without<Dead>>,
    mut heatmap: ResMut<HeatmapState>,
    grid: Res<GridMap>,
) {
    heatmap.ensure_size(grid.width, grid.height);

    let w = heatmap.grid_w;
    for agent in &agents {
        let p = agent.current_pos;
        let idx = (p.y * w + p.x) as usize;
        heatmap.traffic[idx] += 1;
        if heatmap.traffic[idx] > heatmap.max_traffic {
            heatmap.max_traffic = heatmap.traffic[idx];
        }
    }

    if heatmap.mode == HeatmapMode::Traffic {
        heatmap.dirty = true;
    }
}

// ── Criticality accumulation (FixedUpdate) ──────────────────────────

/// Compute per-cell criticality from ADG in-degree + optional betweenness blend.
pub fn accumulate_heatmap_criticality(
    agents: Query<(Entity, &LogicalAgent), Without<Dead>>,
    adg: Res<ActionDependencyGraph>,
    mut heatmap: ResMut<HeatmapState>,
    grid: Res<GridMap>,
    betweenness: Option<Res<super::dependency::BetweennessCriticality>>,
) {
    heatmap.ensure_size(grid.width, grid.height);

    let w = heatmap.grid_w;
    let n = heatmap.criticality.len();

    // Zero out criticality each tick (instantaneous snapshot, not cumulative)
    for v in heatmap.criticality.iter_mut() {
        *v = 0.0;
    }

    let has_betweenness = betweenness.as_ref().is_some_and(|b| !b.scores.is_empty());

    // Compute per-entity in-degree from ADG dependents
    let mut max_c: f32 = 0.0;
    for (entity, agent) in &agents {
        let in_degree = adg.dependents.get(&entity).map_or(0, |v| v.len()) as f32;

        let score = if has_betweenness {
            let bw = betweenness.as_ref().unwrap().scores.get(&entity).copied().unwrap_or(0.0);
            in_degree * 0.4 + bw * 0.6
        } else {
            in_degree
        };

        if score > 0.0 {
            let p = agent.current_pos;
            let idx = (p.y * w + p.x) as usize;
            if idx < n {
                heatmap.criticality[idx] += score;
                if heatmap.criticality[idx] > max_c {
                    max_c = heatmap.criticality[idx];
                }
            }
        }
    }

    heatmap.max_criticality = max_c.max(0.01);
    if heatmap.mode == HeatmapMode::Criticality {
        heatmap.dirty = true;
    }
}

/// Recompute heatmap density from replay snapshot positions instead of live agents.
pub fn replay_heatmap_density(
    history: Res<TickHistory>,
    mut heatmap: ResMut<HeatmapState>,
    grid: Res<GridMap>,
) {
    let cursor = match history.replay_cursor {
        Some(c) => c,
        None => return,
    };
    let snapshot = match history.snapshots.get(cursor) {
        Some(s) => s,
        None => return,
    };

    heatmap.ensure_size(grid.width, grid.height);

    // Reset density — replay shows instantaneous density, not accumulated
    let mut max_d: f32 = 0.0;
    for v in heatmap.density.iter_mut() {
        *v = 0.0;
    }

    let radius = heatmap.density_radius.max(1);
    let w = heatmap.grid_w;
    let h = heatmap.grid_h;
    for snap in &snapshot.agents {
        let pos = snap.pos;
        for dx in -radius..=radius {
            let cx = pos.x + dx;
            if cx < 0 || cx >= w {
                continue;
            }
            for dy in -radius..=radius {
                let cy = pos.y + dy;
                if cy < 0 || cy >= h {
                    continue;
                }
                let dist = (dx.abs() + dy.abs()) as f32;
                let weight = 1.0 / (1.0 + dist * 0.5);
                let idx = (cy * w + cx) as usize;
                heatmap.density[idx] += weight;
                if heatmap.density[idx] > max_d {
                    max_d = heatmap.density[idx];
                }
            }
        }
    }

    heatmap.max_density = max_d.max(0.01);
    if heatmap.mode == HeatmapMode::Density {
        heatmap.dirty = true;
    }
}

// ── Visual update (Update) ──────────────────────────────────────────

/// Write heatmap data into a **new** texture image + manage quad entity.
///
/// On WebGL2 / Bevy 0.18, only `images.add()` (AssetEvent::Added) triggers
/// GPU texture upload. `get_mut()` and `insert()` do not work.
///
/// Uses `Local<Vec<u8>>` to reuse the pixel buffer across frames.
pub fn update_heatmap_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut heatmap_tex: ResMut<HeatmapTexture>,
    mut heatmap: ResMut<HeatmapState>,
    grid: Res<GridMap>,
    existing_quad: Query<Entity, With<HeatmapQuad>>,
    mut pixel_buf: Local<Vec<u8>>,
) {
    if !heatmap.dirty {
        return;
    }
    heatmap.dirty = false;

    let w = grid.width as u32;
    let h = grid.height as u32;
    if w == 0 || h == 0 {
        return;
    }

    heatmap_tex.width = w;
    heatmap_tex.height = h;

    // Reuse pixel buffer across frames — only reallocate on grid resize
    let needed = (w * h * 4) as usize;
    pixel_buf.resize(needed, 0);

    // Fill with fully transparent background — grid/obstacles show through
    pixel_buf.fill(0);

    match heatmap.mode {
        HeatmapMode::Density => {
            let max = heatmap.max_density.max(1.0);
            let inv_range = 1.0 / (max - DENSITY_MIN_THRESHOLD).max(0.001);
            let density = &heatmap.density;
            for (i, &val) in density.iter().enumerate() {
                if val < DENSITY_MIN_THRESHOLD {
                    continue;
                }
                let t = ((val - DENSITY_MIN_THRESHOLD) * inv_range).min(1.0);
                let [r, g, b] = density_color_u8(t);
                let px = i * 4;
                pixel_buf[px] = r;
                pixel_buf[px + 1] = g;
                pixel_buf[px + 2] = b;
                pixel_buf[px + 3] = 180;
            }
        }
        HeatmapMode::Traffic => {
            let max = (heatmap.max_traffic as f32).max(10.0);
            let inv_max = 1.0 / max;
            let traffic = &heatmap.traffic;
            for (i, &val) in traffic.iter().enumerate() {
                if val == 0 {
                    continue;
                }
                let t = (val as f32 * inv_max).min(1.0);
                let [r, g, b] = traffic_color_u8(t);
                let px = i * 4;
                pixel_buf[px] = r;
                pixel_buf[px + 1] = g;
                pixel_buf[px + 2] = b;
                pixel_buf[px + 3] = 180;
            }
        }
        HeatmapMode::Criticality => {
            let max = heatmap.max_criticality.max(1.0);
            let inv_max = 1.0 / max;
            let crit = &heatmap.criticality;
            for (i, &val) in crit.iter().enumerate() {
                if val <= 0.0 {
                    continue;
                }
                let t = (val * inv_max).min(1.0);
                let [r, g, b] = criticality_color_u8(t);
                let px = i * 4;
                pixel_buf[px] = r;
                pixel_buf[px + 1] = g;
                pixel_buf[px + 2] = b;
                pixel_buf[px + 3] = 180;
            }
        }
    }

    // Create a brand-new Image asset (emits AssetEvent::Added → GPU processes it).
    // Move pixel_buf's allocation into Image instead of cloning — eliminates
    // a 64 KB alloc+copy per dirty frame. pixel_buf is re-initialized with
    // capacity for the next frame via resize() at line 407.
    let pixel_data = std::mem::take(&mut *pixel_buf);
    let mut new_image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        pixel_data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    new_image.sampler = ImageSampler::nearest();
    let new_handle = images.add(new_image);

    // Swap the material's texture reference to point at the new image
    if let Some(mat) = materials.get_mut(&heatmap_tex.material_handle) {
        mat.base_color_texture = Some(new_handle.clone());
    }

    // Remove the old image to avoid leaking GPU memory
    images.remove(&heatmap_tex.image_handle);
    heatmap_tex.image_handle = new_handle;

    // Ensure quad entity exists and is visible
    if existing_quad.is_empty() {
        let cx = (w as f32 - 1.0) * 0.5;
        let cz = (h as f32 - 1.0) * 0.5;
        let mesh = meshes.add(Plane3d::new(Vec3::Y, Vec2::new(w as f32 * 0.5, h as f32 * 0.5)));
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(heatmap_tex.material_handle.clone()),
            Transform::from_xyz(cx, 0.10, cz),
            HeatmapQuad,
        ));
    } else {
        // Restore visibility (may have been hidden by hide_heatmap_tiles)
        for entity in &existing_quad {
            commands.entity(entity).insert(Visibility::Inherited);
        }
    }
}

/// Hide heatmap quad when toggled off.
pub fn hide_heatmap_tiles(mut commands: Commands, quads: Query<Entity, With<HeatmapQuad>>) {
    for entity in &quads {
        commands.entity(entity).insert(Visibility::Hidden);
    }
}

/// Despawn heatmap quad on reset + reset texture dimensions.
pub fn despawn_heatmap_tiles(
    mut commands: Commands,
    quads: Query<Entity, With<HeatmapQuad>>,
    heatmap_tex: Option<ResMut<HeatmapTexture>>,
) {
    for entity in &quads {
        commands.entity(entity).despawn();
    }
    if let Some(mut tex) = heatmap_tex {
        tex.width = 0;
        tex.height = 0;
    }
}

// ── Color functions (direct u8 output — no Color roundtrip) ────────

/// Vivid gradient: pale orange (#FFB080) → vivid red (#DC1414).
#[inline]
fn density_color_u8(t: f32) -> [u8; 3] {
    [
        (255.0 - t * 35.0) as u8,  // 255 → 220
        (176.0 - t * 156.0) as u8, // 176 → 20
        (128.0 - t * 108.0) as u8, // 128 → 20
    ]
}

/// Clear blue gradient: sky blue (#7EC8E3) → medium (#2D6FBF) → deep blue (#0A2472).
#[inline]
fn traffic_color_u8(t: f32) -> [u8; 3] {
    if t < 0.5 {
        // Sky blue → medium blue
        let u = t * 2.0;
        [
            (126.0 - u * 81.0) as u8, // 126 → 45
            (200.0 - u * 89.0) as u8, // 200 → 111
            (227.0 - u * 36.0) as u8, // 227 → 191
        ]
    } else {
        // Medium blue → deep blue
        let u = (t - 0.5) * 2.0;
        [
            (45.0 - u * 35.0) as u8,  // 45 → 10
            (111.0 - u * 75.0) as u8, // 111 → 36
            (191.0 - u * 77.0) as u8, // 191 → 114
        ]
    }
}

/// Red gradient: light red (#FF6B6B) → vivid red (#DC2020) → deep crimson (#8B0000).
#[inline]
fn criticality_color_u8(t: f32) -> [u8; 3] {
    if t < 0.5 {
        // light red (0xFF, 0x6B, 0x6B) → vivid red (0xDC, 0x20, 0x20)
        let u = t * 2.0;
        [
            (255.0 - u * 35.0) as u8, // 255 → 220
            (107.0 - u * 75.0) as u8, // 107 → 32
            (107.0 - u * 75.0) as u8, // 107 → 32
        ]
    } else {
        // vivid red (0xDC, 0x20, 0x20) → deep crimson (0x8B, 0x00, 0x00)
        let u = (t - 0.5) * 2.0;
        [
            (220.0 - u * 81.0) as u8, // 220 → 139
            (32.0 - u * 32.0) as u8,  // 32 → 0
            (32.0 - u * 32.0) as u8,  // 32 → 0
        ]
    }
}

// Keep Color-returning versions for tests that validate sRGB ranges
#[cfg(test)]
fn density_color(t: f32) -> Color {
    let [r, g, b] = density_color_u8(t);
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

#[cfg(test)]
fn traffic_color(t: f32) -> Color {
    let [r, g, b] = traffic_color_u8(t);
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

#[cfg(test)]
fn criticality_color(t: f32) -> Color {
    let [r, g, b] = criticality_color_u8(t);
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── State lifecycle ──────────────────────────────────────────────

    #[test]
    fn clear_resets_data_but_preserves_mode_and_radius() {
        let mut state =
            HeatmapState { mode: HeatmapMode::Traffic, density_radius: 3, ..Default::default() };
        state.ensure_size(8, 8);
        let idx_1_1 = state.idx(1, 1);
        let idx_2_2 = state.idx(2, 2);
        state.density[idx_1_1] = 2.5;
        state.max_density = 5.0;
        state.traffic[idx_2_2] = 10;
        state.max_traffic = 10;
        state.criticality[0] = 3.0;
        state.max_criticality = 3.0;
        state.dirty = true;

        state.clear();

        assert!(state.density.is_empty());
        assert_eq!(state.max_density, 0.0);
        assert!(state.traffic.is_empty());
        assert_eq!(state.max_traffic, 0);
        assert!(state.criticality.is_empty());
        assert_eq!(state.max_criticality, 0.0);
        assert!(!state.dirty);
        // User settings preserved
        assert_eq!(state.mode, HeatmapMode::Traffic);
        assert_eq!(state.density_radius, 3);
    }

    // ── Flat buffer allocation ───────────────────────────────────────

    #[test]
    fn ensure_size_allocates_all_buffers() {
        let mut state = HeatmapState::default();
        state.ensure_size(16, 16);
        assert_eq!(state.density.len(), 256);
        assert_eq!(state.traffic.len(), 256);
        assert_eq!(state.criticality.len(), 256);
        assert_eq!(state.grid_w, 16);
        assert_eq!(state.grid_h, 16);
    }

    #[test]
    fn resize_resets_data() {
        let mut state = HeatmapState::default();
        state.ensure_size(8, 8);
        state.density[0] = 5.0;
        state.traffic[0] = 10;
        state.ensure_size(16, 16);
        assert_eq!(state.density[0], 0.0);
        assert_eq!(state.traffic[0], 0);
    }

    #[test]
    fn same_size_preserves_data() {
        let mut state = HeatmapState::default();
        state.ensure_size(8, 8);
        state.density[0] = 5.0;
        state.ensure_size(8, 8);
        assert_eq!(state.density[0], 5.0);
    }

    // ── Color gradient sRGB bounds ───────────────────────────────────

    #[test]
    fn density_color_stays_in_srgb_range() {
        for &t in &[0.0f32, 0.1, 0.2, 0.33, 0.5, 0.66, 0.75, 0.9, 1.0] {
            let [r, g, b, _] = density_color(t).to_srgba().to_f32_array();
            assert!((0.0..=1.0).contains(&r), "density_color({t}): R={r}");
            assert!((0.0..=1.0).contains(&g), "density_color({t}): G={g}");
            assert!((0.0..=1.0).contains(&b), "density_color({t}): B={b}");
        }
    }

    #[test]
    fn traffic_color_stays_in_srgb_range() {
        for &t in &[0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let [r, g, b, _] = traffic_color(t).to_srgba().to_f32_array();
            assert!((0.0..=1.0).contains(&r), "traffic_color({t}): R={r}");
            assert!((0.0..=1.0).contains(&g), "traffic_color({t}): G={g}");
            assert!((0.0..=1.0).contains(&b), "traffic_color({t}): B={b}");
        }
    }

    #[test]
    fn criticality_color_stays_in_srgb_range() {
        for &t in &[0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let [r, g, b, _] = criticality_color(t).to_srgba().to_f32_array();
            assert!((0.0..=1.0).contains(&r), "criticality_color({t}): R={r}");
            assert!((0.0..=1.0).contains(&g), "criticality_color({t}): G={g}");
            assert!((0.0..=1.0).contains(&b), "criticality_color({t}): B={b}");
        }
    }

    // ── Color gradient endpoints (visual correctness) ────────────────

    #[test]
    fn density_gradient_pale_orange_to_vivid_red() {
        let [r0, g0, b0, _] = density_color(0.0).to_srgba().to_f32_array();
        assert!(r0 > 0.9 && g0 > 0.5 && b0 > 0.3, "t=0 should be pale orange");

        let [r1, g1, b1, _] = density_color(1.0).to_srgba().to_f32_array();
        assert!(r1 > 0.8 && g1 < 0.15 && b1 < 0.15, "t=1 should be vivid red");
    }

    #[test]
    fn traffic_gradient_sky_blue_to_deep_blue() {
        let [r0, _, b0, _] = traffic_color(0.0).to_srgba().to_f32_array();
        assert!(b0 > r0 && b0 > 0.7, "t=0 should be sky blue");

        let [r1, g1, b1, _] = traffic_color(1.0).to_srgba().to_f32_array();
        assert!(r1 < 0.15 && g1 < 0.2 && b1 > 0.3, "t=1 should be deep blue");
    }

    #[test]
    fn criticality_gradient_light_red_to_deep_crimson() {
        let [r0, g0, b0, _] = criticality_color(0.0).to_srgba().to_f32_array();
        assert!(r0 > 0.9 && g0 > 0.3 && b0 > 0.3, "t=0 should be light red");

        let [r1, g1, b1, _] = criticality_color(1.0).to_srgba().to_f32_array();
        assert!(r1 > 0.4 && g1 < 0.05 && b1 < 0.05, "t=1 should be deep crimson");
    }
}
