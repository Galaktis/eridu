//! The ziggurat view (`--view ziggurat`, `V`): time as the vertical axis. Every path gets a
//! spot on a square spiral, in the order files first appeared, so the oldest code sits in the
//! middle and each square ring is younger. The history is cut into periods, each a layer of the
//! files alive at its end, stacked with the present at the base and the past above: a growing
//! repository builds a stepped pyramid. A file's life is a column through the layers, a part
//! of the code that died is a hole, and a rewrite shows as a column that stops and a ring that
//! starts. The base is live (the files as they are now, glowing when touched); each time a
//! period ends it rises as a new layer and the tower grows by one step.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::files::FileInstance;
use crate::git::{Action, Change, Commit};
use crate::tree::{FileId, Tree};
use crate::{FILE_EMISSIVE, Palette};

/// All layers together draw about this many files: their number adapts to the repository.
const LAYER_BUDGET: usize = 80_000;
const MIN_LAYERS: usize = 8;
const MAX_LAYERS: usize = 64;
/// Live files are counted this many times over the history, to size the layers.
const SAMPLES: usize = 256;
/// Distance between neighbouring spots, and the files' size there (they sit closer than in
/// the tree, so the layers read as surfaces).
const SPACING: f32 = 0.5;
pub const FILE_SCALE: f32 = 1.4;
/// The tower is about this high, as a share of its base's width…
const HEIGHT_SHARE: f32 = 0.5;
/// …with layers at least this far apart.
const MIN_LAYER_HEIGHT: f32 = 0.6;
/// Past layers glow this much less than the live base.
const PAST_EMISSIVE: f32 = 0.5;
/// The camera frames the tower as a sphere this much bigger than its bounding one: the tree's
/// framing assumes a loose sphere, the tower's is tight.
const FRAME_MARGIN: f32 = 1.5;
/// A period ending lifts the tower one step over about this long.
const RISE_SECONDS: f32 = 0.8;
/// No spot (yet) for this file id.
const NONE: u32 = u32::MAX;

/// `--view`: how the history is drawn.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Tree,
    Ziggurat,
}

#[derive(Resource)]
pub struct Ziggurat {
    /// Each path's spot, by order of first appearance.
    spots: Vec<Vec2>,
    by_path: HashMap<String, u32>,
    /// When each spot's file first appeared (in commit order, so nearly sorted).
    born: Vec<i64>,
    /// The past layers: when each ends, and its files as (spot, kind).
    layers: Vec<(i64, Vec<(u32, u32)>)>,
    layer_height: f32,
    /// The tree's file ids → spots, found on first sight (forgotten after jumps, which reuse ids).
    file_spots: Vec<u32>,
    /// Reused while looking a path up.
    path: String,
    /// Past layers shown, easing towards the count of ended periods so the tower rises calmly.
    shown: f32,
    bounds: (Vec3, f32),
}

/// Where the `i`-th spot of a square spiral is: the center, then rings of 8, 16, 24… cells.
fn square_spot(i: usize) -> Vec2 {
    if i == 0 {
        return Vec2::ZERO;
    }
    let ring = (((i + 1) as f64).sqrt() - 1.0) / 2.0;
    let n = ring.ceil() as i64;
    let k = i as i64 - (2 * n - 1).pow(2);
    let (side, t) = (k / (2 * n), k % (2 * n));
    let (x, y) = match side {
        0 => (n, -n + 1 + t),
        1 => (n - 1 - t, n),
        2 => (-n, n - 1 - t),
        _ => (-n + 1 + t, -n),
    };
    Vec2::new(x as f32, y as f32) * SPACING
}

/// Half the side of the square spiral holding `count` spots.
fn half_side(count: usize) -> f32 {
    ((count.max(1) as f32).sqrt() / 2.0).ceil() * SPACING
}

/// What the ziggurat reads of a commit: a trait, so tests can make histories without git.
pub trait Moment {
    fn time(&self) -> i64;
    fn changes(&self) -> &[Change];
}

impl Moment for Commit {
    fn time(&self) -> i64 {
        self.time
    }

    fn changes(&self) -> &[Change] {
        &self.changes
    }
}

impl Ziggurat {
    /// Reads the whole history once: spots, then the layers.
    pub fn build(commits: &[impl Moment], palette: &mut Palette) -> Self {
        let first = commits[0].time();
        let span = (commits[commits.len() - 1].time() - first).max(1);
        let mut by_path: HashMap<String, u32> = HashMap::new();
        let mut born = Vec::new();
        let mut kinds = Vec::new();
        // First pass: the spots, and how many files live on average.
        let (mut alive, mut live, mut sampled, mut sample) = (Vec::new(), 0usize, 0usize, 0usize);
        let sample_time = |s: usize| first + span * s as i64 / SAMPLES as i64;
        for commit in commits {
            while sample < SAMPLES && commit.time() > sample_time(sample) {
                sampled += live;
                sample += 1;
            }
            for change in commit.changes() {
                let spot = match by_path.get(&change.path) {
                    Some(&spot) => spot,
                    None => {
                        let spot = born.len() as u32;
                        born.push(commit.time());
                        kinds.push(
                            palette.kind(change.path.rsplit('/').next().unwrap_or(&change.path))
                                as u32,
                        );
                        alive.push(false);
                        by_path.insert(change.path.clone(), spot);
                        spot
                    }
                };
                set(&mut alive, &mut live, spot, change.action);
            }
        }
        let average = (sampled / sample.max(1)).max(1);
        let count = (LAYER_BUDGET / average).clamp(MIN_LAYERS, MAX_LAYERS);

        // Second pass: the files alive at the end of each period but the last (the live base).
        alive.iter_mut().for_each(|a| *a = false);
        let ends: Vec<i64> = (1..count)
            .map(|j| first + span * j as i64 / count as i64)
            .collect();
        let mut layers: Vec<(i64, Vec<(u32, u32)>)> = Vec::with_capacity(ends.len());
        let snapshot = |alive: &[bool]| {
            (0..alive.len() as u32)
                .filter(|&s| alive[s as usize])
                .map(|s| (s, kinds[s as usize]))
                .collect()
        };
        for commit in commits {
            while layers.len() < ends.len() && commit.time() > ends[layers.len()] {
                layers.push((ends[layers.len()], snapshot(&alive)));
            }
            for change in commit.changes() {
                set(&mut alive, &mut live, by_path[&change.path], change.action);
            }
        }
        while layers.len() < ends.len() {
            layers.push((ends[layers.len()], snapshot(&alive)));
        }
        let width = 2.0 * half_side(born.len());
        let files: usize = layers.iter().map(|(_, l)| l.len()).sum();
        info!(
            "ziggurat: {} layers, {files} files in them, {} paths",
            layers.len() + 1,
            born.len()
        );
        Self {
            spots: (0..born.len()).map(square_spot).collect(),
            by_path,
            born,
            layer_height: (width * HEIGHT_SHARE / count as f32).max(MIN_LAYER_HEIGHT),
            layers,
            file_spots: Vec::new(),
            path: String::new(),
            shown: 0.0,
            bounds: (Vec3::ZERO, 1.0),
        }
    }

    /// Where file `id` of the tree is drawn: its spot on the live base.
    pub fn pos(&self, id: FileId) -> Option<Vec3> {
        let spot = *self.file_spots.get(id).filter(|&&s| s != NONE)?;
        let p = self.spots[spot as usize];
        Some(Vec3::new(p.x, 0.0, p.y))
    }

    /// Center and radius of the tower as shown.
    pub fn bounds(&self) -> (Vec3, f32) {
        self.bounds
    }

    /// The tree's file ids now stand for other files: look them up again.
    pub fn forget_files(&mut self) {
        self.file_spots.clear();
    }

    /// Finds the spots of the tree's files not seen yet.
    pub fn map_files(&mut self, tree: &Tree) {
        for dir in tree.dirs.iter().filter(|d| d.alive) {
            for file in &dir.files {
                if self.file_spots.len() <= file.id {
                    self.file_spots.resize(file.id + 1, NONE);
                }
                if self.file_spots[file.id] == NONE {
                    self.path.clear();
                    if !dir.path.is_empty() {
                        self.path.push_str(&dir.path);
                        self.path.push('/');
                    }
                    self.path.push_str(&file.name);
                    self.file_spots[file.id] =
                        self.by_path.get(&self.path).copied().unwrap_or(NONE);
                }
            }
        }
    }

    /// The past layers' files at history time `time`, as instances; eases the tower's rise.
    pub fn past_instances(
        &mut self,
        time: f64,
        dt: f32,
        colors: &[LinearRgba],
        out: &mut Vec<FileInstance>,
    ) {
        let ended = self
            .layers
            .partition_point(|(end, _)| (*end as f64) <= time);
        let target = ended as f32;
        // A jump shows its layers at once; playback lifts them one step at a time.
        self.shown = if (target - self.shown).abs() > 1.5 {
            target
        } else {
            self.shown + (target - self.shown) * (dt / RISE_SECONDS).min(1.0)
        };
        for (j, (_, files)) in self.layers[..ended].iter().enumerate() {
            let y = (self.shown - j as f32).max(0.0) * self.layer_height;
            for &(spot, kind) in files {
                let (p, c) = (self.spots[spot as usize], colors[kind as usize]);
                out.push(FileInstance {
                    position: Vec3::new(p.x, y, p.y),
                    scale: FILE_SCALE,
                    color: [c.red, c.green, c.blue, FILE_EMISSIVE * PAST_EMISSIVE],
                });
            }
        }
        let born = self.born.partition_point(|&t| (t as f64) <= time).max(1);
        let (half, height) = (half_side(born), self.shown * self.layer_height);
        let radius = (2.0 * half * half + height * height / 4.0).sqrt() * FRAME_MARGIN;
        self.bounds = (Vec3::new(0.0, height / 2.0, 0.0), radius);
    }
}

/// Marks a spot alive or not after a change, keeping the live count.
fn set(alive: &mut [bool], live: &mut usize, spot: u32, action: Action) {
    let now = action != Action::Delete;
    let was = std::mem::replace(&mut alive[spot as usize], now);
    match (was, now) {
        (false, true) => *live += 1,
        (true, false) => *live -= 1,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_fills_square_rings() {
        // Every cell of the 5×5 square, once.
        let mut cells: Vec<(i32, i32)> = (0..25)
            .map(|i| {
                let p = square_spot(i) / SPACING;
                (p.x.round() as i32, p.y.round() as i32)
            })
            .collect();
        cells.sort();
        cells.dedup();
        assert_eq!(cells.len(), 25);
        assert!(cells.iter().all(|&(x, y)| x.abs() <= 2 && y.abs() <= 2));
    }

    /// A commit: its time and its changes.
    struct At(i64, Vec<Change>);

    impl Moment for At {
        fn time(&self) -> i64 {
            self.0
        }

        fn changes(&self) -> &[Change] {
            &self.1
        }
    }

    fn at(time: i64, changes: &[(Action, &str)]) -> At {
        let changes = changes
            .iter()
            .map(|&(action, path)| Change {
                action,
                path: path.to_string(),
            })
            .collect();
        At(time, changes)
    }

    fn build(commits: &[At]) -> Ziggurat {
        Ziggurat::build(commits, &mut Palette::default())
    }

    /// `files` added at time 0, then quiet commits until `end`.
    fn steady(files: usize, end: i64) -> Vec<At> {
        let names: Vec<String> = (0..files).map(|i| format!("f{i}.rs")).collect();
        let adds: Vec<(Action, &str)> = names.iter().map(|n| (Action::Add, n.as_str())).collect();
        let mut commits = vec![at(0, &adds)];
        commits.extend((1..=10).map(|i| at(end * i / 10, &[])));
        commits
    }

    /// (x, y) of spot `i` in cells.
    fn cell(i: usize) -> (i32, i32) {
        let p = square_spot(i) / SPACING;
        (p.x.round() as i32, p.y.round() as i32)
    }

    #[test]
    fn spiral_spots_are_unique_and_ring_by_ring() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..2000 {
            let (x, y) = cell(i);
            assert!(seen.insert((x, y)), "spot {i} taken twice");
            // Ring n holds spots (2n − 1)² up to (2n + 1)² − 1.
            let n = (((i + 1) as f64).sqrt() - 1.0) / 2.0;
            assert_eq!(x.abs().max(y.abs()), n.ceil() as i32, "spot {i}");
            // Each spot is next to the previous one, so a ring grows as one band.
            if i > 0 {
                let (px, py) = cell(i - 1);
                assert!((x - px).abs() <= 1 && (y - py).abs() <= 1, "spot {i} jumps");
            }
        }
    }

    #[test]
    fn half_side_holds_every_spot() {
        for count in 1..2000 {
            let half = half_side(count);
            let p = square_spot(count - 1);
            assert!(p.x.abs() <= half && p.y.abs() <= half, "{count} spots");
        }
    }

    #[test]
    fn layers_adapt_to_the_files_alive() {
        // 2,000 files alive: 80,000 / 2,000 = 40 layers, the live base and 39 past ones.
        let zig = build(&steady(2000, 1000));
        assert_eq!(zig.layers.len(), 39);
        assert!(zig.layers.iter().all(|(_, files)| files.len() == 2000));
        // A few files: as many layers as allowed; very many: as few.
        assert_eq!(build(&steady(10, 1000)).layers.len(), MAX_LAYERS - 1);
        assert_eq!(build(&steady(20_000, 1000)).layers.len(), MIN_LAYERS - 1);
    }

    #[test]
    fn layers_end_in_order_over_the_span() {
        let zig = build(&steady(10, 6400));
        let ends: Vec<i64> = zig.layers.iter().map(|(end, _)| *end).collect();
        assert!(ends.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(ends[0], 100);
        assert!(*ends.last().unwrap() < 6400);
    }

    #[test]
    fn spots_go_by_first_appearance() {
        let commits = [
            at(0, &[(Action::Add, "a"), (Action::Add, "b")]),
            at(10, &[(Action::Delete, "a"), (Action::Add, "c")]),
            at(20, &[(Action::Add, "a"), (Action::Modify, "b")]),
        ];
        let zig = build(&commits);
        // A file deleted and added again keeps its spot.
        assert_eq!(
            (zig.by_path["a"], zig.by_path["b"], zig.by_path["c"]),
            (0, 1, 2)
        );
        assert_eq!(zig.born, [0, 0, 10]);
        assert_eq!(zig.spots.len(), 3);
        assert_eq!(zig.spots[0], Vec2::ZERO);
    }

    #[test]
    fn layers_hold_the_files_alive_at_their_end() {
        // a lives in the first half only, b all along, c in the second half.
        let commits = [
            at(0, &[(Action::Add, "a"), (Action::Add, "b")]),
            at(500, &[(Action::Delete, "a"), (Action::Add, "c")]),
            at(1000, &[(Action::Modify, "b")]),
        ];
        let zig = build(&commits);
        for (end, files) in &zig.layers {
            let spots: Vec<u32> = files.iter().map(|&(spot, _)| spot).collect();
            if *end < 500 {
                assert_eq!(spots, [0, 1], "at {end}");
            } else {
                assert_eq!(spots, [1, 2], "at {end}");
            }
        }
    }

    #[test]
    fn same_type_files_share_a_kind() {
        let commits = [at(
            0,
            &[
                (Action::Add, "src/a.rs"),
                (Action::Add, "b.md"),
                (Action::Add, "src/c.rs"),
            ],
        )];
        let mut palette = Palette::default();
        let zig = Ziggurat::build(&commits[..], &mut palette);
        // A single commit: every layer holds all its files.
        assert!(zig.layers.iter().all(|(_, files)| files.len() == 3));
        let kinds: Vec<u32> = zig.layers[0].1.iter().map(|&(_, kind)| kind).collect();
        assert_eq!(kinds[0], kinds[2]);
        assert_ne!(kinds[0], kinds[1]);
        assert_eq!(palette.colors.len(), 2);
    }

    #[test]
    fn tree_files_map_to_their_spots() {
        let commits = [
            at(0, &[(Action::Add, "src/a.rs"), (Action::Add, "README.md")]),
            at(10, &[(Action::Add, "src/b.rs")]),
        ];
        let mut zig = build(&commits);
        let mut tree = Tree::new();
        let touch = crate::tree::Touch {
            author: 0,
            commit: 0,
            time: 0,
            animate: false,
        };
        for path in ["src/b.rs", "README.md", "other.txt"] {
            let path = path.to_string();
            let change = Change {
                action: Action::Add,
                path,
            };
            tree.apply(&change, touch);
        }
        zig.map_files(&tree);
        let spot = |i: usize| {
            let p = zig.spots[i];
            Some(Vec3::new(p.x, 0.0, p.y))
        };
        // Tree ids 0, 1, 2 are src/b.rs, README.md, and a path the history never had.
        assert_eq!(zig.pos(0), spot(2));
        assert_eq!(zig.pos(1), spot(1));
        assert_eq!(zig.pos(2), None);
        assert_eq!(zig.pos(3), None);
        zig.forget_files();
        assert_eq!(zig.pos(0), None);
    }

    #[test]
    fn the_tower_rises_calmly_but_jumps_at_once() {
        let mut zig = build(&steady(5, 6400));
        let colors = vec![LinearRgba::WHITE; 4];
        let mut out = Vec::new();
        // Before the first period ends: only the live base, no past layer.
        zig.past_instances(50.0, 1.0 / 60.0, &colors, &mut out);
        assert!(out.is_empty());
        // Playing past one period's end: the new layer rises over a few frames.
        zig.past_instances(150.0, 1.0 / 60.0, &colors, &mut out);
        assert_eq!(out.len(), 5);
        assert!(zig.shown > 0.0 && zig.shown < 0.1);
        // A jump to the end shows every layer at once.
        out.clear();
        zig.past_instances(6400.0, 1.0 / 60.0, &colors, &mut out);
        assert_eq!(zig.shown, zig.layers.len() as f32);
        assert_eq!(out.len(), 5 * zig.layers.len());
        // The oldest layer is on top, the newest right above the base.
        let top = out.iter().map(|f| f.position.y).fold(0.0, f32::max);
        assert!((top - zig.shown * zig.layer_height).abs() < 1e-3);
        let (center, radius) = zig.bounds();
        assert!(out.iter().all(|f| f.position.distance(center) <= radius));
    }
}
