//! Time travel: jumping anywhere in the history (seek bar, `←`/`→`, `Home`, loop mode).
//!
//! Jumping forward applies commits at once (`fast_forward`). Jumping back restores the latest
//! snapshot before the target (tree + view state, taken every `interval` commits while playing
//! or jumping) and fast-forwards from there, so any jump costs at most `interval` commits.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;

use crate::branches::Branches;
use crate::sparks::Sparks;
use crate::tree::{Touch, Tree};
use crate::{Authors, History, Palette, Scene, Settings, git, playback};

/// At most this many snapshots are kept, spread evenly over the history.
const MAX_SNAPSHOTS: usize = 24;
const MIN_SNAPSHOT_INTERVAL: usize = 100;
/// `←`/`→` jump by this fraction of the history.
const KEY_JUMP: f64 = 0.05;
/// `--loop` waits this long at the end before starting over.
const LOOP_PAUSE_SECONDS: f64 = 3.0;
/// Histogram bars of the seek bar.
const BINS: usize = 200;
/// Tags drawn as ticks on the seek bar; more are thinned out.
const MAX_TICKS: usize = 120;
/// The hover tooltip names a tag within this fraction of the bar from the cursor.
const TAG_HOVER: f64 = 0.004;
const BAR_HEIGHT: f32 = 24.0;
const BIN_COLOR: Color = Color::srgba(0.45, 0.55, 0.75, 0.22);
const PLAYED_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.035);
const CURSOR_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.8);
const TICK_COLOR: Color = Color::srgba(1.0, 0.8, 0.3, 0.45);

pub struct TimelinePlugin;

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Snapshots>()
            .init_resource::<Scrubbing>()
            .init_resource::<LastSeek>()
            .add_systems(
                Update,
                (
                    spawn_bar.run_if(resource_added::<History>),
                    (seek_bar, seek_keys, loop_back, apply_seek)
                        .chain()
                        .before(playback),
                    take_snapshot.after(crate::sync_scene),
                )
                    .run_if(resource_exists::<History>),
            );
    }
}

/// How long the last jump took, in milliseconds (reported by `--bench`).
#[derive(Resource, Default)]
pub struct LastSeek(pub Option<f32>);

/// True while the seek bar is being dragged, so the camera doesn't turn at the same time.
#[derive(Resource, Default)]
pub struct Scrubbing(pub bool);

struct Snapshot {
    next: usize,
    tree: Tree,
    scene: Scene,
}

#[derive(Resource, Default)]
struct Snapshots {
    list: Vec<Snapshot>,
}

impl Snapshots {
    fn interval(commits: usize) -> usize {
        (commits / MAX_SNAPSHOTS).max(MIN_SNAPSHOT_INTERVAL)
    }

    /// Keeps the state after `next` commits, unless a snapshot already sits close to it.
    fn offer(&mut self, next: usize, commits: usize, tree: &Tree, scene: &Scene) {
        let interval = Self::interval(commits);
        if next == 0 || self.list.iter().any(|s| s.next.abs_diff(next) < interval) {
            return;
        }
        let at = self.list.partition_point(|s| s.next < next);
        self.list.insert(
            at,
            Snapshot {
                next,
                tree: tree.snapshot(),
                scene: scene.snapshot(),
            },
        );
    }

    fn latest_until(&self, target: usize) -> Option<&Snapshot> {
        self.list.iter().rev().find(|s| s.next <= target)
    }
}

fn take_snapshot(
    history: Res<History>,
    tree: Res<Tree>,
    scene: Res<Scene>,
    mut snapshots: ResMut<Snapshots>,
) {
    // Only at commit boundaries: then the state is exactly "the first `next` commits".
    if history.pending.is_empty() && history.seek.is_none() {
        snapshots.offer(history.next, history.commits.len(), &tree, &scene);
    }
}

/// Jumps to `History::seek`, if set: before commit number `target`.
#[allow(clippy::too_many_arguments)]
fn apply_seek(
    time: Res<Time>,
    mut history: ResMut<History>,
    mut tree: ResMut<Tree>,
    mut scene: ResMut<Scene>,
    mut palette: ResMut<Palette>,
    mut sparks: ResMut<Sparks>,
    mut snapshots: ResMut<Snapshots>,
    mut authors: ResMut<Authors>,
    mut branches: ResMut<Branches>,
    mut last_seek: ResMut<LastSeek>,
) {
    let Some(target) = history.seek.take() else {
        return;
    };
    let started = std::time::Instant::now();
    let now = time.elapsed_secs_f64();
    let history = &mut *history;
    let target = target.min(history.commits.len());
    // Finish the queued changes without animation: the tree is then at a commit boundary.
    let still = |commit: usize| {
        let c = &history.commits[commit];
        Touch {
            author: authors.by_email(&c.email),
            commit,
            time: c.time,
            animate: false,
        }
    };
    for q in history.pending.drain(..).filter(|q| !q.ghost) {
        tree.apply(
            &history.commits[q.commit].changes[q.change],
            still(q.commit),
        );
    }
    scene.absorb(&mut tree, &mut palette, &mut sparks, now);
    // The way history is read changed: the snapshots show other states.
    let remake = std::mem::take(&mut history.remake);
    if remake {
        snapshots.list.clear();
    }

    let snapshot = snapshots.latest_until(target);
    match (
        start(snapshot.map(|s| s.next), history.next, target, remake),
        snapshot,
    ) {
        (Start::Snapshot, Some(snapshot)) => {
            *tree = snapshot.tree.snapshot();
            *scene = snapshot.scene.snapshot();
            history.next = snapshot.next;
        }
        (Start::Scratch, _) => {
            *tree = Tree::new().with_cities(tree.separate_cities);
            *scene = Scene::default();
            history.next = 0;
        }
        _ => {}
    }
    let dirs_before = tree.dirs.len();
    let commits = history.commits.len();
    let mut queued = Vec::new();
    while history.next < target {
        queued.clear();
        history
            .reading
            .queue(&history.commits, history.next, &mut queued);
        for q in queued.iter().filter(|q| !q.ghost) {
            tree.apply(
                &history.commits[q.commit].changes[q.change],
                still(q.commit),
            );
        }
        history.next += 1;
        scene.absorb(&mut tree, &mut palette, &mut sparks, now);
        snapshots.offer(history.next, commits, &tree, &scene);
    }
    // The restored layout is already at rest: only directories created since need settling.
    let new_dirs = tree.dirs.len() - dirs_before;
    tree.settle(new_dirs);
    branches.rebuild(history, &tree, &authors, &mut palette);

    let last = history.commits[commits - 1].time;
    history.time = history.commits.get(target).map_or(last, |c| c.time) as f64;
    history.budget = 0.0;
    scene.calm();
    sparks.clear();
    // Nobody is at work right after a jump: authors reappear with their next commit.
    for author in &mut authors.list {
        author.last_active = f64::NEG_INFINITY;
    }
    last_seek.0 = Some(started.elapsed().as_secs_f32() * 1000.0);
}

/// Where a jump starts replaying from.
#[derive(Debug, PartialEq)]
enum Start {
    Snapshot,
    Scratch,
    Here,
}

/// A jump from `next` to `target`, with the latest snapshot before the target taken after
/// `snapshot` commits, starts from that snapshot when it is closer than where we are (always,
/// going back), or from scratch.
fn start(snapshot: Option<usize>, next: usize, target: usize, remake: bool) -> Start {
    match snapshot {
        Some(s) if s > next || target < next => Start::Snapshot,
        None if target < next || remake => Start::Scratch,
        _ => Start::Here,
    }
}

/// The index of the first commit at or after `fraction` of the history's time span.
pub fn commit_at(history: &History, fraction: f64) -> usize {
    let (first, last) = history.span();
    let t = first + (last - first) * fraction.clamp(0.0, 1.0);
    history.commits.partition_point(|c| (c.time as f64) < t)
}

fn seek_keys(keys: Res<ButtonInput<KeyCode>>, mut history: ResMut<History>) {
    let jump = |history: &History, by: f64| commit_at(history, history.progress() + by);
    if keys.just_pressed(KeyCode::ArrowRight) {
        history.seek = Some(jump(&history, KEY_JUMP));
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        history.seek = Some(jump(&history, -KEY_JUMP));
    }
    if keys.just_pressed(KeyCode::Home) {
        history.seek = Some(history.first);
    }
}

fn loop_back(
    time: Res<Time>,
    settings: Res<Settings>,
    mut ended_at: Local<Option<f64>>,
    mut history: ResMut<History>,
) {
    if !settings.looping || !history.finished() {
        *ended_at = None;
        return;
    }
    if loop_due(&mut ended_at, time.elapsed_secs_f64()) {
        history.seek = Some(history.first);
    }
}

/// Called each frame at the end of the history: true once it has waited `LOOP_PAUSE_SECONDS`
/// there (since `ended_at`, which it sets on the first call and clears when due).
fn loop_due(ended_at: &mut Option<f64>, now: f64) -> bool {
    match *ended_at {
        None => *ended_at = Some(now),
        Some(at) if now - at > LOOP_PAUSE_SECONDS => {
            *ended_at = None;
            return true;
        }
        Some(_) => {}
    }
    false
}

/// Where time `t` sits in the span `first..last`, from 0 to 1.
fn fraction(t: f64, first: f64, last: f64) -> f64 {
    ((t - first) / (last - first)).clamp(0.0, 1.0)
}

/// Commits per bar of the seek bar's histogram.
fn histogram(times: impl Iterator<Item = i64>, first: f64, last: f64) -> [usize; BINS] {
    let mut bins = [0usize; BINS];
    for t in times {
        bins[((fraction(t as f64, first, last) * BINS as f64) as usize).min(BINS - 1)] += 1;
    }
    bins
}

/// Times of the tags within `first..=last`, thinned out to at most `MAX_TICKS`.
fn ticks(tags: &[(i64, String)], first: f64, last: f64) -> Vec<f64> {
    let in_range: Vec<f64> = tags
        .iter()
        .map(|(t, _)| *t as f64)
        .filter(|t| (first..=last).contains(t))
        .collect();
    in_range
        .iter()
        .copied()
        .step_by(in_range.len().div_ceil(MAX_TICKS).max(1))
        .collect()
}

/// The tag closest to time `t`, if within `TAG_HOVER` of the bar.
fn tag_near(tags: &[(i64, String)], t: f64, first: f64, last: f64) -> Option<&str> {
    tags.iter()
        .filter(|(tag_time, _)| ((*tag_time as f64 - t) / (last - first)).abs() < TAG_HOVER)
        .min_by_key(|(tag_time, _)| (*tag_time as f64 - t).abs() as i64)
        .map(|(_, name)| name.as_str())
}

#[derive(Component)]
struct Bar;

#[derive(Component)]
struct Played;

#[derive(Component)]
struct Cursor;

#[derive(Component)]
struct Tooltip;

/// Builds the seek bar once the history is known: an activity histogram (commits per bin),
/// tag ticks, the played part, a cursor, and a hover tooltip.
fn spawn_bar(mut commands: Commands, history: Res<History>, settings: Res<Settings>) {
    let (first, last) = history.span();
    let bins = histogram(history.commits.iter().map(|c| c.time), first, last);
    let max = bins.iter().copied().max().unwrap_or(1).max(1) as f32;

    let bar = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(16),
                right: px(16),
                bottom: px(44),
                height: px(BAR_HEIGHT),
                align_items: AlignItems::FlexEnd,
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.03)),
            Hovered::default(),
            RelativeCursorPosition::default(),
            if settings.hide.timeline {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
            Bar,
        ))
        .id();
    // Square roots keep quiet periods visible next to busy ones.
    for count in bins {
        let bin = commands
            .spawn((
                Node {
                    flex_grow: 1.0,
                    height: percent(100.0 * (count as f32 / max).sqrt()),
                    ..default()
                },
                BackgroundColor(if count > 0 { BIN_COLOR } else { Color::NONE }),
            ))
            .id();
        commands.entity(bar).add_child(bin);
    }
    for t in ticks(&history.tags, first, last) {
        let tick = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(100.0 * fraction(t, first, last) as f32),
                    top: px(0),
                    width: px(1),
                    height: percent(30),
                    ..default()
                },
                BackgroundColor(TICK_COLOR),
            ))
            .id();
        commands.entity(bar).add_child(tick);
    }
    let overlay = |left: f32, width: Val, color: Color| {
        (
            Node {
                position_type: PositionType::Absolute,
                left: percent(left),
                top: px(0),
                width,
                height: percent(100),
                ..default()
            },
            BackgroundColor(color),
        )
    };
    let played = commands
        .spawn((overlay(0.0, percent(0), PLAYED_COLOR), Played))
        .id();
    let cursor = commands
        .spawn((overlay(0.0, px(2), CURSOR_COLOR), Cursor))
        .id();
    let tooltip = commands
        .spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                bottom: px(BAR_HEIGHT + 4.0),
                ..default()
            },
            Visibility::Hidden,
            Tooltip,
        ))
        .id();
    commands
        .entity(bar)
        .add_children(&[played, cursor, tooltip]);
}

/// Hover shows the date (and a nearby tag); click or drag, then release, to jump there.
#[allow(clippy::type_complexity)]
fn seek_bar(
    buttons: Res<ButtonInput<MouseButton>>,
    mut history: ResMut<History>,
    mut scrubbing: ResMut<Scrubbing>,
    bar: Single<(&RelativeCursorPosition, &InheritedVisibility), With<Bar>>,
    mut played: Single<&mut Node, (With<Played>, Without<Cursor>, Without<Tooltip>)>,
    mut cursor: Single<&mut Node, (With<Cursor>, Without<Played>, Without<Tooltip>)>,
    tooltip: Single<
        (&mut Node, &mut Text, &mut Visibility),
        (With<Tooltip>, Without<Played>, Without<Cursor>),
    >,
) {
    let (position, visible) = *bar;
    let hover = position
        .normalized
        .filter(|_| visible.get())
        .map(|n| f64::from(n.x) + 0.5);
    let over = position.cursor_over() && visible.get();
    if buttons.just_pressed(MouseButton::Left) && over {
        scrubbing.0 = true;
    }
    let at = if scrubbing.0 {
        hover.map(|f| f.clamp(0.0, 1.0))
    } else {
        hover.filter(|_| over)
    };
    if scrubbing.0 && buttons.just_released(MouseButton::Left) {
        scrubbing.0 = false;
        if let Some(fraction) = at {
            history.seek = Some(commit_at(&history, fraction));
        }
    }

    // While dragging, the cursor previews where the release will jump.
    let shown = if scrubbing.0 {
        at.unwrap_or(history.progress())
    } else {
        history.progress()
    };
    let width = percent(100.0 * history.progress() as f32);
    if played.width != width {
        played.width = width;
    }
    let left = percent(100.0 * shown as f32);
    if cursor.left != left {
        cursor.left = left;
    }

    let (mut node, mut text, mut visibility) = tooltip.into_inner();
    let Some(fraction) = at else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let (first, last) = history.span();
    let t = first + (last - first) * fraction;
    let wanted = match tag_near(&history.tags, t, first, last) {
        Some(name) => format!("{}  {name}", git::date(t as i64)),
        None => git::date(t as i64),
    };
    if text.0 != wanted {
        text.0 = wanted;
    }
    node.left = percent(100.0 * fraction as f32);
    visibility.set_if_neq(Visibility::Inherited);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(snapshots: &mut Snapshots, next: usize, commits: usize) {
        snapshots.offer(next, commits, &Tree::new(), &Scene::default());
    }

    fn nexts(snapshots: &Snapshots) -> Vec<usize> {
        snapshots.list.iter().map(|s| s.next).collect()
    }

    #[test]
    fn snapshots_are_spread_over_the_history() {
        assert_eq!(Snapshots::interval(1000), MIN_SNAPSHOT_INTERVAL);
        assert_eq!(Snapshots::interval(24_000), 1000);
        // Offered at every commit while playing: one every interval, at most `MAX_SNAPSHOTS`.
        let mut snapshots = Snapshots::default();
        for next in 0..=24_000 {
            offer(&mut snapshots, next, 24_000);
        }
        let expected: Vec<usize> = (0..24).map(|i| 1 + i * 1000).collect();
        assert_eq!(nexts(&snapshots), expected);
    }

    #[test]
    fn snapshots_stay_sorted_whatever_the_order() {
        let mut snapshots = Snapshots::default();
        for next in [500, 0, 150, 900, 520, 300, 120] {
            offer(&mut snapshots, next, 1000);
        }
        // 0 is the empty tree, 520 and 120 too close to others.
        assert_eq!(nexts(&snapshots), [150, 300, 500, 900]);
    }

    #[test]
    fn the_latest_snapshot_before_the_target_is_used() {
        let mut snapshots = Snapshots::default();
        for next in [100, 200, 300] {
            offer(&mut snapshots, next, 1000);
        }
        let latest = |target| snapshots.latest_until(target).map(|s| s.next);
        assert_eq!(latest(50), None);
        assert_eq!(latest(100), Some(100));
        assert_eq!(latest(299), Some(200));
        assert_eq!(latest(5000), Some(300));
    }

    #[test]
    fn jumps_start_from_the_closest_known_state() {
        // Forward, past a snapshot further on than where we are: from it.
        assert_eq!(start(Some(300), 100, 350, false), Start::Snapshot);
        // Forward, the snapshot is behind us: carry on from here.
        assert_eq!(start(Some(300), 320, 350, false), Start::Here);
        assert_eq!(start(None, 100, 350, false), Start::Here);
        // Back: from the snapshot before the target, else from scratch.
        assert_eq!(start(Some(300), 500, 350, false), Start::Snapshot);
        assert_eq!(start(None, 500, 50, false), Start::Scratch);
        // The way of reading changed (no snapshots left): from scratch, even forward.
        assert_eq!(start(None, 100, 350, true), Start::Scratch);
        // Staying put.
        assert_eq!(start(None, 100, 100, false), Start::Here);
    }

    #[test]
    fn loops_after_a_pause_at_the_end() {
        let mut ended_at = None;
        assert!(!loop_due(&mut ended_at, 10.0));
        assert_eq!(ended_at, Some(10.0));
        assert!(!loop_due(&mut ended_at, 10.0 + LOOP_PAUSE_SECONDS));
        assert!(loop_due(&mut ended_at, 10.1 + LOOP_PAUSE_SECONDS));
        // Then waits again at the next end.
        assert_eq!(ended_at, None);
        assert!(!loop_due(&mut ended_at, 20.0));
    }

    #[test]
    fn fractions_are_clamped_to_the_span() {
        assert_eq!(fraction(150.0, 100.0, 200.0), 0.5);
        assert_eq!(fraction(50.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction(250.0, 100.0, 200.0), 1.0);
    }

    #[test]
    fn histogram_counts_commits_per_bin() {
        let span = BINS as i64 * 10;
        let times = [0, 5, 9, 10, span / 2, span - 1, span, span + 100];
        let bins = histogram(times.into_iter(), 0.0, span as f64);
        assert_eq!(bins.iter().sum::<usize>(), times.len());
        assert_eq!((bins[0], bins[1], bins[BINS / 2]), (3, 1, 1));
        // The last commit (and anything beyond) goes in the last bar.
        assert_eq!(bins[BINS - 1], 3);
    }

    fn tags(times: impl Iterator<Item = i64>) -> Vec<(i64, String)> {
        times.map(|t| (t, format!("v{t}"))).collect()
    }

    #[test]
    fn ticks_skip_tags_out_of_range() {
        let tags = tags([5, 10, 50, 100, 150].into_iter());
        assert_eq!(ticks(&tags, 10.0, 100.0), [10.0, 50.0, 100.0]);
        assert!(ticks(&[], 10.0, 100.0).is_empty());
    }

    #[test]
    fn many_tags_are_thinned_out() {
        let tags = tags(0..1000);
        let ticks = ticks(&tags, 0.0, 1000.0);
        assert!(ticks.len() <= MAX_TICKS && ticks.len() > MAX_TICKS / 2);
        // Evenly: every ninth tag.
        assert!(ticks.windows(2).all(|w| w[1] - w[0] == 9.0));
    }

    #[test]
    fn hover_names_the_closest_tag_nearby() {
        let tags = tags([1000, 1003, 5000].into_iter());
        // TAG_HOVER of a 10,000 s span is 40 s.
        assert_eq!(tag_near(&tags, 1002.0, 0.0, 10_000.0), Some("v1003"));
        assert_eq!(tag_near(&tags, 970.0, 0.0, 10_000.0), Some("v1000"));
        assert_eq!(tag_near(&tags, 3000.0, 0.0, 10_000.0), None);
        assert_eq!(tag_near(&tags, 5039.0, 0.0, 10_000.0), Some("v5000"));
        assert_eq!(tag_near(&tags, 5041.0, 0.0, 10_000.0), None);
    }
}
