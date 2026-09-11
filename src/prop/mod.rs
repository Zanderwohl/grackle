//! Modelling the things a map and a class refer to.
//!
//! A **prop** is a piece of geometry with a life of its own: a weapon, a
//! crate, a control point. It is not map content — it is not in the blueprint
//! and a map does not own it — it is *pack* content, sitting beside
//! `weapons.toml` and referred to by name, which is what lets one rocket
//! launcher be the model a weapon is drawn as, a prop a mapper places, and a
//! thing a class carries, without three descriptions of it.
//!
//! ## Why this is a feature list at all
//!
//! The same reason the map is: an edit halfway down is felt by everything
//! after it, so a barrel drilled through stays drilled through when the barrel
//! gets longer. What is deliberately different from
//! [`crate::editor::editable`] is that a modelling feature is an **enum**
//! rather than a `typetag` trait object — a boolean has to know what it is
//! subtracting, so the openness a trait buys would be a costume. See
//! [`feature`] for the whole argument.
//!
//! ## The pieces
//!
//! | Module | What is in it |
//! | --- | --- |
//! | [`csg`] | The kernel: a BSP tree, and three boolean operations over convex polygon soup. |
//! | [`profile`] | 2D loops, the planes they are drawn on, and ear clipping. |
//! | [`solid`] | Sweeps, primitives, transforms, and the bake to a Bevy `Mesh`. |
//! | [`nice_f32`] | Writing numbers to the file the way somebody typed them. |
//! | [`surface`] | Style and tint — what a solid is made of and what colour it was painted. |
//! | [`feature`] | The feature list and the evaluator that replays it. |
//! | [`document`] | The document, undo, and the file format. |
//! | [`figure`] | The body standing behind the prop, for scale. |
//! | [`view`] | Putting the result in the editor's viewports. |
//! | [`camera`] | Orbiting, panning and zooming in those viewports. |
//! | [`ui`] | The panels. |
//!
//! Nothing above [`view`] touches a `World`: evaluating a prop is a pure
//! function of its feature list. That is what makes the kernel testable
//! without a Bevy app, and what will let the *game* build a weapon's mesh from
//! the same code on a machine that has no editor in it.
//!
//! ## Everything here has to reach a browser
//!
//! A map blueprint is SQLite because only the editor ever reads one. A prop is
//! read by the game — it is what a weapon is *drawn as* — so it is flat text
//! read through [`crate::common::assets::AssetSource`], the kernel is
//! hand-written rather than a C library, and the only `std::fs` is on the
//! saving side. See "Targeting wasm" in `CLAUDE.md`.

use bevy::prelude::*;

use crate::common::app_mode::AppMode;
use crate::common::assets::{default_packs, Assets};
use crate::prop::document::{prop_path, PropDoc, PropEditor};

pub mod camera;
pub mod csg;
pub mod document;
pub mod feature;
pub mod figure;
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
        // `Assets` is initialised here as well as by `GamePlugin`, which is
        // idempotent and deliberate: the prop editor reads pack files and
        // should not be silently dependent on the game layer having been
        // wired up first.
        app.init_resource::<Assets>()
            .init_resource::<document::PropEditor>()
            .init_resource::<StartInPropEditor>()
            .add_plugins((view::PropViewPlugin, camera::PropCameraPlugin, ui::PropUiPlugin))
            .add_systems(Startup, load_the_example_prop)
            .add_systems(Update, start_in_the_prop_editor);
    }
}

/// What the prop editor opens onto.
///
/// A weapon rather than an empty document, and rather than a `new.gpp`
/// template the way the map editor has one: an empty modelling tool is a
/// blank screen with no sense of scale, and the first question anybody has is
/// how big a thing should be. It also means the path the *game* will read a
/// weapon's model by is exercised every time the editor starts, rather than
/// being written once and first run in anger a month later.
///
/// Loaded through [`Assets`] rather than `std::fs`, which is the whole point
/// of the format — see [`document`].
const EXAMPLE_PROP: &str = "rocket_launcher";

fn load_the_example_prop(assets: Res<Assets>, mut editor: ResMut<PropEditor>) {
    let packs = default_packs();
    // Highest priority last, the way the weapon catalogue is read, so a pack
    // overriding the example wins.
    for pack in packs.iter() {
        match document::load(&assets, pack, EXAMPLE_PROP) {
            Ok(doc) => {
                editor.open(doc, Some(prop_path(pack, EXAMPLE_PROP)));
                info!("Prop editor opened {EXAMPLE_PROP} from {}", pack.display());
            }
            // Not an error: a pack with no props is an ordinary pack, and an
            // editor that opens on an empty document is a working editor.
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
/// [`StartInPlay`](crate::common::app_mode::StartInPlay) is one: entering the
/// mode has to go through `OnEnter(AppMode::Prop)`, which is where the map is
/// set aside and the cameras are framed. A state set before the app runs would
/// skip the transition and leave somebody modelling inside a room.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct StartInPropEditor(pub bool);

/// Open the prop editor at startup if the command line asked for one.
///
/// Unlike `start_in_play` there is nothing to wait for: a prop is built from
/// its own document, so it does not care whether the map's rooms have reached
/// the world yet.
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

    /// `--prop` is not a keyboard shortcut and is deliberately kept: it is the
    /// only way to reach the mode from a script, and somebody spending an
    /// afternoon modelling should not have to walk in through a map they are
    /// not editing.
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

    /// Without the flag, the editor opens on the map like it always has.
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
