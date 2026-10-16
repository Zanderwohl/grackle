//! Modelling props: geometry referred to by name from a pack, not map
//! content. See "Modelling a prop" in `CLAUDE.md`.
//!
//! | Module | What is in it |
//! | --- | --- |
//! | [`archetype`] | What a prop is to the game: held, a hat, or placed in a map. |
//! | [`baked`] | Props baked into mesh handles, by name, for anything that draws one. |
//! | [`csg`] | The kernel: a BSP tree and three booleans over convex polygon soup. |
//! | [`profile`] | 2D loops, the planes they are drawn on, and ear clipping. |
//! | [`reference`] | Naming corners and edges by the generator that made them. |
//! | [`plane`] | Origin planes, and the plane a selection describes. |
//! | [`sketch`] | 2D drawings on a plane: shared points and lines. |
//! | [`region`] | The closed regions a sketch encloses, and their names. |
//! | [`solid`] | Sweeps, primitives, transforms, and the bake to a `Mesh`. |
//! | [`nice_f32`] | Writing numbers to the file the way somebody typed them. |
//! | [`surface`] | Style and tint. |
//! | [`hold`] | Where the hands go on a weapon, and where it is carried. |
//! | [`feature`] | The feature list and the evaluator that replays it. |
//! | [`document`] | The document, undo, and the file format. |
//! | [`figure`] | The body standing behind the prop, for scale. |
//! | [`view`] | Putting the result in the editor's viewports. |
//! | [`camera`] | Orbiting, panning and zooming in those viewports. |
//! | [`gizmo`] | Moving and sizing the selected feature by dragging it. |
//! | [`sync`] | Handing every weapon's model to whoever connects. |
//! | [`thumbnail`] | Photographing a prop, and keeping the picture on disk. |
//! | [`ui`] | The panels. |
//!
//! - Nothing above [`view`] touches a `World`, so the kernel is testable
//!   without an app and the game can build a mesh with no editor present.
//! - All of it has to reach a browser: flat text through
//!   [`AssetSource`](crate::common::assets::AssetSource), no C library, and
//!   `std::fs` only on the saving side.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::{default_packs, Assets};
use crate::prop::document::{prop_path, PropDoc, PropEditor};

pub mod archetype;
pub mod baked;
pub mod camera;
pub mod csg;
pub mod document;
pub mod feature;
pub mod figure;
pub mod gizmo;
pub mod hold;
pub mod nice_f32;
pub mod plane;
pub mod profile;
pub mod reference;
pub mod region;
pub mod sketch;
pub mod solid;
pub mod sync;
pub mod surface;
pub mod thumbnail;
pub mod ui;
pub mod view;

/// The prop editor: a mode, a viewport, and the panels around it.
pub struct PropEditorPlugin;

impl Plugin for PropEditorPlugin {
    fn build(&self, app: &mut App) {
        // Also initialised by `GamePlugin`; done again here so reading pack
        // files does not depend on the game layer being wired up first.
        app.init_resource::<Assets>()
            .init_resource::<baked::PropCache>()
            .init_resource::<baked::SurfaceMaterials>()
            .add_plugins(crate::common::skeleton::ClipAssetsPlugin)
            .init_resource::<document::PropEditor>()
            .init_resource::<StartInPropEditor>()
            .add_plugins((
                view::PropViewPlugin,
                camera::PropCameraPlugin,
                gizmo::PropGizmoPlugin,
                plane::PropPlanePlugin,
                ui::PropUiPlugin,
            ))
            .add_systems(Startup, load_the_example_prop)
            // Ungated: the save happens here but what must notice it is a
            // body in a round.
            .add_systems(Update, (start_in_the_prop_editor, publish_the_saved_prop));
    }
}

/// A weapon rather than an empty document: a blank tool gives no sense of
/// scale, and it exercises the game's load path on every start.
const EXAMPLE_PROP: &str = "rocket_launcher";

fn load_the_example_prop(assets: Res<Assets>, mut editor: ResMut<PropEditor>) {
    // Highest priority last, the way the weapon catalogue is read, so a pack
    // overriding the example wins.
    for pack in default_packs().iter() {
        match document::load(&assets, pack, EXAMPLE_PROP) {
            Ok(doc) => {
                editor.open(doc, Some(prop_path(pack, EXAMPLE_PROP)));
                info!("Prop editor opened {EXAMPLE_PROP} from {}", pack.display());
            }
            // A pack with no props is an ordinary pack.
            Err(document::PropFileError::Asset(_)) => {}
            Err(e) => error!("{e}"),
        }
    }
    if editor.path.is_none() {
        editor.open(PropDoc::new("prop.untitled"), None);
    }
}

/// Saving is publishing: the cache bakes once per name, so without this a
/// held weapon keeps its pre-save shape for the rest of the process.
///
/// Watches a save counter rather than `Res::is_changed`, which fires on
/// every frame of a drag.
fn publish_the_saved_prop(
    editor: Res<PropEditor>,
    mut cache: ResMut<baked::PropCache>,
    mut surfaces: ResMut<baked::SurfaceMaterials>,
    mut meshes: ResMut<bevy::asset::Assets<Mesh>>,
    mut materials: ResMut<bevy::asset::Assets<StandardMaterial>>,
    mut clips: ResMut<bevy::asset::Assets<crate::common::skeleton::clip::Clip>>,
    mut seen: Local<u64>,
) {
    if editor.saves() == *seen {
        return;
    }
    *seen = editor.saves();

    let Some(name) = editor.saved_name() else { return };
    info!("Saved {name}; handing it to anything holding one");
    cache.adopt(
        name,
        editor.doc(),
        &mut meshes,
        &mut materials,
        &mut surfaces,
        &mut clips,
        editor.path.as_deref(),
    );
}

/// Whether `--prop` was given. A resource rather than an initial state so
/// that entering still runs `OnEnter(AppMode::Prop)`.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInPropEditor(pub bool);

fn start_in_the_prop_editor(
    start: Res<StartInPropEditor>,
    mut next: ResMut<NextState<AppMode>>,
    mut done: Local<bool>,
) {
    if !start.0 || *done {
        return;
    }
    info!("Opening the prop editor, as asked");
    next.set(AppMode::Prop);
    *done = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn app_with_assets() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::render::mesh::MeshPlugin));
        app.init_asset::<StandardMaterial>();
        app.add_plugins(crate::common::skeleton::ClipAssetsPlugin);
        app.init_resource::<baked::PropCache>()
            .init_resource::<baked::SurfaceMaterials>()
            .init_resource::<PropEditor>()
            .add_systems(Update, publish_the_saved_prop);
        app
    }

    /// Fails if a saved model does not reach the cache the game draws from.
    #[test]
    fn saving_a_prop_hands_it_to_the_game() {
        let mut app = app_with_assets();
        app.update();
        assert!(
            !app.world().resource::<baked::PropCache>().knows("cudgel"),
            "nothing was saved and the cache has an opinion anyway"
        );

        app.world_mut()
            .resource_mut::<PropEditor>()
            .mark_saved(PathBuf::from("somewhere/props/cudgel.gpp"));
        app.update();

        let cache = app.world().resource::<baked::PropCache>();
        assert!(cache.knows("cudgel"), "the saved model never reached the cache");
        assert!(cache.generation() > 0, "nothing holding one would look again");
    }

    /// Fails if an unsaved edit re-bakes the cache.
    #[test]
    fn merely_editing_publishes_nothing() {
        let mut app = app_with_assets();
        app.world_mut().resource_mut::<PropEditor>().mark_saved(PathBuf::from("a/cudgel.gpp"));
        app.update();
        let generation = app.world().resource::<baked::PropCache>().generation();

        for _ in 0..5 {
            app.world_mut().resource_mut::<PropEditor>().edit(|doc| {
                doc.name_key = "prop.changed".into();
            });
            app.update();
        }

        assert_eq!(
            app.world().resource::<baked::PropCache>().generation(),
            generation,
            "an edit republished as though it had been saved"
        );
    }

    #[test]
    fn the_flag_opens_the_prop_editor() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .insert_resource(StartInPropEditor(true))
            .add_systems(Update, start_in_the_prop_editor);

        app.update();
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Prop);
    }

    #[test]
    fn without_the_flag_nothing_happens() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<AppMode>()
            .insert_resource(StartInPropEditor(false))
            .add_systems(Update, start_in_the_prop_editor);

        app.update();
        app.update();
        assert_eq!(*app.world().resource::<State<AppMode>>().get(), AppMode::Editor);
    }
}
