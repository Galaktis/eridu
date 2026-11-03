//! The window's icon (title bar, taskbar, Alt-Tab). The executable's own icon, shown in the
//! file explorer, is embedded by `build.rs`. Both come from `tools/icon.py`.

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::NonSendMarker;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::winit::WINIT_WINDOWS;
use winit::window::Icon;

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// Sets the icon once the window exists. Winit windows live on the main thread, hence the marker.
pub fn set_window_icon(_main_thread: NonSendMarker, mut done: Local<bool>) {
    if *done {
        return;
    }
    WINIT_WINDOWS.with_borrow(|winit| {
        if winit.windows.is_empty() {
            return;
        }
        *done = true;
        match icon() {
            Ok(icon) => winit
                .windows
                .values()
                .for_each(|window| window.set_window_icon(Some(icon.clone()))),
            Err(e) => warn!("window icon: {e}"),
        }
    });
}

fn icon() -> Result<Icon, String> {
    let png = ImageType::Extension("png");
    let image = Image::from_buffer(
        ICON_PNG,
        png,
        CompressedImageFormats::NONE,
        true,
        ImageSampler::default(),
        RenderAssetUsages::default(),
    )
    .map_err(|e| e.to_string())?;
    let size = image.size();
    let rgba = image.data.ok_or("no pixels")?;
    Icon::from_rgba(rgba, size.x, size.y).map_err(|e| e.to_string())
}
