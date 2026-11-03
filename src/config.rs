//! Settings: built-in defaults, overridden by a `eridu.toml` (found in the repository or
//! given with `--config`), overridden by command-line flags. TOML keys are the flags' names.

use std::path::{Path, PathBuf};

use crate::identity::Aliases;

use serde::{Deserialize, Serialize};

use crate::director::CameraMode;
use crate::quality::Quality;
use crate::record::{self, Record};
use crate::ziggurat::View;

/// Looked for at the root of the visualized repository.
pub const CONFIG_FILE: &str = "eridu.toml";
/// `--record`'s defaults: Full HD at 60 frames per second.
const DEFAULT_FPS: u32 = 60;
const DEFAULT_SIZE: &str = "1920x1080";

const USAGE: &str = "\
Eridu — the Epic of your Repository

Usage: eridu [options] [repo...]

Several repositories are shown together, each as its own tree, on one timeline.

  -s, --seconds-per-day <n>   Seconds of animation per day of history (default 1)
      --from <date|rev>       Start at a date (YYYY-MM-DD), a commit or a tag
      --to <date|rev>         Stop at a date, a commit or a tag
      --start <percent>       Start part-way through the history, e.g. 40%
      --loop                  Start over when the history ends
      --include <glob>        Only show matching files (repeatable)
      --exclude <glob>        Hide matching files, e.g. 'vendor/' or '*.lock' (repeatable)
      --exclude-user <glob>   Hide an author's commits, by name or email (repeatable)
      --highlight-user <glob> Keep an author always visible and bright (repeatable)
      --follow-user <glob>    Follow an author with the camera
      --hide <items>          Comma-separated: date, hud, legend, timeline, title,
                              names, avatars, branches, beams
      --title <text>          Shown at the top (default: the repository's folder name)
      --logo <image>          A PNG or JPEG shown in the bottom-right corner
      --background <image>    A PNG or JPEG behind the scene
      --avatar-dir <dir>      Avatars by GitHub login, name or email (<login>.png, …)
      --people                Print who's who (names shown, emails, GitHub logins), then quit
      --camera <mode>         cinematic (default: directs its own shots when left alone)
                              or orbit (always frames the whole tree)
      --view <name>           tree (default) or ziggurat: the whole history as stacked
                              layers, the present at the base (`V` switches)
      --branches              Show side branches: their changes wait beside the tree, then
                              flow into it at their merge (`B` toggles it)
      --quality <level>       low, high, ultra or auto (default: auto, which starts at
                              high and steps down if frames get slow)
      --config <file>         Read settings from this file instead of <repo>/eridu.toml
      --save-config <file>    Write the effective settings to a file, then run
      --record <file>         Render a video, then quit: out.mp4 (H.264), .webm, .mkv…
                              through ffmpeg if it's on the PATH, else PNG frames; a
                              path without extension is a folder of PNG frames
      --fps <n>               Frames per second of the video (default 60)
      --size <WxH>            Size of the video (default 1920x1080)
      --bench                 Play the whole history fast, then print frame-time statistics
  -h, --help                  Show this help

Globs: `*` and `?` stay within a path segment, `**` crosses segments. A glob without `/`
matches any segment (`*.lock`); one with `/` is anchored at the repository root (`src/**`).
Paths in a eridu.toml are relative to the file's folder.

People are shown by first name only. In a eridu.toml, a [people] table merges or renames
people, by email or commit name:  Ada = [\"ada@home.org\", \"Ada L.\"]";

/// Everything a `eridu.toml` can set. `None`/empty means "not set here".
#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Repositories to show, when none is given on the command line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_per_day: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Percentage of the history to skip at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub looping: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_user: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hide: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub highlight_user: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<bool>,
    /// The video's frames per second and size (`--record`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// `[people]`: a name to show → the emails or commit names of that person. Last, as
    /// TOML tables come after plain keys.
    #[serde(skip_serializing_if = "Aliases::is_empty")]
    pub people: Aliases,
}

impl Config {
    /// Takes every setting `over` sets; lists are replaced, not merged, so a flag always wins.
    fn overlay(&mut self, over: Config) {
        fn take<T>(into: &mut Option<T>, from: Option<T>) {
            if from.is_some() {
                *into = from;
            }
        }
        fn take_list(into: &mut Vec<String>, from: Vec<String>) {
            if !from.is_empty() {
                *into = from;
            }
        }
        take(&mut self.seconds_per_day, over.seconds_per_day);
        take(&mut self.from, over.from);
        take(&mut self.to, over.to);
        take(&mut self.start, over.start);
        take(&mut self.looping, over.looping);
        take_list(&mut self.include, over.include);
        take_list(&mut self.exclude, over.exclude);
        take_list(&mut self.exclude_user, over.exclude_user);
        take_list(&mut self.hide, over.hide);
        take_list(&mut self.highlight_user, over.highlight_user);
        take(&mut self.follow_user, over.follow_user);
        take(&mut self.title, over.title);
        take(&mut self.logo, over.logo);
        take(&mut self.background, over.background);
        take(&mut self.avatar_dir, over.avatar_dir);
        take(&mut self.quality, over.quality);
        take(&mut self.camera, over.camera);
        take(&mut self.view, over.view);
        take(&mut self.branches, over.branches);
        take(&mut self.fps, over.fps);
        take(&mut self.size, over.size);
        if !over.people.is_empty() {
            self.people = over.people;
        }
    }

    pub fn hidden(&self) -> Result<Hide, String> {
        let mut hide = Hide::default();
        for item in &self.hide {
            let flag = match item.trim() {
                "date" => &mut hide.date,
                "hud" => &mut hide.hud,
                "names" => &mut hide.names,
                "avatars" => &mut hide.avatars,
                "branches" => &mut hide.branches,
                "beams" => &mut hide.beams,
                "legend" => &mut hide.legend,
                "timeline" => &mut hide.timeline,
                "title" => &mut hide.title,
                other => {
                    return Err(format!(
                        "cannot hide `{other}`: expected date, hud, legend, timeline, title, names, avatars, branches or beams"
                    ));
                }
            };
            *flag = true;
        }
        Ok(hide)
    }
}

/// Parts of the scene the user asked not to draw.
#[derive(Default, Clone, Copy)]
pub struct Hide {
    pub date: bool,
    pub hud: bool,
    pub names: bool,
    pub avatars: bool,
    pub branches: bool,
    pub beams: bool,
    pub legend: bool,
    pub timeline: bool,
    pub title: bool,
}

pub struct Options {
    /// At least one.
    pub repos: Vec<PathBuf>,
    pub config: Config,
    pub hide: Hide,
    /// `None` is `auto`.
    pub quality: Option<Quality>,
    pub bench: bool,
    pub camera: CameraMode,
    pub view: View,
    pub branches: bool,
    /// `--record`: render a video and quit.
    pub record: Option<Record>,
    /// `--people`: print who's who and quit.
    pub people: bool,
}

/// Parses the command line and loads the config file it points to (or the repository's).
pub fn load() -> Result<Options, String> {
    parse(std::env::args().skip(1))
}

/// `load` for the given arguments (the program's name left out).
fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let mut flags = Config::default();
    let (mut repos, mut config_file, mut save_to, mut bench, mut people) =
        (Vec::new(), None, None, false, false);
    let mut record_to = None;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-s" | "--seconds-per-day" => flags.seconds_per_day = Some(number(&arg, &value()?)?),
            "--from" => flags.from = Some(value()?),
            "--to" => flags.to = Some(value()?),
            "--start" => flags.start = Some(number(&arg, value()?.trim_end_matches('%'))?),
            "--include" => flags.include.push(value()?),
            "--exclude" => flags.exclude.push(value()?),
            "--exclude-user" => flags.exclude_user.push(value()?),
            "--highlight-user" => flags.highlight_user.push(value()?),
            "--follow-user" => flags.follow_user = Some(value()?),
            "--title" => flags.title = Some(value()?),
            "--logo" => flags.logo = Some(PathBuf::from(value()?)),
            "--background" => flags.background = Some(PathBuf::from(value()?)),
            "--avatar-dir" => flags.avatar_dir = Some(PathBuf::from(value()?)),
            "--quality" => flags.quality = Some(value()?),
            "--camera" => flags.camera = Some(value()?),
            "--view" => flags.view = Some(value()?),
            "--hide" => flags.hide.extend(value()?.split(',').map(str::to_string)),
            "--record" => record_to = Some(PathBuf::from(value()?)),
            "--fps" => {
                flags.fps = Some(
                    value()?
                        .trim()
                        .parse()
                        .map_err(|_| format!("{arg} expects a whole number"))?,
                )
            }
            "--size" => flags.size = Some(value()?),
            "--config" => config_file = Some(PathBuf::from(value()?)),
            "--save-config" => save_to = Some(PathBuf::from(value()?)),
            "--loop" => flags.looping = Some(true),
            "--branches" => flags.branches = Some(true),
            "--bench" => bench = true,
            "--people" => people = true,
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg} (see --help)")),
            _ => repos.push(PathBuf::from(arg)),
        }
    }

    // The config file of the first repository (or of the current folder) applies.
    let mut config = match config_file {
        Some(file) => read(&file)?,
        None => {
            let file = repos
                .first()
                .map_or_else(|| PathBuf::from("."), PathBuf::clone)
                .join(CONFIG_FILE);
            if file.exists() {
                read(&file)?
            } else {
                Config::default()
            }
        }
    };
    config.overlay(flags);
    if repos.is_empty() {
        repos = std::mem::take(&mut config.repos);
    }
    if repos.is_empty() {
        repos.push(PathBuf::from("."));
    }
    if let Some(start) = config.start
        && !(0.0..100.0).contains(&start)
    {
        return Err(format!(
            "start must be a percentage between 0 and 100, not {start}"
        ));
    }
    let hide = config.hidden()?;
    let quality = match config.quality.as_deref().map(str::trim) {
        None | Some("auto") => None,
        Some("low") => Some(Quality::Low),
        Some("high") => Some(Quality::High),
        Some("ultra") => Some(Quality::Ultra),
        Some(other) => {
            return Err(format!(
                "quality must be low, high, ultra or auto, not `{other}`"
            ));
        }
    };
    let camera = match config.camera.as_deref().map(str::trim) {
        None | Some("cinematic") => CameraMode::Cinematic,
        Some("orbit") => CameraMode::Orbit,
        Some(other) => return Err(format!("camera must be cinematic or orbit, not `{other}`")),
    };
    if let Some(file) = save_to {
        // Paths from the command line are relative to the current folder, but a config file's
        // are read from the file's folder: save them whole so they still point at the same files.
        let single = [
            &mut config.logo,
            &mut config.background,
            &mut config.avatar_dir,
        ]
        .into_iter()
        .flatten();
        for path in single.chain(config.repos.iter_mut()) {
            if let Ok(whole) = std::path::absolute(&*path) {
                *path = whole;
            }
        }
        let text = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
        std::fs::write(&file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }
    let view = match config.view.as_deref().map(str::trim) {
        None | Some("tree") => View::Tree,
        Some("ziggurat") => View::Ziggurat,
        Some(other) => return Err(format!("view must be tree or ziggurat, not `{other}`")),
    };
    let branches = config.branches.unwrap_or(false);
    let fps = config.fps.unwrap_or(DEFAULT_FPS);
    if !(1..=240).contains(&fps) {
        return Err(format!("fps must be between 1 and 240, not {fps}"));
    }
    let (width, height) = record::parse_size(config.size.as_deref().unwrap_or(DEFAULT_SIZE))?;
    if bench && record_to.is_some() {
        return Err("--bench and --record don't go together".into());
    }
    let record = record_to.map(|path| Record {
        path,
        fps,
        width,
        height,
    });
    Ok(Options {
        repos,
        config,
        hide,
        quality,
        bench,
        camera,
        view,
        branches,
        record,
        people,
    })
}

/// Reads a config file; its relative paths are taken from the file's folder.
fn read(file: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let mut config: Config =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let folder = file.parent().unwrap_or(Path::new("."));
    let single = [
        &mut config.logo,
        &mut config.background,
        &mut config.avatar_dir,
    ]
    .into_iter()
    .flatten();
    for path in single.chain(config.repos.iter_mut()) {
        if path.is_relative() {
            *path = folder.join(&*path);
        }
    }
    Ok(config)
}

fn number(flag: &str, value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{flag} expects a number, not `{value}`"))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{CONFIG_FILE, Config, Options, parse, read};
    use crate::director::CameraMode;
    use crate::quality::Quality;
    use crate::ziggurat::View;

    /// A folder under the system's temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            let name = format!("eridu-config-{}-{n}", std::process::id());
            let dir = std::env::temp_dir().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn arg(&self) -> String {
            self.0.display().to_string()
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let file = self.0.join(name);
            std::fs::write(&file, text).unwrap();
            file
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|a| a.to_string()))
    }

    fn options(args: &[&str]) -> Options {
        run(args).unwrap_or_else(|e| panic!("{e}"))
    }

    fn error(args: &[&str]) -> String {
        run(args).err().expect("an error")
    }

    /// Parses `flags` for a fresh, empty repository folder (so no stray `eridu.toml` applies).
    fn with_repo(flags: &[&str]) -> Result<Options, String> {
        let repo = TempDir::new();
        let mut args = vec![repo.arg()];
        args.extend(flags.iter().map(|f| f.to_string()));
        parse(args)
    }

    fn ok(flags: &[&str]) -> Options {
        with_repo(flags).unwrap_or_else(|e| panic!("{e}"))
    }

    fn bad(flags: &[&str]) -> String {
        with_repo(flags).err().expect("an error")
    }

    #[test]
    fn defaults_without_flags() {
        let repo = TempDir::new();
        let o = options(&[&repo.arg()]);
        assert_eq!(o.repos, [repo.path()]);
        assert_eq!(o.quality, None);
        assert_eq!(o.camera, CameraMode::Cinematic);
        assert_eq!(o.view, View::Tree);
        assert!(!o.branches && !o.bench && !o.people);
        assert!(o.record.is_none());
        assert!(!o.hide.date && !o.hide.hud && !o.hide.title);
        assert_eq!(o.config.seconds_per_day, None);
    }

    #[test]
    fn value_flags_are_read() {
        let o = ok(&[
            "-s",
            "0.5",
            "--from",
            "2024-01-01",
            "--to",
            "v2",
            "--start",
            "40%",
            "--title",
            "Uruk",
            "--follow-user",
            "ada",
            "--logo",
            "logo.png",
            "--background",
            "bg.jpg",
            "--avatar-dir",
            "faces",
        ]);
        let c = &o.config;
        assert_eq!(c.seconds_per_day, Some(0.5));
        assert_eq!(c.from.as_deref(), Some("2024-01-01"));
        assert_eq!(c.to.as_deref(), Some("v2"));
        assert_eq!(c.start, Some(40.0));
        assert_eq!(c.title.as_deref(), Some("Uruk"));
        assert_eq!(c.follow_user.as_deref(), Some("ada"));
        assert_eq!(c.logo.as_deref(), Some(Path::new("logo.png")));
        assert_eq!(c.background.as_deref(), Some(Path::new("bg.jpg")));
        assert_eq!(c.avatar_dir.as_deref(), Some(Path::new("faces")));
        assert_eq!(
            ok(&["--seconds-per-day", "2"]).config.seconds_per_day,
            Some(2.0)
        );
        assert_eq!(ok(&["--start", "12.5"]).config.start, Some(12.5));
    }

    #[test]
    fn switches_are_read() {
        let o = ok(&["--loop", "--branches", "--bench", "--people"]);
        assert_eq!(o.config.looping, Some(true));
        assert!(o.branches && o.bench && o.people);
    }

    #[test]
    fn repeatable_flags_accumulate() {
        let o = ok(&[
            "--include",
            "src/",
            "--include",
            "*.rs",
            "--exclude",
            "*.lock",
            "--exclude-user",
            "bot*",
            "--exclude-user",
            "ci@x.org",
            "--highlight-user",
            "ada",
        ]);
        assert_eq!(o.config.include, ["src/", "*.rs"]);
        assert_eq!(o.config.exclude, ["*.lock"]);
        assert_eq!(o.config.exclude_user, ["bot*", "ci@x.org"]);
        assert_eq!(o.config.highlight_user, ["ada"]);
    }

    #[test]
    fn hide_takes_comma_lists() {
        let o = ok(&["--hide", "date, names", "--hide", "beams"]);
        assert_eq!(o.config.hide, ["date", " names", "beams"]);
        assert!(o.hide.date && o.hide.names && o.hide.beams);
        assert!(!o.hide.hud && !o.hide.avatars && !o.hide.legend);
        let all = "date,hud,legend,timeline,title,names,avatars,branches,beams";
        let h = ok(&["--hide", all]).hide;
        assert!(h.date && h.hud && h.legend && h.timeline && h.title);
        assert!(h.names && h.avatars && h.branches && h.beams);
        assert!(bad(&["--hide", "date,sky"]).contains("cannot hide `sky`"));
    }

    #[test]
    fn choices_are_read() {
        assert_eq!(ok(&["--quality", "low"]).quality, Some(Quality::Low));
        assert_eq!(ok(&["--quality", "high"]).quality, Some(Quality::High));
        assert_eq!(ok(&["--quality", " ultra "]).quality, Some(Quality::Ultra));
        assert_eq!(ok(&["--quality", "auto"]).quality, None);
        assert_eq!(ok(&["--camera", "orbit"]).camera, CameraMode::Orbit);
        assert_eq!(ok(&["--camera", "cinematic"]).camera, CameraMode::Cinematic);
        assert_eq!(ok(&["--view", "ziggurat"]).view, View::Ziggurat);
        assert_eq!(ok(&["--view", "tree"]).view, View::Tree);
    }

    #[test]
    fn record_takes_fps_and_size() {
        let r = ok(&["--record", "out.mp4"]).record.unwrap();
        assert_eq!(r.path, Path::new("out.mp4"));
        assert_eq!((r.fps, r.width, r.height), (60, 1920, 1080));
        let flags = ["--record", "f", "--fps", " 30 ", "--size", "1280x720"];
        let r = ok(&flags).record.unwrap();
        assert_eq!((r.fps, r.width, r.height), (30, 1280, 720));
        // Without --record they are only checked.
        assert!(ok(&["--fps", "30"]).record.is_none());
    }

    #[test]
    fn bad_command_lines_are_explained() {
        assert_eq!(bad(&["--from"]), "--from needs a value");
        assert_eq!(bad(&["--nope"]), "unknown option --nope (see --help)");
        assert_eq!(bad(&["-s", "fast"]), "-s expects a number, not `fast`");
        assert_eq!(bad(&["--fps", "29.97"]), "--fps expects a whole number");
        assert!(bad(&["--fps", "0"]).starts_with("fps must be between 1 and 240"));
        assert!(bad(&["--fps", "241"]).starts_with("fps must be between 1 and 240"));
        assert!(bad(&["--start", "100"]).starts_with("start must be a percentage"));
        assert!(bad(&["--start", "-1"]).starts_with("start must be a percentage"));
        assert!(bad(&["--quality", "max"]).starts_with("quality must be"));
        assert!(bad(&["--camera", "drone"]).starts_with("camera must be"));
        assert!(bad(&["--view", "pyramid"]).starts_with("view must be"));
        assert!(bad(&["--size", "1920"]).starts_with("size must"));
        let both = bad(&["--bench", "--record", "out.mp4"]);
        assert_eq!(both, "--bench and --record don't go together");
        assert!(error(&["--config", "missing/eridu.toml"]).starts_with("cannot read"));
    }

    #[test]
    fn several_repositories() {
        let (a, b) = (TempDir::new(), TempDir::new());
        let o = options(&[&a.arg(), "--loop", &b.arg()]);
        assert_eq!(o.repos, [a.path(), b.path()]);
    }

    #[test]
    fn repository_config_applies_and_flags_win() {
        let repo = TempDir::new();
        repo.write(
            CONFIG_FILE,
            "seconds-per-day = 3.0\ntitle = \"Eridu\"\nexclude = [\"*.lock\", \"vendor/\"]\n\
             include = [\"src/\"]\nquality = \"low\"\nloop = true\nhide = [\"date\"]\n",
        );
        let o = options(&[&repo.arg(), "-s", "0.25", "--exclude", "docs/"]);
        assert_eq!(o.config.seconds_per_day, Some(0.25));
        assert_eq!(o.config.title.as_deref(), Some("Eridu"));
        // Lists are replaced, not merged.
        assert_eq!(o.config.exclude, ["docs/"]);
        assert_eq!(o.config.include, ["src/"]);
        assert_eq!(o.quality, Some(Quality::Low));
        assert_eq!(o.config.looping, Some(true));
        assert!(o.hide.date);
    }

    #[test]
    fn config_paths_are_relative_to_its_folder() {
        let dir = TempDir::new();
        let file = dir.write(
            "custom.toml",
            "repos = [\"a\", \"b\"]\nlogo = \"logo.png\"\navatar-dir = \"faces\"\n",
        );
        let file = file.display().to_string();
        let o = options(&["--config", &file]);
        assert_eq!(o.repos, [dir.path().join("a"), dir.path().join("b")]);
        assert_eq!(o.config.logo, Some(dir.path().join("logo.png")));
        assert_eq!(o.config.avatar_dir, Some(dir.path().join("faces")));
        assert_eq!(o.config.background, None);
        // Repositories on the command line win over the file's.
        let o = options(&["--config", &file, "here"]);
        assert_eq!(o.repos, [Path::new("here")]);
    }

    #[test]
    fn config_files_are_checked() {
        let dir = TempDir::new();
        let unknown = dir.write("unknown.toml", "colour = \"red\"\n");
        assert!(read(&unknown).err().unwrap().contains("colour"));
        let mistyped = dir.write("mistyped.toml", "fps = \"sixty\"\n");
        assert!(read(&mistyped).is_err());
        let file = dir.write("hide.toml", "hide = [\"clouds\"]\n");
        assert!(error(&["--config", &file.display().to_string()]).contains("cannot hide"));
    }

    #[test]
    fn people_table_is_read() {
        let dir = TempDir::new();
        let file = dir.write(
            "people.toml",
            "title = \"x\"\n[people]\nAda = [\"ada@home.org\", \"Ada L.\"]\nBob = [\"bob@x.org\"]\n",
        );
        let config = read(&file).unwrap();
        assert_eq!(config.people.len(), 2);
        assert_eq!(config.people["Ada"], ["ada@home.org", "Ada L."]);
        assert_eq!(config.people["Bob"], ["bob@x.org"]);
    }

    #[test]
    fn overlay_keeps_what_is_not_set() {
        let mut base = Config {
            title: Some("base".into()),
            fps: Some(30),
            exclude: vec!["a".into()],
            ..Config::default()
        };
        base.people.insert("Ada".into(), vec!["ada@x".into()]);
        base.overlay(Config {
            fps: Some(24),
            ..Config::default()
        });
        assert_eq!(base.title.as_deref(), Some("base"));
        assert_eq!(base.fps, Some(24));
        assert_eq!(base.exclude, ["a"]);
        assert_eq!(base.people.len(), 1);
        let mut people = Config::default();
        people.people.insert("Bob".into(), vec!["bob@x".into()]);
        base.overlay(people);
        assert_eq!(base.people.keys().collect::<Vec<_>>(), ["Bob"]);
    }

    #[test]
    fn saved_config_reads_back_the_same() {
        let dir = TempDir::new();
        dir.write(
            CONFIG_FILE,
            "title = \"From file\"\nexclude = [\"*.lock\"]\n[people]\nAda = [\"ada@x.org\"]\n",
        );
        let saved = dir.path().join("saved.toml").display().to_string();
        let o = options(&[
            &dir.arg(),
            "--save-config",
            &saved,
            "-s",
            "0.5",
            "--hide",
            "date,beams",
            "--branches",
            "--fps",
            "30",
            "--size",
            "1280x720",
            "--start",
            "10",
        ]);
        let text = std::fs::read_to_string(&saved).unwrap();
        let back = read(Path::new(&saved)).unwrap();
        assert_eq!(toml::to_string_pretty(&back).unwrap(), text);
        assert_eq!(back.title.as_deref(), Some("From file"));
        assert_eq!(back.exclude, ["*.lock"]);
        assert_eq!(back.seconds_per_day, Some(0.5));
        assert_eq!(back.hide, ["date", "beams"]);
        assert_eq!(back.branches, Some(true));
        assert_eq!(
            (back.fps, back.size.as_deref()),
            (Some(30), Some("1280x720"))
        );
        assert_eq!(back.start, o.config.start);
        assert_eq!(back.people["Ada"], ["ada@x.org"]);
        // Unset settings aren't written.
        assert!(!text.contains("quality") && !text.contains("logo"));
        // Running from the saved file gives the same settings.
        let again = options(&[&dir.arg(), "--config", &saved]);
        assert_eq!(toml::to_string_pretty(&again.config).unwrap(), text);
    }

    #[test]
    fn save_config_reports_unwritable_files() {
        let dir = TempDir::new();
        let target = dir.path().join("no/such/folder/eridu.toml");
        let e = error(&[&dir.arg(), "--save-config", &target.display().to_string()]);
        assert!(e.starts_with("cannot write"), "{e}");
    }

    #[test]
    fn saved_relative_paths_keep_pointing_at_the_same_file() {
        let dir = TempDir::new();
        let saved = dir.path().join("saved.toml").display().to_string();
        let o = options(&[&dir.arg(), "--logo", "logo.png", "--save-config", &saved]);
        let back = read(Path::new(&saved)).unwrap();
        assert_eq!(back.logo, o.config.logo);
    }
}
