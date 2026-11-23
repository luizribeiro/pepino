#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")/.."

matches=$(git grep -Hw sleep -- 'crates/*.rs') || [[ $? == 1 ]]
# Match imports as well as calls, so an alias cannot bypass the check. Count
# approved lines so tests beside production code cannot reuse an approved call.
allowed=$(cat <<'ALLOW'
crates/pepino-app/src/app.rs:use tokio::time::{Instant, sleep};
crates/pepino-app/src/app.rs:        sleep(delay.min(remaining)).await;
crates/pepino-node/tests/network/support.rs:use tokio::time::{Instant, sleep, timeout};
crates/pepino-node/tests/network/support.rs:        sleep(remaining.min(Duration::from_millis(25))).await;
crates/pepino-sync/src/discover.rs:                _ = tokio::time::sleep(poll) => (),
crates/pepino-sync/src/join_progress.rs:                tokio::time::sleep(Duration::from_millis(250)).await;
crates/pepino-sync/src/join_progress.rs:                tokio::time::sleep(Duration::from_millis(250)).await;
crates/pepino-sync/src/outbox.rs:                _ = tokio::time::sleep(poll_interval) => {},
crates/pepino-sync/src/retry.rs:        tokio::time::sleep(self.next_delay()).await;
crates/ppn/src/daemon/process.rs:        thread::sleep(POLL_INTERVAL);
crates/ppn/src/daemon/process.rs:        thread::sleep(POLL_INTERVAL);
crates/ppn/src/daemon/process.rs:        thread::sleep(POLL_INTERVAL);
crates/ppn/src/daemon/process.rs:                thread::sleep(remaining.min(POLL_INTERVAL));
crates/ppn/src/daemon/relay.rs:            _ = tokio::time::sleep(delay) => (),
crates/ppn/src/daemon/runtime.rs:    tokio::time::sleep(delay).await;
crates/ppn/src/daemon/runtime.rs:    tokio::time::sleep(delay).await;
crates/ppn/tests/common/wait.rs:        thread::sleep(remaining.min(Duration::from_millis(25)));
ALLOW
)
export LC_ALL=C
unexpected=$(comm -23 <(printf '%s\n' "$matches" | sort) <(printf '%s\n' "$allowed" | sort))
if [[ -n $unexpected ]]; then
    echo "Use the shared until helper for condition waits in tests." >&2
    printf '%s\n' "$unexpected" >&2
    exit 1
fi
