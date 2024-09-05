use bevy::{
    math::{IVec2, Vec2, Vec3, Vec3Swizzles},
    prelude::{
        Bundle, Changed, Component, CubicCurve, Entity, EventReader, EventWriter, GlobalTransform,
        Or, Query, ReflectComponent, Res, ResMut, Resource, TransformBundle,
    },
    reflect::Reflect,
    utils::HashMap,
};

use crate::{RebuildTile, TerrainNoiseLayer, TerrainSettings};

#[derive(Bundle)]
pub struct ShapeModifierBundle {
    pub aabb: TerrainTileAabb,
    pub modifier: ShapeModifier,
    pub properties: ModifierProperties,
    pub priority: ModifierPriority,
    pub transform_bundle: TransformBundle,
}

#[derive(Reflect)]
pub enum Shape {
    Circle { radius: f32 },
    // Half-size.
    Rectangle { x: f32, z: f32 },
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ShapeModifier {
    pub shape: Shape,
    pub falloff: f32,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ModifierProperties {
    // TODO: These should be bitflags. They are only bools for testing in editor.
    pub allow_raising: bool,
    pub allow_lowering: bool,
}
impl Default for ModifierProperties {
    fn default() -> Self {
        Self { allow_raising: true, allow_lowering: true }
    }
}

/// Defines to operation the modifier applies to terrain.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub enum ModifierOperation {
    /// Set the height within the modifier's bounds equal to the modifiers global Y coordinate
    #[default]
    Set,
    /// Change the height within the modifier's bounds by the entered value.
    Change(f32),
    Step {
        step: f32,
        smoothing: f32,
    },
    Noise {
        noise: TerrainNoiseLayer,
    },
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct HolePunchModifier {
    /// When true, fill in holes instead of creating them.
    pub invert: bool
}

#[derive(Bundle)]
pub struct TerrainSplineBundle {
    pub tile_aabb: TerrainTileAabb,
    pub spline: TerrainSplineCurve,
    pub properties: TerrainSpline,
    pub spline_cached: TerrainSplineCached,
    pub priority: ModifierPriority,
    pub transform_bundle: TransformBundle,
}

/// Defines the order in which to apply the modifier where lower values are applied earlier.
#[derive(Component, Reflect, Default, PartialEq, Eq, PartialOrd, Ord)]
#[reflect(Component)]
pub struct ModifierPriority(pub i32);

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct TerrainTileAabb {
    pub(super) min: IVec2,
    pub(super) max: IVec2,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TerrainSplineCurve {
    pub curve: CubicCurve<Vec3>,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TerrainSpline {
    pub width: f32,
    pub falloff: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct TerrainSplineCached {
    pub(super) points: Vec<Vec3>,
}

pub(super) struct TileModifierEntry {
    pub(super) entity: Entity,
    /// Acts as a 8x8 map telling us where in the tile this modifier has an effect.
    /// 
    /// Allows us to skip checking modifiers for points that don't overlap, giving speed ups depending on how big the modifier is relative to the tile.
    pub(super) overlap_bits: u64,
}

#[derive(Resource, Default)]
pub struct TileToModifierMapping {
    pub(super) shape: HashMap<IVec2, Vec<TileModifierEntry>>,
    pub(super) splines: HashMap<IVec2, Vec<TileModifierEntry>>,
}

pub(super) fn update_terrain_spline_cache(
    mut query: Query<
        (
            &mut TerrainSplineCached,
            &TerrainSplineCurve,
            &TerrainSpline,
            &GlobalTransform,
        ),
        Or<(
            Changed<TerrainSpline>,
            Changed<TerrainSplineCurve>,
            Changed<GlobalTransform>,
        )>,
    >,
    terrain_settings: Res<TerrainSettings>,
) {
    query.par_iter_mut().for_each(
        |(mut spline_cached, spline, spline_properties, global_transform)| {
            spline_cached.points.clear();

            spline_cached.points.extend(
                spline
                    .curve
                    .iter_positions(80)
                    .map(|point| global_transform.transform_point(point)),
            );

            // Filter points that are very close together.
            let dedup_distance = (spline_properties.width * spline_properties.width)
                .min(terrain_settings.max_spline_simplification_distance);

            spline_cached
                .points
                .dedup_by(|a, b| a.distance_squared(*b) < dedup_distance);
        },
    );
}

pub(super) fn update_terrain_spline_aabb(
    mut query: Query<
        (
            Entity,
            &TerrainSplineCached,
            &TerrainSpline,
            &mut TerrainTileAabb,
        ),
        (
            Changed<TerrainSplineCached>,
            Changed<TerrainSpline>,
        ),
    >,
    terrain_settings: Res<TerrainSettings>,
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    mut rebuild_tiles_event: EventWriter<RebuildTile>,
) {
    let tile_size = terrain_settings.tile_size();

    query.iter_mut().for_each(
        |(entity, spline_cached, spline_properties, mut tile_aabb)| {
            for x in tile_aabb.min.x..=tile_aabb.max.x {
                for y in tile_aabb.min.y..=tile_aabb.max.y {
                    let tile = IVec2::new(x, y);
                    if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(&tile) {
                        if let Some(index) = entries.iter().position(|entry| entity == entry.entity)
                        {
                            entries.swap_remove(index);
                            rebuild_tiles_event.send(RebuildTile(tile));
                        }
                    }
                }
            }

            let (min, max) = if spline_cached.points.is_empty() {
                (IVec2::ZERO, IVec2::ZERO)
            } else {
                let (min, max) = spline_cached.points.iter().fold(
                    (spline_cached.points[0].xz(), spline_cached.points[0].xz()),
                    |(min, max), point| (min.min(point.xz()), max.max(point.xz())),
                );

                let total_width = spline_properties.falloff + spline_properties.width;

                (
                    (min - total_width).as_ivec2() >> terrain_settings.tile_size_power.get(),
                    (max + total_width).as_ivec2() >> terrain_settings.tile_size_power.get(),
                )
            };

            for x in min.x..=max.x {
                for y in min.y..=max.y {
                    let tile = IVec2::new(x, y);
                    let tile_world = (tile << terrain_settings.tile_size_power.get()).as_vec2();

                    let mut overlap_bits = 0;

                    for (a, b) in spline_cached
                        .points
                        .iter()
                        .zip(spline_cached.points.iter().skip(1))
                    {
                        let a_2d = a.xz() - tile_world;
                        let b_2d = b.xz() - tile_world;

                        let min =
                            a_2d.min(b_2d) - spline_properties.width - spline_properties.falloff.max(f32::EPSILON);
                        let max =
                            a_2d.max(b_2d) + spline_properties.width + spline_properties.falloff.max(f32::EPSILON);

                        let min_scaled = ((min / tile_size) * 7.0).as_ivec2();
                        let max_scaled = ((max / tile_size) * 7.0).as_ivec2();

                        if min_scaled.x < 8
                            && min_scaled.y < 8
                            && max_scaled.x >= 0
                            && max_scaled.y >= 0
                        {
                            for y in min_scaled.y.max(0)..=max_scaled.y.min(7) {
                                let i = y * 8;
                                for x in min_scaled.x.max(0)..=max_scaled.x.min(7) {
                                    let bit = i + x;

                                    overlap_bits |= 1 << bit;
                                }
                            }
                        }
                    }

                    if overlap_bits != 0 {
                        let entry = TileModifierEntry {
                            entity,
                            overlap_bits,
                        };

                        if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(&tile) {
                            entries.push(entry);
                        } else {
                            tile_to_modifier_mapping.splines.insert(tile, vec![entry]);
                        }

                        rebuild_tiles_event.send(RebuildTile(tile));
                    }
                }
            }

            tile_aabb.min = min;
            tile_aabb.max = max;
        },
    );
}

pub(super) fn update_shape_modifier_aabb(
    mut query: Query<
        (
            Entity,
            &ShapeModifier,
            &mut TerrainTileAabb,
            &GlobalTransform,
        ),
        Or<(
            Changed<ShapeModifier>,
            Changed<ModifierOperation>,
            Changed<GlobalTransform>,
        )>,
    >,
    terrain_settings: Res<TerrainSettings>,
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    mut rebuild_tiles_event: EventWriter<RebuildTile>,
) {
    let tile_size = terrain_settings.tile_size();

    query
        .iter_mut()
        .for_each(|(entity, shape, mut tile_aabb, global_transform)| {
            for x in tile_aabb.min.x..=tile_aabb.max.x {
                for y in tile_aabb.min.y..=tile_aabb.max.y {
                    let tile = IVec2::new(x, y);

                    if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(&tile) {
                        if let Some(index) = entries
                            .iter()
                            .position(|existing_entity| entity == existing_entity.entity)
                        {
                            entries.swap_remove(index);

                            rebuild_tiles_event.send(RebuildTile(tile));
                        }
                    }
                }
            }

            let (min, max) = match shape.shape {
                Shape::Circle { radius } => (
                    global_transform.translation().xz() + Vec2::splat(-radius),
                    global_transform.translation().xz() + Vec2::splat(radius),
                ),
                Shape::Rectangle { x, z } => {
                    let min = global_transform.transform_point(Vec3::new(-x, 0.0, -z));
                    let max = global_transform.transform_point(Vec3::new(x, 0.0, z));

                    (min.min(max).xz(), max.max(min).xz())
                }
            };

            let min = min - shape.falloff.max(f32::EPSILON);
            let max = max + shape.falloff.max(f32::EPSILON);

            let tile_min = min.as_ivec2() >> terrain_settings.tile_size_power.get();
            let tile_max = max.as_ivec2() >> terrain_settings.tile_size_power.get();

            for x in tile_min.x..=tile_max.x {
                for y in tile_min.y..=tile_max.y {
                    let tile = IVec2::new(x, y);
                    let tile_world = (tile << terrain_settings.tile_size_power.get()).as_vec2();

                    let mut overlap_bits = 0;

                    let min = min - tile_world;
                    let max = max - tile_world;

                    let min_scaled = ((min / tile_size) * 7.0).as_ivec2();
                    let max_scaled = ((max / tile_size) * 7.0).as_ivec2();

                    if min_scaled.x < 8
                        && min_scaled.y < 8
                        && max_scaled.x >= 0
                        && max_scaled.y >= 0
                    {
                        for y in min_scaled.y.max(0)..=max_scaled.y.min(7) {
                            let i = y * 8;
                            for x in min_scaled.x.max(0)..=max_scaled.x.min(7) {
                                let bit = i + x;

                                overlap_bits |= 1 << bit;
                            }
                        }
                    }

                    if overlap_bits != 0 {
                        let entry = TileModifierEntry {
                            entity,
                            overlap_bits,
                        };

                        if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(&tile) {
                            entries.push(entry);
                        } else {
                            tile_to_modifier_mapping.shape.insert(tile, vec![entry]);
                        }

                        rebuild_tiles_event.send(RebuildTile(tile));
                    }
                }
            }

            tile_aabb.min = tile_min;
            tile_aabb.max = tile_max;
        });
}

pub(super) fn update_tile_modifier_priorities(
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    mut event_reader: EventReader<RebuildTile>,
    priority_query: Query<&ModifierPriority>,
) {
    for RebuildTile(tile) in event_reader.read() {
        if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(tile) {
            entries.sort_unstable_by(|a, b| {
                priority_query
                    .get(a.entity)
                    .ok()
                    .cmp(&priority_query.get(b.entity).ok())
            });
        }

        if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(tile) {
            entries.sort_unstable_by(|a, b| {
                priority_query
                    .get(a.entity)
                    .ok()
                    .cmp(&priority_query.get(b.entity).ok())
            });
        }
    }
}
