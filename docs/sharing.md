# Share a board

Create a board for the work you want to share. Your first board is never
shareable. It carries your private catalog, and its history contains the IDs
that grant access to your other boards. Every new member receives the full
history of the board they join.

```sh
# Luiz creates the shared board.
ppn board new House

# Ana runs this on an initialized device.
ppn board request --name 'Garden tablet'

# Luiz reads Ana's request and verifies the fingerprint with her.
ppn board invite - --board House --role write --fingerprint FINGERPRINT < request.txt

# Ana reads Luiz's grant.
ppn board join - --timeout 45s < grant.txt
```

Exchange the request and grant through a private channel. Enter either code
from standard input or use `--from-file`. Use `--fingerprint` for a script
and compare it against the fingerprint shown on the requesting host.
Interactive invitations prompt for confirmation. `--yes` confirms without prompting.
Board requests expire after seven days. Prepare a fresh request each time
you want to join another board.

If the grantor is offline, a join can exit with status 7 (`pending`).
The daemon keeps the invitation and completes the first sync when it finds
the granting device. Use `ppn board ls`, `ppn status`, or `ppn sync` to check
progress.

## Access

| Role | Access |
| --- | --- |
| `read` | Read tasks, comments, and the board's complete history. |
| `write` | Read the history and publish changes to tasks, assignments, and comments. This is the default. |
| `manage` | Write and invite a new person to the board. |

The recipient joins under their existing identity. Their linked devices
learn that the board exists and display **waiting for access**. They need
a separate membership grant in a future release. Changing roles,
removing people, and leaving a shared board also belong to that release.
Device revocation records the change in your identity's roster and reports
the result for each board. If your device is waiting for access to a shared
board, removing the lost device from that board is pending. A board manager
can remove it.

Use `ppn board members House` to see roles, devices, and invitation
details. Add `--json` for full identity and device IDs. Use an identity
name, `name#suffix`, full identity ID, or `me` with `ppn assign` and
`assignee:` filters. When names collide the suffix identifies the person.

Everyone on a shared board can see your identity name and every device's
name and ID, including when devices were added or revoked. Your identity
roster is republished to every board you share.

A read member sends signed identity updates through a separate encrypted
channel. Their granting manager relays the updates and can delay or drop
them. The member's signature prevents the manager from forging updates.

A roster originally signed against a private catalog needs a portable
proof before it can be used in an invitation. The command reports this
case if the roster cannot be verified separately from the catalog.

## Security

A request contains a one-time secret. A grant contains a board ID and is
authenticated using the request secret and the granting device's signing
key. Treat both codes and full board IDs as secrets. Board IDs are
capabilities. Keep them out of logs, shell history, and shared files.
Standard input keeps codes out of process arguments. Look-alike names from
different alphabets can appear identical. Verify the request fingerprint
with the person through a trusted channel when it matters.

Access applies to the selected board. The private catalogs and private
boards of both people stay in their respective identities. Everyone
granted access can see the board's entire history. Removal cannot retract
anything a device already decrypted.
