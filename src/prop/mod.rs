//! Modelling the things a map and a class refer to.
//!
//! A **prop** is geometry with a life of its own — a weapon, a crate, a
//! control point. Not map content: it lives in a pack beside `weapons.toml`
//! and is referred to by name, so one model serves a weapon a class carries,
//! a prop a mapper places, and whatever comes next.
//!
//! | Module | What is in it |
//! | --- | --- |
//! | [`csg`] | The kernel: a BSP tree and three booleans over convex polygon soup. |
//! | [`profile`] | 2D loops, the planes they are drawn on, and ear clipping. |
//! | [`solid`] | Sweeps, primitives, transforms, and the bake to a `Mesh`. |
//! | [`nice_f32`] | Writing numbers to the file the way somebody typed them. |
//! | [`surface`] | Style and tint. |
//! | [`feature`] | The feature list and the evaluator that replays it. |
//! | [`document`] | The document, undo, and the file format. |
//! | [`figure`] | The body standing behind the prop, for scale. |
//! | [`view`] | Putting the result in the editor's viewports. |
//! | [`camera`] | Orbiting, panning and zooming in those viewports. |
//! | [`gizmo`] | Moving and sizing the selected feature by dragging it. |
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

pub mod camera;
pub mod csg;
pub mod document;
pub mod feature;
pub mod figure;
pub mod gizmo;
pub mod nice_f32;
pub mod profile;
pub mod solid;
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
            .init_resource::<document::PropEditor>()
            .init_resource::<StartInPropEditor>()
            .add_plugins((
                view::PropViewPlugin,
                camera::PropCameraPlugin,
                gizmo::PropGizmoPlugin,
                ui::PropUiPlugin,
            ))
            .add_systems(Startup, load_the_example_prop)
            .add_systems(Update, start_in_the_prop_editor);
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
