# Boards

A board is a private, encrypted space for tasks. Each board belongs to your identity and
its linked devices. The first board you create with `ppn init` is the starting board on
each newly linked device.

```sh
ppn board new Workshop
ppn board ls
ppn board show Workshop
ppn board rename Workshop 'Tool Shed'
ppn board use 'Tool Shed'
```

`board new` starts the sync daemon if needed and automatic startup is enabled. It keeps
your current board selected. When another device needs to supply fresh membership keys,
it does so the next time its daemon connects. The board list shows the host's current
board with `*`, open and total task counts, this device's membership, and sync activity.
New board names and renames appear on linked devices as they sync. `board use` changes
only the host on which you run it. `ppn status` and `ppn device ls` show membership by board.

Board names are case-sensitive labels. If a label is shared by several boards, Pepino
lists the candidates. Use the short hexadecimal ID shown by `ppn board ls` to select one.
You can use any unambiguous ID prefix of at least four hexadecimal characters.

## Select a board per project

Pepino reads `--board` first, then `PPN_BOARD`, then the host's current board. An empty
`PPN_BOARD` counts as unset. A missing or ambiguous reference produces an error that names
the source. `ppn board ls`, `ppn board show`, and `ppn status` show which board was
selected and why. `ppn board use` warns if the shell still sets `PPN_BOARD`.

Full board IDs are secrets. Keep them out of committed environment files and use a board
name or a short ID prefix. This direnv `.envrc` sets the project board by name.

```sh
export PPN_BOARD='Tool Shed'
```

```sh
ppn add 'Sharpen the spade'
ppn --board Greenhouse ls
ppn ls --all-boards --json
ppn ls 'board:"Tool Shed"' tag:maintenance
```

By default `ppn ls` lists tasks from the selected board. A `board:` filter can route a
listing to named boards, including through a saved view. An explicit selector narrows
the filter to that board. `--all-boards` starts with every board and adds a board column
or a `board` object to each JSON task. The usual status, sort, and limit options still apply.

Task ID prefixes resolve within the selected board. If an ID only appears elsewhere, the
error points to the matching board. A relation requires both tasks to belong to the same
board. Moving tasks between boards is unsupported.

Request and response examples are in the [board protocol reference](board-protocol.md).
