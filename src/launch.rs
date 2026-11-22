//! What both binaries wire together. `editor` and `grackle` are one app, starting in different
//! modes under different window titles; each `main` is a call to [`run`].

use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use bevy_egui::EguiPlugin;
use bevy_vector_shapes::prelude::*;
use crate::{get, startup};
use crate::common::round_settings::{BotsAtStartup, RoundSettingsFilePlugin, ServerConfig, ServerFiles};
use crate::editor::editable::StartupMap;
use crate::game::rounds::QuitAfterRounds;
use crate::game::spectate::StartSpectating;
use crate::common::fonts::FontsPlugin;
use crate::common::ui_shape::UiShapePlugin;
use crate::common::lang::{change_lang_or_fallback, default_packs, FALLBACK_LANG};
use crate::common::keybind::Keymap;
use crate::common::settings::{Settings, SettingsPlugin};
use crate::common::net::NetPlugin;
use crate::common::net_transport::NetTransportPlugin;
use crate::common::perf::PerfPlugin;
use crate::editor::editable::EditorStepsPlugin;
use crate::editor::input::EditorInputPlugin;
use crate::editor::multicam::MulticamPlugin;
use crate::editor::net_menu::NetMenuPlugin;
use crate::editor::panels::EditorPanelPlugin;
use crate::game::net_bodies::NetBodiesPlugin;
use crate::game::skeleton::SkeletonPlugin;
use crate::game::GamePlugin;
use crate::game::sunlit::SunlitPlugin;
use crate::game::world_prop::WorldPropPlugin;
use crate::game::forward_camera::ForwardCameraPlugin;
use crate::game::decal::DecalPlugin;
use crate::game::spray::SprayPlugin;
use crate::anim::{AnimPlugin, StartInAnimEditor};
use crate::material::{MaterialEditorPlugin, StartInMaterialEditor};
use crate::lang_editor::{LangEditorPlugin, StartInLangEditor};
use crate::prop::thumbnail::ThumbnailPlugin;
use crate::prop::{PropEditorPlugin, StartInPropEditor};
use crate::tool::ToolPlugin;
use crate::common::app_mode::{AppMode, StartInPlay};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binary {
    Game,
    Editor,
}

impl Binary {
    fn title(self) -> String {
        match self {
            Binary::Game => get!("main_menu.window_title"),
            Binary::Editor => get!("editor.title"),
        }
    }

    /// Where a launch that names nothing to open or play starts, after onboarding.
    pub fn home(self) -> AppMode {
        match self {
            Binary::Game => AppMode::MainMenu,
            Binary::Editor => AppMode::Editor,
        }
    }
}

pub fn run(binary: Binary) {
    let editor_params = startup::EditorParams::new()
        .unwrap_or_else(|message| {
            eprintln!("Editor Startup Error:\n{}", message);
            std::process::exit(1);
        });
    let (settings, settings_problem) = Settings::load();
    let server_files = ServerFiles::new(editor_params.server_config.clone(), editor_params.server_profile.clone(), &default_packs());
    let (server, server_problems) = ServerConfig::resolve(&server_files);
    // Falls back to en-US rather than exiting: a bad `--lang` should not stop
    // the editor from opening.
    let lang = editor_params.lang.clone().or_else(|| settings.language.clone());
    change_lang_or_fallback(lang.as_deref().unwrap_or(FALLBACK_LANG), &default_packs());

    App::new()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
               primary_window: Some(Window {
                   title: binary.title(),
                   name: Some("grackle.app".to_owned()),
                   present_mode: PresentMode::AutoVsync,
                   prevent_default_event_handling: true,
                   visible: true,
                   ..default()
               }),
                primary_cursor_options: None,
                exit_condition: ExitCondition::OnPrimaryClosed,
                close_when_requested: true,
            })
            .set(bevy::log::LogPlugin { custom_layer: crate::common::log_file::layer, ..default() })
            // From the working directory, as the packs are read: Bevy's own default is
            // `CARGO_MANIFEST_DIR`, unset outside `cargo run`, else the executable's folder.
            .set(bevy::asset::AssetPlugin {
                file_path: std::env::current_dir()
                    .map(|dir| dir.join(crate::common::assets::ASSET_ROOT).to_string_lossy().into_owned())
                    .unwrap_or_else(|_| crate::common::assets::ASSET_ROOT.to_owned()),
                ..default()
            }),
        )
        .insert_resource(settings)
        .insert_resource(crate::common::recent_files::RecentFiles::load())
        .insert_resource(Keymap::load())
        .insert_resource(editor_params.net.clone())
        .insert_resource(StartInPlay(editor_params.start_playing))
        .insert_resource(StartupMap(editor_params.map.clone()))
        .insert_resource(StartSpectating(editor_params.start_spectating))
        .insert_resource(QuitAfterRounds(editor_params.rounds))
        .insert_resource(crate::common::game_rules::PlayAs(editor_params.class))
        .init_resource::<crate::game::telemetry::RecordRounds>()
        .insert_resource(StartInPropEditor(editor_params.start_modeling))
        .insert_resource(StartInAnimEditor(editor_params.start_animating))
        .insert_resource(StartInMaterialEditor(editor_params.start_texturing))
        .insert_resource(crate::weapon_editor::StartInWeaponEditor(editor_params.start_arming))
        .insert_resource(StartInLangEditor(editor_params.start_translating))
        .insert_resource(server.round())
        .insert_resource(server.listing())
        .insert_resource(BotsAtStartup(server.bots))
        .add_plugins((
            EguiPlugin::default(),
            Shape2dPlugin::default(),
        ))
        // Lightmapped walls carry their own ambient, so this lights only what moves.
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: crate::common::lightmap::BASE_AMBIENT,
            affects_lightmapped_meshes: false,
            ..default()
        })
        .add_plugins((
            EditorInputPlugin,
            NetPlugin,
            NetTransportPlugin,
            (NetBodiesPlugin, crate::game::bot::BotPlugin, crate::game::bot_sense::BotSensePlugin, crate::game::bot_think::BotThinkPlugin, crate::game::bot_fight::BotFightPlugin, crate::game::pull_table::PullTablePlugin),
            NetMenuPlugin,
            MulticamPlugin {
                test_scene: false,
            },
            (EditorPanelPlugin, crate::tool::round_prep::RoundPrepPlugin, crate::editor::kill_history::KillHistoryPlugin),
            EditorStepsPlugin,
            ToolPlugin,
            PropEditorPlugin,
            (AnimPlugin, MaterialEditorPlugin, crate::weapon_editor::WeaponEditorPlugin, LangEditorPlugin),
            (GamePlugin, crate::game::match_cycle::MatchCyclePlugin, crate::game::match_banner::MatchBannerPlugin, crate::game::soundtrack::SoundtrackPlugin),
            (WorldPropPlugin, (ThumbnailPlugin, crate::tool::postcard_bake::PostcardBakePlugin, crate::game::postcards::PostcardHudPlugin), ForwardCameraPlugin, SunlitPlugin, DecalPlugin, SprayPlugin, (crate::game::streak::StreakPlugin, crate::game::scope_vignette::ScopeVignettePlugin, crate::game::laser_dot::LaserDotPlugin, crate::game::muzzle_flash::MuzzleFlashPlugin), crate::game::beam::BeamPlugin, crate::game::class_picker::PortraitWarmUpPlugin, crate::game::gib_blood::GibBloodPlugin, crate::game::flame_jet::FlameJetPlugin, crate::game::hud_fire::HudFirePlugin, crate::game::gust_ring::GustRingPlugin, crate::game::blast_cloud::BlastCloudPlugin, crate::game::rocket_trail::RocketTrailPlugin),
            SkeletonPlugin,
            (PerfPlugin, crate::common::screenshot::ScreenshotPlugin, SettingsPlugin, FontsPlugin, crate::common::music::MusicPlugin, crate::common::icons::IconsPlugin, UiShapePlugin, crate::common::player_settings::PlayerSettingsPlugin { profile: editor_params.profile }),
            ))
        .add_plugins(RoundSettingsFilePlugin { path: server_files.saved.clone(), seed: server_files.shipped.clone() })
        .add_plugins((crate::main_menu::MainMenuPlugin, crate::common::lan_discovery::LanDiscoveryPlugin, crate::server_browser::ServerBrowserPlugin))
        .add_plugins(crate::onboarding::OnboardingPlugin { skip: editor_params.chooses_where_to_start(), home: binary.home() })
        .add_systems(Startup, move || {
            // Read before the log existed, so said now.
            if let Some(problem) = &settings_problem {
                warn!("{problem}");
            }
            for problem in &server_problems {
                warn!("{problem}");
            }
        })
        .run();
}
