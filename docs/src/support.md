# Support

Email **[hi@nasrul.info](mailto:hi@nasrul.info)** for help, bugs, feedback, or
security reports.

## Diagnostics

```sh
taskmaster doctor
```

Include with your email:

- Taskmaster version (the account popover, or Settings → General → Updates)
- macOS version and chip
- agent involved (if any)
- steps to reproduce
- a small, redacted log excerpt

Building Taskmaster from source, running tests, or filing a pull request? Start
with the [developer guide on GitHub](dev/introduction.html) instead.

## Logs

Daemon logs and session state:

```text
~/Library/Application Support/Taskmaster
~/Library/Application Support/taskmaster
```

These may contain prompts, command output, paths, or secrets printed by a
process. Redact before sending.
