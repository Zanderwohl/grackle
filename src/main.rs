//! The `editor` binary: plugin wiring only.
//!
//! Everything it wires up comes from the `grackle` library rather than from a
//! second `mod` tree of its own. That is load-bearing rather than tidiness: a
//! client and a server that must reach the same position from the same inputs
//! cannot be two compiled copies of the simulation, each with its own `LANG`
//! table and its own caches.

use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use bevy_egui::{EguiPlugin};
use bevy_vector_shapes::prelude::*;
use grackle::{get, startup};
use grackle::common::app_mode::StartInPlay;
use grackle::common::lang::{change_lang_or_fallback, default_packs};
use grackle::common::net::NetPlugin;
use grackle::common::net_transport::NetTransportPlugin;
use grackle::common::perf::PerfPlugin;
use grackle::editor::editable::EditorStepsPlugin;
use grackle::editor::input::EditorInputPlugin;
use grackle::editor::multicam::MulticamPlugin;
use grackle::editor::net_menu::NetMenuPlugin;
use grackle::editor::panels::EditorPanelPlugin;
use grackle::game::net_bodies::NetBodiesPlugin;
use grackle::game::skeleton::SkeletonPlugin;
use grackle::game::GamePlugin;
use grackle::tool::ToolPlugin;


fn main() {
    let editor_params = startup::EditorParams::new()
        .unwrap_or_else(|message| {
            eprintln!("Editor Startup Error:\n{}", message);
            std::process::exit(1);
        });
    // Falls back to en-US rather than exiting: a bad `--lang` should not stop
    // the editor from opening.
    change_lang_or_fallback(&editor_params.lang, &default_packs());

    App::new()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
               primary_window: Some(Window {
                   title: get!("editor.title"),
                   name: Some("grackle.app".to_owned()),
                   present_mode: PresentMode::AutoVsync,
                   prevent_default_event_handling: true,
                   visible: true,
                   ..default()
               }),
                primary_cursor_options: None,
                exit_condition: ExitCondition::OnPrimaryClosed,
                close_when_requested: true,
            }),
        )
        .insert_resource(editor_params.net.clone())
        .insert_resource(StartInPlay(editor_params.start_playing))
        .add_plugins((
            EguiPlugin::default(),
            Shape2dPlugin::default(),
        ))
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 100.0,
            ..default()
        })
        .add_plugins((
            EditorInputPlugin,
            NetPlugin,
            NetTransportPlugin,
            NetBodiesPlugin,
            NetMenuPlugin,
            MulticamPlugin {
                test_scene: false,
            },
            EditorPanelPlugin,
            EditorStepsPlugin,
            ToolPlugin,
            GamePlugin,
            SkeletonPlugin,
            PerfPlugin,
            ))
        .run();
}
