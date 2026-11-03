//! Quality tiers, so modest machines stay smooth and strong ones get every effect.
//! `Q` cycles them; `auto` (the default) starts at `high` and steps down when frames are slow.

use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;

use crate::files::FileInstances;
use crate::sparks::Sparks;

/// `auto` steps down after frames averaged longer than this (40 fps) for `AUTO_SLOW_SECONDS`…
const AUTO_SLOW_FRAME: f32 = 1.0 / 40.0;
const AUTO_SLOW_SECONDS: f32 = 3.0;
/// …ignoring the first seconds, when loading and jumping to the start cause hitches.
const AUTO_GRACE_SECONDS: f64 = 5.0;
/// Weight of the newest frame in the frame-time average.
const AUTO_SMOOTHING: f32 = 0.05;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    /// No bloom, no antialiasing, coarse spheres, no sparks.
    Low,
    High,
    /// Larger bloom, finer spheres, twice the sparks.
    Ultra,
}

impl Quality {
    fn next(self) -> Self {
        match self {
            Quality::Low => Quality::High,
            Quality::High => Quality::Ultra,
            Quality::Ultra => Quality::Low,
        }
    }

    /// The tier `auto` steps down to.
    fn lower(self) -> Self {
        match self {
            Quality::Ultra => Quality::High,
            _ => Quality::Low,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::High => "high",
            Quality::Ultra => "ultra",
        }
    }
}

#[derive(Resource)]
pub struct QualityState {
    pub level: Quality,
    /// Still allowed to step down on its own.
    pub auto: bool,
    average_frame: f32,
    slow_for: f32,
}

pub struct QualityPlugin {
    /// `None` is `auto`.
    pub quality: Option<Quality>,
}

impl Plugin for QualityPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(QualityState {
            level: self.quality.unwrap_or(Quality::High),
            auto: self.quality.is_none(),
            average_frame: 0.0,
            slow_for: 0.0,
        })
        .add_systems(Startup, make_meshes)
        .add_systems(Update, (cycle, step_down, apply).chain());
    }
}

/// The file sphere at each tier.
#[derive(Resource)]
struct FileMeshes([Handle<Mesh>; 3]);

fn make_meshes(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut sphere = |detail| {
        meshes.add(
            Sphere::new(0.15)
                .mesh()
                .ico(detail)
                .expect("small subdivisions are valid"),
        )
    };
    commands.insert_resource(FileMeshes([sphere(1), sphere(2), sphere(3)]));
}

fn cycle(keys: Res<ButtonInput<bevy::input::keyboard::Key>>, mut state: ResMut<QualityState>) {
    if crate::keys::typed(&keys, "q") {
        state.level = state.level.next();
        // A choice made by hand is kept.
        state.auto = false;
        info!("quality: {}", state.level.name());
    }
}

fn step_down(time: Res<Time<Real>>, mut state: ResMut<QualityState>) {
    if !state.auto || state.level == Quality::Low || time.elapsed_secs_f64() < AUTO_GRACE_SECONDS {
        return;
    }
    // Read through the `Mut` without marking the state changed every frame.
    let (average, slow_for) = track(state.average_frame, state.slow_for, time.delta_secs());
    state.bypass_change_detection().average_frame = average;
    state.bypass_change_detection().slow_for = slow_for;
    if slow_for > AUTO_SLOW_SECONDS {
        state.level = state.level.lower();
        state.slow_for = 0.0;
        info!(
            "quality: frames took {:.0} ms, stepping down to {}",
            average * 1000.0,
            state.level.name()
        );
    }
}

/// The frame-time average and how long it has been slow, after a frame of `dt` seconds.
fn track(average: f32, slow_for: f32, dt: f32) -> (f32, f32) {
    let average = average + (dt - average) * AUTO_SMOOTHING;
    let slow_for = if average > AUTO_SLOW_FRAME {
        slow_for + dt
    } else {
        0.0
    };
    (average, slow_for)
}

/// Applies the tier whenever it changes.
fn apply(
    state: Res<QualityState>,
    meshes: Res<FileMeshes>,
    camera: Single<Entity, With<Camera3d>>,
    mut layer: Single<&mut Mesh3d, With<FileInstances>>,
    mut sparks: ResMut<Sparks>,
    mut commands: Commands,
) {
    if !state.is_changed() {
        return;
    }
    let mut camera = commands.entity(*camera);
    let (msaa, mesh, sparks_per_file) = match state.level {
        Quality::Low => {
            camera.remove::<Bloom>();
            (Msaa::Off, 0, 0)
        }
        Quality::High => {
            camera.insert(Bloom::NATURAL);
            (Msaa::Sample4, 1, 8)
        }
        Quality::Ultra => {
            camera.insert(Bloom {
                max_mip_dimension: 1024,
                ..Bloom::NATURAL
            });
            (Msaa::Sample4, 2, 16)
        }
    };
    camera.insert(msaa);
    layer.0 = meshes.0[mesh].clone();
    sparks.per_file = sparks_per_file;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seconds until `auto` would step down with frames of `frame` seconds, if within `limit`.
    fn steps_down_after(frame: f32, limit: f32) -> Option<f32> {
        let (mut average, mut slow_for, mut elapsed) = (0.0, 0.0, 0.0);
        while elapsed < limit {
            (average, slow_for) = track(average, slow_for, frame);
            elapsed += frame;
            if slow_for > AUTO_SLOW_SECONDS {
                return Some(elapsed);
            }
        }
        None
    }

    #[test]
    fn q_cycles_through_the_tiers() {
        let mut level = Quality::Low;
        let names: Vec<&str> = (0..4)
            .map(|_| {
                level = level.next();
                level.name()
            })
            .collect();
        assert_eq!(names, ["high", "ultra", "low", "high"]);
    }

    #[test]
    fn auto_steps_down_one_tier_at_a_time() {
        assert_eq!(Quality::Ultra.lower(), Quality::High);
        assert_eq!(Quality::High.lower(), Quality::Low);
        assert_eq!(Quality::Low.lower(), Quality::Low);
    }

    #[test]
    fn slow_frames_step_down_after_a_while() {
        // 20 fps: the average takes a moment to pass 40 fps, then 3 s more.
        let after = steps_down_after(1.0 / 20.0, 10.0).unwrap();
        assert!(
            after > AUTO_SLOW_SECONDS && after < AUTO_SLOW_SECONDS + 1.0,
            "{after}"
        );
    }

    #[test]
    fn smooth_frames_never_step_down() {
        assert_eq!(steps_down_after(1.0 / 60.0, 60.0), None);
        // Just above 40 fps.
        assert_eq!(steps_down_after(1.0 / 41.0, 60.0), None);
    }

    #[test]
    fn hitches_dont_step_down() {
        let (mut average, mut slow_for) = (1.0 / 60.0, 0.0);
        for frame in 0..3600 {
            // A half-second hitch every two seconds.
            let dt = if frame % 120 == 0 { 0.5 } else { 1.0 / 60.0 };
            (average, slow_for) = track(average, slow_for, dt);
            assert!(slow_for <= AUTO_SLOW_SECONDS, "frame {frame}");
        }
        // A fast frame with a fast average resets the count.
        assert_eq!(track(1.0 / 60.0, 2.0, 1.0 / 60.0).1, 0.0);
    }
}
