use grackle::{get, startup};
use grackle::common;

use bevy::prelude::*;
use bevy::window::{ExitCondition, PresentMode};
use bevy_egui::{egui, EguiPrimaryContextPass, EguiContexts, EguiPlugin};
use bevy_egui::egui::{Frame, ScrollArea, Sense, UiBuilder};
use grackle::common::item::item::Item;
use grackle::unlock::{unlock, UnlockProblem};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let editor_params = startup::EditorParams::new()
        .unwrap_or_else(|message| {
            eprintln!("Editor Startup Error:\n{}", message);
            std::process::exit(1);
        });
    common::lang::change_lang(&editor_params.lang)
        .unwrap_or_else(|message| {
            eprintln!("Language map error:\n{}", message);
            std::process::exit(1);
        });
    
    App::new()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: get!("crate_drop.title"),
                    name: Some("grackle-drop-tester.app".to_owned()),
                    present_mode: PresentMode::AutoVsync,
                    prevent_default_event_handling: true,
                    visible: true,
                    ..default()
                }),
                exit_condition: ExitCondition::OnPrimaryClosed,
                close_when_requested: true,
            })
        )
        .add_plugins((
            EguiPlugin { enable_multipass_for_primary_context: true },
        ))
        .init_resource::<State>()
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
    ;
    Ok(())
}

#[derive(Resource)]
struct State {
    items: Vec<Item>,
    selected_item: Option<usize>,
    series: u32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected_item: None,
            series: 1,
        }
    }
}

fn ui(
    mut state: ResMut<State>,
    mut contexts: EguiContexts,
) {
    let ctx = contexts.try_ctx_mut();
    if ctx.is_none() {
        return;
    }
    let ctx = ctx.unwrap();
    
    egui::SidePanel::left("left_panel").show(ctx, |ui| {
        ui.set_min_width(200.0);

        let half_height = ui.available_height() / 2.0;

        // Top half
        ui.vertical(|ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(half_height);
            ui.set_max_height(half_height);
            ScrollArea::vertical().show(ui, |ui| {
                ui.heading(get!("crate_drop.controls.title"));
                ui.add_space(32.0);

                if ui.button(get!("crate_drop.controls.new")).clicked() {
                    match unlock(state.series) {
                        Ok(item) => state.items.push(item),
                        Err(err) => {
                            error!("{}", err);
                        }
                    }
                }

                ui.add_space(32.0);
                ui.add(egui::Slider::new(&mut state.series, 0..=5).text(get!("crate_drop.controls.series")));
            });
        });

        ui.separator();

        // Bottom half
        ScrollArea::vertical().show(ui, |ui| {
            // This should be the vertical halfway point.
            ui.heading(get!("crate_drop.details.title"));
            match state.selected_item {
                None => {
                    ui.label(get!("crate_drop.details.none"));
                }
                Some(selected_idx) => {
                    let item = &state.items[selected_idx];

                    ui.label(item.display_name());
                    // get!("item.trade_restriction")

                    if item.trade_restriction || item.crafting_restriction {
                        ui.add_space(16.0);
                    }
                    if item.trade_restriction {
                        ui.label(get!("item.trade_restriction"));
                    }
                    if item.crafting_restriction {
                        ui.label(get!("item.crafting_restriction"));
                    }
                    if let Some(particle_effect) = &item.particle_effect {
                        ui.add_space(16.0);
                        ui.heading(get!("item.particle_effect_adj"));
                        ui.label(get!("item.particle_effect_info"));
                        ui.label(particle_effect.name());
                    }
                    if let Some(stat_tracker) = &item.stat_tracker {
                        ui.add_space(16.0);
                        ui.heading(get!("item.stat_tracker_adj"));
                        ui.label(get!("item.stat_tracker_info"));
                        ui.label(stat_tracker.tracks_list());
                    }
                }
            }
        });
    });
    
    egui::SidePanel::right("right_panel").show(ctx, |ui| {
        ui.heading(get!("crate_drop.history.title"));

        ScrollArea::vertical().show(ui, |ui| {
            let mut new_selected = None;
            
            for (idx, item) in state.items.iter().enumerate() {
                ui.set_min_width(200.0);
                ui.vertical(|ui| {
                    let response = ui
                        .scope_builder(
                            UiBuilder::new()
                                .id_salt(format!("item_container_{}", idx + 1))
                                .sense(Sense::click())
                            ,
                            |ui| {
                                let response = ui.response();
                                let is_selected = state.selected_item == Some(idx);
                                let visuals = ui.style().interact_selectable(&response, is_selected);

                                let inactive_visuals = &ui.style().visuals.widgets.inactive;
                                let mut stroke = visuals.bg_stroke;
                                stroke.width = inactive_visuals.bg_stroke.width;

                                Frame::canvas(ui.style())
                                    .fill(visuals.bg_fill)
                                    .stroke(stroke) 
                                    .inner_margin(ui.spacing().menu_margin)
                                    .show(ui, |ui| {
                                        let was_selectable = ui.style_mut().interaction.selectable_labels;
                                        ui.style_mut().interaction.selectable_labels = false;

                                        ui.set_min_width(ui.available_width());
                                        ui.label(item.display_name());

                                        ui.add_space(32.0);
                                        if let Some(particle_effect) = &item.particle_effect {
                                            ui.label(particle_effect.name());
                                        }
                                        if let Some(stat_tracker) = &item.stat_tracker {
                                            ui.label(get!("stat_tracker.tracks", "list", stat_tracker.tracks_list()));
                                        }

                                        ui.style_mut().interaction.selectable_labels = was_selectable;
                                    });
                            })
                        .response;

                    if response.clicked() {
                        new_selected = Some(idx);
                    }
                });
            }
            
            if let Some(new_selected) = new_selected {
                state.selected_item = Some(new_selected);
            }
        });
    });
}
