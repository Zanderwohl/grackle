//! Seeds the body animations a pack ships, from the Rust functions they
//! replace. Run from the repo root:
//!
//! ```text
//! cargo run --bin bake_body_clips
//! ```
//!
//! The sampling is [`grackle::common::skeleton::seed`], shared with the
//! fidelity test. This program only decides when not to write: a file that
//! differs from a fresh bake (compared parsed, since the editor lays files out
//! its own way) or will not parse is kept unless `--force`.

use std::path::PathBuf;

use clap::Parser;

use grackle::common::clip_file::{rig_clips_dir, CLIPS_DIR, CLIP_EXTENSION};
use grackle::common::skeleton::clip::StateAnimation;
use grackle::common::skeleton::seed::{bake, raptor_written, Seed, SEEDS, WRITTEN};

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

enum Outcome {
    Written,
    Unchanged,
    Kept(String),
}

fn main() {
    let args = Args::parse();

    // `raptor/flinch_back` names one of the raptor's.
    let written: Vec<(String, StateAnimation)> = WRITTEN
        .iter()
        .map(|(name, clip)| (name.to_string(), clip()))
        .chain(raptor_written().into_iter().map(|(name, clip)| (format!("raptor/{name}"), clip)))
        .filter(|(name, _)| args.states.is_empty() || args.states.iter().any(|wanted| wanted == name))
        .map(|(name, clip)| (name, StateAnimation::Single(clip)))
        .collect();
    let written_names: Vec<String> = WRITTEN
        .iter()
        .map(|(name, _)| name.to_string())
        .chain(grackle::common::skeleton::raptor_seed::FLINCHES.iter().map(|name| format!("raptor/{name}")))
        .collect();
    let states: Vec<String> =
        args.states.iter().filter(|wanted| !written_names.contains(wanted)).cloned().collect();

    let chosen: Vec<Seed> = if args.states.is_empty() {
        SEEDS.to_vec()
    } else if states.is_empty() {
        Vec::new()
    } else {
        let mut chosen = Vec::new();
        for name in &states {
            // `idle` is every rig's idle; `raptor/idle` is one.
            let named = |seed: &&Seed| seed.file_name() == name || format!("{}/{}", seed.rig, seed.file_name()) == *name;
            match SEEDS.iter().filter(named).copied().collect::<Vec<_>>() {
                found if !found.is_empty() => chosen.extend(found),
                _ => {
                    eprintln!(
                        "'{name}' is not a migrated state or a written clip. Try one of: {}",
                        SEEDS
                            .iter()
                            .map(|seed| seed.file_name())
                            .chain(written_names.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    std::process::exit(2);
                }
            }
        }
        chosen
    };

    let seeded = chosen.into_iter().map(|seed| {
        let directory = match seed.rig {
            grackle::common::skeleton::rig::HUMANOID => args.pack.join(CLIPS_DIR),
            rig => args.pack.join(rig_clips_dir(rig)),
        };
        (directory, seed.file_name().to_string(), bake(seed))
    });
    let written = written.into_iter().map(|(name, clip)| match name.split_once('/') {
        Some((rig, name)) => (args.pack.join(rig_clips_dir(rig)), name.to_string(), clip),
        None => (args.pack.join(CLIPS_DIR), name, clip),
    });

    let mut kept = 0;
    for (directory, name, clip) in seeded.chain(written) {
        std::fs::create_dir_all(&directory)
            .unwrap_or_else(|e| panic!("{}: {e}", directory.display()));
        let path = directory.join(format!("{name}.{CLIP_EXTENSION}"));
        let text = clip.to_toml().expect("a clip should serialize");

        match outcome(&path, &clip, &text, args.force) {
            Outcome::Written => {
                std::fs::write(&path, &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                println!(
                    "{} — {} track(s), {} key(s)",
                    path.display(),
                    clip.clips().iter().map(|clip| clip.tracks.len()).sum::<usize>(),
                    clip.clips()
                        .iter()
                        .flat_map(|clip| &clip.tracks)
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
        // Non-zero so a build script notices files it did not write.
        eprintln!(
            "\n{kept} file(s) kept. Pass --force to replace them with a fresh \
             seeding, losing whatever has been authored in them.",
        );
        std::process::exit(1);
    }
}

fn outcome(path: &std::path::Path, clip: &StateAnimation, text: &str, force: bool) -> Outcome {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return Outcome::Written;
    };
    if force {
        return Outcome::Written;
    }
    if existing == text {
        return Outcome::Unchanged;
    }
    match StateAnimation::from_toml(&existing) {
        Ok(found) => {
            let mut fresh = clip.clone();
            fresh.normalize();
            if found == fresh {
                Outcome::Unchanged
            } else {
                Outcome::Kept("edited since it was seeded".into())
            }
        }
        Err(why) => Outcome::Kept(format!("will not parse ({why}), so it is not ours to replace")),
    }
}
