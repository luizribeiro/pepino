# Link your devices

Use the same identity and boards on several machines, see which devices you
have, and remove one. Linking passes two secret codes between the machines: a
request from the new device and a grant from an existing one.

## Link a second device

Both machines need the same `network` setting
([configuration](scripting.md#configure-ppn)) and a way to reach each other:
the same local network, or the internet through the default relay servers.
The existing device must be online.

1. On the new machine, which must not have run `ppn init`, create a request:

   ```console
   $ umask 077
   $ ppn device request --name desktop > request.txt
   Device desktop, fingerprint XXXX-XXXX-XXXX-XXXX. This request code is secret. Expires 2027-02-20 05:04.
   ```

2. Move `request.txt` to a device you already use, over a channel you trust.
   There, check that the fingerprint matches the one the new machine printed,
   then create the grant:

   ```sh
   ppn device add --from-file request.txt --fingerprint XXXX-XXXX-XXXX-XXXX > grant.txt
   ```

   On a terminal you can omit `--fingerprint`; `ppn` shows the fingerprint
   and asks you to confirm.

3. Move `grant.txt` back to the new machine and join:

   ```console
   $ ppn device join --from-file grant.txt --timeout 60s
   Device joined. Initial sync complete. Granted by laptop (803f57).
   ```

A request is valid for 90 days. The new device gets your role on every board. Delete both files afterwards:
the request holds a one-time secret and the grant holds your board IDs.

If the join cannot reach the granting device in time, it exits with status 7.
The join is kept; the daemon completes it when the other device is reachable,
and `ppn status` shows the progress. To give up before joining, run
`ppn device request --cancel`.

## See and rename your devices

```console
$ ppn device ls
desktop (504ed2) · active · member of 1 of 1 board · joined · this device · added by 803f57 on 2026-11-22 05:04
  Home (003257) · joined
laptop (803f57) · active · member of 1 of 1 board · seen on the board · first device · last seen 2026-11-22 05:04
  Home (003257) · member
```

```sh
ppn device ls --active              # hide revoked devices
ppn device rename 'Garden laptop'   # this device
ppn device rename desktop Workbench # another device
ppn identity rename 'Ada G'         # the name other people see
```

## Remove a device

To retire a device you still have, run this on it:

```sh
ppn device leave --yes
```

The device stops syncing at once, and the first device completes the removal
the next time it is online. The command prints the data directory and
credentials file to delete if you want to wipe the machine.

To remove a device you no longer control, run this on the first device:

```sh
ppn device revoke Workbench --yes
```

Removal changes the keys of every board, so the removed device cannot read
anything new. It keeps everything it already had. Once it learns of the
removal, its write commands fail with status 5.

To use a removed machine again, delete its data directory and link it as a
new device.

## Protect the first device

The device where you ran `ppn init` anchors your identity. `ppn device ls`
marks it `first device`. Only it can revoke devices, it cannot be revoked or
leave, and you cannot move that role to another device.

- **If you lose it**, your other devices keep working and can still link new
  ones, but you can no longer revoke any device.
- **If it is stolen**, the thief holds an identity you cannot revoke. Run
  `ppn init` in a fresh data directory to create a new identity, ask to be
  re-invited to shared boards, and ask their managers to
  [remove the old identity](sharing.md#change-a-role-or-remove-someone).

## Back up a device

Back up the whole data directory; `ppn status` prints its path. A device
that loses the keys in it must be linked again as a new device. Your tasks
survive on your other devices.
