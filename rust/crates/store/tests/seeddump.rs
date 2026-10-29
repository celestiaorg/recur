//! The seeddump binary writes the committed e2e fixture (as JSON values).

use recur_domain::json::canonical;
use serde_json::Value;

#[test]
fn seeddump_matches_committed_fixture() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_seeddump"))
        .output()
        .expect("run seeddump");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    let want: Value =
        serde_json::from_str(include_str!("../../services/src/mock/seed.json")).unwrap();
    pretty_assertions::assert_eq!(canonical(got), canonical(want));
}
