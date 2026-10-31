//! The `editor` binary: plugin wiring only. Everything comes from the
//! `grackle` library; a second `mod` tree would be a second copy of the
//! simulation, with its own `LANG` table and caches.

use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use bevy_egui::{EguiPlugin};
use bevy_vector_shapes::prelude::*;
use grackle::{get, startup};
use grackle::common::app_mode::StartInPlay;
use grackle::common::round_settings::{BotsAtStartup, ServerConfig};
use grackle::editor::editable::StartupMap;
use grackle::game::rounds::QuitAfterRounds;
use grackle::game::spectate::StartSpectating;
use grackle::common::fonts::FontsPlugin;
use grackle::common::ui_shape::UiShapePlugin;
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
use grackle::game::world_prop::WorldPropPlugin;
use grackle::game::forward_camera::ForwardCameraPlugin;
use grackle::anim::{AnimPlugin, StartInAnimEditor};
use grackle::material::{MaterialEditorPlugin, StartInMaterialEditor};
use grackle::prop::thumbnail::ThumbnailPlugin;
use grackle::prop::{PropEditorPlugin, StartInPropEditor};
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
        .insert_resource(StartupMap(editor_params.map.clone()))
        .insert_resource(StartSpectating(editor_params.start_spectating))
        .insert_resource(QuitAfterRounds(editor_params.rounds))
        .insert_resource(StartInPropEditor(editor_params.start_modeling))
        .insert_resource(StartInAnimEditor(editor_params.start_animating))
        .insert_resource(StartInMaterialEditor(editor_params.start_texturing))
        .insert_resource(editor_params.server.as_ref().map(ServerConfig::round).unwrap_or_default())
        .insert_resource(BotsAtStartup(editor_params.server.as_ref().map_or(0, |server| server.bots)))
        .add_plugins((
            EguiPlugin::default(),
            Shape2dPlugin::default(),
        ))
        // Lightmapped walls carry their own ambient, so this lights only what moves.
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: grackle::common::lightmap::BASE_AMBIENT,
            affects_lightmapped_meshes: false,
            ..default()
        })
        .add_plugins((
            EditorInputPlugin,
            NetPlugin,
            NetTransportPlugin,
            (NetBodiesPlugin, grackle::game::bot::BotPlugin, grackle::game::bot_sense::BotSensePlugin, grackle::game::bot_think::BotThinkPlugin, grackle::game::bot_fight::BotFightPlugin, grackle::game::pull_table::PullTablePlugin),
            NetMenuPlugin,
            MulticamPlugin {
                test_scene: false,
            },
            (EditorPanelPlugin, grackle::tool::round_prep::RoundPrepPlugin),
            EditorStepsPlugin,
            ToolPlugin,
            PropEditorPlugin,
            (AnimPlugin, MaterialEditorPlugin),
            GamePlugin,
            (WorldPropPlugin, ThumbnailPlugin, ForwardCameraPlugin),
            SkeletonPlugin,
            (PerfPlugin, FontsPlugin, grackle::common::icons::IconsPlugin, UiShapePlugin, grackle::common::player_settings::PlayerSettingsPlugin { profile: editor_params.profile }),
            ))
        .run();
}
