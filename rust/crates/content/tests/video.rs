//! The platform media pipeline behind `VideoPlayer` on a real clip (`fixtures/red-blue.mp4`:
//! one red second, one blue, 160x90 H.264 with AAC). `harness = false` so AVFoundation gets the
//! main thread and its run loop; Linux plays it through GStreamer.

use std::time::{Duration, Instant};

use recur_content::video::decoder::{Decoder, Frame, Status};

const CLIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/red-blue.mp4");

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
    let mut d = Decoder::open(std::path::Path::new(CLIP)).expect("open clip");
    let s: Status = until("metadata", || {
        Some(d.status()).filter(|s| s.ready && s.duration > 0.)
    });
    assert_eq!(s.size, (160, 90));
    assert!((s.duration - 2.).abs() < 0.1, "duration {}", s.duration);
    assert!(!s.playing);

    let first = until("the first frame", || d.frame());
    let (r, g, b) = center(&first);
    assert!(r > 200 && g < 60 && b < 60, "first frame {:?}", (r, g, b));

    d.seek(1.5);
    let later = until("a blue frame after seeking", || {
        d.frame().filter(|f| {
            let (r, _, b) = center(f);
            b > 200 && r < 60
        })
    });
    assert_eq!((later.width, later.height), (160, 90));

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
        "paused playback moved"
    );
    assert!(!d.status().playing);
    println!("video: ok");
}
