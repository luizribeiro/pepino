fn main() {
    println!("cargo::rerun-if-env-changed=PEPINO_NETWORK_TESTS");
    println!("cargo::rustc-check-cfg=cfg(pepino_network_tests)");
    if std::env::var("PEPINO_NETWORK_TESTS").as_deref() == Ok("1") {
        println!("cargo::rustc-cfg=pepino_network_tests");
    }
}
