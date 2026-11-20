//! The `grackle` binary's first screen ([`AppMode::MainMenu`]).
//!
//! Its nodes target the egui context camera, the one camera live outside the viewport modes (see
//! [`crate::anim::timeline`]). egui draws over Bevy UI but does not stop Bevy's picking, so while the
//! connect dialog, the Settings page or the Quit question is up a [`Blocker`] covers the menu and
//! takes the pointer.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::fonts::DisplayFont;
use crate::common::keybind::{self, Action, Actions};
use crate::common::settings::{OpenSettings, SettingsWindow};
use crate::common::swatch::Swatch;
use crate::editor::net_menu::{ConnectDialog, ConnectMode};
use crate::server_browser::ServerBrowser;
use crate::game::menu::{menu_button, menu_panel, menu_title, panel_swatch, ITEM_SIZE};
use crate::game::hud::{LEAN, WINDOW_MARGIN, ZONE_DROP, ZONE_STROKE};
use crate::game::kill_icon;
use crate::game::plate::{plate_row_node, plate_text, PlateRow, Plate, PlateIcon};
use crate::game::plate_button::{plate_button_node, ButtonActivated, PlateButton};
use crate::get;

const BACKGROUND: Color = Color::srgb(0.07, 0.07, 0.08);
const BLOCKER_SHADE: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);
const MENU_LAYER: i32 = 10;
const BLOCKER_LAYER: i32 = 11;
const QUIT_LAYER: i32 = 12;
const DOCK_ICON: f32 = 32.0;
/// Logical pixels inside a dock button's stroke, past the drop on the far sides.
const DOCK_PADDING: f32 = 8.0;
const DOCK_GAP: f32 = 12.0;
/// Logical pixels inside the dock plate's stroke, past the drop on the far sides.
const DOCK_MARGIN: f32 = 10.0;

pub struct MainMenuPlugin;

impl Plugin for MainMenuPlugin {
    fn build(&self, app: &mut App) {
        keybind::ensure(app);
        app.init_resource::<QuitQuestion>()
            .init_resource::<DisplayFont>()
            .init_resource::<ConnectDialog>()
            .init_resource::<ServerBrowser>()
            .add_message::<AppExit>()
            .add_observer(choose)
            .add_systems(
                Update,
                (
                    dismiss_the_question.run_if(in_state(AppMode::MainMenu)),
                    show_or_hide,
                    show_what_is_over_it,
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::MainMenu), stop_asking);
    }
}

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct QuitQuestion {
    pub open: bool,
}

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component)]
struct Blocker;

#[derive(Component)]
struct Dock;

#[derive(Component)]
struct QuitRoot;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    BrowseServers,
    DirectJoin,
    Tools,
    Settings,
    Quit,
}

impl MenuItem {
    /// Each cluster is a joined row, left to right, with each button's icon.
    const DOCK: [&'static [(MenuItem, &'static str)]; 2] =
        [&[(MenuItem::Settings, "gear"), (MenuItem::Tools, "hammer")], &[(MenuItem::Quit, "power")]];

    fn dock_swatch(self) -> Swatch {
        match self {
            MenuItem::Quit => Swatch::danger(),
            _ => Swatch::neutral(),
        }
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum QuitItem {
    Quit,
    Back,
}

/// Not `OnEnter`: the egui camera is spawned in `Startup`, after the initial state is entered, and a
/// node with no camera draws nowhere.
fn show_or_hide(
    mut commands: Commands,
    mode: Res<State<AppMode>>,
    existing: Query<(Entity, Has<UiTargetCamera>), With<MainMenuRoot>>,
    camera: Query<Entity, With<bevy_egui::PrimaryEguiContext>>,
    font: Res<DisplayFont>,
    pack: Option<Res<crate::common::assets::Assets>>,
    images: Option<ResMut<Assets<Image>>>,
    mut drawn: Local<HashMap<&'static str, Option<Handle<Image>>>>,
) {
    let wanted = *mode.get() == AppMode::MainMenu;
    let camera = camera.single().ok();
    if wanted && existing.is_empty() {
        let icons = match (pack, images) {
            (Some(pack), Some(mut images)) => MenuItem::DOCK.map(|cluster| {
                let mut icon = |name: &'static str| {
                    drawn.entry(name).or_insert_with(|| kill_icon::load(name, |relative| pack.read_from_packs(relative), &mut images)).clone()
                };
                cluster.iter().map(|&(_, name)| icon(name)).collect()
            }),
            _ => MenuItem::DOCK.map(|cluster| vec![None; cluster.len()]),
        };
        spawn_the_menu(&mut commands, camera, &font.0, icons);
        return;
    }
    for (root, targeted) in &existing {
        match (wanted, targeted, camera) {
            (false, _, _) => commands.entity(root).despawn(),
            (true, false, Some(camera)) => {
                commands.entity(root).insert(UiTargetCamera(camera));
            }
            _ => {}
        }
    }
}

/// `icons` per [`MenuItem::DOCK`] cluster and button.
fn spawn_the_menu(commands: &mut Commands, camera: Option<Entity>, font: &Handle<Font>, icons: [Vec<Option<Handle<Image>>>; 2]) {
    let full_screen = Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::Center,
        ..default()
    };
    let mut root = commands.spawn((MainMenuRoot, GlobalZIndex(MENU_LAYER), full_screen.clone(), BackgroundColor(BACKGROUND)));
    if let Some(camera) = camera {
        root.insert(UiTargetCamera(camera));
    }
    root.with_children(|root| {
        root.spawn(menu_panel()).with_children(|menu| {
            menu.spawn(menu_title(font, get!("main_menu.title")));
            menu.spawn((MenuItem::BrowseServers, menu_button(Swatch::neutral())))
                .with_child(plate_text(font, get!("main_menu.browse_servers"), ITEM_SIZE));
            menu.spawn((MenuItem::DirectJoin, menu_button(Swatch::neutral())))
                .with_child(plate_text(font, get!("main_menu.direct_join"), ITEM_SIZE));
        });
        root.spawn((
            Dock,
            Plate::new(panel_swatch()),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(WINDOW_MARGIN),
                bottom: Val::Px(WINDOW_MARGIN),
                column_gap: Val::Px(DOCK_GAP),
                padding: dock_padding(),
                ..default()
            },
        ))
        .with_children(|dock| {
            for (cluster, icons) in MenuItem::DOCK.into_iter().zip(icons) {
                dock.spawn((PlateRow, plate_row_node())).with_children(|row| {
                    for (&(item, _), icon) in cluster.iter().zip(icons) {
                        let swatch = item.dock_swatch();
                        row.spawn((item, PlateButton::new(swatch), Plate::new(swatch), Pickable::default(), dock_button_node())).with_children(|button| {
                            // Without an image, the node holds the icon's place.
                            let mut slot = button.spawn((PlateIcon, Pickable::IGNORE, Node { width: Val::Px(DOCK_ICON), height: Val::Px(DOCK_ICON), ..default() }));
                            if let Some(icon) = icon {
                                slot.insert(ImageNode::new(icon));
                            }
                        });
                    }
                });
            }
        });
        // Picked before the menu, so a click on a dialog above it reaches no button.
        root.spawn((Blocker, GlobalZIndex(BLOCKER_LAYER), Pickable::default(), Node { display: Display::None, ..full_screen.clone() }, BackgroundColor(BLOCKER_SHADE)));
        root.spawn((QuitRoot, GlobalZIndex(QUIT_LAYER), Node { display: Display::None, ..full_screen }))
            .with_children(|quit| {
                quit.spawn(menu_panel()).with_children(|panel| {
                    panel.spawn(menu_title(font, get!("main_menu.quit_modal.title")));
                    for (item, label) in [
                        (QuitItem::Quit, get!("main_menu.quit_modal.quit")),
                        (QuitItem::Back, get!("main_menu.quit_modal.back")),
                    ] {
                        panel.spawn((item, menu_button(Swatch::neutral()))).with_child(plate_text(font, label, ITEM_SIZE));
                    }
                });
            });
    });
}

#[allow(clippy::too_many_arguments)]
fn choose(
    activated: On<ButtonActivated>,
    items: Query<&MenuItem>,
    quit_items: Query<&QuitItem>,
    mut commands: Commands,
    mut question: ResMut<QuitQuestion>,
    mut dialog: ResMut<ConnectDialog>,
    mut browser: ResMut<ServerBrowser>,
    mut next: ResMut<NextState<AppMode>>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Ok(item) = items.get(activated.entity) {
        match item {
            MenuItem::BrowseServers => browser.open = true,
            MenuItem::DirectJoin => dialog.show(ConnectMode::Join),
            MenuItem::Tools => next.set(AppMode::Editor),
            MenuItem::Settings => commands.insert_resource(OpenSettings),
            MenuItem::Quit => question.open = true,
        }
    }
    if let Ok(item) = quit_items.get(activated.entity) {
        match item {
            QuitItem::Quit => {
                exit.write(AppExit::Success);
            }
            QuitItem::Back => question.open = false,
        }
    }
}

fn dismiss_the_question(actions: Res<Actions>, mut question: ResMut<QuitQuestion>) {
    if question.open && actions.just_pressed(Action::Cancel) {
        question.open = false;
    }
}

/// The plate slants over a taller box than its buttons, so the left side gains the slant across the
/// bottom padding and the right the slant across the top.
fn dock_padding() -> UiRect {
    let top = ZONE_STROKE + DOCK_MARGIN;
    let bottom = ZONE_STROKE + ZONE_DROP.y + DOCK_MARGIN;
    UiRect {
        left: Val::Px(ZONE_STROKE + DOCK_MARGIN + LEAN * bottom),
        right: Val::Px(ZONE_STROKE + ZONE_DROP.x + DOCK_MARGIN + LEAN * top),
        top: Val::Px(top),
        bottom: Val::Px(bottom),
    }
}

fn dock_button_node() -> Node {
    Node {
        padding: UiRect {
            left: Val::Px(ZONE_STROKE + DOCK_PADDING),
            right: Val::Px(ZONE_STROKE + ZONE_DROP.x + DOCK_PADDING),
            top: Val::Px(ZONE_STROKE + DOCK_PADDING),
            bottom: Val::Px(ZONE_STROKE + ZONE_DROP.y + DOCK_PADDING),
        },
        ..plate_button_node()
    }
}

/// The blocker is up under the Quit question too.
fn show_what_is_over_it(
    question: Res<QuitQuestion>,
    dialog: Res<ConnectDialog>,
    browser: Res<ServerBrowser>,
    settings: Option<Res<SettingsWindow>>,
    asked: Option<Res<OpenSettings>>,
    mut nodes: Query<(&mut Node, Has<Blocker>), Or<(With<Blocker>, With<QuitRoot>)>>,
) {
    let covered = question.open || dialog.open || browser.open || asked.is_some() || settings.is_some_and(|window| window.is_open());
    for (mut node, blocker) in &mut nodes {
        let display = match if blocker { covered } else { question.open } {
            true => Display::Flex,
            false => Display::None,
        };
        if node.display != display {
            node.display = display;
        }
    }
}

/// Left open, the question would be up again the next time the menu is.
fn stop_asking(mut question: ResMut<QuitQuestion>) {
    question.open = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(start: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::state::app::StatesPlugin, bevy::input::InputPlugin));
        app.insert_state(start);
        app.add_plugins(MainMenuPlugin);
        app.update();
        app
    }

    fn showing<C: Component>(app: &mut App) -> bool {
        app.world_mut().query_filtered::<Entity, With<C>>().iter(app.world()).next().is_some()
    }

    fn displayed<C: Component>(app: &mut App) -> bool {
        app.world_mut()
            .query_filtered::<&Node, With<C>>()
            .iter(app.world())
            .any(|node| node.display != Display::None)
    }

    fn activate<C: Component + PartialEq>(app: &mut App, wanted: C) {
        let entity = app
            .world_mut()
            .query::<(Entity, &C)>()
            .iter(app.world())
            .find(|(_, item)| **item == wanted)
            .map(|(entity, _)| entity)
            .expect("the menu does not offer it");
        app.world_mut().trigger(ButtonActivated { entity });
        app.world_mut().flush();
    }

    fn mode(app: &App) -> AppMode {
        *app.world().resource::<State<AppMode>>().get()
    }

    fn labels(app: &mut App) -> Vec<String> {
        app.world_mut().query::<&Text>().iter(app.world()).map(|text| text.0.clone()).collect()
    }

    /// Fails if a menu spawned before the egui camera existed is never drawn through it.
    #[test]
    fn a_menu_spawned_before_the_camera_is_given_it() {
        let mut app = app(AppMode::MainMenu);
        let camera = app.world_mut().spawn(bevy_egui::PrimaryEguiContext).id();
        app.update();
        let targets: Vec<Entity> = app.world_mut().query_filtered::<&UiTargetCamera, With<MainMenuRoot>>().iter(app.world()).map(|target| target.0).collect();
        assert_eq!(targets, vec![camera]);
    }

    /// Fails if the menu is missing in its own mode, outlives it, or leaves out a row.
    #[test]
    fn the_menu_is_up_only_in_its_own_mode() {
        let mut app = app(AppMode::MainMenu);
        assert!(showing::<MainMenuRoot>(&mut app));
        let shown = labels(&mut app);
        for key in ["browse_servers", "direct_join"] {
            let label = get!(format!("main_menu.{key}"));
            assert!(shown.contains(&label), "{label} is missing from {shown:?}");
        }
        assert!(!displayed::<Blocker>(&mut app) && !displayed::<QuitRoot>(&mut app));

        app.world_mut().resource_mut::<NextState<AppMode>>().set(AppMode::Editor);
        app.update();
        app.update();
        assert!(!showing::<MainMenuRoot>(&mut app), "the menu outlived its mode");
        assert!(!showing::<MainMenuRoot>(&mut self::app(AppMode::Editor)));
    }

    /// Fails if Browse Servers does not open the browser, or leaves the menu clickable under it.
    #[test]
    fn browse_servers_opens_the_browser_over_a_blocked_menu() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::BrowseServers);
        app.update();
        assert!(app.world().resource::<ServerBrowser>().open);
        assert!(displayed::<Blocker>(&mut app), "the menu took clicks under the browser");
    }

    /// Fails if a dock button loses its icon or gains text, or Quit is joined to the others.
    #[test]
    fn settings_tools_and_quit_are_icons_in_the_dock() {
        let mut app = app(AppMode::MainMenu);
        let row_of = |app: &mut App, wanted: MenuItem| {
            let button = app.world_mut().query::<(Entity, &MenuItem)>().iter(app.world()).find(|(_, found)| **found == wanted).unwrap().0;
            let row = app.world().get::<ChildOf>(button).unwrap().parent();
            assert!(app.world().get::<PlateRow>(row).is_some(), "{wanted:?} is not in a joined row");
            row
        };
        let joined = row_of(&mut app, MenuItem::Settings);
        let dock = app.world().get::<ChildOf>(joined).unwrap().parent();
        assert!(app.world().get::<Dock>(dock).is_some() && app.world().get::<Plate>(dock).is_some(), "the dock has no plate behind it");
        assert_eq!(row_of(&mut app, MenuItem::Tools), joined);
        assert_ne!(row_of(&mut app, MenuItem::Quit), joined, "Quit is joined to Settings and Tools");
        for &(item, name) in MenuItem::DOCK.iter().copied().flatten() {
            let button = app
                .world_mut()
                .query::<(Entity, &MenuItem)>()
                .iter(app.world())
                .find(|(_, found)| **found == item)
                .map(|(entity, _)| entity)
                .unwrap_or_else(|| panic!("{item:?} is not in the dock"));
            let children = app.world().get::<Children>(button).unwrap();
            assert!(children.iter().any(|child| app.world().get::<PlateIcon>(child).is_some()), "{item:?} has no icon");
            assert!(children.iter().all(|child| app.world().get::<Text>(child).is_none()), "{item:?} is written out");
            let bytes = std::fs::read(format!("assets/default/icons/{name}.svg")).unwrap_or_else(|_| panic!("no icons/{name}.svg"));
            assert!(kill_icon::rasterize(&bytes).is_ok(), "{name} does not draw");
        }
    }

    #[test]
    fn tools_opens_the_map_editor() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::Tools);
        app.update();
        assert_eq!(mode(&app), AppMode::Editor);
    }

    /// Fails if Direct Join does not open the join dialog, or leaves the menu clickable under it.
    #[test]
    fn direct_join_opens_the_connect_dialog_over_a_blocked_menu() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::DirectJoin);
        app.update();
        let dialog = app.world().resource::<ConnectDialog>();
        assert!(dialog.open && dialog.mode == ConnectMode::Join);
        assert!(displayed::<Blocker>(&mut app), "the menu took clicks under the dialog");
    }

    #[test]
    fn settings_asks_for_the_settings_page() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::Settings);
        assert!(app.world().contains_resource::<OpenSettings>());
        app.update();
        assert!(displayed::<Blocker>(&mut app));
    }

    /// Runs `Update` directly, since `InputPlugin` clears `just_pressed` in `PreUpdate`.
    fn press_escape(app: &mut App) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Escape);
        keybind::resolve_now(app.world_mut());
        app.world_mut().run_schedule(Update);
        app.world_mut().flush();
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::Escape);
    }

    /// Fails if Quit leaves without asking, or Back or Escape do not take the question down.
    #[test]
    fn quit_asks_and_back_or_escape_dismisses_it() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::Quit);
        app.update();
        assert!(displayed::<QuitRoot>(&mut app) && displayed::<Blocker>(&mut app));
        assert!(app.world().resource::<Messages<AppExit>>().is_empty(), "Quit left without asking");

        activate(&mut app, QuitItem::Back);
        app.update();
        assert!(!displayed::<QuitRoot>(&mut app) && !displayed::<Blocker>(&mut app));

        activate(&mut app, MenuItem::Quit);
        app.update();
        press_escape(&mut app);
        assert!(!displayed::<QuitRoot>(&mut app), "Escape left the question up");
    }

    #[test]
    fn quit_in_the_question_exits() {
        let mut app = app(AppMode::MainMenu);
        activate(&mut app, MenuItem::Quit);
        activate(&mut app, QuitItem::Quit);
        let exits: Vec<AppExit> = app.world().resource::<Messages<AppExit>>().iter_current_update_messages().cloned().collect();
        assert_eq!(exits, vec![AppExit::Success]);
    }
}
