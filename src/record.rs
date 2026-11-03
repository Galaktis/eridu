//! `--record out.mp4`: renders the history to a video. Time advances by exactly one frame per
//! frame (`TimeUpdateStrategy::ManualDuration`), however long a frame takes, so the video is
//! smooth on any machine. The scene renders offscreen at the video's size, each frame is read
//! back from the GPU and handed to a writer thread: `ffmpeg` when it's on the `PATH` (raw
//! frames on its stdin), else a PNG sequence. The window only shows a preview.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::time::TimeUpdateStrategy;
use bevy::ui::IsDefaultUiCamera;
use bevy::window::PrimaryWindow;

use bevy::prelude::*;

use crate::{History, Identities};

/// The video holds on the final state this long before it ends.
const HOLD_SECONDS: f64 = 3.0;
/// Frames waiting for the writer; when it's behind, rendering waits (each 1080p frame is 8 MB).
const QUEUE_FRAMES: usize = 4;
/// Readbacks arrive a few frames late: after the last frame, wait at most this many for them.
const DRAIN_FRAMES: u32 = 30;
/// The history holds at its start this many frames before recording, so the late readbacks
/// that open the video already show the start (and every run starts on the same frame).
const WARMUP_FRAMES: u32 = 10;
/// The window title shows the progress, refreshed this often (in frames).
const TITLE_REFRESH_FRAMES: u32 = 30;
/// Containers that get H.264 (`libx264`), the codec every player reads; other extensions
/// (`.webm`, `.gif`…) get ffmpeg's own choice.
const H264_CONTAINERS: &[&str] = &["mp4", "mov", "mkv"];
/// x264's quality (lower is better, 18 looks lossless) and speed/size trade-off.
const H264_CRF: &str = "18";
const H264_PRESET: &str = "medium";

/// `--record`, `--fps`, `--size`.
#[derive(Clone, Debug)]
pub struct Record {
    pub path: PathBuf,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
}

pub struct RecordPlugin(pub Option<Record>);

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        let Some(record) = self.0.clone() else {
            return;
        };
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / record.fps as f64,
        )))
        .insert_resource(Recorder::new(record))
        .add_systems(PostStartup, capture)
        .add_systems(Last, finish);
    }
}

/// The video being written.
#[derive(Resource)]
pub struct Recorder {
    record: Record,
    frames: Option<SyncSender<Vec<u8>>>,
    writer: Option<JoinHandle<Result<String, String>>>,
    /// Frames rendered since recording started, and frames sent to the writer (a few behind).
    rendered: u32,
    written: u32,
    /// Recording starts once the history is loaded and at its start position, and GitHub has
    /// answered about every author (a video has all its avatars from the first frame).
    started: bool,
    /// When the history ended (in app time), then the frame count to stop at.
    ended_at: Option<f64>,
    stop_at: Option<u32>,
    /// Frames rendered since `stop_at` was set, waiting for the late readbacks.
    draining: u32,
    /// Frames held at the start, once every avatar is in.
    warmup: u32,
    began: Instant,
}

impl Recorder {
    fn new(record: Record) -> Self {
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_FRAMES);
        let writer = {
            let record = record.clone();
            std::thread::spawn(move || write_frames(&record, rx))
        };
        Self {
            record,
            frames: Some(tx),
            writer: Some(writer),
            rendered: 0,
            written: 0,
            started: false,
            ended_at: None,
            stop_at: None,
            draining: 0,
            warmup: 0,
            began: Instant::now(),
        }
    }
}

/// Points the camera at an offscreen image of the video's size, reads it back every frame,
/// and shows it in the window.
fn capture(
    recorder: Res<Recorder>,
    camera: Single<Entity, With<Camera3d>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
) {
    let Record { width, height, .. } = recorder.record;
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    let image = images.add(image);
    // The HUD and avatars go into the video, not the window.
    commands
        .entity(*camera)
        .insert((RenderTarget::from(image.clone()), IsDefaultUiCamera));
    commands
        .spawn(Readback::texture(image.clone()))
        .observe(receive_frame);
    let preview = commands
        .spawn((
            Camera2d,
            Camera {
                order: 1,
                ..default()
            },
        ))
        .id();
    commands.spawn((
        ImageNode::new(image),
        Node {
            width: percent(100),
            height: percent(100),
            ..default()
        },
        UiTargetCamera(preview),
    ));
    window
        .resolution
        .set(width as f32 / 2.0, height as f32 / 2.0);
    info!(
        "recording {}x{} at {} fps to {}",
        width,
        height,
        recorder.record.fps,
        recorder.record.path.display()
    );
}

/// A frame came back from the GPU: hand it to the writer (waiting if it's behind).
fn receive_frame(frame: On<ReadbackComplete>, mut recorder: ResMut<Recorder>) {
    if !recorder.started
        || recorder
            .stop_at
            .is_some_and(|stop| recorder.written >= stop)
    {
        return;
    }
    // The image before its first render is all zeros (the scene's background never is).
    if recorder.written == 0 && frame.data.iter().all(|&b| b == 0) {
        return;
    }
    let Some(frames) = &recorder.frames else {
        return;
    };
    if frames.send(frame.data.clone()).is_ok() {
        recorder.written += 1;
    } else {
        // The writer gave up (ffmpeg failed): stop now, `finish` reports why.
        recorder.stop_at = Some(recorder.written);
    }
}

/// Starts with the history (once every avatar is in), stops a little after its end, then
/// waits for the writer and quits.
#[allow(clippy::too_many_arguments)]
fn finish(
    time: Res<Time>,
    history: Option<ResMut<History>>,
    identities: Option<Res<Identities>>,
    mut recorder: ResMut<Recorder>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    readbacks: Query<Entity, With<Readback>>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut history) = history else {
        return;
    };
    if !recorder.started {
        // Hold the history at its start meanwhile.
        history.paused = true;
        match identities.as_deref() {
            Some(i) if !i.finished => {
                window.title =
                    format!("Eridu — waiting for GitHub: avatars {}/{}", i.done, i.total);
                return;
            }
            _ if recorder.warmup < WARMUP_FRAMES => {
                recorder.warmup += 1;
                return;
            }
            _ => {
                history.paused = false;
                recorder.started = true;
            }
        }
    }
    recorder.rendered += 1;
    let now = time.elapsed_secs_f64();
    if recorder.ended_at.is_none() && history.finished() {
        recorder.ended_at = Some(now);
    }
    if recorder.stop_at.is_none() && recorder.ended_at.is_some_and(|at| now - at >= HOLD_SECONDS) {
        // Every frame rendered so far, including those still on their way back.
        recorder.stop_at = Some(recorder.rendered);
    }
    if recorder.rendered.is_multiple_of(TITLE_REFRESH_FRAMES) {
        let seconds = recorder.written / recorder.record.fps;
        window.title = format!(
            "Eridu — recording {:.0}% ({}:{:02} of video)",
            history.progress() * 100.0,
            seconds / 60,
            seconds % 60
        );
    }
    let Some(stop) = recorder.stop_at else {
        return;
    };
    recorder.draining += 1;
    if recorder.written < stop && recorder.draining < DRAIN_FRAMES {
        return;
    }
    // Closing the channel ends the writer, which finishes the file; no more readbacks.
    recorder.frames = None;
    readbacks.iter().for_each(|e| commands.entity(e).despawn());
    let result = recorder.writer.take().map(|w| {
        w.join()
            .unwrap_or_else(|_| Err("the writer crashed".into()))
    });
    let written = recorder.written;
    let rendering = recorder.began.elapsed().as_secs_f32();
    match result {
        Some(Ok(output)) => {
            println!(
                "recorded {written} frames ({:.1} s of video) in {rendering:.0} s to {output}",
                written as f32 / recorder.record.fps as f32
            );
            exit.write(AppExit::Success);
        }
        Some(Err(e)) => {
            eprintln!("eridu: recording failed: {e}");
            exit.write(AppExit::from_code(1));
        }
        None => {}
    }
}

/// The writer thread: frames to ffmpeg, or to PNG files when there's no ffmpeg.
fn write_frames(
    record: &Record,
    frames: std::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<String, String> {
    let row = record.width as usize * 4;
    // Rows are padded to the GPU's copy alignment.
    let padded = row.next_multiple_of(256);
    let unpad = |data: Vec<u8>| -> Vec<u8> {
        if padded == row {
            return data;
        }
        data.chunks(padded)
            .flat_map(|r| &r[..row])
            .copied()
            .collect()
    };
    let video = record.path.extension().is_some();
    match if video { ffmpeg(record)? } else { None } {
        Some(mut child) => {
            let mut stdin = child.stdin.take().ok_or("no stdin for ffmpeg")?;
            for frame in frames {
                if stdin.write_all(&unpad(frame)).is_err() {
                    break;
                }
            }
            drop(stdin);
            let status = child.wait().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("ffmpeg exited with {status}"));
            }
            Ok(record.path.display().to_string())
        }
        None => {
            let dir = if video {
                record.path.with_extension("")
            } else {
                record.path.clone()
            };
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            for (i, frame) in frames.into_iter().enumerate() {
                let file = dir.join(format!("frame_{:06}.png", i + 1));
                save_png(&file, record, unpad(frame))?;
            }
            Ok(format!("{} (PNG frames)", dir.display()))
        }
    }
}

/// Starts ffmpeg reading raw RGBA frames on its stdin; `None` (PNGs instead) when there's none.
fn ffmpeg(record: &Record) -> Result<Option<Child>, String> {
    let extension = record
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
    ]);
    command.args([
        "-s",
        &format!("{}x{}", record.width, record.height),
        "-r",
        &record.fps.to_string(),
        "-i",
        "-",
    ]);
    if H264_CONTAINERS.contains(&extension.as_str()) {
        command.args(["-c:v", "libx264", "-preset", H264_PRESET, "-crf", H264_CRF]);
        // HD video is BT.709; say so, or players guess and shift the colors.
        command.args(["-vf", "scale=out_color_matrix=bt709", "-pix_fmt", "yuv420p"]);
        command.args([
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ]);
        command.args(["-movflags", "+faststart"]);
    }
    command.arg(&record.path).stdin(Stdio::piped());
    match command.spawn() {
        Ok(child) => Ok(Some(child)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!("ffmpeg isn't on the PATH: writing PNG frames instead");
            Ok(None)
        }
        Err(e) => Err(format!("cannot start ffmpeg: {e}")),
    }
}

fn save_png(file: &Path, record: &Record, data: Vec<u8>) -> Result<(), String> {
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: record.width,
            height: record.height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD,
    );
    let image = image.try_into_dynamic().map_err(|e| e.to_string())?;
    image
        .save(file)
        .map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// Parses `--size`: `1920x1080`, both even (the video's chroma is subsampled by two).
pub fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let error = || format!("size must look like 1920x1080, not `{text}`");
    let (w, h) = text.trim().split_once(['x', 'X']).ok_or_else(error)?;
    let (w, h): (u32, u32) = (
        w.parse().map_err(|_| error())?,
        h.parse().map_err(|_| error())?,
    );
    if w < 16 || h < 16 || w > 8192 || h > 8192 || w % 2 == 1 || h % 2 == 1 {
        return Err(format!(
            "size must be even and between 16 and 8192 pixels a side, not `{text}`"
        ));
    }
    Ok((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("1920x1080"), Ok((1920, 1080)));
        assert_eq!(parse_size(" 1280X720 "), Ok((1280, 720)));
        assert!(parse_size("1921x1080").is_err());
        assert!(parse_size("1920").is_err());
        assert!(parse_size("ax720").is_err());
    }
}
