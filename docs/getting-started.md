# Get started

Create your identity and first board, add a few tasks, and learn how sync
works. You need `ppn` on your `PATH`; see the [README](../README.md#install).

## Create your identity and first board

```sh
ppn init --name 'Ada Gardener' --device laptop --board Home
```

All three flags are optional. They default to your user name, the hostname,
and `Personal`. `init` creates three things and prints a summary of them:

- an **identity**: you, under the name other people will see;
- a **device**: this machine, with its own keys;
- your first **board**: a private, encrypted list of tasks.

This machine is now your **first device**. It is the only one that can revoke
your other devices, so [keep it](devices.md#protect-the-first-device). Your
first board stays private to your devices; to work with other people,
[create another board](sharing.md).

## Add and finish tasks

```console
$ ppn add 'Replace the greenhouse latch' --tag hardware --due tomorrow
Created fde839  Replace the greenhouse latch
$ ppn add 'Order screws'
Created db19ac  Order screws
$ ppn ls
fde839 todo  Replace the greenhouse latch  #hardware due:2026-11-23
db19ac todo  Order screws
$ ppn start fde8
Started fde839  Replace the greenhouse latch
$ ppn done db19
Done db19ac  Order screws
```

Refer to a task by any unambiguous prefix of its ID, at least four characters
long. `ppn ls` lists open tasks; `ppn ls --all` includes finished ones.
[Work with tasks](tasks.md) covers the rest.

## Understand sync

Every task command reads and writes a local database, so `ppn` works offline
and answers at once. A background daemon syncs your boards with your other
devices and with the people you share boards with. Your first write starts it;
you do not need to manage it. Devices find each other on the local network,
or across the internet through relay servers.

```sh
ppn status    # identity, boards, peers, pending events
ppn sync      # wait until pending changes are exchanged
```

See [The sync daemon](daemon.md) to run it under a service manager or turn
automatic startup off.

## Next steps

- [Link your devices](devices.md) to use the same boards on another machine.
- [Share a board](sharing.md) with other people.
- `ppn help` and `ppn <command> --help` describe every command and flag.
