//! Port of `report_color_test.go`.

use recur_bundles::*;

#[test]
fn format_report_color() {
    let r = Report {
        passed: 1,
        failed: 1,
        results: vec![
            CaseResult {
                bundle: "B".into(),
                name: "pass".into(),
                ok: true,
                ..Default::default()
            },
            CaseResult {
                bundle: "B".into(),
                name: "fail".into(),
                ok: false,
                detail: "boom".into(),
                file: "x.toml".into(),
            },
        ],
    };
    assert_eq!(
        format_report(&r, false),
        "ok\tB\tpass\nFAIL\tB\tfail\n  boom\n  (x.toml)\n1 passed, 1 failed\n"
    );
    assert_eq!(
        format_report(&r, true),
        "\x1b[32mok\x1b[0m\tB\tpass\n\x1b[1m\x1b[31mFAIL\x1b[0m\tB\tfail\n  \x1b[31mboom\x1b[0m\n  \x1b[2m(x.toml)\x1b[0m\n1 passed, \x1b[1m\x1b[31m1 failed\x1b[0m\n"
    );
    let all_ok = Report {
        passed: 1,
        ..Default::default()
    };
    assert_eq!(
        format_report(&all_ok, true),
        "\x1b[32m1 passed\x1b[0m, 0 failed\n"
    );
}
