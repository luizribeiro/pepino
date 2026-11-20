mod fixture;

use fixture::pair;

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn enrolled_devices_share_a_board_and_keep_distinct_credentials() {
    let (_dir, a, b) = pair().await;
    assert_eq!(a.board(), b.board());
    assert_ne!(
        a.settings.credentials.device_id(),
        b.settings.credentials.device_id()
    );
}

#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn freshly_enrolled_engines_report_idle_and_shut_down() {
    let (_dir, a, b) = pair().await;
    let (left, right) = tokio::join!(a.start(), b.start());
    tokio::join!(a.await_events(&left, 0), b.await_events(&right, 0));
    fixture::views_match(&a, &b).await;
    tokio::join!(left.stop(), right.stop());
}
