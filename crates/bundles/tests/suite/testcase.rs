//! Bundle test cases.

use recur_bundles::*;

use crate::common::*;

#[test]
fn load_tests_file_payloads() {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/tests/fixtures/in.txt"), "hello\n");
    must_write(&format!("{dir}/tests/fixtures/out.txt"), "HELLO\n");
    must_write(
        &format!("{dir}/tests/case.toml"),
        "\n[[test]]\nname = \"from files\"\ncommand = \"upper-ts\"\n[test.context]\ninputFile = \"fixtures/in.txt\"\nselectedTextFile = \"fixtures/in.txt\"\n[test.expect]\nstdoutFile = \"fixtures/out.txt\"\noutput = \"replaceSelection\"\n",
    );
    let tests = load_tests(&dir).unwrap();
    assert_eq!(tests.len(), 1);
    let tc = &tests[0].case;
    assert_eq!(tc.context.input, "hello\n");
    assert_eq!(tc.expect.stdout, "HELLO\n");
    assert!(tc.context.input_file.is_empty() && tc.expect.stdout_file.is_empty());
}

#[test]
fn load_tests_rejects_inline_and_file() {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/tests/in.txt"), "x");
    must_write(
        &format!("{dir}/tests/bad.toml"),
        "\n[[test]]\nname = \"both\"\ncommand = \"c\"\n[test.context]\ninput = \"inline\"\ninputFile = \"in.txt\"\n[test.expect]\nstdout = \"x\"\n",
    );
    let err = load_tests(&dir).unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "{dir}/tests/bad.toml: test \"both\": input: set inline value or inputFile, not both"
        )
    );
}

#[test]
fn load_tests_rules() {
    let (_g, dir) = temp_dir();
    assert!(load_tests(&dir).unwrap().is_empty());
    must_write(&format!("{dir}/tests/a.toml"), "[[test]]\nname = \"x\"\n");
    assert_eq!(
        load_tests(&dir).unwrap_err().0,
        format!("{dir}/tests/a.toml: test[0]: command or snippet is required")
    );
    must_write(
        &format!("{dir}/tests/a.toml"),
        "[[test]]\ncommand = \"c\"\nsnippet = \"s\"\n",
    );
    assert_eq!(
        load_tests(&dir).unwrap_err().0,
        format!("{dir}/tests/a.toml: test[0]: set command or snippet, not both")
    );
    must_write(
        &format!("{dir}/tests/a.toml"),
        "[[test]]\ncommand = \"c\"\n[test.context]\ninputFile = \"../../x\"\n",
    );
    assert_eq!(
        load_tests(&dir).unwrap_err().0,
        format!(
            "{dir}/tests/a.toml: test \"a.toml#1\": inputFile \"../../x\" escapes test directory"
        )
    );
}
