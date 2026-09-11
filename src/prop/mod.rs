//! Modelling the things a map and a class refer to.
//!
//! A **prop** is geometry with a life of its own — a weapon, a crate, a
//! control point. Not map content: it lives in a pack beside `weapons.toml`
//! and is referred to by name, so one model serves a weapon a class carries,
//! a prop a mapper places, and whatever comes next.
//!
//! | Module | What is in it |
//! | --- | --- |
//! | [`baked`] | Props baked into mesh handles, by name, for anything that draws one. |
//! | [`csg`] | The kernel: a BSP tree and three booleans over convex polygon soup. |
//! | [`profile`] | 2D loops, the planes they are drawn on, and ear clipping. |
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
//! | [`ui`] | The panels. |
//!
//! Two things hold the design together, both argued in "Modelling a prop" in
//! `CLAUDE.md`:
//!
//! - **Nothing above [`view`] touches a `World`.** Evaluating a prop is a pure
//!   function of its feature list, which is what makes the kernel testable
//!   without a Bevy app and what will let the *game* build a weapon's mesh on
//!   a machine with no editor in it.
//! - **All of it has to reach a browser.** Hence flat text through
//!   [`AssetSource`](crate::common::assets::AssetSource), a hand-written
//!   kernel rather than a C library, and `std::fs` only on the saving side.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::{default_packs, Assets};
use crate::prop::document::{prop_path, PropDoc, PropEditor};

pub mod baked;
pub mod camera;
pub mod csg;
pub mod document;
pub mod feature;
pub mod figure;
pub mod gizmo;
pub mod hold;
pub mod nice_f32;
pub mod profile;
pub mod solid;
pub mod sync;
pub mod surface;
pub mod ui;
pub mod view;

/// The prop editor: a mode, a viewport, and the panels around it.
pub struct PropEditorPlugin;

impl Plugin for PropEditorPlugin {
    fn build(&self, app: &mut App) {
        // Also initialised by `GamePlugin`; done again here so reading pack
        // files does not depend on the game layer being wired up first.
        app.init_resource::<Assets>()
            // Shared with the game, which bakes the same props to put them in
            // somebody's hands — see `prop::baked`.
            .init_resource::<baked::PropCache>()
            .init_resource::<baked::SurfaceMaterials>()
            .init_resource::<document::PropEditor>()
            .init_resource::<StartInPropEditor>()
            .add_plugins((
                view::PropViewPlugin,
                camera::PropCameraPlugin,
                gizmo::PropGizmoPlugin,
                ui::PropUiPlugin,
            ))
            .add_systems(Startup, load_the_example_prop)
            // Ungated: saving happens in the prop editor and the thing that
            // has to notice is a body in a round, so a system that stopped at
            // the mode boundary would be one that only ever ran where it had
            // nothing to do.
            .add_systems(Update, (start_in_the_prop_editor, publish_the_saved_prop));
    }
}

/// What the prop editor opens onto.
///
/// A weapon rather than an empty document: a blank modelling tool gives no
/// sense of scale, and it means the path the *game* will read a model by is
/// exercised every time the editor starts.
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

/// Hand a saved model to whatever is holding one.
///
/// **Saving is the publish.** The cache is keyed by name and bakes once, so
/// without this a weapon drawn before the edit keeps the shape it had for the
/// rest of the process — you save, press F5, and are holding the old barrel.
/// [`PropCache::adopt`] is the same door the server's models come through, and
/// it bumps the generation, which is what makes a body redress.
///
/// Watched through a counter rather than `Res::is_changed`, because every
/// frame of a drag changes the editor and only a write to disk is a publish.
fn publish_the_saved_prop(
    editor: Res<PropEditor>,
    mut cache: ResMut<baked::PropCache>,
    mut surfaces: ResMut<baked::SurfaceMaterials>,
    mut meshes: ResMut<bevy::asset::Assets<Mesh>>,
    mut materials: ResMut<bevy::asset::Assets<StandardMaterial>>,
    mut seen: Local<u64>,
) {
    if editor.saves() == *seen {
        return;
    }
    *seen = editor.saves();

    let Some(name) = editor.saved_name() else { return };
    info!("Saved {name}; handing it to anything holding one");
    cache.adopt(name, editor.doc(), &mut meshes, &mut materials, &mut surfaces);
}

/// Whether `--prop` was given.
///
/// A resource rather than a different initial state, for the reason
/// [`StartInPlay`](crate::common::app_mode::StartInPlay) is one: entering has
/// to go through `OnEnter(AppMode::Prop)`, where the map is set aside and the
/// cameras are framed. A state set before the app runs skips all of it.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInPropEditor(pub bool);

/// Unlike `start_in_play` there is nothing to wait for: a prop is built from
/// its own document, not from the map's entities.
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
        app.init_resource::<baked::PropCache>()
            .init_resource::<baked::SurfaceMaterials>()
            .init_resource::<PropEditor>()
            .add_systems(Update, publish_the_saved_prop);
        app
    }

    /// The whole point: a model edited in the prop editor is the model in
    /// somebody's hands, without restarting the process.
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

    /// The cache is what the *game* reads, and re-baking it on every frame of
    /// a drag would be the boolean kernel running at frame rate for nobody.
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

    /// The mode has no keyboard shortcut, so this flag is the only way into it
    /// from a script.
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
