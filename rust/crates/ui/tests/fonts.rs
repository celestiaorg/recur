//! Bundled faces register with the platform text system and CSS weight matching picks the same
//! face the browser picks. Needs the main thread, hence `harness = false`.

use gpui_kit::{HeadlessAppContext, px};
use recur_ui::fonts::{FACES, Family};
use std::sync::Arc;

fn main() {
    let cx = HeadlessAppContext::with_platform(
        gpui_kit::platform::current_platform(true).text_system(),
        Arc::new(()),
        || Ok(None),
    );
    let ts = cx.text_system().clone();
    ts.add_fonts(
        FACES
            .iter()
            .map(|f| std::borrow::Cow::Borrowed(f.2))
            .collect(),
    )
    .expect("bundled faces load");
    let face = |family: &str, weight: f32| {
        let f = Family::Stack(family.to_string().into(), Vec::new()).font(
            weight,
            false,
            Default::default(),
        );
        ts.resolve_font(&f)
    };
    // Each bundled weight is its own face.
    for (family, weight, _) in FACES {
        for (other, w2, _) in FACES {
            if family == other && weight != w2 {
                assert_ne!(
                    face(family, *weight),
                    face(family, *w2),
                    "{family} {weight} vs {w2}"
                );
            }
        }
        let id = face(family, *weight);
        assert!(
            ts.advance(id, px(13.), 'm').is_ok(),
            "{family} {weight} has glyphs"
        );
    }
    // CSS Fonts 4 matching: 550/650/700 on Inter land on 600; 400 on Space Grotesk on 500.
    assert_eq!(face("Inter", 550.), face("Inter", 600.));
    assert_eq!(face("Inter", 700.), face("Inter", 600.));
    assert_eq!(face("Space Grotesk", 400.), face("Space Grotesk", 500.));
    assert_eq!(face("JetBrains Mono", 650.), face("JetBrains Mono", 700.));
    // Inter's vertical metrics drive every line box: hhea 1984 / −494 over 2048.
    let inter = face("Inter", 400.);
    assert_eq!(ts.ascent(inter, px(2048.)), px(1984.));
    assert_eq!(ts.descent(inter, px(2048.)), px(-494.));
    println!("fonts: ok");
}
