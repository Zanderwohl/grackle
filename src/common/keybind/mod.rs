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
//! egui only draws: menu hints through [`hint`], and the Shortcuts page of
//! the Settings window. Capturing a new binding reads Bevy's input too.
//!
//! **Escape and Enter are fixed** ([`Action::FIXED`]): Escape is the pause menu
//! and the editors' cancel, Enter their confirm. Nothing can be bound to them,
//! so the key that gets the mouse back is always the same key.
//!
//! **While egui wants the keyboard, no key fires** (mouse buttons still do):
//! `Ctrl+Z` in a text field means undo the text. Read from the previous
//! frame's [`EguiWantsInput`], since egui drops a text field's focus on
//! Escape in the frame the key arrives.
//!
//! Two matching rules ([`Action::exact`]): an editor command wants exactly its
//! modifiers, so `Ctrl+Shift+S` is not also `Ctrl+S`; a game action wants at
//! least them, so `R` reloads while crouched on Ctrl. When several loose
//! actions match one press, those whose chord another's chord extends are
//! dropped, so `Shift+B` does not also fire `B`.

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
    ($($variant:ident => $key:literal, $context:ident, $exact:literal;)*) => {
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
            /// rather than at least those. See the module documentation.
            pub fn exact(self) -> bool {
                match self { $(Action::$variant => $exact),* }
            }
        }
    };
}

actions! {
    ToggleMode => "toggle_mode", Everywhere, false;
    Settings => "settings", Everywhere, true;
    PerfOverlay => "perf_overlay", Everywhere, false;
    ShowHitboxes => "show_hitboxes", Everywhere, false;
    ShowBones => "show_bones", Everywhere, false;

    New => "new", Editing, true;
    Save => "save", Editing, true;
    SaveAs => "save_as", Editing, true;
    Undo => "undo", Editing, true;
    Redo => "redo", Editing, true;
    Delete => "delete", Editing, true;
    Modify => "modify", Editing, false;
    Alternate => "alternate", Editing, false;
    Cancel => "cancel", Editing, false;
    Confirm => "confirm", Editing, false;

    FlyForward => "fly_forward", MapEditor, false;
    FlyBack => "fly_back", MapEditor, false;
    FlyLeft => "fly_left", MapEditor, false;
    FlyRight => "fly_right", MapEditor, false;
    FlyUp => "fly_up", MapEditor, false;
    FlyDown => "fly_down", MapEditor, false;

    FrameProp => "frame_prop", PropEditor, true;
    ResetView => "reset_view", PropEditor, true;
    SketchLine => "sketch_line", PropEditor, true;
    SketchErase => "sketch_erase", PropEditor, true;

    PlayClip => "play_clip", Animation, true;
    SetKey => "set_key", Animation, true;

    MoveForward => "move_forward", Play, false;
    MoveBack => "move_back", Play, false;
    MoveLeft => "move_left", Play, false;
    MoveRight => "move_right", Play, false;
    Jump => "jump", Play, false;
    Crouch => "crouch", Play, false;
    Sprint => "sprint", Play, false;
    Fire => "fire", Play, false;
    AltFire => "alt_fire", Play, false;
    Reload => "reload", Play, false;
    Bucket1 => "bucket_1", Play, false;
    Bucket2 => "bucket_2", Play, false;
    Bucket3 => "bucket_3", Play, false;
    Bucket4 => "bucket_4", Play, false;
    Bucket5 => "bucket_5", Play, false;
    Bucket6 => "bucket_6", Play, false;
    Bucket7 => "bucket_7", Play, false;
    Bucket8 => "bucket_8", Play, false;
    Bucket9 => "bucket_9", Play, false;
    ToggleView => "toggle_view", Play, false;
    Scoreboard => "scoreboard", Play, false;
    TeamPicker => "team_picker", Play, false;
    Taunt => "taunt", Play, false;
    Spray => "spray", Play, false;
    Pause => "pause", Play, false;
    Humiliate => "humiliate", Play, false;
    Suicide => "suicide", Play, false;
    SeatBot => "seat_bot", Play, false;
    RetireBot => "retire_bot", Play, false;
    NavOverlay => "nav_overlay", Play, false;
    BotRoutes => "bot_routes", Play, false;
    SafeZone => "safe_zone", Play, false;
}

impl Action {
    /// The actions on Escape and Enter. They read from the table like any
    /// other action but cannot be rebound, and are not saved.
    pub const FIXED: [Action; 3] = [Action::Pause, Action::Cancel, Action::Confirm];

    /// The weapon buckets, `1` first.
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

/// What each action is doing this frame. Written by [`resolve_actions`] only.
#[derive(Resource, Clone, Debug)]
pub struct Actions {
    pressed: Vec<bool>,
    just_pressed: Vec<bool>,
    just_released: Vec<bool>,
}

impl Default for Actions {
    fn default() -> Self {
        let none = vec![false; Action::ALL.len()];
        Self { pressed: none.clone(), just_pressed: none.clone(), just_released: none }
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

    /// `positive` minus `negative`, each pressed counting one.
    pub fn axis(&self, negative: Action, positive: Action) -> f32 {
        self.pressed(positive) as i8 as f32 - self.pressed(negative) as i8 as f32
    }

    /// The action as pressed, for a test that does not run [`resolve_actions`].
    #[cfg(test)]
    pub fn press(&mut self, action: Action) {
        self.pressed[action.index()] = true;
        self.just_pressed[action.index()] = true;
    }

    /// What `keymap` makes of these inputs. Needs no world, for the tests.
    pub fn read(
        keymap: &Keymap,
        keys: Option<&ButtonInput<KeyCode>>,
        buttons: Option<&ButtonInput<MouseButton>>,
        quiet_keys: bool,
    ) -> Self {
        let mut actions = Self::default();
        let held = keys.map(Held::of).unwrap_or_default();
        let state = |input: Input| -> (bool, bool, bool) {
            match input {
                Input::Key(key) => match keys {
                    Some(keys) if !quiet_keys => (keys.pressed(key), keys.just_pressed(key), keys.just_released(key)),
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
                let (pressed, just_pressed, just_released) = state(chord.input);
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

/// Set while the Settings window is open, so a key being bound, or one pressed
/// over the window, does nothing else.
#[derive(Resource, Default, PartialEq)]
pub struct Quiet(pub bool);

/// Reads the inputs into [`Actions`]. Every input resource is optional, so a
/// test world with only a keyboard still resolves.
pub fn resolve_actions(
    keymap: Res<Keymap>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    buttons: Option<Res<ButtonInput<MouseButton>>>,
    egui: Option<Res<EguiWantsInput>>,
    quiet: Res<Quiet>,
    mut actions: ResMut<Actions>,
) {
    if quiet.0 {
        *actions = Actions::default();
        return;
    }
    let quiet = egui.is_some_and(|egui| egui.wants_any_keyboard_input());
    *actions = Actions::read(&keymap, keys.as_deref(), buttons.as_deref(), quiet);
}

/// Runs [`resolve_actions`] on `world` now, for tests that run systems one at
/// a time rather than through `PreUpdate`.
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

/// Adds [`KeybindPlugin`] unless something already has. Every plugin that
/// reads [`Actions`] calls it, since those plugins also stand alone in tests.
pub fn ensure(app: &mut App) {
    if !app.is_plugin_added::<KeybindPlugin>() {
        app.add_plugins(KeybindPlugin);
    }
}

fn hints_id() -> egui::Id {
    egui::Id::new("grackle_keymap")
}

/// Copies the keymap into egui's memory for [`hint`], since a menu gets only
/// a `Ui`. Called by the Settings plugin each frame the keymap changes.
pub fn publish(ctx: &egui::Context, keymap: &Keymap) {
    ctx.data_mut(|data| data.insert_temp(hints_id(), Arc::new(keymap.clone())));
}

/// The first chord of `action`, for a menu entry, or `None` with no binding.
pub fn hint(ctx: &egui::Context, action: Action) -> Option<String> {
    let keymap = ctx.data(|data| data.get_temp::<Arc<Keymap>>(hints_id()))?;
    keymap.chords(action).first().map(|chord| chord.label())
}

/// The heading over an action's context on the Shortcuts page.
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

    /// The editor commands among what `held` then `hit` fires.
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

    /// Typing `s` into a viewport must not save.
    #[test]
    fn a_bare_letter_is_not_a_command() {
        for key in [KeyCode::KeyS, KeyCode::KeyZ, KeyCode::KeyN] {
            assert!(commands(&[], key).is_empty(), "{key:?}");
        }
    }

    /// Fails if a crouched player cannot reload, as an exact rule would have it.
    #[test]
    fn a_game_key_fires_with_a_modifier_held() {
        let fired = fired(&[KeyCode::ControlLeft], KeyCode::KeyR);
        assert!(fired.contains(&Action::Reload), "{fired:?}");
    }

    /// Fails if `Shift+B` also seats a bot.
    #[test]
    fn the_longer_chord_shadows_the_shorter() {
        let fired = fired(&[KeyCode::ShiftLeft], KeyCode::KeyB);
        assert!(fired.contains(&Action::RetireBot));
        assert!(!fired.contains(&Action::SeatBot), "{fired:?}");
        assert!(super::tests::fired(&[], KeyCode::KeyB).contains(&Action::SeatBot));
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
    fn while_egui_has_the_keyboard_only_the_mouse_fires() {
        let mut buttons = ButtonInput::<MouseButton>::default();
        buttons.press(MouseButton::Left);
        let actions = Actions::read(&Keymap::default(), Some(&keys(&[KeyCode::ControlLeft], KeyCode::KeyS)), Some(&buttons), true);
        assert!(!actions.just_pressed(Action::Save));
        assert!(actions.just_pressed(Action::Fire));
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
