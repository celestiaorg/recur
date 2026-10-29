//! The bundle service.

use super::{Fx, write};

#[tokio::test(flavor = "multi_thread")]
async fn invalidate_catalog_rereads_script_bodies() {
    let fx = Fx::new().await;
    let dir = format!("{}/A", fx.svc.config().bundles.default_dir);
    write(
        format!("{dir}/bundle.toml"),
        r#"
[bundle]
name = "A"
id = "11111111-1111-4111-8111-111111111111"
[[command]]
id = "11111111-1111-4111-8111-111111111201"
name = "Run"
category = "Text"
interpreter = "typescript"
script = "src/x.ts"
"#,
    );
    let read = || {
        let cat = fx.svc.discover_catalog().unwrap();
        assert_eq!(cat.commands.len(), 1);
        cat.commands[0].body.clone()
    };
    write(format!("{dir}/src/x.ts"), "old");
    assert_eq!(read(), "old");
    write(format!("{dir}/src/x.ts"), "new");
    fx.s.bundle.invalidate_catalog().await.unwrap();
    assert_eq!(read(), "new");
    fx.close().await;
}
