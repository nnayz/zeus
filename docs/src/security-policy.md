# Security policy

## Supported versions

Security fixes target the latest Zeus release. Keep Zeus and the coding-agent
CLIs it launches current.

## Report a vulnerability

Email **[hi@nasrul.info](mailto:hi@nasrul.info)** with the subject line
`Zeus security`.

Include:

- affected Zeus version and macOS version
- minimal reproduction
- expected impact
- any suggested mitigation

Do not attach private terminal output, tokens, or personal paths unless they are
strictly required to understand the issue — and mark that mail as confidential.

Acknowledgement should arrive within seven days. Fix and disclosure timing
depend on severity and complexity.

## Scope

Zeus intentionally runs local shells, coding agents, MCP tools, and optional
remote commands. A tool doing something the operator explicitly authorized is
not itself a Zeus vulnerability.

In scope:

- permission-boundary bypasses
- unsafe update or IPC behavior
- credential disclosure
- session isolation failures
- unintended remote execution

See the [security model](security-model.md) for trust boundaries.
