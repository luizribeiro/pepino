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

## Rust tests

Enter the development shell with `nix develop`. It provides `cargo-nextest`
and sets `PEPINO_NETWORK_TESTS=1`. Use the crate test loop before each commit.
Repeat the last command for each changed crate.

```sh
export CARGO_PROFILE_DEV_DEBUG=0
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p pepino-app  # Substitute the changed crate
```

Run the full suite before handing off a branch.

```sh
cargo nextest run --workspace
cargo test --workspace --doc
```

With a warm build, run `scripts/check-suite-budget.sh` before merging. It
runs both commands, reports wall time and the ten slowest Nextest tests, and
fails if the suite exceeds `PEPINO_SUITE_BUDGET_SECONDS` (120 seconds by
default). If the one-minute load average at launch is at least 75% of the
CPU count, the time limit is skipped and test failures still fail the check.

Use the load harness to repeat the suite with or without synthetic CPU load.

```sh
scripts/test-under-load.sh 10 0
scripts/test-under-load.sh 5 8
```

The arguments specify the number of workspace runs and busy-loop processes.
Each iteration runs the network-enabled Nextest suite and the doctests. The
harness completes all iterations, prints each failed run's output, retains
logs under `target/test-under-load.*`, and exits unsuccessfully if any run
failed.

On Linux, the default daemon publication and ingestion recovery tests use
`/dev/shm` when it is available and `TMPDIR` is unset. The SQLite files
remain available across daemon restarts.

### Scale tests

Run these separately from the workspace suite. Each command selects one
ignored scale test.

```sh
cargo nextest run -p pepino-node --test network --run-ignored ignored-only -E 'test(=scale::restart_skips_acknowledged_history_at_scale)'
cargo nextest run -p pepino-node --lib --run-ignored ignored-only -E 'test(=delivery::tests::a_paused_consumer_resumes_with_the_queue_full_at_scale)'
cargo nextest run -p pepino-store --lib --run-ignored ignored-only -E 'test(=edges::tests::loads_long_chains_in_adversarial_register_order_at_scale)'
cargo nextest run -p ppn --lib --run-ignored ignored-only -E 'test(=daemon::shutdown_tests::sigterm_recovers_events_during_publishing_and_ingesting_at_scale)'
cargo nextest run -p ppn --lib --run-ignored ignored-only -E 'test(=daemon::shutdown_tests::sigkill_recovers_events_during_publishing_and_ingesting_at_scale)'
```

The daemon scale tests fill the receiver's memory buffer and interrupt it
with a task still in the SQLite delivery spool. They give the full burst
up to three minutes to reach the receiver and another three minutes to
replay after restart.

To sample more generated store histories, raise the property case count.

```sh
PROPTEST_CASES=256 cargo nextest run -p pepino-store --lib -E 'test(convergence::)'
```
