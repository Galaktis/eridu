//! The cinematic camera: when nobody touches the camera, it directs shots instead of only
//! orbiting the whole tree. It drifts to where a burst of activity happens, follows an author
//! through a few commits, and pulls back for wide shots in quiet periods or when a new city
//! appears. Calm above all: every shot lasts a while, and moves are long, eased and capped.

use bevy::prelude::*;

use crate::tree::Tree;
use crate::{Authors, Scene};

/// The camera directs itself after this long without the user moving it.
const IDLE_SECONDS: f64 = 10.0;
/// Least time on a wide shot, and on a close one, before the next.
const WIDE_SECONDS: f64 = 25.0;
const CLOSE_SECONDS: f64 = 10.0;
/// A close shot pulls back after this long, or this long without anything changing.
const CLOSE_MAX_SECONDS: f64 = 20.0;
const QUIET_SECONDS: f64 = 4.0;
/// A burst is worth a close shot with this many glowing files, in this fraction of the tree.
const BURST_FILES: usize = 3;
const BURST_SHARE: f32 = 0.6;
/// Least radius of a close shot, as a share of the tree's and absolute: the view should keep a
/// big part of the project, so a close shot leans towards the action more than it zooms in.
const CLOSE_MIN_SHARE: f32 = 0.8;
const CLOSE_MIN_RADIUS: f32 = 8.0;
/// Time constant of the shot's target, so a moving burst doesn't drag the camera around.
const TARGET_SMOOTH_SECONDS: f32 = 2.0;
/// Time constant of the camera's moves, and their caps: the focus moves at most this many
/// view distances per second, the distance changes at most this fraction per second, the pitch
/// turns at most this many radians per second.
const MOVE_SECONDS: f32 = 2.5;
const MAX_PAN: f32 = 0.2;
const MAX_ZOOM: f32 = 0.2;
const MAX_TILT: f32 = 0.05;
/// Pitch of wide and close shots (wide ones look down more), and rotation speed of close ones
/// relative to the usual auto-rotation.
const WIDE_PITCH: f32 = -0.5;
const CLOSE_PITCH: f32 = -0.35;
const CLOSE_ROTATION: f32 = 0.6;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CameraMode {
    Cinematic,
    Orbit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shot {
    Wide,
    Burst,
    Author(usize),
}

#[derive(Resource)]
pub struct Director {
    pub mode: CameraMode,
    shot: Shot,
    started: f64,
    /// Close shots so far, to alternate bursts and authors.
    close_shots: u32,
    /// The last time the user moved the camera.
    last_input: f64,
    /// When a file last started glowing.
    last_activity: f64,
    glowing: usize,
    cities: usize,
    /// The shot's smoothed (center, radius).
    target: Option<(Vec3, f32)>,
}

/// Where the director wants the camera this frame.
pub struct Framing {
    pub center: Vec3,
    pub radius: f32,
    pub pitch: f32,
    /// Multiplies the auto-rotation speed.
    pub rotation: f32,
}

impl Director {
    pub fn new(mode: CameraMode) -> Self {
        Self {
            mode,
            shot: Shot::Wide,
            started: 0.0,
            close_shots: 0,
            last_input: f64::NEG_INFINITY,
            last_activity: 0.0,
            glowing: 0,
            cities: 0,
            target: None,
        }
    }

    /// The user moved the camera: hands it over for a while.
    pub fn input(&mut self, now: f64) {
        self.last_input = now;
        self.target = None;
    }

    pub fn directing(&self, now: f64) -> bool {
        self.mode == CameraMode::Cinematic && now - self.last_input > IDLE_SECONDS
    }

    /// Picks the shot and returns what to frame.
    pub fn frame(
        &mut self,
        now: f64,
        dt: f32,
        tree: &Tree,
        scene: &Scene,
        authors: &Authors,
    ) -> Framing {
        if scene.glowing.len() > self.glowing {
            self.last_activity = now;
        }
        self.glowing = scene.glowing.len();
        let whole = tree.bounds();
        let cities = if tree.separate_cities {
            tree.dirs
                .iter()
                .filter(|d| d.alive && d.parent == Some(0))
                .count()
        } else {
            0
        };
        let new_city = cities > self.cities;
        self.cities = cities;

        let burst = burst(tree, scene).filter(|&(_, r)| r < whole.1 * BURST_SHARE);
        let quiet = now - self.last_activity > QUIET_SECONDS;
        let held = now - self.started;
        let next = match self.shot {
            Shot::Wide if held >= WIDE_SECONDS && !quiet && burst.is_some() => {
                let busiest = (self.close_shots % 2 == 1)
                    .then(|| busiest_author(scene, authors))
                    .flatten();
                Some(busiest.map_or(Shot::Burst, Shot::Author))
            }
            Shot::Burst | Shot::Author(_)
                if held >= CLOSE_MAX_SECONDS || (held >= CLOSE_SECONDS && (quiet || new_city)) =>
            {
                Some(Shot::Wide)
            }
            Shot::Burst if burst.is_none() && held >= CLOSE_SECONDS => Some(Shot::Wide),
            _ => None,
        };
        if let Some(shot) = next {
            if shot != Shot::Wide {
                self.close_shots += 1;
            }
            debug!("camera: {shot:?} shot");
            self.shot = shot;
            self.started = now;
        }

        let least = (whole.1 * CLOSE_MIN_SHARE).max(CLOSE_MIN_RADIUS);
        let wanted = match self.shot {
            Shot::Wide => Some(whole),
            Shot::Burst => burst.map(|(c, r)| (c, r.max(least))),
            Shot::Author(a) => scene
                .work_area(&|id| tree.file_pos(id), authors, a)
                .map(|(c, r)| (c, r.max(least))),
        }
        .unwrap_or(whole);
        let k = 1.0 - (-dt / TARGET_SMOOTH_SECONDS).exp();
        let (center, radius) = match self.target {
            Some((c, r)) => (c.lerp(wanted.0, k), r + (wanted.1 - r) * k),
            None => wanted,
        };
        self.target = Some((center, radius));
        let close = self.shot != Shot::Wide;
        Framing {
            center,
            radius,
            pitch: if close { CLOSE_PITCH } else { WIDE_PITCH },
            rotation: if close { CLOSE_ROTATION } else { 1.0 },
        }
    }
}

/// Center and radius of the files glowing right now, if enough of them do.
fn burst(tree: &Tree, scene: &Scene) -> Option<(Vec3, f32)> {
    if scene.glowing.len() < BURST_FILES {
        return None;
    }
    let positions = || scene.glowing.iter().filter_map(|&id| tree.file_pos(id));
    let count = positions().count();
    if count < BURST_FILES {
        return None;
    }
    let center = positions().sum::<Vec3>() / count as f32;
    Some((
        center,
        positions().map(|p| p.distance(center)).fold(0.0, f32::max),
    ))
}

/// The author behind most of the glowing files, if they are behind at least `BURST_FILES`.
fn busiest_author(scene: &Scene, authors: &Authors) -> Option<usize> {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for view in scene
        .glowing
        .iter()
        .filter_map(|&id| scene.files[id].as_ref())
    {
        let a = authors.resolve(view.touched_by);
        match counts.iter_mut().find(|(b, _)| *b == a) {
            Some((_, n)) => *n += 1,
            None => counts.push((a, 1)),
        }
    }
    counts
        .into_iter()
        .filter(|&(_, n)| n >= BURST_FILES)
        .max_by_key(|&(_, n)| n)
        .map(|(a, _)| a)
}

/// Moves `value` towards `target` with time constant `MOVE_SECONDS`, at most `max` per second.
pub fn ease(value: f32, target: f32, dt: f32, max: f32) -> f32 {
    let step = (target - value) * (1.0 - (-dt / MOVE_SECONDS).exp());
    value + step.clamp(-max * dt, max * dt)
}

/// Moves the focus towards `target` the same way, at most `MAX_PAN` view distances per second.
pub fn ease_focus(focus: Vec3, target: Vec3, distance: f32, dt: f32) -> Vec3 {
    let step = (target - focus) * (1.0 - (-dt / MOVE_SECONDS).exp());
    focus + step.clamp_length_max(MAX_PAN * distance * dt)
}

/// Eases a distance in log space, so zooming in and out feel the same, at most `MAX_ZOOM` per second.
pub fn ease_distance(distance: f32, target: f32, dt: f32) -> f32 {
    ease(distance.max(1e-3).ln(), target.max(1e-3).ln(), dt, MAX_ZOOM).exp()
}

/// Turns the pitch towards `target`, at most `MAX_TILT` radians per second.
pub fn ease_pitch(pitch: f32, target: f32, dt: f32) -> f32 {
    ease(pitch, target, dt, MAX_TILT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Action, Change};
    use crate::tree::{LAYOUT_DT, Touch};

    const DT: f32 = 0.5;

    /// Files 0 to 3 in one directory, the rest spread over 60 others.
    fn tree() -> Tree {
        let mut tree = Tree::new();
        let touch = Touch {
            author: 0,
            commit: 0,
            time: 0,
            animate: false,
        };
        let hot = (0..4).map(|f| format!("hot/f{f}"));
        let rest = (0..60).flat_map(|d| (0..3).map(move |f| format!("d{}/e{d}/f{f}", d % 6)));
        for path in hot.chain(rest) {
            let change = Change {
                action: Action::Add,
                path,
            };
            tree.apply(&change, touch);
        }
        for _ in 0..2000 {
            tree.update(LAYOUT_DT);
        }
        tree
    }

    /// Plays `seconds` from `from`, `DT` a frame, with `glowing` files glowing: with `busy`,
    /// one more starts glowing every other frame. Returns the framings.
    fn play(
        director: &mut Director,
        tree: &Tree,
        glowing: &[usize],
        busy: bool,
        from: f64,
        seconds: f64,
    ) -> Vec<Framing> {
        let mut scene = Scene::default();
        let authors = Authors::default();
        let frames = (seconds / DT as f64) as usize;
        (0..frames)
            .map(|i| {
                let shown = if busy && i % 2 == 0 {
                    glowing.len() - 1
                } else {
                    glowing.len()
                };
                scene.glowing.clear();
                scene.glowing.extend_from_slice(&glowing[..shown]);
                let now = from + i as f64 * DT as f64;
                director.frame(now, DT, tree, &scene, &authors)
            })
            .collect()
    }

    fn is_close(framing: &Framing) -> bool {
        framing.pitch == CLOSE_PITCH
    }

    #[test]
    fn starts_wide_on_the_whole_tree() {
        let tree = tree();
        let mut director = Director::new(CameraMode::Cinematic);
        let framings = play(&mut director, &tree, &[], false, 0.0, 30.0);
        let (center, radius) = tree.bounds();
        for f in &framings {
            assert_eq!((f.center, f.radius), (center, radius));
            assert_eq!((f.pitch, f.rotation), (WIDE_PITCH, 1.0));
        }
    }

    #[test]
    fn a_burst_gets_a_close_shot_after_a_long_wide_one() {
        let tree = tree();
        let mut director = Director::new(CameraMode::Cinematic);
        let framings = play(&mut director, &tree, &[0, 1, 2, 3], true, 0.0, 60.0);
        let close: Vec<bool> = framings.iter().map(is_close).collect();
        let at = |t: f64| close[(t / DT as f64) as usize];
        // Wide for 25 s, close for 20 s, then wide again.
        assert!(!at(24.5) && at(25.0) && at(44.5) && !at(45.0));
        assert_eq!(framings[50].rotation, CLOSE_ROTATION);
    }

    #[test]
    fn close_shots_keep_most_of_the_tree_in_view() {
        let tree = tree();
        let least = (tree.bounds().1 * CLOSE_MIN_SHARE).max(CLOSE_MIN_RADIUS);
        let mut director = Director::new(CameraMode::Cinematic);
        let framings = play(&mut director, &tree, &[0, 1, 2, 3], true, 0.0, 45.0);
        assert!(framings.iter().any(is_close));
        assert!(framings.iter().all(|f| f.radius >= least * 0.999));
        // The shot leans towards the burst: its center moved off the tree's.
        let burst = tree.file_pos(0).unwrap();
        let last = framings.last().unwrap();
        assert!(last.center.distance(burst) < tree.bounds().0.distance(burst));
    }

    #[test]
    fn a_quiet_close_shot_ends_sooner() {
        let tree = tree();
        let mut director = Director::new(CameraMode::Cinematic);
        play(&mut director, &tree, &[0, 1, 2, 3], true, 0.0, 25.5);
        // The burst stops growing at 25.5 s: quiet 4 s later, back to wide 10 s into the shot.
        let framings = play(&mut director, &tree, &[0, 1, 2, 3], false, 25.5, 20.0);
        let close: Vec<bool> = framings.iter().map(is_close).collect();
        let at = |t: f64| close[((t - 25.5) / DT as f64) as usize];
        assert!(at(34.5) && !at(35.0));
    }

    #[test]
    fn scattered_activity_is_no_burst() {
        let tree = tree();
        // The file farthest from the tree's center, and the three farthest from it, at the
        // other end of the tree.
        let pos = |id| tree.file_pos(id).unwrap();
        let farthest_from = |p: Vec3| {
            let mut ids: Vec<usize> = (0..tree.file_count()).collect();
            ids.sort_by(|&a, &b| pos(b).distance(p).total_cmp(&pos(a).distance(p)));
            ids
        };
        let one = farthest_from(tree.bounds().0)[0];
        let others = farthest_from(pos(one));
        let far = [one, others[0], others[1], others[2]];
        let mut scene = Scene::default();
        scene.glowing.extend_from_slice(&far[..3]);
        let spread = burst(&tree, &scene);
        assert!(spread.is_some_and(|(_, r)| r >= tree.bounds().1 * BURST_SHARE));
        let mut director = Director::new(CameraMode::Cinematic);
        let framings = play(&mut director, &tree, &far, true, 0.0, 40.0);
        assert!(!framings.iter().any(is_close));
    }

    #[test]
    fn a_burst_needs_enough_files() {
        let tree = tree();
        let mut scene = Scene::default();
        scene.glowing.extend_from_slice(&[0, 1]);
        assert!(burst(&tree, &scene).is_none());
        scene.glowing.push(2);
        let (center, radius) = burst(&tree, &scene).unwrap();
        let positions: Vec<Vec3> = (0..3).map(|id| tree.file_pos(id).unwrap()).collect();
        let mean = positions.iter().sum::<Vec3>() / 3.0;
        assert!(center.distance(mean) < 1e-4);
        let farthest = positions
            .iter()
            .map(|p| p.distance(mean))
            .fold(0.0, f32::max);
        assert!((radius - farthest).abs() < 1e-4);
        // Deleted files don't count.
        scene.glowing[2] = 10_000;
        assert!(burst(&tree, &scene).is_none());
    }

    #[test]
    fn the_user_takes_the_camera_for_a_while() {
        let mut director = Director::new(CameraMode::Cinematic);
        assert!(director.directing(0.0));
        director.input(5.0);
        assert!(!director.directing(5.0 + IDLE_SECONDS));
        assert!(director.directing(5.1 + IDLE_SECONDS));
        let orbit = Director::new(CameraMode::Orbit);
        assert!(!orbit.directing(1000.0));
    }

    #[test]
    fn input_drops_the_smoothed_target() {
        let tree = tree();
        let mut director = Director::new(CameraMode::Cinematic);
        play(&mut director, &tree, &[0, 1, 2, 3], true, 0.0, 30.0);
        director.input(30.0);
        assert!(director.target.is_none());
    }

    #[test]
    fn ease_approaches_without_overshooting() {
        let mut value = 0.0;
        for _ in 0..600 {
            let next = ease(value, 1.0, 1.0 / 60.0, 10.0);
            assert!(next > value && next <= 1.0);
            value = next;
        }
        // Ten seconds is four time constants.
        assert!(value > 0.97);
    }

    #[test]
    fn ease_is_capped() {
        assert!((ease(0.0, 1000.0, 0.1, 0.2) - 0.02).abs() < 1e-6);
        assert!((ease(0.0, -1000.0, 0.1, 0.2) + 0.02).abs() < 1e-6);
        assert!((ease_pitch(0.0, -1.5, 1.0) + MAX_TILT).abs() < 1e-6);
        assert_eq!(ease(3.0, 3.0, 0.1, 0.2), 3.0);
    }

    #[test]
    fn zooming_in_and_out_feel_the_same() {
        let dt = 1.0 / 60.0;
        let inward = ease_distance(100.0, 10.0, dt) / 100.0;
        let outward = ease_distance(10.0, 100.0, dt) / 10.0;
        assert!((inward * outward - 1.0).abs() < 1e-4);
        // At most `MAX_ZOOM` of the distance a second.
        assert!(ease_distance(100.0, 1.0, 1.0) >= 100.0 * (-MAX_ZOOM).exp() * 0.999);
        assert!(ease_distance(100.0, 1e6, 1.0) <= 100.0 * MAX_ZOOM.exp() * 1.001);
    }

    #[test]
    fn panning_is_capped_by_the_view_distance() {
        let to = Vec3::new(1000.0, 0.0, 0.0);
        let moved = ease_focus(Vec3::ZERO, to, 50.0, 1.0);
        assert!((moved.length() - MAX_PAN * 50.0).abs() < 1e-3);
        assert!(moved.y == 0.0 && moved.x > 0.0);
        // A small step isn't capped: the usual exponential ease.
        let near = ease_focus(Vec3::ZERO, Vec3::X, 50.0, 0.1);
        let expected = 1.0 - (-0.1 / MOVE_SECONDS).exp();
        assert!((near.x - expected).abs() < 1e-6);
    }
}
