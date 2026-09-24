# vcpkg overlay triplets

Small overlays over vcpkg's builtin triplets, passed to `vcpkg install`
with `--overlay-triplets <dir>` (both workflows, see
`.github/workflows/ci.yml` and `.github/workflows/cd.yml`).

`release/` pins every supported triplet to the release configuration
(`set(VCPKG_BUILD_TYPE release)`): vcpkg otherwise builds every port
twice — release and debug — which doubles the FFmpeg/port build time in
CI and CD. Our build scripts only ever consume the release layout
(`FFMPEG_DIR=<prefix>` with `<prefix>/lib/pkgconfig`, plus the linker
paths those `.pc` files carry), so the debug half was pure waste; the C
libraries' ABI is identical for the debug cargo binary CI tests.

Notes:

- The non-build-type settings mirror vcpkg's builtin triplets
  (`scripts/triplets/`) so the overlay stays a drop-in replacement; a
  vcpkg upgrade that changes a builtin triplet should be mirrored here.
- `VCPKG_BUILD_TYPE` must be set in the triplet: the environment
  variable of the same name is ignored in manifest mode.
- Do not try `debug` here: several ports (e.g. zlib) patch files in the
  release layout and fail when only the debug configuration is built.
