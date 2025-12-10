mod common;
mod startup;
mod editor;
mod tool;

use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use bevy_egui::{EguiPlugin};
use bevy_vector_shapes::prelude::*;
use crate::common::lang::change_lang;
use crate::common::perf::PerfPlugin;
use crate::editor::editable::EditorStepsPlugin;
use crate::editor::input::EditorInputPlugin;
use crate::editor::multicam::MulticamPlugin;
use crate::editor::panels::EditorPanelPlugin;
use crate::tool::ToolPlugin;


fn main() {
    let editor_params = startup::EditorParams::new()
        .unwrap_or_else(|message| {
            eprintln!("Editor Startup Error:\n{}", message);
            std::process::exit(1);
        });
    change_lang(&editor_params.lang)
        .unwrap_or_else(|message| {
            eprintln!("Language map error:\n{}", message);
            std::process::exit(1);
        });

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
        .add_plugins((
            EguiPlugin::default(),
            Shape2dPlugin::default(),
        ))
        .add_plugins((
            EditorInputPlugin,
            MulticamPlugin {
                test_scene: true,
            },
            EditorPanelPlugin,
            EditorStepsPlugin,
            ToolPlugin,
            PerfPlugin,
            ))
        .run();
}
