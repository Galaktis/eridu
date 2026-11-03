use std::collections::HashMap;

use bevy::prelude::{Resource, Vec3};

use crate::git::{Action, Change};

/// Distance between neighbouring files around their directory.
const FILE_SPACING: f32 = 1.0;
/// Extra length of the branch between a directory and its parent.
const BRANCH_LENGTH: f32 = 4.0;
/// Minimum gap kept between two directories.
const PADDING: f32 = 2.0;
const SPRING: f32 = 6.0;
const REPULSION: f32 = 8.0;
const DAMPING: f32 = 0.92;
const GOLDEN_ANGLE: f32 = 2.399_963;
/// With several repositories, each is a "city": cities whose bounding spheres come closer than
/// this gap push each other away as a whole.
const CITY_GAP: f32 = 8.0;
const CITY_REPULSION: f32 = 2.0;
/// √2 − 1: a second irrational step, so file heights don't line up with their angles.
const SILVER_STEP: f32 = 0.414_213_56;
/// Layout steps run at most after a jump through history, so the tree appears already settled…
const SETTLE_STEPS: usize = 400;
/// …a few per directory created by the jump…
const SETTLE_STEPS_PER_DIR: usize = 4;
/// …stopping early once no directory moves faster than this (units per second).
const SETTLED_SPEED: f32 = 0.05;
/// Time step of the layout, matching Bevy's default fixed timestep (64 Hz).
pub const LAYOUT_DT: f32 = 1.0 / 64.0;

/// Identifies a file for its whole life, whatever slot of its directory it moves to.
pub type FileId = usize;

#[derive(Clone)]
pub struct File {
    pub id: FileId,
    pub name: String,
}

#[derive(Clone)]
pub struct Dir {
    /// Path from the repository root, `""` for the root.
    pub path: String,
    pub parent: Option<usize>,
    /// 1 + index in `Tree::cities` of the top-level directory this one is in (0: the root).
    city: usize,
    /// False when neither the directory nor any subdirectory has files left: it is hidden.
    pub alive: bool,
    pub pos: Vec3,
    vel: Vec3,
    pub files: Vec<File>,
    /// File name → slot in `files`.
    slots: HashMap<String, usize>,
}

impl Dir {
    pub fn radius(&self) -> f32 {
        1.0 + FILE_SPACING * (self.files.len() as f32).cbrt()
    }
}

/// Where the file in `slot` sits relative to its directory. Files fill a ball, evenly spread:
/// a 3D take on the sunflower pattern.
pub fn file_offset(slot: usize) -> Vec3 {
    let i = slot as f32;
    let theta = i * GOLDEN_ANGLE;
    let y = 1.0 - 2.0 * (i * SILVER_STEP).fract();
    let ring = (1.0 - y * y).sqrt();
    Vec3::new(ring * theta.cos(), y, ring * theta.sin()) * FILE_SPACING * (i + 0.5).cbrt()
}

/// What happened to files, for the view to mirror. Events name files by id; the view looks
/// up their current place when it handles them, so it always sees the latest state.
#[derive(Clone)]
pub enum Event {
    Added(FileId),
    /// `pos` is where the file was.
    Deleted {
        id: FileId,
        pos: Vec3,
        touch: Touch,
    },
    /// A commit added or modified the file.
    Touched {
        id: FileId,
        touch: Touch,
        action: Action,
    },
}

/// Who changed a file, in which commit, and whether to animate it (not when jumping).
#[derive(Clone, Copy)]
pub struct Touch {
    pub author: usize,
    pub commit: usize,
    /// The commit's unix time, for the files' heat.
    pub time: i64,
    pub animate: bool,
}

/// The repository's directory tree, laid out with a small spring simulation.
#[derive(Resource, Clone)]
pub struct Tree {
    pub dirs: Vec<Dir>,
    index: HashMap<String, usize>,
    /// File id → (directory, slot), `None` once deleted.
    places: Vec<Option<(usize, usize)>>,
    pub events: Vec<Event>,
    /// Set when a directory gained its first file or lost its last one.
    alive_dirty: bool,
    /// Several repositories, one per top-level directory: keep them apart.
    pub separate_cities: bool,
    /// Top-level directories.
    cities: Vec<usize>,
    city_push: Vec<(Vec3, f32, f32, Vec3)>,
    /// Reused buffers, so a layout step doesn't allocate.
    radii: Vec<f32>,
    force: Vec<Vec3>,
    order: Vec<usize>,
}

impl Tree {
    pub fn new() -> Self {
        let root = Dir {
            path: String::new(),
            parent: None,
            city: 0,
            alive: true,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            files: Vec::new(),
            slots: HashMap::new(),
        };
        Self {
            dirs: vec![root],
            index: HashMap::from([(String::new(), 0)]),
            places: Vec::new(),
            events: Vec::new(),
            alive_dirty: false,
            separate_cities: false,
            cities: Vec::new(),
            city_push: Vec::new(),
            radii: Vec::new(),
            force: Vec::new(),
            order: Vec::new(),
        }
    }

    /// Keeps each top-level directory (one per repository) apart from the others.
    pub fn with_cities(mut self, separate: bool) -> Self {
        self.separate_cities = separate;
        self
    }

    /// A copy of the tree's state, without pending events or scratch buffers.
    pub fn snapshot(&self) -> Self {
        let mut copy = self.clone();
        copy.events = Vec::new();
        (copy.radii, copy.force, copy.order) = (Vec::new(), Vec::new(), Vec::new());
        copy
    }

    pub fn file_count(&self) -> usize {
        self.dirs.iter().map(|d| d.files.len()).sum()
    }

    /// The directory, slot and file of a live file.
    pub fn find(&self, id: FileId) -> Option<(usize, usize, &File)> {
        let (dir, slot) = (*self.places.get(id)?)?;
        Some((dir, slot, &self.dirs[dir].files[slot]))
    }

    /// The file's path from the repository root.
    pub fn file_path(&self, id: FileId) -> Option<String> {
        let (dir, _, file) = self.find(id)?;
        let dir = &self.dirs[dir].path;
        Some(if dir.is_empty() {
            file.name.clone()
        } else {
            format!("{dir}/{}", file.name)
        })
    }

    pub fn file_pos(&self, id: FileId) -> Option<Vec3> {
        let (dir, slot) = (*self.places.get(id)?)?;
        Some(self.dirs[dir].pos + file_offset(slot))
    }

    /// Where a file is, as its directory and the offset from it; for a file the tree doesn't
    /// have (yet), somewhere around its closest live directory.
    pub fn place(&self, path: &str) -> (usize, Vec3) {
        let (mut dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        if let Some(&d) = self.index.get(dir)
            && let Some(&slot) = self.dirs[d].slots.get(name)
        {
            return (d, file_offset(slot));
        }
        // The root is always there and alive, so this ends.
        loop {
            if let Some(&d) = self.index.get(dir).filter(|&&d| self.dirs[d].alive) {
                return (d, jitter(path) * self.dirs[d].radius());
            }
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }

    /// Returns the index of the directory at `path`, creating it (and its parents) if needed.
    fn dir(&mut self, path: &str) -> usize {
        if let Some(&i) = self.index.get(path) {
            return i;
        }
        let parent_path = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        let parent = self.dir(parent_path);
        let base = self.dirs[parent].pos;
        // Spawn new branches pointing away from the grandparent so the tree grows outwards.
        let outward = self.dirs[parent].parent.map_or(Vec3::ZERO, |g| {
            (base - self.dirs[g].pos).normalize_or_zero()
        });
        let city = if parent == 0 {
            self.cities.push(self.dirs.len());
            self.cities.len()
        } else {
            self.dirs[parent].city
        };
        self.dirs.push(Dir {
            path: path.to_string(),
            parent: Some(parent),
            city,
            alive: true,
            pos: base + (outward + jitter(path)) * BRANCH_LENGTH,
            vel: Vec3::ZERO,
            files: Vec::new(),
            slots: HashMap::new(),
        });
        let dir = self.dirs.len() - 1;
        self.index.insert(path.to_string(), dir);
        dir
    }

    /// Applies one file change.
    pub fn apply(&mut self, change: &Change, touch: Touch) {
        let (dir_path, name) = change.path.rsplit_once('/').unwrap_or(("", &change.path));
        if change.action == Action::Delete {
            // Deleting a file never seen (filtered out when added) must not create its directory.
            let slot = self
                .index
                .get(dir_path)
                .and_then(|&dir| Some((dir, *self.dirs[dir].slots.get(name)?)));
            if let Some((dir, slot)) = slot {
                self.delete(dir, slot, touch);
            }
            return;
        }
        let dir = self.dir(dir_path);
        let id = match self.dirs[dir].slots.get(name).copied() {
            Some(slot) => self.dirs[dir].files[slot].id,
            None => {
                let id = self.places.len();
                let d = &mut self.dirs[dir];
                self.alive_dirty |= d.files.is_empty();
                self.places.push(Some((dir, d.files.len())));
                d.slots.insert(name.to_string(), d.files.len());
                d.files.push(File {
                    id,
                    name: name.to_string(),
                });
                self.events.push(Event::Added(id));
                id
            }
        };
        self.events.push(Event::Touched {
            id,
            touch,
            action: change.action,
        });
    }

    /// Removes a file; the directory's last file takes its slot, so only one file moves.
    fn delete(&mut self, dir: usize, slot: usize, touch: Touch) {
        let d = &mut self.dirs[dir];
        let pos = d.pos + file_offset(slot);
        let file = d.files.swap_remove(slot);
        d.slots.remove(&file.name);
        self.places[file.id] = None;
        self.events.push(Event::Deleted {
            id: file.id,
            pos,
            touch,
        });
        if let Some(moved) = d.files.get(slot) {
            d.slots.insert(moved.name.clone(), slot);
            self.places[moved.id] = Some((dir, slot));
        }
        self.alive_dirty |= d.files.is_empty();
    }

    /// Center and radius of a sphere containing the whole tree.
    pub fn bounds(&self) -> (Vec3, f32) {
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for dir in self.dirs.iter().filter(|d| d.alive) {
            let r = Vec3::splat(dir.radius());
            min = min.min(dir.pos - r);
            max = max.max(dir.pos + r);
        }
        ((min + max) / 2.0, (max - min).length() / 2.0)
    }

    /// Runs the layout until it is roughly at rest, after a jump through history that created
    /// `new_dirs` directories.
    pub fn settle(&mut self, new_dirs: usize) {
        for step in 0..SETTLE_STEPS.min(new_dirs * SETTLE_STEPS_PER_DIR) {
            self.update(LAYOUT_DT);
            let at_rest = || {
                self.dirs
                    .iter()
                    .all(|d| !d.alive || d.vel.length_squared() < SETTLED_SPEED * SETTLED_SPEED)
            };
            if step % 10 == 9 && at_rest() {
                break;
            }
        }
    }

    /// Advances the layout: branches act as springs, directories push apart when they overlap.
    pub fn update(&mut self, dt: f32) {
        if std::mem::take(&mut self.alive_dirty) {
            self.refresh_alive();
        }
        let n = self.dirs.len();
        self.radii.clear();
        self.radii.extend(self.dirs.iter().map(Dir::radius));
        self.force.clear();
        self.force.resize(n, Vec3::ZERO);
        let (dirs, radii, force) = (&self.dirs, &self.radii, &mut self.force);
        for i in (1..n).filter(|&i| dirs[i].alive) {
            let p = dirs[i].parent.unwrap_or(0);
            let delta = dirs[i].pos - dirs[p].pos;
            let dist = delta.length().max(0.01);
            let rest = radii[p] + radii[i] + BRANCH_LENGTH;
            let f = delta / dist * (dist - rest) * SPRING;
            force[i] -= f;
            force[p] += f * 0.5;
        }

        // Repulsion only acts between overlapping directories, so sweep and prune: sorted by
        // the left edge of their reach along x, each directory only meets the ones whose reach
        // starts before its own ends. Near-linear instead of comparing every pair.
        let reach = |i: usize| radii[i] + PADDING / 2.0;
        self.order.clear();
        self.order.extend((0..n).filter(|&i| dirs[i].alive));
        self.order.sort_unstable_by(|&a, &b| {
            (dirs[a].pos.x - reach(a)).total_cmp(&(dirs[b].pos.x - reach(b)))
        });
        for (k, &i) in self.order.iter().enumerate() {
            let right = dirs[i].pos.x + reach(i);
            for &j in &self.order[k + 1..] {
                if dirs[j].pos.x - reach(j) > right {
                    break;
                }
                let delta = dirs[i].pos - dirs[j].pos;
                let dist = delta.length().max(0.01);
                let gap = reach(i) + reach(j);
                if dist < gap {
                    let f = delta / dist * (gap - dist) * REPULSION;
                    force[i] += f;
                    force[j] -= f;
                }
            }
        }
        if self.separate_cities && self.cities.len() > 1 {
            self.push_cities();
        }
        // The root stays pinned at the origin.
        for (dir, f) in self
            .dirs
            .iter_mut()
            .zip(&self.force)
            .skip(1)
            .filter(|(d, _)| d.alive)
        {
            dir.vel = (dir.vel + *f * dt) * DAMPING;
            dir.pos += dir.vel * dt;
        }
    }

    /// Pushes overlapping cities apart, each as a whole (the same force on all its directories),
    /// so their branches never interleave. O(directories + cities²).
    fn push_cities(&mut self) {
        // Per city: sum of positions, directory count, radius, push.
        let stats = &mut self.city_push;
        stats.clear();
        stats.resize(self.cities.len(), (Vec3::ZERO, 0.0, 0.0, Vec3::ZERO));
        let live = || {
            self.dirs
                .iter()
                .enumerate()
                .filter(|(_, d)| d.alive && d.city > 0)
        };
        for (_, dir) in live() {
            stats[dir.city - 1].0 += dir.pos;
            stats[dir.city - 1].1 += 1.0;
        }
        for stat in stats.iter_mut().filter(|s| s.1 > 0.0) {
            stat.0 /= stat.1;
        }
        for (i, dir) in live() {
            let stat = &mut stats[dir.city - 1];
            stat.2 = stat.2.max(dir.pos.distance(stat.0) + self.radii[i]);
        }
        for a in 0..stats.len() {
            for b in a + 1..stats.len() {
                if stats[a].1 == 0.0 || stats[b].1 == 0.0 {
                    continue;
                }
                let delta = stats[b].0 - stats[a].0;
                let dist = delta.length().max(0.01);
                let gap = stats[a].2 + stats[b].2 + CITY_GAP;
                if dist < gap {
                    let push = delta / dist * (gap - dist) * CITY_REPULSION;
                    stats[a].3 -= push;
                    stats[b].3 += push;
                }
            }
        }
        for (i, dir) in self
            .dirs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.alive && d.city > 0)
        {
            self.force[i] += self.city_push[dir.city - 1].3;
        }
    }

    /// Marks which directories still have files, themselves or below them.
    fn refresh_alive(&mut self) {
        for dir in &mut self.dirs {
            dir.alive = !dir.files.is_empty();
        }
        self.dirs[0].alive = true;
        // A directory is always created after its parent, so walking backwards visits children first.
        for i in (1..self.dirs.len()).rev() {
            if let (true, Some(p)) = (self.dirs[i].alive, self.dirs[i].parent) {
                self.dirs[p].alive = true;
            }
        }
    }
}

/// FNV-1a: a stable hash, so colors and layout come out the same on every run.
pub fn hash(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// A pseudo-random offset in [-1, 1]³ derived from `path`.
fn jitter(path: &str) -> Vec3 {
    let h = hash(path);
    let unit = |shift: u32| ((h >> shift) & 0x3ff) as f32 / 511.5 - 1.0;
    Vec3::new(unit(0), unit(10), unit(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Center and radius of the live directories under the top-level directory `city`.
    fn sphere(tree: &Tree, city: usize) -> (Vec3, f32) {
        let dirs: Vec<&Dir> = tree
            .dirs
            .iter()
            .filter(|d| d.alive && d.city == city)
            .collect();
        let center = dirs.iter().map(|d| d.pos).sum::<Vec3>() / dirs.len() as f32;
        (
            center,
            dirs.iter()
                .map(|d| d.pos.distance(center) + d.radius())
                .fold(0.0, f32::max),
        )
    }

    const TOUCH: Touch = Touch {
        author: 0,
        commit: 0,
        time: 0,
        animate: false,
    };

    fn change(tree: &mut Tree, action: Action, path: &str) {
        let path = path.to_string();
        tree.apply(&Change { action, path }, TOUCH);
    }

    fn add(tree: &mut Tree, path: &str) {
        change(tree, Action::Add, path);
    }

    fn delete(tree: &mut Tree, path: &str) {
        change(tree, Action::Delete, path);
    }

    fn dir_index(tree: &Tree, path: &str) -> usize {
        tree.index[path]
    }

    /// The events since the last call, as short strings.
    fn events(tree: &mut Tree) -> Vec<String> {
        tree.events
            .drain(..)
            .map(|e| match e {
                Event::Added(id) => format!("added {id}"),
                Event::Deleted { id, .. } => format!("deleted {id}"),
                Event::Touched { id, action, .. } => format!("{action:?} {id}"),
            })
            .collect()
    }

    #[test]
    fn adding_creates_directories_and_ids() {
        let mut tree = Tree::new();
        add(&mut tree, "src/ui/a.rs");
        add(&mut tree, "src/b.rs");
        add(&mut tree, "README.md");
        // The root, src and src/ui.
        assert_eq!(tree.dirs.len(), 3);
        assert_eq!(tree.dirs[dir_index(&tree, "src/ui")].parent, Some(1));
        assert_eq!(tree.file_count(), 3);
        assert_eq!(tree.file_path(0).as_deref(), Some("src/ui/a.rs"));
        assert_eq!(tree.file_path(1).as_deref(), Some("src/b.rs"));
        assert_eq!(tree.file_path(2).as_deref(), Some("README.md"));
        assert_eq!(tree.file_path(3), None);
        assert_eq!(
            events(&mut tree),
            ["added 0", "Add 0", "added 1", "Add 1", "added 2", "Add 2"]
        );
    }

    #[test]
    fn modifying_keeps_the_id_and_only_touches() {
        let mut tree = Tree::new();
        add(&mut tree, "a/x.rs");
        events(&mut tree);
        change(&mut tree, Action::Modify, "a/x.rs");
        // An add of a file already there is a touch too.
        add(&mut tree, "a/x.rs");
        assert_eq!(events(&mut tree), ["Modify 0", "Add 0"]);
        assert_eq!(tree.file_count(), 1);
    }

    #[test]
    fn modifying_an_unseen_file_adds_it() {
        // Its add was filtered out, or came before the history shown.
        let mut tree = Tree::new();
        change(&mut tree, Action::Modify, "a/x.rs");
        assert_eq!(events(&mut tree), ["added 0", "Modify 0"]);
        assert_eq!(tree.file_path(0).as_deref(), Some("a/x.rs"));
    }

    #[test]
    fn deleting_moves_the_last_file_into_the_hole() {
        let mut tree = Tree::new();
        for name in ["d/a", "d/b", "d/c"] {
            add(&mut tree, name);
        }
        let d = dir_index(&tree, "d");
        let was = tree.file_pos(0).unwrap();
        events(&mut tree);
        delete(&mut tree, "d/a");
        let Some(Event::Deleted { id, pos, .. }) = tree.events.pop() else {
            panic!("no deletion event");
        };
        assert_eq!((id, pos), (0, was));
        assert!(tree.find(0).is_none() && tree.file_pos(0).is_none());
        // c took a's slot, b stayed: ids don't change, only one file moved.
        let found = tree
            .find(2)
            .map(|(dir, slot, f)| (dir, slot, f.name.as_str()));
        assert_eq!(found, Some((d, 0, "c")));
        assert_eq!(tree.find(1).map(|(_, slot, _)| slot), Some(1));
        assert_eq!(tree.file_pos(2), Some(tree.dirs[d].pos + file_offset(0)));
        // The slots follow: deleting c now deletes the right file.
        delete(&mut tree, "d/c");
        assert_eq!(events(&mut tree), ["deleted 2"]);
        assert_eq!(tree.file_path(1).as_deref(), Some("d/b"));
    }

    #[test]
    fn deleting_the_last_slot_moves_nothing() {
        let mut tree = Tree::new();
        add(&mut tree, "d/a");
        add(&mut tree, "d/b");
        delete(&mut tree, "d/b");
        assert_eq!(tree.find(0).map(|(_, slot, _)| slot), Some(0));
        assert_eq!(tree.file_count(), 1);
    }

    #[test]
    fn deleting_an_unseen_file_changes_nothing() {
        let mut tree = Tree::new();
        add(&mut tree, "a/x");
        events(&mut tree);
        delete(&mut tree, "b/c/y");
        delete(&mut tree, "a/z");
        assert!(tree.events.is_empty());
        // No directory made for it.
        assert_eq!(tree.dirs.len(), 2);
    }

    #[test]
    fn a_file_added_again_gets_a_new_id() {
        let mut tree = Tree::new();
        add(&mut tree, "a/x");
        delete(&mut tree, "a/x");
        add(&mut tree, "a/x");
        assert_eq!(tree.find(1).map(|(_, _, f)| f.name.as_str()), Some("x"));
        assert!(tree.find(0).is_none());
    }

    #[test]
    fn emptied_directories_die_with_their_parents() {
        let mut tree = Tree::new();
        add(&mut tree, "a/b/c/x");
        add(&mut tree, "a/y");
        add(&mut tree, "z/w");
        delete(&mut tree, "a/b/c/x");
        delete(&mut tree, "z/w");
        tree.update(LAYOUT_DT);
        let alive = |tree: &Tree, path| tree.dirs[dir_index(tree, path)].alive;
        assert!(alive(&tree, "") && alive(&tree, "a"));
        assert!(!alive(&tree, "a/b") && !alive(&tree, "a/b/c") && !alive(&tree, "z"));
        // A file coming back revives its directory and the ones above it.
        add(&mut tree, "a/b/c/x");
        tree.update(LAYOUT_DT);
        assert!(alive(&tree, "a/b") && alive(&tree, "a/b/c"));
    }

    #[test]
    fn dead_directories_stay_put() {
        let mut tree = Tree::new();
        add(&mut tree, "a/x");
        add(&mut tree, "b/y");
        delete(&mut tree, "b/y");
        tree.update(LAYOUT_DT);
        let b = dir_index(&tree, "b");
        let pos = tree.dirs[b].pos;
        for _ in 0..100 {
            tree.update(LAYOUT_DT);
        }
        assert_eq!(tree.dirs[b].pos, pos);
    }

    #[test]
    fn place_finds_files_and_the_closest_live_directory() {
        let mut tree = Tree::new();
        add(&mut tree, "a/b/x");
        add(&mut tree, "a/y");
        add(&mut tree, "a/b/z");
        let b = dir_index(&tree, "a/b");
        assert_eq!(tree.place("a/b/z"), (b, file_offset(1)));
        // Not there yet: around its directory, within its reach, always at the same spot.
        let (dir, offset) = tree.place("a/b/new");
        assert_eq!(dir, b);
        assert!(offset.length() <= tree.dirs[b].radius() * 3f32.sqrt());
        assert_eq!(tree.place("a/b/new"), (dir, offset));
        delete(&mut tree, "a/b/x");
        delete(&mut tree, "a/b/z");
        tree.update(LAYOUT_DT);
        assert_eq!(tree.place("a/b/x").0, dir_index(&tree, "a"));
        assert_eq!(tree.place("q/r/s").0, 0);
    }

    #[test]
    fn file_offsets_fill_a_ball_evenly() {
        let offsets: Vec<Vec3> = (0..300).map(file_offset).collect();
        for (i, o) in offsets.iter().enumerate() {
            // Slot i sits on the sphere holding i + ½ files' worth of volume.
            let expected = FILE_SPACING * (i as f32 + 0.5).cbrt();
            assert!((o.length() - expected).abs() < 1e-4, "slot {i}");
        }
        let closest = (0..offsets.len())
            .flat_map(|i| (i + 1..offsets.len()).map(move |j| (i, j)))
            .map(|(i, j)| offsets[i].distance(offsets[j]))
            .fold(f32::MAX, f32::min);
        assert!(closest > 0.15 * FILE_SPACING, "files overlap: {closest}");
        // And the directory's radius holds them all.
        let radius = 1.0 + FILE_SPACING * (offsets.len() as f32).cbrt();
        assert!(offsets.iter().all(|o| o.length() < radius));
    }

    #[test]
    fn radius_grows_with_the_files() {
        let mut tree = Tree::new();
        add(&mut tree, "d/a");
        let one = tree.dirs[1].radius();
        for i in 0..63 {
            add(&mut tree, &format!("d/f{i}"));
        }
        assert_eq!(tree.dirs[1].radius(), 1.0 + 4.0 * FILE_SPACING);
        assert!(one < tree.dirs[1].radius());
    }

    /// A tree of a few hundred files in nested directories.
    fn sample_tree() -> Tree {
        let mut tree = Tree::new();
        for d in 0..40 {
            for f in 0..(d % 7 + 1) {
                add(&mut tree, &format!("m{}/s{}/d{d}/f{f}.rs", d % 3, d % 5));
            }
        }
        tree
    }

    #[test]
    fn settling_separates_directories_and_keeps_the_root() {
        let mut tree = sample_tree();
        let n = tree.dirs.len();
        tree.settle(n);
        assert_eq!(tree.dirs[0].pos, Vec3::ZERO);
        for (i, a) in tree.dirs.iter().enumerate() {
            assert!(a.pos.is_finite(), "{} is lost", a.path);
            if let Some(p) = a.parent {
                let dist = a.pos.distance(tree.dirs[p].pos);
                assert!(dist < 4.0 * (a.radius() + tree.dirs[p].radius() + BRANCH_LENGTH));
            }
            for b in &tree.dirs[i + 1..] {
                let dist = a.pos.distance(b.pos);
                assert!(
                    dist > 0.5 * (a.radius() + b.radius()),
                    "{} and {} overlap: {dist}",
                    a.path,
                    b.path
                );
            }
        }
    }

    #[test]
    fn settling_stops_once_at_rest() {
        let mut tree = sample_tree();
        for _ in 0..3000 {
            tree.update(LAYOUT_DT);
        }
        let at_rest: Vec<Vec3> = tree.dirs.iter().map(|d| d.pos).collect();
        tree.settle(1000);
        let moved = tree
            .dirs
            .iter()
            .zip(&at_rest)
            .map(|(d, p)| d.pos.distance(*p))
            .fold(0.0, f32::max);
        assert!(moved < 0.5, "moved {moved}");
        // Nothing new: nothing to settle.
        let before: Vec<Vec3> = tree.dirs.iter().map(|d| d.pos).collect();
        tree.settle(0);
        assert!(tree.dirs.iter().zip(&before).all(|(d, p)| d.pos == *p));
    }

    #[test]
    fn bounds_hold_every_live_directory() {
        let mut tree = sample_tree();
        tree.settle(100);
        let (center, radius) = tree.bounds();
        for dir in tree.dirs.iter().filter(|d| d.alive) {
            assert!(dir.pos.distance(center) + dir.radius() <= radius * 1.001);
        }
    }

    #[test]
    fn snapshots_keep_files_but_not_events() {
        let mut tree = sample_tree();
        let mut copy = tree.snapshot();
        assert!(copy.events.is_empty() && !tree.events.is_empty());
        assert_eq!(copy.file_count(), tree.file_count());
        assert_eq!(copy.file_path(10), tree.file_path(10));
        // The copy goes on like the original.
        add(&mut tree, "new/file");
        add(&mut copy, "new/file");
        let id = tree.file_count() - 1;
        let place = |tree: &Tree| tree.find(id).map(|(d, s, _)| (d, s));
        assert_eq!(place(&copy), place(&tree));
    }

    #[test]
    fn hash_is_fnv1a() {
        assert_eq!(hash(""), 0x811c_9dc5);
        assert_eq!(hash("a"), 0xe40c_292c);
        assert_eq!(hash("foobar"), 0xbf9c_f968);
    }

    #[test]
    fn jitter_stays_in_the_unit_cube() {
        for i in 0..1000 {
            let j = jitter(&format!("dir{i}"));
            assert!(j.abs().max_element() <= 1.0);
        }
        assert_eq!(jitter("src"), jitter("src"));
        assert_ne!(jitter("src"), jitter("doc"));
    }

    #[test]
    fn top_level_directories_are_cities() {
        let mut tree = Tree::new().with_cities(true);
        add(&mut tree, "a/x/y");
        add(&mut tree, "b/z");
        add(&mut tree, "a/w");
        assert_eq!(tree.cities, [1, 3]);
        let city = |path| tree.dirs[dir_index(&tree, path)].city;
        assert_eq!((city(""), city("a"), city("a/x"), city("b")), (0, 1, 1, 2));
    }

    #[test]
    fn cities_end_up_apart() {
        let mut tree = Tree::new().with_cities(true);
        for repo in ["a", "b"] {
            for dir in 0..30 {
                for file in 0..10 {
                    add(&mut tree, &format!("{repo}/d{}/e{dir}/f{file}.rs", dir % 5));
                }
            }
        }
        for _ in 0..3000 {
            tree.update(LAYOUT_DT);
        }
        let ((ca, ra), (cb, rb)) = (sphere(&tree, 1), sphere(&tree, 2));
        assert!(
            ca.distance(cb) > ra + rb,
            "cities overlap: {} < {} + {}",
            ca.distance(cb),
            ra,
            rb
        );
    }
}
