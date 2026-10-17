# Documentation

Two guides, two audiences. Put a page in exactly one of them unless you are
deliberately linking across.

| Write about | Put it in | Source |
|-------------|-----------|--------|
| Installing, shortcuts, sessions, worktrees, orchestration, fleet use, settings, the `taskmaster` CLI as an operator, remote setup and day-to-day operation, Companion pairing and use, privacy, support | User guide | `docs/src/` |
| A task a user can finish with shipped behavior, including prerequisites, steps, expected results, and pitfalls | User guide, Playbooks | `docs/src/playbooks/` |
| Building, the crate map, manifests and MCP integration, Engine and Holder internals, tests, packaging, release engineering | Developer guide | `docs/dev/src/` |
| An architecture or protocol decision that already has a canonical file | That file. Link it from the [engineering index](engineering-index.md). Do not paste a second copy into either guide. | `taskmaster/*.md` and the Companion specs |

Classify by purpose. A CLI flag or an advanced remote workflow stays in the
user guide when it tells someone how to use Taskmaster. Move it here when it tells
someone how to change Taskmaster.

## Where it is published

The website in `website/` publishes the user guide and the playbooks at one
base path, `/docs/`. Playbooks are `/docs/playbooks/…`, not a separate site
and not a separate `/guides/` tree. `docs/src/SUMMARY.md` is the order.

This developer guide is Markdown on GitHub only. Do not add it to the website
nav or to `SUMMARY.md`. `docs/dev/src/SUMMARY.md` is the reading order in the
repository, not a published book.

There is no mdBook and no Fumadocs build. Do not reintroduce either for the
docs site.

## Rules

- Keep user-guide URLs stable. A chapter slug in `docs/src/SUMMARY.md` becomes
  `/docs/<slug>/`. The introduction is `/docs/`.
- When a user page has to move, add a redirect in `website/scripts/docs.mjs`
  and fix links. Do not leave the old chapter in `docs/src/SUMMARY.md`, or it
  stays in the website navigation and search.
- Do not copy developer chapters into `docs/src` to "make the link work."
  Link to the GitHub file.
- Mark historical port plans as historical. `taskmaster/PORT.md` and `taskmaster/PLAN.md`
  are records, not current design. See [Historical documents](historical.md).
- User-facing update behavior stays in the user guide
  ([Updates](../../src/updates.md)). Updater implementation stays in
  `taskmaster/UPDATING.md`.
- Playbooks describe shipped behavior only. A future feature does not get a
  playbook until users can do the steps.
- Do not log credentials, prompts, or full environments in examples.

## Preview and check

From the repository root:

```sh
cd website
npm test
npm run dev
```

Requires Node.js 22 or newer. No install step. The site is at
<http://127.0.0.1:4177/>, the user guide at <http://127.0.0.1:4177/docs/>, and
playbooks at <http://127.0.0.1:4177/docs/playbooks/>.

`npm test` fails if a user chapter is missing from `SUMMARY.md`, if a
playbook leaves `/docs/`, or if a developer page is emitted as a site
chapter. GitHub Pages sets `BASE_PATH` (usually `/taskmaster`) at deploy time so
the same `/docs/` tree is served under the project site.

## Audience of the current guides

The chapter list in `docs/src/SUMMARY.md` is the website navigation. The
chapter list in `docs/dev/src/SUMMARY.md` is the GitHub reading order. The
audit of repository documents that are not chapters is the
[engineering index](engineering-index.md).
