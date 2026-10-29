//! `fileserver_test.go` and the `/gh-image` route of `gh_image_test.go`
//! (URL policy and fetching are covered in recur-forge).

use std::sync::Arc;

use recur_domain::Project;
use recur_forge::testing::FakeForge;
use recur_forge::{Forge, ForgeError, ForgeErrorKind, ImageBytes};

use super::{Fx, FxOpts, git, write};
use crate::api::Asset;
use crate::app::assets::set_transcode_tiff;

const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn project(fx: &Fx, id: &str, root: &str) {
    fx.store()
        .add_project(Project {
            id: id.into(),
            name: id.into(),
            repo_path: root.into(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
}

fn q(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Fx {
    async fn get(&self, url: &str) -> Asset {
        self.s
            .assets
            .fetch_asset(url)
            .await
            .unwrap_or_else(|e| panic!("{url}: {} {}", e.status, e.message))
    }

    async fn status(&self, url: &str) -> u16 {
        match self.s.assets.fetch_asset(url).await {
            Ok(_) => 200,
            Err(e) => e.status,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_workspace_file_streams_body() {
    let fx = Fx::new().await;
    let root = fx.temp();
    let body = "hello\n".repeat(50_000);
    write(format!("{root}/big.txt"), &body);
    std::fs::write(format!("{root}/icon.png"), PNG).unwrap();
    write(format!("{root}/doc.pdf"), "%PDF-1.4\n");
    std::fs::write(format!("{root}/blob.bin"), [0u8, 1]).unwrap();
    project(&fx, "p-fs", &root);
    let file = |path: &str| format!("/workspace-file?project=p-fs&path={}", q(path));

    let text = fx.get(&file("big.txt")).await;
    assert_eq!(text.body, body.as_bytes());
    assert!(
        text.content_type.starts_with("text/plain"),
        "{}",
        text.content_type
    );
    let img = fx.get(&file("icon.png")).await;
    assert_eq!(
        (img.body.as_slice(), img.content_type.as_str()),
        (&PNG[..], "image/png")
    );
    let pdf = fx.get(&file("doc.pdf")).await;
    assert_eq!(
        (pdf.body.as_slice(), pdf.content_type.as_str()),
        (&b"%PDF-1.4\n"[..], "application/pdf")
    );
    assert_eq!(fx.status(&file("blob.bin")).await, 415);
    assert!(matches!(fx.status(&file("../secret")).await, 400 | 404));

    let loose = format!("{}/shot.png", fx.temp());
    std::fs::write(&loose, PNG).unwrap();
    fx.svc.loose.allow(&loose);
    assert_eq!(fx.get(&file(&loose)).await.body, PNG);

    assert_eq!(fx.status("/workspace-file").await, 400);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_workspace_file_at_base() {
    let fx = Fx::new().await;
    let root = fx.temp();
    let mut newer = PNG;
    newer[PNG.len() - 1] ^= 0xff;
    git(&root, &["init", "-b", "main"]);
    std::fs::write(format!("{root}/icon.png"), PNG).unwrap();
    write(format!("{root}/notes.txt"), "hi\n");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "init"]);
    std::fs::write(format!("{root}/icon.png"), newer).unwrap();
    project(&fx, "p-img", &root);
    let file = |rest: &str| format!("/workspace-file?project=p-img&{rest}");

    let base = fx.get(&file("path=icon.png&at=base")).await;
    assert_eq!(base.body, PNG, "at=base served working-tree bytes");
    assert_eq!(base.content_type, "image/png");
    assert_eq!(fx.get(&file("path=icon.png")).await.body, newer);
    assert_eq!(fx.status(&file("path=notes.txt&at=base")).await, 400);
    assert_eq!(fx.get(&file("path=icon.png&at=HEAD")).await.body, PNG);
    assert_eq!(fx.status(&file("path=icon.png&at=HEAD:x")).await, 400);
    assert_eq!(fx.status(&file("path=nope.png&at=base")).await, 404);
    assert!(matches!(
        fx.status(&file("path=../secret.png&at=base")).await,
        400 | 404
    ));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_chat_image() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    let png = format!("{dir}/shot.png");
    std::fs::write(&png, PNG).unwrap();
    recur_code::allow_external_path(&png);
    let txt = format!("{dir}/notes.txt");
    write(&txt, "hi");
    recur_code::allow_external_path(&txt);
    let missing = format!("{dir}/nope.png");
    recur_code::allow_external_path(&missing);
    let url = |p: &str| format!("/chat-image?path={}", q(p));

    let got = fx.get(&url(&png)).await;
    assert_eq!(
        (got.body.as_slice(), got.content_type.as_str()),
        (&PNG[..], "image/png")
    );
    assert!(matches!(fx.status(&url("shot.png")).await, 400 | 415));
    assert!(matches!(fx.status(&url(&txt)).await, 400 | 415));
    assert_eq!(fx.status(&url(&missing)).await, 404);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_external_file() {
    let fx = Fx::new().await;
    let dir = fx.temp();
    let csv = format!("{dir}/export-111.csv");
    write(&csv, "a,b\n1,2\n");
    let bin = format!("{dir}/blob.bin");
    std::fs::write(&bin, [0u8, 1]).unwrap();
    recur_code::allow_external_path(&csv);
    recur_code::allow_external_path(&bin);
    let url = |p: &str| format!("/external-file?path={}", q(p));

    let got = fx.get(&url(&csv)).await;
    assert_eq!(got.body, b"a,b\n1,2\n");
    assert!(
        got.content_type.starts_with("text/plain"),
        "{}",
        got.content_type
    );
    assert_eq!(fx.status(&url(&bin)).await, 415);
    assert_eq!(fx.status(&url("export-111.csv")).await, 400);
    let other = format!("{dir}/other.csv");
    write(&other, "a,b\n1,2\n");
    assert_eq!(fx.status(&url(&other)).await, 403, "undropped");
    fx.close().await;
}

/// A gray 8-bit strip TIFF header claiming `w`x`h` with a single pixel byte.
fn tiff_header(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"II*\x00".to_vec();
    b.extend(8u32.to_le_bytes());
    b.extend(6u16.to_le_bytes());
    for (tag, typ, val) in [
        (256u16, 4u16, w),
        (257, 4, h),
        (258, 3, 8),
        (262, 3, 1),
        (273, 4, 86),
        (279, 4, 1),
    ] {
        b.extend(tag.to_le_bytes());
        b.extend(typ.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(val.to_le_bytes());
    }
    b.extend(0u32.to_le_bytes());
    b.push(0);
    b
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_workspace_file_tiff() {
    let fx = Fx::new().await;
    let root = fx.temp();
    let mut small = Vec::new();
    image::DynamicImage::new_luma8(3, 2)
        .write_to(
            &mut std::io::Cursor::new(&mut small),
            image::ImageFormat::Tiff,
        )
        .unwrap();
    git(&root, &["init", "-b", "main"]);
    std::fs::write(format!("{root}/scan.tif"), &small).unwrap();
    std::fs::write(
        format!("{root}/huge.tiff"),
        tiff_header(1 << 13, (1 << 13) + 1),
    )
    .unwrap();
    std::fs::write(
        format!("{root}/overflow.tif"),
        tiff_header(u32::MAX, u32::MAX),
    )
    .unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "init"]);
    project(&fx, "p-tiff", &root);
    let file = |rest: &str| format!("/workspace-file?project=p-tiff&{rest}");

    let prev = set_transcode_tiff(true);
    for query in ["path=scan.tif", "path=scan.tif&at=base"] {
        set_transcode_tiff(true);
        let got = fx.get(&file(query)).await;
        assert_eq!(got.content_type, "image/png", "{query}");
        let img = image::load_from_memory_with_format(&got.body, image::ImageFormat::Png).unwrap();
        assert_eq!((img.width(), img.height()), (3, 2), "{query}");

        set_transcode_tiff(false);
        let raw = fx.get(&file(query)).await;
        assert_eq!(raw.content_type, "image/tiff", "{query}");
        assert_eq!(raw.body, small, "{query}");
    }
    set_transcode_tiff(true);
    for name in ["huge.tiff", "overflow.tif"] {
        assert_eq!(
            fx.status(&file(&format!("path={name}"))).await,
            415,
            "{name}"
        );
    }
    set_transcode_tiff(prev);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_forge_image() {
    const ASSET: &str =
        "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let forge = Arc::new(FakeForge::new());
    forge.on_fetch_image(|(url,)| async move {
        match url.as_str() {
            ASSET => Ok(Some(ImageBytes {
                content_type: "image/png".into(),
                bytes: PNG.to_vec(),
            })),
            "https://github.com/html" => {
                Err(ForgeError::new(ForgeErrorKind::Http(415), "not an image"))
            }
            _ => Ok(None),
        }
    });
    let fx = Fx::with(FxOpts {
        forges: Some(vec![forge as Arc<dyn Forge>]),
        ..FxOpts::default()
    })
    .await;
    let url = |u: &str| format!("/gh-image?url={}", q(u));
    let got = fx.get(&url(ASSET)).await;
    assert_eq!(got.body, PNG);
    assert_eq!(got.content_type, "image/png");
    assert_eq!(got.cache_control, "private, max-age=86400");
    assert_eq!(fx.status(&url("https://evil.example/pixel.png")).await, 400);
    assert_eq!(fx.status(&url("https://github.com/html")).await, 415);
    assert_eq!(fx.status("/gh-image").await, 400);
    fx.close().await;
}
