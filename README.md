# Olive Video Editor Community Edition[![Build status](https://github.com/olive-editor/olive/workflows/CI/badge.svg?branch=master)](https://github.com/olive-editor/olive/actions?query=branch%3Amaster)

Olive is a free non-linear video editor for Windows, macOS, and Linux.

Unfortunately, the original author has not submitted code updates for over 7 months, and no public contact information (email or otherwise) is available to reach them directly.

Now, we are maintaining a community edition for this project.
![screen](https://olivevideoeditor.org/img/020-2.png)


**NOTE: Olive is alpha software and is considered highly unstable. While we highly appreciate users testing and providing usage information, please use at your own risk.**

## Binaries
The original author compiled following binaries:
- [0.1.0 alpha](https://github.com/olive-editor/olive/releases/tag/0.1.0)
- [0.2.0 unstable development build](https://github.com/olive-editor/olive/releases/tag/0.2.0-nightly)

## OpenFX Support TODO
- Implement plugin discovery/loading from a given path and populate the cache (currently creates host/cache only). `app/pluginSupport/OliveHost.cpp`
- Wire output clip image storage: allocate a backing buffer, set `kOfxImagePropData`, and update bounds/rowBytes before render. `app/pluginSupport/OliveClip.cpp`
- Provide real input clip image fetches (currently returns an empty `Image` for inputs). `app/pluginSupport/OliveClip.cpp`
- Ensure render path sets per-frame output data and handles ROD/bounds correctly. `app/render/plugin/pluginrenderer.cpp`
- Add missing param instance types (String, Double3D/Integer3D, Group/Page, Custom/Bytes) and mapping to node inputs. `app/pluginSupport/OlivePluginInstance.cpp`, `app/node/plugins/Plugin.cpp`
- Implement `editBegin`/`editEnd`, progress, and timeline hooks instead of stubs. `app/pluginSupport/OlivePluginInstance.cpp`, `app/pluginSupport/OlivePluginInstance.h`
- Integrate persistent message handling with the app UI (currently TODO placeholders). `app/pluginSupport/OlivePluginInstance.cpp`
- Decide and enforce project extent/fielding behavior instead of the current placeholder comment. `app/pluginSupport/OlivePluginInstance.cpp`
- Add OpenGL texture render suite support or explicitly disable it (currently `loadTexture` returns null). `app/pluginSupport/OliveClip.h`
