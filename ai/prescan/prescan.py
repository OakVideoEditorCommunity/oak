#!/usr/bin/env python3
"""prescan.py - forensic pre-screen for media files (evidence layer).

Runs standalone against a media file: it never touches the editing engine,
the SDK, or any project state. All evidence comes from ffmpeg/ffprobe over
the file itself (FFmpeg fallback path).

Two similarity channels:

  A  scene     ffmpeg lavfi scene score per frame (hard-cut sensitive).
  B  phash     32x32 gray -> DCT -> 8x8 low-frequency band -> 64-bit hash;
               hamming distance between adjacent samples, normalized to
               [0, 1] by dividing by 63 (robust to slow camera motion).

Dual-threshold hysteresis:

  scene >= high                          -> hard cut candidate
  scene >= low  or  phash local spike    -> soft candidate (action/transition band)

A phash "spike" is a distance that exceeds the local baseline (median of the
+/-1 s neighborhood) by at least --phash-margin bits; this keeps footage whose
distance is uniformly high (animation) from producing spurious candidates.

Time discipline follows the plan's schema rule: every timecode in the output
is an exact rational {num, den} plus an integer frame number on the frame grid
of the container's average frame rate. No bare floating-point timecodes.

A shot with no frame-to-frame change (solid colour, freeze frame, still image)
is valid input and yields an empty cut list (`cuts: []`,
`scan.scene_frames_scored: 0`, exit 0) — never a tool failure. Only decode
failures (non-zero ffmpeg exit, unreadable stream) and missing tools are
errors.

Usage:
    python3 prescan.py INPUT [--out PATH] [--high F] [--low F]
                       [--sample-fps F] [--phash-margin N]
                       [--merge-window SEC] [--compact]

Exit codes: 0 success, 2 input/parameter error (message on stderr).
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
from fractions import Fraction
from pathlib import Path
from typing import Any

SCHEMA = "oak.prescan.cutlist.v1"
FFMPEG = "ffmpeg"
FFPROBE = "ffprobe"

# Defaults (see README "Thresholds" for tuning guidance).
DEFAULT_HIGH = 0.40
DEFAULT_LOW = 0.15
DEFAULT_SAMPLE_FPS = 2.0
DEFAULT_PHASH_MARGIN = 12          # bits above local baseline
DEFAULT_MERGE_WINDOW = Fraction(15, 100)   # seconds (rational, not a timecode
                                            # of the media, so kept exact)

SAMPLE_SIZE = 32                   # pHash input is 32x32 gray
MIN_SAMPLE_FPS = 0.25
MAX_SAMPLE_FPS = 8.0
MIN_PHASH_MARGIN = 1
MAX_PHASH_MARGIN = 40
SPIKE_BASELINE_WINDOW = Fraction(1)       # +/- seconds around each sample
PHASH_BITS = 63                     # DC coefficient excluded

_PTS_RE = re.compile(r"pts_time:([0-9.]+)")
_SCORE_RE = re.compile(r"lavfi\.scene_score=([0-9.]+)")


class PrescanError(RuntimeError):
    """Input, environment or parameter failure; reported as a CLI error."""


# ---------------------------------------------------------------------------
# process helpers
# ---------------------------------------------------------------------------

def _run(cmd: list[str], *, timeout: int = 600) -> subprocess.CompletedProcess:
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, errors="replace", timeout=timeout
        )
    except FileNotFoundError:
        raise PrescanError(
            f"required executable not found: {cmd[0]} (install ffmpeg and ensure it is on PATH)"
        ) from None
    except subprocess.TimeoutExpired:
        raise PrescanError(f"command timed out after {timeout}s: {' '.join(cmd[:5])} ...") from None
    return proc


def _parse_rate(raw: Any) -> Fraction:
    """ffprobe rational string ('30/1', '0/0') -> Fraction (0 when unusable)."""
    if raw is None:
        return Fraction(0)
    text = str(raw)
    if "/" in text:
        num, _, den = text.partition("/")
        try:
            return Fraction(int(num), int(den))
        except (ValueError, ZeroDivisionError):
            return Fraction(0)
    try:
        return Fraction(text)
    except ValueError:
        return Fraction(0)


def _parse_decimal(raw: Any) -> Fraction:
    """ffprobe decimal seconds string -> exact Fraction (e.g. '14.026667')."""
    try:
        return Fraction(str(raw))
    except (ValueError, ZeroDivisionError):
        return Fraction(0)


def _rational(value: Fraction) -> dict[str, int]:
    return {"num": int(value.numerator), "den": int(value.denominator)}


def _frame_of(t: Fraction, fps: Fraction) -> int:
    """Nearest frame index for a time on the media's frame grid (half up)."""
    return int((t * fps + Fraction(1, 2)).__floor__())


# ---------------------------------------------------------------------------
# probe
# ---------------------------------------------------------------------------

def probe(path: Path) -> dict[str, Any]:
    """ffprobe -> media metadata (fps/duration as exact Fractions)."""
    if not path.exists():
        raise PrescanError(f"input not found: {path}")
    if not path.is_file():
        raise PrescanError(f"not a regular file: {path}")
    if path.stat().st_size == 0:
        raise PrescanError(f"empty file: {path}")

    proc = _run([
        FFPROBE, "-v", "error", "-print_format", "json",
        "-show_format", "-show_streams", str(path),
    ])
    if proc.returncode != 0:
        raise PrescanError(
            f"ffprobe cannot read {path.name} (not a media file?): "
            f"{proc.stderr.strip()[:300]}"
        )
    try:
        data = json.loads(proc.stdout or "{}")
    except json.JSONDecodeError:
        raise PrescanError(f"ffprobe returned invalid JSON for {path.name}") from None

    streams = data.get("streams") or []
    fmt = data.get("format") or {}
    video = next(
        (
            s for s in streams
            if s.get("codec_type") == "video"
            and not s.get("disposition", {}).get("attached_pic")
        ),
        None,
    )
    if video is None:
        raise PrescanError(f"no video stream in {path.name}")

    fps = _parse_rate(video.get("avg_frame_rate"))
    if fps <= 0:
        fps = _parse_rate(video.get("r_frame_rate"))
    if fps <= 0:
        raise PrescanError(f"cannot determine frame rate of {path.name}")

    duration = _parse_decimal(fmt.get("duration"))
    if duration <= 0:
        duration = _parse_decimal(video.get("duration"))
    if duration <= 0:
        raise PrescanError(f"cannot determine duration of {path.name}")

    nb_frames = None
    raw_nb = video.get("nb_frames")
    if raw_nb is not None and str(raw_nb).isdigit():
        nb_frames = int(raw_nb)
    if nb_frames is None or nb_frames <= 0:
        nb_frames = int((duration * fps + Fraction(1, 2)).__floor__())

    return {
        "width": int(video.get("width") or 0),
        "height": int(video.get("height") or 0),
        "fps": fps,
        "duration": duration,
        "total_frames": nb_frames,
        "video_codec": video.get("codec_name"),
    }


# ---------------------------------------------------------------------------
# channel A: ffmpeg scene score
# ---------------------------------------------------------------------------

def scene_scores(path: Path) -> list[tuple[Fraction, float]]:
    """Per-frame scene difference score (A channel). Timestamps are Fractions.

    An empty list is a **valid** result: `select='gt(scene,0)'` emits nothing
    for a static shot (solid colour, freeze frame, still image), so "no
    scores" only means "no frame differs from its predecessor". The caller
    turns that into an empty cut table (`cuts: []`,
    `scene_frames_scored: 0`) instead of a failure. A non-zero ffmpeg exit
    with no scores is a real decode failure and still raises.
    """
    proc = _run([
        FFMPEG, "-hide_banner", "-nostats", "-i", str(path),
        "-vf", "select='gt(scene,0)',metadata=print",
        "-an", "-f", "null", "-",
    ])
    scores: list[tuple[Fraction, float]] = []
    last_pts: Fraction | None = None
    for line in (proc.stdout + "\n" + proc.stderr).splitlines():
        m_pts = _PTS_RE.search(line)
        if m_pts:
            try:
                last_pts = Fraction(m_pts.group(1))
            except ValueError:
                last_pts = None
        m_val = _SCORE_RE.search(line)
        if m_val is not None and last_pts is not None:
            scores.append((last_pts, float(m_val.group(1))))
    if not scores and proc.returncode != 0:
        raise PrescanError(
            f"ffmpeg scene scan failed for {path.name} (exit {proc.returncode}): "
            f"{proc.stderr.strip()[-300:] or 'no diagnostics'}"
        )
    return scores


# ---------------------------------------------------------------------------
# channel B: pHash samples
# ---------------------------------------------------------------------------

def gray_samples(
    path: Path, sample_fps: float, fps: Fraction
) -> tuple[list[Fraction], bytes]:
    """Sample size x size gray frames at sample_fps (rawvideo pipe).

    Sample timestamps are snapped onto the media frame grid; ffmpeg's fps
    filter samples approximately on that grid (documented approximation in
    README, channel B only provides spike evidence, never frame-accurate
    cut positions).
    """
    size = SAMPLE_SIZE
    proc = subprocess.run(
        [
            FFMPEG, "-hide_banner", "-nostats", "-i", str(path),
            "-vf", f"fps={sample_fps},scale={size}:{size}:flags=area",
            "-pix_fmt", "gray", "-f", "rawvideo", "-",
        ],
        capture_output=True,
        timeout=600,
    )
    if proc.returncode != 0 and not proc.stdout:
        raise PrescanError(
            f"ffmpeg gray sampling failed for {path.name}: "
            f"{proc.stderr.decode(errors='replace').strip()[:300]}"
        )
    frame_bytes = size * size
    count = len(proc.stdout) // frame_bytes
    step = Fraction(str(sample_fps)).limit_denominator(10**6)
    times: list[Fraction] = []
    for i in range(count):
        idx = int((Fraction(i) * fps / step + Fraction(1, 2)).__floor__())
        times.append(Fraction(idx, 1) / fps)
    return times, proc.stdout[: count * frame_bytes]


_COS: dict[tuple[int, int], list[float]] = {}


def _dct_cos(n: int, u: int) -> list[float]:
    key = (n, u)
    if key not in _COS:
        _COS[key] = [math.cos((2 * x + 1) * u * math.pi / (2 * n)) for x in range(n)]
    return _COS[key]


def phash(gray_block: bytes, n: int = SAMPLE_SIZE) -> int:
    """32x32 gray -> separable DCT -> 8x8 low-frequency band -> 64-bit hash.

    The DC coefficient is dropped; the remaining 63 coefficients are
    thresholded at their median to produce the bits.
    """
    if len(gray_block) != n * n:
        raise PrescanError(f"phash expects {n * n} bytes, got {len(gray_block)}")
    rows = [gray_block[i * n:(i + 1) * n] for i in range(n)]
    # row DCT, first 8 coefficients only
    rows8: list[list[float]] = []
    for row in rows:
        rf = [float(v) for v in row]
        cos = [_dct_cos(n, u) for u in range(8)]
        rows8.append([
            sum(rf[x] * cos[u][x] for x in range(n)) for u in range(8)
        ])
    # column DCT over the 8 retained coefficients
    coeffs: list[float] = []
    for u in range(8):
        col = [rows8[i][u] for i in range(n)]
        for v in range(8):
            mv = _dct_cos(n, v)
            coeffs.append(sum(col[i] * mv[i] for i in range(n)))
    body = sorted(coeffs[1:])
    median = body[len(body) // 2]
    bits = 0
    for i in range(1, len(coeffs)):
        if coeffs[i] > median:
            bits |= 1 << (i - 1)
    return bits


def hamming(a: int, b: int) -> int:
    return (a ^ b).bit_count()


# ---------------------------------------------------------------------------
# scan: two channels + hysteresis
# ---------------------------------------------------------------------------

def _validate_params(
    high: float, low: float, sample_fps: float, phash_margin: int,
    merge_window: Fraction,
) -> None:
    if not (0 < low < high < 1):
        raise PrescanError(f"thresholds must satisfy 0 < low < high < 1 (got low={low}, high={high})")
    if not (MIN_SAMPLE_FPS <= sample_fps <= MAX_SAMPLE_FPS):
        raise PrescanError(
            f"--sample-fps must be within [{MIN_SAMPLE_FPS}, {MAX_SAMPLE_FPS}] (got {sample_fps})"
        )
    if not (MIN_PHASH_MARGIN <= phash_margin <= MAX_PHASH_MARGIN):
        raise PrescanError(
            f"--phash-margin must be within [{MIN_PHASH_MARGIN}, {MAX_PHASH_MARGIN}] bits "
            f"(got {phash_margin})"
        )
    if not (Fraction(0) < merge_window < Fraction(2)):
        raise PrescanError(f"--merge-window must be in (0, 2) seconds (got {merge_window})")


def scan(
    path: Path,
    *,
    high: float = DEFAULT_HIGH,
    low: float = DEFAULT_LOW,
    sample_fps: float = DEFAULT_SAMPLE_FPS,
    phash_margin: int = DEFAULT_PHASH_MARGIN,
    merge_window: Fraction = DEFAULT_MERGE_WINDOW,
) -> dict[str, Any]:
    """Run both channels and emit the cut-list report (see SCHEMA)."""
    _validate_params(high, low, sample_fps, phash_margin, merge_window)

    media = probe(path)
    fps: Fraction = media["fps"]

    scores = scene_scores(path)
    times, raw_gray = gray_samples(path, sample_fps, fps)
    sample_count = SAMPLE_SIZE * SAMPLE_SIZE
    hashes = [
        phash(raw_gray[i * sample_count:(i + 1) * sample_count])
        for i in range(len(times))
    ]
    dists: list[int] = [0]
    for i in range(1, len(hashes)):
        dists.append(hamming(hashes[i], hashes[i - 1]))

    # phash spike = distance exceeding the +/-1 s local baseline median
    spikes: dict[int, bool] = {}
    for i, t in enumerate(times):
        window = [
            dists[j] for j, tj in enumerate(times)
            if j != i and abs(tj - t) <= SPIKE_BASELINE_WINDOW
        ]
        if len(window) < 2:
            spikes[i] = dists[i] >= phash_margin
            continue
        baseline = sorted(window)[len(window) // 2]
        spikes[i] = (dists[i] - baseline) >= phash_margin

    def nearest_sample(t: Fraction) -> int | None:
        if not times:
            return None
        return min(range(len(times)), key=lambda i: abs(times[i] - t))

    # --- gather candidates: every scene-scored frame + every phash spike ---
    raw: dict[Fraction, dict[str, Any]] = {}

    def entry(t: Fraction) -> dict[str, Any]:
        if t not in raw:
            raw[t] = {"scene": 0.0, "spike": False}
        return raw[t]

    for t, sc in scores:
        e = entry(t)
        e["scene"] = max(e["scene"], sc)
    for i, t in enumerate(times):
        if spikes[i]:
            entry(t)["spike"] = True

    # --- hysteresis filter + classification ---
    candidates: list[dict[str, Any]] = []
    for t in sorted(raw):
        scene = raw[t]["scene"]
        spike = raw[t]["spike"]
        if not (scene >= low or spike):
            continue
        hard = scene >= high or (spike and scene >= low)
        idx = nearest_sample(t)
        bits = dists[idx] if idx is not None else 0
        candidates.append({
            "t": t,
            "scene": scene,
            "bits": bits,
            "kind": "hard" if hard else "soft",
        })

    # --- merge peaks closer than merge_window, keep the strongest ---
    merged: list[dict[str, Any]] = []
    for cand in candidates:
        if merged and abs(cand["t"] - merged[-1]["t"]) <= merge_window:
            prev = merged[-1]

            def rank(c: dict[str, Any]) -> tuple[float, float]:
                return (c["scene"], c["bits"] / PHASH_BITS)

            if rank(prev) >= rank(cand):
                continue
            merged[-1] = cand
        else:
            merged.append(cand)

    # --- snap to the frame grid (T0 timecode discipline: frame + rational    ---
    # --- time, never bare floats; raw ffmpeg pts only locate the frame)      ---
    cuts_by_frame: dict[int, dict[str, Any]] = {}
    for c in merged:
        frame = _frame_of(c["t"], fps)
        snapped = {
            "frame": frame,
            "time": _rational(Fraction(frame, 1) / fps),
            "scene_score": round(c["scene"], 4),
            "phash_bits": int(c["bits"]),
            "phash_norm": round(c["bits"] / PHASH_BITS, 4),
            "kind": c["kind"],
        }
        prev = cuts_by_frame.get(frame)
        if prev is None or (prev["scene_score"], prev["phash_norm"]) < (
            snapped["scene_score"], snapped["phash_norm"]
        ):
            cuts_by_frame[frame] = snapped
    cuts = [cuts_by_frame[f] for f in sorted(cuts_by_frame)]

    return {
        "schema": SCHEMA,
        "source": {"name": path.name},
        "media": {
            "width": media["width"],
            "height": media["height"],
            "fps": _rational(fps),
            "duration": _rational(media["duration"]),
            "total_frames": media["total_frames"],
            "video_codec": media["video_codec"],
        },
        "params": {
            "channel_a": "ffmpeg_scene_score",
            "channel_b": "phash_dct_32_8_hamming_norm63",
            "high": high,
            "low": low,
            "sample_fps": sample_fps,
            "phash_margin_bits": phash_margin,
            "merge_window": _rational(merge_window),
            "spike_baseline_window": _rational(SPIKE_BASELINE_WINDOW),
        },
        "scan": {
            "scene_frames_scored": len(scores),
            "phash_samples": len(times),
        },
        "cuts": cuts,
        "note": (
            "kind=hard: scene>=high (or phash spike with scene>=low) - primary cut points. "
            "kind=soft: scene>=low or phash spike - action/transition band, needs visual review. "
            "times are exact rationals {num, den} seconds, quantized to the frame grid "
            "(time == frame/fps); frame is the nearest frame index at media fps."
        ),
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="prescan",
        description=(
            "Forensic pre-screen of a media file: dual-channel cut detection "
            "(scene score + pHash) with dual-threshold hysteresis; emits a "
            "JSON cut list with rational timecodes."
        ),
    )
    p.add_argument("input", help="media file to scan")
    p.add_argument("--out", metavar="PATH", help="write JSON to PATH instead of stdout")
    p.add_argument("--high", type=float, default=DEFAULT_HIGH,
                   help=f"hard-cut scene threshold (default {DEFAULT_HIGH})")
    p.add_argument("--low", type=float, default=DEFAULT_LOW,
                   help=f"soft-cut scene threshold (default {DEFAULT_LOW})")
    p.add_argument("--sample-fps", type=float, default=DEFAULT_SAMPLE_FPS,
                   help=f"pHash channel sampling rate (default {DEFAULT_SAMPLE_FPS})")
    p.add_argument("--phash-margin", type=int, default=DEFAULT_PHASH_MARGIN,
                   help=f"spike margin over local baseline in bits (default {DEFAULT_PHASH_MARGIN})")
    p.add_argument("--merge-window", metavar="SEC", default=None,
                   help="merge peaks closer than SEC seconds (default 0.15)")
    p.add_argument("--compact", action="store_true", help="single-line JSON output")
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)

    merge_window = DEFAULT_MERGE_WINDOW
    if args.merge_window is not None:
        try:
            merge_window = Fraction(str(args.merge_window))
        except (ValueError, ZeroDivisionError):
            print("prescan: error: --merge-window must be a decimal number", file=sys.stderr)
            return 2

    try:
        report = scan(
            Path(args.input),
            high=args.high,
            low=args.low,
            sample_fps=args.sample_fps,
            phash_margin=args.phash_margin,
            merge_window=merge_window,
        )
    except PrescanError as exc:
        print(f"prescan: error: {exc}", file=sys.stderr)
        return 2

    text = json.dumps(
        report,
        indent=None if args.compact else 2,
        ensure_ascii=False,
        separators=(",", ":") if args.compact else None,
    )
    if args.out:
        out_path = Path(args.out)
        try:
            out_path.write_text(text + "\n", encoding="utf-8")
        except OSError as exc:
            print(f"prescan: error: cannot write {out_path}: {exc}", file=sys.stderr)
            return 2
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
