mod backdrop;
mod branches;
mod colors;
mod config;
mod director;
mod files;
mod git;
mod github;
mod glob;
mod hud;
mod icon;
mod identity;
mod interact;
mod keys;
mod pace;
mod perf;
mod quality;
mod record;
mod sparks;
mod timeline;
mod tree;
mod ziggurat;

use std::collections::{HashMap, VecDeque};
use std::f32::consts::{FRAC_PI_4, PI, TAU};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::picking::hover::Hovered;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureDimension, TextureFormat};
use bevy::render::view::NoIndirectDrawing;
use bevy::window::{MonitorSelection, PresentMode, WindowMode};

use branches::Branches;
use config::Hide;
use director::{CameraMode, Director};
use files::{FileInstance, FileInstances};
use git::{Action, Commit, Loaded};
use github::Identity;
use glob::Glob;
use identity::People;
use interact::{AuthorLabel, Focus, Hover};
use pace::{Pace, glow};
use sparks::Sparks;
use tree::{Event, FileId, Touch, Tree, file_offset};
use ziggurat::{View, Ziggurat};

/// Seconds of animation per day of history.
const DEFAULT_SECONDS_PER_DAY: f64 = 1.0;
/// Quiet periods longer than this many seconds of animation are skipped.
const AUTO_SKIP_SECONDS: f64 = 1.0;
/// How long a touched file glows and stays linked to its author.
/// Authors disappear after this long without committing.
/// At most this many authors on the halo (beyond `--highlight-user`): when more are active
/// (a big repository played fast), those with the most commits show, so the tree stays visible.
const MAX_SHOWN_AUTHORS: usize = 20;
/// An `--avatar-dir` image wins over any GitHub avatar.
const LOCAL_AVATAR: u32 = u32::MAX;
/// Height of the authors' halo, as a fraction of the view height.
const HALO_SIZE: f32 = 0.75;
/// On each commit an author's avatar darts this fraction of the way to their work, and back.
const DASH_REACH: f32 = 0.08;
const DASH_SECONDS: f64 = 0.8;
/// Authors glide to their halo slot at this fraction of the way per second, never faster
/// than `HALO_MAX_SPEED` radians per second…
const HALO_EASE: f32 = 0.8;
const HALO_MAX_SPEED: f32 = 0.5;
/// …and where they work is smoothed over about this long, so one commit elsewhere doesn't
/// send them across the screen.
const WORK_SMOOTHING_SECONDS: f32 = 2.0;
/// Two neighbours on the halo only swap places when their work is this far the other way
/// round (radians): swapping makes their labels cross, so authors working in the same spot
/// would keep crossing over each other.
const HALO_SWAP_ANGLE: f32 = 0.35;
/// Neighbours on the halo keep at least this share of an even slot between them, pushed apart
/// at up to `HALO_PUSH_SPEED` radians per second, so labels don't pile up while gliding.
const HALO_MIN_GAP: f32 = 0.7;
const HALO_PUSH_SPEED: f32 = 1.5;
/// Files an author touched last, used to find where they work.
const RECENT_FILES: usize = 24;
const AVATAR_SIZE: f32 = 40.0;
/// Radians per second the camera turns on its own while not dragged.
const AUTO_ROTATE_SPEED: f32 = 0.1;
const BACKGROUND: Color = Color::srgb(0.01, 0.012, 0.02);
const BRANCH_COLOR: Color = Color::srgba(0.4, 0.45, 0.6, 0.35);
/// `--bench` plays the whole history in about this many seconds.
const BENCH_SECONDS: f64 = 60.0;
/// Emissive strength of a file at rest, and how much a fresh change adds (enough to bloom).
const FILE_EMISSIVE: f32 = 0.15;
const GLOW_EMISSIVE: f32 = 12.0;
/// A freshly touched file is this many times bigger, shrinking back as its glow fades.
const GLOW_GROWTH: f32 = 2.0;
/// Files changed again and again swell a little: each change adds 1 to a file's heat, which
/// halves every `HEAT_HALF_LIFE_DAYS` of history. Heat beyond the first change swells the file
/// up to `MAX_SWELL` times, reaching ~63% of that at `HEAT_SCALE`. It always shrinks back.
const HEAT_HALF_LIFE_DAYS: f64 = 14.0;
const MAX_SWELL: f32 = 1.5;
const HEAT_SCALE: f32 = 4.0;
/// Beams and glows by action: added, modified, deleted.
const ADD_COLOR: Color = Color::srgb(0.29, 0.87, 0.5);
const MODIFY_COLOR: Color = Color::srgb(0.38, 0.65, 0.98);
const DELETE_COLOR: Color = Color::srgb(0.97, 0.44, 0.44);
/// Beams are this much brighter than their color, enough to bloom a little.
const BEAM_BRIGHTNESS: f32 = 2.0;
/// Width of every line (branches and beams), in pixels; Bevy's default of 2 looked heavy.
const LINE_WIDTH: f32 = 1.0;
/// File changes applied per second at the default speed (scaled with the speed), so a big
/// commit appears file by file instead of all in one frame.
const CHANGES_PER_SECOND: f64 = 200.0;
/// …but a backlog is always cleared within about this long, however big the commit.
const SWEEP_SECONDS: f64 = 1.5;
/// The history clock waits while more than this much (in seconds) of changes is queued, so
/// the date doesn't run ahead of what is shown.
const BACKLOG_PAUSE_SECONDS: f64 = 0.25;
/// Zoom factor per second while `+`/`-` is held.
const KEY_ZOOM_SPEED: f32 = 1.5;
const DEFAULT_PITCH: f32 = -0.5;
/// The spotlit author's files (hovered or followed) glow this much more; everyone else's dim.
const SPOT_EMISSIVE: f32 = 1.5;
const SPOT_DIM: f32 = 0.3;
const SPOT_BEAM_DIM: f32 = 0.15;
/// Highlighted authors' files glow this much more.
const HIGHLIGHT_EMISSIVE: f32 = 0.6;
/// The hovered file grows this much, and its branch lights up back to the root.
const HOVER_GROWTH: f32 = 1.8;
const BRANCH_HOVER_COLOR: Color = Color::srgba(1.0, 0.9, 0.6, 0.9);
/// Framing radius around a focused file, and the least around a followed author's work.
const FILE_FOCUS_RADIUS: f32 = 4.0;
const AUTHOR_FOCUS_RADIUS: f32 = 14.0;

#[derive(Resource)]
struct History {
    commits: Vec<Commit>,
    /// The next commit to queue.
    next: usize,
    /// Current position in the history, as a unix timestamp.
    time: f64,
    seconds_per_day: f64,
    paused: bool,
    /// Changes of reached commits not applied yet.
    pending: VecDeque<Queued>,
    /// Fraction of a change carried over to the next frame.
    budget: f64,
    /// Where playback started (`--from`, `--start`): `Home` and `--loop` go back there.
    first: usize,
    /// A jump requested for this frame: to just before this commit (see `timeline`).
    seek: Option<usize>,
    /// Set when the way history is read changed (`B`): the next jump rebuilds from scratch.
    remake: bool,
    reading: Reading,
    tags: Vec<(i64, String)>,
}

/// A change to apply: commit and change indices, and whether it lands on a side branch (shown
/// beside the tree until its merge, `--branches`) instead of the tree.
#[derive(Clone, Copy)]
struct Queued {
    commit: usize,
    change: usize,
    ghost: bool,
}

/// How the history is read: by default every commit changes the tree when it happens; with
/// `--branches` the tree follows the main line, and side-branch commits land at their merge.
struct Reading {
    branches: bool,
    /// Main-line merge → the side-branch commits it brings, oldest first.
    brings: HashMap<usize, Vec<usize>>,
}

impl Reading {
    fn new(commits: &[Commit], branches: bool) -> Self {
        let mut brings: HashMap<usize, Vec<usize>> = HashMap::new();
        for (c, commit) in commits.iter().enumerate() {
            if let Some(merge) = commit.merged_by {
                brings.entry(merge).or_default().push(c);
            }
        }
        Self { branches, brings }
    }

    /// Queues what reaching commit `c` does.
    fn queue(&self, commits: &[Commit], c: usize, out: &mut impl Extend<Queued>) {
        let changes = |commit: usize, ghost| {
            (0..commits[commit].changes.len()).map(move |change| Queued {
                commit,
                change,
                ghost,
            })
        };
        if !self.branches {
            out.extend(changes(c, false));
        } else if commits[c].merged_by.is_some() {
            out.extend(changes(c, true));
        } else {
            for &side in self.brings.get(&c).into_iter().flatten() {
                out.extend(changes(side, false));
            }
            out.extend(changes(c, false));
        }
    }
}

impl History {
    /// How much faster than the default speed the history plays.
    fn speed(&self) -> f64 {
        DEFAULT_SECONDS_PER_DAY / self.seconds_per_day
    }

    fn finished(&self) -> bool {
        self.next >= self.commits.len() && self.pending.is_empty()
    }

    /// Times of the first and last commits (never equal, so fractions of it are defined).
    fn span(&self) -> (f64, f64) {
        let first = self.commits[0].time as f64;
        (
            first,
            (self.commits[self.commits.len() - 1].time as f64).max(first + 1.0),
        )
    }

    /// How far the clock is through the history, from 0 to 1.
    fn progress(&self) -> f64 {
        let (first, last) = self.span();
        ((self.time - first) / (last - first)).clamp(0.0, 1.0)
    }
}

#[derive(Resource)]
struct Settings {
    repos: Vec<PathBuf>,
    hide: Hide,
    seconds_per_day: f64,
    looping: bool,
    bench: bool,
    highlight_user: Vec<Glob>,
    follow_user: Option<Glob>,
    title: String,
    logo: Option<PathBuf>,
    background: Option<PathBuf>,
    /// `--avatar-dir`'s images, by lowercased file name without extension.
    avatars: HashMap<String, PathBuf>,
    /// `[people]` of the config.
    people: identity::Aliases,
    /// `--branches`, at startup.
    branches: bool,
}

/// The history, read and filtered in a background thread so the window opens at once.
#[derive(Resource)]
struct Loading(Mutex<Receiver<Result<Loaded, String>>>);

/// What GitHub knows about authors (logins, profile names, avatars), arriving from a background thread.
#[derive(Resource)]
struct Identities {
    receiver: Mutex<Receiver<Identity>>,
    /// Emails asked about, and answers received so far (the HUD shows the progress).
    total: usize,
    done: usize,
    /// Every answer is in: a video can start.
    finished: bool,
}

/// File kinds (one per file type, see `colors::file_type`), their names and colors.
#[derive(Resource, Default)]
struct Palette {
    kinds: HashMap<String, usize>,
    names: Vec<String>,
    colors: Vec<LinearRgba>,
}

impl Palette {
    /// Returns the kind of a file, picking its color on first sight.
    fn kind(&mut self, name: &str) -> usize {
        let file_type = colors::file_type(name);
        if let Some(&kind) = self.kinds.get(&file_type) {
            return kind;
        }
        self.colors
            .push(LinearRgba::from(colors::type_color(&file_type)));
        self.names.push(file_type.clone());
        self.kinds.insert(file_type, self.colors.len() - 1);
        self.colors.len() - 1
    }
}

/// What the view keeps about files, by file id. Files aren't entities: they are drawn as
/// instances of the file layer (see `files.rs`).
#[derive(Resource, Default, Clone)]
struct Scene {
    /// By file id; `None` once deleted.
    files: Vec<Option<FileView>>,
    /// Files currently glowing, the only ones that get a beam.
    glowing: Vec<FileId>,
    /// `file_offset` of each slot, computed once.
    offsets: Vec<Vec3>,
    /// Live files per kind, for the legend.
    counts: Vec<usize>,
    /// Beams to files just deleted (their file is gone, so they are kept here).
    deletions: Vec<Deletion>,
    /// By author (as recorded, before merges): the files they touched last.
    recent: Vec<VecDeque<FileId>>,
}

#[derive(Clone)]
struct Deletion {
    author: usize,
    pos: Vec3,
    at: f64,
}

#[derive(Clone)]
struct FileView {
    kind: usize,
    /// What the last change did, for the glow and beam color.
    action: Action,
    /// Elapsed app time of the last change, for the glow animation.
    touched_at: f64,
    /// Index of the author who last touched the file.
    touched_by: usize,
    glowing: bool,
    /// Commits that added or modified it, and the last one.
    changes: u32,
    last_commit: usize,
    /// Recent activity at history time `heat_at` (see `HEAT_HALF_LIFE_DAYS`); 0 once cold.
    heat: f32,
    heat_at: i64,
}

/// How much a file of this heat swells: not at all for one recent change, at most `MAX_SWELL`.
fn swell(heat: f32) -> f32 {
    1.0 + (MAX_SWELL - 1.0) * (1.0 - (-(heat - 1.0).max(0.0) / HEAT_SCALE).exp())
}

impl FileView {
    /// The heat at history time `time`.
    fn heat(&self, time: f64) -> f32 {
        let days = ((time - self.heat_at as f64) / 86400.0).max(0.0);
        self.heat * (-days / HEAT_HALF_LIFE_DAYS).exp2() as f32
    }
}

/// An author on screen (one person, see `identity::Person`), drawn as a UI overlay (avatar +
/// name) following a point on the halo around the tree.
struct Author {
    /// Set when they turned out to be the same person as another author.
    merged: bool,
    /// The commits of the email whose avatar is shown (`LOCAL_AVATAR` for `--avatar-dir`):
    /// someone with several emails shows the avatar of the one they use most.
    avatar_from: Option<u32>,
    /// UI row holding the avatar and the name.
    label: Entity,
    avatar: Entity,
    name: Entity,
    /// Always shown and brighter (`--highlight-user`).
    highlighted: bool,
    pos: Vec3,
    /// Position on the halo, in radians.
    angle: f32,
    /// Smoothed screen angle of where they work, around the focus.
    work_angle: Option<f32>,
    on_halo: bool,
    last_active: f64,
    /// Their commits in the whole history: who keeps a place on a crowded halo.
    commits: u32,
    /// Changed something lately (`Pace::idle`), and not pushed off the halo by more recent
    /// authors (`MAX_SHOWN_AUTHORS`); set each frame by `limit_shown`.
    active: bool,
    crowded: bool,
    /// When their last commit started, for the dash towards their work.
    dashed_at: f64,
}

impl Author {
    /// On the halo: active (and among the most recent), or highlighted.
    fn shown(&self) -> bool {
        !self.merged && (self.highlighted || (self.active && !self.crowded))
    }
}

/// Who's who (`people`) and how each person is drawn (`list`), both indexed by person.
#[derive(Resource, Default)]
struct Authors {
    people: People,
    list: Vec<Author>,
    /// Reused by `limit_shown`.
    ranked: Vec<usize>,
    /// Commits per email, to pick a person's avatar.
    email_commits: HashMap<String, u32>,
}

impl Authors {
    /// Follows merges to the author who now stands for `a`.
    fn resolve(&self, a: usize) -> usize {
        self.people.resolve(a)
    }

    /// The author behind an email.
    fn by_email(&self, email: &str) -> usize {
        self.people.by_email[email]
    }

    /// The name shown for author `a`.
    fn name(&self, a: usize) -> &str {
        &self.people.list[self.resolve(a)].display
    }

    /// Says who's active (changed something in the last `idle` seconds), and keeps at most
    /// `MAX_SHOWN_AUTHORS` of them on the halo: those with the most commits overall (someone
    /// passing by with one commit only shows when there's room), then the most recent.
    fn limit_shown(&mut self, now: f64, idle: f64) {
        for author in &mut self.list {
            author.active = now - author.last_active < idle;
            author.crowded = false;
        }
        let Self { list, ranked, .. } = self;
        ranked.clear();
        ranked.extend(
            (0..list.len()).filter(|&a| !list[a].merged && !list[a].highlighted && list[a].active),
        );
        if ranked.len() <= MAX_SHOWN_AUTHORS {
            return;
        }
        let first = |&a: &usize, &b: &usize| {
            (list[b].commits, list[b].last_active)
                .partial_cmp(&(list[a].commits, list[a].last_active))
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        ranked.select_nth_unstable_by(MAX_SHOWN_AUTHORS, first);
        for &a in &ranked[MAX_SHOWN_AUTHORS..] {
            list[a].crowded = true;
        }
    }

    /// Hides `from` at once, now that `into` stands for them (`People` merged them).
    fn merge(&mut self, from: usize, into: usize) {
        let last_active = self.list[from].last_active.max(self.list[into].last_active);
        self.list[into].last_active = last_active;
        self.list[into].highlighted |= self.list[from].highlighted;
        self.list[into].commits += self.list[from].commits;
        self.list[from].last_active = f64::NEG_INFINITY;
        self.list[from].merged = true;
    }
}

#[derive(Resource)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    /// User zoom factor on top of the automatic framing.
    zoom: f32,
    focus: Vec3,
    distance: f32,
}

fn main() -> AppExit {
    let (options, range) =
        match config::load().and_then(|o| git::Range::new(&o.repos, &o.config).map(|r| (o, r))) {
            Ok(loaded) => loaded,
            Err(e) => {
                eprintln!("eridu: {e}");
                return AppExit::from_code(2);
            }
        };
    let (tx, rx) = mpsc::channel();
    let (repos, config) = (options.repos.clone(), options.config);
    let several = repos.len() > 1;
    let seconds_per_day = config.seconds_per_day.unwrap_or(DEFAULT_SECONDS_PER_DAY);
    let looping = config.looping.unwrap_or(false);
    // Authors are compared lowercased, like `Commit::author` and `Commit::email`.
    let user_glob = |pattern: &String| Glob::new(&pattern.to_lowercase());
    let highlight_user = config.highlight_user.iter().map(user_glob).collect();
    let follow_user = config.follow_user.as_ref().map(user_glob);
    let title = config
        .title
        .clone()
        .unwrap_or_else(|| git::repo_names(&options.repos).join(" & "));
    let (logo, background, people) = (
        config.logo.clone(),
        config.background.clone(),
        config.people.clone(),
    );
    let avatars = config
        .avatar_dir
        .as_deref()
        .map(github::avatar_files)
        .unwrap_or_default();
    if options.people {
        return print_people(&repos, &config, range);
    }
    let camera = options.camera;
    // A bench measures one tier, not the auto steps; a video takes its time, at the best one.
    let quality = match (options.bench, &options.record) {
        (true, _) => Some(options.quality.unwrap_or(quality::Quality::High)),
        (_, Some(_)) => Some(options.quality.unwrap_or(quality::Quality::Ultra)),
        _ => options.quality,
    };
    // A video plays the history once.
    let looping = looping && options.record.is_none();
    std::thread::spawn(move || tx.send(git::history(&repos, &config, range)));

    let fast = options.bench || options.record.is_some();
    let present_mode = if fast {
        PresentMode::AutoNoVsync
    } else {
        PresentMode::AutoVsync
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("Eridu — {title}"),
                present_mode,
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            files::FilesPlugin,
            hud::HudPlugin,
            interact::InteractPlugin,
            backdrop::BackdropPlugin,
            quality::QualityPlugin { quality },
            timeline::TimelinePlugin,
            perf::PerfPlugin {
                bench: options.bench,
            },
            record::RecordPlugin(options.record),
        ))
        .insert_resource(ClearColor(BACKGROUND))
        .insert_resource(Settings {
            repos: options.repos,
            hide: options.hide,
            seconds_per_day,
            looping,
            bench: options.bench,
            highlight_user,
            follow_user,
            title,
            logo,
            background,
            avatars,
            people,
            branches: options.branches,
        })
        .insert_resource(Loading(Mutex::new(rx)))
        .insert_resource(Tree::new().with_cities(several))
        .init_resource::<Scene>()
        .init_resource::<Palette>()
        .init_resource::<Sparks>()
        .init_resource::<Branches>()
        .init_resource::<Pace>()
        .insert_resource(Orbit {
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            zoom: 1.0,
            focus: Vec3::ZERO,
            distance: 20.0,
        })
        .insert_resource(Director::new(camera))
        .insert_resource(options.view)
        .add_systems(Startup, (setup, thin_lines))
        .add_systems(FixedUpdate, layout.run_if(resource_exists::<History>))
        .add_systems(
            Update,
            ((controls, receive_history).chain(), icon::set_window_icon),
        )
        .add_systems(
            Update,
            (
                apply_identities,
                playback,
                sync_scene,
                branches::place_ghosts,
                ziggurat_view,
                update_files,
                orbit_camera,
                move_authors,
                draw_lines,
                place_labels,
            )
                .chain()
                .after(receive_history)
                .run_if(resource_exists::<History>),
        )
        .run()
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.spawn((
        Camera3d::default(),
        // A tonemapper that desaturates to white makes bright glows look natural.
        Tonemapping::TonyMcMapface,
        Bloom::NATURAL,
        // The file layer draws with plain (not indirect) instanced draws.
        NoIndirectDrawing,
        Transform::from_xyz(0.0, 10.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 3000.0,
            ..default()
        },
        Transform::from_xyz(1.0, 2.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    files::spawn_layer(
        &mut commands,
        meshes.add(Sphere::new(0.15).mesh().ico(2).unwrap()),
    );
}

/// Once the history is read: groups its authors, starts looking them up on GitHub, and jumps
/// to the start position.
fn receive_history(
    loading: Option<Res<Loading>>,
    settings: Res<Settings>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(loading) = loading else {
        return;
    };
    let Ok(Ok(received)) = loading.0.lock().map(|rx| rx.try_recv()) else {
        return;
    };
    commands.remove_resource::<Loading>();
    let Loaded {
        commits,
        start,
        tags,
    } = match received {
        Ok(loaded) => loaded,
        Err(e) => {
            error!("{e}");
            exit.write(AppExit::error());
            return;
        }
    };

    let people = People::new(
        commits
            .iter()
            .map(|c| (c.author.as_str(), c.email.as_str())),
        &settings.people,
    );
    info!(
        "{} authors behind {} emails",
        people.list.len(),
        people.by_email.len()
    );
    let mut list: Vec<Author> = people
        .list
        .iter()
        .enumerate()
        .map(|(a, person)| spawn_author(&mut commands, a, person, settings.hide))
        .collect();
    // `--avatar-dir` images, found by email, the part before `@`, a commit name or the name shown.
    if !settings.avatars.is_empty() {
        for (author, person) in list.iter_mut().zip(&people.list) {
            let local_parts = person
                .emails
                .iter()
                .map(|e| e.split('@').next().unwrap_or(e).to_string());
            let keys: Vec<String> = person
                .emails
                .iter()
                .cloned()
                .chain(local_parts)
                .chain(person.names.iter().map(|(n, _)| n.to_lowercase()))
                .chain([person.display.to_lowercase()])
                .collect();
            if let Some(path) = keys.iter().find_map(|k| settings.avatars.get(k)) {
                set_avatar(&mut commands, &mut images, author, path);
            }
        }
    }
    // `--highlight-user` and `--follow-user` match a name or an email of the person.
    let mut email_commits = HashMap::new();
    let matches = |glob: &Glob, commit: &Commit| {
        glob.matches(&commit.author.to_lowercase()) || glob.matches(&commit.email)
    };
    for commit in &commits {
        let a = people.by_email[&commit.email];
        list[a].highlighted |= settings.highlight_user.iter().any(|g| matches(g, commit));
        list[a].commits += 1;
        *email_commits.entry(commit.email.clone()).or_default() += 1;
    }
    if let Some(glob) = &settings.follow_user
        && let Some(commit) = commits.iter().find(|c| matches(glob, c))
    {
        commands.insert_resource(Focus::Author(people.by_email[&commit.email]));
    }
    commands.insert_resource(Authors {
        people,
        list,
        ranked: Vec::new(),
        email_commits,
    });
    let asked = lookups(&commits, start, &settings.repos);
    let total = asked.len();
    commands.insert_resource(Identities {
        receiver: Mutex::new(github::resolve(asked)),
        total,
        done: 0,
        finished: false,
    });

    let reading = Reading::new(&commits, settings.branches);
    info!(
        "{} commits on side branches, brought by {} merges",
        reading.brings.values().map(Vec::len).sum::<usize>(),
        reading.brings.len()
    );
    let time = commits[start].time as f64;
    let seconds_per_day = if settings.bench {
        let days = (commits[commits.len() - 1].time as f64 - time).max(86400.0) / 86400.0;
        BENCH_SECONDS / days
    } else {
        settings.seconds_per_day
    };
    commands.insert_resource(History {
        commits,
        next: 0,
        time,
        seconds_per_day,
        paused: false,
        pending: VecDeque::new(),
        budget: 0.0,
        first: start,
        // Jumps to the start position: applies the commits before it at once.
        seek: Some(start),
        remake: false,
        reading,
        tags,
    });
}

/// One `(email, sha of one of their commits, its repository)` per email, for GitHub to tell
/// who they are: in the order they appear on screen from commit `start`, then the earlier ones.
fn lookups(commits: &[Commit], start: usize, repos: &[PathBuf]) -> Vec<(String, String, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    let (before, after) = commits.split_at(start.min(commits.len()));
    after
        .iter()
        .chain(before)
        .filter(|c| seen.insert(c.email.as_str()))
        .map(|c| (c.email.clone(), c.sha.clone(), repos[c.repo].clone()))
        .collect()
}

/// `--people`: reads the history, asks GitHub (or its cache) about everyone, prints who's who.
fn print_people(repos: &[PathBuf], config: &config::Config, range: git::Range) -> AppExit {
    let loaded = match git::history(repos, config, range) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("eridu: {e}");
            return AppExit::from_code(2);
        }
    };
    let commits = &loaded.commits;
    let mut people = People::new(
        commits
            .iter()
            .map(|c| (c.author.as_str(), c.email.as_str())),
        &config.people,
    );
    eprintln!("Asking GitHub about {} emails...", people.by_email.len());
    for identity in github::resolve(lookups(commits, 0, repos)) {
        people.learn(
            &identity.email,
            identity.login.as_deref(),
            identity.profile.as_deref(),
        );
    }
    people.name_all();
    print!("{}", people.report());
    AppExit::Success
}

/// Spawns an author's (hidden) overlay: a round avatar, colored until the real one arrives, and a name.
fn spawn_author(
    commands: &mut Commands,
    index: usize,
    person: &identity::Person,
    hide: Hide,
) -> Author {
    let color = colors::name_color(&person.emails[0]);
    let shown = |hidden: bool| if hidden { Display::None } else { Display::Flex };
    let avatar = commands
        .spawn((
            Node {
                width: px(AVATAR_SIZE),
                height: px(AVATAR_SIZE),
                border: UiRect::all(px(2)),
                border_radius: BorderRadius::MAX,
                display: shown(hide.avatars),
                ..default()
            },
            BorderColor::all(color),
            BackgroundColor(color.with_alpha(0.5)),
        ))
        .id();
    let name = commands
        .spawn((
            Text::new(person.display.clone()),
            TextFont {
                font_size: FontSize::Px(15.0),
                ..default()
            },
            TextColor(color),
            Node {
                display: shown(hide.names),
                ..default()
            },
        ))
        .id();
    let label = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                align_items: AlignItems::Center,
                column_gap: px(6),
                ..default()
            },
            Visibility::Hidden,
            // Hovering a label spotlights the author's work; clicking it follows them.
            Hovered::default(),
            AuthorLabel(index),
        ))
        .add_children(&[avatar, name])
        .id();
    Author {
        merged: false,
        avatar_from: None,
        label,
        avatar,
        name,
        highlighted: false,
        pos: Vec3::ZERO,
        angle: 0.0,
        work_angle: None,
        on_halo: false,
        last_active: f64::NEG_INFINITY,
        commits: 0,
        active: false,
        crowded: false,
        dashed_at: f64::NEG_INFINITY,
    }
}

#[allow(clippy::too_many_arguments)]
fn controls(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    typed: Res<ButtonInput<bevy::input::keyboard::Key>>,
    history: Option<ResMut<History>>,
    authors: Option<Res<Authors>>,
    mut settings: ResMut<Settings>,
    mut orbit: ResMut<Orbit>,
    mut director: ResMut<Director>,
    mut mode: ResMut<View>,
    mut window: Single<&mut Window>,
    mut nodes: Query<&mut Node>,
) {
    if keys::typed(&typed, "v") {
        *mode = match *mode {
            View::Tree => View::Ziggurat,
            View::Ziggurat => View::Tree,
        };
        info!("view: {:?}", *mode);
    }
    if keys::typed(&typed, "c") {
        director.mode = match director.mode {
            CameraMode::Cinematic => CameraMode::Orbit,
            CameraMode::Orbit => CameraMode::Cinematic,
        };
        info!("camera: {:?}", director.mode);
    }
    if keys.just_pressed(KeyCode::F11) {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        };
    }
    if keys::typed(&typed, "r") {
        (orbit.yaw, orbit.pitch, orbit.zoom) = (0.0, DEFAULT_PITCH, 1.0);
    }
    let zoom = (KEY_ZOOM_SPEED.ln() * time.delta_secs()).exp();
    if keys::holding(&typed, &["+", "="]) || keys.pressed(KeyCode::NumpadAdd) {
        orbit.zoom = (orbit.zoom / zoom).max(0.05);
    }
    if keys::holding(&typed, &["-", "_"]) || keys.pressed(KeyCode::NumpadSubtract) {
        orbit.zoom = (orbit.zoom * zoom).min(10.0);
    }
    if keys::typed(&typed, "n")
        && let Some(authors) = &authors
    {
        settings.hide.names = !settings.hide.names;
        let display = if settings.hide.names {
            Display::None
        } else {
            Display::Flex
        };
        for author in &authors.list {
            if let Ok(mut node) = nodes.get_mut(author.name) {
                node.display = display;
            }
        }
    }
    let Some(mut history) = history else {
        return;
    };
    if keys::typed(&typed, "b") {
        history.reading.branches = !history.reading.branches;
        // The tree then shows other states of the files: it is rebuilt from the start.
        history.remake = true;
        history.seek = Some(history.next);
        info!("branches: {}", history.reading.branches);
    }
    if keys.just_pressed(KeyCode::Space) {
        history.paused = !history.paused;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        history.seconds_per_day /= 2.0;
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        history.seconds_per_day *= 2.0;
    }
}

/// Reads a PNG or JPEG from disk.
fn load_image(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let sampler = ImageSampler::default();
    Image::from_buffer(
        &bytes,
        ImageType::Extension(&extension),
        CompressedImageFormats::NONE,
        true,
        sampler,
        RenderAssetUsages::default(),
    )
    .map(rgba)
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// A 16-bit grayscale PNG (with or without alpha) decodes to an integer texture, which UI
/// materials (the round avatars) can't sample, and wgpu then stops the app: make it 8-bit RGBA.
fn rgba(image: Image) -> Image {
    let channels = match image.texture_descriptor.format {
        TextureFormat::R16Uint => 1,
        TextureFormat::Rg16Uint => 2,
        _ => return image,
    };
    let Some(data) = &image.data else {
        return image;
    };
    // Little-endian 16-bit values: their high byte is the 8-bit value.
    let pixels = data
        .chunks_exact(2 * channels)
        .flat_map(|p| {
            let alpha = if channels == 2 { p[3] } else { u8::MAX };
            [p[1], p[1], p[1], alpha]
        })
        .collect();
    let size = image.texture_descriptor.size;
    Image::new(
        size,
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        image.asset_usage,
    )
}

/// Shows a local image as the author's avatar; GitHub's is then not used.
fn set_avatar(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    author: &mut Author,
    path: &Path,
) {
    match load_image(path) {
        Ok(image) => {
            author.avatar_from = Some(LOCAL_AVATAR);
            commands
                .entity(author.avatar)
                .insert(ImageNode::new(images.add(image)));
        }
        Err(e) => warn!("avatar: {e}"),
    }
}

/// Applies what GitHub knows as it arrives: merges people with the same account or profile
/// name, renames them (a profile can give a first name), and swaps in avatars.
#[allow(clippy::too_many_arguments)]
fn apply_identities(
    settings: Res<Settings>,
    mut identities: ResMut<Identities>,
    mut authors: ResMut<Authors>,
    mut images: ResMut<Assets<Image>>,
    mut texts: Query<&mut Text>,
    mut commands: Commands,
) {
    let Identities {
        receiver,
        done,
        finished,
        ..
    } = &mut *identities;
    if *finished {
        return;
    }
    let Ok(receiver) = receiver.get_mut() else {
        return;
    };
    let mut learned = false;
    loop {
        let identity = match receiver.try_recv() {
            Ok(identity) => identity,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                *finished = true;
                break;
            }
        };
        *done += 1;
        let Some(a) = authors.people.learn(
            &identity.email,
            identity.login.as_deref(),
            identity.profile.as_deref(),
        ) else {
            continue;
        };
        learned = true;
        for (from, into) in authors.people.take_merges() {
            authors.merge(from, into);
        }
        // A local avatar named after the login wins over GitHub's.
        if authors.list[a].avatar_from != Some(LOCAL_AVATAR)
            && let Some(path) = identity
                .login
                .as_ref()
                .and_then(|login| settings.avatars.get(&login.to_lowercase()))
        {
            set_avatar(&mut commands, &mut images, &mut authors.list[a], path);
        }
        let weight = authors
            .email_commits
            .get(&identity.email)
            .copied()
            .unwrap_or(0);
        let author = &mut authors.list[a];
        if author.avatar_from.is_some_and(|shown| shown >= weight) {
            continue;
        }
        if let Some((bytes, mime)) = identity.avatar {
            let image = Image::from_buffer(
                &bytes,
                ImageType::MimeType(&mime),
                CompressedImageFormats::NONE,
                true,
                ImageSampler::default(),
                RenderAssetUsages::default(),
            )
            .map(rgba);
            match image {
                Ok(image) => {
                    author.avatar_from = Some(weight);
                    commands
                        .entity(author.avatar)
                        .insert(ImageNode::new(images.add(image)));
                }
                Err(e) => warn!("avatar of {}: {e}", identity.email),
            }
        }
    }
    if learned {
        for a in authors.people.name_all() {
            if let Ok(mut text) = texts.get_mut(authors.list[a].name) {
                text.0 = authors.people.list[a].display.clone();
            }
        }
    }
}

/// Advances the history clock, queues the changes of every commit it reaches, and applies them
/// at a capped rate: a huge commit sweeps through its files instead of landing in one frame.
fn playback(
    time: Res<Time>,
    mut history: ResMut<History>,
    mut tree: ResMut<Tree>,
    mut authors: ResMut<Authors>,
    mut branches: ResMut<Branches>,
    mut palette: ResMut<Palette>,
    mut pace: ResMut<Pace>,
) {
    if history.paused || history.finished() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs_f64();
    let (mut changes, mut reached) = (0, 0);
    let rate =
        (CHANGES_PER_SECOND * history.speed()).max(history.pending.len() as f64 / SWEEP_SECONDS);
    let History {
        commits,
        next,
        time: repo_time,
        seconds_per_day,
        pending,
        budget,
        reading,
        ..
    } = &mut *history;

    if (pending.len() as f64) < rate * BACKLOG_PAUSE_SECONDS {
        let history_per_second = 86400.0 / *seconds_per_day;
        *repo_time += dt * history_per_second;
        if let Some(commit) = commits.get(*next)
            && commit.time as f64 > *repo_time + AUTO_SKIP_SECONDS * history_per_second
        {
            *repo_time = commit.time as f64;
        }
        while let Some(commit) = commits.get(*next).filter(|c| c.time as f64 <= *repo_time) {
            // Commits without files (merges) still show their author at work.
            if commit.changes.is_empty() {
                let a = authors.by_email(&commit.email);
                authors.list[a].last_active = now;
            }
            // With `--branches`, a merge on the main line brings its branch in.
            if reading.branches {
                branches.merge(*next, now);
            }
            reading.queue(commits, *next, pending);
            *next += 1;
            reached += 1;
        }
    }

    *budget = (*budget + rate * dt).min(rate.max(1.0));
    while *budget >= 1.0 {
        let Some(q) = pending.pop_front() else {
            *budget = 0.0;
            break;
        };
        let commit = &commits[q.commit];
        let a = authors.by_email(&commit.email);
        match commit.merged_by {
            // A ghost reached after its merge (a long backlog) would wait for nothing.
            Some(merge) if q.ghost => {
                if merge >= *next {
                    branches.add(
                        &tree,
                        commits,
                        merge,
                        q.commit,
                        q.change,
                        a,
                        &mut palette,
                        now,
                    );
                }
            }
            _ => tree.apply(
                &commit.changes[q.change],
                Touch {
                    author: a,
                    commit: q.commit,
                    time: commit.time,
                    animate: true,
                },
            ),
        }
        authors.list[a].last_active = now;
        if q.change == 0 {
            authors.list[a].dashed_at = now;
        }
        *budget -= 1.0;
        changes += 1;
    }
    pace.update(changes, reached, dt);
}

fn layout(time: Res<Time>, mut tree: ResMut<Tree>) {
    tree.update(time.delta_secs());
}

impl Scene {
    /// Mirrors the tree's changes into the view: new files get their kind, touched ones start
    /// glowing, deleted ones burst into sparks.
    fn absorb(&mut self, tree: &mut Tree, palette: &mut Palette, sparks: &mut Sparks, now: f64) {
        let Scene {
            files,
            glowing,
            counts,
            deletions,
            recent,
            ..
        } = self;
        // Taken out and handed back, so the event buffer keeps its capacity.
        let mut events = std::mem::take(&mut tree.events);
        for event in events.drain(..) {
            match event {
                Event::Added(id) => {
                    if files.len() <= id {
                        files.resize_with(id + 1, || None);
                    }
                    // Already gone if it was deleted within the same frame.
                    if let Some((_, _, file)) = tree.find(id) {
                        let kind = palette.kind(&file.name);
                        if counts.len() <= kind {
                            counts.resize(kind + 1, 0);
                        }
                        counts[kind] += 1;
                        files[id] = Some(FileView {
                            kind,
                            action: Action::Add,
                            touched_at: f64::NEG_INFINITY,
                            touched_by: 0,
                            glowing: false,
                            changes: 0,
                            last_commit: 0,
                            heat: 0.0,
                            heat_at: 0,
                        });
                    }
                }
                Event::Deleted { id, pos, touch } => {
                    let Some(view) = files[id].take() else {
                        continue;
                    };
                    counts[view.kind] -= 1;
                    if touch.animate {
                        sparks.burst(pos, palette.colors[view.kind], now);
                        deletions.push(Deletion {
                            author: touch.author,
                            pos,
                            at: now,
                        });
                    }
                }
                Event::Touched { id, touch, action } => {
                    let Some(Some(view)) = files.get_mut(id) else {
                        continue;
                    };
                    view.changes += 1;
                    view.last_commit = touch.commit;
                    view.heat = view.heat(touch.time as f64) + 1.0;
                    view.heat_at = touch.time;
                    view.touched_by = touch.author;
                    view.action = action;
                    if touch.animate {
                        view.touched_at = now;
                        if !view.glowing {
                            view.glowing = true;
                            glowing.push(id);
                        }
                        if recent.len() <= touch.author {
                            recent.resize_with(touch.author + 1, VecDeque::new);
                        }
                        let theirs = &mut recent[touch.author];
                        theirs.push_back(id);
                        if theirs.len() > RECENT_FILES {
                            theirs.pop_front();
                        }
                    }
                }
            }
        }
        tree.events = events;
    }

    /// A copy for a timeline snapshot.
    fn snapshot(&self) -> Self {
        let mut copy = self.clone();
        copy.calm();
        copy
    }

    /// Stops every glow and beam, after a jump through history.
    fn calm(&mut self) {
        for &id in &self.glowing {
            if let Some(Some(view)) = self.files.get_mut(id) {
                view.glowing = false;
                view.touched_at = f64::NEG_INFINITY;
            }
        }
        self.glowing.clear();
        self.deletions.clear();
        self.recent.clear();
    }

    /// Center and radius of where author `a` (or anyone merged into them) worked last, with
    /// files drawn where `place` says (see `place`).
    fn work_area(
        &self,
        place: &impl Fn(FileId) -> Option<Vec3>,
        authors: &Authors,
        a: usize,
    ) -> Option<(Vec3, f32)> {
        let positions = || {
            let theirs = (0..self.recent.len()).filter(move |&b| authors.resolve(b) == a);
            theirs.flat_map(|b| self.recent[b].iter().filter_map(|&id| place(id)))
        };
        let count = positions().count();
        if count == 0 {
            return None;
        }
        let center = positions().sum::<Vec3>() / count as f32;
        let radius = positions().map(|p| p.distance(center)).fold(0.0, f32::max);
        Some((center, radius))
    }
}

/// Where file `id` is drawn: in the tree, or on the ziggurat's live base.
fn place<'a>(tree: &'a Tree, zig: Option<&'a Ziggurat>) -> impl Fn(FileId) -> Option<Vec3> + 'a {
    move |id| match zig {
        Some(zig) => zig.pos(id),
        None => tree.file_pos(id),
    }
}

/// Builds the ziggurat the first time it is shown.
fn ziggurat_view(
    mode: Res<View>,
    history: Res<History>,
    zig: Option<Res<Ziggurat>>,
    mut palette: ResMut<Palette>,
    mut commands: Commands,
) {
    if *mode == View::Ziggurat && zig.is_none() {
        commands.insert_resource(Ziggurat::build(&history.commits, &mut palette));
    }
}

fn sync_scene(
    time: Res<Time>,
    mut tree: ResMut<Tree>,
    mut scene: ResMut<Scene>,
    mut palette: ResMut<Palette>,
    mut sparks: ResMut<Sparks>,
) {
    scene.absorb(
        &mut tree,
        &mut palette,
        &mut sparks,
        time.elapsed_secs_f64(),
    );
}

fn action_color(action: Action) -> LinearRgba {
    LinearRgba::from(match action {
        Action::Add => ADD_COLOR,
        Action::Modify => MODIFY_COLOR,
        Action::Delete => DELETE_COLOR,
    })
}

/// Rebuilds the file layer's instances: a tight loop over live files (no per-file entity),
/// then the sparks.
#[allow(clippy::too_many_arguments)]
fn update_files(
    time: Res<Time>,
    history: Res<History>,
    tree: Res<Tree>,
    palette: Res<Palette>,
    authors: Res<Authors>,
    hover: Res<Hover>,
    focus: Res<Focus>,
    branches: Res<Branches>,
    mode: Res<View>,
    last_seek: Res<timeline::LastSeek>,
    zig: Option<ResMut<Ziggurat>>,
    pace: Res<Pace>,
    mut scene: ResMut<Scene>,
    mut sparks: ResMut<Sparks>,
    mut layer: Single<&mut FileInstances>,
) {
    let now = time.elapsed_secs_f64();
    let fade = pace.glow;
    let spot = hover.spotlight(*focus).map(|a| authors.resolve(a));
    let any_highlighted = authors.list.iter().any(|a| a.highlighted);
    let Scene {
        files: views,
        glowing,
        offsets,
        deletions,
        ..
    } = &mut *scene;
    glowing.retain(|&id| match &mut views[id] {
        Some(view) => {
            view.glowing = glow(now, view.touched_at, fade) > 0.0;
            view.glowing
        }
        None => false,
    });
    deletions.retain(|d| glow(now, d.at, fade) > 0.0);
    let instances = &mut layer.0;
    instances.clear();
    let mut zig = zig.filter(|_| *mode == View::Ziggurat);
    if let Some(zig) = zig.as_mut() {
        if last_seek.is_changed() {
            zig.forget_files();
        }
        zig.map_files(&tree);
        zig.past_instances(history.time, time.delta_secs(), &palette.colors, instances);
    }
    let zig = zig.as_deref();
    for dir in tree.dirs.iter().filter(|d| d.alive) {
        while offsets.len() < dir.files.len() {
            offsets.push(file_offset(offsets.len()));
        }
        for (file, offset) in dir.files.iter().zip(offsets.iter()) {
            let Some(view) = &mut views[file.id] else {
                continue;
            };
            let (position, size) = match zig {
                Some(zig) => match zig.pos(file.id) {
                    Some(position) => (position, ziggurat::FILE_SCALE),
                    None => continue,
                },
                None => (dir.pos + *offset, 1.0),
            };
            // Most files are cold (heat 0 or 1): no swell, and no exponential to compute.
            let mut swell = 1.0;
            if view.heat > 1.0 {
                let heat = view.heat(history.time);
                if heat > 1.0 {
                    swell = self::swell(heat);
                } else {
                    view.heat = 0.0;
                }
            }
            let mut color = palette.colors[view.kind];
            let mut glow = 0.0;
            if view.glowing {
                // A fresh change shows its action's color, fading back to the file's own.
                glow = self::glow(now, view.touched_at, fade);
                color = color.mix(&action_color(view.action), glow);
            }
            let mut emissive = FILE_EMISSIVE + GLOW_EMISSIVE * glow;
            let mut scale = swell + GLOW_GROWTH * glow;
            if spot.is_some() || any_highlighted {
                let by = authors.resolve(view.touched_by);
                match spot {
                    Some(a) if a == by => emissive += SPOT_EMISSIVE,
                    Some(_) => color *= SPOT_DIM,
                    None if authors.list[by].highlighted => emissive += HIGHLIGHT_EMISSIVE,
                    None => {}
                }
            }
            if hover.file == Some(file.id) {
                scale *= HOVER_GROWTH;
                emissive += SPOT_EMISSIVE;
            }
            instances.push(FileInstance {
                position,
                scale: scale * size,
                color: [color.red, color.green, color.blue, emissive],
            });
        }
    }
    sparks.update(now, time.delta_secs());
    // Ghosts and sparks are where files are in the tree.
    if zig.is_none() {
        branches.instances(now, fade, &palette.colors, instances);
        sparks.instances(now, instances);
    }
}

/// A shown author on the halo: index, screen angle of their work, its center, where they are now.
type HaloEntry = (usize, f32, Option<Vec3>, f32);

/// Lays active authors out on a halo around the tree, in the camera's view plane, ordered by
/// where they work (so beams stay short and don't cross the screen), evenly spaced, gliding
/// when someone joins or leaves. On each commit an avatar darts towards its work and back.
#[allow(clippy::too_many_arguments)]
fn move_authors(
    time: Res<Time>,
    orbit: Res<Orbit>,
    camera: Single<&Camera, With<Camera3d>>,
    tree: Res<Tree>,
    scene: Res<Scene>,
    mode: Res<View>,
    zig: Option<Res<Ziggurat>>,
    pace: Res<Pace>,
    mut active: Local<Vec<HaloEntry>>,
    mut authors: ResMut<Authors>,
) {
    let place = place(&tree, zig.as_deref().filter(|_| *mode == View::Ziggurat));
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    let ease = (dt * HALO_EASE).min(1.0);
    let smoothing = (dt / WORK_SMOOTHING_SECONDS).min(1.0);
    // The signed shortest turn from angle `a` to angle `b`.
    let turn = |a: f32, b: f32| (b - a + PI).rem_euclid(TAU) - PI;
    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    let (right, up) = (rotation * Vec3::X, rotation * Vec3::Y);
    // Half the view height at the focus, with Bevy's default vertical field of view (π/4).
    let radius_y = orbit.distance * (FRAC_PI_4 / 2.0).tan() * HALO_SIZE;
    // The view's shape, not the window's: they differ while recording.
    let aspect = camera
        .logical_viewport_size()
        .map_or(1.0, |size| size.x / size.y.max(1.0));
    let radius_x = radius_y * aspect.clamp(1.0, 1.6);

    authors.limit_shown(now, pace.idle);
    // Each shown author, the screen angle of their work around the focus, and its center.
    active.clear();
    for a in 0..authors.list.len() {
        if !authors.list[a].shown() {
            authors.list[a].work_angle = None;
            continue;
        }
        let work = scene
            .work_area(&place, &authors, a)
            .map(|(center, _)| center);
        let author = &mut authors.list[a];
        if let Some(c) = work {
            let target = (c - orbit.focus)
                .dot(up)
                .atan2((c - orbit.focus).dot(right));
            let smoothed = author
                .work_angle
                .map_or(target, |w| w + turn(w, target) * smoothing);
            author.work_angle = Some(smoothed);
        }
        let wanted = author.work_angle.unwrap_or(author.angle);
        // Where they are now keeps the order; newcomers go where they work.
        let at = if author.on_halo { author.angle } else { wanted };
        active.push((a, wanted, work, at.rem_euclid(TAU)));
    }
    for author in &mut authors.list {
        author.on_halo &= author.shown();
    }
    active.sort_unstable_by(|x, y| x.3.total_cmp(&y.3));
    // Then neighbours whose work is clearly the other way round swap, one place per frame.
    for k in 1..active.len() {
        if turn(active[k - 1].1, active[k].1) < -HALO_SWAP_ANGLE {
            active.swap(k - 1, k);
        }
    }
    // Evenly spaced slots in that order, turned to best match where everyone works.
    let n = active.len() as f32;
    let step = |k: usize| TAU * k as f32 / n;
    let (sin, cos) = active
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(s, c), (k, x)| {
            let d = x.1 - step(k);
            (s + d.sin(), c + d.cos())
        });
    let offset = f32::atan2(sin, cos);
    for (k, &(a, _, _, _)) in active.iter().enumerate() {
        let slot = step(k) + offset;
        let author = &mut authors.list[a];
        if author.on_halo {
            // Glide the short way round to the new slot, calmly.
            let max = HALO_MAX_SPEED * dt;
            author.angle += (turn(author.angle, slot) * ease).clamp(-max, max);
        } else {
            author.angle = slot;
            author.on_halo = true;
        }
    }
    // Keep neighbours apart: gliding at different paces can bunch them up.
    if active.len() > 1 {
        let gap = HALO_MIN_GAP * TAU / n;
        let max = HALO_PUSH_SPEED * dt;
        for k in 0..active.len() {
            let (a, b) = (active[k].0, active[(k + 1) % active.len()].0);
            let between = turn(authors.list[a].angle, authors.list[b].angle);
            if between < gap {
                let push = ((gap - between) / 2.0).min(max);
                authors.list[a].angle -= push;
                authors.list[b].angle += push;
            }
        }
    }
    for &(a, _, work, _) in active.iter() {
        let author = &mut authors.list[a];
        let theta = author.angle;
        let on_halo =
            orbit.focus + right * (radius_x * theta.cos()) + up * (radius_y * theta.sin());
        let dash = ((now - author.dashed_at) / DASH_SECONDS).clamp(0.0, 1.0);
        let pulse = (dash as f32 * PI).sin() * DASH_REACH;
        author.pos = work.map_or(on_halo, |w| on_halo.lerp(w, pulse));
    }
}

/// Orbits around the tree, keeping it framed: drag to rotate, scroll to zoom. Left alone, the
/// cinematic camera (`director.rs`) directs the shots, calmly.
#[allow(clippy::too_many_arguments)]
fn orbit_camera(
    time: Res<Time>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    tree: Res<Tree>,
    scene: Res<Scene>,
    authors: Res<Authors>,
    focus: Res<Focus>,
    scrubbing: Res<timeline::Scrubbing>,
    last_seek: Res<timeline::LastSeek>,
    mode: Res<View>,
    zig: Option<Res<Ziggurat>>,
    mut director: ResMut<Director>,
    mut orbit: ResMut<Orbit>,
    mut seen: Local<Option<(f32, f32, f32)>>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    let dragging = buttons.pressed(MouseButton::Left) && !scrubbing.0;
    // Whatever the user does to the camera hands it over for a while: drags, the wheel, clicks,
    // jumps (the framing changes at once), and keys (seen as changes since the last frame).
    let keyed = seen.is_some_and(|s| s != (orbit.yaw, orbit.pitch, orbit.zoom));
    let clicked = buttons.get_just_pressed().next().is_some();
    if (dragging && motion.delta != Vec2::ZERO)
        || scroll.delta.y != 0.0
        || clicked
        || keyed
        || last_seek.is_changed()
    {
        director.input(now);
    }
    if dragging {
        orbit.yaw -= motion.delta.x * 0.005;
        orbit.pitch = (orbit.pitch - motion.delta.y * 0.005).clamp(-1.5, 1.5);
    }
    orbit.zoom = (orbit.zoom * (1.0 - scroll.delta.y * 0.1)).clamp(0.05, 10.0);

    // The ziggurat is a still overview: the camera only turns around it.
    let zig = zig.as_deref().filter(|_| *mode == View::Ziggurat);
    if matches!(*focus, Focus::Tree) && zig.is_none() && director.directing(now) {
        let framing = director.frame(now, dt, &tree, &scene, &authors);
        orbit.yaw += AUTO_ROTATE_SPEED * framing.rotation * dt;
        orbit.pitch = director::ease_pitch(orbit.pitch, framing.pitch, dt);
        orbit.zoom = director::ease_distance(orbit.zoom, 1.0, dt);
        let distance = framing.radius.max(FILE_FOCUS_RADIUS) * 1.8 * orbit.zoom;
        orbit.focus = director::ease_focus(orbit.focus, framing.center, orbit.distance, dt);
        orbit.distance = director::ease_distance(orbit.distance, distance, dt);
    } else {
        if !dragging {
            orbit.yaw += AUTO_ROTATE_SPEED * dt;
        }
        let place = place(&tree, zig);
        let (center, radius) = match *focus {
            Focus::File(id) => place(id).map(|p| (p, FILE_FOCUS_RADIUS)),
            Focus::Author(a) => scene
                .work_area(&place, &authors, a)
                .map(|(c, r)| (c, r.max(AUTHOR_FOCUS_RADIUS))),
            Focus::Tree => None,
        }
        .unwrap_or_else(|| zig.map_or_else(|| tree.bounds(), Ziggurat::bounds));
        let ease = (dt * 2.0).min(1.0);
        orbit.focus = orbit.focus.lerp(center, ease);
        let distance = radius.max(FILE_FOCUS_RADIUS) * 1.8 * orbit.zoom;
        orbit.distance += (distance - orbit.distance) * ease;
    }
    *seen = Some((orbit.yaw, orbit.pitch, orbit.zoom));

    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    camera.translation = orbit.focus + rotation * Vec3::Z * orbit.distance;
    camera.look_at(orbit.focus, Vec3::Y);
}

fn thin_lines(mut config: ResMut<GizmoConfigStore>) {
    config.config_mut::<DefaultGizmoConfigGroup>().0.line.width = LINE_WIDTH;
}

#[allow(clippy::too_many_arguments)]
fn draw_lines(
    time: Res<Time>,
    settings: Res<Settings>,
    tree: Res<Tree>,
    scene: Res<Scene>,
    authors: Res<Authors>,
    hover: Res<Hover>,
    focus: Res<Focus>,
    branches: Res<Branches>,
    mode: Res<View>,
    zig: Option<Res<Ziggurat>>,
    pace: Res<Pace>,
    mut gizmos: Gizmos,
) {
    let now = time.elapsed_secs_f64();
    let fade = pace.glow;
    let zig = zig.as_deref().filter(|_| *mode == View::Ziggurat);
    // The ziggurat has no branches: only the beams to its live base.
    if !settings.hide.branches && zig.is_none() {
        for dir in tree.dirs.iter().filter(|d| d.alive) {
            if let Some(parent) = dir.parent {
                gizmos.line(tree.dirs[parent].pos, dir.pos, BRANCH_COLOR);
            }
        }
    }
    // The hovered file's branch, back to the root.
    if let Some((mut dir, _, _)) = hover.file.and_then(|id| tree.find(id)) {
        while let Some(parent) = tree.dirs[dir].parent {
            gizmos.line(
                tree.dirs[parent].pos,
                tree.dirs[dir].pos,
                BRANCH_HOVER_COLOR,
            );
            dir = parent;
        }
    }
    if zig.is_none() {
        branches.lines(&mut gizmos);
    }
    if settings.hide.beams {
        return;
    }
    let place = place(&tree, zig);
    // Beams from each author to the files they just touched, colored by what they did.
    let spot = hover.spotlight(*focus).map(|a| authors.resolve(a));
    let mut beam = |author: usize, to: Vec3, action: Action, glow: f32| {
        let author = authors.resolve(author);
        // Someone off the halo has nowhere to shine from (their files still glow).
        if !authors.list[author].shown() {
            return;
        }
        let dim = if spot.is_some_and(|s| s != author) {
            SPOT_BEAM_DIM
        } else {
            1.0
        };
        let color = action_color(action) * BEAM_BRIGHTNESS;
        gizmos.line(
            authors.list[author].pos,
            to,
            LinearRgba {
                alpha: glow * dim,
                ..color
            },
        );
    };
    // When busy, only a share of the changes get a beam (`Pace`); all of them still glow.
    for &id in &scene.glowing {
        if let (Some(Some(view)), Some(pos)) = (scene.files.get(id), place(id))
            && pace.beam(id as u64, view.last_commit as u64)
        {
            beam(
                view.touched_by,
                pos,
                view.action,
                glow(now, view.touched_at, fade),
            );
        }
    }
    if zig.is_some() {
        return;
    }
    for deletion in scene
        .deletions
        .iter()
        .filter(|d| pace.beam(d.pos.x.to_bits().into(), d.at.to_bits()))
    {
        beam(
            deletion.author,
            deletion.pos,
            Action::Delete,
            glow(now, deletion.at, fade),
        );
    }
    for ghost in branches
        .ghosts()
        .filter(|g| now - g.at < fade && pace.beam(g.dir as u64, g.at.to_bits()))
    {
        beam(
            ghost.author,
            ghost.pos,
            ghost.action,
            glow(now, ghost.at, fade),
        );
    }
}

fn place_labels(
    authors: Res<Authors>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut labels: Query<(&mut Node, &mut Visibility)>,
) {
    let (camera, camera_transform) = *camera;
    for author in &authors.list {
        let Ok((mut node, mut visibility)) = labels.get_mut(author.label) else {
            continue;
        };
        match camera.world_to_viewport(camera_transform, author.pos) {
            Ok(at) if author.shown() => {
                // Center the avatar on the author's position.
                node.left = px(at.x - AVATAR_SIZE / 2.0);
                node.top = px(at.y - AVATAR_SIZE / 2.0);
                visibility.set_if_neq(Visibility::Inherited);
            }
            _ => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_files_swell_and_cool_down() {
        let view = FileView {
            kind: 0,
            action: Action::Modify,
            touched_at: 0.0,
            touched_by: 0,
            glowing: false,
            changes: 3,
            last_commit: 0,
            heat: 3.0,
            heat_at: 0,
        };
        let days = |d: f64| d * 86400.0;
        assert_eq!(view.heat(days(0.0)), 3.0);
        assert!((view.heat(days(HEAT_HALF_LIFE_DAYS)) - 1.5).abs() < 1e-4);
        // A single change doesn't swell; many swell, never past `MAX_SWELL`; all cool down.
        assert_eq!(swell(1.0), 1.0);
        assert!(swell(3.0) > 1.1 && swell(1000.0) <= MAX_SWELL);
        assert!(view.heat(days(120.0)) < 1.0);
    }
}
