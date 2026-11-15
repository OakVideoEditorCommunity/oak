# ai/prescan — forensic pre-screen (T2 evidence layer)

Standalone cut-point pre-screen for the AI editing pipeline
(`docs/zh/plans/ai-editing-feature-plan.md`, milestone **T2**). It reads
**only the media file** — no engine process, no SDK, no project state — and
emits a JSON cut list (evidence for downstream planning; zero LLM calls).

## Usage

```bash
python3 prescan.py INPUT.mp4                     # JSON cut list to stdout
python3 prescan.py INPUT.mp4 --out cuts.json     # write to file instead
python3 prescan.py INPUT.mp4 --high 0.35 --low 0.10 --sample-fps 4
```

| Option | Default | Meaning |
|---|---|---|
| `--out PATH` | (stdout) | write the JSON cut list to `PATH` |
| `--high F` | `0.40` | scene score threshold for **hard** cut candidates |
| `--low F` | `0.15` | scene score threshold for **soft** candidates |
| `--sample-fps F` | `2.0` | channel B sampling rate (allowed range 0.25–8) |
| `--phash-margin N` | `12` | spike margin over local pHash baseline, in bits (1–40) |
| `--merge-window SEC` | `0.15` | merge detection peaks closer than this (seconds) |
| `--compact` | off | single-line JSON |

Exit codes: `0` success, `2` input/parameter error (message on stderr as
`prescan: error: ...`; never JSON on stdout).

## Algorithm

**Channel A — scene score (hard-cut sensitive).** ffmpeg runs a per-frame
scene difference (`select='gt(scene,0)',metadata=print`) and reports a score
in `[0, 1]` for every frame where the picture changes relative to the
previous frame.

**Channel B — pHash (robust to slow camera motion).** Frames are sampled at
`--sample-fps`, downscaled to 32×32 gray, transformed with a separable DCT,
and the 8×8 low-frequency band is thresholded at its median to a 64-bit hash
(DC coefficient dropped). The Hamming distance between adjacent samples,
normalized as `phash_norm = bits / 63`, measures perceptual change.

**Spike test (channel B evidence).** A sample is a *spike* when its distance
exceeds the median of its ±1 s neighborhood by at least `--phash-margin`
bits. This suppresses footage that is uniformly high-distance (animation,
heavy grain) and fires on localized identity changes.

**Dual-threshold hysteresis + merge.**

```
keep candidate  if  scene >= low  or  phash_spike
classify       hard if scene >= high or (phash_spike and scene >= low)
               soft otherwise            # action / transition band
merge          peaks within --merge-window, keeping the strongest
               (rank = scene_score, then phash_norm)
```

`hard` cuts are the primary cut points; `soft` candidates mark action or
transition bands and are meant for visual review downstream.

**Timecode discipline.** Raw ffmpeg timestamps are only used to *locate* a
frame. Every reported time is snapped to the frame grid: `frame` is the
nearest frame index at the container's average frame rate and
`time == {num, den}` seconds with `time == frame / fps` exactly — no bare
floating-point timecodes (same rule as the T0 tool schema).

## Output schema (real excerpt)

```json
{
  "schema": "oak.prescan.cutlist.v1",
  "source": { "name": "test_scenes.mp4" },
  "media": {
    "width": 640, "height": 360,
    "fps": { "num": 30, "den": 1 },
    "duration": { "num": 14, "den": 1 },
    "total_frames": 420, "video_codec": "h264"
  },
  "params": {
    "channel_a": "ffmpeg_scene_score",
    "channel_b": "phash_dct_32_8_hamming_norm63",
    "high": 0.4, "low": 0.15,
    "sample_fps": 2.0, "phash_margin_bits": 12,
    "merge_window": { "num": 3, "den": 20 },
    "spike_baseline_window": { "num": 1, "den": 1 }
  },
  "scan": { "scene_frames_scored": 213, "phash_samples": 28 },
  "cuts": [
    { "frame": 180, "time": { "num": 6, "den": 1 },
      "scene_score": 0.7391, "phash_bits": 30, "phash_norm": 0.4762,
      "kind": "hard" },
    { "frame": 360, "time": { "num": 12, "den": 1 },
      "scene_score": 0.0018, "phash_bits": 28, "phash_norm": 0.4444,
      "kind": "soft" }
  ],
  "note": "kind=hard: scene>=high ... (see file)"
}
```

The header records media metadata (rational fps/duration, total frames) and
the exact algorithm parameters, so any run is reproducible from its own
output.

## Dependencies

- Python **≥ 3.10** (standard library only: `argparse`, `fractions`,
  `subprocess`, ... — no third-party packages; **PIL / numpy are not
  required**).
- **ffmpeg and ffprobe on `PATH`** (used for decoding, scene scoring and
  probing).

CI therefore needs exactly: a Python ≥ 3.10 interpreter and an ffmpeg build
with `libx264` (for the test material recipe). No network, no keys.

## Tests

```bash
python3 test_prescan.py        # 32 tests, ~70-140 s (load-dependent)
```

The suite generates a 14 s / 30 fps material with **known cut points** via
ffmpeg (fixed recipe, `RECIPE` in `test_prescan.py`):

```bash
ffmpeg -f lavfi -i testsrc2=size=640x360:rate=30:duration=6 \
       -f lavfi -i smptebars=size=640x360:rate=30:duration=6 \
       -f lavfi -i rgbtestsrc=size=640x360:rate=30:duration=3 \
       -f lavfi -t 3 -i anullsrc=r=48000:cl=stereo \
       -f lavfi -t 14 -i sine=frequency=440:sample_rate=48000:duration=11 \
       -filter_complex "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p,settb=AVTB[c];\
[2:v]settb=AVTB[g];[c][g]xfade=transition=fade:duration=1:offset=11[v];\
[3:a][4:a]concat=n=2:v=0:a=1[a]" -map "[v]" -map "[a]" \
       -c:v libx264 -preset veryfast -crf 23 -c:a aac test_scenes.mp4
```

Content map and assertion criteria:

| segment | frames | expected evidence |
|---|---|---|
| 0–6 s `testsrc2` (motion) | 0–179 | no cut (motion is below thresholds) |
| **hard cut @ 6.0 s** | **180** | `hard`, within ±5 frames (channel A, score ≈ 0.74) |
| 6–11 s `smptebars` (static) | 179–329 | no cut |
| **dissolve 11–12 s** (`xfade` fade → `rgbtestsrc`) | 330–360 | `soft` candidate with frame in `[324, 366]` |
| 12–14 s `rgbtestsrc` (static) | 360–419 | no cut |

Dissolve criteria: a gradual fade keeps the per-frame scene score far below
`--low` (measured max 0.014 on this material), so the hit comes from the
channel B spike; for a static-to-static dissolve the hash jump materializes
at the **first frame of the incoming scene** (360 = 12.0 s, the band
boundary), classified `soft` because no single frame jumps by `scene >= high`.

**Golden comparison.** `golden/test_scenes.cutlist.json` stores the
projected cut table (schema, media meta, thresholds, per-cut kind/frame/
scores). The test asserts exact match on structure and thresholds, frame
tolerance ±5, score tolerance ±0.15. Regenerate after an *intentional*
algorithm or ffmpeg change:

```bash
PRESCAN_UPDATE_GOLDEN=1 python3 test_prescan.py
```

The golden ties to the ffmpeg/x264 build that produced it (decoded pixels
drive the scores); a toolchain bump may legitimately require regeneration.

## Known limitations

- **Motion-heavy segments.** Continuously animated content (e.g. moving
  patterns, heavy grain) keeps the pHash baseline high, which both masks
  dissolves into/out of such segments and can produce `soft` false positives
  when a local spike still clears the margin. Measured: on a variant with an
  animated `gradients` tail, the 11–12 s dissolve spike was 10 bits over
  baseline (margin 12 → missed) while the animated tail produced soft spikes
  of exactly +12 bits (false positives at 14.5 s). The static-flank recipe
  above is the canonical, robust case; tune `--phash-margin` /
  `--sample-fps` per footage type.
- **Low-contrast hard cuts.** A cut between visually similar shots scores a
  small scene difference and may fall below `--low`; lower `--low` (watch
  false positives) or raise `--sample-fps`.
- **Dissolves are reported as `soft`, not frame-accurate transitions.**
  The detector marks where evidence peaks (typically the completion frame of
  the fade), not the fade's start; downstream review should treat the whole
  `--merge-window` neighborhood as the transition band.
- **Channel B timestamps are approximate.** ffmpeg's `fps` filter samples
  near (not provably exactly on) the frame grid; only channel A provides
  frame-accurate positions. Channel B contributes spike evidence only.
- **Average frame rate only.** The frame grid uses `avg_frame_rate`
  (falling back to `r_frame_rate`); VFR material is quantized on that grid.
- **Audio is ignored.** Silence detection is out of scope for this layer.
- **Single video stream** (first non-cover stream) is scanned.
- **Static shots yield an empty cut table, not an error.** A shot with no
  frame-to-frame change (solid colour, freeze frame, still image) produces no
  ffmpeg scene scores at all, because `select='gt(scene,0)'` emits nothing.
  That is valid input: the CLI exits 0 and reports `cuts: []` with
  `scan.scene_frames_scored: 0` (channel B still records its
  `phash_samples`). Only a non-zero ffmpeg exit with no scores — a real
  decode failure — is still reported as an input error (exit 2), and a
  missing `ffmpeg`/`ffprobe` binary remains a hard error rather than an empty
  result.
- **Truncated/partially damaged files can exit 0.** A faststart container cut
  mid-mdat may have `ffmpeg -f null` return 0 while only part of the stream was
  decoded (it warns `partial file` on stderr). Such material is treated as a
  normal clip and produces a cut table from the frames that did decode. This
  predates the static-shot fix and is unchanged by it.
- **Residual boundary of the static-shot rule.** The empty-table path keys on
  "no scores **and** zero ffmpeg exit". A corrupt file that decodes **no**
  frames yet still exits 0 would therefore be reported as a static shot
  (`cuts: []`) instead of an input error. No reproducible case was constructed
  during the T2 regression review, so it is recorded as a known residual; if
  it ever surfaces, guard `scan()` on `phash_samples == 0` (a genuinely static
  clip always yields samples on channel B).

## Files

| file | role |
|---|---|
| `prescan.py` | standalone CLI + library (`scan()`, `probe()`) |
| `test_prescan.py` | material generation, detection/golden/error-path tests |
| `golden/test_scenes.cutlist.json` | golden cut table for the fixed material |
