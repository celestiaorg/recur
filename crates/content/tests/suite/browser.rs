use recur_content::browser::{content_rules, is_blocked, normalize_url};

#[test]
fn the_address_bar_keeps_schemes_and_guesses_the_rest() {
    assert_eq!(
        normalize_url(" https://example.com/a "),
        "https://example.com/a"
    );
    assert_eq!(
        normalize_url("http://localhost:5173"),
        "http://localhost:5173"
    );
    assert_eq!(normalize_url("about:blank"), "about:blank");
    assert_eq!(normalize_url("localhost:5173"), "http://localhost:5173");
    assert_eq!(normalize_url("localhost"), "http://localhost");
    assert_eq!(normalize_url("127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
    assert_eq!(
        normalize_url("app.localhost/login"),
        "http://app.localhost/login"
    );
    assert_eq!(normalize_url("[::1]"), "http://[::1]");
    assert_eq!(normalize_url("myapp.test:3000"), "http://myapp.test:3000");
    assert_eq!(normalize_url("example.com"), "https://example.com");
    assert_eq!(
        normalize_url("github.com/foo?q=1"),
        "https://github.com/foo?q=1"
    );
    assert_eq!(normalize_url("  "), "");
    assert_eq!(normalize_url("hello world"), "");
}

#[test]
fn recur_ports_are_blocked_on_any_host() {
    let ports = [51234];
    for url in [
        "http://127.0.0.1:51234/tok/mcp",
        "http://localhost:51234/",
        "http://LOCALHOST:51234",
        "http://[::1]:51234/",
        "ws://127.0.0.1:51234/",
        "http://rebind.example:51234/",
    ] {
        assert!(is_blocked(url, &ports), "{url}");
    }
    for url in [
        "http://localhost:5173/",
        "http://localhost:512345/",
        "https://example.com/",
        "about:blank",
        "not a url",
    ] {
        assert!(!is_blocked(url, &ports), "{url}");
    }
    assert!(!is_blocked("http://localhost:51234/", &[]));
}

#[test]
fn the_rule_list_has_one_block_rule_per_port() {
    let rules: serde_json::Value = serde_json::from_str(&content_rules(&[51234, 4000])).unwrap();
    let filters: Vec<&str> = rules
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            assert_eq!(r["action"]["type"], "block");
            r["trigger"]["url-filter"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        filters,
        [
            "^[a-z][-a-z0-9+.]*://[^/]*:51234[/?#]",
            "^[a-z][-a-z0-9+.]*://[^/]*:4000[/?#]"
        ]
    );
}
