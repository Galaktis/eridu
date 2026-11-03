//! Mouse and focus: hovering files and authors, clicking to focus the camera on a file (with
//! its history) or to follow an author, and `F` to cycle through active authors.
//!
//! Files aren't entities, so picking is done here: the mouse ray is tested against each live
//! directory's bounding sphere first, then against the files of the directories it crosses.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::git::{self, Action};
use crate::record::Recorder;
use crate::timeline::Scrubbing;
use crate::tree::{FileId, Tree};
use crate::ziggurat::View;
use crate::{Authors, History, Scene, playback};

/// How close (in world units) the mouse ray must pass to a file to hover it.
const PICK_RADIUS: f32 = 0.35;
/// Picking re-runs at least this often while the mouse is still (the scene moves under it).
const PICK_REFRESH_SECONDS: f32 = 0.1;
/// A press and release closer than this (in pixels) is a click, not a drag.
const CLICK_SLOP: f32 = 5.0;
/// Entries in the file history panel.
const HISTORY_ROWS: usize = 20;
const PANEL_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.75);

/// What the camera frames.
#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    #[default]
    Tree,
    Author(usize),
    File(FileId),
}

/// What the mouse is over.
#[derive(Resource, Default)]
pub struct Hover {
    pub file: Option<FileId>,
    pub author: Option<usize>,
}

impl Hover {
    /// The author whose work stands out: the hovered one, else the followed one.
    pub fn spotlight(&self, focus: Focus) -> Option<usize> {
        self.author.or(match focus {
            Focus::Author(a) => Some(a),
            _ => None,
        })
    }
}

/// Marks an author's UI label, so hovering it can be traced back to the author.
#[derive(Component)]
pub struct AuthorLabel(pub usize);

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Focus>()
            .init_resource::<Hover>()
            .add_systems(Startup, spawn)
            .add_systems(
                Update,
                (
                    hover_authors,
                    pick_file,
                    clicks,
                    follow_key,
                    tooltip,
                    history_panel,
                )
                    .chain()
                    .after(playback)
                    // A video's window is a preview: the mouse there doesn't point into the scene.
                    .run_if(resource_exists::<History>)
                    .run_if(not(resource_exists::<Recorder>)),
            );
    }
}

#[derive(Component)]
struct Tooltip;

#[derive(Component)]
struct Panel;

fn spawn(mut commands: Commands) {
    let text = (
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
    );
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                padding: UiRect::all(px(8)),
                border_radius: BorderRadius::all(px(4)),
                ..default()
            },
            BackgroundColor(PANEL_COLOR),
            Visibility::Hidden,
            // Never in the way of the mouse it follows.
            Pickable::IGNORE,
            Tooltip,
        ))
        .with_child(text.clone());
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(64),
                right: px(16),
                max_width: px(520),
                padding: UiRect::all(px(12)),
                border_radius: BorderRadius::all(px(6)),
                ..default()
            },
            BackgroundColor(PANEL_COLOR),
            Visibility::Hidden,
            Panel,
        ))
        .with_child(text);
}

fn hover_authors(labels: Query<(&AuthorLabel, &Hovered)>, mut hover: ResMut<Hover>) {
    let hovered = labels
        .iter()
        .find(|(_, h)| h.get())
        .map(|(label, _)| label.0);
    if hover.author != hovered {
        hover.author = hovered;
    }
}

/// Finds the file under the mouse, if any (and no author label is in the way).
#[allow(clippy::too_many_arguments)]
fn pick_file(
    time: Res<Time>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    tree: Res<Tree>,
    scrubbing: Res<Scrubbing>,
    mode: Res<View>,
    mut hover: ResMut<Hover>,
    mut last: Local<(Option<Vec2>, f32)>,
) {
    let cursor = window.cursor_position();
    last.1 += time.delta_secs();
    if cursor == last.0 && last.1 < PICK_REFRESH_SECONDS {
        return;
    }
    *last = (cursor, 0.0);
    let (camera, transform) = *camera;
    // Files are only picked in the tree (the ziggurat's past layers aren't files).
    let pickable = hover.author.is_none() && !scrubbing.0 && *mode == View::Tree;
    let ray = cursor
        .filter(|_| pickable)
        .and_then(|c| camera.viewport_to_world(transform, c).ok());
    let picked = ray.and_then(|ray| pick(&tree, ray.origin, *ray.direction));
    if hover.file != picked {
        hover.file = picked;
    }
}

/// The nearest file along the ray.
fn pick(tree: &Tree, origin: Vec3, direction: Vec3) -> Option<FileId> {
    // Distance along the ray, and squared distance from it, of a point.
    let probe = |p: Vec3| {
        let along = (p - origin).dot(direction);
        (along, (p - origin - direction * along).length_squared())
    };
    let mut best: Option<(f32, FileId)> = None;
    for dir in tree.dirs.iter().filter(|d| d.alive && !d.files.is_empty()) {
        let reach = dir.radius() + PICK_RADIUS;
        let (along, off) = probe(dir.pos);
        if off > reach * reach || along < -reach {
            continue;
        }
        for (slot, file) in dir.files.iter().enumerate() {
            let (along, off) = probe(dir.pos + crate::tree::file_offset(slot));
            if along > 0.0 && off < PICK_RADIUS * PICK_RADIUS && best.is_none_or(|(t, _)| along < t)
            {
                best = Some((along, file.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// A click on an author follows them (again: stops), on a file focuses it, elsewhere frames
/// the whole tree again. Drags (camera, seek bar) are not clicks.
fn clicks(
    buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    scrubbing: Res<Scrubbing>,
    hover: Res<Hover>,
    mut pressed_at: Local<Option<Vec2>>,
    mut focus: ResMut<Focus>,
) {
    let cursor = window.cursor_position();
    if buttons.just_pressed(MouseButton::Left) {
        *pressed_at = cursor.filter(|_| !scrubbing.0);
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let Some((from, to)) = pressed_at.take().zip(cursor) else {
        return;
    };
    if from.distance(to) > CLICK_SLOP {
        return;
    }
    *focus = clicked(*focus, &hover);
}

/// What a click focuses, given what it was on.
fn clicked(focus: Focus, hover: &Hover) -> Focus {
    match (hover.author, hover.file) {
        (Some(a), _) if focus == Focus::Author(a) => Focus::Tree,
        (Some(a), _) => Focus::Author(a),
        (None, Some(id)) => Focus::File(id),
        (None, None) => Focus::Tree,
    }
}

/// `F` follows the next active author, then the next, and finally stops following.
fn follow_key(
    keys: Res<ButtonInput<bevy::input::keyboard::Key>>,
    authors: Res<Authors>,
    mut focus: ResMut<Focus>,
) {
    if !crate::keys::typed(&keys, "f") {
        return;
    }
    let after = match *focus {
        Focus::Author(a) => a + 1,
        _ => 0,
    };
    let next = (after..authors.list.len()).find(|&a| authors.list[a].shown());
    *focus = next.map_or(Focus::Tree, Focus::Author);
}

fn author_name(authors: &Authors, a: usize) -> &str {
    authors.name(a)
}

/// Path, change count and last change of the hovered file, next to the mouse.
#[allow(clippy::too_many_arguments)]
fn tooltip(
    window: Single<&Window, With<PrimaryWindow>>,
    hover: Res<Hover>,
    tree: Res<Tree>,
    scene: Res<Scene>,
    history: Res<History>,
    authors: Res<Authors>,
    tooltip: Single<(&mut Node, &mut Visibility, &Children), With<Tooltip>>,
    mut texts: Query<&mut Text>,
) {
    let (mut node, mut visibility, children) = tooltip.into_inner();
    let found = hover
        .file
        .and_then(|id| Some((tree.file_path(id)?, scene.files.get(id)?.as_ref()?)));
    let (Some((path, view)), Some(cursor)) = (found, window.cursor_position()) else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let date = git::date(history.commits[view.last_commit].time);
    let changes = if view.changes == 1 {
        "1 change".to_string()
    } else {
        format!("{} changes", view.changes)
    };
    let wanted = format!(
        "{path}\n{changes}, last on {date} by {}",
        author_name(&authors, view.touched_by)
    );
    if let Ok(mut text) = texts.get_mut(children[0])
        && text.0 != wanted
    {
        text.0 = wanted;
    }
    node.left = px(cursor.x + 16.0);
    node.top = px(cursor.y + 16.0);
    visibility.set_if_neq(Visibility::Inherited);
}

/// When a file is focused: its latest changes, newest first (looked up once per focus).
#[allow(clippy::too_many_arguments)]
fn history_panel(
    focus: Res<Focus>,
    tree: Res<Tree>,
    history: Res<History>,
    authors: Res<Authors>,
    mut shown: Local<Option<FileId>>,
    panel: Single<(&mut Visibility, &Children), With<Panel>>,
    mut texts: Query<&mut Text>,
) {
    let (mut visibility, children) = panel.into_inner();
    let Focus::File(id) = *focus else {
        *shown = None;
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    if *shown == Some(id) {
        return;
    }
    let Some(path) = tree.file_path(id) else {
        return;
    };
    *shown = Some(id);
    let mut lines = vec![path.clone()];
    let touches = history.commits[..history.next.min(history.commits.len())]
        .iter()
        .rev()
        .filter_map(|c| {
            c.changes
                .iter()
                .find(|ch| ch.path == path)
                .map(|ch| (c, ch.action))
        })
        .take(HISTORY_ROWS);
    for (commit, action) in touches {
        let verb = match action {
            Action::Add => "added   ",
            Action::Modify => "modified",
            Action::Delete => "deleted ",
        };
        let author = authors
            .people
            .by_email
            .get(&commit.email)
            .map_or("?", |&a| author_name(&authors, a));
        lines.push(format!(
            "{}  {verb}  {}  {author}",
            git::date(commit.time),
            &commit.sha[..7]
        ));
    }
    if let Ok(mut text) = texts.get_mut(children[0]) {
        text.0 = lines.join("\n");
    }
    *visibility = Visibility::Inherited;
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
        tree.settle(paths.len());
        tree
    }

    #[test]
    fn picks_within_the_pick_radius_only() {
        let tree = tree(&["a/x"]);
        let target = tree.file_pos(0).unwrap();
        let ray = |offset: f32| pick(&tree, target + Vec3::new(offset, 0.0, 50.0), -Vec3::Z);
        assert_eq!(ray(0.0), Some(0));
        assert_eq!(ray(PICK_RADIUS * 0.9), Some(0));
        assert_eq!(ray(PICK_RADIUS * 1.1), None);
    }

    #[test]
    fn files_behind_the_camera_are_not_picked() {
        let tree = tree(&["a/x"]);
        let target = tree.file_pos(0).unwrap();
        assert_eq!(pick(&tree, target + Vec3::Z, Vec3::Z), None);
    }

    #[test]
    fn deleted_files_are_not_picked() {
        let mut tree = tree(&["a/x", "b/y"]);
        let target = tree.file_pos(0).unwrap();
        let delete = Change {
            action: Action::Delete,
            path: "a/x".to_string(),
        };
        tree.apply(
            &delete,
            Touch {
                author: 0,
                commit: 0,
                time: 0,
                animate: false,
            },
        );
        tree.update(crate::tree::LAYOUT_DT);
        assert_eq!(pick(&tree, target + Vec3::Z * 50.0, -Vec3::Z), None);
    }

    #[test]
    fn the_pick_is_the_nearest_file_near_the_ray() {
        let paths: Vec<String> = (0..200)
            .map(|i| format!("d{}/e{}/f{i}", i % 4, i % 9))
            .collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let tree = tree(&paths);
        let (center, radius) = tree.bounds();
        let mut hits = 0;
        for i in 0..300 {
            // Rays from all around, aimed at a file each.
            let a = i as f32 * 2.399_963;
            let origin =
                center + Vec3::new(a.cos(), (i as f32 * 0.37).sin(), a.sin()) * radius * 2.0;
            let direction = (tree.file_pos(i % 200).unwrap() - origin).normalize();
            let brute = (0..200)
                .filter_map(|id| {
                    let p = tree.file_pos(id)?;
                    let along = (p - origin).dot(direction);
                    let off = (p - origin - direction * along).length();
                    (along > 0.0 && off < PICK_RADIUS).then_some((along, id))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, id)| id);
            assert_eq!(pick(&tree, origin, direction), brute, "ray {i}");
            hits += usize::from(brute.is_some());
        }
        assert_eq!(hits, 300);
    }

    #[test]
    fn clicks_focus_what_they_are_on() {
        let on = |author, file| Hover { author, file };
        assert_eq!(clicked(Focus::Tree, &on(None, Some(4))), Focus::File(4));
        assert_eq!(clicked(Focus::Tree, &on(Some(2), None)), Focus::Author(2));
        // An author label is above the files behind it.
        assert_eq!(
            clicked(Focus::File(4), &on(Some(2), Some(4))),
            Focus::Author(2)
        );
        // Clicking the followed author again, or the void, frames the tree.
        assert_eq!(clicked(Focus::Author(2), &on(Some(2), None)), Focus::Tree);
        assert_eq!(
            clicked(Focus::Author(1), &on(Some(2), None)),
            Focus::Author(2)
        );
        assert_eq!(clicked(Focus::File(4), &on(None, None)), Focus::Tree);
    }

    #[test]
    fn the_spotlight_is_the_hovered_author_else_the_followed_one() {
        let hover = |author| Hover { author, file: None };
        assert_eq!(hover(Some(3)).spotlight(Focus::Author(1)), Some(3));
        assert_eq!(hover(None).spotlight(Focus::Author(1)), Some(1));
        assert_eq!(hover(None).spotlight(Focus::File(1)), None);
        assert_eq!(hover(None).spotlight(Focus::Tree), None);
    }

    #[test]
    fn picks_the_nearest_file_on_the_ray() {
        let mut tree = Tree::new();
        let touch = Touch {
            author: 0,
            commit: 0,
            time: 0,
            animate: false,
        };
        for path in ["src/a.rs", "src/b.rs", "docs/c.md"] {
            tree.apply(
                &Change {
                    action: Action::Add,
                    path: path.to_string(),
                },
                touch,
            );
        }
        tree.settle(3);
        for id in 0..3 {
            let target = tree.file_pos(id).unwrap();
            let origin = target + bevy::math::Vec3::new(0.0, 0.0, 50.0);
            assert_eq!(pick(&tree, origin, (target - origin).normalize()), Some(id));
        }
        let away = bevy::math::Vec3::new(1000.0, 0.0, 0.0);
        assert_eq!(pick(&tree, away, bevy::math::Vec3::X), None);
    }
}
