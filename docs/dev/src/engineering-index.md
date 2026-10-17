# Engineering index

Canonical engineering documents stay where they are. This page records who
they are for. "Historical" means the file is context for a completed port or
audit, not the spec to implement against.

| Document | Audience | Status |
|----------|----------|--------|
| `docs/` User Guide | Using Taskmaster | Current product manual |
| `docs/dev/` Developer Guide | Contributing and maintaining | Current contributor manual |
| `README.md` | Both; install plus pointers | Current repository entry |
| `SECURITY.md` | Reporting vulnerabilities | Pointer to the User Guide security policy and trust model |
| `PRIVACY.md`, `SUPPORT.md` | Users | Pointers into the User Guide |
| `DESIGN.md` | UI contributors | Current visual language. Not a feature list |
| `AGENTS.md` | Contributors and coding agents | Current repository instructions. Remote work follows `taskmaster/REMOTE_PORT.md` |
| `taskmaster/README.md` | Contributors | Desktop build notes, GPUI pin, `dev.sh` |
| `taskmaster/REMOTE_PORT.md` | Contributors changing remote sessions | **Authoritative** remote architecture. The transport refactor is complete |
| `taskmaster/PACKAGING.md` | Release engineering | **Authoritative** packaging |
| `taskmaster/UPDATING.md` | Release engineering | **Authoritative** updater implementation. User behavior is `docs/src/updates.md` |
| `taskmaster/COMPANION_GATEWAY.md` | Companion contributors | Gateway runbook and evidence. Not a production-availability claim |
| `taskmaster/COMPANION_TERMINAL.md` | Companion contributors | Terminal v1 command ownership |
| [Companion architecture](companion-architecture.md) | Companion contributors | Trust model and spike decision for the external API |
| `taskmaster/NODE.md` | Both | Pointer to the User Guide remote-nodes page |
| `ios/TaskmasterCompanion/README.md` | Companion client developers | How to run the SwiftUI client locally |
| `prototypes/companion-web/README.md` | Companion client developers | Synthetic fixture only |
| `taskmaster/assets/icon-assets/README.md` | Contributors changing the app icon | Icon Composer source and regeneration |
| `taskmaster/PERF.md` | Performance work | Measurements and the packaged acceptance gate. Older numbers are context |
| `taskmaster/PERF-AUDIT.md` | Performance work | Historical audit (2026-08-06), not a standing spec |
| `taskmaster/PORT.md` | Anyone reading git history | **Historical** Swift-to-Rust engine port |
| `taskmaster/PLAN.md` | Anyone reading git history | **Historical** GPUI client migration plan |

User Guide chapters and playbooks are for operators. They are listed in
`docs/src/SUMMARY.md`. Developer chapters are listed in
`docs/dev/src/SUMMARY.md`.

When a new canonical note is added under `taskmaster/`, link it here and say
whether it is current or historical. Do not also paste it into the User
Guide.
