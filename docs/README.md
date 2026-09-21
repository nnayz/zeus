# Zeus docs

User-facing documentation is a [Fumadocs](https://fumadocs.dev) app in `docs/`.
It is a separate Next.js site so it does not share styles or deploys with the
marketing site in `site/`.

```sh
cd docs
npm install
npm run dev          # http://localhost:3000/docs
npm run build        # production build
```

Pages live in `content/docs`. The folder structure defines the URLs. Every page
needs a `title` in its frontmatter. `meta.json` controls sidebar order.

Do not edit the `.source/` folder. Fumadocs MDX generates it.

Site name and GitHub info are in `lib/shared.ts`. Layout options are in
`lib/layout.shared.tsx`.

Deploy this app as its own project (for example Vercel at
[docs.zeus.nasrul.info](https://docs.zeus.nasrul.info)). Keep the default `/docs`
route so a subdomain or a `/docs/*` proxy both work.
