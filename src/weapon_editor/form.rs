//! One form for every spec, walked by reflection, so a field added to a spec is in the editor
//! the day it is written. A struct is a row per field, an `Option` a checkbox, any other enum a
//! combo with the chosen variant's fields beneath. Units come from the field's `Unit` attribute.
//!
//! Switching a variant or clearing an option keeps what it held in [`FormMemory`] for the rest
//! of the session, so trying a projectile and going back to hitscan loses nothing. A variant never
//! held starts from its fields' `Default`s, read from the type registry.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::reflect::enums::{DynamicEnum, DynamicVariant, Enum, EnumInfo, VariantInfo};
use bevy::reflect::structs::{DynamicStruct, Struct};
use bevy::reflect::tuple::DynamicTuple;
use bevy::reflect::{PartialReflect, ReflectMut, TypeInfo, TypeRegistry};
use bevy_egui::egui;

use crate::common::weapon::Unit;
use crate::get;

const LABEL_WIDTH: f32 = 150.0;

/// Keyed by the field's path from the form's root and the variant it held.
#[derive(Resource, Default)]
pub struct FormMemory(HashMap<(String, String), Box<dyn PartialReflect>>);

pub struct Form<'a> {
    pub registry: &'a TypeRegistry,
    pub memory: &'a mut FormMemory,
}

pub fn field_key(name: &str) -> String {
    format!("weapon_editor.fields.{name}")
}

pub fn variant_key(name: &str) -> String {
    format!("weapon_editor.variants.{}", snake(name))
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

pub fn unit_key(unit: Unit) -> &'static str {
    match unit {
        Unit::Meters => "weapon_editor.units.meters",
        Unit::Seconds => "weapon_editor.units.seconds",
        Unit::MetersPerSecond => "weapon_editor.units.meters_per_second",
        Unit::Radians => "weapon_editor.units.degrees",
        Unit::RadiansPerSecond => "weapon_editor.units.radians_per_second",
        Unit::PerSecond => "weapon_editor.units.per_second",
        Unit::Times => "weapon_editor.units.times",
        Unit::Fraction => "weapon_editor.units.fraction",
        Unit::Ratio => "weapon_editor.units.ratio",
        Unit::MetersPerSecondPerDamage => "weapon_editor.units.meters_per_second_per_damage",
    }
}

pub fn is_option(info: &EnumInfo) -> bool {
    info.type_path().starts_with("core::option::Option")
}

/// Whether anything changed.
pub fn edit_struct(ui: &mut egui::Ui, value: &mut dyn Struct, path: &str, form: &mut Form) -> bool {
    let info = value.get_represented_struct_info();
    let mut changed = false;
    for index in 0..value.field_len() {
        let Some(name) = value.name_at(index).map(str::to_owned) else { continue };
        let unit = info.and_then(|info| info.field_at(index)).and_then(|field| field.get_attribute::<Unit>()).copied();
        let Some(field) = value.field_at_mut(index) else { continue };
        changed |= edit_field(ui, &get!(field_key(&name)), field, unit, &format!("{path}.{name}"), form);
    }
    changed
}

fn edit_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut dyn PartialReflect,
    unit: Option<Unit>,
    path: &str,
    form: &mut Form,
) -> bool {
    match value.reflect_mut() {
        ReflectMut::Opaque(value) => row(ui, label, |ui| number(ui, value, unit)),
        ReflectMut::Struct(value) => {
            ui.label(label);
            ui.indent(path, |ui| edit_struct(ui, value, path, form)).inner
        }
        ReflectMut::Enum(value) => match value.get_represented_enum_info() {
            Some(info) if is_option(info) => option(ui, label, value, info, unit, path, form),
            Some(info) => enumeration(ui, label, value, info, path, form),
            None => false,
        },
        _ => {
            row(ui, label, |ui| {
                ui.weak("…");
                false
            })
        }
    }
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(label).truncate()),
        );
        add(ui)
    })
    .inner
}

/// `clamp_existing_to_range(false)`: a value the file holds outside the range is shown as it is,
/// not quietly rewritten by being looked at.
fn number(ui: &mut egui::Ui, value: &mut dyn PartialReflect, unit: Option<Unit>) -> bool {
    let suffix = unit.map(|unit| get!(unit_key(unit))).unwrap_or_default();
    if let Some(value) = value.try_downcast_mut::<f32>() {
        if unit == Some(Unit::Radians) {
            let mut degrees = value.to_degrees();
            let changed = ui
                .add(
                    egui::DragValue::new(&mut degrees)
                        .speed(0.5)
                        .range(0.0..=180.0)
                        .clamp_existing_to_range(false)
                        .suffix(suffix),
                )
                .changed();
            if changed {
                *value = degrees.to_radians();
            }
            return changed;
        }
        let (speed, most) = match unit {
            Some(Unit::Fraction) => (0.01, 1.0),
            Some(Unit::Seconds | Unit::PerSecond | Unit::Times | Unit::MetersPerSecondPerDamage) => (0.01, f32::INFINITY),
            Some(Unit::Meters) => (0.05, f32::INFINITY),
            _ => (0.1, f32::INFINITY),
        };
        return ui
            .add(
                egui::DragValue::new(value)
                    .speed(speed)
                    .range(0.0..=most)
                    .clamp_existing_to_range(false)
                    .suffix(suffix),
            )
            .changed();
    }
    if let Some(value) = value.try_downcast_mut::<u32>() {
        return ui.add(egui::DragValue::new(value).suffix(suffix)).changed();
    }
    if let Some(value) = value.try_downcast_mut::<u8>() {
        return ui.add(egui::DragValue::new(value).suffix(suffix)).changed();
    }
    if let Some(value) = value.try_downcast_mut::<bool>() {
        return ui.checkbox(value, "").changed();
    }
    if let Some(value) = value.try_downcast_mut::<String>() {
        return ui.text_edit_singleline(value).changed();
    }
    ui.weak("…");
    false
}

fn option(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut dyn Enum,
    info: &EnumInfo,
    unit: Option<Unit>,
    path: &str,
    form: &mut Form,
) -> bool {
    let was_on = value.variant_name() == "Some";
    let mut on = was_on;
    let inline = value.field_at(0).is_some_and(|inner| !matches!(inner.reflect_ref(), bevy::reflect::ReflectRef::Struct(_)));
    let mut changed = ui
        .horizontal(|ui| {
            let mut changed = false;
            ui.allocate_ui_with_layout(
                egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| ui.checkbox(&mut on, label),
            );
            if was_on && inline && let Some(inner) = value.field_at_mut(0) {
                changed |= number(ui, inner, unit);
            }
            changed
        })
        .inner;

    if on != was_on {
        switch(value, info, if on { "Some" } else { "None" }, path, form);
        changed = true;
    }
    if on && !inline && let Some(ReflectMut::Struct(inner)) = value.field_at_mut(0).map(PartialReflect::reflect_mut) {
        changed |= ui.indent(path, |ui| edit_struct(ui, inner, path, form)).inner;
    }
    changed
}

fn enumeration(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut dyn Enum,
    info: &'static EnumInfo,
    path: &str,
    form: &mut Form,
) -> bool {
    let current = value.variant_name().to_owned();
    let mut chosen = None;
    row(ui, label, |ui| {
        egui::ComboBox::from_id_salt(path).selected_text(get!(variant_key(&current))).show_ui(ui, |ui| {
            for name in info.variant_names() {
                if ui.selectable_label(*name == current, get!(variant_key(name))).clicked() && *name != current {
                    chosen = Some(*name);
                }
            }
        });
        false
    });
    let mut changed = false;
    if let Some(name) = chosen {
        switch(value, info, name, path, form);
        changed = true;
    }

    let variant = info.variant(value.variant_name());
    let path = format!("{path}.{}", value.variant_name());
    ui.indent(&path, |ui| {
        for index in 0..value.field_len() {
            let (name, unit) = match variant {
                Some(VariantInfo::Struct(fields)) => {
                    let field = fields.field_at(index);
                    (field.map(|field| field.name()), field.and_then(|field| field.get_attribute::<Unit>()).copied())
                }
                Some(VariantInfo::Tuple(fields)) => {
                    (None, fields.field_at(index).and_then(|field| field.get_attribute::<Unit>()).copied())
                }
                _ => (None, None),
            };
            let Some(field) = value.field_at_mut(index) else { continue };
            changed |= match (name, field.reflect_mut()) {
                // A newtype variant (`Hitscan(HitscanSpec)`) shows its spec's fields as its own.
                (None, ReflectMut::Struct(inner)) => edit_struct(ui, inner, &path, form),
                (name, _) => {
                    let label = name.map(|name| get!(field_key(name))).unwrap_or_default();
                    let field_path = format!("{path}.{}", name.unwrap_or("0"));
                    edit_field(ui, &label, field, unit, &field_path, form)
                }
            };
        }
    })
    .inner;
    changed
}

/// Puts what `value` held into memory and takes `to` out of it, or builds it from defaults.
fn switch(value: &mut dyn Enum, info: &EnumInfo, to: &str, path: &str, form: &mut Form) {
    if let Ok(held) = value.to_dynamic() {
        form.memory.0.insert((path.to_owned(), value.variant_name().to_owned()), held);
    }
    let Some(target) = form
        .memory
        .0
        .remove(&(path.to_owned(), to.to_owned()))
        .or_else(|| default_variant(form.registry, info, to))
    else {
        warn!("No default for {to} in {}", info.type_path());
        return;
    };
    if let Err(e) = value.try_apply(target.as_ref()) {
        warn!("Could not switch {} to {to}: {e}", info.type_path());
    }
}

pub fn default_variant(registry: &TypeRegistry, info: &EnumInfo, name: &str) -> Option<Box<dyn PartialReflect>> {
    let variant = match info.variant(name)? {
        VariantInfo::Unit(_) => DynamicVariant::Unit,
        VariantInfo::Tuple(fields) => {
            let mut tuple = DynamicTuple::default();
            for field in fields.iter() {
                tuple.insert_boxed(default_of(registry, field.type_id())?);
            }
            DynamicVariant::Tuple(tuple)
        }
        VariantInfo::Struct(fields) => {
            let mut fields_of = DynamicStruct::default();
            for field in fields.iter() {
                fields_of.insert_boxed(field.name(), default_of(registry, field.type_id())?);
            }
            DynamicVariant::Struct(fields_of)
        }
    };
    Some(Box::new(DynamicEnum::new(name, variant)))
}

fn default_of(registry: &TypeRegistry, type_id: std::any::TypeId) -> Option<Box<dyn PartialReflect>> {
    if let Some(default) = registry.get_type_data::<ReflectDefault>(type_id) {
        return Some(default.default().into_partial_reflect());
    }
    match registry.get_type_info(type_id)? {
        TypeInfo::Enum(info) if is_option(info) => default_variant(registry, info, "None"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use bevy::reflect::{Typed, TypeInfo};

    use super::*;
    use crate::common::weapon::{Cadence, Magazine, Mounted, WeaponAction};

    fn registry() -> TypeRegistry {
        let mut registry = TypeRegistry::default();
        registry.register::<Mounted>();
        registry.register::<Magazine>();
        registry
    }

    /// Every field and variant the form can reach, and whether each `f32` states its unit.
    fn walk(info: &'static TypeInfo, fields: &mut Vec<String>, variants: &mut Vec<String>, unitless: &mut Vec<String>) {
        let visit = |name: Option<&str>, field_info: Option<&'static TypeInfo>, unit: Option<&Unit>, fields: &mut Vec<String>, variants: &mut Vec<String>, unitless: &mut Vec<String>| {
            if let Some(name) = name {
                fields.push(name.to_owned());
            }
            let Some(field_info) = field_info else { return };
            let numeric = |path: &str| path == "f32";
            let inner_path = match field_info {
                TypeInfo::Enum(e) if is_option(e) => match e.variant("Some") {
                    Some(VariantInfo::Tuple(t)) => t.field_at(0).map(|f| f.type_path()).unwrap_or(""),
                    _ => "",
                },
                other => other.type_path(),
            };
            if numeric(inner_path) && unit.is_none() {
                unitless.push(name.unwrap_or("?").to_owned());
            }
            walk(field_info, fields, variants, unitless);
        };
        match info {
            TypeInfo::Struct(s) => {
                for f in s.iter() {
                    visit(Some(f.name()), f.type_info(), f.get_attribute::<Unit>(), fields, variants, unitless);
                }
            }
            TypeInfo::Enum(e) if is_option(e) => {
                if let Some(VariantInfo::Tuple(t)) = e.variant("Some") {
                    for f in t.iter() {
                        if let Some(inner) = f.type_info() {
                            walk(inner, fields, variants, unitless);
                        }
                    }
                }
            }
            TypeInfo::Enum(e) => {
                for v in e.iter() {
                    variants.push(v.name().to_owned());
                    match v {
                        VariantInfo::Struct(s) => {
                            for f in s.iter() {
                                visit(Some(f.name()), f.type_info(), f.get_attribute::<Unit>(), fields, variants, unitless);
                            }
                        }
                        VariantInfo::Tuple(t) => {
                            for f in t.iter() {
                                visit(None, f.type_info(), f.get_attribute::<Unit>(), fields, variants, unitless);
                            }
                        }
                        VariantInfo::Unit(_) => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn reachable() -> (Vec<String>, Vec<String>, Vec<String>) {
        let (mut fields, mut variants, mut unitless) = (vec![], vec![], vec![]);
        walk(Mounted::type_info(), &mut fields, &mut variants, &mut unitless);
        walk(Magazine::type_info(), &mut fields, &mut variants, &mut unitless);
        (fields, variants, unitless)
    }

    /// Read from the file because the language in force is global state other tests move.
    #[test]
    fn every_weapon_field_has_a_label() {
        let text = std::fs::read_to_string("assets/default/lang/en-US.toml").expect("en-US ships");
        let table: toml::Value = toml::from_str(&text).expect("en-US is valid TOML");
        let has = |key: &str| {
            let mut at = &table;
            for part in key.split('.') {
                match at.get(part) {
                    Some(next) => at = next,
                    None => return false,
                }
            }
            at.as_str().is_some()
        };
        let (fields, variants, _) = reachable();
        let mut missing: Vec<String> = fields.iter().map(|f| field_key(f)).chain(variants.iter().map(|v| variant_key(v))).filter(|key| !has(key)).collect();
        for unit in [Unit::Meters, Unit::Seconds, Unit::MetersPerSecond, Unit::Radians, Unit::RadiansPerSecond, Unit::PerSecond, Unit::Times, Unit::Fraction, Unit::Ratio, Unit::MetersPerSecondPerDamage] {
            if !has(unit_key(unit)) {
                missing.push(unit_key(unit).to_owned());
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "en-US has no label for {missing:?}");
    }

    #[test]
    fn every_number_states_its_unit() {
        let (_, _, unitless) = reachable();
        assert!(unitless.is_empty(), "no `Unit` on {unitless:?}");
    }

    /// Fails if a variant cannot be built from defaults, which would leave its combo entry dead.
    #[test]
    fn every_variant_can_be_switched_to() {
        let registry = registry();
        for info in [WeaponAction::type_info(), Cadence::type_info(), crate::common::weapon::Cost::type_info()] {
            let TypeInfo::Enum(info) = info else { panic!("not an enum") };
            for name in info.variant_names() {
                let mut mounted = Mounted::EMPTY;
                let built = default_variant(&registry, info, name).unwrap_or_else(|| panic!("no default for {name}"));
                let target: &mut dyn PartialReflect = match info.type_path() {
                    p if p.ends_with("WeaponAction") => &mut mounted.action,
                    p if p.ends_with("Cadence") => &mut mounted.cadence,
                    _ => &mut mounted.cost,
                };
                target.try_apply(built.as_ref()).unwrap_or_else(|e| panic!("{name}: {e}"));
                let ReflectMut::Enum(after) = target.reflect_mut() else { panic!() };
                assert_eq!(after.variant_name(), *name);
            }
        }
    }

    #[test]
    fn switching_back_restores_what_was_there() {
        let registry = registry();
        let mut memory = FormMemory::default();
        let mut form = Form { registry: &registry, memory: &mut memory };
        let mut action = WeaponAction::Hitscan(crate::common::weapon::HitscanSpec { damage: 42, ..default() });
        let TypeInfo::Enum(info) = WeaponAction::type_info() else { panic!() };
        let ReflectMut::Enum(value) = action.reflect_mut() else { panic!() };
        switch(value, info, "Projectile", "primary.action", &mut form);
        assert!(matches!(action, WeaponAction::Projectile(_)));
        let ReflectMut::Enum(value) = action.reflect_mut() else { panic!() };
        switch(value, info, "Hitscan", "primary.action", &mut form);
        assert!(matches!(action, WeaponAction::Hitscan(spec) if spec.damage == 42), "{action:?}");
    }
}
