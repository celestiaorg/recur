use super::*;

fn render(src: &str) -> String {
    render_markdown(src, ParseOptions::default())
}

fn demoted(src: &str) -> String {
    render_markdown(
        src,
        ParseOptions {
            demote_images: true,
        },
    )
}

#[test]
fn keeps_short_http_labels_as_anchors() {
    let html = render("See [the PR](https://github.com/org/repo/pull/1).");
    assert!(html.contains("href=\"https://github.com/org/repo/pull/1\""));
    assert!(html.contains("the PR"));
}

#[test]
fn expands_github_emoji_shortcodes_outside_code() {
    let html = render("Ship it :rocket: and `:smile:`");
    assert!(html.contains('🚀'));
    assert!(html.contains(":smile:"));
    assert!(!html.contains(":rocket:"));
}

#[test]
fn does_not_turn_relative_paths_into_anchors() {
    let html = render("See [file.go](internal/file.go).");
    assert!(!html.contains("<a "));
    assert!(html.contains("file.go"));
}

#[test]
fn highlights_fenced_code_and_strips_raw_script_tags() {
    let html = render("```js\nconst x = 1\n```\n<script>alert(1)</script>");
    assert!(html.contains("sh-kw"));
    assert!(!html.contains("<script"));
}

#[test]
fn parse_markdown_does_not_sanitize() {
    let raw = parse_markdown("```js\nconst x = 1\n```", ParseOptions::default());
    assert!(raw.contains("sh-kw"));
}

const PIXEL: &str = "https://tracker.example/pixel.png";

#[test]
fn demote_does_not_emit_a_live_img_for_any_image_form() {
    let forms = [
        format!("![]({PIXEL})"),
        format!("![CI]({PIXEL} \"build\")"),
        format!("![CI]({PIXEL} 'build')"),
        format!("![CI]({PIXEL} (build))"),
        format!("![CI [main]]({PIXEL})"),
        format!("![shield][badge]\n\n[badge]: {PIXEL}"),
    ];
    for src in &forms {
        let html = demoted(src);
        assert!(!html.contains("<img"), "{src}");
        assert!(html.contains(&format!("href=\"{PIXEL}\"")), "{src}");
    }
}

#[test]
fn linked_badge_images_keep_the_outer_href() {
    let html = demoted("[![CI](https://img.shields.io/badge.svg)](https://github.com/org/repo)");
    assert!(!html.contains("<img"));
    assert!(html.contains("href=\"https://github.com/org/repo\""));
    assert!(html.contains(">CI<"));
    assert!(!html.contains("img.shields.io"));
    assert_eq!(html.matches("<a ").count(), 1);
}

#[test]
fn demote_does_not_rewrite_image_syntax_in_code() {
    let fenced = demoted("```md\n![alt](https://tracker.example/pixel.png)\n```");
    assert!(!fenced.contains("<img"));
    assert!(fenced.contains("![alt](https://tracker.example/pixel.png)"));
    let inline = demoted("Use `![alt](https://tracker.example/pixel.png)` in a README.");
    assert!(!inline.contains("<img"));
    assert!(inline.contains("![alt](https://tracker.example/pixel.png)"));
}

#[test]
fn default_parse_still_emits_img() {
    let html = render(&format!("![]({PIXEL})"));
    assert!(html.contains("<img"));
    assert!(html.contains(PIXEL));
}

const ASSET: &str =
    "https://github.com/user-attachments/assets/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const REPO_ASSET: &str =
    "https://github.com/acme/app/assets/1/bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
const CDN: &str = "https://private-user-images.githubusercontent.com/1/shot.png?jwt=x";

#[test]
fn proxies_markdown_images_hosted_on_github() {
    let html = render(&format!("![shot]({ASSET})"));
    assert!(html.contains("<img"));
    assert!(html.contains("/gh-image?url="));
    assert!(html.contains(&crate::js::encode_uri_component(ASSET)));
    assert!(!html.contains(&format!("src=\"{ASSET}\"")));
}

#[test]
fn proxies_repo_assets_and_githubusercontent() {
    assert!(render(&format!("![]({REPO_ASSET})")).contains("/gh-image?url="));
    assert!(render(&format!("![]({CDN})")).contains("/gh-image?url="));
}

#[test]
fn keeps_github_html_img_tags_the_editor_pastes() {
    let html = render(&format!(
        "<img width=\"800\" alt=\"Screenshot\" src=\"{ASSET}\" />\n\nSee above."
    ));
    assert!(html.contains("<img"));
    assert!(html.contains("alt=\"Screenshot\""));
    assert!(html.contains("/gh-image?url="));
    assert!(html.contains("See above"));
}

#[test]
fn still_strips_non_github_html_images_and_scripts() {
    let html = render(&format!(
        "<img src=\"{PIXEL}\" alt=\"x\"><script>alert(1)</script>"
    ));
    assert!(!html.contains("<img"));
    assert!(!html.contains("<script"));
    assert!(!html.contains(PIXEL));
}

#[test]
fn github_login_is_not_an_image() {
    assert!(!render("<img src=\"https://github.com/login\" alt=\"x\">").contains("<img"));
}

#[test]
fn does_not_proxy_http_github_urls() {
    assert!(!render("![](http://github.com/user-attachments/assets/x)").contains("/gh-image?url="));
}

#[test]
fn does_not_rewrite_github_image_syntax_inside_fences() {
    let html = render(&format!("```md\n<img src=\"{ASSET}\">\n```"));
    assert!(!html.contains("<img"));
    assert!(html.contains("&lt;img"));
}

#[test]
fn demote_still_turns_github_images_into_links() {
    let html = demoted(&format!("![shot]({ASSET})"));
    assert!(!html.contains("<img"));
    assert!(html.contains(&format!("href=\"{ASSET}\"")));
}

#[test]
fn anchors_get_target_and_rel_and_lose_foreign_classes() {
    let html = sanitize_markdown(
        "<a href=\"https://x.test\" class=\"sb live\" id=\"y\" style=\"position:fixed\">x</a><span class=\"sh-kw evil\">k</span>",
    );
    assert_eq!(
        html,
        "<a href=\"https://x.test\" rel=\"noopener noreferrer\" target=\"_blank\">x</a><span class=\"sh-kw\">k</span>"
    );
}

#[test]
fn sanitizer_drops_scripts_handlers_frames_and_unsafe_urls() {
    let html = sanitize_markdown(concat!(
        "<script>alert(1)</script><img src=x onerror=alert(1)><iframe src=\"https://x.test\"></iframe>",
        "<a href=\"javascript:alert(1)\">j</a><a href=\"data:text/html,x\">d</a>",
        "<p onclick=\"x()\" style=\"color:red\" id=\"y\">p</p><svg><a href=\"https://x\">s</a></svg>",
        "<img src=\"data:image/png;base64,AAAA\">"
    ));
    for bad in [
        "script",
        "alert",
        "iframe",
        "javascript",
        "data:text",
        "onclick",
        "style",
        "id=",
        "svg",
    ] {
        assert!(!html.contains(bad), "{bad}: {html}");
    }
    assert!(
        html.contains("src=\"data:image/png;base64,AAAA\""),
        "{html}"
    );
}

#[test]
fn task_checkboxes_sit_inside_loose_items_paragraphs() {
    let html = render("- [ ] a\n\n- [x] b");
    assert!(
        html.contains("<p><input type=\"checkbox\" disabled=\"\"> a</p>"),
        "{html}"
    );
    assert!(
        html.contains("<p><input type=\"checkbox\" checked=\"\" disabled=\"\"> b</p>"),
        "{html}"
    );
    let tight = render("- [x] c");
    assert!(
        tight.contains("<li><input type=\"checkbox\" checked=\"\" disabled=\"\"> c</li>"),
        "{tight}"
    );
}
