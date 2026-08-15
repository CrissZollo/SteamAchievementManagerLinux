//! Background image fetching with a texture cache.
//!
//! Game capsules and achievement icons come from Steam's CDN. The Windows
//! version downloads them on `BackgroundWorker`s and feeds an `ImageList`;
//! here a small pool of threads decodes into `egui::ColorImage`, and the UI
//! thread uploads textures as results arrive.
//!
//! Only images that are actually on screen get requested, so opening a game
//! with 300 achievements does not queue 300 downloads up front.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::OnceLock;
use std::time::Duration;

/// Largest dimension we keep. Capsules are ~231x87 and icons 64x64, but the
/// legacy `logo` fallback can be much bigger.
const MAX_DIMENSION: u32 = 512;

/// Concurrent downloads. Enough to fill a scrolling grid quickly without
/// hammering the CDN.
const MAX_CONCURRENT: usize = 6;

/// Refuse absurd payloads; these assets are tens of kilobytes.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

enum Entry {
    Loading,
    Failed,
    Ready(egui::TextureHandle),
}

struct Loaded {
    url: String,
    image: Option<egui::ColorImage>,
}

pub struct ImageStore {
    entries: HashMap<String, Entry>,
    queue: VecDeque<String>,
    active: usize,
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageStore {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            entries: HashMap::new(),
            queue: VecDeque::new(),
            active: 0,
            tx,
            rx,
        }
    }

    /// Texture for `url`, requesting it if this is the first time it is asked
    /// for. Returns `None` while loading or after a failure.
    pub fn texture(&mut self, url: &str) -> Option<&egui::TextureHandle> {
        if !self.entries.contains_key(url) {
            self.entries.insert(url.to_string(), Entry::Loading);
            self.queue.push_back(url.to_string());
        }
        match self.entries.get(url) {
            Some(Entry::Ready(handle)) => Some(handle),
            _ => None,
        }
    }

    /// Take delivery of finished downloads and start queued ones.
    ///
    /// Call once per frame. Returns true if anything changed, so the caller
    /// can request a repaint.
    pub fn pump(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;

        while let Ok(loaded) = self.rx.try_recv() {
            self.active = self.active.saturating_sub(1);
            let entry = match loaded.image {
                Some(image) => {
                    let handle = ctx.load_texture(&loaded.url, image, egui::TextureOptions::LINEAR);
                    Entry::Ready(handle)
                }
                None => Entry::Failed,
            };
            self.entries.insert(loaded.url, entry);
            changed = true;
        }

        while self.active < MAX_CONCURRENT {
            let Some(url) = self.queue.pop_front() else {
                break;
            };
            self.spawn(url);
            self.active += 1;
            changed = true;
        }

        changed
    }

    fn spawn(&self, url: String) {
        let tx = self.tx.clone();
        // A detached thread per request is fine at this concurrency, and it
        // keeps the module free of a thread-pool dependency.
        std::thread::Builder::new()
            .name("sam-image".into())
            .spawn(move || {
                let image = fetch_and_decode(&url);
                // The receiver is gone only if the window has closed, in which
                // case dropping the result is exactly right.
                let _ = tx.send(Loaded { url, image });
            })
            .expect("spawning an image thread should not fail");
    }

    /// How many downloads are outstanding, for the status bar.
    pub fn outstanding(&self) -> usize {
        self.active + self.queue.len()
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent("sam-linux")
            .build()
            .into()
    })
}

fn fetch_and_decode(url: &str) -> Option<egui::ColorImage> {
    let mut response = agent().get(url).call().ok()?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
        .ok()?;
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?;

    // Shrink oversized art before it reaches VRAM.
    let decoded = if decoded.width() > MAX_DIMENSION || decoded.height() > MAX_DIMENSION {
        decoded.thumbnail(MAX_DIMENSION, MAX_DIMENSION)
    } else {
        decoded
    };

    let rgba = decoded.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_png() {
        // 1x1 opaque red PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let image = decode(png).expect("should decode");
        assert_eq!(image.size, [1, 1]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not an image").is_none());
    }
}
