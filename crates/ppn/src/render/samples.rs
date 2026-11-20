use pepino_app::TaskSummary;
use serde_json::json;

use super::Dates;

pub(super) fn dates() -> Dates<'static> {
    Dates {
        time: super::dates::tests_support::invocation_time(),
        format: "%d/%m/%Y",
    }
}

pub(super) fn task() -> TaskSummary {
    let time = json!({"wall_ms": 1795149900000u64, "counter": 2});
    serde_json::from_value(json!({
        "id":"97ad97ab","title":"Replace the greenhouse latch","description":"Use stainless hardware.",
        "status":"todo","due":"2026-11-21","priority":2,"fields":{"estimate":{"type":"integer","value":3}},
        "tags":["hardware"],"assignees":["a6e1a65f"],"created":time,"updated":time,"author":"6b376b12",
        "parent":null,"children":[],"blocks":[],"blocked_by":["97ad97ff"],"relates":[],
        "blocked":true,"ready":false,"comment_count":1
    })).unwrap()
}
