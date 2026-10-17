# Taskmaster website

Static site adapted from the [Diri website](https://github.com/cristicretu/diri)
(Apache-2.0). The user guide and the playbooks are generated from `docs/src/`
and published under the same `/docs/` base. Developer pages in `docs/dev/src/`
stay on GitHub and are not part of the build.

## Preview

From this directory, with Node.js 22 or newer:

```sh
npm test
npm run dev
```

Open http://127.0.0.1:4177. Docs: http://127.0.0.1:4177/docs/. Playbooks:
http://127.0.0.1:4177/docs/playbooks/. There is no install step.

`npm test` checks chapter coverage, playbook URLs, download parsing, and that
developer chapters are not emitted. `npm run build` writes `dist/`. GitHub
Pages sets `BASE_PATH` and `SITE_ORIGIN` so the same files work under
`/taskmaster/`.

## What is generated

| Pages | Source |
| --- | --- |
| `/`, `/install/`, `/security/`, `/releases/` | `pages/` and `releases.json` |
| `/docs/` and `/docs/playbooks/` | `docs/src/` in `docs/src/SUMMARY.md` order |

Adding a user chapter means adding the Markdown file and a `SUMMARY.md` link.
Playbook files live in `docs/src/playbooks/` and use the same renderer and the
same `/docs/` prefix.
