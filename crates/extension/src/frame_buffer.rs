use collections::HashMap;
use gpui::RenderImage;
use parking_lot::Mutex;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameBufferId(pub u64);

/// Accessible from both the WASM host thread (which writes new frames)
/// and the main/UI thread (which reads them for display).
pub struct SharedFrameBuffer {
    pub id: FrameBufferId,
    pub width: u32,
    pub height: u32,
    current_frame: Mutex<Option<Arc<RenderImage>>>,
    generation: AtomicU64,
}

impl SharedFrameBuffer {
    pub fn new(id: FrameBufferId, width: u32, height: u32) -> Self {
        Self {
            id,
            width,
            height,
            current_frame: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Called from the WASM host thread to publish a new frame.
    pub fn set_frame(&self, frame: Arc<RenderImage>) {
        *self.current_frame.lock() = Some(frame);
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Called from the viewer on the main thread to read the latest frame.
    pub fn current_frame(&self) -> Option<Arc<RenderImage>> {
        self.current_frame.lock().clone()
    }

    /// The viewer can poll this to detect new frames without taking the lock.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

pub struct FrameBufferRegistry {
    buffers: Mutex<HashMap<FrameBufferId, Arc<SharedFrameBuffer>>>,
    next_id: AtomicU64,
}

impl FrameBufferRegistry {
    pub fn new() -> Self {
        Self {
            buffers: Mutex::new(HashMap::default()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn create(&self, width: u32, height: u32) -> Arc<SharedFrameBuffer> {
        let id = FrameBufferId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let buffer = Arc::new(SharedFrameBuffer::new(id, width, height));
        self.buffers.lock().insert(id, buffer.clone());
        buffer
    }

    pub fn get(&self, id: FrameBufferId) -> Option<Arc<SharedFrameBuffer>> {
        self.buffers.lock().get(&id).cloned()
    }

    pub fn remove(&self, id: FrameBufferId) -> Option<Arc<SharedFrameBuffer>> {
        self.buffers.lock().remove(&id)
    }

    pub fn list(&self) -> Vec<FrameBufferId> {
        self.buffers.lock().keys().copied().collect()
    }
}

impl gpui::Global for FrameBufferRegistry {}