# Documentation

User-facing docs are an [mdBook](https://rust-lang.github.io/mdBook/) under
`docs/`.

```sh
# from repo root
brew install mdbook             # or: cargo install mdbook --locked
mdbook serve docs --open        # http://localhost:3000
mdbook build docs               # output in docs/book/
```

| Book page | Source |
|-----------|--------|
| Introduction | [`src/introduction.md`](src/introduction.md) |
| Getting started | [`src/getting-started.md`](src/getting-started.md) |
| Remote hosts | [`src/remote-hosts.md`](src/remote-hosts.md) |
| Remote nodes | [`src/remote-nodes.md`](src/remote-nodes.md) |
| Updates | [`src/updates.md`](src/updates.md) |
| Security model | [`src/security-model.md`](src/security-model.md) |
| Security policy | [`src/security-policy.md`](src/security-policy.md) |
| Privacy | [`src/privacy.md`](src/privacy.md) |
| Support | [`src/support.md`](src/support.md) |
| Roadmap | [`src/roadmap.md`](src/roadmap.md) |

Engineering notes stay next to the code (`zeus/PACKAGING.md`, `zeus/PORT.md`,
`zeus/REMOTE_PORT.md`, `zeus/PERF.md`).
