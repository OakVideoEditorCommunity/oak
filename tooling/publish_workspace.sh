#!/usr/bin/env bash
# Publish the Oak engine graph to crates.io in dependency order.
#
# Usage:
#   tooling/publish_workspace.sh --dry-run   # metadata/registry check only
#   tooling/publish_workspace.sh             # real publish
#
# Every crate below declares `version = "=0.5.2"` on its internal path
# dependencies, so the order matters: cargo refuses to package a crate whose
# dependency version is not in the registry yet.

set -euo pipefail
cd "$(dirname "$0")/.."

DRY_RUN=0
if [[ ${1:-} == "--dry-run" ]]; then
    DRY_RUN=1
fi

ORDER=(
    oak-core
    oak-ffmpeg-link
    oak-codec
    oak-audio
    oak-undo
    oak-node
    oak-render
    oak-ofx-plugin
    oak-otio
    oak-storage
    oak-task
    oak-timeline
    oak-engine
    oak-engine-rust-sdk
)

for crate in "${ORDER[@]}"; do
    echo "==> $crate"
    if (( DRY_RUN )); then
        cargo publish -p "$crate" --dry-run
    else
        cargo publish -p "$crate"
    fi
done
