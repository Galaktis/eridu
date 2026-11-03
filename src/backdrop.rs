//! `--background`: an image behind the scene. It is a quad that hangs far in front of the
//! camera (as its child) and always fills the view, so the 3D scene draws over it with depth.

use bevy::prelude::*;

use crate::{Settings, load_image, setup};

/// Distance of the backdrop from the camera, beyond any tree (the far plane is at 1000).
const DISTANCE: f32 = 900.0;
/// The image is dimmed so the files stay readable over it (and bright parts don't bloom).
const BRIGHTNESS: f32 = 0.45;

pub struct BackdropPlugin;

impl Plugin for BackdropPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn.after(setup))
            .add_systems(Update, fit);
    }
}

#[derive(Component)]
struct Backdrop;

fn spawn(
    settings: Res<Settings>,
    camera: Single<Entity, With<Camera3d>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let Some(path) = &settings.background else {
        return;
    };
    let image = match load_image(path) {
        Ok(image) => images.add(image),
        Err(e) => return warn!("background: {e}"),
    };
    let material = StandardMaterial {
        base_color: Color::srgb(BRIGHTNESS, BRIGHTNESS, BRIGHTNESS),
        base_color_texture: Some(image),
        unlit: true,
        ..default()
    };
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
        MeshMaterial3d(materials.add(material)),
        Transform::from_xyz(0.0, 0.0, -DISTANCE),
        ChildOf(*camera),
        Backdrop,
    ));
}

/// Scales the quad to the view (Bevy's default vertical field of view is π/4).
fn fit(
    camera: Single<&Camera, With<Camera3d>>,
    mut backdrop: Query<&mut Transform, With<Backdrop>>,
) {
    let height = 2.0 * DISTANCE * (std::f32::consts::FRAC_PI_4 / 2.0).tan();
    let aspect = camera
        .logical_viewport_size()
        .map_or(1.0, |size| size.x / size.y.max(1.0));
    let scale = Vec3::new(height * aspect, height, 1.0);
    for mut transform in &mut backdrop {
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
}
