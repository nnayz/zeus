# Documentation

Taskmaster has two guides. Only one of them is a website.

| Guide | Who it is for | Source | Published |
|-------|----------------|--------|-----------|
| User guide, including playbooks | Installing and operating Taskmaster | `docs/src/` | [nnayz.github.io/taskmaster/docs/](https://nnayz.github.io/taskmaster/docs/) |
| Developer guide | Building, testing, and maintaining Taskmaster | `docs/dev/src/` | GitHub only. Not on the website. |

Playbooks are user-facing pages under the same `/docs/` base, at
`/docs/playbooks/`. They are not a separate site. The website in `website/`
renders both from `docs/src/SUMMARY.md`. There is no mdBook and no Fumadocs
build.

## Where a new page goes

Put operator instructions in `docs/src/` and add them to `docs/src/SUMMARY.md`.
That includes CLI commands and advanced remote workflows when they explain
how to use Taskmaster. Task write-ups with prerequisites, steps, expected results,
and pitfalls go under `docs/src/playbooks/` and in the Playbooks section of
`SUMMARY.md`.

Put contributor instructions in `docs/dev/src/` and add them to
`docs/dev/src/SUMMARY.md`. That includes the workspace map, manifests, tests,
packaging, and pointers at Engine, Holder, and Companion internals. Do not add
those pages to the website.

Do not copy an architecture decision into either guide when a canonical file
already exists. Link it from `docs/dev/src/engineering-index.md` instead.
`taskmaster/REMOTE_PORT.md`, `taskmaster/PACKAGING.md`, `taskmaster/UPDATING.md`, and the
Companion specs are the sources of truth for their subjects.
`taskmaster/PORT.md` and `taskmaster/PLAN.md` are historical. Say so if you cite them.

If a user-guide URL must change, add a redirect in `website/scripts/docs.mjs`
and repair links. Leaving the old chapter in `docs/src/SUMMARY.md` puts it
back into the website navigation and search.

## Preview and check

Node.js 22 or newer. From the repository root:

```sh
cd website
npm test
npm run dev
```

User guide: <http://127.0.0.1:4177/docs/>. Playbooks:
<http://127.0.0.1:4177/docs/playbooks/>.

GitHub Pages publishes `website/dist` with `BASE_PATH` set, so the same tree
is served at `https://nnayz.github.io/taskmaster/docs/`.

## User guide chapters

| Section | Source |
|---------|--------|
| Introduction | [`src/introduction.md`](src/introduction.md) |
| Getting started | [`src/getting-started.md`](src/getting-started.md) |
| The workbench | [`src/workbench.md`](src/workbench.md) |
| Keyboard | [`src/keyboard.md`](src/keyboard.md) |
| Agents and status | [`src/agents.md`](src/agents.md) |
| Worktrees | [`src/worktrees.md`](src/worktrees.md) |
| Orchestration | [`src/orchestration.md`](src/orchestration.md) |
| Fleet patterns | [`src/fleet.md`](src/fleet.md) |
| Settings | [`src/settings.md`](src/settings.md) |
| Command line | [`src/cli.md`](src/cli.md) |
| Remote hosts | [`src/remote-hosts.md`](src/remote-hosts.md) |
| Remote nodes | [`src/remote-nodes.md`](src/remote-nodes.md) |
| iOS Companion | [`src/ios-companion.md`](src/ios-companion.md) |
| Updates | [`src/updates.md`](src/updates.md) |
| Playbooks | [`src/playbooks/`](src/playbooks/) |
| Security model | [`src/security-model.md`](src/security-model.md) |
| Security policy | [`src/security-policy.md`](src/security-policy.md) |
| Privacy | [`src/privacy.md`](src/privacy.md) |
| Support | [`src/support.md`](src/support.md) |
| Roadmap | [`src/roadmap.md`](src/roadmap.md) |

Companion architecture and the screenshot soak test are developer pages:
[`dev/src/companion-architecture.md`](dev/src/companion-architecture.md) and
[`dev/src/testing.md`](dev/src/testing.md). Old user-guide URLs redirect to
GitHub.
