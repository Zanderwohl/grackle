//! Keyboard and mouse bindings, in three layers:
//!
//! 1. [`Keymap`] says which [`Chord`]s fire which [`Action`], and is stored in
//!    `assets/user/shortcuts.toml`.
//! 2. [`resolve_actions`] reads Bevy's `ButtonInput`s in `PreUpdate` and
//!    writes [`Actions`]: for each action, pressed, just pressed, just
//!    released. That is before `RunFixedMainLoop`, so the fixed loop sees a
//!    press in the frame it arrived.
//! 3. Systems read [`Actions`], never a `KeyCode`.
//!
//! egui only draws bindings: menu hints through [`hint`], and the Shortcuts
//! page.
//!
//! Escape and Enter are fixed ([`Action::FIXED`]), so the key that gives the
//! mouse back cannot be rebound away.
//!
//! While egui wants the keyboard, no key fires except bare modifiers; mouse
//! buttons still do. That is read from the previous frame's
//! [`EguiWantsInput`], because egui drops a text field's focus in the frame
//! the Escape arrives.
//!
//! An editor command matches exactly its modifiers, so `Ctrl+Shift+S` is not
//! also `Ctrl+S`. A game action matches at least them, so `R` reloads while
//! crouched on Ctrl ([`Action::exact`]). When two loose actions match one
//! press, the one whose chord the other extends is dropped, so a `Shift+G`
//! binding does not also fire `G`.

pub mod chord;
pub mod keymap;
pub mod sheet;

use std::sync::Arc;

use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::input::EguiWantsInput;

pub use chord::{Chord, Held, Input};
pub use keymap::Keymap;

use crate::get;

/// Where an action means something. Two actions clash only where their
/// contexts overlap ([`Context::overlaps`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Context {
    Everywhere,
    /// Every authoring mode.
    Editing,
    MapEditor,
    PropEditor,
    Animation,
    Play,
}

impl Context {
    pub const ALL: [Context; 6] =
        [Context::Everywhere, Context::Editing, Context::MapEditor, Context::PropEditor, Context::Animation, Context::Play];

    pub fn overlaps(self, other: Context) -> bool {
        use Context::*;
        let editing = |c| matches!(c, Editing | MapEditor | PropEditor | Animation);
        self == other || self == Everywhere || other == Everywhere || (editing(self) && editing(other) && (self == Editing || other == Editing))
    }

    pub fn name_key(self) -> &'static str {
        match self {
            Context::Everywhere => "keybind.context.everywhere",
            Context::Editing => "keybind.context.editing",
            Context::MapEditor => "keybind.context.map_editor",
            Context::PropEditor => "keybind.context.prop_editor",
            Context::Animation => "keybind.context.animation",
            Context::Play => "keybind.context.play",
        }
    }
}

macro_rules! actions {
    ($($variant:ident => $key:literal, $context:ident, $exact:literal, $debug:literal;)*) => {
        /// Something a key press or a mouse button asks for.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub enum Action { $($variant),* }

        impl Action {
            /// Every action, in the order the Shortcuts page lists them.
            pub const ALL: &'static [Action] = &[$(Action::$variant),*];

            /// Its name in `shortcuts.toml` and under `keybind.action` in
            /// the lang files.
            pub fn to_key(self) -> &'static str {
                match self { $(Action::$variant => $key),* }
            }

            pub fn context(self) -> Context {
                match self { $(Action::$variant => Context::$context),* }
            }

            /// Whether the chord's modifiers must be exactly those held,
            /// rather than at least those.
            pub fn exact(self) -> bool {
                match self { $(Action::$variant => $exact),* }
            }

            /// A debug view or cheat, listed apart on the Shortcuts page. It
            /// still clashes by its [`Context`].
            pub fn is_debug(self) -> bool {
                match self { $(Action::$variant => $debug),* }
            }
        }
    };
}

actions! {
    ToggleMode => "toggle_mode", Everywhere, false, false;
    Settings => "settings", Everywhere, true, false;
    Screenshot => "screenshot", Everywhere, false, false;

    New => "new", Editing, true, false;
    Save => "save", Editing, true, false;
    SaveAs => "save_as", Editing, true, false;
    Undo => "undo", Editing, true, false;
    Redo => "redo", Editing, true, false;
    Delete => "delete", Editing, true, false;
    Modify => "modify", Editing, false, false;
    Alternate => "alternate", Editing, false, false;
    Cancel => "cancel", Editing, false, false;
    Confirm => "confirm", Editing, false, false;

    FlyForward => "fly_forward", MapEditor, false, false;
    FlyBack => "fly_back", MapEditor, false, false;
    FlyLeft => "fly_left", MapEditor, false, false;
    FlyRight => "fly_right", MapEditor, false, false;
    FlyUp => "fly_up", MapEditor, false, false;
    FlyDown => "fly_down", MapEditor, false, false;

    FrameProp => "frame_prop", PropEditor, true, false;
    ResetView => "reset_view", PropEditor, true, false;
    SketchLine => "sketch_line", PropEditor, true, false;
    SketchErase => "sketch_erase", PropEditor, true, false;

    PlayClip => "play_clip", Animation, true, false;
    SetKey => "set_key", Animation, true, false;

    MoveForward => "move_forward", Play, false, false;
    MoveBack => "move_back", Play, false, false;
    MoveLeft => "move_left", Play, false, false;
    MoveRight => "move_right", Play, false, false;
    Jump => "jump", Play, false, false;
    Crouch => "crouch", Play, false, false;
    Sprint => "sprint", Play, false, false;
    Fire => "fire", Play, false, false;
    AltFire => "alt_fire", Play, false, false;
    Reload => "reload", Play, false, false;
    Bucket1 => "bucket_1", Play, false, false;
    Bucket2 => "bucket_2", Play, false, false;
    Bucket3 => "bucket_3", Play, false, false;
    Bucket4 => "bucket_4", Play, false, false;
    Bucket5 => "bucket_5", Play, false, false;
    Bucket6 => "bucket_6", Play, false, false;
    Bucket7 => "bucket_7", Play, false, false;
    Bucket8 => "bucket_8", Play, false, false;
    Bucket9 => "bucket_9", Play, false, false;
    ToggleView => "toggle_view", Play, false, false;
    Scoreboard => "scoreboard", Play, false, false;
    TeamPicker => "team_picker", Play, false, false;
    Taunt => "taunt", Play, false, false;
    Spray => "spray", Play, false, false;
    Pause => "pause", Play, false, false;

    PerfOverlay => "perf_overlay", Everywhere, false, true;
    ShowHitboxes => "show_hitboxes", Everywhere, false, true;
    ShowBones => "show_bones", Everywhere, false, true;
    Humiliate => "humiliate", Play, false, true;
    Suicide => "suicide", Play, false, true;
    Explode => "explode", Play, false, true;
    NavOverlay => "nav_overlay", Play, false, true;
    BotRoutes => "bot_routes", Play, false, true;
    SafeZone => "safe_zone", Play, false, true;
}

impl Action {
    /// On Escape and Enter. Never rebound and never saved.
    pub const FIXED: [Action; 3] = [Action::Pause, Action::Cancel, Action::Confirm];

    pub const BUCKETS: [Action; 9] = [
        Action::Bucket1,
        Action::Bucket2,
        Action::Bucket3,
        Action::Bucket4,
        Action::Bucket5,
        Action::Bucket6,
        Action::Bucket7,
        Action::Bucket8,
        Action::Bucket9,
    ];

    /// Fire while the Settings window is open, and while a text field has the
    /// keyboard when the chord is a function key, which types nothing. Never
    /// while a binding is waited for, so the key can be bound.
    pub const THROUGH_QUIET: [Action; 1] = [Action::Screenshot];

    pub fn is_fixed(self) -> bool {
        Self::FIXED.contains(&self)
    }

    /// Only actions a file may bind, so a file cannot rebind Escape or Enter.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|action| !action.is_fixed() && action.to_key() == key)
    }

    fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> String {
        crate::common::lang::get_parsed(&format!("keybind.action.{}", self.to_key()))
    }
}

const COUNT: usize = Action::ALL.len();

/// Written by [`resolve_actions`] only, and only when it differs.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Actions {
    pressed: [bool; COUNT],
    just_pressed: [bool; COUNT],
    just_released: [bool; COUNT],
}

impl Default for Actions {
    fn default() -> Self {
        Self { pressed: [false; COUNT], just_pressed: [false; COUNT], just_released: [false; COUNT] }
    }
}

impl Actions {
    pub fn pressed(&self, action: Action) -> bool {
        self.pressed[action.index()]
    }

    pub fn just_pressed(&self, action: Action) -> bool {
        self.just_pressed[action.index()]
    }

    pub fn just_released(&self, action: Action) -> bool {
        self.just_released[action.index()]
    }

    pub fn axis(&self, negative: Action, positive: Action) -> f32 {
        self.pressed(positive) as i8 as f32 - self.pressed(negative) as i8 as f32
    }

    fn only(self, kept: &[Action]) -> Self {
        let mut only = Self::default();
        for &action in kept {
            let i = action.index();
            only.pressed[i] = self.pressed[i];
            only.just_pressed[i] = self.just_pressed[i];
            only.just_released[i] = self.just_released[i];
        }
        only
    }

    /// For a test that does not run [`resolve_actions`].
    #[cfg(test)]
    pub fn press(&mut self, action: Action) {
        self.pressed[action.index()] = true;
        self.just_pressed[action.index()] = true;
    }

    /// Needs no world, for the tests.
    pub fn read(
        keymap: &Keymap,
        keys: Option<&ButtonInput<KeyCode>>,
        buttons: Option<&ButtonInput<MouseButton>>,
        quiet_keys: bool,
    ) -> Self {
        let mut actions = Self::default();
        let held = keys.map(Held::of).unwrap_or_default();
        let state = |action: Action, chord: Chord| -> (bool, bool, bool) {
            let heard = !quiet_keys || (Action::THROUGH_QUIET.contains(&action) && chord.types_nothing());
            match chord.input {
                // A bare modifier types nothing, so a Shift-click still adds to
                // a selection while a text field has the keyboard.
                Input::Key(key) => match keys {
                    Some(keys) if heard || chord::modifier_of(key).is_some() => {
                        (keys.pressed(key), keys.just_pressed(key), keys.just_released(key))
                    }
                    _ => (false, false, false),
                },
                Input::Mouse(button) => match buttons {
                    Some(buttons) => (buttons.pressed(button), buttons.just_pressed(button), buttons.just_released(button)),
                    None => (false, false, false),
                },
            }
        };

        // (action, chord) for every chord pressed this frame, to shadow below.
        let mut fresh: Vec<(Action, Chord)> = Vec::new();
        for &action in Action::ALL {
            let mut released = false;
            for &chord in keymap.chords(action) {
                let (pressed, just_pressed, just_released) = state(action, chord);
                let modifiers = chord.modifiers_match(held, action.exact());
                if pressed && modifiers {
                    actions.pressed[action.index()] = true;
                }
                if just_pressed && modifiers {
                    fresh.push((action, chord));
                }
                released |= just_released;
            }
            actions.just_released[action.index()] = released && !actions.pressed[action.index()];
        }

        for &(action, chord) in &fresh {
            let extended = fresh.iter().any(|&(other, longer)| {
                other != action
                    && !action.exact()
                    && !other.exact()
                    && other.context().overlaps(action.context())
                    && longer.input == chord.input
                    && longer.modifiers().count() > chord.modifiers().count()
                    && chord.modifiers_match(longer.modifiers(), false)
            });
            if extended {
                actions.pressed[action.index()] = false;
            } else {
                actions.just_pressed[action.index()] = true;
            }
        }
        actions
    }
}

/// Written by the Settings window, so a key being bound does nothing else.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quiet {
    #[default]
    Off,
    /// The window is open: only [`Action::THROUGH_QUIET`] fires.
    Window,
    /// A binding is waited for: nothing fires.
    Binding,
}

/// Every input resource is optional, so a test world with only a keyboard
/// still resolves.
pub fn resolve_actions(
    keymap: Res<Keymap>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    egui: Option<Res<EguiWantsInput>>,
    quiet: Res<Quiet>,
    mut actions: ResMut<Actions>,
) {
    let typing = egui.is_some_and(|egui| egui.wants_any_keyboard_input());
    let read = || Actions::read(&keymap, keys.as_deref(), buttons.as_deref(), typing);
    let heard = match *quiet {
        Quiet::Off => read(),
        Quiet::Window => read().only(&Action::THROUGH_QUIET),
        Quiet::Binding => Actions::default(),
    };
    actions.set_if_neq(heard);
}

/// For tests that run systems one at a time rather than through `PreUpdate`.
pub fn resolve_now(world: &mut World) {
    ensure_resources(world);
    world.run_system_cached(resolve_actions).expect("resolve_actions failed");
}

fn ensure_resources(world: &mut World) {
    world.init_resource::<Keymap>();
    world.init_resource::<Actions>();
    world.init_resource::<Quiet>();
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolveActions;

pub struct KeybindPlugin;

impl Plugin for KeybindPlugin {
    fn build(&self, app: &mut App) {
        ensure_resources(app.world_mut());
        app.add_systems(PreUpdate, resolve_actions.in_set(ResolveActions).after(InputSystems));
    }
}

/// Every plugin that reads [`Actions`] calls this, since those plugins also
/// stand alone in tests.
pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<KeybindPlugin>() {
        app.add_plugins(KeybindPlugin);
    }
}

fn hints_id() -> egui::Id {
    egui::Id::new("grackle_keymap")
}

/// For [`hint`], since a menu gets only a `Ui`.
pub fn publish(ctx: &egui::Context, keymap: &Keymap) {
    ctx.data_mut(|data| data.insert_temp(hints_id(), Arc::new(keymap.clone())));
}

/// The first chord, for a menu entry.
pub fn hint(ctx: &egui::Context, action: Action) -> Option<String> {
    let keymap = ctx.data(|data| data.get_temp::<Arc<Keymap>>(hints_id()))?;
    keymap.chords(action).first().map(|chord| chord.label())
}

pub fn context_name(context: Context) -> String {
    get!(context.name_key())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(held: &[KeyCode], hit: KeyCode) -> ButtonInput<KeyCode> {
        let mut keys = ButtonInput::<KeyCode>::default();
        for key in held {
            keys.press(*key);
        }
        // Held from an earlier frame, so only `hit` is an edge.
        keys.clear();
        keys.press(hit);
        keys
    }

    fn fired(held: &[KeyCode], hit: KeyCode) -> Vec<Action> {
        let actions = Actions::read(&Keymap::default(), Some(&keys(held, hit)), None, false);
        Action::ALL.iter().copied().filter(|a| actions.just_pressed(*a)).collect()
    }

    #[test]
    fn every_bindable_action_reads_back_from_its_key() {
        for &action in Action::ALL {
            match action.is_fixed() {
                true => assert_eq!(Action::from_key(action.to_key()), None, "{action:?} can be rebound"),
                false => assert_eq!(Action::from_key(action.to_key()), Some(action)),
            }
        }
    }

    fn commands(held: &[KeyCode], hit: KeyCode) -> Vec<Action> {
        fired(held, hit).into_iter().filter(|a| a.context() == Context::Editing).collect()
    }

    #[test]
    fn shift_separates_save_from_save_as() {
        assert_eq!(commands(&[KeyCode::ControlLeft], KeyCode::KeyS), vec![Action::Save]);
        assert_eq!(commands(&[KeyCode::SuperLeft, KeyCode::ShiftLeft], KeyCode::KeyS), vec![Action::SaveAs]);
    }

    #[test]
    fn undo_and_redo_are_the_usual_three_ways() {
        assert_eq!(commands(&[KeyCode::SuperLeft], KeyCode::KeyZ), vec![Action::Undo]);
        assert_eq!(commands(&[KeyCode::SuperLeft, KeyCode::ShiftLeft], KeyCode::KeyZ), vec![Action::Redo]);
        assert_eq!(commands(&[KeyCode::ControlLeft], KeyCode::KeyY), vec![Action::Redo]);
    }

    fn screenshot_heard(quiet: Quiet, keymap: &Keymap, hit: KeyCode) -> bool {
        let mut world = World::new();
        world.insert_resource(keymap.clone());
        world.insert_resource(keys(&[], hit));
        world.insert_resource(quiet);
        resolve_now(&mut world);
        world.resource::<Actions>().just_pressed(Action::Screenshot)
    }

    /// Fails if the Settings window hides itself from screenshots, or F12 binds as a screenshot.
    #[test]
    fn a_screenshot_fires_over_the_settings_window_but_not_while_binding() {
        let keymap = Keymap::default();
        assert!(screenshot_heard(Quiet::Off, &keymap, KeyCode::F12));
        assert!(screenshot_heard(Quiet::Window, &keymap, KeyCode::F12));
        assert!(!screenshot_heard(Quiet::Binding, &keymap, KeyCode::F12));
        let saving = Actions::read(&keymap, Some(&keys(&[KeyCode::ControlLeft], KeyCode::KeyS)), None, false)
            .only(&Action::THROUGH_QUIET);
        assert!(!saving.just_pressed(Action::Save), "the window let more than the screenshot through");
    }

    /// Fails if a screenshot bound to a letter fires as that letter is typed.
    #[test]
    fn a_screenshot_fires_over_a_text_field_only_from_a_function_key() {
        let read = |keymap: &Keymap, hit| {
            Actions::read(keymap, Some(&keys(&[], hit)), None, true).just_pressed(Action::Screenshot)
        };
        assert!(read(&Keymap::default(), KeyCode::F12));
        let mut lettered = Keymap::default();
        lettered.bind(Action::Screenshot, 0, Chord::key(KeyCode::KeyP));
        assert!(!read(&lettered, KeyCode::KeyP));
    }

    /// Typing `s` into a viewport must not save.
    #[test]
    fn a_bare_letter_is_not_a_command() {
        for key in [KeyCode::KeyS, KeyCode::KeyZ, KeyCode::KeyN] {
            assert!(commands(&[], key).is_empty(), "{key:?}");
        }
    }

    /// Fails if a crouched player cannot reload.
    #[test]
    fn a_game_key_fires_with_a_modifier_held() {
        let fired = fired(&[KeyCode::ControlLeft], KeyCode::KeyR);
        assert!(fired.contains(&Action::Reload), "{fired:?}");
    }

    /// Fails if a `Shift+G` binding also fires the action on `G`.
    #[test]
    fn the_longer_chord_shadows_the_shorter() {
        let mut keymap = Keymap::default();
        keymap.bind(Action::Spray, 0, Chord::key(KeyCode::KeyG).with_shift());
        let read = |held: &[KeyCode]| Actions::read(&keymap, Some(&keys(held, KeyCode::KeyG)), None, false);
        let shifted = read(&[KeyCode::ShiftLeft]);
        assert!(shifted.just_pressed(Action::Spray));
        assert!(!shifted.just_pressed(Action::Taunt), "Shift+G also taunted");
        assert!(read(&[]).just_pressed(Action::Taunt));
    }

    #[test]
    fn a_held_key_is_pressed_and_its_edge_only_once() {
        let mut input = ButtonInput::<KeyCode>::default();
        input.press(KeyCode::KeyW);
        let first = Actions::read(&Keymap::default(), Some(&input), None, false);
        assert!(first.pressed(Action::MoveForward) && first.just_pressed(Action::MoveForward));
        input.clear();
        let second = Actions::read(&Keymap::default(), Some(&input), None, false);
        assert!(second.pressed(Action::MoveForward) && !second.just_pressed(Action::MoveForward));
        input.release(KeyCode::KeyW);
        let third = Actions::read(&Keymap::default(), Some(&input), None, false);
        assert!(!third.pressed(Action::MoveForward) && third.just_released(Action::MoveForward));
    }

    /// Fails if typing into a text field fires a key, or stops the mouse.
    #[test]
    fn while_egui_has_the_keyboard_only_the_mouse_and_modifiers_fire() {
        let mut buttons = ButtonInput::<MouseButton>::default();
        buttons.press(MouseButton::Left);
        let held = [KeyCode::ControlLeft, KeyCode::ShiftLeft];
        let actions = Actions::read(&Keymap::default(), Some(&keys(&held, KeyCode::KeyS)), Some(&buttons), true);
        assert!(!actions.just_pressed(Action::SaveAs));
        assert!(actions.just_pressed(Action::Fire));
        assert!(actions.pressed(Action::Modify), "a Shift-click stopped adding to a selection");
    }

    #[test]
    fn contexts_overlap_where_both_could_be_live() {
        assert!(Context::Everywhere.overlaps(Context::Play));
        assert!(Context::Editing.overlaps(Context::PropEditor));
        assert!(!Context::PropEditor.overlaps(Context::Animation));
        assert!(!Context::Play.overlaps(Context::MapEditor));
    }

    /// Fails if an action has no name, so the Shortcuts page shows its key.
    #[test]
    fn every_action_and_context_has_a_name() {
        let Ok(text) = std::fs::read_to_string("assets/default/lang/en-US.toml") else { return };
        let table: toml::Value = toml::from_str(&text).unwrap();
        let keybind = table.get("keybind").expect("en-US has no [keybind]");
        for &action in Action::ALL {
            assert!(
                keybind.get("action").and_then(|t| t.get(action.to_key())).is_some_and(|v| v.is_str()),
                "en-US has no keybind.action.{}",
                action.to_key(),
            );
        }
        for context in Context::ALL {
            let key = context.name_key().strip_prefix("keybind.context.").unwrap();
            assert!(keybind.get("context").and_then(|t| t.get(key)).is_some(), "no {}", context.name_key());
        }
    }
}
