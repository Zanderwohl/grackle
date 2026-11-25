# Sounds

A pack's sounds and music are played as **Ogg Vorbis**. Bevy decodes Vorbis with its default
audio feature, and its decoder is pure Rust, so it works in a browser build. Vorbis also loops
without a gap, whereas an MP3 encoder pads the start and end of a file, which puts a click in a
looping sound.

Sounds live at `assets/{pack}/sounds/{freesound id}/`. Each folder holds the original file, the
`.ogg` encoded from it, and an `attribution.txt` with the source's credit line.

## Adding a sound

1. Make the folder and put the original and its `attribution.txt` in it.
2. Install the encoders once: `brew install ffmpeg vorbis-tools`. The script needs `oggenc` from
   vorbis-tools, because ffmpeg's built-in Vorbis encoder is experimental and low quality.
3. From the repo root, run:

   ```bash
   scripts/encode_sounds.sh
   ```

   With no arguments it goes through every pack's `sounds/` folder. You can also pass folders or
   files to encode just those. It skips any source that already has an `.ogg`, so running it
   again is safe.
4. Commit the `.ogg`, the `attribution.txt`, any `.gitignore` the script wrote, and the original
   if it is 1 MB or smaller.

## What the script does

- **Quality.** It encodes at Vorbis quality 9, which is about 320 kbps for stereo and 160 kbps
  for mono. For an MP3 source it uses roughly the source's own bit rate (quality ≈ kbps / 32),
  because more bits than the MP3 had only make the file bigger.
- **Sample rate.** A source above 48 kHz is resampled to 48 kHz. Channels are left as they are.
- **Choice of source.** If a folder holds both an MP3 and a lossless file (WAV, AIFF or FLAC)
  with the same name, it encodes from the lossless one.
- **Big originals.** It adds any original over 1 MB to the `.gitignore` in its folder, so the
  original stays on your disk but is not committed. Only the `.ogg` ships.

## Format preference in code

Where one folder holds a sound in several formats with the same name (`shot.wav` beside
`shot.ogg`), `music::one_format_each` keeps only the one earliest in `SOUND_EXTENSIONS`, which
lists Ogg first ([`src/common/music.rs`](../src/common/music.rs)). The music library uses it
today. Code that loads files from `sounds/` should use it too, so an original left beside its
`.ogg` is never played.
