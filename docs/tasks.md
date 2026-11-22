# Work with tasks

Create, change, connect, and find tasks. Every command here works offline and
acts on the [selected board](boards.md). Task arguments accept any unambiguous
ID prefix of four or more characters.

## Add a task

```sh
ppn add 'Paint the shed door' --tag paint --priority 2 --due fri \
  --description 'Two coats, exterior green.' --set estimate=3
```

Only the title is required. `--due` takes `YYYY-MM-DD`, `today`, `tomorrow`,
a weekday, or an offset such as `+3d` or `+1w` (`ppn help dates`). `--priority`
is any integer. `--set KEY=VALUE` stores a custom field. Pass `-` as the title
or the description to read it from stdin.

## Change a task

```sh
ppn show fde8
ppn edit fde8 --title 'Replace the latch' --due none --priority 1
ppn edit fde8 --unset priority --untag hardware
ppn edit fde8
```

`ppn show` prints a task with its description, links, and comments.
`ppn edit` with flags changes only what you name. Without flags, on a
terminal, it opens the task in `$VISUAL` or `$EDITOR`.

## Move a task through its statuses

```sh
ppn start fde8        # todo -> doing
ppn done fde8 db19    # several tasks at once
ppn cancel 4ec3
ppn reopen db19       # back to todo
```

A task is `todo`, `doing`, `done`, or `cancelled`. The first two are open,
the last two closed.

`ppn rm fde8 --yes` deletes a task for good, on every device.

## Tag, assign, and comment

```sh
ppn tag fde8 hardware outdoor
ppn untag fde8 outdoor
ppn assign fde8 me
ppn unassign fde8 me
ppn comment fde8 'Use stainless hardware.'
```

Assign `me`, or anyone on the board by display name. When two people share a
name, the error lists a `name#suffix` form for each. `ppn comment fde8 -`
reads the text from stdin and `--from-file` reads a file.

## Break work down and order it

```sh
ppn add 'Buy paint' --parent bbf0 --blocked-by db19
ppn link db19 blocks fde8
ppn link 07d2 relates 295f
ppn unlink db19 blocks fde8
ppn edit 295f --no-parent
```

A task can have one parent, and any number of `blocks` and `relates` links.
Both tasks must be on the same board.

```console
$ ppn tree
bbf0bc todo  Paint the shed door  #paint p2
  07d232 todo  Sand the door
  295f23 todo  Buy paint  blocked
$ ppn deps fde8
blocked by
  db19ac todo  Order stainless screws  #hardware due:2026-11-27
$ ppn ready
```

`ppn ready` lists `todo` tasks with no open blocker: what you can start now.
A task stops being blocked when its blocker is done, cancelled, or deleted.

## List, search, and filter

`ppn ls` lists the open tasks. Add terms to narrow the list; all terms must
match.

```sh
ppn ls tag:hardware                  # by tag
ppn ls status:doing assignee:me
ppn ls 'due:<=fri' -tag:paint        # comparison, and - to negate
ppn ls tag:hardware,garden           # comma means "or"
ppn ls latch                         # text in titles, descriptions, comments
ppn ls is:blocked
ppn ls --all 'estimate:<=3'          # custom field; --all includes closed tasks
ppn ls --sort due --limit 2
```

Quote any term containing `<`, `>`, or spaces. `ppn help filters` lists every
key (`status`, `due`, `priority`, `parent`, `has`, `is`, and more). An unknown key
returns exit status 2 and suggests a spelling.

`--sort` takes `created`, `updated`, `due`, `priority`, `status`, or `title`,
comma-separated; prefix a key with `-` to reverse it.

## Save a view

```console
$ ppn view save week is:open 'due:<=+1w'
Saved week
$ ppn ls @week tag:garden
f52bbb todo  Water the tomatoes  #garden due:2026-11-23
$ ppn view ls
week  is:open due:<=+1w
```

A view is a named filter. Use it as `@name`, alone or with more terms.
Relative dates are evaluated each time you use it. Saved views follow you to
all your devices; if you change one on two devices, the later change wins.
`ppn view rm week` deletes it everywhere.

## Read the history

```sh
ppn log fde8          # one task
ppn log --limit 10    # the newest ten events on the board
```
