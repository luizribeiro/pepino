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
| `read` | Read tasks, comments, and the board's complete history. Give this to people who follow the work. |
| `write` | Read the history and publish changes to tasks, assignments, and comments. Give this to people who work on the board. This is the default. |
| `manage` | Write and manage invitations, roles, and removals. Give this to trusted people who administer the board. |

Recipients join under their existing identities. A manager's daemon
adds confirmed active devices at the person's role and removes revoked
devices. Each device signs a confirmation that identifies the person
by identity ID and root signing key. A new device shows **waiting for
access** until a manager enrolls it. A device that confirms two people
on the same board shows an **identity conflict** and loses access.

Board members can see each person's display name and signed device roster,
including device names, public IDs, and admission and revocation details.
Identity cards also carry expiring device enrollment bundles with public
signing and encryption material. A bundle only supplies the public
credentials needed to enroll a device that has confirmed its identity.
The card contains no private keys or private catalog capabilities.

Use `ppn board members House` to see people, effective device membership,
and membership history. **Device roles differ** indicates a role change
that has yet to reach every device. **Removal pending** indicates a
revoked device that still has transport access. `ppn log --board House`
also shows invitations, role changes, removals, and completed leaves.
Records that lose to a concurrent removal appear as **superseded**. An
invitation restores membership only if it follows every recorded removal
for that person. A change that was declined because its author lost
manager access or because it would leave the board without a manager
appears as **not applied**. Use `--json` for full IDs and timestamps.

A read member sends signed roster updates and leave requests through an
encrypted channel. Managers receive and relay those records. They can
delay or drop updates. A device signature protects each roster edit and
leave request from forgery. The departing manager hands the channel to
another board manager before leaving. A blocked handoff is reported as
an incomplete change.

## Manage members

Select a person by name, `name#suffix`, full identity ID, or `me`. If a
name matches multiple people, the error lists the qualified names to
use. The same references work with `ppn assign` and `assignee:` filters.

```sh
ppn board role Ana write --board House
ppn board remove Ana --board House --yes
ppn board leave --board House --yes
ppn board forget House --yes
```

Role changes and removals apply to all confirmed devices. A command reports
completion after checking the transport membership. An incomplete result
explains what still needs to happen. A non-manager
who leaves pauses board task sync locally while a manager's daemon
processes the signed request. Use `ppn status` or `ppn board ls` to check
its progress. A sole managing person must promote another person
before demoting themselves or leaving. Device revocation also checks
for the last manager.

Removing or demoting another manager requires `--yes` and
`--i-understand-mutual-removal-risk`. A manager leaving a board with one
other manager must also give this acknowledgement. Two managers can issue
conflicting changes before they see one another's actions. The transport
may apply both and leave the board unrecoverable. Coordinate manager changes before issuing these commands.

Removing a person rotates the board's keys. The former member keeps
everything they already decrypted, and the transport may still deliver
ciphertext. A device that was offline during removal can be blocked on
reconnect before it learns of the removal. Re-inviting a former member
gives them the complete history, including work from the time they
were away. Past tasks, comments, and assignments keep their authors
and assignees.

After departure, the local history is read-only. `ppn board forget House
--yes` deletes that host's application history and catalog entry for
House. Stop the local daemon before forgetting a board. Run the command
on each device where you want to remove the local history. A device-level
removal leaves the person's board membership intact and cannot be used to
forget the board.
The node database can retain encrypted transport operations and
membership metadata. Copies already exported or backed up must be
deleted separately.

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
