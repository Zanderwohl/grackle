//! The credits: Grackle's own, then the packs' music and fonts. [`credits`]
//! draws them into any `Ui`; the Settings window's Attribution page is one
//! place it is shown.

use bevy_egui::egui::{self, Ui};

use crate::common::fonts::PackFonts;
use crate::common::music::{MusicAttribution, MusicCredit, MusicLibrary};
use crate::get;

const TITLE_SIZE: f32 = 28.0;
const SECTION_GAP: f32 = 20.0;
const ENTRY_GAP: f32 = 10.0;

pub fn credits(ui: &mut Ui, music: &MusicLibrary, fonts: &PackFonts) {
    ui.heading(egui::RichText::new(get!("credits.title")).size(TITLE_SIZE));
    ui.label(get!("credits.copyright"));
    ui.label(get!("credits.rights"));
    ui.add_space(ENTRY_GAP);
    linked_label(ui, &get!("credits.built_on"));
    linked_label(ui, &get!("credits.bevy_license"));

    ui.add_space(SECTION_GAP);
    ui.heading(get!("credits.music"));
    music_credits(ui, music);

    ui.add_space(SECTION_GAP);
    ui.heading(get!("credits.fonts"));
    font_credits(ui, fonts);
}

fn music_credits(ui: &mut Ui, music: &MusicLibrary) {
    if music.credits.is_empty() {
        ui.weak(get!("credits.no_music"));
        return;
    }
    let (credited, uncredited): (Vec<&MusicCredit>, Vec<&MusicCredit>) =
        music.credits.iter().partition(|credit| credit.attribution.is_some());
    for credit in credited {
        ui.add_space(ENTRY_GAP);
        ui.strong(credit.name());
        match &credit.attribution {
            Some(MusicAttribution::Fields { artist, copyright, license, source, .. }) => {
                if let Some(artist) = artist {
                    ui.label(get!("credits.artist", "artist", artist));
                }
                for line in [copyright, license].into_iter().flatten() {
                    ui.label(line);
                }
                if let Some(source) = source {
                    link_or_label(ui, source, source);
                }
            }
            Some(MusicAttribution::Text(text)) => {
                for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    link_or_label(ui, line, line);
                }
            }
            None => {}
        }
        for track in &credit.tracks {
            ui.weak(track.path.rsplit('/').next().unwrap_or(&track.path));
        }
    }
    if !uncredited.is_empty() {
        ui.add_space(ENTRY_GAP);
        ui.strong(get!("credits.uncredited"));
        for track in uncredited.iter().flat_map(|credit| credit.tracks.iter()) {
            ui.weak(track.path.strip_prefix("music/").unwrap_or(&track.path));
        }
    }
}

fn font_credits(ui: &mut Ui, fonts: &PackFonts) {
    if fonts.fonts.is_empty() {
        ui.weak(get!("credits.no_fonts"));
        return;
    }
    for font in &fonts.fonts {
        let attribution = &font.attribution;
        ui.add_space(ENTRY_GAP);
        ui.strong(&attribution.name);
        if let Some(designer) = &attribution.designer {
            ui.label(get!("credits.designer", "designer", designer));
        }
        for line in [&attribution.copyright, &attribution.license].into_iter().flatten() {
            ui.label(line);
        }
    }
}

/// A web address is a link; anything else is plain text.
fn link_or_label(ui: &mut Ui, text: &str, url: &str) {
    if url.starts_with("https://") || url.starts_with("http://") {
        ui.hyperlink_to(text, url);
    } else {
        ui.label(text);
    }
}

/// One wrapped line in which each `[text](url)` is a link, so a translation
/// can move the link within its sentence.
pub fn linked_label(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for piece in split_links(text) {
            match piece {
                Piece::Text(text) => {
                    ui.label(text);
                }
                Piece::Link { text, url } => link_or_label(ui, text, url),
            }
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Link { text: &'a str, url: &'a str },
}

/// A `[` with no `](…)` after it is text.
fn split_links(mut text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    while !text.is_empty() {
        let link = text.find('[').and_then(|open| {
            let close = open + text[open..].find("](")?;
            let end = close + 2 + text[close + 2..].find(')')?;
            Some((open, &text[open + 1..close], &text[close + 2..end], end + 1))
        });
        let Some((open, label, url, rest)) = link else {
            pieces.push(Piece::Text(text));
            break;
        };
        if open > 0 {
            pieces.push(Piece::Text(&text[..open]));
        }
        pieces.push(Piece::Link { text: label, url });
        text = &text[rest..];
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_split_out_of_their_sentence() {
        assert_eq!(
            split_links("Built on [Bevy](https://bevy.org), by [them](x)."),
            [
                Piece::Text("Built on "),
                Piece::Link { text: "Bevy", url: "https://bevy.org" },
                Piece::Text(", by "),
                Piece::Link { text: "them", url: "x" },
                Piece::Text("."),
            ]
        );
        assert_eq!(split_links("a [b] (c"), [Piece::Text("a [b] (c")]);
        assert_eq!(split_links(""), []);
    }

    /// Breaks if a credits string is missing from the lang file, or drawing them
    /// panics on an empty library.
    #[test]
    fn the_credits_draw_with_nothing_found() {
        for key in [
            "credits.title",
            "credits.copyright",
            "credits.rights",
            "credits.built_on",
            "credits.bevy_license",
            "credits.music",
            "credits.fonts",
            "credits.no_music",
            "credits.no_fonts",
            "credits.uncredited",
        ] {
            assert!(!get!(key).starts_with('<'), "{key} is missing");
        }
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            credits(ui, &MusicLibrary::default(), &PackFonts::default());
        });
        output.textures_delta.clear();
    }
}
