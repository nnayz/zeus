# diri-ui

Shared GPUI tokens and components ported from Dirijor's Swift design system:

- `DesignSystem.swift`: radii, typography, semantic colors, fill/spacing/metric tokens, motion, and floating surfaces.
- `BrandLogos.swift`: the four 24×24 SVG paths and an M/L/H/V/C/S/Q/T/A/Z parser with arc-to-cubic conversion.
- `StatusGlyph.swift`: branded static and animated status marks, shared wall-clock phase math, shell caret, and attention dots.

Run the complete visual state gallery with:

```sh
cargo run -p diri-ui --example gallery
```

## Adding an icon

Ordinary interface icons belong to the shared **Diri Line** SVG family. Do not
add a text glyph, emoji, or a new SF Symbol call for app UI. Provider identities
such as Claude and Codex are brand marks and remain in `brand.rs`; this workflow
is for semantic controls and navigation icons.

1. Add a kebab-case file to `assets/icons/`. Author it on a `0 0 24 24`
   view box using `currentColor`, not a fixed color. The family defaults to a
   `1.75` stroke with round caps and joins:

   ```svg
   <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"
        fill="none" stroke="currentColor" stroke-width="1.75"
        stroke-linecap="round" stroke-linejoin="round">
     <path d="..."/>
   </svg>
   ```

   Keep the silhouette legible at 14–20 px. Prefer a few decisive paths over
   small details, and visually center the shape inside the view box.

2. Register the asset in `src/icon.rs`:

   - add a semantic `IconName` variant;
   - add the variant to `IconName::ALL` and update the array length;
   - map it in `IconName::asset_path`;
   - embed it in `embedded_svg` with `include_bytes!`.

   The `every_icon_has_an_embedded_asset` test catches incomplete registration.

3. Use the semantic icon directly in new code:

   ```rust
   Icon::new(IconName::Folder, IconSize::REGULAR, colors.secondary)
   ```

   Use the shared optical scale (`COMPACT`, `REGULAR`, `LARGE`, or `DISPLAY`)
   instead of adding another one-off icon size.

   If an existing call site still passes an SF Symbol name, also map that name
   in `IconName::from_system_name` and add it to
   `every_legacy_symbol_used_by_the_app_resolves`. The compatibility bridge is
   only for migration; new APIs should accept `IconName` rather than strings.

4. Verify the asset and inspect it alongside the full family from the `diri/`
   workspace root:

   ```sh
   xmllint --noout crates/diri-ui/assets/icons/*.svg
   cargo test -p diri-ui
   cargo check -p diri-app
   cargo run -p diri-ui --example gallery
   ```

`IconAssets` embeds the SVG bytes in the application binary, so adding an icon
does not require a packaging or runtime resource-copy step.

## Intentional GPUI approximations

- GPUI's rounded rectangles use circular corners, so the SwiftUI continuous-corner squircles are represented with the same radius values but standard rounded corners.
- This pinned GPUI revision has no angular/conic gradient paint. The working mark uses a rotating two-stop linear gradient clipped to the vector path. Rotation direction, 2.4-second period, brand tint, and absolute shared phase match the Swift implementation; only the sweep's gradient geometry differs.

Animated `StatusGlyph` entities tick at 10 fps only while marked visible, motion-enabled, in a loud state, and hosted by an active window. Consumers mounting glyphs in virtualized or hidden panes must call `set_visible` when visibility changes.
