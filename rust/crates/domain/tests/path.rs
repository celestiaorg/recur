use recur_domain::path::{base, clean, dir, eval_symlinks, ext, glob_match, join, rel};

#[test]
fn clean_is_lexical() {
    for (input, want) in [
        ("", "."),
        ("abc", "abc"),
        ("abc/def", "abc/def"),
        ("a/b/c", "a/b/c"),
        (".", "."),
        ("..", ".."),
        ("../..", "../.."),
        ("../../abc", "../../abc"),
        ("/abc", "/abc"),
        ("/", "/"),
        ("abc/", "abc"),
        ("abc/def/", "abc/def"),
        ("//abc", "/abc"),
        ("///abc", "/abc"),
        ("abc//def//ghi", "abc/def/ghi"),
        ("abc/./def", "abc/def"),
        ("/./abc/def", "/abc/def"),
        ("abc/.", "abc"),
        ("abc/def/ghi/../jkl", "abc/def/jkl"),
        ("abc/def/../ghi/../jkl", "abc/jkl"),
        ("abc/def/..", "abc"),
        ("abc/def/../..", "."),
        ("/abc/def/../..", "/"),
        ("abc/def/../../..", ".."),
        ("/abc/def/../../..", "/"),
        ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
        ("/../abc", "/abc"),
        ("abc/./../def", "def"),
        ("abc//./../def", "def"),
        ("abc/../../././../def", "../../def"),
    ] {
        assert_eq!(clean(input), want, "clean({input:?})");
    }
}

#[test]
fn rel_cases() {
    for (b, t, want) in [
        ("a/b", "a/b", Some(".")),
        ("a/b/.", "a/b", Some(".")),
        ("a/b", "a/b/.", Some(".")),
        ("./a/b", "a/b", Some(".")),
        ("a/b", "./a/b", Some(".")),
        ("ab/cd", "ab/cde", Some("../cde")),
        ("ab/cd", "ab/c", Some("../c")),
        ("a/b", "a/b/c/d", Some("c/d")),
        ("a/b", "a/b/../c", Some("../c")),
        ("a/b/../c", "a/b", Some("../b")),
        ("a/b/c", "a/c/d", Some("../../c/d")),
        ("a/b", "c/d", Some("../../c/d")),
        ("a/b/c/d", "a/b", Some("../..")),
        ("a/b/c/d", "a/b/", Some("../..")),
        ("a/b/c/d/", "a/b", Some("../..")),
        ("a/b/c/d/", "a/b/", Some("../..")),
        ("../../a/b", "../../a/b/c/d", Some("c/d")),
        ("/a/b", "/a/b", Some(".")),
        ("/a/b/.", "/a/b", Some(".")),
        ("/a/b", "/a/b/.", Some(".")),
        ("/ab/cd", "/ab/cde", Some("../cde")),
        ("/ab/cd", "/ab/c", Some("../c")),
        ("/a/b", "/a/b/c/d", Some("c/d")),
        ("/a/b", "/a/b/../c", Some("../c")),
        ("/a/b/../c", "/a/b", Some("../b")),
        ("/a/b/c", "/a/c/d", Some("../../c/d")),
        ("/a/b", "/c/d", Some("../../c/d")),
        ("/a/b/c/d", "/a/b", Some("../..")),
        ("/a/b/c/d", "/a/b/", Some("../..")),
        ("/a/b/c/d/", "/a/b", Some("../..")),
        ("/a/b/c/d/", "/a/b/", Some("../..")),
        ("/../../a/b", "/../../a/b/c/d", Some("c/d")),
        (".", "a/b", Some("a/b")),
        (".", "..", Some("..")),
        ("..", ".", None),
        ("..", "a", None),
        ("../..", "..", None),
        ("a", "/a", None),
        ("/a", "a", None),
    ] {
        assert_eq!(rel(b, t).as_deref(), want, "rel({b:?}, {t:?})");
    }
}

#[test]
fn glob_match_cases() {
    for (pattern, s, want) in [
        ("abc", "abc", Ok(true)),
        ("*", "abc", Ok(true)),
        ("*c", "abc", Ok(true)),
        ("a*", "a", Ok(true)),
        ("a*", "abc", Ok(true)),
        ("a*", "ab/c", Ok(false)),
        ("a*/b", "abc/b", Ok(true)),
        ("a*/b", "a/c/b", Ok(false)),
        ("a*b*c*d*e*/f", "axbxcxdxe/f", Ok(true)),
        ("a*b*c*d*e*/f", "axbxcxdxexxx/f", Ok(true)),
        ("a*b*c*d*e*/f", "axbxcxdxe/xxx/f", Ok(false)),
        ("a*b*c*d*e*/f", "axbxcxdxexxx/fff", Ok(false)),
        ("a*b?c*x", "abxbbxdbxebxczzx", Ok(true)),
        ("a*b?c*x", "abxbbxdbxebxczzy", Ok(false)),
        ("ab[c]", "abc", Ok(true)),
        ("ab[b-d]", "abc", Ok(true)),
        ("ab[e-g]", "abc", Ok(false)),
        ("ab[^c]", "abc", Ok(false)),
        ("ab[^b-d]", "abc", Ok(false)),
        ("ab[^e-g]", "abc", Ok(true)),
        ("a\\*b", "a*b", Ok(true)),
        ("a\\*b", "ab", Ok(false)),
        ("a?b", "a☺b", Ok(true)),
        ("a[^a]b", "a☺b", Ok(true)),
        ("a???b", "a☺b", Ok(false)),
        ("a[^a][^a][^a]b", "a☺b", Ok(false)),
        ("[a-ζ]*", "α", Ok(true)),
        ("*[a-ζ]", "A", Ok(false)),
        ("a?b", "a/b", Ok(false)),
        ("a*b", "a/b", Ok(false)),
        ("[\\]a]", "]", Ok(true)),
        ("[\\-]", "-", Ok(true)),
        ("[x\\-]", "x", Ok(true)),
        ("[x\\-]", "-", Ok(true)),
        ("[x\\-]", "z", Ok(false)),
        ("[\\-x]", "x", Ok(true)),
        ("[\\-x]", "-", Ok(true)),
        ("[\\-x]", "a", Ok(false)),
        ("[]a]", "]", Err(())),
        ("[-]", "-", Err(())),
        ("[x-]", "x", Err(())),
        ("[x-]", "-", Err(())),
        ("[x-]", "z", Err(())),
        ("[-x]", "x", Err(())),
        ("[-x]", "-", Err(())),
        ("[-x]", "a", Err(())),
        ("\\", "a", Err(())),
        ("[a-b-c]", "a", Err(())),
        ("[", "a", Err(())),
        ("[^", "a", Err(())),
        ("[^bc", "a", Err(())),
        ("a[", "a", Err(())),
        ("a[", "ab", Err(())),
        ("a[", "x", Err(())),
        ("a/b[", "x", Err(())),
        ("*x", "xxx", Ok(true)),
    ] {
        assert_eq!(
            glob_match(pattern, s).map_err(|_| ()),
            want,
            "match({pattern:?}, {s:?})"
        );
    }
}

#[test]
fn base_dir_ext() {
    assert_eq!(base("/a/b/"), "b");
    assert_eq!(base("/"), "/");
    assert_eq!(base(""), ".");
    assert_eq!(dir("/a/b"), "/a");
    assert_eq!(dir("a"), ".");
    assert_eq!(dir("/"), "/");
    assert_eq!(ext("x/y.tmLanguage.json"), ".json");
    assert_eq!(ext("x.d/y"), "");
    assert_eq!(join(&["a", "", "../b"]), "b");
    assert_eq!(join(&["", ""]), "");
}

#[test]
fn eval_symlinks_resolves_dotdot_after_links() {
    let tmp = std::env::temp_dir().join(format!("recur-path-{}", std::process::id()));
    let real = tmp.join("real/sub");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, tmp.join("link")).unwrap();
    let root = eval_symlinks(&tmp.to_string_lossy()).unwrap();
    let got = eval_symlinks(&format!("{}/link/..", tmp.to_string_lossy()));
    assert_eq!(got, Some(format!("{root}/real")));
    assert_eq!(eval_symlinks(&format!("{root}/missing")), None);
    std::fs::remove_dir_all(&tmp).unwrap();
}
