//! Bundle settings.

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_settings() {
    let (_g, dir) = temp_dir();
    let toml = r#"
[bundle]
name = "Linear"
id = "recur.linear"

[[setting]]
id = "recur.linear.api-key"
name = "api_key"
label = "API key"
secret = true
placeholder = "lin_api_…"
url = "https://linear.app/settings/api"
urlLabel = "Open Linear"

[[setting]]
id = "recur.linear.team-key"
name = "team_key"
label = "Default team key"
placeholder = "ENG"
"#;
    let parsed = parse_manifest_full(&dir, toml.as_bytes()).unwrap();
    assert_eq!(parsed.settings.len(), 2);
    let s = &parsed.settings[0];
    assert_eq!(s.id, "recur.linear.api-key");
    assert_eq!(s.name, "api_key");
    assert!(s.secret);
    assert_eq!(s.url_label, "Open Linear");
    assert_eq!(setting_env_name(&s.name), "RECUR_SETTING_API_KEY");
}

#[test]
fn parse_settings_rejects() {
    let (_g, dir) = temp_dir();
    for (name, toml, want) in [
        (
            "bad name",
            "[bundle]\nid = \"b\"\nname = \"B\"\n[[setting]]\nid = \"b.k\"\nname = \"API-KEY\"\n",
            "name",
        ),
        (
            "dup name",
            "[bundle]\nid = \"b\"\nname = \"B\"\n[[setting]]\nid = \"b.one\"\nname = \"token\"\n[[setting]]\nid = \"b.two\"\nname = \"token\"\n",
            "duplicate name",
        ),
        (
            "dup id",
            "[bundle]\nid = \"b\"\nname = \"B\"\n[[setting]]\nid = \"b.one\"\nname = \"a\"\n[[setting]]\nid = \"b.one\"\nname = \"b\"\n",
            "duplicate id",
        ),
    ] {
        let err = parse_manifest_full(&dir, toml.as_bytes()).unwrap_err();
        assert!(err.0.to_lowercase().contains(want), "{name}: {err}");
    }
}

#[test]
fn parse_settings_cap() {
    let (_g, dir) = temp_dir();
    let mut b = String::from("[bundle]\nid = \"b\"\nname = \"B\"\n");
    for i in 0..=SETTING_MAX {
        b.push_str(&format!("[[setting]]\nid = \"b.s{i}\"\nname = \"s{i}\"\n"));
    }
    let err = parse_manifest_full(&dir, b.as_bytes()).unwrap_err();
    assert_eq!(err.0, "setting: at most 16 entries per bundle");
}

#[test]
fn discover_setting_user_overrides() {
    let (_g1, user) = temp_dir();
    let (_g2, def) = temp_dir();
    write_bundle(
        &def,
        "Pack",
        "[bundle]\nid = \"acme.pack\"\nname = \"Pack\"\n[[setting]]\nid = \"acme.pack.token\"\nname = \"token\"\nlabel = \"Shipped\"\n",
    );
    write_bundle(
        &user,
        "Pack",
        "[bundle]\nid = \"acme.pack\"\nname = \"Pack\"\n[[setting]]\nid = \"acme.pack.token\"\nname = \"token\"\nlabel = \"User\"\nsecret = true\n",
    );
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let s = find_setting_by_id(&cat.settings, "acme.pack.token").expect("missing");
    assert_eq!(s.label, "User");
    assert!(s.secret);
    assert_eq!(s.source_root, "user");
    assert_eq!(settings_for_bundle(&cat.settings, "acme.pack").len(), 1);
}

#[test]
fn shipped_linear_settings() {
    let dir = repo_path("bundles/Linear");
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    let parsed = parse_manifest_full(&dir, &data).unwrap();
    assert_eq!(parsed.settings.len(), 2);
    let api = find_setting_by_id(&parsed.settings, "recur.linear.api-key").unwrap();
    assert_eq!(api.name, "api_key");
    assert!(api.secret);
    let team = find_setting_by_id(&parsed.settings, "recur.linear.team-key").unwrap();
    assert_eq!(team.name, "team_key");
    assert!(!team.secret);
}
