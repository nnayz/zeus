# Command line

The packaged app ships `zeus` next to the Engine. After a cask or DMG
install it should already be on your `PATH`. `zeus --help` prints the
map:

```text
zeus <resource> <action> [target] [options]
```

`--json` is accepted on the session, worktree, events, and artifacts
commands when you want to script.

## Doctor

```sh
zeus doctor
```

Checks that the Engine is reachable, that `claude` and `codex` are on
`PATH`, and that the session state file exists. Start here when the app
cannot connect or a spawn fails.

The version string to include in a support mail is the one in the account
popover (and on the update row).

## Sessions

```sh
zeus session list
zeus session list --all
zeus status                          # alias of session list --all

zeus session get <id>
zeus session read <id> --source scrollback --lines 80
zeus session send <id> "run the tests again"
zeus session send <id> "looks good" --no-submit
zeus session wait <id> --until done --timeout 600

zeus session spawn claude-code --cwd ~/src/mldrills --worktree \
  --title auth-rewrite \
  --prompt "Implement the plan in AGENTS.md. Stop at tests."
zeus session spawn codex --host forge --cwd ~/code/mldrills

zeus session archive <id>
zeus session archive <id> --undo
zeus session release <id>            # kill the process tree, keep the row
zeus session release <id> --remove
```

`wait --until` accepts `done`, `idle`, `working`, `starting`,
`needs-input`, and `exited`. Default timeout is 10 minutes.

Spawn kinds are catalog ids: `claude-code`, `codex`, `cursor`, `grok`,
`opencode`, `gemini`, `shell`, and the rest `zeus doctor` lists.

## Worktrees

```sh
zeus worktree list --repo ~/src/mldrills
zeus worktree create --repo ~/src/mldrills --branch spike-auth --base main
zeus worktree remove --repo ~/src/mldrills --path /path/to/worktree
```

## Artifacts, events, ports

```sh
zeus artifacts <id>
zeus ports
zeus events subscribe --session <id>
zeus events wait --session <id> --until needsInput
```

## MCP and hooks

```sh
zeus mcp-tools
zeus mcp-call --tool list_agents
```

`mcp-stdio` is the server the app injects into hosted agents. You rarely
need to run it yourself.

`zeus hook` and `zeus notify` are fail-open forwarders for Claude hooks
and Codex notify. They exist so those CLIs can poke Zeus. They are not
an interactive UI.

## A tiny conductor script

```sh
id=$(zeus session spawn grok --cwd "$PWD" | awk '{print $1}')
zeus session wait "$id" --until done --timeout 300
zeus session read "$id" --source scrollback --lines 40
```

Pair this with [Orchestration](orchestration.md) when the lead is another
agent, and with [Fleet patterns](fleet.md) when you are the lead.
