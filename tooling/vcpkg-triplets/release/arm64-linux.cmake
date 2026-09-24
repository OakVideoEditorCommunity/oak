# Oak's build-type overlay triplet for arm64-linux (see the note in
# ../README.md): the settings mirror vcpkg's builtin arm64-linux.cmake and
# only the build type is pinned. Pass this directory with
# `--overlay-triplets tooling/vcpkg-triplets/release`.
set(VCPKG_TARGET_ARCHITECTURE arm64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_LIBRARY_LINKAGE static)

set(VCPKG_CMAKE_SYSTEM_NAME Linux)

# Build only the release configuration (see x64-linux.cmake).
set(VCPKG_BUILD_TYPE release)
