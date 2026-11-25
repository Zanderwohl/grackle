#!/bin/bash
# Encodes every sound under the given folders (default: every pack's sounds/) to Ogg Vorbis
# beside its source, and names a source over 1 MB in its folder's .gitignore.
# Needs ffmpeg and oggenc (Homebrew: ffmpeg, vorbis-tools). Run from the repo root.
# See documentation/sounds.md.
set -euo pipefail

BIG=$((1024 * 1024))
[ $# -eq 0 ] && set -- assets/*/sounds

find "$@" -type f \( -iname '*.wav' -o -iname '*.mp3' -o -iname '*.aif' -o -iname '*.aiff' -o -iname '*.flac' \) -print0 |
while IFS= read -r -d '' src; do
  stem="${src%.*}"; out="$stem.ogg"; ext="${src##*.}"
  lossy=false; case "$ext" in mp3|MP3) lossy=true;; esac
  # Encode from a lossless source when the stem has one.
  if $lossy; then for l in wav aif aiff flac; do [ -e "$stem.$l" ] && continue 2; done; fi

  if [ ! -e "$out" ]; then
    # q9 is about 320 kbps stereo. A lossy source is not worth more bits than it had: q ~ kbps / 32.
    quality=9
    if $lossy; then
      kbps=$(( $(ffprobe -v error -select_streams a:0 -show_entries stream=bit_rate -of csv=p=0 "$src") / 1000 ))
      quality=$(( kbps / 32 )); [ "$quality" -gt 9 ] && quality=9
    fi
    rate=$(ffprobe -v error -select_streams a:0 -show_entries stream=sample_rate -of csv=p=0 "$src")
    resample=(); [ "$rate" -gt 48000 ] && resample=(-ar 48000)
    # -nostdin: ffmpeg otherwise reads the file list this loop is reading.
    ffmpeg -nostdin -v error -i "$src" -map 0:a:0 ${resample[@]+"${resample[@]}"} -c:a pcm_s24le -f wav - |
      oggenc --quiet --ignorelength -q "$quality" -o "$out" - 2>/dev/null
    echo "$(wc -c < "$src" | tr -d ' ') -> $(wc -c < "$out" | tr -d ' ')  $out"
  fi

  if [ "$(wc -c < "$src")" -gt "$BIG" ]; then
    ignore="$(dirname "$src")/.gitignore"; name="$(basename "$src")"
    grep -qxF "$name" "$ignore" 2>/dev/null || { echo "$name" >> "$ignore"; echo "ignored $src"; }
  fi
done
