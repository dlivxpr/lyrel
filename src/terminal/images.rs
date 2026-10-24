//! Pane-local derived graphics cache. The daemon alone owns resource and placement
//! lifetime; a Resource never implies a display. One worker decodes each retained
//! resource version once, and every placement shares its pixels and atlas marker.
//! Anchors are captured in stream order. Screen/history lifecycle beyond these
//! absolute rows belongs to the later screen-tracking work, not this cache.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use image::{Frame, RgbaImage};
use smallvec::SmallVec;

use lyrel_core::core::kitty_graphics::{GraphicsUpdate, Image, Placement, WireFormat};

#[derive(Clone)]
pub struct RenderPlacement {
    pub data: Arc<RenderImage>,
    pub painted: Arc<AtomicBool>,
    pub placement: Placement,
    pub anchor_row: i64,
    pub anchor_col: usize,
    width_px: u32,
    height_px: u32,
    revision: u64,
}

impl RenderPlacement {
    /// Destination crop and the full texture's destination rectangle. Passing
    /// these separately to GPUI selects atlas UVs without copying source pixels.
    /// Offsets are protocol device pixels; grid geometry is logical pixels.
    pub fn paint_bounds(
        &self,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        history_size: usize,
        display_offset: i32,
        scale: f32,
    ) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        let placement = &self.placement;
        let row = self.anchor_row - history_size as i64 + display_offset as i64;
        let natural_width = placement.w as f32 / scale;
        let natural_height = placement.h as f32 / scale;
        let available_width = (cell_width.as_f32() * placement.cols as f32
            - placement.offset_x as f32 / scale)
            .max(0.);
        let available_height = (line_height.as_f32() * placement.rows as f32
            - placement.offset_y as f32 / scale)
            .max(0.);
        let ratio = match (placement.cols > 0, placement.rows > 0) {
            (true, true) => {
                (available_width / natural_width).min(available_height / natural_height)
            }
            (true, false) => available_width / natural_width,
            (false, true) => available_height / natural_height,
            (false, false) => 1.,
        };
        let crop = Bounds::new(
            point(
                origin.x
                    + cell_width * self.anchor_col as f32
                    + px(placement.offset_x as f32 / scale),
                origin.y + line_height * row as f32 + px(placement.offset_y as f32 / scale),
            ),
            size(px(natural_width * ratio), px(natural_height * ratio)),
        );
        let sx = crop.size.width / placement.w.max(1) as f32;
        let sy = crop.size.height / placement.h.max(1) as f32;
        let full = Bounds::new(
            point(
                crop.origin.x - sx * placement.x as f32,
                crop.origin.y - sy * placement.y as f32,
            ),
            size(sx * self.width_px as f32, sy * self.height_px as f32),
        );
        (crop, full)
    }
}

struct AnchoredPlacement {
    placement: Placement,
    row: i64,
    col: usize,
    revision: u64,
}

struct DecodedResource {
    data: Arc<RenderImage>,
    width: u32,
    height: u32,
    painted: Arc<AtomicBool>,
}

struct Resource {
    pending: Option<Arc<Image>>,
    decoded: Option<DecodedResource>,
    revision: u64,
}

#[derive(Default)]
struct StoreInner {
    resources: BTreeMap<u64, Resource>,
    placements: BTreeMap<u64, AnchoredPlacement>,
    retired: Vec<Arc<RenderImage>>,
    closed: bool,
    reader_generation: u64,
    next_revision: u64,
}

#[derive(Default)]
struct SharedStore {
    state: Mutex<StoreInner>,
    ready: Condvar,
}

#[derive(Clone, Default)]
pub struct ImageStore(Arc<SharedStore>);

fn retire(resource: Resource, retired: &mut Vec<Arc<RenderImage>>) {
    if let Some(decoded) = resource.decoded
        && decoded.painted.load(Ordering::Acquire)
    {
        retired.push(decoded.data);
    }
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<RenderPlacement> {
        let inner = self.0.state.lock().unwrap();
        let mut images: Vec<_> = inner
            .placements
            .values()
            .filter_map(|anchored| {
                let decoded = inner
                    .resources
                    .get(&anchored.placement.resource)?
                    .decoded
                    .as_ref()?;
                Some(RenderPlacement {
                    data: decoded.data.clone(),
                    painted: decoded.painted.clone(),
                    placement: anchored.placement.clone(),
                    anchor_row: anchored.row,
                    anchor_col: anchored.col,
                    width_px: decoded.width,
                    height_px: decoded.height,
                    revision: anchored.revision,
                })
            })
            .collect();
        images.sort_unstable_by_key(|image| {
            (
                image.placement.z,
                image.placement.image_id,
                image.placement.key,
            )
        });
        images
    }

    /// Check both placement revision and resource revision under the same lock
    /// that removal/completion uses, before allocating an atlas tile.
    pub fn claim_for_paint(&self, image: &RenderPlacement) -> bool {
        let inner = self.0.state.lock().unwrap();
        let current = inner
            .placements
            .get(&image.placement.key)
            .is_some_and(|placement| placement.revision == image.revision)
            && inner
                .resources
                .get(&image.placement.resource)
                .and_then(|resource| resource.decoded.as_ref())
                .is_some_and(|resource| Arc::ptr_eq(&resource.data, &image.data));
        if current {
            image.painted.store(true, Ordering::Release);
        }
        current
    }

    pub fn take_retired(&self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.0.state.lock().unwrap().retired)
    }

    /// Call after the reader and its decode worker have stopped.
    pub fn take_for_release(&self) -> Vec<Arc<RenderImage>> {
        self.clear();
        self.take_retired()
    }

    /// Reset a detached/relinked pane's nonpersistent derived state. Clearing
    /// the resource map also invalidates any in-flight decode completion.
    pub fn clear(&self) {
        let mut inner = self.0.state.lock().unwrap();
        inner.placements.clear();
        let resources = std::mem::take(&mut inner.resources);
        for resource in resources.into_values() {
            retire(resource, &mut inner.retired);
        }
    }

    /// Invalidate the old reader synchronously before resetting/relinking the
    /// grid. A late frame, completion, or worker Drop then cannot affect the
    /// successor, and no UI-thread join is necessary.
    pub fn retire_reader(&self) {
        let mut inner = self.0.state.lock().unwrap();
        inner.reader_generation = inner.reader_generation.checked_add(1)
            .expect("graphics reader generation exhausted");
        inner.closed = true;
        drop(inner);
        self.0.ready.notify_all();
    }

    fn apply(&self, generation: u64, update: GraphicsUpdate, row: i64, col: usize) -> bool {
        let mut inner = self.0.state.lock().unwrap();
        if inner.closed || inner.reader_generation != generation {
            return false;
        }
        match update {
            GraphicsUpdate::Resource { key, image } => {
                inner.next_revision = inner.next_revision.wrapping_add(1);
                let revision = inner.next_revision;
                let resource = Resource {
                    pending: Some(image),
                    decoded: None,
                    revision,
                };
                if let Some(old) = inner.resources.insert(key, resource) {
                    retire(old, &mut inner.retired);
                }
                drop(inner);
                self.0.ready.notify_one();
                false
            }
            GraphicsUpdate::Place(placement) => {
                inner.next_revision = inner.next_revision.wrapping_add(1);
                let revision = inner.next_revision;
                inner.placements.insert(
                    placement.key,
                    AnchoredPlacement {
                        placement,
                        row,
                        col,
                        revision,
                    },
                );
                true
            }
            GraphicsUpdate::Remove {
                placements,
                resources,
            } => {
                // Never interpret kitty selectors or remove unnamed placements.
                for key in placements {
                    inner.placements.remove(&key);
                }
                for key in resources {
                    if let Some(resource) = inner.resources.remove(&key) {
                        retire(resource, &mut inner.retired);
                    }
                }
                true
            }
        }
    }
}

/// One encoded payload per daemon resource key, plus one in-flight decode.
/// Deferred work stays in the resource map, bounded by the authoritative core
/// byte/resource limits: no small queue may discard a still-referenced version.
pub struct DecodeWorker {
    store: ImageStore,
    generation: u64,
    handle: Option<JoinHandle<()>>,
}

impl DecodeWorker {
    pub fn spawn(store: ImageStore, wake: impl Fn() + Send + 'static) -> Self {
        Self::spawn_with_decoder(store, wake, decode)
    }

    fn spawn_with_decoder(
        store: ImageStore,
        wake: impl Fn() + Send + 'static,
        decoder: impl Fn(&mut Image) -> Option<(Arc<RenderImage>, u32, u32)> + Send + 'static,
    ) -> Self {
        let generation = {
            let mut inner = store.0.state.lock().unwrap();
            inner.reader_generation = inner.reader_generation.checked_add(1)
                .expect("graphics reader generation exhausted");
            inner.closed = false;
            inner.reader_generation
        };
        store.0.ready.notify_all();
        let worker_store = store.clone();
        let handle = std::thread::Builder::new()
            .name("lyrel-image-decode".into())
            .spawn(move || Self::run(worker_store, generation, wake, decoder))
            .expect("start graphics decode worker");
        Self {
            store,
            generation,
            handle: Some(handle),
        }
    }

    pub fn apply(&self, update: GraphicsUpdate, row: i64, col: usize) -> bool {
        self.store.apply(self.generation, update, row, col)
    }

    fn run(
        store: ImageStore,
        generation: u64,
        wake: impl Fn(),
        decoder: impl Fn(&mut Image) -> Option<(Arc<RenderImage>, u32, u32)>,
    ) {
        loop {
            let (key, revision, image) = {
                let mut inner = store.0.state.lock().unwrap();
                loop {
                    if inner.closed || inner.reader_generation != generation {
                        return;
                    }
                    if let Some((&key, resource)) = inner
                        .resources
                        .iter_mut()
                        .find(|(_, resource)| resource.pending.is_some())
                    {
                        break (key, resource.revision, resource.pending.take().unwrap());
                    }
                    inner = store.0.ready.wait(inner).unwrap();
                }
            };
            let mut image = Arc::unwrap_or_clone(image);
            let decoded = decoder(&mut image);
            let visible_change = {
                let mut inner = store.0.state.lock().unwrap();
                if inner.closed || inner.reader_generation != generation {
                    return;
                }
                let current = inner
                    .resources
                    .get_mut(&key)
                    .filter(|resource| resource.revision == revision);
                if let (Some(resource), Some((data, width, height))) = (current, decoded) {
                    resource.decoded = Some(DecodedResource {
                        data,
                        width,
                        height,
                        painted: Arc::new(AtomicBool::new(false)),
                    });
                    inner
                        .placements
                        .values()
                        .any(|placement| placement.placement.resource == key)
                } else {
                    false
                }
            };
            if visible_change {
                wake();
            }
        }
    }
}

impl Drop for DecodeWorker {
    fn drop(&mut self) {
        {
            let mut inner = self.store.0.state.lock().unwrap();
            if inner.reader_generation == self.generation {
                inner.closed = true;
            }
        }
        self.store.0.ready.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Decode once into straight-alpha BGRA, the native GPUI atlas representation.
/// Raw uncompressed RGBA transfers its allocation rather than copying pixels.
pub fn decode(img: &mut Image) -> Option<(Arc<RenderImage>, u32, u32)> {
    use lyrel_core::core::graphics_state::MAX_IMAGE_DIMENSION;
    use lyrel_core::core::kitty_graphics::MAX_IMAGE_BYTES;

    let bytes = (img.width as usize).checked_mul(img.height as usize)?.checked_mul(4)?;
    if bytes == 0 || bytes > MAX_IMAGE_BYTES || img.data.len() > MAX_IMAGE_BYTES
        || img.width > MAX_IMAGE_DIMENSION || img.height > MAX_IMAGE_DIMENSION
        || (img.format == WireFormat::Png && img.compressed)
    {
        return None;
    }
    let (mut rgba, w, h) = match img.format {
        WireFormat::Png => {
            let mut reader = image::ImageReader::with_format(
                std::io::Cursor::new(&img.data),
                image::ImageFormat::Png,
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(img.width);
            limits.max_image_height = Some(img.height);
            limits.max_alloc = Some(2 * lyrel_core::core::kitty_graphics::MAX_IMAGE_BYTES as u64);
            reader.limits(limits);
            let buf = reader.decode().ok()?.into_rgba8();
            let (w, h) = buf.dimensions();
            (buf.into_raw(), w, h)
        }
        WireFormat::Rgb | WireFormat::Rgba => {
            let (w, h) = (img.width, img.height);
            (img.take_rgba8()?, w, h)
        }
    };
    let bytes = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if w != img.width || h != img.height {
        return None;
    }
    if w == 0 || h == 0 || rgba.len() != bytes {
        return None;
    }
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let render = RenderImage::new(SmallVec::from_elem(
        Frame::new(RgbaImage::from_raw(w, h, rgba)?),
        1,
    ));
    Some((Arc::new(render), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, atomic::AtomicUsize};

    fn resource(key: u64, red: u8) -> GraphicsUpdate {
        GraphicsUpdate::Resource {
            key,
            image: Arc::new(Image {
                width: 1,
                height: 1,
                format: WireFormat::Rgba,
                compressed: false,
                data: vec![red, 20, 30, 128],
            }),
        }
    }

    fn placement(key: u64, resource: u64) -> GraphicsUpdate {
        let mut placement = Placement::default();
        placement.key = key;
        placement.resource = resource;
        placement.w = 1;
        placement.h = 1;
        GraphicsUpdate::Place(placement)
    }

    fn wait_for(store: &ImageStore, predicate: impl Fn(&[RenderPlacement]) -> bool) {
        for _ in 0..200 {
            if predicate(&store.snapshot()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(predicate(&store.snapshot()), "decode did not complete");
    }

    #[test]
    fn removed_in_flight_resource_cannot_resurrect_or_overwrite_replacement() {
        let store = ImageStore::new();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first = Arc::new(AtomicBool::new(true));
        let wakes = Arc::new(AtomicUsize::new(0));
        let worker = DecodeWorker::spawn_with_decoder(
            store.clone(),
            {
                let wakes = wakes.clone();
                move || {
                    wakes.fetch_add(1, Ordering::SeqCst);
                }
            },
            {
                let entered = entered.clone();
                let release = release.clone();
                move |image| {
                    if first.swap(false, Ordering::SeqCst) {
                        entered.wait();
                        release.wait();
                    }
                    decode(image)
                }
            },
        );
        worker.apply(resource(1, 1), 0, 0);
        worker.apply(placement(10, 1), 3, 4);
        entered.wait();
        worker.apply(
            GraphicsUpdate::Remove {
                placements: vec![10],
                resources: vec![1],
            },
            0,
            0,
        );
        worker.apply(resource(2, 90), 0, 0);
        worker.apply(placement(10, 2), 40, 8);
        release.wait();
        wait_for(&store, |images| images.len() == 1);
        let images = store.snapshot();
        assert_eq!((images[0].anchor_row, images[0].anchor_col), (40, 8));
        assert_eq!(images[0].data.as_bytes(0).unwrap(), &[30, 20, 90, 128]);
        drop(worker);
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "cancelled decode never wakes or lands"
        );
    }

    #[test]
    fn deferred_resources_keep_every_referenced_version_and_latest_placement_revision() {
        let store = ImageStore::new();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first = Arc::new(AtomicBool::new(true));
        let worker = DecodeWorker::spawn_with_decoder(store.clone(), || {}, {
            let entered = entered.clone();
            let release = release.clone();
            move |image| {
                if first.swap(false, Ordering::SeqCst) {
                    entered.wait();
                    release.wait();
                }
                decode(image)
            }
        });
        worker.apply(resource(1, 1), 0, 0);
        worker.apply(placement(1, 1), 1, 0);
        entered.wait();
        for key in 2..=9 {
            worker.apply(resource(key, key as u8), 0, 0);
            worker.apply(placement(key, key), key as i64, 0);
        }
        worker.apply(placement(1, 1), 77, 9);
        worker.apply(
            GraphicsUpdate::Remove {
                placements: vec![2],
                resources: vec![],
            },
            0,
            0,
        );
        release.wait();
        wait_for(&store, |images| images.len() == 8);
        let images = store.snapshot();
        for image in &images {
            assert_eq!(
                image.data.as_bytes(0).unwrap(),
                &[30, 20, image.placement.resource as u8, 128]
            );
        }
        let updated = images
            .iter()
            .find(|image| image.placement.key == 1)
            .unwrap();
        assert_eq!((updated.anchor_row, updated.anchor_col), (77, 9));
        assert!(!images.iter().any(|image| image.placement.key == 2));
        // Removing a placement does not free its resource; daemon may display it later.
        worker.apply(placement(20, 2), 88, 0);
        wait_for(&store, |images| {
            images.iter().any(|image| image.placement.key == 20)
        });
        assert_eq!(
            store
                .snapshot()
                .iter()
                .find(|image| image.placement.key == 20)
                .unwrap()
                .data
                .as_bytes(0)
                .unwrap(),
            &[30, 20, 2, 128]
        );
    }

    #[test]
    fn retired_decoder_cannot_apply_complete_or_close_its_successor() {
        let store = ImageStore::new();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let old = DecodeWorker::spawn_with_decoder(store.clone(), || {}, {
            let entered = entered.clone();
            let release = release.clone();
            move |image| {
                entered.wait();
                release.wait();
                decode(image)
            }
        });
        old.apply(resource(1, 10), 0, 0);
        old.apply(placement(1, 1), 1, 2);
        entered.wait();
        // Relink retires the reader synchronously, but never joins its decoder
        // on the UI thread. Hold the old real decode until the new one paints.
        store.retire_reader();
        store.clear();
        let new = DecodeWorker::spawn(store.clone(), || {});
        new.apply(resource(2, 90), 0, 0);
        new.apply(placement(2, 2), 4, 5);
        wait_for(&store, |images| images.len() == 1);
        let current = store.snapshot();
        assert_eq!(current[0].data.as_bytes(0).unwrap(), &[30, 20, 90, 128]);
        assert!(store.claim_for_paint(&current[0]));
        assert!(!old.apply(GraphicsUpdate::Remove {
            placements: vec![2], resources: vec![2],
        }, 0, 0), "a retired reader cannot delete new link images");
        old.apply(resource(3, 11), 0, 0);
        old.apply(placement(3, 3), 9, 9);
        release.wait();
        drop(old);
        // Old Drop occurs after the successor is usable. It must neither land
        // its stale pixel nor close the new worker's wake/admission gate.
        new.apply(resource(4, 100), 0, 0);
        new.apply(placement(4, 4), 6, 7);
        wait_for(&store, |images| images.len() == 2);
        assert!(store.snapshot().iter().all(|image| image.placement.resource != 1 && image.placement.resource != 3));
        new.apply(GraphicsUpdate::Remove {
            placements: vec![2, 4], resources: vec![2, 4],
        }, 0, 0);
        assert!(store.snapshot().is_empty(), "successor deletion remains live");
    }

    #[test]
    fn shared_resource_has_one_atlas_retirement_and_stale_placement_cannot_paint() {
        let store = ImageStore::new();
        let worker = DecodeWorker::spawn(store.clone(), || {});
        worker.apply(resource(1, 50), 0, 0);
        worker.apply(placement(10, 1), 1, 0);
        worker.apply(placement(11, 1), 2, 0);
        wait_for(&store, |images| images.len() == 2);
        let images = store.snapshot();
        assert!(Arc::ptr_eq(&images[0].data, &images[1].data));
        assert!(Arc::ptr_eq(&images[0].painted, &images[1].painted));
        assert!(store.claim_for_paint(&images[0]));
        worker.apply(placement(10, 1), 9, 0);
        assert!(!store.claim_for_paint(&images[0]));
        worker.apply(
            GraphicsUpdate::Remove {
                placements: vec![10],
                resources: vec![],
            },
            0,
            0,
        );
        assert!(
            store.take_retired().is_empty(),
            "other placement still uses the atlas tile"
        );
        assert!(store.claim_for_paint(&images[1]));
        worker.apply(
            GraphicsUpdate::Remove {
                placements: vec![11],
                resources: vec![1],
            },
            0,
            0,
        );
        assert!(!store.claim_for_paint(&images[1]));
        let retired = store.take_retired();
        assert_eq!(retired.len(), 1);
        assert!(Arc::ptr_eq(&retired[0], &images[0].data));
        assert!(store.take_for_release().is_empty());
        worker.apply(resource(2, 100), 0, 0);
        worker.apply(placement(20, 2), 0, 0);
        worker.apply(placement(21, 2), 0, 0);
        wait_for(&store, |images| images.len() == 2);
        let current = store.snapshot();
        assert!(store.claim_for_paint(&current[0]));
        assert!(store.claim_for_paint(&current[1]));
        drop(worker);
        let released = store.take_for_release();
        assert_eq!(
            released.len(),
            1,
            "pane release evicts each shared atlas resource once"
        );
        assert!(Arc::ptr_eq(&released[0], &current[0].data));
        assert!(store.snapshot().is_empty());
        assert!(store.take_for_release().is_empty());
    }
}
