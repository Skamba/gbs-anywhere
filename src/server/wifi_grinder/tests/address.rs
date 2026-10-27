//! The grinder's address, as people type it.

use crate::server::wifi_grinder::{WifiConfig, grinder_address};

#[test]
fn addresses_get_the_api_port() {
    let url = |h: &str| WifiConfig::from_host(h).unwrap().base_url;
    assert_eq!(url("192.168.1.30"), "https://192.168.1.30:30001");
    assert_eq!(url(" grinder.local/ "), "https://grinder.local:30001");
    assert_eq!(url("192.168.1.30:"), "https://192.168.1.30:30001");
    assert_eq!(url("192.168.1.30:4000"), "https://192.168.1.30:4000");
    assert_eq!(url("fe80::1"), "https://[fe80::1]:30001");
    assert_eq!(url("[fe80::1]"), "https://[fe80::1]:30001");
    assert_eq!(url("[fe80::1]:4000"), "https://[fe80::1]:4000");
    assert_eq!(url("http://127.0.0.1:9"), "http://127.0.0.1:9");
    assert!(WifiConfig::from_host("  ").is_err());
    assert!(WifiConfig::from_host("192.168.1.30\n").is_ok(), "trimmed");
    for bad in ["two words", "192.168.1.30\r\nx", "a\tb", &"a".repeat(3000)] {
        assert!(WifiConfig::from_host(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_app_sets_only_a_host_and_port() {
    let addr = |s: &str| grinder_address(s).map_err(|e| e.to_string());
    assert_eq!(addr(" 192.168.1.30 ").unwrap(), "192.168.1.30");
    assert_eq!(addr("Grinder.Local").unwrap(), "grinder.local");
    assert_eq!(addr("192.168.1.30:4000").unwrap(), "192.168.1.30:4000");
    assert_eq!(addr("192.168.1.30:").unwrap(), "192.168.1.30");
    assert_eq!(addr("fe80::1").unwrap(), "[fe80::1]");
    assert_eq!(addr("[FE80::1]:4000").unwrap(), "[fe80::1]:4000");
    assert_eq!(addr("[fe80::1]").unwrap(), "[fe80::1]");
    for bad in [
        "",
        "http://192.168.1.30",
        "192.168.1.30/x",
        "me@192.168.1.30",
        "two words",
        "192.168.1.30\nx",
        "192.168.1.30:0",
        "192.168.1.30:70000",
        "192.168.1.30:80:80",
        "[fe80::1",
        "[nope]:1",
        "-x",
        &"a".repeat(256),
    ] {
        let e = addr(bad).unwrap_err();
        assert!(e.contains("e.g. 192.168.1.30"), "{bad:?}: {e}");
    }
    // What the app sets always goes to the grinder over HTTPS.
    let url = WifiConfig::from_host(&addr("[fe80::1]:4000").unwrap()).unwrap();
    assert_eq!(url.base_url, "https://[fe80::1]:4000");
}
