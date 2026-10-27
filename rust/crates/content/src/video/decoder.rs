//! The platform media pipeline behind [`super::VideoPlayer`]: AVFoundation on macOS (the
//! frameworks WebKit plays `<video>` through there) and GStreamer elsewhere (WebKitGTK's).
//! Either way frames come out as BGRA bytes for a GPUI image and audio goes to the system.

use std::path::Path;

/// A decoded frame: BGRA rows, `width * 4` bytes each.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What the media element knows (`readyState`, `videoWidth`, `duration`, `currentTime`,
/// `paused`, `ended`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    pub ready: bool,
    pub failed: bool,
    pub size: (u32, u32),
    /// Seconds; 0 until known.
    pub duration: f64,
    pub position: f64,
    pub playing: bool,
    pub ended: bool,
}

#[cfg(target_os = "macos")]
pub use mac::Decoder;

#[cfg(not(target_os = "macos"))]
pub use gst::Decoder;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{AnyThread as _, MainThreadMarker};
    use objc2_av_foundation::{
        AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    };
    use objc2_core_media::{CMTime, kCMTimeZero};
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelFormatType_32BGRA,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    pub struct Decoder {
        player: Retained<AVPlayer>,
        item: Retained<AVPlayerItem>,
        output: Retained<AVPlayerItemVideoOutput>,
    }

    impl Decoder {
        pub fn open(path: &Path) -> Option<Self> {
            let mtm = MainThreadMarker::new()?;
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            // SAFETY: AVFoundation objects created and used on the main thread; the
            // dictionary's key is CoreVideo's CFString constant, toll-free bridged to NSString.
            unsafe {
                let item = AVPlayerItem::playerItemWithURL(&url, mtm);
                let key: &NSString =
                    &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
                let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
                let attrs =
                    NSDictionary::from_slices(&[key], &[&*format as &objc2::runtime::AnyObject]);
                let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                    AVPlayerItemVideoOutput::alloc(),
                    Some(&attrs),
                );
                item.addOutput(&output);
                let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
                Some(Decoder {
                    player,
                    item,
                    output,
                })
            }
        }

        pub fn status(&self) -> Status {
            // SAFETY: property reads on the main thread.
            unsafe {
                let ready = self.item.status() == AVPlayerItemStatus::ReadyToPlay;
                let failed = self.item.status() == AVPlayerItemStatus::Failed;
                let size = self.item.presentationSize();
                let secs = |t: CMTime| {
                    let s = t.seconds();
                    if s.is_finite() { s.max(0.) } else { 0. }
                };
                let duration = secs(self.item.duration());
                let position = secs(self.player.currentTime());
                let playing = self.player.rate() != 0.;
                Status {
                    ready,
                    failed,
                    size: (size.width.round() as u32, size.height.round() as u32),
                    duration,
                    position,
                    playing,
                    ended: ready && duration > 0. && position >= duration - 0.01 && !playing,
                }
            }
        }

        /// The frame for the current time, if it changed since the last call.
        pub fn frame(&mut self) -> Option<Frame> {
            // SAFETY: CoreVideo buffer access between lock and unlock, rows copied out within
            // the buffer's reported bounds.
            unsafe {
                let t = self.item.currentTime();
                if !self.output.hasNewPixelBufferForItemTime(t) {
                    return None;
                }
                let pb = self
                    .output
                    .copyPixelBufferForItemTime_itemTimeForDisplay(t, std::ptr::null_mut())?;
                if CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly) != 0 {
                    return None;
                }
                let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
                let stride = CVPixelBufferGetBytesPerRow(&pb);
                let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
                let mut bgra = Vec::with_capacity(w * h * 4);
                if !base.is_null() {
                    for row in 0..h {
                        let src = std::slice::from_raw_parts(base.add(row * stride), w * 4);
                        bgra.extend_from_slice(src);
                    }
                }
                CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
                (bgra.len() == w * h * 4).then_some(Frame {
                    width: w as u32,
                    height: h as u32,
                    bgra,
                })
            }
        }

        /// `play()`; at the end it starts over, as the media element does.
        pub fn play(&mut self) {
            if self.status().ended {
                self.seek(0.);
            }
            // SAFETY: main thread.
            unsafe { self.player.play() }
        }

        pub fn pause(&mut self) {
            // SAFETY: main thread.
            unsafe { self.player.pause() }
        }

        /// Exact seek (`currentTime = t`).
        pub fn seek(&mut self, seconds: f64) {
            // SAFETY: main thread; plain CMTime values.
            unsafe {
                self.player.seekToTime_toleranceBefore_toleranceAfter(
                    CMTime::with_seconds(seconds.max(0.), 600),
                    kCMTimeZero,
                    kCMTimeZero,
                );
            }
        }

        pub fn set_muted(&self, muted: bool) {
            // SAFETY: main thread.
            unsafe { self.player.setMuted(muted) }
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            // SAFETY: main thread.
            unsafe { self.player.pause() }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod gst {
    use super::*;
    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;
    use std::sync::{Arc, Mutex};

    pub struct Decoder {
        playbin: gst::Element,
        latest: Arc<Mutex<Option<Frame>>>,
        size: Arc<Mutex<(u32, u32)>>,
        ended: Arc<Mutex<bool>>,
        failed: Arc<Mutex<bool>>,
        target_playing: bool,
    }

    impl Decoder {
        pub fn open(path: &Path) -> Option<Self> {
            gst::init().ok()?;
            let uri = gst::glib::filename_to_uri(path, None).ok()?;
            let playbin = gst::ElementFactory::make("playbin")
                .property("uri", uri.as_str())
                .build()
                .ok()?;
            let caps = gst_video::VideoCapsBuilder::new()
                .format(gst_video::VideoFormat::Bgra)
                .build();
            let sink = gst_app::AppSink::builder()
                .caps(&caps)
                .max_buffers(1)
                .drop(true)
                .build();
            let latest: Arc<Mutex<Option<Frame>>> = Arc::default();
            let size: Arc<Mutex<(u32, u32)>> = Arc::default();
            let (l, sz) = (latest.clone(), size.clone());
            let store = move |sample: gst::Sample| -> Result<gst::FlowSuccess, gst::FlowError> {
                let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                    return Err(gst::FlowError::Error);
                };
                let info =
                    gst_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::Error)?;
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let (w, h) = (info.width(), info.height());
                let stride = info.stride()[0] as usize;
                let mut bgra = Vec::with_capacity(w as usize * h as usize * 4);
                for row in 0..h as usize {
                    let start = row * stride;
                    bgra.extend_from_slice(&map[start..start + w as usize * 4]);
                }
                *sz.lock().map_err(|_| gst::FlowError::Error)? = (w, h);
                *l.lock().map_err(|_| gst::FlowError::Error)? = Some(Frame {
                    width: w,
                    height: h,
                    bgra,
                });
                Ok(gst::FlowSuccess::Ok)
            };
            // The preroll sample is the paused frame (the poster after load and each seek).
            let preroll = store.clone();
            sink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |s| store(s.pull_sample().map_err(|_| gst::FlowError::Eos)?))
                    .new_preroll(move |s| {
                        preroll(s.pull_preroll().map_err(|_| gst::FlowError::Eos)?)
                    })
                    .build(),
            );
            playbin.set_property("video-sink", &sink);
            let ended: Arc<Mutex<bool>> = Arc::default();
            let failed: Arc<Mutex<bool>> = Arc::default();
            let bus = playbin.bus()?;
            let (e, f) = (ended.clone(), failed.clone());
            bus.set_sync_handler(move |_, msg| {
                match msg.view() {
                    gst::MessageView::Eos(_) => {
                        if let Ok(mut g) = e.lock() {
                            *g = true;
                        }
                    }
                    gst::MessageView::Error(_) => {
                        if let Ok(mut g) = f.lock() {
                            *g = true;
                        }
                    }
                    _ => {}
                }
                gst::BusSyncReply::Drop
            });
            playbin.set_state(gst::State::Paused).ok()?;
            Some(Decoder {
                playbin,
                latest,
                size,
                ended,
                failed,
                target_playing: false,
            })
        }

        pub fn status(&self) -> Status {
            let size = self.size.lock().map(|s| *s).unwrap_or_default();
            let secs = |t: Option<gst::ClockTime>| t.map_or(0., |t| t.nseconds() as f64 / 1e9);
            let ended = self.ended.lock().map(|e| *e).unwrap_or(false);
            Status {
                ready: size.0 > 0,
                failed: self.failed.lock().map(|f| *f).unwrap_or(false),
                size,
                duration: secs(self.playbin.query_duration::<gst::ClockTime>()),
                position: secs(self.playbin.query_position::<gst::ClockTime>()),
                playing: self.target_playing && !ended,
                ended,
            }
        }

        pub fn frame(&mut self) -> Option<Frame> {
            self.latest.lock().ok()?.take()
        }

        pub fn play(&mut self) {
            if self.ended.lock().map(|e| *e).unwrap_or(false) {
                self.seek(0.);
            }
            self.target_playing = true;
            let _ = self.playbin.set_state(gst::State::Playing);
        }

        pub fn pause(&mut self) {
            self.target_playing = false;
            let _ = self.playbin.set_state(gst::State::Paused);
        }

        pub fn seek(&mut self, seconds: f64) {
            if let Ok(mut e) = self.ended.lock() {
                *e = false;
            }
            let t = gst::ClockTime::from_nseconds((seconds.max(0.) * 1e9) as u64);
            let _ = self
                .playbin
                .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, t);
        }

        pub fn set_muted(&self, muted: bool) {
            self.playbin.set_property("mute", muted);
        }
    }

    impl Drop for Decoder {
        fn drop(&mut self) {
            let _ = self.playbin.set_state(gst::State::Null);
        }
    }
}
