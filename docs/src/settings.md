# Settings

`⌘,` or **Settings** in the command palette. Four tabs.

## General

**Default agent.** What `⌘T` and a folder result in Quick Open launch. Used by
the sidebar's primary New Agent hint as well.

**Projects sidebar on the right.** Mirrors the chrome: projects on the
trailing edge, inspector on the leading one.

**Word wrap.** Soft-wraps long lines in inspector Review and Code.

**Start Zeus at login.** Opens the app after you sign in to macOS. Sessions
were already alive (the Engine outlives the window). This just puts the
glass in front of you.

**Confirm before closing a session.** Asks before you kill a running
process with `⌘W`.

**Gentle status chimes.** Quiet sounds for needs-input, completion, and
memory pauses. Working sessions stay silent.

**Updates.** Background checks about 20 seconds after launch, then every
six hours. Download and restart always wait for a click. See
[Updates](updates.md).

**Quick Open roots.** Extra entries for Quick Open's Projects & Folders
section, one per line and scanned four levels deep. Empty means the default
folder plus parents of open projects.

## Terminal

**Color theme.** Applies to the app chrome and every open terminal at
once. The catalog includes Zeus Dark (default), Zeus Light, Solarized,
Dracula, One Dark, Gruvbox, Tokyo Night, Catppuccin, Vesper, Nord,
Rosé Pine, Kanagawa, Everforest, GitHub Light, and friends.

**Font size.** Terminal text only, 10 to 20 pt. `⌘+` / `⌘-` / `⌘0` do the
same from the keyboard.

## Resources

**Hibernate idle sessions.** Off, 5 minutes, 15 (default), 30, or 1 hour.
A frozen session is not killed. Opening it wakes the same process at the
same screen.

**Memory limit.** 2, 4, 6 (default), or 8 GB per session. Crossing it
freezes that session the same way.

Use these when the fleet is large and the laptop is small. They are how
overnight runs stay polite.

## Remote

Add, edit, or remove SSH execution hosts here. That is the supported path.
The on-disk catalog is
`~/Library/Application Support/Zeus/hosts.json` if you prefer to edit JSON.

Each host has a name, an SSH destination (`you@forge` or an `~/.ssh/config`
alias), a default remote cwd, and optional first-party node fields
(endpoint, token file, node id). See [Remote hosts](remote-hosts.md) and
[Remote nodes](remote-nodes.md).

`id` is generated from the name and then frozen, because sessions persist
it. Rename the label all you like. Do not expect to rewrite the id by
hand without confusing old rows.

## What is not a setting

The last selected session, sidebar width, window placement, and which
inspector tab you were on all persist automatically. You should not have
to re-tune the furniture every launch.
