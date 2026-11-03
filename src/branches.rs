//! `--branches` (`B`): side branches, rebuilt from the commit graph. Git doesn't record which
//! branch a commit was on, so the main line is HEAD's first-parent chain and everything a merge
//! on it brings in is that merge's branch (`git::Commit::merged_by`). The tree then only shows
//! the main line: a side-branch change appears as a ghost floating right above the file it
//! changes, tied to it by a thread, until its merge, when the branch drops into the tree and its
//! changes land. Whatever floats is work not merged yet.

use bevy::prelude::*;

use crate::files::FileInstance;
use crate::git::{Action, Commit};
use crate::pace::glow;
use crate::tree::{Tree, hash};
use crate::{
    Authors, FILE_EMISSIVE, GLOW_EMISSIVE, GLOW_GROWTH, History, Palette, action_color, colors,
};

/// Ghosts float above their files by this share of the tree's radius (at least `MIN_LIFT`),
/// each branch a little higher (up to `LIFT_SPREAD` more, picked by its merge) so that two
/// branches over the same files don't mingle.
const LIFT: f32 = 0.08;
const LIFT_SPREAD: f32 = 0.5;
const MIN_LIFT: f32 = 4.0;
/// At the merge, a branch's ghosts flow into the tree over this long.
const FLOW_SECONDS: f64 = 1.5;
/// Ghosts are smaller and dimmer than files, and a faint line ties each to its file.
const GHOST_SCALE: f32 = 0.8;
const GHOST_DIM: f32 = 0.55;
const GHOST_LINE_ALPHA: f32 = 0.12;
/// A deletion's ghost keeps this much of the deletion color.
const DELETE_TINT: f32 = 0.6;
/// Where each ghost's file is gets looked up again every this many frames (the tree moves
/// slowly; looking up every ghost every frame would cost more than drawing them).
const REFRESH_FRAMES: usize = 30;

/// The side branches not merged yet, at the history's current position.
#[derive(Resource, Default)]
pub struct Branches {
    pub list: Vec<Branch>,
}

pub struct Branch {
    /// The main-line commit that merges it.
    merge: usize,
    /// Its extra height, from 0 to 1 (times `LIFT_SPREAD`).
    level: f32,
    /// Its first author's color, for the lines to the files.
    pub color: LinearRgba,
    pub ghosts: Vec<Ghost>,
    /// Ghost index by path: a file changed twice on the branch has one ghost.
    by_path: std::collections::HashMap<String, usize>,
    /// When the merge was reached: the branch is flowing into the tree.
    flowing: Option<f64>,
    /// Next ghost whose place to look up.
    cursor: usize,
}

pub struct Ghost {
    path: String,
    kind: usize,
    pub action: Action,
    pub author: usize,
    /// App time of its last change, for the glow and the beam.
    pub at: f64,
    /// Where its file is (or would appear) in the tree: a directory and an offset from it.
    pub dir: usize,
    offset: Vec3,
    /// This frame: its file's position, and its own.
    pub target: Vec3,
    pub pos: Vec3,
}

impl Branch {
    /// 0 before the merge, easing to 1 as it flows into the tree.
    fn flow(&self, now: f64) -> f32 {
        self.flowing.map_or(0.0, |t| {
            let f = ((now - t) / FLOW_SECONDS).clamp(0.0, 1.0) as f32;
            f * f * (3.0 - 2.0 * f)
        })
    }
}

impl Branches {
    /// A side-branch change was reached: shows it on its branch (merged by commit `merge`).
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        tree: &Tree,
        commits: &[Commit],
        merge: usize,
        commit: usize,
        change: usize,
        author: usize,
        palette: &mut Palette,
        at: f64,
    ) {
        let change = &commits[commit].changes[change];
        let b = match self.list.iter().position(|b| b.merge == merge) {
            Some(b) => b,
            None => {
                self.list.push(Branch {
                    merge,
                    level: (hash(&commits[merge].sha) % 1000) as f32 / 1000.0,
                    color: LinearRgba::from(colors::name_color(&commits[commit].email)),
                    ghosts: Vec::new(),
                    by_path: Default::default(),
                    flowing: None,
                    cursor: 0,
                });
                self.list.len() - 1
            }
        };
        let branch = &mut self.list[b];
        if let Some(&g) = branch.by_path.get(&change.path) {
            let ghost = &mut branch.ghosts[g];
            (ghost.action, ghost.author, ghost.at) = (change.action, author, at);
            return;
        }
        let name = change.path.rsplit('/').next().unwrap_or(&change.path);
        let (dir, offset) = tree.place(&change.path);
        let target = tree.dirs[dir].pos + offset;
        branch
            .by_path
            .insert(change.path.clone(), branch.ghosts.len());
        branch.ghosts.push(Ghost {
            path: change.path.clone(),
            kind: palette.kind(name),
            action: change.action,
            author,
            at,
            dir,
            offset,
            target,
            pos: target,
        });
    }

    /// Commit `merge` was reached: its branch flows into the tree.
    pub fn merge(&mut self, merge: usize, now: f64) {
        if let Some(branch) = self.list.iter_mut().find(|b| b.merge == merge) {
            branch.flowing = Some(now);
        }
    }

    /// The branches open at the history's position, after a jump: what was reached but not merged.
    pub fn rebuild(
        &mut self,
        history: &History,
        tree: &Tree,
        authors: &Authors,
        palette: &mut Palette,
    ) {
        self.list.clear();
        if !history.reading.branches {
            return;
        }
        let commits = &history.commits;
        for (c, commit) in commits.iter().enumerate().take(history.next) {
            if let Some(merge) = commit.merged_by.filter(|&m| m >= history.next) {
                let author = authors.by_email(&commit.email);
                for k in 0..commit.changes.len() {
                    self.add(
                        tree,
                        commits,
                        merge,
                        c,
                        k,
                        author,
                        palette,
                        f64::NEG_INFINITY,
                    );
                }
            }
        }
    }

    /// Places the ghosts, and drops the branches done flowing.
    pub fn update(&mut self, tree: &Tree, now: f64) {
        self.list
            .retain(|b| b.flowing.is_none_or(|t| now - t < FLOW_SECONDS));
        let lift = (tree.bounds().1 * LIFT).max(MIN_LIFT);
        for branch in &mut self.list {
            let up = Vec3::Y * lift * (1.0 + LIFT_SPREAD * branch.level) * (1.0 - branch.flow(now));
            // A flowing branch follows its files closely: new ones just got their real place.
            let n = branch.ghosts.len();
            let refresh = if branch.flowing.is_some() {
                n
            } else {
                n.div_ceil(REFRESH_FRAMES)
            };
            for _ in 0..refresh {
                let ghost = &mut branch.ghosts[branch.cursor % n];
                (ghost.dir, ghost.offset) = tree.place(&ghost.path);
                branch.cursor = (branch.cursor + 1) % n;
            }
            for ghost in &mut branch.ghosts {
                ghost.target = tree.dirs[ghost.dir].pos + ghost.offset;
                ghost.pos = ghost.target + up;
            }
        }
    }

    /// The ghosts, as file-layer instances.
    pub fn instances(
        &self,
        now: f64,
        fade: f64,
        colors: &[LinearRgba],
        out: &mut Vec<FileInstance>,
    ) {
        for branch in &self.list {
            let shrink = 1.0 - 0.5 * branch.flow(now);
            for ghost in &branch.ghosts {
                let glow = glow(now, ghost.at, fade);
                let tint = if ghost.action == Action::Delete {
                    DELETE_TINT
                } else {
                    0.0
                };
                let color = (colors[ghost.kind] * GHOST_DIM)
                    .mix(&action_color(ghost.action), glow.max(tint));
                out.push(FileInstance {
                    position: ghost.pos,
                    scale: GHOST_SCALE * (1.0 + GLOW_GROWTH * glow) * shrink,
                    color: [
                        color.red,
                        color.green,
                        color.blue,
                        FILE_EMISSIVE + GLOW_EMISSIVE * glow,
                    ],
                });
            }
        }
    }

    /// The faint lines from each ghost to its file.
    pub fn lines(&self, gizmos: &mut Gizmos) {
        for branch in &self.list {
            let color = branch.color.with_alpha(GHOST_LINE_ALPHA);
            for ghost in &branch.ghosts {
                gizmos.line(ghost.pos, ghost.target, color);
            }
        }
    }

    pub fn ghosts(&self) -> impl Iterator<Item = &Ghost> {
        self.list.iter().flat_map(|b| &b.ghosts)
    }
}

/// Keeps the ghosts where their files are.
pub fn place_ghosts(time: Res<Time>, tree: Res<Tree>, mut branches: ResMut<Branches>) {
    if !branches.list.is_empty() {
        branches.update(&tree, time.elapsed_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Change;
    use crate::tree::Touch;

    fn tree(paths: &[&str]) -> Tree {
        let mut tree = Tree::new();
        let touch = Touch {
            author: 0,
            commit: 0,
            time: 0,
            animate: false,
        };
        for path in paths {
            let path = path.to_string();
            tree.apply(
                &Change {
                    action: Action::Add,
                    path,
                },
                touch,
            );
        }
        tree.settle(10);
        tree
    }

    /// A branch merged by commit `merge`, with a ghost for each path.
    fn branch(tree: &Tree, merge: usize, paths: &[(&str, Action)]) -> Branch {
        let ghosts = paths
            .iter()
            .map(|&(path, action)| {
                let (dir, offset) = tree.place(path);
                let target = tree.dirs[dir].pos + offset;
                Ghost {
                    path: path.to_string(),
                    kind: 0,
                    action,
                    author: 0,
                    at: f64::NEG_INFINITY,
                    dir,
                    offset,
                    target,
                    pos: target,
                }
            })
            .collect();
        Branch {
            merge,
            level: 0.5,
            color: LinearRgba::WHITE,
            ghosts,
            by_path: Default::default(),
            flowing: None,
            cursor: 0,
        }
    }

    #[test]
    fn flow_eases_from_zero_to_one() {
        let tree = tree(&["a/x"]);
        let mut b = branch(&tree, 0, &[]);
        assert_eq!(b.flow(100.0), 0.0);
        b.flowing = Some(10.0);
        assert_eq!(b.flow(5.0), 0.0);
        assert_eq!(b.flow(10.0), 0.0);
        assert_eq!(b.flow(10.0 + FLOW_SECONDS / 2.0), 0.5);
        assert_eq!(b.flow(10.0 + FLOW_SECONDS), 1.0);
        assert_eq!(b.flow(1000.0), 1.0);
        let steps: Vec<f32> = (0..=30).map(|i| b.flow(10.0 + i as f64 * 0.05)).collect();
        assert!(steps.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn reaching_a_merge_starts_its_branch_flowing() {
        let tree = tree(&["a/x"]);
        let mut branches = Branches {
            list: vec![branch(&tree, 5, &[]), branch(&tree, 9, &[])],
        };
        branches.merge(9, 3.0);
        branches.merge(42, 3.0);
        assert_eq!(branches.list[0].flowing, None);
        assert_eq!(branches.list[1].flowing, Some(3.0));
    }

    #[test]
    fn ghosts_float_above_their_files() {
        let tree = tree(&["a/x", "a/y", "b/z"]);
        let paths = [("a/x", Action::Modify), ("b/new", Action::Add)];
        let mut branches = Branches {
            list: vec![branch(&tree, 1, &paths)],
        };
        branches.update(&tree, 0.0);
        let lift = (tree.bounds().1 * LIFT).max(MIN_LIFT) * (1.0 + LIFT_SPREAD * 0.5);
        for ghost in branches.ghosts() {
            assert!((ghost.pos - ghost.target - Vec3::Y * lift).length() < 1e-4);
        }
        // An existing file's ghost is right above it; a new one above where it will appear.
        let ghost = &branches.list[0].ghosts[0];
        assert_eq!(ghost.target, tree.file_pos(0).unwrap());
        let (dir, offset) = tree.place("b/new");
        assert_eq!(
            branches.list[0].ghosts[1].target,
            tree.dirs[dir].pos + offset
        );
    }

    #[test]
    fn merged_branches_drop_into_the_tree_then_go() {
        let tree = tree(&["a/x"]);
        let mut branches = Branches {
            list: vec![branch(&tree, 1, &[("a/x", Action::Modify)])],
        };
        branches.merge(1, 10.0);
        branches.update(&tree, 10.0 + FLOW_SECONDS * 0.999);
        let ghost = &branches.list[0].ghosts[0];
        assert!(ghost.pos.distance(ghost.target) < 0.01);
        branches.update(&tree, 10.0 + FLOW_SECONDS);
        assert!(branches.list.is_empty());
    }

    #[test]
    fn ghosts_are_drawn_dim_and_shrink_as_they_land() {
        let tree = tree(&["a/x"]);
        let paths = [("a/x", Action::Modify), ("a/y", Action::Delete)];
        let mut branches = Branches {
            list: vec![branch(&tree, 1, &paths)],
        };
        let colors = [LinearRgba::WHITE];
        let mut out = Vec::new();
        branches.instances(0.0, 1.0, &colors, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].scale, GHOST_SCALE);
        assert_eq!(
            out[0].color,
            [GHOST_DIM, GHOST_DIM, GHOST_DIM, FILE_EMISSIVE]
        );
        // A deletion keeps some of the deletion color.
        let tinted =
            (LinearRgba::WHITE * GHOST_DIM).mix(&action_color(Action::Delete), DELETE_TINT);
        assert_eq!(out[1].color[..3], [tinted.red, tinted.green, tinted.blue]);
        // Just changed: it glows and swells; landing: it shrinks to half.
        branches.list[0].ghosts[0].at = 0.0;
        out.clear();
        branches.instances(0.0, 1.0, &colors, &mut out);
        assert_eq!(out[0].scale, GHOST_SCALE * (1.0 + GLOW_GROWTH));
        branches.list[0].ghosts[0].at = f64::NEG_INFINITY;
        branches.merge(1, 0.0);
        out.clear();
        branches.instances(FLOW_SECONDS, 1.0, &colors, &mut out);
        assert_eq!(out[0].scale, GHOST_SCALE * 0.5);
    }
}
