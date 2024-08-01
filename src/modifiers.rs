use bevy::{log::info, math::{IVec2, Vec2, Vec3, Vec3Swizzles}, prelude::{Bundle, Changed, Component, CubicCurve, Entity, GlobalTransform, Or, Query, ReflectComponent, Res, ResMut, Resource, TransformBundle}, reflect::Reflect, utils::{HashMap, HashSet}};

use crate::{DirtyTiles, MaximumSplineSimplificationDistance, TerrainSettings};

#[derive(Bundle)]
pub struct ShapeModifierBundle {
    pub aabb: TerrainTileAabb,
    pub modifier: ShapeModifier,
    pub priority: ModifierPriority,
    pub transform_bundle: TransformBundle
}

#[derive(Reflect)]
pub enum Shape {
    Circle {
        radius: f32,
    },
    Rectangle {
        x: f32,
        z: f32, 
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ShapeModifier {
    pub shape: Shape,
    pub falloff: f32
}

#[derive(Bundle)]
pub struct TerrainSplineBundle {
    pub aabb: TerrainAabb,
    pub tile_aabb: TerrainTileAabb,
    pub spline: TerrainSpline,
    pub properties: TerrainSplineProperties,
    pub spline_cached: TerrainSplineCached,
    pub priority: ModifierPriority,
    pub transform_bundle: TransformBundle
}
#[derive(Component, Reflect, Default, PartialEq, Eq, PartialOrd, Ord)]
#[reflect(Component)]
pub struct ModifierPriority(pub u32);

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct TerrainAabb {
    pub(super) min: Vec2,
    pub(super) max: Vec2
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct TerrainTileAabb {
    pub(super) min: IVec2,
    pub(super) max: IVec2
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TerrainSpline {
    pub curve: CubicCurve<Vec3>,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct TerrainSplineProperties {
    pub width: f32,
    pub falloff: f32
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub(super) struct TerrainSplineCached {
    pub(super) points: Vec<Vec3>,
}

pub(super) struct TileSplineEntry {
    pub(super) entity: Entity,
    pub(super) overlap_bits: u64
}

#[derive(Resource, Default)]
pub struct TileToModifierMapping {
    /// Basically the u64 acts as a bitmask which tells us where in the tile we have an effect. This allows us to skip checking modifiers for height map entries that don't interact.
    pub(super) shape: HashMap<IVec2, Vec<Entity>>,
    pub(super) splines: HashMap<IVec2, Vec<TileSplineEntry>>,
}

pub(super) fn update_terrain_spline_cache(
    mut query: Query<(&mut TerrainSplineCached, &mut TerrainAabb, &TerrainSpline, &TerrainSplineProperties, &GlobalTransform), Or<(Changed<TerrainSpline>, Changed<GlobalTransform>)>>,
    spline_simplification_distance: Res<MaximumSplineSimplificationDistance>
) {
    query.par_iter_mut().for_each(|(mut spline_cached, mut terrain_aabb, spline, spline_properties, global_transform)| {
        spline_cached.points.clear();

        spline_cached.points.extend(spline.curve.iter_positions(80).map(|point| global_transform.transform_point(point)));

        // Filter points that are very close together.
        let dedup_distance = (spline_properties.width * spline_properties.width).min(spline_simplification_distance.0);

        spline_cached.points.dedup_by(|a, b| a.distance_squared(*b) < dedup_distance);

        let(min, max) = if spline_cached.points.is_empty() {
            (Vec2::ZERO, Vec2::ZERO)
        } else {
            let (min, max) = spline_cached.points.iter().fold((spline_cached.points[0].xz(), spline_cached.points[0].xz()), |(min, max), point| (
                min.min(point.xz()),
                max.max(point.xz())
            ));

            let total_width = spline_properties.falloff + spline_properties.width;

            (
                min - total_width,
                max + total_width
            )
        };

        terrain_aabb.min = min;
        terrain_aabb.max = max;
    });
}

pub(super) fn update_terrain_spline_aabb(
    mut query: Query<(Entity, &TerrainSplineCached, &TerrainSplineProperties, &mut TerrainTileAabb), (Changed<TerrainSplineCached>, Changed<TerrainSplineProperties>)>,
    terrain_settings: Res<TerrainSettings>,
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    mut dirty_priorities: ResMut<DirtyTiles>
) {
    let tile_size = terrain_settings.tile_size();

    query.iter_mut().for_each(|(entity, spline_cached, spline_properties, mut tile_aabb)| {
        for x in tile_aabb.min.x..=tile_aabb.max.x {
            for y in tile_aabb.min.y..=tile_aabb.max.y {
                if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(&IVec2::new(x, y)) {
                    if let Some(index) = entries.iter().position(|entry| entity == entry.entity) {
                        entries.swap_remove(index);
                    }
                }
            }
        }

        let(min, max) = if spline_cached.points.is_empty() {
            (IVec2::ZERO, IVec2::ZERO)
        } else {
            let (min, max) = spline_cached.points.iter().fold((spline_cached.points[0].xz(), spline_cached.points[0].xz()), |(min, max), point| (
                min.min(point.xz()),
                max.max(point.xz())
            ));

            let total_width = spline_properties.falloff + spline_properties.width;

            (
                (min - total_width).as_ivec2() >> terrain_settings.tile_size_power,
                (max + total_width).as_ivec2() >> terrain_settings.tile_size_power
            )
        };

        for x in min.x..=max.x {
            for y in min.y..=max.y {
                let tile = IVec2::new(x, y);
                let tile_world = (tile << terrain_settings.tile_size_power).as_vec2();

                let mut overlap_bits = 0;
                
                for (a, b) in spline_cached.points.iter().zip(spline_cached.points.iter().skip(1)) {
                    let a_2d = a.xz() - tile_world;
                    let b_2d = b.xz() - tile_world;

                    let min = a_2d.min(b_2d) - spline_properties.width - spline_properties.falloff;
                    let max = a_2d.max(b_2d) + spline_properties.width + spline_properties.falloff;

                    let min_scaled = ((min / tile_size) * 8.0).as_ivec2();
                    let max_scaled = ((max / tile_size) * 8.0).as_ivec2();

                    if min_scaled.x < 8 && min_scaled.y < 8 && max_scaled.x >= 0 && max_scaled.y >= 0 {
                        
                        for y in min_scaled.y.max(0)..=max_scaled.y.min(8) {
                            let i = y * 8;
                            for x in min_scaled.x.max(0)..=max_scaled.x.min(8) {
                                let bit = i + x;

                                overlap_bits |= 1 << bit;
                            }
                        }
                    }
                }

                let entry = TileSplineEntry {
                    entity,
                    overlap_bits,
                };

                if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(&tile) {
                    entries.push(entry);
                } else {
                    tile_to_modifier_mapping.splines.insert(tile, vec![entry]);
                }

                dirty_priorities.0.insert(tile);
            }
        }

        tile_aabb.min = min;
        tile_aabb.max = max;        
    });
}

pub(super) fn update_shape_modifier_aabb(
    mut query: Query<(Entity, &ShapeModifier, &mut TerrainTileAabb, &GlobalTransform), Or<(Changed<ShapeModifier>, Changed<GlobalTransform>)>>,
    terrain_settings: Res<TerrainSettings>,
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    mut dirty_tiles: ResMut<DirtyTiles>
) {
    query.iter_mut().for_each(|(entity, shape, mut tile_aabb, global_transform)| {
        for x in tile_aabb.min.x..=tile_aabb.max.x {
            for y in tile_aabb.min.y..=tile_aabb.max.y {
                let tile = IVec2::new(x, y);
                
                if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(&tile) {
                    if let Some(index) = entries.iter().position(|existing_entity| entity == *existing_entity) {
                        entries.swap_remove(index);

                        dirty_tiles.0.insert(tile);
                    }
                }
            }
        }
        
        let(min, max) = match shape.shape {
            Shape::Circle { radius } => {
                (
                    Vec2::splat(-radius),
                    Vec2::splat(radius)
                )
            },
            Shape::Rectangle { x, z } => {
                (
                    Vec2::new(-x, -z) / 2.0,
                    Vec2::new(x, z) / 2.0
                )
            },
        };

        let tile_min = (global_transform.translation().xz() + min - shape.falloff).as_ivec2() >> terrain_settings.tile_size_power;
        let tile_max = (global_transform.translation().xz() + max + shape.falloff).as_ivec2() >> terrain_settings.tile_size_power;

        for x in tile_min.x..=tile_max.x {
            for y in tile_min.y..=tile_max.y {
                let tile = IVec2::new(x, y);
                
                if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(&tile) {
                    entries.push(entity);
                } else {
                    tile_to_modifier_mapping.shape.insert(tile, vec![entity]);
                }

                dirty_tiles.0.insert(tile);
            }
        }

        tile_aabb.min = tile_min;
        tile_aabb.max = tile_max;
    });
}

pub(super) fn update_tile_modifier_priorities(
    mut tile_to_modifier_mapping: ResMut<TileToModifierMapping>,
    dirty_tiles: Res<DirtyTiles>,
    priority_query: Query<&ModifierPriority>,
) {
    for tile in dirty_tiles.0.iter() {
        if let Some(entries) = tile_to_modifier_mapping.shape.get_mut(tile) {
            entries.sort_unstable_by(|a, b| priority_query.get(*a).ok().cmp(&priority_query.get(*b).ok()));
        }

        if let Some(entries) = tile_to_modifier_mapping.splines.get_mut(tile) {
            entries.sort_unstable_by(|a, b| priority_query.get(a.entity).ok().cmp(&priority_query.get(b.entity).ok()));
        }
    }
}