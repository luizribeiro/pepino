# Share a board

Invite another person to a board, set what they can do, and remove them or
leave. Both people need `ppn` set up with the same `network` setting, and
their devices must be able to [reach each other](daemon.md).

Your first board cannot be shared. Create one for the shared work:

```sh
ppn board new House
```

Everyone you invite can read the board's **entire history**, including work
done before they joined. Everyone on a shared board must run the same `ppn`
version; upgrade together. Builds from before the fix for readmitted devices
cannot read some membership changes on boards with several removals, so
upgrade every device and relay that syncs the board. When a device cannot
read an operation, `ppn status` marks the board with
`Read error: Could not read a board operation during sync` (`read_error` in
`--json`) and the daemon log has a matching warning. It clears after the
next sync that completes cleanly.

## Invite someone

The invitation passes two secret codes over a private channel: a request from
the person joining and a grant from a manager of the board.

1. The person joining creates a request and sends it to you:

   ```console
   $ ppn board request > request.txt
   Device tablet, fingerprint XXXX-XXXX-XXXX-XXXX. This request code is secret. Expires 2026-11-29 05:04.
   ```

2. You confirm the fingerprint with them, by voice or another channel you
   trust, then create the grant and send it back:

   ```sh
   ppn board invite --from-file request.txt --board House --role write \
     --fingerprint XXXX-XXXX-XXXX-XXXX > grant.txt
   ```

3. They join while one of your devices is online:

   ```console
   $ ppn board join --from-file grant.txt --timeout 60s
   Board joined. Initial sync complete.
   ```

A request is valid for seven days and for one invitation. If the join cannot
reach your device in time it exits with status 7 and completes later by
itself; `ppn board ls` shows the progress. The person's other devices get
access automatically when a manager's device is online. If assigning by name
returns `unknown person` just after a join, run `ppn sync` to receive the
person's identity card and retry.

## Choose a role

| Role | Can do |
| --- | --- |
| `read` | Read tasks, comments, and history. |
| `write` | Also create and change tasks and comments. The default. |
| `manage` | Also invite people, change roles, and remove people. |

A role belongs to a person and applies to all their devices.

## See who is on a board

```console
$ ppn board members House
Members of House
Ada G (manage)
  Garden laptop · member · added 2026-11-22T10:03:55.433Z
Ana (write) · invited by Ada G on 2026-11-22T10:04:54.065903935Z
  tablet · member · added 2026-11-22T10:04:46.216Z
2026-11-22T10:04:54.111096Z · Ada G invited Ana as write
```

Members see each other's display names and device names. `ppn log --board
House` shows membership changes next to task history.

## Change a role or remove someone

```sh
ppn board role Ana read --board House
ppn board remove Ana --board House --yes
```

Name a person by display name or by the `name#suffix` form that an ambiguity
error suggests. For role changes you can also use `me`.

Removal changes the board's keys, so the person cannot read anything new. It
cannot take back what their devices already hold. If you invite them again
they get the full history, including the time they were away.

A board must keep at least one manager: `ppn` refuses a change that would
leave none.

**Two managers must not remove or demote each other at the same time.** If
both changes go through, the board has no manager and nobody can ever fix
it. Demoting or removing a manager therefore needs
`--yes --i-understand-mutual-removal-risk`; agree on the change with the other
manager first.

## Leave a board and delete your copy

```sh
ppn board leave House --yes
```

This works offline. It removes the board from your private catalog, deletes
its tasks and history from the app database, and stops syncing the board.
Your other devices do the same when they next sync the catalog. Each device
asks its relays to drop its copies. The encrypted operations and keys stay
in the device's node store, and the board ID stays in the catalog tombstone.
Deleted pages can persist in database files until overwritten. To erase all
local data on this device, remove its data directory. A fresh invitation
from a manager restores access and history.

Leaving revokes nothing. Other members still see you listed and share with your
devices until a manager removes you.

If you are the only manager in your local view, promote someone first to keep
invitations and removals possible. If you still want to leave, confirm the
warning at a terminal or pass `--yes --i-understand-last-manager-risk`.

After a manager removes you, you can still read your local history. To delete
it from your devices, run `ppn board forget House --yes`.

## Boards with many removals

Each time a person or device is removed from a board, the board's keys
change, and every later invitation carries all the keys the inviting device
has kept. That bundle grows by about 76 bytes per removal, and past 16 KiB
other devices cannot read the invitation, so nobody new can be added:
roughly 210 removals, fewer when several devices have rotated keys. At three
quarters of that size `ppn status` shows a warning on the board,
`near_member_limit` is `true` in `ppn status --json`, and the daemon log has
the warning once. There is no way to shrink the bundle yet: to keep adding
people, create a new board and move the work there.
