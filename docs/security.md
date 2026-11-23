# Security model

What pepino protects, what it does not, and what you must keep secret.
pepino is experimental, builds on [p2panda](https://p2panda.org) 0.8, and has
had no security audit. Do not rely on it against a capable attacker.

## What is protected

- **Board contents are encrypted between members.** Tasks, comments, and tags leave
  a device only as ciphertext that members' devices can read. Relay servers
  and always-on relays cannot.
- **Each device has its own keys.** They never leave the device. Every change
  is signed by the device that made it, and other devices reject forgeries.
- **Membership is invite-only.** A device reads a board only after a manager
  grants it access, and a role limits what its changes can do.

## What is not protected

- **Local data is plain.** The task database on each device is not encrypted,
  and the keys sit beside it in files only your user can read. Use disk
  encryption.
- **Metadata is visible.** Whoever carries or observes the traffic sees which
  nodes sync a board, when, and how much, though not what.
- **Removal is not retroactive.** A removed person or device keeps everything
  it already received. Removal changes the keys, so it protects only what
  comes after. A person whose removal card is still in flight or was refused
  after the remover lost Manage access can enroll another device by revoking
  the removed device in their own roster until an effective removal card applies.
- **Leaving is local.** Leave and forget delete the board's tasks and task
  history from the app database and stop syncing it. See
  [what stays on the device](sharing.md#what-stays-on-the-device), including
  encrypted operations, keys, and earlier catalog entries. A manager must
  remove you for other members to stop sharing with your devices.
- **History is all or nothing.** A new member or device reads the whole board
  history.
- **Names prove nothing.** People choose their own display names. The
  fingerprint check during an invitation is the only proof of who you add.

## Keep board IDs and codes secret

A board's full ID is a secret. Anyone who learns it can download the board's
ciphertext and metadata, though not read the contents. **A board ID cannot be
changed.** If one leaks, the only remedy is a new board.

- The short ID that `ppn board ls` prints is safe to show. Full IDs appear in
  `--json` output: keep that out of logs, bug reports, and shared files.
- A grant code contains board IDs, and a request code contains a one-time
  secret. Send them over a private channel, pass them on stdin or from a file
  ([scripting](scripting.md#keep-secrets-out-of-arguments)), and delete them
  after use.

## What a relay operator sees

An [always-on relay](relay.md) stores ciphertext for the boards handed to it.
Its operator learns each board's ID, the size and timing of its traffic, and
which nodes sync it. The operator cannot read tasks, but can delay or drop
data, and still knows the board ID after you take the board off the relay.

The public relay servers that help devices connect see network addresses and
timing, not board contents.

## Known limits

- A stolen first device means
  [a new identity](devices.md#protect-the-first-device).
- Two managers who remove each other at the same moment
  [destroy the board](sharing.md#change-a-role-or-remove-someone).
- A device revoked while offline may never learn of it.
- The daemon's control socket trusts every process running as your user.
