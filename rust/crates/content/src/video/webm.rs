//! WebM and Ogg on macOS, which AVFoundation does not open but WebKit (the Go app's player)
//! plays: VP8 and VP9 through libvpx, AV1 through libaom, Theora through theora-rs, Opus through
//! libopus and Vorbis through lewton, with `cpal` for the sound. The licenses are recorded in
//! `docs/PARITY.md`. A thread demuxes and decodes ahead into short queues; the audio device's
//! clock (samples played) drives which frame shows, or the wall clock without a sound track.
//! Same interface as `mac::Decoder`, picked by sniffing the container.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::decoder::{Frame, Status};

/// Frames decoded ahead of the clock.
const VIDEO_AHEAD: usize = 6;
/// Seconds of sound decoded ahead of the clock.
const AUDIO_AHEAD: f64 = 1.0;

/// Whether `path` is a container this decoder opens (an EBML or Ogg header).
pub fn sniff(path: &Path) -> bool {
    let mut head = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && (head == [0x1a, 0x45, 0xdf, 0xa3] || &head == b"OggS")
}

/// Decoded sound: interleaved f32 at `rate` with `channels`.
struct Sound {
    rate: u32,
    channels: usize,
    samples: VecDeque<f32>,
}

#[derive(Default)]
struct Shared {
    status: Status,
    frames: VecDeque<(f64, Frame)>,
    sound: Option<Sound>,
    /// The playback position when the clock last started or moved, seconds.
    base: f64,
    /// Sample frames the device took since `base` (the audio clock).
    played: u64,
    /// When the wall clock last started, without a sound track.
    started: Option<Instant>,
    /// A seek for the decode thread, and a generation that drops stale output.
    seek: Option<f64>,
    generation: u64,
    eof: bool,
    muted: bool,
    closed: bool,
    /// No output device: the wall clock drives playback and the sound is dropped.
    wall: bool,
}

impl Shared {
    fn position(&self) -> f64 {
        let moved = match (&self.sound, self.started) {
            (Some(s), _) if self.status.playing && !self.wall => self.played as f64 / s.rate as f64,
            (_, Some(t)) => t.elapsed().as_secs_f64(),
            _ => 0.,
        };
        let p = self.base + moved;
        if self.status.duration > 0. {
            p.min(self.status.duration)
        } else {
            p
        }
    }

    /// Moves the clock's base to now, so a rate change starts from the current position.
    fn rebase(&mut self) {
        self.base = self.position();
        self.played = 0;
        self.started = (self.status.playing && !self.audio_clock()).then(Instant::now);
    }

    fn audio_clock(&self) -> bool {
        self.sound.is_some() && !self.wall
    }
}

type Sync = Arc<(Mutex<Shared>, Condvar)>;

pub struct Decoder {
    shared: Sync,
    thread: Option<std::thread::JoinHandle<()>>,
    output: Option<cpal::Stream>,
    shown: Option<f64>,
}

impl Decoder {
    pub fn open(path: &Path) -> Option<Self> {
        if !sniff(path) {
            return None;
        }
        let shared: Sync = Arc::default();
        let (s, p) = (shared.clone(), path.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("recur-media".into())
            .spawn(move || decode_thread(&p, &s))
            .ok()?;
        Some(Decoder {
            shared,
            thread: Some(thread),
            output: None,
            shown: None,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        let s = self.lock();
        let position = s.position();
        let ended = s.status.ready
            && s.eof
            && s.frames.is_empty()
            && s.status.duration > 0.
            && position >= s.status.duration - 0.05;
        Status {
            position,
            ended,
            playing: s.status.playing && !ended,
            ..s.status
        }
    }

    /// The frame for the current time, if it changed since the last call.
    pub fn frame(&mut self) -> Option<Frame> {
        let mut s = self.lock();
        let now = s.position();
        let mut pick = None;
        // The latest frame due; a paused or seeked player shows the first one queued.
        while let Some((pts, _)) = s.frames.front() {
            if *pts <= now + 0.001 || (pick.is_none() && self.shown.is_none()) {
                pick = s.frames.pop_front();
            } else {
                break;
            }
        }
        drop(s);
        self.shared.1.notify_all();
        let (pts, frame) = pick?;
        self.shown = Some(pts);
        Some(frame)
    }

    /// `play()`; at the end it starts over, as the media element does.
    pub fn play(&mut self) {
        if self.status().ended {
            self.seek(0.);
        }
        if self.lock().status.playing {
            return;
        }
        if self.output.is_none() && self.lock().sound.is_some() {
            self.output = open_output(&self.shared);
            if self.output.is_none() {
                let mut s = self.lock();
                s.wall = true;
                if let Some(snd) = &mut s.sound {
                    snd.samples.clear();
                }
            }
        }
        if let Some(o) = &self.output {
            let _ = cpal::traits::StreamTrait::play(o);
        }
        let mut s = self.lock();
        s.status.playing = true;
        s.rebase();
    }

    pub fn pause(&mut self) {
        let mut s = self.lock();
        if s.status.playing {
            s.base = s.position();
            s.played = 0;
            s.started = None;
            s.status.playing = false;
        }
    }

    /// Exact seek (`currentTime = t`): the decode thread restarts at `t`.
    pub fn seek(&mut self, seconds: f64) {
        let mut s = self.lock();
        let t = seconds.max(0.);
        s.seek = Some(t);
        s.generation += 1;
        s.frames.clear();
        if let Some(snd) = &mut s.sound {
            snd.samples.clear();
        }
        s.base = t;
        s.played = 0;
        s.started = (s.status.playing && !s.audio_clock()).then(Instant::now);
        s.eof = false;
        drop(s);
        self.shown = None;
        self.shared.1.notify_all();
    }

    pub fn set_muted(&self, muted: bool) {
        self.lock().muted = muted;
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.lock().closed = true;
        self.shared.1.notify_all();
        self.output = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The system output, pulling the decoded sound at the device's rate (linear resampling) and
/// counting what it played for the clock. `None` without a device or a sound track.
fn open_output(shared: &Sync) -> Option<cpal::Stream> {
    use cpal::traits::{DeviceTrait as _, HostTrait as _};
    let device = cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    let (out_rate, out_channels) = (config.sample_rate() as f64, config.channels() as usize);
    let shared = shared.clone();
    let mut phase = 0f64;
    device
        .build_output_stream(
            config.config(),
            move |data: &mut [f32], _| {
                let mut s = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                let (playing, muted) = (s.status.playing, s.muted);
                let Some(snd) = s.sound.as_mut() else {
                    data.fill(0.);
                    return;
                };
                let step = snd.rate as f64 / out_rate;
                let ch = snd.channels.max(1);
                let mut used = 0u64;
                for out in data.chunks_mut(out_channels) {
                    if !playing || snd.samples.len() < ch * 2 {
                        out.fill(0.);
                        continue;
                    }
                    for (c, o) in out.iter_mut().enumerate() {
                        let k = c.min(ch - 1);
                        let (a, b) = (snd.samples[k], snd.samples[ch + k]);
                        let v = a + (b - a) * phase as f32;
                        *o = if muted { 0. } else { v };
                    }
                    phase += step;
                    while phase >= 1. && snd.samples.len() >= ch {
                        snd.samples.drain(..ch);
                        phase -= 1.;
                        used += 1;
                    }
                }
                if playing {
                    s.played += used;
                }
                drop(s);
                shared.1.notify_all();
            },
            |_| {},
            None,
        )
        .ok()
}

// ---- decoding ----

/// RGB from YUV planes (BT.601 below 720 lines, BT.709 from there, limited range, as WebKit's
/// default for untagged video).
struct Planes<'a> {
    y: &'a [u8],
    u: &'a [u8],
    v: &'a [u8],
    y_stride: usize,
    c_stride: usize,
    /// Chroma subsampling: 1 (4:4:4) or 2 (4:2:x) per axis.
    cx: usize,
    cy: usize,
    width: usize,
    height: usize,
}

fn to_bgra(p: &Planes) -> Frame {
    let hd = p.height >= 720;
    let (rv, gu, gv, bu) = if hd {
        (1.793, -0.213, -0.533, 2.112)
    } else {
        (1.596, -0.392, -0.813, 2.017)
    };
    let mut bgra = vec![0u8; p.width * p.height * 4];
    for row in 0..p.height {
        let yr = &p.y[row * p.y_stride..];
        let cr = (row / p.cy) * p.c_stride;
        for (col, &luma) in yr.iter().enumerate().take(p.width) {
            let y = 1.164 * (luma as f32 - 16.);
            let ci = cr + col / p.cx;
            let u = p.u.get(ci).copied().unwrap_or(128) as f32 - 128.;
            let v = p.v.get(ci).copied().unwrap_or(128) as f32 - 128.;
            let clamp = |x: f32| x.round().clamp(0., 255.) as u8;
            let o = (row * p.width + col) * 4;
            bgra[o] = clamp(y + bu * u);
            bgra[o + 1] = clamp(y + gu * u + gv * v);
            bgra[o + 2] = clamp(y + rv * v);
            bgra[o + 3] = 255;
        }
    }
    Frame {
        width: p.width as u32,
        height: p.height as u32,
        bgra,
    }
}

/// 16-bit little-endian samples (a 10- or 12-bit image) as 8-bit.
fn narrow(plane: &[u8], bits: u32) -> Vec<u8> {
    plane
        .chunks_exact(2)
        .map(|b| (u16::from_le_bytes([b[0], b[1]]) >> (bits - 8)).min(255) as u8)
        .collect()
}

enum Video {
    Vpx(shiguredo_libvpx::Decoder),
    Aom(Box<shiguredo_aom::Decoder>),
    Theora(Box<theora_rs::DecoderContext>, Box<theora_rs::Info>),
}

impl Video {
    fn decode(&mut self, data: &[u8], granule: i64) -> Vec<Frame> {
        let mut out = Vec::new();
        match self {
            Video::Vpx(d) => {
                if d.decode(data).is_err() {
                    return out;
                }
                while let Ok(Some(f)) = d.next_frame() {
                    let (w, h) = (f.width(), f.height());
                    let hi = f.is_high_depth();
                    let planes = |p: &[u8]| if hi { narrow(p, 10) } else { p.to_vec() };
                    let (y, u, v) = (
                        planes(f.y_plane()),
                        planes(f.u_plane()),
                        planes(f.v_plane()),
                    );
                    let div = if hi { 2 } else { 1 };
                    out.push(to_bgra(&Planes {
                        y: &y,
                        u: &u,
                        v: &v,
                        y_stride: f.y_stride() / div,
                        c_stride: f.u_stride() / div,
                        cx: 2,
                        cy: 2,
                        width: w,
                        height: h,
                    }));
                }
            }
            Video::Aom(d) => {
                if d.decode(data).is_err() {
                    return out;
                }
                while let Some(f) = d.next_frame() {
                    let (w, h) = (f.width(), f.height());
                    let hi = f.is_high_depth();
                    let (Ok(y), Ok(u), Ok(v)) = (f.y_plane(), f.u_plane(), f.v_plane()) else {
                        continue;
                    };
                    let (Ok(ys), Ok(us)) = (f.y_stride(), f.u_stride()) else {
                        continue;
                    };
                    let (cx, cy) = match f.format() {
                        Ok(shiguredo_aom::ImageFormat::I444) => (1, 1),
                        Ok(shiguredo_aom::ImageFormat::I422) => (2, 1),
                        _ => (2, 2),
                    };
                    let planes = |p: &[u8]| if hi { narrow(p, 10) } else { p.to_vec() };
                    let div = if hi { 2 } else { 1 };
                    out.push(to_bgra(&Planes {
                        y: &planes(y),
                        u: &planes(u),
                        v: &planes(v),
                        y_stride: ys / div,
                        c_stride: us / div,
                        cx,
                        cy,
                        width: w,
                        height: h,
                    }));
                }
            }
            Video::Theora(d, info) => {
                let packet = theora_rs::OggPacket {
                    packet: data.to_vec(),
                    b_o_s: false,
                    e_o_s: false,
                    granulepos: granule,
                    packetno: 0,
                };
                if d.packetin(&packet).is_err() {
                    return out;
                }
                let Ok(img) = d.ycbcr_ref() else {
                    return out;
                };
                // Rows may run bottom-up (a negative stride from the plane's offset): copy
                // them top-down within the picture region.
                let rows =
                    |p: &theora_rs::ImgPlaneRef, x0: usize, y0: usize, w: usize, h: usize| {
                        let mut v = Vec::with_capacity(w * h);
                        for r in 0..h {
                            let at = p.data_offset as isize
                                + ((y0 + r) as isize) * p.stride as isize
                                + x0 as isize;
                            let at = at.max(0) as usize;
                            v.extend_from_slice(p.data.get(at..at + w).unwrap_or(&vec![128; w]));
                        }
                        v
                    };
                let (cx, cy) = match info.pixel_fmt {
                    theora_rs::PixelFmt::Pf444 => (1, 1),
                    theora_rs::PixelFmt::Pf422 => (2, 1),
                    _ => (2, 2),
                };
                let (px, py) = (info.pic_x as usize, info.pic_y as usize);
                let (w, h) = (info.pic_width as usize, info.pic_height as usize);
                let (cw, ch) = (w.div_ceil(cx), h.div_ceil(cy));
                let y = rows(&img[0], px, py, w, h);
                let u = rows(&img[1], px / cx, py / cy, cw, ch);
                let v = rows(&img[2], px / cx, py / cy, cw, ch);
                out.push(to_bgra(&Planes {
                    y: &y,
                    u: &u,
                    v: &v,
                    y_stride: w,
                    c_stride: cw,
                    cx,
                    cy,
                    width: w,
                    height: h,
                }));
            }
        }
        out
    }
}

enum Audio {
    Opus {
        dec: shiguredo_opus::Decoder,
        channels: usize,
        /// Samples to drop at the start (`pre_skip`, 48 kHz).
        skip: usize,
    },
    Vorbis {
        ident: Box<lewton::header::IdentHeader>,
        setup: Box<lewton::header::SetupHeader>,
        pwr: lewton::audio::PreviousWindowRight,
    },
}

impl Audio {
    fn rate(&self) -> u32 {
        match self {
            Audio::Opus { .. } => 48_000,
            Audio::Vorbis { ident, .. } => ident.audio_sample_rate,
        }
    }

    fn channels(&self) -> usize {
        match self {
            Audio::Opus { channels, .. } => *channels,
            Audio::Vorbis { ident, .. } => ident.audio_channels as usize,
        }
    }

    /// One packet as interleaved f32.
    fn decode(&mut self, data: &[u8]) -> Vec<f32> {
        match self {
            Audio::Opus {
                dec,
                skip,
                channels,
            } => {
                let Ok(pcm) = dec.decode(data) else {
                    return Vec::new();
                };
                let drop = (*skip * *channels).min(pcm.len());
                *skip -= drop / (*channels).max(1);
                pcm[drop..].iter().map(|&s| s as f32 / 32768.).collect()
            }
            Audio::Vorbis { ident, setup, pwr } => {
                let Ok(pcm) = lewton::audio::read_audio_packet(ident, setup, data, pwr) else {
                    return Vec::new();
                };
                let n = pcm.first().map_or(0, Vec::len);
                let mut out = Vec::with_capacity(n * pcm.len());
                for i in 0..n {
                    for c in &pcm {
                        out.push(c[i] as f32 / 32768.);
                    }
                }
                out
            }
        }
    }

    /// `OpusHead` (channels at 9, pre-skip at 10); mapping family 0 (mono or stereo) only.
    fn opus(head: &[u8]) -> Option<Audio> {
        if head.len() < 19 || &head[..8] != b"OpusHead" || head[18] != 0 {
            return None;
        }
        let channels = head[9].clamp(1, 2);
        let skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        let dec =
            shiguredo_opus::Decoder::new(shiguredo_opus::DecoderConfig::new(48_000, channels))
                .ok()?;
        Some(Audio::Opus {
            dec,
            channels: channels as usize,
            skip,
        })
    }

    /// The three Vorbis headers.
    fn vorbis(headers: &[Vec<u8>]) -> Option<Audio> {
        let [id, _comment, setup] = headers else {
            return None;
        };
        let ident = lewton::header::read_header_ident(id).ok()?;
        let setup = lewton::header::read_header_setup(
            setup,
            ident.audio_channels,
            (ident.blocksize_0, ident.blocksize_1),
        )
        .ok()?;
        Some(Audio::Vorbis {
            ident: Box::new(ident),
            setup: Box::new(setup),
            pwr: lewton::audio::PreviousWindowRight::new(),
        })
    }
}

/// A demuxed packet: which stream, its time in seconds, its bytes.
struct Packet {
    video: bool,
    pts: f64,
    data: Vec<u8>,
    keyframe: bool,
    granule: i64,
}

trait Demux {
    fn next(&mut self) -> Option<Packet>;
    /// Moves to a keyframe at or before `t`; `false` when the format cannot (decode from the
    /// start instead).
    fn seek(&mut self, t: f64) -> bool;
}

struct Opened {
    demux: Box<dyn Demux>,
    video: Option<Video>,
    audio: Option<Audio>,
    size: (u32, u32),
    duration: f64,
}

// ---- WebM ----

struct Webm {
    path: PathBuf,
    file: matroska_demuxer::MatroskaFile<BufReader<File>>,
    video: Option<u64>,
    audio: Option<u64>,
    scale: f64,
    /// The video track's keyframe timestamps, read once for the first seek.
    keyframes: Option<Vec<u64>>,
}

impl Webm {
    /// A demux-only pass over the file (no decoding) for the keyframes' timestamps.
    fn index(&self) -> Vec<u64> {
        let mut out = Vec::new();
        let Some(track) = self.video else { return out };
        let Ok(f) = File::open(&self.path) else {
            return out;
        };
        let Ok(mut file) = matroska_demuxer::MatroskaFile::open(BufReader::new(f)) else {
            return out;
        };
        let mut frame = matroska_demuxer::Frame::default();
        while let Ok(true) = file.next_frame(&mut frame) {
            if frame.track == track && (out.is_empty() || frame.is_keyframe != Some(false)) {
                out.push(frame.timestamp);
            }
        }
        out
    }
}

impl Demux for Webm {
    fn next(&mut self) -> Option<Packet> {
        let mut f = matroska_demuxer::Frame::default();
        loop {
            if !self.file.next_frame(&mut f).ok()? {
                return None;
            }
            let video = Some(f.track) == self.video;
            if video || Some(f.track) == self.audio {
                return Some(Packet {
                    video,
                    pts: f.timestamp as f64 * self.scale,
                    data: std::mem::take(&mut f.data),
                    keyframe: f.is_keyframe.unwrap_or(true),
                    granule: 0,
                });
            }
        }
    }

    /// To the last keyframe at or before `t`: the demuxer's own seek stops at the first frame
    /// after `t`, which a decoder cannot start from.
    fn seek(&mut self, t: f64) -> bool {
        if self.video.is_none() {
            return self.file.seek((t / self.scale) as u64).is_ok();
        }
        let keys = match &self.keyframes {
            Some(k) => k,
            None => self.keyframes.insert(self.index()),
        };
        let target = (t / self.scale) as u64;
        let at = keys
            .iter()
            .rev()
            .find(|k| **k <= target)
            .copied()
            .unwrap_or(0);
        self.file.seek(at).is_ok()
    }
}

/// Matroska's Xiph lacing in `CodecPrivate` (Vorbis's three headers).
fn xiph_headers(private: &[u8]) -> Option<Vec<Vec<u8>>> {
    let count = *private.first()? as usize + 1;
    let mut at = 1;
    let mut sizes = Vec::new();
    for _ in 0..count - 1 {
        let mut n = 0usize;
        loop {
            let b = *private.get(at)?;
            at += 1;
            n += b as usize;
            if b != 255 {
                break;
            }
        }
        sizes.push(n);
    }
    let mut out = Vec::new();
    for n in sizes {
        out.push(private.get(at..at + n)?.to_vec());
        at += n;
    }
    out.push(private.get(at..)?.to_vec());
    Some(out)
}

fn open_webm(path: &Path) -> Option<Opened> {
    let file = matroska_demuxer::MatroskaFile::open(BufReader::new(File::open(path).ok()?)).ok()?;
    let scale = file.info().timestamp_scale().get() as f64 / 1e9;
    let duration = file.info().duration().map_or(0., |d| d * scale);
    let (mut video, mut audio, mut vtrack, mut atrack, mut size) = (None, None, None, None, (0, 0));
    for t in file.tracks() {
        let id = t.track_number().get();
        match t.codec_id() {
            "V_VP8" | "V_VP9" if video.is_none() => {
                let codec = if t.codec_id() == "V_VP8" {
                    shiguredo_libvpx::DecoderCodec::Vp8
                } else {
                    shiguredo_libvpx::DecoderCodec::Vp9
                };
                video = shiguredo_libvpx::Decoder::new(shiguredo_libvpx::DecoderConfig::new(codec))
                    .ok()
                    .map(Video::Vpx);
            }
            "V_AV1" if video.is_none() => {
                video = shiguredo_aom::Decoder::new(shiguredo_aom::DecoderConfig::new())
                    .ok()
                    .map(|d| Video::Aom(Box::new(d)));
            }
            "A_OPUS" if audio.is_none() => audio = t.codec_private().and_then(Audio::opus),
            "A_VORBIS" if audio.is_none() => {
                audio = t
                    .codec_private()
                    .and_then(xiph_headers)
                    .and_then(|h| Audio::vorbis(&h));
            }
            _ => continue,
        }
        if t.codec_id().starts_with('V') && vtrack.is_none() && video.is_some() {
            vtrack = Some(id);
            if let Some(v) = t.video() {
                size = (v.pixel_width().get() as u32, v.pixel_height().get() as u32);
            }
        }
        if t.codec_id().starts_with('A') && atrack.is_none() && audio.is_some() {
            atrack = Some(id);
        }
    }
    Some(Opened {
        demux: Box::new(Webm {
            path: path.to_path_buf(),
            file,
            video: vtrack,
            audio: atrack,
            scale,
            keyframes: None,
        }),
        video,
        audio,
        size,
        duration,
    })
}

// ---- Ogg ----

struct Ogg {
    reader: ogg::PacketReader<BufReader<File>>,
    video: Option<u32>,
    audio: Option<u32>,
    /// Theora's granule shift and frame rate, Vorbis's or Opus's sample rate.
    theora: Option<(u8, f64)>,
    audio_rate: f64,
    /// The last sound time, for packets that end no page (Ogg stamps only a page's last).
    audio_at: f64,
    /// The next video frame's index (a page stamps only its last packet's).
    frame: i64,
}

impl Demux for Ogg {
    fn next(&mut self) -> Option<Packet> {
        loop {
            let p = self.reader.read_packet().ok()??;
            let serial = p.stream_serial();
            let granule = p.absgp_page() as i64;
            if Some(serial) == self.video {
                if p.data.first().is_none_or(|b| b & 0x80 != 0) {
                    continue;
                }
                let (shift, fps) = self.theora.unwrap_or((6, 25.));
                if p.last_in_page() && granule >= 0 {
                    // Theora 3.2.1 numbers frames from 1 in the granule.
                    self.frame = (granule >> shift) + (granule & ((1 << shift) - 1)) - 1;
                }
                let pts = self.frame.max(0) as f64 / fps;
                self.frame += 1;
                let keyframe = p.data.first().is_some_and(|b| b & 0x40 == 0);
                let granule = if p.last_in_page() { granule } else { -1 };
                return Some(Packet {
                    video: true,
                    pts,
                    data: p.data,
                    keyframe,
                    granule,
                });
            }
            if Some(serial) == self.audio {
                // Vorbis headers have an odd first byte; Opus's are named.
                let d = &p.data;
                if d.first()
                    .is_some_and(|b| b & 1 == 1 && !d.starts_with(b"Opus"))
                    && (d.get(1..7) == Some(b"vorbis"))
                    || d.starts_with(b"OpusHead")
                    || d.starts_with(b"OpusTags")
                {
                    continue;
                }
                if p.last_in_page() && granule >= 0 {
                    self.audio_at = granule as f64 / self.audio_rate;
                }
                return Some(Packet {
                    video: false,
                    pts: self.audio_at,
                    data: p.data,
                    keyframe: true,
                    granule,
                });
            }
        }
    }

    fn seek(&mut self, _: f64) -> bool {
        false
    }
}

fn open_ogg(path: &Path) -> Option<Opened> {
    let mut reader = ogg::PacketReader::new(BufReader::new(File::open(path).ok()?));
    let (mut theora, mut vserial, mut aserial) = (None, None, None);
    let mut vorbis: Vec<Vec<u8>> = Vec::new();
    let mut audio = None;
    // Headers come first in each stream; read until both streams' are in.
    while let Ok(Some(p)) = reader.read_packet() {
        let serial = p.stream_serial();
        let d = &p.data;
        if d.starts_with(b"\x80theora")
            || (Some(serial) == vserial && d.first().is_some_and(|b| b & 0x80 != 0))
        {
            vserial = Some(serial);
            let parser: &mut theora_rs::HeaderParser =
                theora.get_or_insert_with(theora_rs::HeaderParser::new);
            let _ = parser.push(&theora_rs::OggPacket {
                packet: d.clone(),
                b_o_s: p.first_in_stream(),
                e_o_s: false,
                granulepos: 0,
                packetno: 0,
            });
        } else if d.starts_with(b"\x01vorbis")
            || (Some(serial) == aserial && audio.is_none() && vorbis.len() < 3)
        {
            aserial = Some(serial);
            vorbis.push(d.clone());
            if vorbis.len() == 3 {
                audio = Audio::vorbis(&vorbis);
            }
        } else if d.starts_with(b"OpusHead") {
            aserial = Some(serial);
            audio = Audio::opus(d);
        } else if !d.starts_with(b"OpusTags") {
            break;
        }
    }
    let (video, info, shift_fps) = match theora.filter(|t| t.is_ready()) {
        Some(parser) => {
            let dec = parser.decoder().ok();
            let (info, _, _) = parser.into_parts();
            let shift = info.keyframe_granule_shift as u8;
            let fps = info.fps_numerator as f64 / info.fps_denominator.max(1) as f64;
            (
                dec.map(|d| Video::Theora(Box::new(d), Box::new(info.clone()))),
                Some(info),
                Some((shift, fps)),
            )
        }
        None => (None, None, None),
    };
    let size = info.map_or((0, 0), |i| (i.pic_width, i.pic_height));
    let audio_rate = audio.as_ref().map_or(48_000, Audio::rate) as f64;
    let duration = ogg_duration(path, vserial, aserial, shift_fps, audio_rate);
    // A fresh reader from the start: the demuxer skips header packets.
    let reader = ogg::PacketReader::new(BufReader::new(File::open(path).ok()?));
    let demux = Ogg {
        reader,
        video: video.as_ref().and(vserial),
        audio: audio.as_ref().and(aserial),
        theora: shift_fps,
        audio_rate,
        audio_at: 0.,
        frame: 0,
    };
    Some(Opened {
        demux: Box::new(demux),
        video,
        audio,
        size,
        duration,
    })
}

/// The last granule of the video (else audio) stream, from the file's tail.
fn ogg_duration(
    path: &Path,
    video: Option<u32>,
    audio: Option<u32>,
    theora: Option<(u8, f64)>,
    rate: f64,
) -> f64 {
    let Ok(mut f) = File::open(path) else {
        return 0.;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(256 * 1024);
    let mut tail = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut tail).is_err() {
        return 0.;
    }
    let (mut v, mut a) = (None, None);
    let mut i = 0;
    while i + 27 <= tail.len() {
        if &tail[i..i + 4] == b"OggS" {
            let granule = i64::from_le_bytes(tail[i + 6..i + 14].try_into().unwrap_or([0; 8]));
            let serial = u32::from_le_bytes(tail[i + 14..i + 18].try_into().unwrap_or([0; 4]));
            if granule >= 0 {
                if Some(serial) == video {
                    v = Some(granule);
                } else if Some(serial) == audio {
                    a = Some(granule);
                }
            }
            i += 4;
        } else {
            i += 1;
        }
    }
    match (v, theora, a) {
        (Some(g), Some((shift, fps)), _) => ((g >> shift) + (g & ((1 << shift) - 1))) as f64 / fps,
        (_, _, Some(g)) => g as f64 / rate,
        _ => 0.,
    }
}

// ---- the thread ----

fn open_any(path: &Path) -> Option<Opened> {
    let mut head = [0u8; 4];
    File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head == b"OggS" {
        open_ogg(path)
    } else {
        open_webm(path)
    }
}

fn decode_thread(path: &Path, shared: &Sync) {
    let (lock, cv) = &**shared;
    fn guard(m: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }
    let Some(mut o) = open_any(path) else {
        guard(lock).status.failed = true;
        return;
    };
    {
        let mut s = guard(lock);
        s.status.size = o.size;
        s.status.duration = o.duration;
        s.status.failed = o.video.is_none() && o.audio.is_none();
        s.sound = o.audio.as_ref().map(|a| Sound {
            rate: a.rate(),
            channels: a.channels(),
            samples: VecDeque::new(),
        });
    }
    let mut generation = 0;
    // Output before this time is decoded but dropped (an exact seek).
    let mut from = 0f64;
    let mut need_key = false;
    // The last frame before an exact seek's time: it is what shows at that time.
    let mut held: Option<Frame> = None;
    loop {
        let mut s = guard(lock);
        if s.closed {
            return;
        }
        if let Some(t) = s.seek.take() {
            generation = s.generation;
            drop(s);
            if !o.demux.seek(t) {
                match open_any(path) {
                    Some(fresh) => o = fresh,
                    None => return,
                }
            }
            from = t;
            need_key = true;
            held = None;
            continue;
        }
        let video_full = o.video.is_none() || s.frames.len() >= VIDEO_AHEAD;
        let sound_full = s.wall
            || s.sound.as_ref().is_none_or(|snd| {
                snd.samples.len() as f64 >= AUDIO_AHEAD * snd.rate as f64 * snd.channels as f64
            });
        if s.eof || (video_full && sound_full) {
            let _ = cv.wait_timeout(s, Duration::from_millis(20));
            continue;
        }
        drop(s);
        let Some(p) = o.demux.next() else {
            guard(lock).eof = true;
            continue;
        };
        if p.video {
            if need_key && !p.keyframe {
                continue;
            }
            need_key = false;
            let Some(v) = o.video.as_mut() else { continue };
            let pts = p.pts;
            let mut frames = v.decode(&p.data, p.granule);
            if pts + 0.0005 < from {
                held = frames.pop().or(held);
                continue;
            }
            let mut s = guard(lock);
            if s.generation != generation {
                continue;
            }
            if s.status.size == (0, 0)
                && let Some(f) = frames.first()
            {
                s.status.size = (f.width, f.height);
            }
            if let Some(h) = held.take()
                && pts > from + 0.0005
            {
                s.frames.push_back((from, h));
            }
            for f in frames {
                s.frames.push_back((pts, f));
            }
            s.status.ready = true;
        } else if let Some(a) = o.audio.as_mut() {
            let pcm = a.decode(&p.data);
            let mut s = guard(lock);
            if s.generation != generation {
                continue;
            }
            if o.video.is_none() {
                s.status.ready = true;
            }
            if p.pts + 0.0005 < from {
                continue;
            }
            if !s.wall
                && let Some(snd) = &mut s.sound
            {
                snd.samples.extend(pcm);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    /// Each clip's sound decodes to its 2s, 440 Hz sine (Vorbis or Opus, WebM or Ogg).
    #[test]
    fn the_sound_tracks_decode() {
        for (name, rate) in [
            ("red-blue-vp8.webm", 44_100),
            ("red-blue-vp9.webm", 48_000),
            ("red-blue-av1.webm", 48_000),
            ("red-blue.ogv", 44_100),
        ] {
            let mut o = open_any(&fixture(name)).expect(name);
            let mut audio = o.audio.take().expect(name);
            assert_eq!(audio.rate(), rate, "{name}");
            let mut pcm = Vec::new();
            while let Some(p) = o.demux.next() {
                if !p.video {
                    pcm.extend(audio.decode(&p.data));
                }
            }
            let secs = pcm.len() as f64 / audio.channels() as f64 / rate as f64;
            assert!((secs - 2.).abs() < 0.1, "{name}: {secs}s of sound");
            let peak = pcm.iter().fold(0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.05, "{name}: silent ({peak})");
        }
    }

    #[test]
    fn only_webm_and_ogg_are_sniffed() {
        assert!(sniff(&fixture("red-blue-vp8.webm")));
        assert!(sniff(&fixture("red-blue.ogv")));
        assert!(!sniff(&fixture("red-blue.mp4")));
    }
}
