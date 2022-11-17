use std::f32::consts::PI;

use bevy::{prelude::*, window::close_on_esc};
use bevy_mod_outline::{
    AutoGenerateOutlineNormalsPlugin, Outline, OutlineBundle, OutlinePlugin, OutlineStencil,
};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugin(OutlinePlugin)
        .add_plugin(AutoGenerateOutlineNormalsPlugin)
        .insert_resource(AmbientLight {
            color: Color::WHITE,
            brightness: 1.0,
        })
        .add_startup_system(setup)
        .add_system(setup_scene_once_loaded)
        .add_system(close_on_esc)
        .run();
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Insert a resource with the current animation
    commands.insert_resource::<Handle<AnimationClip>>(asset_server.load("Fox.glb#Animation0"));

    // Camera
    commands.spawn_bundle(Camera3dBundle {
        transform: Transform::from_xyz(100.0, 100.0, 150.0)
            .looking_at(Vec3::new(0.0, 20.0, 0.0), Vec3::Y),
        ..default()
    });

    // Plane
    commands.spawn_bundle(PbrBundle {
        mesh: meshes.add(Mesh::from(shape::Plane { size: 500000.0 })),
        material: materials.add(Color::rgb(0.3, 0.5, 0.3).into()),
        ..default()
    });

    // Light
    commands.spawn_bundle(DirectionalLightBundle {
        transform: Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.)),
        directional_light: DirectionalLight {
            shadows_enabled: true,
            ..default()
        },
        ..default()
    });

    // Fox
    commands.spawn_bundle(SceneBundle {
        scene: asset_server.load("Fox.glb#Scene0"),
        ..default()
    });
}

// Once the scene is loaded, start the animation and add an outline
fn setup_scene_once_loaded(
    mut commands: Commands,
    animation: Res<Handle<AnimationClip>>,
    mut player: Query<&mut AnimationPlayer>,
    entities: Query<Entity, With<Handle<Mesh>>>,
    mut done: Local<bool>,
) {
    if !*done {
        if let Ok(mut player) = player.get_single_mut() {
            player.play(animation.clone_weak()).repeat();
            for entity in entities.iter() {
                commands.entity(entity).insert_bundle(OutlineBundle {
                    outline: Outline {
                        visible: true,
                        width: 3.0,
                        colour: Color::RED,
                    },
                    stencil: OutlineStencil,
                });
            }
            *done = true;
        }
    }
}