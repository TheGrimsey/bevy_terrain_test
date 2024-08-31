use bevy::{
    math::{IVec2, Vec3Swizzles},
    prelude::{
        Changed, Commands, Component, DetectChanges, Entity, EventWriter, GlobalTransform, Query, ReflectComponent, Res, ResMut, Resource, With, Without
    },
    reflect::Reflect,
    utils::HashMap,
};
use fixedbitset::FixedBitSet;

use crate::{Heights, RebuildTile, TerrainSettings};

/// Bitset marking which points are holes.
/// Size should equal the amount of vertices in a terrain tile.
#[derive(Component, Debug)]
pub struct Holes(pub(super) FixedBitSet);

#[derive(Component, Reflect, Debug, Default)]
#[reflect(Component)]
pub struct Terrain(pub(super) IVec2);

/// Using a Vec<Entity> to prevent accidental overlaps from breaking the previous tile.
#[derive(Resource, Default)]
pub(super) struct TileToTerrain(pub(super) HashMap<IVec2, Vec<Entity>>);

pub(super) fn insert_components(
    mut commands: Commands,
    terrain_settings: Res<TerrainSettings>,
    query: Query<Entity, (With<Terrain>, Without<Heights>, Without<Holes>)>
) {
    let heights = terrain_settings.edge_points as usize * terrain_settings.edge_points as usize;

    query.iter().for_each(|entity| {
        commands.entity(entity).insert((
            Heights(vec![0.0; heights].into_boxed_slice()),
            Holes(FixedBitSet::with_capacity(heights))
        ));
    });
}

pub(super) fn update_tiling(
    mut tile_to_terrain: ResMut<TileToTerrain>,
    mut rebuild_tiles_event: EventWriter<RebuildTile>,
    mut query: Query<(Entity, &mut Terrain, &GlobalTransform), Changed<GlobalTransform>>,
    terrain_setttings: Res<TerrainSettings>,
) {
    query
        .iter_mut()
        .for_each(|(entity, mut terrain_coordinate, global_transform)| {
            let coordinate =
                global_transform.translation().xz().as_ivec2() >> terrain_setttings.tile_size_power.get();

            if terrain_coordinate.is_added() || terrain_coordinate.0 != coordinate {
                if let Some(entries) = tile_to_terrain.0.get_mut(&terrain_coordinate.0) {
                    if let Some(index) = entries.iter().position(|e| *e == entity) {
                        entries.swap_remove(index);
                    }
                }

                if let Some(entries) = tile_to_terrain.0.get_mut(&coordinate) {
                    entries.push(entity);
                } else {
                    tile_to_terrain.0.insert(coordinate, vec![entity]);
                }

                terrain_coordinate.0 = coordinate;
                rebuild_tiles_event.send(RebuildTile(coordinate));
            }
        });
}
