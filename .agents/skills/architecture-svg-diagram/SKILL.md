---
name: architecture-svg-diagram
description: Create polished, dark-mode software architecture diagrams as hand-authored SVGs. Use for system, agent, infrastructure, data-flow, RAG, platform, service, deployment, integration, or component diagrams when the user wants a premium technical visual, a README/docs/landing/pitch SVG, or a diagram "like" a supplied architecture graphic. Use when the user runs /architecture-svg-diagram.
---

# Architecture SVG Diagram

Produce a standalone, hand-authored `.svg` — a designed product graphic, not a generic flowchart.

Before drawing, read [references/visual-system.md](references/visual-system.md) and copy [templates/skeleton.svg](templates/skeleton.svg). Reuse that visual language only. Do not copy a reference diagram's architecture, logos, composition, or proprietary artwork.

## Output

The `.svg` must open in a browser, scale without quality loss, and stay editable in Figma, Illustrator, Inkscape, or a code editor. It uses semantic `<g id="...">` groups, needs no JavaScript, remains understandable without external fonts, embeds no raster screenshots unless asked, keeps connectors/cards/labels/icons aligned, matches the user's architecture, and stays readable at normal README width.

If the user gives rough notes, infer hierarchy without inventing major system behavior. Make layout decisions instead of blocking on minor ambiguity. Never substitute Mermaid or a bitmap when the user asked for a designed SVG.

## Workflow

1. **Parse.** Build `nodes` (`id`, `title`, `type`, `parent/group`, `technologies`) and `edges` (`source`, `target`, `direction`, `label`, `relationship`). Resolve duplicate names. Separate architectural components from implementation technologies.

2. **Hierarchy.** Classify only the levels that exist: L0 external actors, L1 entry/access, L2 orchestration/runtime, L3 services/agents, L4 storage/integrations, L5 outputs/infrastructure.

3. **Layout.** Simplest correct layout: columns for sequence, rows for layers, dashed groups for ownership/runtime/trust boundaries, nested cards only for containment. One dominant reading direction — left→right or top→bottom — not both repeatedly.

   Default:

   ```
   External Inputs
     → API / Gateway / Auth
     → Core Runtime / Orchestrator
         → Agents / Services / Workers
         → Data / Storage / Integrations
     → Outputs / Notifications
   ```

   Wide alternative: `Inputs → Core Platform → Execution → Data / Integrations`, infrastructure on a lower row.

4. **Size from content.** Width from title length, chip count, provider tiles, and importance. Do not force identical widths if that cramps content. Peer cards share height; rows share Y; columns share X. Use the spacing grid in the visual system — no 3–7px eyeballed offsets.

5. **Connectors before details.** Place card rectangles, then major routes, then chips and icons. Connect to visual centers or clear edge ports.

6. **Details.** Add only what improves understanding: section icons, titles, chips, integration tiles, short edge labels, small annotations. A major card is one title, one icon, optionally 1–8 chips/tiles, optionally one short subtitle. No paragraphs inside cards. If more than ~12–15 integrations sit in one category, group or label the category instead of every logo.

7. **QA.** Inspect and fix overlaps, clipped labels, arrows behind cards, inconsistent radii or strokes, weak contrast, unexplained empty space, wrong flow direction, and crossings that layout can remove.

   A viewer should answer without a caption: where interaction enters; the core runtime; which components do the work; which systems store state; which externals are integrated; the major flows; which components belong together. If those are not obvious, fix hierarchy before adding decoration.

## Technology forms

| Form | When | Example |
|---|---|---|
| Major panel | structurally important | API Gateway, Agent Runtime, Vector Store |
| Chip | implementation choice | FastAPI, Postgres, OAuth |
| Icon tile | recognition helps | GitHub, Slack, AWS |

Do not reduce core concepts to logo-only tiles.

## Domain patterns

**Agents.** Distinguish user, app/API, orchestrator, agents/workers, model providers, tools, memory/state, retrieval, files, and integrations. An LLM is a dependency of the runtime, not the whole agent.

**RAG.** Separate ingestion from query-time retrieval. If ingestion is async, show that.

```
Documents → Parse → Chunk → Embed → Vector DB
User → Query → Embed → Retrieve → Rerank → LLM → Answer
```

**Events.** Label sync vs async paths, or use dashed connectors. Do not encode the difference with color alone.

**Storage.** Label source of truth vs cache vs vector index vs object store vs ephemeral state when the distinction matters.

## Delivery

1. Save as a descriptive filename (`architecture.svg`).
2. Valid XML. Inspect once; fix clipping and overlap.
3. `.svg` is the primary output. PNG preview only if useful or requested.
4. A supplied reference image is visual inspiration unless the user owns it and asks for a faithful recreation.
