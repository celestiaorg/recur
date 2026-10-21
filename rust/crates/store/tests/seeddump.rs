//! The seeddump binary writes exactly the committed e2e fixture.

#[test]
fn seeddump_matches_committed_fixture() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_seeddump"))
        .output()
        .expect("run seeddump");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let got = String::from_utf8(out.stdout).unwrap();
    let want = include_str!("../../../../frontend/e2e/fixtures/seed.json");
    if got != want {
        let first = got
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "seeddump differs from seed.json at line {}:\n got: {:?}\nwant: {:?}",
            first + 1,
            got.lines().nth(first),
            want.lines().nth(first)
        );
    }
}
