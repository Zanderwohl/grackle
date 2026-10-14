//! Seeds the body animations a pack ships, from the Rust functions they
//! replace.
//!
//! Run from the repo root:
//!
//! ```text
//! cargo run --bin bake_body_clips
//! ```
//!
//! The sampling itself is [`grackle::common::skeleton::seed`], in the library,
//! because the fidelity test bakes a clip too and two samplers would be two
//! that drift. This program is the part that touches the disk, and the whole of
//! what it adds is the decision about **when not to**.
//!
//! # It will not overwrite what you have authored
//!
//! A clip is seeded once and edited thereafter, so a re-bake is almost always
//! somebody running the command again — for a state that has just been added,
//! or because it is in a README — and not somebody asking for three days of
//! timeline work to be thrown away. So each file is compared with what would be
//! written and **left alone if it differs**, with `--force` as the way to say
//! you meant it.
//!
//! The comparison is on the parsed clip rather than on the bytes: saving from
//! the animation editor is `toml::to_string_pretty`, which reorders nothing but
//! does lay a file out its own way, and a file that is textually different and
//! semantically identical has had nothing authored in it.
//!
//! A file that will not parse is also kept, not replaced. It is more likely to
//! be a clip mid-edit with a typo in it than a file anybody wants reverted, and
//! `--force` says so either way.

use std::path::PathBuf;

use clap::Parser;

use grackle::common::clip_file::{CLIPS_DIR, CLIP_EXTENSION};
use grackle::common::skeleton::clip::Clip;
use grackle::common::skeleton::seed::{bake, Seed, SEEDS};

#[derive(Parser)]
#[command(about = "Seed a pack's body animations from the built-in pose functions")]
struct Args {
    /// Which states to seed. Every migrated one, if none are named.
    states: Vec<String>,

    /// Replace files that have been edited since they were seeded.
    #[arg(long)]
    force: bool,

    /// The pack to write into.
    #[arg(long, default_value = "assets/default")]
    pack: PathBuf,
}

/// What became of one state.
enum Outcome {
    Written,
    /// Already says what a bake would say. Nothing to do and nothing at risk.
    Unchanged,
    /// Edited since it was seeded, so it was left where it was.
    Kept(String),
}

fn main() {
    let args = Args::parse();

    let chosen: Vec<Seed> = if args.states.is_empty() {
        SEEDS.to_vec()
    } else {
        let mut chosen = Vec::new();
        for name in &args.states {
            match SEEDS.iter().find(|seed| seed.state.clip_name() == name) {
                Some(seed) => chosen.push(*seed),
                None => {
                    eprintln!(
                        "'{name}' is not a migrated state. Try one of: {}",
                        SEEDS
                            .iter()
                            .map(|seed| seed.state.clip_name())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    std::process::exit(2);
                }
            }
        }
        chosen
    };

    let directory = args.pack.join(CLIPS_DIR);
    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|e| panic!("{}: {e}", directory.display()));

    let mut kept = 0;
    for seed in chosen {
        let clip = bake(seed);
        let path = directory.join(format!("{}.{CLIP_EXTENSION}", seed.state.clip_name()));
        let text = toml::to_string_pretty(&clip).expect("a clip should serialise");

        match outcome(&path, &clip, &text, args.force) {
            Outcome::Written => {
                std::fs::write(&path, &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                println!(
                    "{} — {} track(s), {} key(s)",
                    path.display(),
                    clip.tracks.len(),
                    clip.tracks
                        .iter()
                        .map(|track| track.channel.key_count())
                        .sum::<usize>(),
                );
            }
            Outcome::Unchanged => {
                println!("{} — already a faithful seeding; left alone", path.display());
            }
            Outcome::Kept(why) => {
                println!("{} — kept: {why}", path.display());
                kept += 1;
            }
        }
    }

    if kept > 0 {
        // Non-zero, so a script that bakes as a build step notices rather than
        // carrying on against files it did not write.
        eprintln!(
            "\n{kept} file(s) kept. Pass --force to replace them with a fresh \
             seeding, losing whatever has been authored in them.",
        );
        std::process::exit(1);
    }
}

/// What to do about one file, given what a bake would put there.
fn outcome(path: &std::path::Path, clip: &Clip, text: &str, force: bool) -> Outcome {
    let Ok(existing) = std::fs::read_to_string(path) else {
        // Not there, or unreadable. Either way there is nothing to lose.
        return Outcome::Written;
    };
    if force {
        return Outcome::Written;
    }
    // Identical bytes needs no parse, and is the usual answer for a pack
    // nobody has touched.
    if existing == text {
        return Outcome::Unchanged;
    }
    match toml::from_str::<Clip>(&existing) {
        Ok(mut found) => {
            found.normalise();
            let mut fresh = clip.clone();
            fresh.normalise();
            if found == fresh {
                Outcome::Unchanged
            } else {
                Outcome::Kept("edited since it was seeded".into())
            }
        }
        Err(why) => Outcome::Kept(format!("will not parse ({why}), so it is not ours to replace")),
    }
}
