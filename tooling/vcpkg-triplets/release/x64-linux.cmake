# Oak's build-type overlay triplet for x64-linux (see the note in
# ../README.md): the settings mirror vcpkg's builtin x64-linux.cmake and
# only the build type is pinned. Pass this directory with
# `--overlay-triplets tooling/vcpkg-triplets/release`.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)

set(VCPKG_CMAKE_SYSTEM_NAME Linux)

# Build only the release configuration. vcpkg otherwise builds every port
# twice (release + debug) and our build scripts/vcpkg caches only ever use
# the release layout (FFMPEG_DIR's lib/pkgconfig).
set(VCPKG_BUILD_TYPE release)
