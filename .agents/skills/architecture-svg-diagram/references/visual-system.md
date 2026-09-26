# Visual system

Dark canvas, strong white outlines, compact chips, rounded containers, crisp orthogonal connectors. Restrained color. Designed product graphic, not a flowchart.

## Canvas

Wide landscape. Start from `viewBox="0 0 2320 1100"`. Adapt to content; complex diagrams usually sit between 1800 and 2600 wide. Fill `#0E0E10` (near-black, not pure black).

## Tokens

| Role | Value |
|---|---|
| Canvas / chip fill | `#0E0E10` |
| Primary panel | `#1A1A1D` |
| Primary text / strong border | `#FFFFFF` |
| Secondary border | `#3F3F46` |
| Muted text | `#A1A1AA` |
| Very muted text | `#71717A` |

Brand colors only on recognizable provider tiles, or one chosen accent.

No shadows, glows, glass blur, gradients, neon, 3D boxes, pastel flowchart fills, emojis-as-icons, or huge arrowheads unless the user asks.

## Type

```
font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif
```

Prefer SVG `<text>`. The diagram must remain understandable if Inter is missing.

| Role | Size | Weight | Fill |
|---|---|---|---|
| Diagram title | 30–36 | 600–700 | `#FFFFFF` |
| Card title | 26–32 | 600 | `#FFFFFF` |
| Chip label | 19–22 | 500–600 | `#FFFFFF` |
| Annotation | 16–18 | 400–500 | `#A1A1AA` |

Centered labels use `text-anchor="middle"` and `dominant-baseline="middle"`. Inside a known card, compute a stable baseline — do not nudge with transforms.

## Shapes

**Panel.** Fill `#1A1A1D`, stroke `#FFFFFF` 2.5, rx 14. Height 140–160 (nested 105–125). Generous inner padding.

**Group.** Fill none, stroke `#FFFFFF` 2.5, dasharray `10 9`, rx 18. Use sparingly for scope, runtime, cluster, or trust boundary — not decoration.

**Chip.** Fill `#0E0E10`, stroke `#3F3F46` 2, rx 10, height 44, horizontal padding ≥ 14. Small circular separators for compact phrases when a full arrow would be excessive.

**Provider tile.** 44×44, rx 10–11. Brand fill allowed; keep the mark white when that matches the real logo and contrast. Missing official icon → simple line icon, never a fabricated logo. Embed paths locally; do not hotlink.

**Section icon.** 28–36 px, stroke `#FFFFFF` ~2, round caps/joins, fill none. Same size across peer cards.

Icons, in order: custom line icon → approved open-source path → official provider mark (when license allows) → text-only chip.

## Spacing

```
Outer margin:  50–70
Major gap:     45–60
Card gap:      20–30
Chip gap:      10–14
Icon tile gap: 10
```

Whole-number coordinates, or simple halves. No values like `x="713.482937192"`. Major strokes stay at 2.5 — hairlines disappear at README scale.

## Connectors

Stroke `#FFFFFF`, width 2.5, fill none. Use markers, not hand-drawn triangles:

```svg
<marker id="arrow" viewBox="0 0 10 10" refX="8.5" refY="5"
        markerWidth="6" markerHeight="6" orient="auto">
  <path d="M0 0.6L9 5L0 9.4z" fill="#FFFFFF"/>
</marker>
```

`marker-end="url(#arrow)"` for one-way flow. Reverse marker only when the edge is bidirectional. No arrowheads on decorative grouping lines.

Routing, in order: straight H, straight V, orthogonal elbows, diagonal only if it materially reduces clutter. No arbitrary curves. Softened corners are small quadratic transitions, not swoops.

```svg
<path class="flow" d="M120 300 H180 V420 H240" marker-end="url(#arrow)"/>
```

Connect to the visual center or a clear port. Never through text, across a label, over an icon tile, with an ambiguous terminus, or riding a card border for a long stretch.

Arrows mean request, data, execution, dependency, event, or sync/async flow. If the category differs, label it (`HTTP`, `WebSocket`, `Events`, `Reads / writes`, `Tool calls`, `Embeddings`, `Files`) rather than inventing a new color.

## File structure

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 W H">
  <defs><!-- markers, style, symbols --></defs>
  <rect id="background" width="100%" height="100%" fill="#0E0E10"/>
  <g id="title">...</g>
  <g id="inputs">...</g>
  <g id="core-runtime">...</g>
  <g id="services">...</g>
  <g id="data">...</g>
  <g id="integrations">...</g>
  <g id="connectors">...</g>
</svg>
```

Classes in `<style>` instead of repeating inline attributes:

```css
.panel { fill:#1A1A1D; stroke:#FFF; stroke-width:2.5; }
.chip  { fill:#0E0E10; stroke:#3F3F46; stroke-width:2; }
.title { fill:#FFF; font:600 30px Inter,system-ui,sans-serif; }
.label { fill:#FFF; font:500 20px Inter,system-ui,sans-serif; }
.muted { fill:#A1A1AA; font:400 17px Inter,system-ui,sans-serif; }
.flow  { fill:none; stroke:#FFF; stroke-width:2.5; }
```

## Primitives

```svg
<rect class="panel" x="60" y="220" width="380" height="150" rx="14"/>
<rect class="chip" x="90" y="300" width="110" height="44" rx="10"/>
<rect x="510" y="425" width="1760" height="150" rx="18"
      fill="none" stroke="#FFF" stroke-width="2.5" stroke-dasharray="10 9"/>
<path class="flow" d="M440 295 H510" marker-end="url(#arrow)"/>
<g id="provider-openai">
  <rect x="100" y="100" width="44" height="44" rx="10"
        fill="#000" stroke="#3F3F46" stroke-width="1.6"/>
  <!-- icon path centered in the tile -->
</g>
```
