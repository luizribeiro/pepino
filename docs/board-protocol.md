# Board additions to application protocol v1

The existing v1 JSON and CBOR envelopes accept optional `board` and `all_boards` fields.
A board reference can be an exact name, a full ID, or a unique hexadecimal ID prefix
of four or more digits. Omitted `board` selects the host's current board. The default
for `all_boards` is false. This field applies to task-listing requests.

```json
{"v":1,"board":"Workshop","request":{"op":"add_task","title":"Sharpen the spade"}}
{"v":1,"all_boards":true,"request":{"op":"list_open_tasks","filter":[],"today":"2026-11-20"}}
{"v":1,"request":{"op":"board_list"}}
{"v":1,"request":{"op":"board_show","board":"515151"}}
{"v":1,"request":{"op":"board_use","board":"Workshop"}}
{"v":1,"request":{"op":"board_rename","board":"Workshop","name":"Tool Shed"}}
```

`board_list` returns kind `boards` with a `boards` array. Show, use, and rename return
kind `board` with a `board` object. Each entry contains `id`, `name`, `current`,
`open`, `total`, and `membership`. Membership is `waiting_to_be_added`,
`waiting_for_first_sync`, or `joined`. An omitted `board_show.board` uses the
invocation's selection. `board_use` saves the host preference. `board_rename` records
a catalog revision for the daemon to publish. A blank name yields `invalid_input`.

```json
{"v":1,"result":{"ok":{"kind":"board","board":{"id":"5151515151515151515151515151515151515151515151515151515151515151","name":"Tool Shed","current":true,"open":3,"total":5,"membership":"joined"}}}}
```

A listing that spans boards or uses `all_boards` adds a `board` object
with its full `id` and `name` to each task. Ambiguous board lookups yield the
`ambiguous_board` app error shape with `field`, `board`, and `candidates`. The CLI
uses the stable `ambiguous_id` error code. A missing task with matches elsewhere has
`task_on_other_boards` containing the reference and `alternate_boards`. A cross-board
relation yields `invalid_input`.

Creation uses the daemon's same-user socket. The node creates an encrypted space and
the daemon attaches the returned handle to its sync engine.

```json
{"v":1,"request":{"op":"create_board","name":"Workshop"}}
{"v":1,"result":{"kind":"board_created","board":"5151515151515151515151515151515151515151515151515151515151515151"}}
```

Daemon status includes observed device membership by board in `board_members` and
per-board activity in `boards`. The CLI adds `selected_board` to `board ls`,
`board show`, and `status`, with `id`, `name`, and `source` (`--board`,
`PPN_BOARD`, or `current`).
