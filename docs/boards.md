# Use several boards

Create boards to separate areas of work, and choose which board a command
acts on. A board is a private, encrypted set of tasks. New boards appear on
all [your devices](devices.md), and any board except your first can be
[shared](sharing.md).

## Create, rename, and list boards

```console
$ ppn board new Workshop
$ ppn board rename Workshop 'Tool Shed'
$ ppn board ls
* Home (003257)  4/6 open  joined
  Tool Shed (847796)  1/1 open  joined
Selected board: Home (003257) via current
```

`*` marks this device's current board. The columns after the name are open
and total tasks, your membership, and sync activity. Renames reach your
other devices as they sync.

Refer to a board by its name or by the short ID in parentheses. Names are
case-sensitive and need not be unique; if two boards share a name, use the
ID.

## Choose the board for a command

`ppn` picks the board from the first of these that is set:

1. `--board NAME` on the command line;
2. the `PPN_BOARD` environment variable;
3. this device's current board, set with `ppn board use NAME`.

```sh
ppn board use 'Tool Shed'
ppn add 'Sharpen the spade' --board Home
```

`ppn board use` changes only the device you run it on. To tie a board to a
project directory, export `PPN_BOARD` from a direnv `.envrc`:

```sh
export PPN_BOARD='Tool Shed'
```

Use the name there, never a full board ID:
[board IDs are secrets](security.md#keep-board-ids-and-codes-secret).

## List tasks across boards

```sh
ppn ls --all-boards
ppn ls 'board:"Tool Shed"' tag:maintenance
```

`--all-boards` adds a board column to each row. A `board:` filter term selects
a board by name and also works inside a [saved view](tasks.md#save-a-view).

Task IDs resolve within the selected board, and links join tasks on the same
board only. You cannot move a task to another board.

## Delete your copy of a board

Use `ppn board leave House` to [delete a board from your devices](sharing.md#leave-a-board-and-delete-your-copy).
Use `ppn board forget House` after a manager removes you. Both commands
require confirmation and remove the board from your private catalog. Leaving
revokes nothing. Only a manager can remove you from the shared board.
