# Diri GPUI macOS patch

This directory is copied from `zed-industries/zed` revision
`dc2a339d5d043da448a3f7ddc7c0a85c63864aad`, crate `gpui_macos`.

Diri's patch makes the renderer's full-window path and 4x MSAA textures lazy.
Normal Diri scenes use quads, glyph sprites, and CoreGraphics-rasterized brand
marks, so eagerly allocating these textures consumed substantial unified memory
without rendering any paths. Scenes that do contain a path still allocate the
same textures on demand and retain upstream antialiasing quality.

`src/shaders.metallib` is the unchanged upstream shader source compiled from
that pinned revision. Bundling it keeps ordinary Diri builds reproducible on
machines where Xcode's separately downloaded Metal Toolchain is not installed.
