#!/usr/bin/env python3
"""Zero-tolerance guard: every first-party Oak consumer links only the SDK.

Consumer crates (`oak-app`, `oak-cli`, `oak-worker`, `examples/sdk_editor`)
must not depend on an internal Oak crate nor mention one in their sources.
The only allowed engine entry is the published `quercus-engine-rs` crate
(Quercus Nonlinear Video Editing Engine Rust SDK) from crates.io.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONSUMERS = (
    ROOT / "crates" / "oak-app",
    ROOT / "crates" / "oak-cli",
    ROOT / "crates" / "oak-worker",
    ROOT / "examples" / "sdk_editor",
)
INTERNAL = (
    "oak-audio",
    "oak-codec",
    "oak-core",
    "oak-engine",
    "oak-node",
    "oak-ofx-plugin",
    "oak-otio",
    "oak-render",
    "oak-storage",
    "oak-task",
    "oak-timeline",
    "oak-undo",
)
ALLOWED_DEPENDENCY = "quercus-engine-rs"
REFERENCE = re.compile(
    r"\boak_(?:audio|codec|core|engine|node|ofx_plugin|otio|render|storage|task|timeline|undo)::"
)
DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def source_files(crate: Path) -> list[Path]:
    files = list((crate / "src").rglob("*.rs"))
    if (crate / "tests").is_dir():
        files.extend((crate / "tests").rglob("*.rs"))
    return files


def direct_dependencies(crate: Path) -> set[str]:
    manifest = tomllib.loads((crate / "Cargo.toml").read_text(encoding="utf-8"))
    found: set[str] = set()

    def inspect(section: str, values: object) -> None:
        if not isinstance(values, dict):
            return
        for alias, declaration in values.items():
            package = alias
            if isinstance(declaration, dict):
                package = declaration.get("package", alias)
            if package in INTERNAL:
                found.add(package)

    for section in DEPENDENCY_SECTIONS:
        inspect(section, manifest.get(section))
    for target in manifest.get("target", {}).values():
        if isinstance(target, dict):
            for section in DEPENDENCY_SECTIONS:
                inspect(section, target.get(section))
    return found


def main() -> int:
    errors: list[str] = []
    for consumer in CONSUMERS:
        name = consumer.name
        for dependency in sorted(direct_dependencies(consumer)):
            errors.append(f"{name}: direct Oak dependency [{dependency}]")
        for path in source_files(consumer):
            text = path.read_text(encoding="utf-8")
            for match in REFERENCE.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{path.relative_to(ROOT)}:{line}: internal ABI reference `{match.group(0)}`"
                )
    if errors:
        print("SDK consumer boundary violated:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print(f"SDK consumers OK: {len(CONSUMERS)} crates use only {ALLOWED_DEPENDENCY}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
