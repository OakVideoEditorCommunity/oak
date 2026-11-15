#!/usr/bin/env python3
"""Tests for ai/prescan/prescan.py (T2 forensic pre-screen).

Run:
    python3 test_prescan.py          # or: python3 -m unittest test_prescan -v

The test material is generated on the fly with ffmpeg. Recipe (14 s, 30 fps,
known cut points), also documented in README.md:

    ffmpeg -f lavfi -i testsrc2=size=640x360:rate=30:duration=6 \
           -f lavfi -i smptebars=size=640x360:rate=30:duration=6 \
           -f lavfi -i rgbtestsrc=size=640x360:rate=30:duration=3 \
           -f lavfi -t 3 -i anullsrc=r=48000:cl=stereo \
           -f lavfi -t 14 -i sine=frequency=440:sample_rate=48000:duration=11 \
           -filter_complex "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p,settb=AVTB[c];\
    [2:v]settb=AVTB[g];[c][g]xfade=transition=fade:duration=1:offset=11[v];\
    [3:a][4:a]concat=n=2:v=0:a=1[a]" -map "[v]" -map "[a]" \
           -c:v libx264 -preset veryfast -crf 23 -c:a aac test_scenes.mp4

Known cuts / detection criteria (assertion criteria):
  - hard cut at 6 s (frame 180 @30 fps): must be detected within +-5 frames
    with kind == "hard" (channel A scene score ~0.74 >> high=0.40).
  - dissolve 11-12 s (frames 330-360): must yield a kind == "soft" candidate
    with frame in [330, 366] (band +-0.2 s). Rationale: during a gradual
    crossfade the per-frame scene score stays far below --low (measured
    max 0.014 on this material), so the hit comes from the channel B pHash
    spike; for a static-to-static dissolve the hash jump materializes at the
    first frame of the incoming scene (360 = 12.0 s, the band boundary).
  - audio silence 0-3 s is part of the material only; prescan reads video.

Golden comparison: golden/test_scenes.cutlist.json stores the projected
cut table (structure, kinds, frames, thresholds) produced by this recipe.
Regenerate after an intentional algorithm/ffmpeg change with:
    PRESCAN_UPDATE_GOLDEN=1 python3 test_prescan.py
Scores are compared with tolerance; structure and frames per assertion.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from fractions import Fraction
from unittest import mock
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import prescan  # noqa: E402  (local module under test)

PRESCAN_PY = HERE / "prescan.py"
GOLDEN = HERE / "golden" / "test_scenes.cutlist.json"

MATERIAL_NAME = "test_scenes.mp4"
HARD_CUT_FRAME = 180          # 6.0 s @ 30 fps
DISSOLVE_START_FRAME = 330    # 11.0 s
DISSOLVE_END_FRAME = 360      # 12.0 s
DISSOLVE_TOLERANCE_FRAMES = 6  # band +-0.2 s

# ffmpeg recipe: fixed parameters, deterministic material (see module docstring)
RECIPE = [
    "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
    "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=6",
    "-f", "lavfi", "-i", "smptebars=size=640x360:rate=30:duration=6",
    "-f", "lavfi", "-i", "rgbtestsrc=size=640x360:rate=30:duration=3",
    "-f", "lavfi", "-t", "3", "-i", "anullsrc=r=48000:cl=stereo",
    "-f", "lavfi", "-t", "14", "-i",
    "sine=frequency=440:sample_rate=48000:duration=11",
    "-filter_complex",
    "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p,settb=AVTB[c];"
    "[2:v]settb=AVTB[g];[c][g]xfade=transition=fade:duration=1:offset=11[v];"
    "[3:a][4:a]concat=n=2:v=0:a=1[a]",
    "-map", "[v]", "-map", "[a]",
    "-c:v", "libx264", "-preset", "veryfast", "-crf", "23",
    "-c:a", "aac",
]

SCORE_TOLERANCE = 0.15
FRAME_TOLERANCE = 5


def run_cli(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(PRESCAN_PY), *args],
        capture_output=True, text=True, timeout=300,
    )


def golden_projection(report: dict) -> dict:
    """Stable projection of a report for golden comparison (no source name)."""
    return {
        "schema": report["schema"],
        "media": {
            "width": report["media"]["width"],
            "height": report["media"]["height"],
            "fps": report["media"]["fps"],
            "duration": report["media"]["duration"],
            "total_frames": report["media"]["total_frames"],
        },
        "params": report["params"],
        "cuts": [
            {
                "frame": c["frame"],
                "kind": c["kind"],
                "scene_score": c["scene_score"],
                "phash_norm": c["phash_norm"],
            }
            for c in report["cuts"]
        ],
    }


class PrescanBase(unittest.TestCase):
    """Shared material + one canonical CLI run (generated once)."""

    tmpdir: tempfile.TemporaryDirectory
    material: Path
    report: dict          # canonical run (defaults)
    report2: dict         # second identical run (determinism)

    @classmethod
    def setUpClass(cls) -> None:
        cls.tmpdir = tempfile.TemporaryDirectory(prefix="prescan_test_")
        base = Path(cls.tmpdir.name)
        cls.material = base / MATERIAL_NAME
        proc = subprocess.run(
            [*RECIPE, str(cls.material)],
            capture_output=True, text=True, timeout=300,
        )
        if proc.returncode != 0 or not cls.material.is_file():
            raise RuntimeError(f"material generation failed: {proc.stderr[:500]}")

        r1 = run_cli(str(cls.material))
        if r1.returncode != 0:
            raise RuntimeError(f"prescan run1 failed: {r1.stderr[:500]}")
        cls.report = json.loads(r1.stdout)

        r2 = run_cli(str(cls.material))
        if r2.returncode != 0:
            raise RuntimeError(f"prescan run2 failed: {r2.stderr[:500]}")
        cls.report2 = json.loads(r2.stdout)

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tmpdir.cleanup()


class TestKnownCuts(PrescanBase):
    """Detection acceptance on material with known cut points."""

    def test_hard_cut_detected_within_5_frames(self) -> None:
        hards = [c for c in self.report["cuts"] if c["kind"] == "hard"]
        self.assertTrue(hards, "no hard cut detected")
        nearest = min(abs(c["frame"] - HARD_CUT_FRAME) for c in hards)
        self.assertLessEqual(
            nearest, FRAME_TOLERANCE,
            f"hard cut not within +-{FRAME_TOLERANCE} frames of "
            f"{HARD_CUT_FRAME}: {[c['frame'] for c in hards]}",
        )

    def test_exactly_one_hard_cut(self) -> None:
        hards = [c for c in self.report["cuts"] if c["kind"] == "hard"]
        self.assertEqual(len(hards), 1, f"expected 1 hard cut, got {hards}")

    def test_dissolve_band_yields_soft_candidate(self) -> None:
        lo = DISSOLVE_START_FRAME - DISSOLVE_TOLERANCE_FRAMES
        hi = DISSOLVE_END_FRAME + DISSOLVE_TOLERANCE_FRAMES
        hits = [
            c for c in self.report["cuts"]
            if c["kind"] == "soft" and lo <= c["frame"] <= hi
        ]
        self.assertTrue(
            hits,
            f"no soft candidate in dissolve band [{lo}, {hi}]: "
            f"{[(c['frame'], c['kind']) for c in self.report['cuts']]}",
        )

    def test_dissolve_evidence_is_phash_spike_not_scene(self) -> None:
        """A gradual fade must not claim hard status via scene score."""
        lo = DISSOLVE_START_FRAME - DISSOLVE_TOLERANCE_FRAMES
        hi = DISSOLVE_END_FRAME + DISSOLVE_TOLERANCE_FRAMES
        hit = next(
            c for c in self.report["cuts"]
            if c["kind"] == "soft" and lo <= c["frame"] <= hi
        )
        self.assertLess(hit["scene_score"], self.report["params"]["low"])
        self.assertGreaterEqual(hit["phash_norm"], 0.15)
        self.assertEqual(hit["kind"], "soft")

    def test_false_positive_bounds(self) -> None:
        cuts = self.report["cuts"]
        hards = [c for c in cuts if c["kind"] == "hard"]
        softs = [c for c in cuts if c["kind"] == "soft"]
        self.assertLessEqual(len(hards), 2, f"hard false positives: {hards}")
        self.assertLessEqual(len(softs), 3, f"soft false positives: {softs}")

    def test_detection_rate(self) -> None:
        """Both known cuts detected -> 2/2."""
        self.assertEqual(len(self.report["cuts"]), 2,
                         f"expected exactly the 2 known cuts: {self.report['cuts']}")


class TestOutputContract(PrescanBase):
    """Schema / timecode discipline of the JSON cut list."""

    def test_schema_header(self) -> None:
        self.assertEqual(self.report["schema"], prescan.SCHEMA)
        self.assertIn("media", self.report)
        self.assertIn("params", self.report)
        self.assertIn("note", self.report)

    def test_media_meta_rational(self) -> None:
        media = self.report["media"]
        for key in ("fps", "duration"):
            self.assertIsInstance(media[key]["num"], int, key)
            self.assertIsInstance(media[key]["den"], int, key)
            self.assertGreater(media[key]["den"], 0, key)
        self.assertEqual(media["fps"], {"num": 30, "den": 1})
        self.assertEqual(media["duration"], {"num": 14, "den": 1})
        self.assertEqual(media["total_frames"], 420)
        self.assertEqual((media["width"], media["height"]), (640, 360))

    def test_every_cut_time_is_rational_and_on_frame_grid(self) -> None:
        fps = Fraction(
            self.report["media"]["fps"]["num"], self.report["media"]["fps"]["den"]
        )
        for c in self.report["cuts"]:
            self.assertIsInstance(c["frame"], int)
            t = c["time"]
            self.assertEqual(set(t), {"num", "den"}, f"bare time fields: {t}")
            self.assertIsInstance(t["num"], int)
            self.assertIsInstance(t["den"], int)
            self.assertGreater(t["den"], 0)
            # time == frame / fps exactly (frame-grid quantization)
            self.assertEqual(Fraction(t["num"], t["den"]), Fraction(c["frame"], 1) / fps)
            # scores are normalized numbers, not timecodes
            self.assertGreaterEqual(c["scene_score"], 0.0)
            self.assertLessEqual(c["scene_score"], 1.0)
            self.assertGreaterEqual(c["phash_norm"], 0.0)
            self.assertLessEqual(c["phash_norm"], 1.0)
            self.assertIn(c["kind"], ("hard", "soft"))

    def test_params_echo_thresholds(self) -> None:
        params = self.report["params"]
        self.assertEqual(params["high"], prescan.DEFAULT_HIGH)
        self.assertEqual(params["low"], prescan.DEFAULT_LOW)
        self.assertEqual(params["sample_fps"], prescan.DEFAULT_SAMPLE_FPS)
        self.assertEqual(params["phash_margin_bits"], prescan.DEFAULT_PHASH_MARGIN)
        self.assertEqual(params["merge_window"], {"num": 3, "den": 20})

    def test_scan_counters_nonzero(self) -> None:
        self.assertGreater(self.report["scan"]["scene_frames_scored"], 0)
        self.assertGreater(self.report["scan"]["phash_samples"], 0)

    def test_repeatable_output(self) -> None:
        self.assertEqual(self.report, self.report2, "two identical runs differ")


class TestGolden(PrescanBase):
    """Golden comparison of the cut table for the fixed material."""

    def test_golden_compare(self) -> None:
        actual = golden_projection(self.report)
        if os.environ.get("PRESCAN_UPDATE_GOLDEN") == "1":
            GOLDEN.parent.mkdir(parents=True, exist_ok=True)
            GOLDEN.write_text(
                json.dumps(actual, indent=2, ensure_ascii=False) + "\n",
                encoding="utf-8",
            )
        self.assertTrue(GOLDEN.is_file(), f"golden missing: {GOLDEN}")
        golden = json.loads(GOLDEN.read_text(encoding="utf-8"))

        # structure: schema / media / thresholds must match exactly
        self.assertEqual(actual["schema"], golden["schema"])
        self.assertEqual(actual["media"], golden["media"])
        self.assertEqual(actual["params"], golden["params"])

        # cut table: same structure (count + kinds + frames within tolerance)
        self.assertEqual(len(actual["cuts"]), len(golden["cuts"]),
                         "cut count drifted from golden")
        for got, want in zip(actual["cuts"], golden["cuts"]):
            self.assertEqual(got["kind"], want["kind"],
                             f"kind mismatch at frame {want['frame']}")
            self.assertLessEqual(
                abs(got["frame"] - want["frame"]), FRAME_TOLERANCE,
                f"frame drift vs golden: got {got['frame']}, want {want['frame']}",
            )
            self.assertLessEqual(
                abs(got["scene_score"] - want["scene_score"]), SCORE_TOLERANCE,
                f"scene_score drift at frame {want['frame']}",
            )
            self.assertLessEqual(
                abs(got["phash_norm"] - want["phash_norm"]), SCORE_TOLERANCE,
                f"phash_norm drift at frame {want['frame']}",
            )


class TestStaticMaterial(unittest.TestCase):
    """Static (single-colour) material is valid input, not an input error.

    Regression for the T2 false failure: a shot with no frame-to-frame
    change makes ffmpeg's `select='gt(scene,0)'` emit nothing, which used to
    abort the CLI (exit 2, "ffmpeg scene scan produced no scores"). The
    contract is an **empty cut table** (`cuts: []`, `scene_frames_scored: 0`)
    with exit 0, so an agent working on a still shot gets "no cuts" rather
    than a tool failure.
    """

    STATIC_RECIPE = [
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "color=c=gray:size=320x240:rate=30:duration=4",
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "23",
        "-pix_fmt", "yuv420p",
    ]

    tmpdir: tempfile.TemporaryDirectory
    material: Path
    proc: subprocess.CompletedProcess

    @classmethod
    def setUpClass(cls) -> None:
        cls.tmpdir = tempfile.TemporaryDirectory(prefix="prescan_static_")
        cls.material = Path(cls.tmpdir.name) / "static_gray.mp4"
        generated = subprocess.run(
            [*cls.STATIC_RECIPE, str(cls.material)],
            capture_output=True, text=True, timeout=300,
        )
        if generated.returncode != 0 or not cls.material.is_file():
            raise RuntimeError(f"static material generation failed: {generated.stderr[:500]}")
        cls.proc = run_cli(str(cls.material))

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tmpdir.cleanup()

    def test_static_material_exits_zero(self) -> None:
        self.assertEqual(self.proc.returncode, 0, self.proc.stderr)
        self.assertEqual(self.proc.stderr, "", "a static shot must not warn")

    def test_cut_table_is_empty(self) -> None:
        report = json.loads(self.proc.stdout)
        self.assertEqual(report["cuts"], [], "static shot must yield no cuts")
        self.assertEqual(report["scan"]["scene_frames_scored"], 0)
        # Channel B still sampled the file: empty cuts are a result, not a
        # missing scan.
        self.assertGreater(report["scan"]["phash_samples"], 0)

    def test_report_contract_still_valid(self) -> None:
        report = json.loads(self.proc.stdout)
        self.assertEqual(report["schema"], prescan.SCHEMA)
        self.assertEqual(report["media"]["total_frames"], 120)
        self.assertEqual(report["media"]["fps"], {"num": 30, "den": 1})
        self.assertEqual(report["media"]["duration"], {"num": 4, "den": 1})
        self.assertEqual(report["params"]["high"], prescan.DEFAULT_HIGH)


class TestCliOptions(PrescanBase):
    """CLI surface: --out, threshold overrides, compact output."""

    def test_out_writes_file_stdout_empty(self) -> None:
        out = Path(self.tmpdir.name) / "cutlist.json"
        proc = run_cli(str(self.material), "--out", str(out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout.strip(), "", "--out should not print to stdout")
        data = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(data["schema"], prescan.SCHEMA)
        self.assertEqual(len(data["cuts"]), len(self.report["cuts"]))

    def test_threshold_override_echoed(self) -> None:
        proc = run_cli(str(self.material), "--high", "0.30", "--low", "0.10")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        data = json.loads(proc.stdout)
        self.assertEqual(data["params"]["high"], 0.30)
        self.assertEqual(data["params"]["low"], 0.10)
        # looser thresholds must not lose the known hard cut
        hards = [c for c in data["cuts"] if c["kind"] == "hard"]
        self.assertTrue(any(abs(c["frame"] - HARD_CUT_FRAME) <= FRAME_TOLERANCE
                            for c in hards))

    def test_compact_output_is_single_line_json(self) -> None:
        proc = run_cli(str(self.material), "--compact")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(len(proc.stdout.strip().splitlines()), 1)
        data = json.loads(proc.stdout)
        self.assertEqual(len(data["cuts"]), len(self.report["cuts"]))


class TestErrorPaths(PrescanBase):
    """Bad inputs must fail fast with exit code 2 and a stderr message."""

    def assert_cli_error(self, *args: str) -> None:
        proc = run_cli(*args)
        self.assertEqual(proc.returncode, 2,
                         f"expected exit 2 for {args}: rc={proc.returncode}")
        self.assertIn("prescan: error:", proc.stderr)
        self.assertEqual(proc.stdout.strip(), "",
                         "error path must not emit JSON on stdout")

    def test_missing_file(self) -> None:
        self.assert_cli_error(str(Path(self.tmpdir.name) / "no_such.mp4"))

    def test_empty_file(self) -> None:
        empty = Path(self.tmpdir.name) / "empty.mp4"
        empty.write_bytes(b"")
        self.assert_cli_error(str(empty))

    def test_non_media_text_file(self) -> None:
        txt = Path(self.tmpdir.name) / "notes.txt"
        txt.write_text("this is not a media file\n", encoding="utf-8")
        self.assert_cli_error(str(txt))

    def test_directory_input(self) -> None:
        self.assert_cli_error(str(Path(self.tmpdir.name)))

    def test_invalid_thresholds(self) -> None:
        self.assert_cli_error(str(self.material), "--high", "0.1", "--low", "0.5")

    def test_out_unwritable_path(self) -> None:
        bad_out = Path(self.tmpdir.name) / "no_such_dir" / "x.json"
        proc = run_cli(str(self.material), "--out", str(bad_out))
        self.assertEqual(proc.returncode, 2, proc.stderr)
        self.assertIn("prescan: error:", proc.stderr)


class TestUnits(unittest.TestCase):
    """Pure-python units (no subprocess)."""

    def test_phash_identical_blocks_distance_zero(self) -> None:
        block = bytes((x * 7 + 13) % 256 for x in range(32 * 32))
        self.assertEqual(prescan.hamming(prescan.phash(block),
                                         prescan.phash(block)), 0)

    def test_phash_different_blocks_distance_positive(self) -> None:
        a = bytes((x * 7 + 13) % 256 for x in range(32 * 32))
        b = bytes((x * 11 + 29) % 256 for x in range(32 * 32))
        d = prescan.hamming(prescan.phash(a), prescan.phash(b))
        self.assertGreater(d, 0)
        self.assertLessEqual(d, 63)

    def test_frame_of_rounds_half_up(self) -> None:
        fps = Fraction(30, 1)
        self.assertEqual(prescan._frame_of(Fraction(6, 1), fps), 180)
        self.assertEqual(prescan._frame_of(Fraction(181, 30), fps), 181)
        # 180.5 frames -> rounds half up to 181
        self.assertEqual(prescan._frame_of(Fraction(361, 60), fps), 181)

    def test_parse_rate(self) -> None:
        self.assertEqual(prescan._parse_rate("30/1"), Fraction(30, 1))
        self.assertEqual(prescan._parse_rate("0/0"), Fraction(0))
        self.assertEqual(prescan._parse_rate(None), Fraction(0))

    def test_validate_params_rejects_bad_order(self) -> None:
        with self.assertRaises(prescan.PrescanError):
            prescan._validate_params(0.1, 0.5, 2.0, 12, Fraction(15, 100))
        with self.assertRaises(prescan.PrescanError):
            prescan._validate_params(0.4, 0.15, 99.0, 12, Fraction(15, 100))

    def test_missing_ffmpeg_is_still_an_error(self) -> None:
        """A missing tool must never degrade into an empty cut table."""
        with mock.patch.object(prescan, "FFMPEG", "/nonexistent/ffmpeg-for-test"):
            with self.assertRaises(prescan.PrescanError) as ctx:
                prescan.scene_scores(Path("whatever.mp4"))
        self.assertIn("required executable not found", str(ctx.exception))

    def test_ffmpeg_failure_without_scores_is_still_an_error(self) -> None:
        """Zero scores + non-zero exit = decode failure, not a static shot."""
        fake = subprocess.CompletedProcess(
            args=["ffmpeg"], returncode=1, stdout="",
            stderr="Invalid data found when processing input",
        )
        with mock.patch.object(prescan, "_run", return_value=fake):
            with self.assertRaises(prescan.PrescanError) as ctx:
                prescan.scene_scores(Path("broken.mp4"))
        self.assertIn("scene scan failed", str(ctx.exception))
        self.assertIn("exit 1", str(ctx.exception))


if __name__ == "__main__":
    unittest.main(verbosity=2)
