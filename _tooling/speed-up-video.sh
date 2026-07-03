#!/usr/bin/env bash
# speed-up-video.sh — re-encode a video to play faster than normal.
#
# Speeds up both the picture (setpts) and the sound (atempo) by the same factor
# so they stay in sync. atempo raises the tempo WITHOUT raising the pitch, so
# voices don't turn chipmunky. Defaults to 30% faster — the middle of the
# intended 25–35% range.
#
# Usage:
#   _tooling/speed-up-video.sh <input> [output] [percent]
#     input    source video (required)
#     output   destination file (default: <input>-fast.<ext>)
#     percent  how much faster, e.g. 25, 30, 35 (default: 30)
#
# Examples:
#   _tooling/speed-up-video.sh splash.mp4                 # 30% faster → splash-fast.mp4
#   _tooling/speed-up-video.sh splash.mp4 quick.mp4 35    # 35% faster → quick.mp4
#
# 30% faster means playback runs at 1.30×, so a 60s clip becomes ~46s.

set -euo pipefail

INPUT="${1:-}"
OUTPUT="${2:-}"
PERCENT="${3:-30}"

if [ -z "$INPUT" ]; then
    echo "usage: $(basename "$0") <input> [output] [percent]" >&2
    exit 1
fi

command -v ffmpeg  >/dev/null 2>&1 || { echo "error: ffmpeg not found on PATH."  >&2; exit 1; }
command -v ffprobe >/dev/null 2>&1 || { echo "error: ffprobe not found on PATH." >&2; exit 1; }

if [ ! -f "$INPUT" ]; then
    echo "error: input file not found: $INPUT" >&2
    exit 1
fi

# Validate percent is a positive number. atempo handles up to 2.0× in one pass,
# i.e. 100% faster; beyond the intended range we still allow it but cap at 100
# so the single atempo filter stays valid.
if ! printf '%s' "$PERCENT" | grep -Eq '^[0-9]+(\.[0-9]+)?$'; then
    echo "error: percent must be a number (got '$PERCENT')." >&2
    exit 1
fi
if awk "BEGIN{exit !($PERCENT <= 0)}"; then
    echo "error: percent must be greater than 0 (got '$PERCENT')." >&2
    exit 1
fi
if awk "BEGIN{exit !($PERCENT > 100)}"; then
    echo "error: percent above 100 needs chained atempo filters; keep it ≤ 100." >&2
    exit 1
fi
# Gentle nudge if outside the 25–35% the tool is meant for — not an error.
if awk "BEGIN{exit !($PERCENT < 25 || $PERCENT > 35)}"; then
    echo "note: $PERCENT% is outside the intended 25–35% range, continuing anyway." >&2
fi

# Playback factor (1.30 for 30% faster) and its inverse for setpts.
FACTOR="$(awk "BEGIN{printf \"%.5f\", 1 + $PERCENT/100}")"
INV="$(awk "BEGIN{printf \"%.5f\", 1 / (1 + $PERCENT/100)}")"

# Default output next to nothing clever: <stem>-fast.<ext>.
if [ -z "$OUTPUT" ]; then
    ext="${INPUT##*.}"
    stem="${INPUT%.*}"
    if [ "$ext" = "$INPUT" ]; then      # no extension on input
        OUTPUT="${INPUT}-fast"
    else
        OUTPUT="${stem}-fast.${ext}"
    fi
fi

# Pick a video encoder this ffmpeg build can actually use. setpts changes
# timestamps, so the video must be re-encoded — stream copy isn't an option.
# Preference: libx264 (best) → libopenh264 → mpeg4 (native, always compiled in,
# so this never comes up empty — Fedora's ffmpeg-free ships no H.264 encoder).
# Override with FFV_CODEC for e.g. a hardware encoder (h264_nvenc, h264_vaapi).
#
# We *probe* each encoder by encoding one throwaway frame rather than trusting
# `ffmpeg -encoders`: some builds list an encoder (e.g. libopenh264) whose
# backing library is missing, so it appears available but fails at runtime.
encoder_works() {
    ffmpeg -hide_banner -loglevel error -f lavfi -i color=c=black:s=64x64:d=1 \
        -c:v "$1" -frames:v 1 -f null - >/dev/null 2>&1
}

VCODEC="${FFV_CODEC:-}"
if [ -z "$VCODEC" ]; then
    for c in libx264 libopenh264 mpeg4; do
        if encoder_works "$c"; then VCODEC="$c"; break; fi
    done
fi
if [ -z "$VCODEC" ]; then
    echo "error: no usable video encoder found (tried libx264, libopenh264, mpeg4)." >&2
    echo "       Set FFV_CODEC to an encoder listed by 'ffmpeg -encoders'." >&2
    exit 1
fi
if [ "$VCODEC" != "libx264" ]; then
    echo "note: using '$VCODEC' (libx264 unavailable); quality/size may differ." >&2
fi

# Quality flags by encoder family: x264/x265 use CRF; mpeg-family use -q:v;
# everything else (openh264, hardware, VP8/9) is bitrate-based, so match the
# source's video bitrate (falling back to 2.5 Mbit/s).
VQUAL=()
case "$VCODEC" in
    libx264|libx265)
        VQUAL=(-crf 18 -preset medium) ;;
    mpeg4|mpeg2video|msmpeg4*)
        VQUAL=(-q:v 4) ;;                     # 2 (best) … 31 (worst)
    *)
        src_br="$(ffprobe -v error -select_streams v:0 -show_entries stream=bit_rate \
            -of csv=p=0 "$INPUT" 2>/dev/null | head -n1 || true)"
        case "$src_br" in ''|N/A|0) src_br=2500000 ;; esac
        VQUAL=(-b:v "$src_br") ;;
esac

# Does the source have an audio track? If not, skip the atempo filter (it would
# error on a video-only file) and produce a silent, sped-up video.
has_audio="$(ffprobe -v error -select_streams a -show_entries stream=index \
    -of csv=p=0 "$INPUT" 2>/dev/null | head -n1 || true)"

echo "Speeding up '$INPUT' by ${PERCENT}% (${FACTOR}× playback) → '$OUTPUT'  [video: $VCODEC]"

if [ -n "$has_audio" ]; then
    ffmpeg -hide_banner -y -i "$INPUT" \
        -filter:v "setpts=${INV}*PTS" \
        -filter:a "atempo=${FACTOR}" \
        -c:v "$VCODEC" "${VQUAL[@]}" -pix_fmt yuv420p \
        -c:a aac -b:a 192k \
        "$OUTPUT"
else
    ffmpeg -hide_banner -y -i "$INPUT" \
        -filter:v "setpts=${INV}*PTS" \
        -an \
        -c:v "$VCODEC" "${VQUAL[@]}" -pix_fmt yuv420p \
        "$OUTPUT"
fi

echo "Done: $OUTPUT"
