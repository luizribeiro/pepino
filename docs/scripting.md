# Scripting and configuration

Drive `ppn` from scripts and other programs: JSON output, exit statuses,
secrets on stdin, and the settings that change its behaviour.

## Get JSON

Add `--json` to any command, or set `PPN_JSON=1`:

```console
$ ppn ls --json tag:hardware | jq -r '.tasks[] | "\(.id[0:6]) \(.title)"'
fde839 Replace the greenhouse latch
```

A successful command writes one JSON document to stdout, with a `schema`
version, a `kind`, full IDs, and UTC timestamps. Commands that change
something return what they changed, so `ppn add --json` gives you the new
task's ID.

Errors and warnings go to stderr, also as JSON:

```console
$ ppn show ffff --json
{"error":{"code":"not_found","message":"No task matches ffff", …},"kind":"error","schema":1}
```

`ppn help --json` describes every command, flag, exit status, and error code
in one document.

To create a task from JSON, pipe an object with at least a `title`
(`ppn help json` lists the other keys):

```sh
echo '{"title":"Water the tomatoes","due":"tomorrow","tags":["garden"]}' |
  ppn add --from-json -
```

## Check the exit status

| Status | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Failure, including a missing `--yes` and a `ppn sync` that timed out. |
| 2 | Invalid arguments or input. |
| 3 | Task, board, or person not found. |
| 4 | The ID or name matches more than one item. |
| 5 | Not permitted, for example writing with the `read` role. |
| 6 | The daemon or network could not be reached. |
| 7 | A join is saved and will complete later. |
| 8 | The data directory holds a device and cannot become a relay. |

For finer distinctions, read `error.code` in the JSON error.

## Run without prompts

`ppn` prompts only on a terminal. In a script, a command that needs
confirmation fails with status 1 unless you pass `--yes`. Commands that wait,
such as `sync` and the joins, take `--timeout 45s`.

## Keep secrets out of arguments

Request and grant codes are secrets, and command-line arguments are visible
to other users of the machine. Pass codes on stdin or from a file:

```sh
ppn device join - < grant.txt
ppn board invite --from-file request.txt --board House --fingerprint XXXX-XXXX-XXXX-XXXX
```

With `--json`, a request is in the `request` field and a grant in
`grant.code`. Do not log either.

## Configure ppn

`ppn` needs no configuration. To change a setting, create
`~/.config/pepino/config.toml`:

```toml
network = "pepino"        # devices sync only with the same network name
mdns = true               # find peers on the local network
# relay_urls = []         # no relay servers: local network only

[ui]
date_format = "%Y-%m-%d"
```

```sh
ppn config show     # every setting, its value, and where it comes from
ppn config path     # the files ppn reads
ppn config check    # validate them
```

A setting comes from the first place that sets it: a command-line flag, a
`PPN_*` variable, your config file, `/etc/pepino/config.toml`, then the
built-in default. `--config FILE` or `PPN_CONFIG` replaces both files.

| Variable | Effect |
| --- | --- |
| `PPN_BOARD` | Board to act on, by name or short ID. See [boards](boards.md#choose-the-board-for-a-command). |
| `PPN_JSON` | `1` for JSON output. |
| `PPN_CONFIG` | Config file to use instead of the user and system files. |
| `PPN_DATA_DIR` | Data directory. |
| `PPN_NETWORK` | Network name. |
| `PPN_NO_DAEMON` | `1` to never start the daemon automatically. |
| `PPN_COLOR` | `auto`, `always`, or `never`. `NO_COLOR` is honoured too. |

Every setting has a variable; `ppn help config --json` lists them all.
