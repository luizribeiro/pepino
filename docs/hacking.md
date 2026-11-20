# Hacking on Pepino

Run `nix flake check --print-build-logs` from the repository root to build the
package and run the checks for your system.

On Linux, the Nix build users need permission to create unprivileged user and
network namespaces and map their UID and GID in the new user namespace. The
Rust `test` check uses a private network namespace with a loopback multicast
route. From an unprivileged shell, `unshare --user --map-root-user --net true`
probes the host policy for your account. The Nix build users need the same
permissions. Check that `user.max_user_namespaces` is greater than zero and
that any AppArmor policy allows the namespace and ID mapping. GitHub Actions
uses Ubuntu's boot-scoped `kernel.apparmor_restrict_unprivileged_userns=0`
setting on its disposable Linux runners. See the
[Ubuntu guidance on unprivileged user namespaces](https://documentation.ubuntu.com/release-notes/24.04/#unprivileged-user-namespace-restrictions).

On `x86_64-linux`, the `vm` check also needs hardware virtualization enabled
and a working `/dev/kvm` that the Nix build users can read and write. The Nix
builder must advertise `kvm` and `nixos-test` in `system-features`.

## Run individual checks

```sh
system=$(nix eval --impure --raw --expr builtins.currentSystem)
nix build --no-link --print-build-logs ".#checks.$system.default" # Package build
nix build --no-link --print-build-logs ".#checks.$system.config"  # Service configuration
nix build --no-link --print-build-logs ".#checks.$system.fmt"     # Rust formatting
nix build --no-link --print-build-logs ".#checks.$system.clippy"  # Rust linting
nix build --no-link --print-build-logs ".#checks.$system.test"    # Rust tests
nix build --no-link --print-build-logs .#checks.x86_64-linux.vm  # NixOS VM on x86_64-linux
```
