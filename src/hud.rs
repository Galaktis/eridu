//! Screen overlays: the date, the status line, the file-type legend (`L`) and the help (`H`).

use bevy::asset::AssetId;
use bevy::prelude::*;

use bevy::input::keyboard::Key;

use crate::interact::Focus;
use crate::record::Recorder;
use crate::ziggurat::View;
use crate::{History, Identities, Palette, Scene, Settings, git, keys, load_image};

/// File types listed in the legend, most numerous first.
const LEGEND_ROWS: usize = 10;
/// The legend is re-sorted this often; counts change too fast to follow every frame.
const LEGEND_REFRESH_SECONDS: f32 = 0.5;
const DIM_TEXT: Color = Color::srgb(0.6, 0.6, 0.6);
const TITLE_TEXT: Color = Color::srgba(1.0, 1.0, 1.0, 0.75);
/// The full Fira Mono Medium, in place of the ASCII-only subset Bevy ships: names have accents
/// (`Jérémy`, `Ágúst`), and so do file paths. SIL Open Font License, see `assets/`.
const FONT: &[u8] = include_bytes!("../assets/FiraMono-Medium.ttf");
/// Largest side of the `--logo`, in pixels.
const LOGO_SIZE: f32 = 96.0;

const HELP: &str = "\
Space         pause / resume
Up / Down     faster / slower
Left / Right  jump back / ahead 5%
Home          back to the start
Seek bar      click or drag to jump
+ / -         zoom in / out (or the mouse wheel)
Left drag     rotate the camera
R             reset the camera
C             camera: cinematic (directs itself when
              left alone) or orbit
B             side branches beside the tree, flowing
              in at their merge
V             view: tree or ziggurat (the history as
              stacked layers, the past on top)
L             file types legend
F             follow the next active author
Click         an author: follow them; a file: focus it
              and show its history; elsewhere: whole tree
N             author names
Q             quality: low, high, ultra
F3            performance stats
F11           fullscreen
H             this help
Esc           close the help / stop following / quit";

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        // Replaces the default font, which every text uses.
        let font = Font::from_bytes(FONT.to_vec());
        app.world_mut()
            .resource_mut::<Assets<Font>>()
            .insert(AssetId::default(), font)
            .expect("the default font id is valid");
        app.add_systems(Startup, spawn)
            .add_systems(Update, toggles)
            .add_systems(
                Update,
                (update_text, update_legend).run_if(resource_exists::<History>),
            );
    }
}

#[derive(Component)]
enum Hud {
    Date,
    Status,
}

#[derive(Component)]
struct Legend;

/// One legend row: a color dot and a label.
#[derive(Component)]
struct LegendRow {
    dot: Entity,
    label: Entity,
}

#[derive(Component)]
struct Help;

fn spawn(mut commands: Commands, settings: Res<Settings>, mut images: ResMut<Assets<Image>>) {
    if !settings.hide.title {
        commands
            .spawn(Node {
                position_type: PositionType::Absolute,
                top: px(18),
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..default()
            })
            .with_child((
                Text::new(settings.title.clone()),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(TITLE_TEXT),
            ));
    }
    if let Some(path) = &settings.logo {
        match load_image(path) {
            Ok(image) => {
                commands.spawn((
                    ImageNode::new(images.add(image)),
                    Node {
                        position_type: PositionType::Absolute,
                        right: px(16),
                        bottom: px(84),
                        max_width: px(LOGO_SIZE),
                        max_height: px(LOGO_SIZE),
                        ..default()
                    },
                ));
            }
            Err(e) => warn!("logo: {e}"),
        }
    }
    commands.spawn((
        Text::new("Reading the history…"),
        TextFont {
            font_size: FontSize::Px(28.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            ..default()
        },
        Hud::Date,
    ));
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(DIM_TEXT),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(16),
            left: px(16),
            ..default()
        },
        Hud::Status,
    ));

    let legend = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(64),
                left: px(16),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            },
            if settings.hide.legend {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
            Legend,
        ))
        .id();
    for _ in 0..LEGEND_ROWS {
        let dot = commands
            .spawn((
                Node {
                    width: px(10),
                    height: px(10),
                    border_radius: BorderRadius::MAX,
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .id();
        let label = commands
            .spawn((
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(DIM_TEXT),
            ))
            .id();
        let row = commands
            .spawn((
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    display: Display::None,
                    ..default()
                },
                LegendRow { dot, label },
            ))
            .add_children(&[dot, label])
            .id();
        commands.entity(legend).add_child(row);
    }

    // Centered by a full-screen container.
    let panel = commands
        .spawn((
            Node {
                padding: UiRect::all(px(24)),
                border_radius: BorderRadius::all(px(8)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.8)),
        ))
        .with_child((
            Text::new(HELP),
            TextFont {
                font_size: FontSize::Px(16.0),
                ..default()
            },
        ))
        .id();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Visibility::Hidden,
            Help,
        ))
        .add_child(panel);
}

fn flip(visibility: &mut Visibility) {
    *visibility = match *visibility {
        Visibility::Hidden => Visibility::Inherited,
        _ => Visibility::Hidden,
    };
}

fn toggles(
    keys: Res<ButtonInput<KeyCode>>,
    typed: Res<ButtonInput<Key>>,
    mut help: Single<&mut Visibility, (With<Help>, Without<Legend>)>,
    mut legend: Single<&mut Visibility, (With<Legend>, Without<Help>)>,
    mut focus: ResMut<Focus>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys::typed(&typed, "h") {
        flip(&mut help);
    }
    if keys::typed(&typed, "l") {
        flip(&mut legend);
    }
    // Esc steps back: closes the help, else stops following or focusing, else quits.
    if keys.just_pressed(KeyCode::Escape) {
        if **help != Visibility::Hidden {
            **help = Visibility::Hidden;
        } else if *focus != Focus::Tree {
            *focus = Focus::Tree;
        } else {
            exit.write(AppExit::Success);
        }
    }
}

fn update_text(
    history: Res<History>,
    settings: Res<Settings>,
    mode: Res<View>,
    recording: Option<Res<Recorder>>,
    identities: Option<Res<Identities>>,
    mut texts: Query<(&mut Text, &Hud)>,
) {
    // While GitHub is asked about authors (avatars fill in as answers come).
    let asking = match identities.as_deref() {
        Some(i) if !i.finished && i.total > 0 => format!("   avatars {}/{}", i.done, i.total),
        _ => String::new(),
    };
    for (mut text, hud) in &mut texts {
        let wanted = match hud {
            Hud::Date if settings.hide.date => String::new(),
            Hud::Date => git::date(history.time as i64),
            Hud::Status if settings.hide.hud => String::new(),
            Hud::Status => format!(
                "{}/{} commits   {} s/day{}{}{}{asking}{}",
                history.next,
                history.commits.len(),
                history.seconds_per_day,
                if *mode == View::Ziggurat {
                    "   ziggurat"
                } else {
                    ""
                },
                if history.reading.branches {
                    "   branches"
                } else {
                    ""
                },
                if history.paused { "  (paused)" } else { "" },
                // A video has no keys to press.
                if recording.is_some() {
                    ""
                } else {
                    "   [H] help"
                },
            ),
        };
        // Only touch the text when it changes: a changed text is laid out again.
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_legend(
    time: Res<Time>,
    palette: Res<Palette>,
    scene: Res<Scene>,
    mut since: Local<f32>,
    mut order: Local<Vec<usize>>,
    rows: Query<(&LegendRow, &mut Node)>,
    mut dots: Query<&mut BackgroundColor>,
    mut labels: Query<&mut Text>,
) {
    *since += time.delta_secs();
    if *since < LEGEND_REFRESH_SECONDS {
        return;
    }
    *since = 0.0;
    order.clear();
    order.extend((0..scene.counts.len()).filter(|&k| scene.counts[k] > 0));
    order.sort_unstable_by_key(|&k| std::cmp::Reverse(scene.counts[k]));
    for (i, (row, mut node)) in rows.into_iter().enumerate() {
        let Some(&kind) = order.get(i) else {
            node.display = Display::None;
            continue;
        };
        node.display = Display::Flex;
        if let Ok(mut dot) = dots.get_mut(row.dot) {
            dot.set_if_neq(BackgroundColor(palette.colors[kind].into()));
        }
        if let Ok(mut label) = labels.get_mut(row.label) {
            let wanted = format!("{}  {}", palette.names[kind], scene.counts[kind]);
            if label.0 != wanted {
                label.0 = wanted;
            }
        }
    }
}
