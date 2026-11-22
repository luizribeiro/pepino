# Hacking on pepino

Build, test, and check pepino from a checkout.

Run `nix flake check --print-build-logs` to build the package and run every
check for your system.

On Linux, the Rust `test` check runs in a private network namespace, so the
Nix build users must be allowed to create unprivileged user and network
namespaces. Probe your host with `unshare --user --map-root-user --net true`.
If it fails, check that `user.max_user_namespaces` is above zero and that
AppArmor allows it; see the
[Ubuntu guidance on unprivileged user namespaces](https://documentation.ubuntu.com/release-notes/24.04/#unprivileged-user-namespace-restrictions).

The `vm` check, on `x86_64-linux` only, runs a relay and two devices in NixOS
VMs. It needs a `/dev/kvm` the Nix build users can use, and a builder that
advertises `kvm` and `nixos-test` in `system-features`.

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

The development shell (`nix develop`) provides `cargo-nextest` and sets
`PEPINO_NETWORK_TESTS=1`. Before each commit, run this loop, repeating the
last command for each crate you changed.

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

The arguments are the number of suite runs and of busy-loop processes. The
harness finishes every run, prints the output of failed ones, keeps logs
under `target/test-under-load.*`, and fails if any run failed.

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

To sample more generated store histories, raise the property case count.

```sh
PROPTEST_CASES=256 cargo nextest run -p pepino-store --lib -E 'test(convergence::)'
```
