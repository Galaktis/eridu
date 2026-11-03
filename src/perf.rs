//! Performance tools: the `F3` overlay and the `--bench` run.

use std::time::Instant;

use bevy::diagnostic::{
    DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin,
};
use bevy::prelude::*;

use crate::History;
use crate::pace::Pace;
use crate::quality::QualityState;
use crate::timeline::{LastSeek, commit_at};
use crate::tree::Tree;

/// After playing, the bench jumps to these points of the history (as fractions) and times each jump.
const BENCH_SEEKS: &[f64] = &[0.1, 0.5, 0.05, 0.9, 0.3, 1.0];

/// The overlay refreshes this often, so updating its text costs nothing noticeable.
const OVERLAY_REFRESH_SECONDS: f32 = 0.25;

pub struct PerfPlugin {
    pub bench: bool,
}

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            FrameTimeDiagnosticsPlugin::default(),
            EntityCountDiagnosticsPlugin::default(),
        ))
        .add_systems(Startup, spawn_overlay)
        .add_systems(Update, (toggle_overlay, update_overlay));
        if self.bench {
            app.insert_resource(Bench {
                started: Instant::now(),
                loaded_after: None,
                frames: Vec::new(),
                peak: Peak::default(),
                seeks: None,
            })
            .add_systems(Last, record_bench);
        }
    }
}

#[derive(Component)]
struct Overlay;

fn spawn_overlay(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.5, 0.9, 0.6)),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            right: px(16),
            ..default()
        },
        Visibility::Hidden,
        Overlay,
    ));
}

fn toggle_overlay(
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: Single<&mut Visibility, With<Overlay>>,
) {
    if keys.just_pressed(KeyCode::F3) {
        **overlay = match **overlay {
            Visibility::Hidden => Visibility::Inherited,
            _ => Visibility::Hidden,
        };
    }
}

fn update_overlay(
    time: Res<Time<Real>>,
    diagnostics: Res<DiagnosticsStore>,
    tree: Res<Tree>,
    quality: Res<QualityState>,
    pace: Res<Pace>,
    mut since: Local<f32>,
    overlay: Single<(&mut Text, &Visibility), With<Overlay>>,
) {
    let (mut text, visibility) = overlay.into_inner();
    *since += time.delta_secs();
    if *visibility == Visibility::Hidden || *since < OVERLAY_REFRESH_SECONDS {
        return;
    }
    *since = 0.0;
    let value = |path| {
        diagnostics
            .get(path)
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0)
    };
    let frame_times: Vec<f64> = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .map(|d| d.values().copied().collect())
        .unwrap_or_default();
    text.0 = format!(
        "{:.0} fps   {:.2} ms (worst {:.2})\n{} entities\n{} directories   {} files\nquality {}{}\n{}",
        value(&FrameTimeDiagnosticsPlugin::FPS),
        value(&FrameTimeDiagnosticsPlugin::FRAME_TIME),
        frame_times.iter().copied().fold(0.0, f64::max),
        value(&EntityCountDiagnosticsPlugin::ENTITY_COUNT),
        tree.dirs.iter().filter(|d| d.alive).count(),
        tree.file_count(),
        quality.level.name(),
        if quality.auto { " (auto)" } else { "" },
        pace.describe(),
    );
}

#[derive(Default)]
struct Peak {
    entities: f64,
    dirs: usize,
    files: usize,
}

#[derive(Resource)]
struct Bench {
    started: Instant,
    loaded_after: Option<f32>,
    /// Frame times in milliseconds, from the first frame of playback.
    frames: Vec<f32>,
    peak: Peak,
    /// Index in `BENCH_SEEKS` of the next jump, once playback is over.
    seeks: Option<usize>,
}

/// The index of the slowest frame (the first, if several).
fn slowest(frames: &[f32]) -> usize {
    frames
        .iter()
        .enumerate()
        .fold((0, 0.0), |m, (i, &t)| if t > m.1 { (i, t) } else { m })
        .0
}

/// The `q` quantile (0 to 1) of sorted, non-empty frame times: the nearest rank below.
fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q) as usize]
}

/// Records every frame of playback and prints the statistics once the history ends; then
/// times a few jumps through history and quits.
fn record_bench(
    time: Res<Time<Real>>,
    history: Option<ResMut<History>>,
    tree: Res<Tree>,
    diagnostics: Res<DiagnosticsStore>,
    mut last_seek: ResMut<LastSeek>,
    mut bench: ResMut<Bench>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut history) = history else {
        return;
    };
    if let Some(next) = bench.seeks {
        if let Some(ms) = last_seek.0.take() {
            println!(
                "bench: jump to {:>3.0}% took {ms:.1} ms",
                BENCH_SEEKS[next - 1] * 100.0
            );
        }
        match BENCH_SEEKS.get(next) {
            Some(&fraction) => {
                history.seek = Some(commit_at(&history, fraction));
                bench.seeks = Some(next + 1);
            }
            None => {
                exit.write(AppExit::Success);
            }
        }
        return;
    }
    if bench.loaded_after.is_none() {
        bench.loaded_after = Some(bench.started.elapsed().as_secs_f32());
        return;
    }
    bench.frames.push(time.delta_secs() * 1000.0);
    let entities = diagnostics
        .get(&EntityCountDiagnosticsPlugin::ENTITY_COUNT)
        .and_then(|d| d.value())
        .unwrap_or(0.0);
    let peak = &mut bench.peak;
    peak.entities = peak.entities.max(entities);
    peak.dirs = peak.dirs.max(tree.dirs.iter().filter(|d| d.alive).count());
    peak.files = peak.files.max(tree.file_count());
    if !history.finished() {
        return;
    }
    let mut frames = std::mem::take(&mut bench.frames);
    let seconds: f32 = frames.iter().sum::<f32>() / 1000.0;
    // The slowest frame and when it happened: a one-off startup hitch reads differently from a stall.
    let worst_at = slowest(&frames);
    frames.sort_unstable_by(f32::total_cmp);
    let at = |q: f32| percentile(&frames, q);
    println!(
        "bench: history ready after {:.2} s; {} commits played in {seconds:.1} s, {} frames ({:.0} fps)",
        bench.loaded_after.unwrap_or(0.0),
        history.commits.len(),
        frames.len(),
        frames.len() as f32 / seconds,
    );
    println!(
        "bench: frame time p50 {:.2} ms   p95 {:.2} ms   p99 {:.2} ms   max {:.2} ms (frame {worst_at})",
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0),
    );
    println!(
        "bench: peak {} files, {} live directories, {:.0} entities; {} files at the end",
        bench.peak.files,
        bench.peak.dirs,
        bench.peak.entities,
        tree.file_count(),
    );
    last_seek.0 = None;
    bench.seeks = Some(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_take_the_nearest_rank_below() {
        let frames: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        assert_eq!(percentile(&frames, 0.0), 1.0);
        assert_eq!(percentile(&frames, 0.5), 50.0);
        assert_eq!(percentile(&frames, 0.95), 95.0);
        assert_eq!(percentile(&frames, 0.99), 99.0);
        assert_eq!(percentile(&frames, 1.0), 100.0);
        assert_eq!(percentile(&[7.0], 0.99), 7.0);
    }

    #[test]
    fn the_slowest_frame_is_found() {
        assert_eq!(slowest(&[1.0, 5.0, 2.0, 5.0, 3.0]), 1);
        assert_eq!(slowest(&[4.0]), 0);
        assert_eq!(slowest(&[]), 0);
    }

    #[test]
    fn bench_seeks_stay_in_the_history() {
        assert!(BENCH_SEEKS.iter().all(|f| (0.0..=1.0).contains(f)));
        // Back and forth: jumps both ways get timed.
        assert!(BENCH_SEEKS.windows(2).any(|w| w[1] < w[0]));
        assert!(BENCH_SEEKS.windows(2).any(|w| w[1] > w[0]));
    }
}
