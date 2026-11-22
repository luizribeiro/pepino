# pepino

pepino (`ppn`) is a local-first, peer-to-peer task manager for the terminal:
your tasks live on private, encrypted boards that sync directly between your
devices and the people you invite.

It is for people who want to manage tasks from the command line on several
devices, offline first, with no account and no server of their own.

**Status: experimental.** The sync protocol may change between versions, so
upgrade all your devices, and everyone on a shared board, together. Not
audited.

## Install

With Nix flakes:

```sh
nix profile install github:luizribeiro/pepino
```

From source, with Rust 1.96 or newer:

```sh
git clone https://github.com/luizribeiro/pepino
cd pepino
cargo build --release -p ppn    # the binary is target/release/ppn
```

For the NixOS and nix-darwin modules, see [docs/nix.md](docs/nix.md).

## Quick start

```console
$ ppn init --name 'Ada Gardener'
$ ppn add 'Replace the greenhouse latch' --tag hardware --due tomorrow
Created fde839  Replace the greenhouse latch
$ ppn ls
fde839 todo  Replace the greenhouse latch  #hardware due:2026-11-23
$ ppn done fde8
Done fde839  Replace the greenhouse latch
```

To use the same boards on a second device, pass two secret codes between the
devices over a private channel:

```sh
ppn device request > request.txt                    # on the new device
ppn device add --from-file request.txt > grant.txt  # on the first device
ppn device join --from-file grant.txt               # on the new device
```

The middle step asks you to confirm the new device's fingerprint; see
[Link your devices](docs/devices.md).

## Documentation

- [Get started](docs/getting-started.md): first board, first tasks, how sync works
- [Work with tasks](docs/tasks.md): edit, tag, link, search, saved views
- [Use several boards](docs/boards.md)
- [Link your devices](docs/devices.md)
- [Share a board](docs/sharing.md)
- [Run an always-on relay](docs/relay.md)
- [The sync daemon](docs/daemon.md)
- [Install on NixOS and macOS with Nix](docs/nix.md)
- [Scripting and configuration](docs/scripting.md)
- [Security model](docs/security.md)
- Reference: [protocol notes](docs/board-protocol.md), [hacking on pepino](docs/hacking.md)

## Licence

[MIT](LICENSE.md)
