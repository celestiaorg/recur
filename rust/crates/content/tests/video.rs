//! The platform media pipeline behind `VideoPlayer` on real clips: one red second, one blue,
//! 160x90, as H.264 with AAC (`red-blue.mp4`), VP8 with Vorbis, VP9 with Opus and AV1 with
//! Opus in WebM, and Theora with Vorbis in Ogg (made with ffmpeg). `harness = false` so
//! AVFoundation gets the main thread and its run loop; macOS opens WebM and Ogg with its own
//! decoder (WebKit's codecs), Linux plays everything through GStreamer.

use std::time::{Duration, Instant};

use recur_content::video::decoder::{Decoder, Frame, Status};

const CLIPS: &[&str] = &[
    "red-blue.mp4",
    "red-blue-vp8.webm",
    "red-blue-vp9.webm",
    "red-blue-av1.webm",
    "red-blue.ogv",
];

fn pump(ms: u64) {
    #[cfg(target_os = "macos")]
    {
        use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
        // SAFETY: the default mode constant is a static CFString.
        #[allow(unsafe_code)]
        let mode = unsafe { kCFRunLoopDefaultMode };
        CFRunLoop::run_in_mode(mode, Duration::from_millis(ms), false);
    }
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(Duration::from_millis(ms));
}

fn until<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        pump(20);
    }
}

/// The center pixel as (r, g, b).
fn center(f: &Frame) -> (u8, u8, u8) {
    let i = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
    (f.bgra[i + 2], f.bgra[i + 1], f.bgra[i])
}

fn main() {
    for clip in CLIPS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(clip);
        check(&path);
        println!("video {clip}: ok");
    }
    println!("video: ok");
}

fn check(clip: &std::path::Path) {
    let name = clip.display();
    let mut d = Decoder::open(clip).unwrap_or_else(|| panic!("open {name}"));
    let s: Status = until("metadata", || {
        Some(d.status()).filter(|s| s.ready && s.duration > 0.)
    });
    assert_eq!(s.size, (160, 90), "{name}");
    assert!(
        (s.duration - 2.).abs() < 0.15,
        "{name}: duration {}",
        s.duration
    );
    assert!(!s.playing, "{name}");

    let first = until("the first frame", || d.frame());
    let (r, g, b) = center(&first);
    assert!(
        r > 200 && g < 60 && b < 60,
        "{name}: first frame {:?}",
        (r, g, b)
    );

    d.seek(1.5);
    let later = until("a blue frame after seeking", || {
        d.frame().filter(|f| {
            let (r, _, b) = center(f);
            b > 200 && r < 60
        })
    });
    assert_eq!((later.width, later.height), (160, 90), "{name}");

    d.seek(0.);
    d.play();
    until("playback", || {
        Some(()).filter(|_| d.status().position > 0.3)
    });
    d.pause();
    pump(200);
    let held = d.status().position;
    pump(300);
    assert!(
        (d.status().position - held).abs() < 0.05,
        "{name}: paused playback moved"
    );
    assert!(!d.status().playing, "{name}");
}
