//! Remote image fetching, decoding and caching.
//!
//! Downloads run on a worker thread so the UI never blocks on the network.
//! Decoding goes through WIC, which is already part of Windows — no image
//! crate, and it converts straight to premultiplied BGRA, the exact format the
//! canvas composites.
//!
//! Results are cached twice: in memory for the session, and on disk as raw
//! pixels so a restart costs neither a download nor a decode.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use crate::config::cache_dir;
use crate::http;
use crate::model::Participant;
use crate::ui::render::Bitmap;
use crate::ui::Notifier;

pub const CDN_HOST: &str = "cdn.discordapp.com";

/// Fetch at twice the drawn size so the circular downscale stays sharp.
const OVERSAMPLE: u32 = 2;

struct Request {
    key: String,
    host: String,
    path: String,
    size: u32,
}

pub struct ImageCache {
    bitmaps: HashMap<String, Bitmap>,
    /// Keys already queued, so a slow download is not requested every repaint.
    in_flight: HashSet<String>,
    /// Keys that failed, so a 404 is not retried forever.
    failed: HashSet<String>,
    requests: Sender<Request>,
    results: Receiver<(String, Option<Bitmap>)>,
}

impl ImageCache {
    /// `notify` is called on the worker thread whenever a fetch completes.
    pub fn new(notify: Notifier) -> Self {
        let (requests, request_rx) = channel::<Request>();
        let (result_tx, results) = channel::<(String, Option<Bitmap>)>();

        std::thread::Builder::new()
            .name("image-fetch".to_string())
            .spawn(move || worker(request_rx, result_tx, notify))
            .expect("spawn image worker");

        ImageCache {
            bitmaps: HashMap::new(),
            in_flight: HashSet::new(),
            failed: HashSet::new(),
            requests,
            results,
        }
    }

    /// Look up an image, queueing a fetch if it is not here yet.
    ///
    /// `key` must identify the image content; `size` is folded in so the same
    /// picture at two sizes does not collide.
    pub fn get(&mut self, key: &str, host: &str, path: &str, size: u32) -> Option<&Bitmap> {
        let full_key = format!("{key}@{size}");

        if self.bitmaps.contains_key(&full_key) {
            return self.bitmaps.get(&full_key);
        }

        if !self.in_flight.contains(&full_key) && !self.failed.contains(&full_key) {
            self.in_flight.insert(full_key.clone());
            let _ = self.requests.send(Request {
                key: full_key,
                host: host.to_string(),
                path: path.to_string(),
                size,
            });
        }

        None
    }

    pub fn avatar(&mut self, participant: &Participant, size: u32) -> Option<&Bitmap> {
        let key = participant.avatar_key();
        let path = participant.avatar_path(size * OVERSAMPLE);
        self.get(&key, CDN_HOST, &path, size)
    }

    /// Fetch by absolute URL, as returned by Discord's `GET_GUILD`.
    pub fn from_url(&mut self, key: &str, url: &str, size: u32) -> Option<&Bitmap> {
        let (host, path) = split_url(url)?;
        // Discord's CDN honours a size query, and asking for a small image
        // saves both bandwidth and decode time.
        let separator = if path.contains('?') { '&' } else { '?' };
        let sized = format!("{path}{separator}size={}", (size * OVERSAMPLE).next_power_of_two());
        self.get(key, &host, &sized, size)
    }

    /// Collect finished downloads. Returns true if anything new arrived, which
    /// is the UI's cue to repaint.
    pub fn collect(&mut self) -> bool {
        let mut changed = false;
        while let Ok((key, bitmap)) = self.results.try_recv() {
            self.in_flight.remove(&key);
            match bitmap {
                Some(bitmap) => {
                    self.bitmaps.insert(key, bitmap);
                    changed = true;
                }
                None => {
                    self.failed.insert(key);
                }
            }
        }
        changed
    }

    /// Drop everything, e.g. after a DPI change made every cached size wrong.
    pub fn clear(&mut self) {
        self.bitmaps.clear();
        self.failed.clear();
    }
}

/// Split `https://host/path?query` into its host and path parts.
fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    match rest.find('/') {
        Some(slash) => Some((rest[..slash].to_string(), rest[slash..].to_string())),
        None => Some((rest.to_string(), "/".to_string())),
    }
}

fn worker(
    requests: Receiver<Request>,
    results: Sender<(String, Option<Bitmap>)>,
    notify: Notifier,
) {
    unsafe {
        // WIC is COM; this thread needs an apartment.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    while let Ok(request) = requests.recv() {
        let bitmap = load(&request);
        let produced = bitmap.is_some();
        if results.send((request.key, bitmap)).is_err() {
            break;
        }
        if produced {
            notify.notify();
        }
    }
}

fn load(request: &Request) -> Option<Bitmap> {
    if let Some(bitmap) = read_disk_cache(&request.key) {
        return Some(bitmap);
    }

    let response = http::get(&request.host, &request.path).ok()?;
    if !response.is_success() {
        return None;
    }

    let bitmap = decode(&response.body, request.size)?;
    write_disk_cache(&request.key, &bitmap);
    Some(bitmap)
}

/// Decode an image to premultiplied BGRA at exactly `size` x `size`.
fn decode(bytes: &[u8], size: u32) -> Option<Bitmap> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;

        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;

        let decoder = factory
            .CreateDecoderFromStream(
                &stream.cast::<windows::Win32::System::Com::IStream>().ok()?,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;

        let frame = decoder.GetFrame(0).ok()?;

        // Scale first, then convert: scaling in the source format keeps the
        // converter from doing the work twice.
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, size, size, WICBitmapInterpolationModeFant)
            .ok()?;

        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaler,
                // Premultiplied BGRA is exactly what the canvas blends.
                &GUID_WICPixelFormat32bppPBGRA as *const GUID,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;

        let mut bitmap = Bitmap::new(size as i32, size as i32);
        let stride = size * 4;
        let buffer = std::slice::from_raw_parts_mut(
            bitmap.pixels.as_mut_ptr() as *mut u8,
            bitmap.pixels.len() * 4,
        );

        converter.CopyPixels(std::ptr::null(), stride, buffer).ok()?;
        Some(bitmap)
    }
}

/// Raw pixels on disk: `[width u32][height u32][premultiplied BGRA]`.
/// Cheaper to read back than re-decoding a PNG, and trivially versionable by
/// changing the file name.
fn cache_file(key: &str) -> std::path::PathBuf {
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    cache_dir().join(format!("{safe}.img"))
}

fn read_disk_cache(key: &str) -> Option<Bitmap> {
    let bytes = std::fs::read(cache_file(key)).ok()?;
    if bytes.len() < 8 {
        return None;
    }

    let width = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as i32;
    let height = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as i32;
    let expected = (width as usize) * (height as usize) * 4;

    if width <= 0 || height <= 0 || bytes.len() != 8 + expected {
        return None;
    }

    let pixels = bytes[8..]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    Some(Bitmap {
        width,
        height,
        pixels,
    })
}

fn write_disk_cache(key: &str, bitmap: &Bitmap) {
    let mut bytes = Vec::with_capacity(8 + bitmap.pixels.len() * 4);
    bytes.extend_from_slice(&(bitmap.width as u32).to_le_bytes());
    bytes.extend_from_slice(&(bitmap.height as u32).to_le_bytes());
    for pixel in &bitmap.pixels {
        bytes.extend_from_slice(&pixel.to_le_bytes());
    }

    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(cache_file(key), bytes);
}
