# Command line

The packaged app ships `taskmaster` next to the Engine. After a cask or DMG
install it should already be on your `PATH`. `taskmaster --help` prints the
map:

```text
taskmaster <resource> <action> [target] [options]
```

`--json` is accepted on the session, worktree, events, and artifacts
commands when you want to script.

## Doctor

```sh
taskmaster doctor
```

Checks that the Engine is reachable, that `claude` and `codex` are on
`PATH`, and that the session state file exists. Start here when the app
cannot connect or a spawn fails.

The version string to include in a support mail is the one in the account
popover (and on the update row).

## Sessions

```sh
taskmaster session list
taskmaster session list --all
taskmaster status                          # alias of session list --all

taskmaster session get <id>
taskmaster session read <id> --source scrollback --lines 80
taskmaster session send <id> "run the tests again"
taskmaster session send <id> "looks good" --no-submit
taskmaster session wait <id> --until done --timeout 600

taskmaster session spawn claude-code --cwd ~/src/mldrills --worktree \
  --title auth-rewrite \
  --prompt "Implement the plan in AGENTS.md. Stop at tests."
taskmaster session spawn codex --host forge --cwd ~/code/mldrills

taskmaster session archive <id>
taskmaster session archive <id> --undo
taskmaster session release <id>            # kill the process tree, keep the row
taskmaster session release <id> --remove
```

`wait --until` accepts `done`, `idle`, `working`, `starting`,
`needs-input`, and `exited`. Default timeout is 10 minutes.

Spawn kinds are catalog ids: `claude-code`, `codex`, `cursor`, `grok`,
`opencode`, `gemini`, `shell`, and the rest `taskmaster doctor` lists.

## Worktrees

```sh
taskmaster worktree list --repo ~/src/mldrills
taskmaster worktree create --repo ~/src/mldrills --branch spike-auth --base main
taskmaster worktree remove --repo ~/src/mldrills --path /path/to/worktree
```

## Artifacts, events, ports

```sh
taskmaster artifacts <id>
taskmaster ports
taskmaster events subscribe --session <id>
taskmaster events wait --session <id> --until needsInput
```

## MCP and hooks

```sh
taskmaster mcp-tools
taskmaster mcp-call --tool list_agents
```

`mcp-stdio` is the server the app injects into hosted agents. You rarely
need to run it yourself.

`taskmaster hook` and `taskmaster notify` are fail-open forwarders for Claude hooks
and Codex notify. They exist so those CLIs can poke Taskmaster. They are not
an interactive UI.

## A tiny conductor script

```sh
id=$(taskmaster session spawn grok --cwd "$PWD" | awk '{print $1}')
taskmaster session wait "$id" --until done --timeout 300
taskmaster session read "$id" --source scrollback --lines 40
```

Pair this with [Orchestration](orchestration.md) when the lead is another
agent, and with [Fleet patterns](fleet.md) when you are the lead.
