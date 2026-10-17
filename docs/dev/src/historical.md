# Historical documents

These files stay in the tree so the port can be audited. They are not the
spec for new work. If one of them conflicts with `taskmaster/REMOTE_PORT.md` or
with the code, it does not win.

| Document | What it was | What to use instead |
|----------|-------------|---------------------|
| [`taskmaster/PLAN.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/PLAN.md) | GPUI client migration plan, including a Swift daemon that this plan did not rewrite | `taskmaster/README.md` and the [workspace map](workspace.md). The Rust Engine is the daemon |
| [`taskmaster/PORT.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/PORT.md) | Record of replacing the retired Swift engine | Current behavior in the crates. Remote transport in `taskmaster/REMOTE_PORT.md` |
| [`taskmaster/PERF-AUDIT.md`](https://github.com/nnayz/taskmaster/blob/main/taskmaster/PERF-AUDIT.md) | Point-in-time performance audit on 2026-08-06 | `taskmaster/PERF.md` for the measurement record, tests for acceptance |

`taskmaster/README.md` calls `PLAN.md` the historical record of the port from the
retired Swift client. Comments in the code about Swift compatibility are the
same kind of history. They do not add requirements.

`taskmaster/PERF.md` mixes a current packaged gate with older measurements. Read
the status note at the top of that file before quoting a number. The audit
file does not.

The Companion architecture page keeps its own decision record, including
notes about what the spike did not approve. That page is current for the
Companion contract. It is not a historical port plan. Do not "clean it up"
into this table.
