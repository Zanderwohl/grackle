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

use grackle::common::clip_file::{CLIPS_DIR, CLIP_EXTENSION};
use grackle::common::skeleton::clip::StateAnimation;
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

enum Outcome {
    Written,
    Unchanged,
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
        let text = clip.to_toml().expect("a clip should serialise");

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
