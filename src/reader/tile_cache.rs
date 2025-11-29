use lru::LruCache;
use once_cell::sync::Lazy;
use std::cmp::max;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

const CACHE_CAPACITY_BYTES: usize = 512 * 1024 * 1024; // 512 MB upper bound

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TileKind {
    Chunked,
    Lzw,
}

#[derive(Clone, Eq, PartialEq)]
struct TileKey {
    path: Arc<str>,
    kind: TileKind,
    index: u32,
}

impl Hash for TileKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.kind.hash(state);
        self.index.hash(state);
    }
}

impl TileKey {
    fn new(path: &Path, kind: TileKind, index: usize) -> Self {
        let path_str: Box<str> = path.to_string_lossy().into_owned().into_boxed_str();
        TileKey {
            path: Arc::from(path_str),
            kind,
            index: index as u32,
        }
    }
}

struct CacheEntry {
    data: Arc<Vec<f32>>,
    size_bytes: usize,
}

struct TileCache {
    current_bytes: usize,
    capacity_bytes: usize,
    entries: LruCache<TileKey, CacheEntry>,
}

impl TileCache {
    fn new(capacity_bytes: usize) -> Self {
        TileCache {
            current_bytes: 0,
            capacity_bytes,
            entries: LruCache::unbounded(),
        }
    }

    fn get(&mut self, key: &TileKey) -> Option<Arc<Vec<f32>>> {
        self.entries.get(key).map(|entry| Arc::clone(&entry.data))
    }

    fn contains(&mut self, key: &TileKey) -> bool {
        self.entries.contains(key)
    }

    fn insert(&mut self, key: TileKey, data: Arc<Vec<f32>>, size_bytes: usize) {
        if size_bytes > self.capacity_bytes {
            return;
        }

        if let Some(old) = self.entries.pop(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.size_bytes);
        }

        while self.current_bytes + size_bytes > self.capacity_bytes {
            if let Some((_key, entry)) = self.entries.pop_lru() {
                self.current_bytes = self.current_bytes.saturating_sub(entry.size_bytes);
            } else {
                break;
            }
        }

        self.current_bytes = self.current_bytes.saturating_add(size_bytes);
        self.entries.put(key, CacheEntry { data, size_bytes });
    }
}

static TILE_CACHE: Lazy<Mutex<TileCache>> = Lazy::new(|| {
    let cap = max(CACHE_CAPACITY_BYTES, 64 * 1024 * 1024); // never below 64MB
    Mutex::new(TileCache::new(cap))
});

fn make_key(path: &Path, kind: TileKind, index: usize) -> TileKey {
    TileKey::new(path, kind, index)
}

pub fn get(path: &Path, kind: TileKind, index: usize) -> Option<Arc<Vec<f32>>> {
    let key = make_key(path, kind, index);
    TILE_CACHE.lock().unwrap().get(&key)
}

pub fn contains(path: &Path, kind: TileKind, index: usize) -> bool {
    let key = make_key(path, kind, index);
    TILE_CACHE.lock().unwrap().contains(&key)
}

pub fn insert(path: &Path, kind: TileKind, index: usize, data: Arc<Vec<f32>>) {
    let size_bytes = data.len() * std::mem::size_of::<f32>();
    let key = make_key(path, kind, index);
    TILE_CACHE.lock().unwrap().insert(key, data, size_bytes);
}
