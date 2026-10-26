//! Pane-local derived graphics cache. The live daemon owns resource and placement
//! lifetime; a Resource never implies a display. One worker decodes each retained
//! resource version once, and every placement shares its pixels and atlas marker.
//! Coordinates and screen membership arrive from the daemon. Only an explicitly
//! retired reader permits local user clear of the retained presentation.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Weak};
use std::thread::JoinHandle;

use gpui_kit::{Bounds, Pixels, Point, RenderImage, point, px, size};
use image::{Frame, RgbaImage};
use smallvec::SmallVec;

use lyrel_core::core::kitty_graphics::{
    GraphicsUpdate, Image, PlaceholderSlice, Placement, WireFormat, placeholder_prototype,
};

#[derive(Clone)]
pub struct RenderPlacement {
    pub data: Option<Arc<RenderImage>>,
    pub evicted: bool,
    pub painted: Arc<AtomicBool>,
    pub placement: Placement,
    pub anchor_row: i64,
    pub anchor_col: usize,
    width_px: u32,
    height_px: u32,
    revision: u64,
    resource_revision: u64,
    history_origin: i64,
    placeholder: Option<(i32, PlaceholderSlice)>,
    _residency: Option<Arc<Residency>>,
    _metadata: (Arc<Reservation>, Arc<Reservation>),
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
        _history_size: usize,
        display_offset: i32,
        scale: f32,
    ) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let scale = if scale.is_finite() && scale > 0. {
            scale
        } else {
            1.
        };
        let placement = &self.placement;
        if let Some((row, slice)) = &self.placeholder {
            let geometry = rio_vt::ansi::kitty_virtual::compute_run_geometry(
                &slice.run,
                placement.span_cols,
                placement.span_rows,
                self.width_px,
                self.height_px,
                (placement.x, placement.y, placement.w, placement.h),
                cell_width.as_f32(),
                line_height.as_f32(),
                origin.x.as_f32(),
                origin.y.as_f32() + *row as f32 * line_height.as_f32(),
                0,
                slice.column,
            );
            let Some(geometry) = geometry else {
                let empty = Bounds::new(origin, size(px(0.), px(0.)));
                return (empty, empty);
            };
            let crop = Bounds::new(
                point(px(geometry.x), px(geometry.y)),
                size(px(geometry.width), px(geometry.height)),
            );
            let [u0, v0, u1, v1] = geometry.source_rect;
            let width = geometry.width / (u1 - u0);
            let height = geometry.height / (v1 - v0);
            let full = Bounds::new(
                point(px(geometry.x - u0 * width), px(geometry.y - v0 * height)),
                size(px(width), px(height)),
            );
            return (crop, full);
        }
        let row = self.anchor_row - self.history_origin + display_offset as i64;
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
        let clip_top = origin.y + line_height * (row + i64::from(placement.clip_top)) as f32;
        let clip_bottom = origin.y + line_height * (row + i64::from(placement.clip_bottom)) as f32;
        let clipped = crop.intersect(&Bounds::new(
            point(crop.origin.x, clip_top),
            size(crop.size.width, (clip_bottom - clip_top).max(px(0.))),
        ));
        (clipped, full)
    }
}

struct AnchoredPlacement {
    placement: Placement,
    revision: u64,
    metadata: Arc<Reservation>,
}

// Each offer reserves actual source bytes, decoder peak, CPU output, and a
// whole minimum-rounded atlas page before the daemon accepts its upload.
const PANE_CACHE_BYTES: usize = 384 * 1024 * 1024;
const DESKTOP_CACHE_BYTES: usize = 1024 * 1024 * 1024;
const PANE_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const DESKTOP_SOURCE_BYTES: usize = 256 * 1024 * 1024;
const PANE_DECODED_CAPACITY: u64 = 128 * 1024 * 1024;
const PANE_METADATA_BYTES: usize = lyrel_core::daemon::protocol::MAX_PANE_GRAPHICS_METADATA_BYTES;
const DESKTOP_METADATA_BYTES: usize =
    lyrel_core::daemon::protocol::MAX_AGGREGATE_GRAPHICS_METADATA_BYTES;
const OWNER_METADATA_BYTES: u64 =
    lyrel_core::daemon::protocol::GRAPHICS_METADATA_ENTRY_BYTES as u64;

fn gpu_page_bytes(width: u32, height: u32) -> Option<usize> {
    (width.max(1024) as usize)
        .checked_mul(height.max(1024) as usize)?
        .checked_mul(4)
}

type InspectPressure = dyn Fn(&ImageStore) -> Option<(u64, u64, bool)> + Send + Sync;
struct PressureConsumer {
    store: Weak<SharedStore>,
    inspect: Box<InspectPressure>,
    send: Box<dyn Fn(u64) + Send + Sync>,
}

static PRESSURE_CONSUMERS: LazyLock<Mutex<Vec<Weak<PressureConsumer>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static RECEIVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Budget {
    used: AtomicUsize,
    limit: usize,
}

impl Budget {
    fn new(limit: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit,
        }
    }

    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .is_ok()
    }
}

#[cfg(test)]
thread_local! {
    static TEST_CACHE_LIMITS: std::cell::RefCell<Option<(usize, Arc<Budget>, Arc<Budget>)>> = const { std::cell::RefCell::new(None) };
    static TEST_SOURCES: Arc<Budget> = Arc::new(Budget::new(DESKTOP_SOURCE_BYTES));
    static TEST_METADATA_LIMITS: std::cell::RefCell<Option<(usize, Arc<Budget>)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn with_test_cache_limits<T>(pane: usize, aggregate: usize, f: impl FnOnce() -> T) -> T {
    struct Reset(Option<(usize, Arc<Budget>, Arc<Budget>)>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_CACHE_LIMITS.with(|limits| *limits.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(TEST_CACHE_LIMITS.with(|limits| {
        limits.replace(Some((
            pane,
            Arc::new(Budget::new(aggregate)),
            Arc::new(Budget::new(DESKTOP_SOURCE_BYTES)),
        )))
    }));
    f()
}

#[cfg(test)]
pub(super) fn with_test_metadata_limits<T>(
    pane: usize,
    aggregate: usize,
    f: impl FnOnce() -> T,
) -> T {
    struct Reset(Option<(usize, Arc<Budget>)>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_METADATA_LIMITS.with(|limits| *limits.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(
        TEST_METADATA_LIMITS
            .with(|limits| limits.replace(Some((pane, Arc::new(Budget::new(aggregate)))))),
    );
    f()
}

struct Reservation {
    pane: Arc<Budget>,
    aggregate: Arc<Budget>,
    bytes: usize,
}

impl Reservation {
    fn new(pane: &Arc<Budget>, aggregate: &Arc<Budget>, bytes: usize) -> Option<Self> {
        if !pane.reserve(bytes) {
            return None;
        }
        if !aggregate.reserve(bytes) {
            pane.used.fetch_sub(bytes, Ordering::AcqRel);
            return None;
        }
        Some(Self {
            pane: pane.clone(),
            aggregate: aggregate.clone(),
            bytes,
        })
    }

    fn shrink(&mut self, bytes: usize) {
        let released = self.bytes - bytes;
        self.pane.used.fetch_sub(released, Ordering::AcqRel);
        self.aggregate.used.fetch_sub(released, Ordering::AcqRel);
        self.bytes = bytes;
    }

    fn grow(&mut self, bytes: usize) -> Option<()> {
        let additional = bytes.saturating_sub(self.bytes);
        let mut extra = Self::new(&self.pane, &self.aggregate, additional)?;
        self.bytes += extra.bytes;
        extra.bytes = 0;
        Some(())
    }

    fn split(&mut self, bytes: usize) -> Self {
        self.bytes -= bytes;
        Self {
            pane: self.pane.clone(),
            aggregate: self.aggregate.clone(),
            bytes,
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.pane.used.fetch_sub(self.bytes, Ordering::AcqRel);
        self.aggregate.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

// Offers reserve every retained CPU/GPU footprint atomically, but only one
// decoder runs per store, including across reader generations. Each pending or
// active decode owns a claim on the largest scratch requirement. Cancellation
// cannot return the active decoder's claim before its buffers have been freed.
struct DecodeScratchBudget {
    state: Mutex<DecodeScratchState>,
    decoder: Mutex<()>,
}

struct DecodeScratchState {
    claims: BTreeMap<usize, usize>,
    reservation: Reservation,
}

struct DecodeScratch {
    budget: Arc<DecodeScratchBudget>,
    bytes: usize,
}

impl DecodeScratchBudget {
    fn new(pane: &Arc<Budget>, aggregate: &Arc<Budget>) -> Self {
        Self {
            state: Mutex::new(DecodeScratchState {
                claims: BTreeMap::new(),
                reservation: Reservation::new(pane, aggregate, 0).unwrap(),
            }),
            decoder: Mutex::new(()),
        }
    }

    fn claim(self: &Arc<Self>, bytes: usize) -> Option<DecodeScratch> {
        let mut state = self.state.lock().unwrap();
        if bytes > state.reservation.bytes {
            state.reservation.grow(bytes)?;
        }
        *state.claims.entry(bytes).or_default() += 1;
        Some(DecodeScratch {
            budget: self.clone(),
            bytes,
        })
    }
}

impl Drop for DecodeScratch {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap();
        let count = state.claims.get_mut(&self.bytes).unwrap();
        *count -= 1;
        if *count == 0 {
            state.claims.remove(&self.bytes);
        }
        let maximum = state.claims.last_key_value().map_or(0, |(&bytes, _)| bytes);
        state.reservation.shrink(maximum);
    }
}

struct Residency {
    data: Arc<RenderImage>,
    _cpu: Reservation,
    gpu: Mutex<Option<Reservation>>,
    registered: AtomicBool,
}

struct WindowImage {
    residency: Weak<Residency>,
    displayed: Weak<AtomicBool>,
}

thread_local! {
    static WINDOW_IMAGES: std::cell::RefCell<HashMap<gpui_kit::WindowId, Vec<WindowImage>>> = std::cell::RefCell::new(HashMap::new());
}

struct OfferedResource {
    key: u64,
    width: u32,
    height: u32,
    encoded_bytes: usize,
    peak: Option<Reservation>,
    scratch: Option<DecodeScratch>,
    source: Option<Reservation>,
    metadata: Option<Arc<Reservation>>,
    generation: Option<u64>,
}

struct DecodedResource {
    residency: Arc<Residency>,
}

struct Resource {
    pending: Option<Arc<Image>>,
    decoded: Option<DecodedResource>,
    width: u32,
    height: u32,
    evicted: bool,
    revision: u64,
    painted: Arc<AtomicBool>,
    peak: Option<Reservation>,
    scratch: Option<DecodeScratch>,
    source: Option<Reservation>,
    received: u64,
    pressure_requested: bool,
    metadata: Arc<Reservation>,
}

#[derive(Default)]
struct StoreInner {
    resources: BTreeMap<u64, Resource>,
    placements: BTreeMap<u64, AnchoredPlacement>,
    retired: Vec<Arc<Residency>>,
    offers: BTreeMap<u64, OfferedResource>,
    closed: bool,
    reader_generation: u64,
    next_revision: u64,
    screen: u64,
    history_origin: i64,
    viewport_epoch: u64,
    local_origin: i64,
    viewport_known: bool,
    cell_width: u16,
    cell_height: u16,
}

struct SharedStore {
    state: Mutex<StoreInner>,
    ready: Condvar,
    pane_budget: Arc<Budget>,
    aggregate_budget: Arc<Budget>,
    decode_scratch: Arc<DecodeScratchBudget>,
    pane_source: Arc<Budget>,
    aggregate_source: Arc<Budget>,
    pane_metadata: Arc<Budget>,
    aggregate_metadata: Arc<Budget>,
    pressure: Mutex<Option<Arc<PressureConsumer>>>,
}

impl Default for SharedStore {
    fn default() -> Self {
        static AGGREGATE: LazyLock<Arc<Budget>> =
            LazyLock::new(|| Arc::new(Budget::new(DESKTOP_CACHE_BYTES)));
        static METADATA: LazyLock<Arc<Budget>> =
            LazyLock::new(|| Arc::new(Budget::new(DESKTOP_METADATA_BYTES)));
        #[cfg(not(test))]
        static SOURCES: LazyLock<Arc<Budget>> =
            LazyLock::new(|| Arc::new(Budget::new(DESKTOP_SOURCE_BYTES)));
        let pane_budget = Arc::new(Budget::new(PANE_CACHE_BYTES));
        let aggregate_budget = AGGREGATE.clone();
        #[cfg(not(test))]
        let sources = SOURCES.clone();
        #[cfg(test)]
        let (pane_budget, aggregate_budget, sources) = TEST_CACHE_LIMITS.with(|limits| {
            limits
                .borrow()
                .as_ref()
                .map(|(pane, aggregate, sources)| {
                    (
                        Arc::new(Budget::new(*pane)),
                        aggregate.clone(),
                        sources.clone(),
                    )
                })
                .unwrap_or_else(|| (pane_budget, aggregate_budget, TEST_SOURCES.with(Arc::clone)))
        });
        let pane_metadata = Arc::new(Budget::new(PANE_METADATA_BYTES));
        let aggregate_metadata = METADATA.clone();
        #[cfg(test)]
        let (pane_metadata, aggregate_metadata) = TEST_METADATA_LIMITS.with(|limits| {
            limits
                .borrow()
                .as_ref()
                .map(|(pane, aggregate)| (Arc::new(Budget::new(*pane)), aggregate.clone()))
                .unwrap_or((pane_metadata, aggregate_metadata))
        });
        Self {
            state: Mutex::default(),
            ready: Condvar::new(),
            decode_scratch: Arc::new(DecodeScratchBudget::new(&pane_budget, &aggregate_budget)),
            pane_budget,
            aggregate_budget,
            pane_source: Arc::new(Budget::new(PANE_SOURCE_BYTES)),
            aggregate_source: sources,
            pressure: Mutex::default(),
            pane_metadata,
            aggregate_metadata,
        }
    }
}

#[derive(Clone, Default)]
pub struct ImageStore(Arc<SharedStore>);

fn retire(resource: Resource, retired: &mut Vec<Arc<Residency>>) {
    if let Some(decoded) = resource.decoded
        && resource.painted.load(Ordering::Acquire)
    {
        retired.push(decoded.residency);
    }
}

fn take_metadata_offer(
    inner: &mut StoreInner,
    generation: u64,
    key: u64,
) -> Option<Arc<Reservation>> {
    let token = inner.offers.iter().find_map(|(&token, offer)| {
        (offer.key == key
            && offer.generation == Some(generation)
            && offer.width == 0
            && offer.height == 0)
            .then_some(token)
    })?;
    inner.offers.remove(&token)?.metadata
}

impl ImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(super) fn release_probe(&self) -> impl Fn() -> bool + use<> {
        let store = Arc::downgrade(&self.0);
        move || store.upgrade().is_none()
    }

    #[cfg(test)]
    pub(super) fn recovery_diagnostics(&self) -> String {
        let inner = self.0.state.lock().unwrap();
        let resources: Vec<_> = inner
            .resources
            .iter()
            .map(|(key, resource)| {
                (
                    *key,
                    resource.width,
                    resource.height,
                    resource.evicted,
                    resource.pending.is_some(),
                    resource.decoded.as_ref().map(|decoded| {
                        let bytes = decoded.residency.data.as_bytes(0).unwrap();
                        (bytes.len(), bytes[..bytes.len().min(32)].to_vec())
                    }),
                )
            })
            .collect();
        let placements: Vec<_> = inner
            .placements
            .values()
            .map(|anchored| &anchored.placement)
            .collect();
        format!(
            "screen={} origin={} generation={} closed={} resources (key,w,h,evicted,pending,pixels)={resources:?} placements={placements:?}",
            inner.screen, inner.history_origin, inner.reader_generation, inner.closed
        )
    }

    pub fn graphics_capacity(&self) -> (u64, u64) {
        (PANE_SOURCE_BYTES as u64, PANE_DECODED_CAPACITY)
    }

    /// Atomically admit the actual resource footprint before its protocol ACK.
    pub fn offer(
        &self,
        token: u64,
        key: u64,
        width: u32,
        height: u32,
        encoded_bytes: u64,
        metadata_bytes: u64,
    ) -> bool {
        self.offer_for_generation(
            None,
            token,
            key,
            width,
            height,
            encoded_bytes,
            metadata_bytes,
        )
    }

    #[cfg(test)]
    pub(super) fn admit_test_resource(&self, update: &GraphicsUpdate, packet_bytes: usize) -> bool {
        let (key, width, height, encoded_bytes) = match update {
            GraphicsUpdate::Resource { key, image } => {
                (*key, image.width, image.height, packet_bytes as u64)
            }
            GraphicsUpdate::Place(placement) => (placement.key, 0, 0, 0),
            GraphicsUpdate::Evicted { key, .. } => (*key, 0, 0, 0),
            _ => return true,
        };
        static TOKENS: AtomicU64 = AtomicU64::new(1);
        let token = TOKENS.fetch_add(1, Ordering::Relaxed);
        let (generation, existing) = {
            let inner = self.0.state.lock().unwrap();
            let existing = match update {
                GraphicsUpdate::Place(placement) => inner.placements.contains_key(&placement.key),
                GraphicsUpdate::Resource { key, .. } | GraphicsUpdate::Evicted { key, .. } => {
                    inner.resources.contains_key(key)
                }
                _ => false,
            };
            (inner.reader_generation, existing)
        };
        if existing && width == 0 {
            return true;
        }
        self.offer_for_generation(
            Some(generation),
            token,
            key,
            width,
            height,
            encoded_bytes,
            if existing { 0 } else { OWNER_METADATA_BYTES },
        )
    }

    fn offer_for_generation(
        &self,
        generation: Option<u64>,
        token: u64,
        key: u64,
        width: u32,
        height: u32,
        encoded_bytes: u64,
        metadata_bytes: u64,
    ) -> bool {
        let mut inner = self.0.state.lock().unwrap();
        if generation
            .is_some_and(|generation| inner.closed || inner.reader_generation != generation)
            || inner.offers.contains_key(&token)
            || inner
                .offers
                .values()
                .any(|offer| offer.key == key && (offer.width == 0) == (width == 0))
        {
            return false;
        }
        if metadata_bytes != 0 && metadata_bytes != OWNER_METADATA_BYTES {
            return false;
        }
        if metadata_bytes == 0
            && !inner.resources.contains_key(&key)
            && !inner.placements.contains_key(&key)
        {
            return false;
        }
        let Some(mut offered) =
            self.reserve_resource(key, width, height, encoded_bytes, metadata_bytes)
        else {
            drop(inner);
            let metadata_fits = metadata_bytes as usize
                <= self
                    .0
                    .pane_metadata
                    .limit
                    .saturating_sub(self.0.pane_metadata.used.load(Ordering::Acquire))
                && metadata_bytes as usize
                    <= self
                        .0
                        .aggregate_metadata
                        .limit
                        .saturating_sub(self.0.aggregate_metadata.used.load(Ordering::Acquire));
            if width != 0 && height != 0 && metadata_fits {
                self.request_pressure(width, height, encoded_bytes);
            }
            return false;
        };
        offered.generation = generation;
        inner.offers.insert(token, offered);
        true
    }

    fn reserve_resource(
        &self,
        key: u64,
        width: u32,
        height: u32,
        encoded_bytes: u64,
        metadata_bytes: u64,
    ) -> Option<OfferedResource> {
        use lyrel_core::core::graphics_state::{MAX_IMAGE_DECODED_BYTES, MAX_IMAGE_DIMENSION};
        let encoded_bytes = usize::try_from(encoded_bytes).ok()?;
        let metadata = if metadata_bytes == 0 {
            None
        } else {
            Some(Arc::new(Reservation::new(
                &self.0.pane_metadata,
                &self.0.aggregate_metadata,
                metadata_bytes as usize,
            )?))
        };
        if width == 0 && height == 0 && encoded_bytes == 0 {
            return Some(OfferedResource {
                key,
                width,
                height,
                encoded_bytes,
                peak: None,
                scratch: None,
                source: None,
                metadata,
                generation: None,
            });
        }
        let pixels = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if width == 0
            || height == 0
            || width > MAX_IMAGE_DIMENSION
            || height > MAX_IMAGE_DIMENSION
            || pixels > MAX_IMAGE_DECODED_BYTES
            || encoded_bytes > lyrel_core::daemon::protocol::MAX_FRAME
        {
            return None;
        }
        let source_bytes = encoded_bytes.checked_mul(2)?.checked_add(256 * 1024)?;
        let source = Reservation::new(&self.0.pane_source, &self.0.aggregate_source, source_bytes)?;
        // Preserve the original 4×RGBA decoder bound: one retained output per
        // resource plus 3×RGBA shared scratch, never simultaneous decodes.
        let scratch = self.0.decode_scratch.claim(pixels.checked_mul(3)?)?;
        let bytes = pixels
            .checked_add(encoded_bytes)?
            .checked_add(gpu_page_bytes(width, height)?)?;
        let peak = Reservation::new(&self.0.pane_budget, &self.0.aggregate_budget, bytes)?;
        Some(OfferedResource {
            key,
            width,
            height,
            encoded_bytes,
            peak: Some(peak),
            scratch: Some(scratch),
            source: Some(source),
            metadata,
            generation: None,
        })
    }

    pub fn cancel_offer(&self, token: u64) {
        self.0.state.lock().unwrap().offers.remove(&token);
    }

    pub fn cancel_unclaimed_offers(&self) {
        self.0
            .state
            .lock()
            .unwrap()
            .offers
            .retain(|_, offer| offer.generation.is_some());
    }

    pub fn set_cell_metrics(&self, width: u16, height: u16) {
        let mut inner = self.0.state.lock().unwrap();
        inner.cell_width = width.max(1);
        inner.cell_height = height.max(1);
    }

    pub fn register_pressure(
        &self,
        inspect: impl Fn(&ImageStore) -> Option<(u64, u64, bool)> + Send + Sync + 'static,
        send: impl Fn(u64) + Send + Sync + 'static,
    ) {
        let entry = Arc::new(PressureConsumer {
            store: Arc::downgrade(&self.0),
            inspect: Box::new(inspect),
            send: Box::new(send),
        });
        *self.0.pressure.lock().unwrap() = Some(entry.clone());
        let mut registry = PRESSURE_CONSUMERS.lock().unwrap();
        registry.retain(|entry| entry.strong_count() > 0);
        registry.push(Arc::downgrade(&entry));
    }

    fn request_pressure(&self, width: u32, height: u32, encoded_bytes: u64) {
        let encoded = usize::try_from(encoded_bytes).unwrap_or(usize::MAX);
        let pixels = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        let source = encoded.saturating_mul(2).saturating_add(256 * 1024);
        let peak = pixels
            .saturating_mul(4)
            .saturating_add(encoded)
            .saturating_add(gpu_page_bytes(width, height).unwrap_or(usize::MAX));
        if source > self.0.pane_source.limit
            || peak > self.0.pane_budget.limit
            || source > self.0.aggregate_source.limit
            || peak > self.0.aggregate_budget.limit
        {
            return;
        }
        let scratch = self
            .0
            .decode_scratch
            .state
            .lock()
            .unwrap()
            .reservation
            .bytes;
        let peak = pixels
            .saturating_add(encoded)
            .saturating_add(gpu_page_bytes(width, height).unwrap_or(usize::MAX))
            .saturating_add(pixels.saturating_mul(3).saturating_sub(scratch));
        let local_only = source
            > self
                .0
                .pane_source
                .limit
                .saturating_sub(self.0.pane_source.used.load(Ordering::Acquire))
            || peak
                > self
                    .0
                    .pane_budget
                    .limit
                    .saturating_sub(self.0.pane_budget.used.load(Ordering::Acquire));
        let entries: Vec<_> = {
            let mut registry = PRESSURE_CONSUMERS.lock().unwrap();
            registry.retain(|entry| entry.strong_count() > 0);
            registry.iter().filter_map(Weak::upgrade).collect()
        };
        let mut oldest = None;
        for entry in entries {
            let Some(store) = entry.store.upgrade().map(ImageStore) else {
                continue;
            };
            if local_only && !Arc::ptr_eq(&store.0, &self.0) {
                continue;
            }
            if !Arc::ptr_eq(&store.0.aggregate_budget, &self.0.aggregate_budget)
                && !Arc::ptr_eq(&store.0.aggregate_source, &self.0.aggregate_source)
            {
                continue;
            }
            let Some((key, received, unreferenced)) = (entry.inspect)(&store) else {
                continue;
            };
            let order = (!unreferenced, received);
            if oldest.as_ref().is_none_or(|(best, _, _, _)| order < *best) {
                oldest = Some((order, key, store, entry));
            }
        }
        if let Some(((_, received), key, store, entry)) = oldest {
            let requested = {
                let mut inner = store.0.state.lock().unwrap();
                if inner.closed {
                    return;
                }
                inner.resources.get_mut(&key).is_some_and(|resource| {
                    if resource.received != received || resource.pressure_requested {
                        return false;
                    }
                    resource.pressure_requested = true;
                    true
                })
            };
            if requested {
                (entry.send)(key);
            }
        }
    }

    pub(super) fn viewport(
        &self,
        local_origin: i64,
        offset: usize,
        rows: usize,
        cols: usize,
        displayed: bool,
    ) -> Option<crate::daemon::protocol::GraphicsViewport> {
        let inner = self.0.state.lock().unwrap();
        if !inner.viewport_known {
            return None;
        }
        Some(crate::daemon::protocol::GraphicsViewport {
            screen: inner.screen,
            epoch: inner.viewport_epoch,
            top: (offset != 0).then(|| {
                inner
                    .history_origin
                    .saturating_add(local_origin - inner.local_origin)
                    .saturating_sub(offset as i64)
            }),
            rows: if displayed {
                rows.min(u16::MAX as usize) as u16
            } else {
                0
            },
            cols: cols.min(u16::MAX as usize) as u16,
        })
    }

    /// The caller holds the live terminal grid while deriving rows, then enters
    /// this store. No registry/store lock is held while acquiring another grid.
    pub fn oldest_reclaimable(
        &self,
        live_rows: usize,
        live_cols: usize,
        local_origin: i64,
        display_offset: usize,
        displayed: bool,
        rows: &[(i32, Vec<PlaceholderSlice>)],
    ) -> Option<(u64, u64, bool)> {
        let inner = self.0.state.lock().unwrap();
        if inner.closed {
            return None;
        }
        let referenced: BTreeSet<_> = inner
            .placements
            .values()
            .map(|p| p.placement.resource)
            .collect();
        let mut protected = BTreeSet::new();
        for anchored in inner.placements.values() {
            let placement = &anchored.placement;
            let row =
                placement.anchor_row - inner.history_origin - (local_origin - inner.local_origin)
                    + display_offset as i64;
            if (placement.screen != inner.screen && placement.live)
                || (placement.screen == inner.screen
                    && displayed
                    && !placement.virtual_placement
                    && placement.anchor_col < live_cols as u32
                    && row + i64::from(placement.clip_top) < live_rows as i64
                    && row + i64::from(placement.clip_bottom) > 0)
            {
                protected.insert(placement.resource);
            }
        }
        for (_, slices) in rows {
            if !displayed {
                break;
            }
            for slice in slices {
                let Some(prototype) = placeholder_prototype(
                    inner.placements.values().map(|p| &p.placement),
                    inner.screen,
                    &slice.run,
                ) else {
                    continue;
                };
                let Some(resource) = inner.resources.get(&prototype.resource) else {
                    continue;
                };
                if slice.run.row >= prototype.span_rows || slice.run.col >= prototype.span_cols {
                    continue;
                }
                let mut run = slice.run.clone();
                run.width = run.width.min(prototype.span_cols - run.col);
                if rio_vt::ansi::kitty_virtual::compute_run_geometry(
                    &run,
                    prototype.span_cols,
                    prototype.span_rows,
                    resource.width,
                    resource.height,
                    (prototype.x, prototype.y, prototype.w, prototype.h),
                    f32::from(inner.cell_width.max(1)),
                    f32::from(inner.cell_height.max(1)),
                    0.,
                    0.,
                    0,
                    slice.column,
                )
                .is_some()
                {
                    protected.insert(prototype.resource);
                }
            }
        }
        inner
            .resources
            .iter()
            .filter_map(|(&key, resource)| {
                if protected.contains(&key) || resource.evicted || resource.pressure_requested {
                    return None;
                }
                Some((key, resource.received, !referenced.contains(&key)))
            })
            .min_by_key(|(_, received, unreferenced)| (!*unreferenced, *received))
    }

    pub fn snapshot(&self) -> Vec<RenderPlacement> {
        self.snapshot_with_placeholders(&[])
    }

    /// Runs are derived from the same locked grid snapshot as terminal text.
    /// A prototype itself never appears in the display list.
    pub fn snapshot_with_placeholders(
        &self,
        rows: &[(i32, Vec<PlaceholderSlice>)],
    ) -> Vec<RenderPlacement> {
        let inner = self.0.state.lock().unwrap();
        let render = |anchored: &AnchoredPlacement, placeholder| {
            let resource = inner.resources.get(&anchored.placement.resource)?;
            if resource.decoded.is_none() && !resource.evicted {
                return None;
            }
            Some(RenderPlacement {
                data: resource
                    .decoded
                    .as_ref()
                    .map(|decoded| decoded.residency.data.clone()),
                evicted: resource.evicted,
                painted: resource.painted.clone(),
                placement: anchored.placement.clone(),
                anchor_row: anchored.placement.anchor_row,
                anchor_col: anchored.placement.anchor_col as usize,
                width_px: resource.width,
                height_px: resource.height,
                revision: anchored.revision,
                history_origin: inner.history_origin,
                resource_revision: resource.revision,
                placeholder,
                _residency: resource
                    .decoded
                    .as_ref()
                    .map(|decoded| decoded.residency.clone()),
                _metadata: (resource.metadata.clone(), anchored.metadata.clone()),
            })
        };
        let mut images: Vec<_> = inner
            .placements
            .values()
            .filter(|anchored| {
                anchored.placement.screen == inner.screen && !anchored.placement.virtual_placement
            })
            .filter_map(|anchored| render(anchored, None))
            .collect();
        for (row, slices) in rows {
            for slice in slices {
                let Some(prototype) = placeholder_prototype(
                    inner.placements.values().map(|p| &p.placement),
                    inner.screen,
                    &slice.run,
                ) else {
                    continue;
                };
                if slice.run.row >= prototype.span_rows || slice.run.col >= prototype.span_cols {
                    continue;
                }
                let mut slice = slice.clone();
                slice.run.width = slice.run.width.min(prototype.span_cols - slice.run.col);
                if let Some(image) = render(&inner.placements[&prototype.key], Some((*row, slice)))
                {
                    images.push(image);
                }
            }
        }
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
    /// Resource/placement replay precedes the authoritative Screen. Decoding
    /// may finish in that interval, but the default origin is not paintable.
    pub fn claim_for_paint(&self, image: &RenderPlacement) -> bool {
        let inner = self.0.state.lock().unwrap();
        let current = inner.viewport_known
            && inner
                .placements
                .get(&image.placement.key)
                .is_some_and(|placement| placement.revision == image.revision)
            && inner.screen == image.placement.screen
            && inner.history_origin == image.history_origin
            && inner
                .resources
                .get(&image.placement.resource)
                .is_some_and(|resource| {
                    if image.evicted {
                        resource.evicted && resource.revision == image.resource_revision
                    } else {
                        resource
                            .decoded
                            .as_ref()
                            .zip(image.data.as_ref())
                            .is_some_and(|(decoded, data)| {
                                Arc::ptr_eq(&decoded.residency.data, data)
                            })
                    }
                });
        if current {
            image.painted.store(true, Ordering::Release);
        }
        current
    }

    fn take_retired(&self) -> Vec<Arc<Residency>> {
        std::mem::take(&mut self.0.state.lock().unwrap().retired)
    }

    #[cfg(test)]
    fn take_for_release(&self) -> Vec<Arc<Residency>> {
        // Test consumers exercise the same retirement list; production drains it
        // through release_all so GPU release precedes reservation release.
        self.clear();
        self.take_retired()
    }

    /// The window owns atlas release, including for panes which never repaint.
    /// Keep reservations until GPUI has removed each retired atlas image.
    pub fn release_retired(&self, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
        let retired = self.take_retired();
        if retired.is_empty() {
            return;
        }
        // Compaction must remove every terminal tile before returning an old
        // page's ticket. Hidden tiles still release immediately, but only a
        // displayed owner needs a new frame. Surviving CPU images register
        // again when painted, including after their hidden pane returns.
        let mut invalidated_visible = false;
        WINDOW_IMAGES.with(|windows| {
            let images = windows
                .borrow_mut()
                .remove(&window.window_handle().window_id());
            for entry in images.into_iter().flatten() {
                let Some(image) = entry.residency.upgrade() else {
                    continue;
                };
                image.registered.store(false, Ordering::Release);
                if image.gpu.lock().unwrap().is_none() {
                    continue;
                }
                invalidated_visible |= entry
                    .displayed
                    .upgrade()
                    .is_some_and(|displayed| displayed.load(Ordering::Relaxed));
                cx.drop_image(image.data.clone(), Some(window));
            }
        });
        for retired in retired {
            cx.drop_image(retired.data.clone(), Some(window));
            retired.gpu.lock().unwrap().take();
        }
        if invalidated_visible {
            window.refresh();
        }
        self.0.ready.notify_all();
    }

    pub fn register_for_paint(
        &self,
        image: &RenderPlacement,
        window: &gpui_kit::Window,
        displayed: &Arc<AtomicBool>,
    ) {
        let Some(residency) = &image._residency else {
            return;
        };
        if residency.registered.swap(true, Ordering::AcqRel) {
            return;
        }
        WINDOW_IMAGES.with(|windows| {
            let mut windows = windows.borrow_mut();
            windows.retain(|_, images| {
                images.retain(|image| image.residency.strong_count() > 0);
                !images.is_empty()
            });
            windows
                .entry(window.window_handle().window_id())
                .or_default()
                .push(WindowImage {
                    residency: Arc::downgrade(residency),
                    displayed: Arc::downgrade(displayed),
                });
        });
    }

    pub fn release_all(&self, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
        self.clear();
        self.release_retired(window, cx);
    }

    /// Reset a detached/relinked pane's nonpersistent derived state. Clearing
    /// the resource map also invalidates any in-flight decode completion.
    pub fn clear(&self) {
        self.clear_resources(false);
    }

    pub fn clear_for_relink(&self) {
        self.clear_resources(true);
    }

    fn clear_resources(&self, preserve_unclaimed: bool) {
        let mut inner = self.0.state.lock().unwrap();
        inner.placements.clear();
        inner
            .offers
            .retain(|_, offer| preserve_unclaimed && offer.generation.is_none());
        let resources = std::mem::take(&mut inner.resources);
        for resource in resources.into_values() {
            retire(resource, &mut inner.retired);
        }
    }

    /// Clear the retained active screen after its reader has been retired.
    /// Test visibility against the old daemon origin: Rio's ED2 can move the
    /// cleared text into history before the new origin is supplied here.
    pub(super) fn clear_presentation(&self, history: bool, rows: usize, history_origin: i64) {
        let mut inner = self.0.state.lock().unwrap();
        debug_assert!(inner.closed);
        let screen = inner.screen;
        let base = inner.history_origin;
        let end = base.saturating_add(rows as i64);
        inner.placements.retain(|_, anchored| {
            let p = &anchored.placement;
            p.screen != screen
                || (!history
                    && (p.virtual_placement
                        || p.anchor_row + i64::from(p.clip_bottom) <= base
                        || p.anchor_row + i64::from(p.clip_top) >= end))
        });
        let retained: BTreeSet<_> = inner
            .placements
            .values()
            .map(|p| p.placement.resource)
            .collect();
        let resources = std::mem::take(&mut inner.resources);
        for (key, resource) in resources {
            if retained.contains(&key) {
                inner.resources.insert(key, resource);
            } else {
                retire(resource, &mut inner.retired);
            }
        }
        inner.history_origin = history_origin;
    }

    /// Invalidate the old reader synchronously before resetting/relinking the
    /// grid. A late frame, completion, or worker Drop then cannot affect the
    /// successor, and no UI-thread join is necessary.
    pub fn retire_reader(&self) {
        let mut inner = self.0.state.lock().unwrap();
        inner.reader_generation = inner
            .reader_generation
            .checked_add(1)
            .expect("graphics reader generation exhausted");
        inner.closed = true;
        inner.offers.retain(|_, offer| offer.generation.is_none());
        for resource in inner.resources.values_mut() {
            resource.pending.take();
            resource.peak.take();
            resource.scratch.take();
            resource.source.take();
        }
        drop(inner);
        self.0.ready.notify_all();
    }

    fn apply(&self, generation: u64, update: GraphicsUpdate) -> bool {
        let mut inner = self.0.state.lock().unwrap();
        if inner.closed || inner.reader_generation != generation {
            return false;
        }
        match update {
            GraphicsUpdate::Resource { key, image } => {
                let token = inner.offers.iter().find_map(|(&token, offer)| {
                    (offer.key == key && offer.width > 0 && offer.generation == Some(generation))
                        .then_some(token)
                });
                let offered = token.and_then(|token| inner.offers.remove(&token));
                let Some(mut offered) = offered else {
                    return false;
                };
                if offered.width != image.width
                    || offered.height != image.height
                    || offered.encoded_bytes < image.data.len()
                {
                    return false;
                }
                let Some(mut source) = offered
                    .source
                    .take()
                    .filter(|source| source.bytes >= image.data.capacity())
                else {
                    return false;
                };
                let Some(peak) = offered.peak.take() else {
                    return false;
                };
                let metadata = inner
                    .resources
                    .get(&key)
                    .map(|resource| resource.metadata.clone())
                    .or(offered.metadata.take());
                let Some(metadata) = metadata else {
                    return false;
                };
                source.shrink(image.data.capacity());
                inner.next_revision = inner.next_revision.wrapping_add(1);
                let revision = inner.next_revision;
                let resource = Resource {
                    width: image.width,
                    height: image.height,
                    pending: Some(image),
                    decoded: None,
                    revision,
                    evicted: false,
                    painted: Arc::new(AtomicBool::new(false)),
                    peak: Some(peak),
                    scratch: offered.scratch.take(),
                    source: Some(source),
                    metadata,
                    received: RECEIVE_SEQUENCE
                        .try_update(Ordering::AcqRel, Ordering::Acquire, |sequence| {
                            sequence.checked_add(1)
                        })
                        .expect("graphics receive sequence exhausted"),
                    pressure_requested: false,
                };
                if let Some(old) = inner.resources.insert(key, resource) {
                    retire(old, &mut inner.retired);
                }
                drop(inner);
                self.0.ready.notify_one();
                false
            }
            GraphicsUpdate::Evicted { key, width, height } => {
                let offered_metadata = take_metadata_offer(&mut inner, generation, key);
                let metadata = inner
                    .resources
                    .get(&key)
                    .map(|resource| resource.metadata.clone())
                    .or(offered_metadata);
                let Some(metadata) = metadata else {
                    return false;
                };
                inner.next_revision = inner.next_revision.wrapping_add(1);
                let revision = inner.next_revision;
                let resource = Resource {
                    pending: None,
                    decoded: None,
                    width,
                    height,
                    evicted: true,
                    revision,
                    painted: Arc::new(AtomicBool::new(false)),
                    peak: None,
                    scratch: None,
                    source: None,
                    received: 0,
                    pressure_requested: false,
                    metadata,
                };
                if let Some(old) = inner.resources.insert(key, resource) {
                    retire(old, &mut inner.retired);
                }
                true
            }
            GraphicsUpdate::Place(placement) => {
                let offered_metadata = take_metadata_offer(&mut inner, generation, placement.key);
                let metadata = inner
                    .placements
                    .get(&placement.key)
                    .map(|placement| placement.metadata.clone())
                    .or(offered_metadata);
                let Some(metadata) = metadata else {
                    return false;
                };
                inner.next_revision = inner.next_revision.wrapping_add(1);
                let revision = inner.next_revision;
                inner.placements.insert(
                    placement.key,
                    AnchoredPlacement {
                        placement,
                        revision,
                        metadata,
                    },
                );
                true
            }
            GraphicsUpdate::Screen {
                screen,
                history_origin,
                epoch,
            } => {
                let changed = inner.screen != screen
                    || inner.history_origin != history_origin
                    || inner.viewport_epoch != epoch
                    || !inner.viewport_known;
                inner.screen = screen;
                inner.history_origin = history_origin;
                inner.viewport_epoch = epoch;
                inner.viewport_known = true;
                changed
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
        decoder: impl Fn(&mut Image) -> Option<(Arc<RenderImage>, u32, u32, usize)> + Send + 'static,
    ) -> Self {
        let generation = {
            let mut inner = store.0.state.lock().unwrap();
            inner.reader_generation = inner
                .reader_generation
                .checked_add(1)
                .expect("graphics reader generation exhausted");
            inner.closed = false;
            // A retired view keeps its known coordinates for local history
            // paint; a new reader must wait for its own authoritative Screen.
            inner.viewport_known = false;
            let generation = inner.reader_generation;
            for offer in inner.offers.values_mut() {
                if offer.generation.is_none() {
                    offer.generation = Some(generation);
                }
            }
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

    pub(super) fn sync_local_origin(&self, local_origin: i64) {
        let mut inner = self.store.0.state.lock().unwrap();
        if inner.reader_generation == self.generation && !inner.closed {
            inner.local_origin = local_origin;
        }
    }

    pub fn apply(&self, update: GraphicsUpdate) -> bool {
        self.store.apply(self.generation, update)
    }

    pub fn has_resource_grant(&self, update: &GraphicsUpdate) -> bool {
        let inner = self.store.0.state.lock().unwrap();
        if inner.closed || inner.reader_generation != self.generation {
            return false;
        }
        let (key, pixel_offer) = match update {
            GraphicsUpdate::Resource { key, .. } => (*key, true),
            GraphicsUpdate::Place(placement) if !inner.placements.contains_key(&placement.key) => {
                (placement.key, false)
            }
            GraphicsUpdate::Evicted { key, .. } if !inner.resources.contains_key(key) => {
                (*key, false)
            }
            _ => return true,
        };
        inner.offers.values().any(|offer| {
            offer.key == key
                && (offer.width > 0) == pixel_offer
                && offer.generation == Some(self.generation)
        })
    }

    pub fn store_metrics(&self, width: u16, height: u16) {
        let mut inner = self.store.0.state.lock().unwrap();
        if inner.reader_generation == self.generation && !inner.closed {
            inner.cell_width = width.max(1);
            inner.cell_height = height.max(1);
        }
    }

    pub fn offer(
        &self,
        token: u64,
        key: u64,
        width: u32,
        height: u32,
        encoded_bytes: u64,
        metadata_bytes: u64,
    ) -> bool {
        self.store.offer_for_generation(
            Some(self.generation),
            token,
            key,
            width,
            height,
            encoded_bytes,
            metadata_bytes,
        )
    }

    pub fn cancel_offer(&self, token: u64) {
        let mut inner = self.store.0.state.lock().unwrap();
        if inner.reader_generation == self.generation {
            inner.offers.remove(&token);
        }
    }

    pub(super) fn complete_pressure(&self, key: u64) {
        let mut inner = self.store.0.state.lock().unwrap();
        if !inner.closed
            && inner.reader_generation == self.generation
            && let Some(resource) = inner.resources.get_mut(&key)
        {
            // Success already arrived as an ordered Evicted mutation. Refusal
            // is also complete: later admission demand may try again after
            // any authority protection change, including another viewport.
            // Neither completion schedules work or retries by itself.
            resource.pressure_requested = false;
        }
    }

    fn run(
        store: ImageStore,
        generation: u64,
        wake: impl Fn(),
        decoder: impl Fn(&mut Image) -> Option<(Arc<RenderImage>, u32, u32, usize)>,
    ) {
        loop {
            let (key, revision, image, mut reservation, scratch, source) = {
                let mut inner = store.0.state.lock().unwrap();
                loop {
                    if inner.closed || inner.reader_generation != generation {
                        return;
                    }
                    let next = inner.resources.iter_mut().find_map(|(&key, resource)| {
                        if resource.pending.is_none() {
                            return None;
                        }
                        Some((
                            key,
                            resource.revision,
                            resource.pending.take().unwrap(),
                            resource.peak.take().unwrap(),
                            resource.scratch.take().unwrap(),
                            resource.source.take(),
                        ))
                    });
                    if let Some(next) = next {
                        break next;
                    }
                    inner = store.0.ready.wait(inner).unwrap();
                }
            };
            // This lock is independent of the store lock. In particular an old
            // generation may finish decoding while its successor queues work,
            // but cannot overlap the successor's decoder peak.
            let _decoder = store.0.decode_scratch.decoder.lock().unwrap();
            let mut image = Arc::unwrap_or_clone(image);
            let decoded = decoder(&mut image);
            drop(image);
            drop(source);
            let visible_change = {
                let mut inner = store.0.state.lock().unwrap();
                if inner.closed || inner.reader_generation != generation {
                    return;
                }
                let current = inner
                    .resources
                    .get_mut(&key)
                    .filter(|resource| resource.revision == revision);
                if let (Some(resource), Some((data, width, height, allocated_bytes))) =
                    (current, decoded)
                {
                    let gpu = reservation.split(gpu_page_bytes(width, height).unwrap());
                    reservation.shrink(allocated_bytes);
                    resource.decoded = Some(DecodedResource {
                        residency: Arc::new(Residency {
                            data,
                            _cpu: reservation,
                            gpu: Mutex::new(Some(gpu)),
                            registered: AtomicBool::new(false),
                        }),
                    });
                    inner
                        .placements
                        .values()
                        .any(|placement| placement.placement.resource == key)
                } else {
                    false
                }
            };
            drop(scratch);
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
                inner.offers.retain(|_, offer| offer.generation.is_none());
                for resource in inner.resources.values_mut() {
                    resource.pending.take();
                    resource.peak.take();
                    resource.scratch.take();
                    resource.source.take();
                }
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
fn decode(img: &mut Image) -> Option<(Arc<RenderImage>, u32, u32, usize)> {
    use lyrel_core::core::graphics_state::MAX_IMAGE_DIMENSION;
    use lyrel_core::core::kitty_graphics::MAX_IMAGE_BYTES;

    let bytes = (img.width as usize)
        .checked_mul(img.height as usize)?
        .checked_mul(4)?;
    if bytes == 0
        || bytes > MAX_IMAGE_BYTES
        || img.data.len() > MAX_IMAGE_BYTES
        || img.width > MAX_IMAGE_DIMENSION
        || img.height > MAX_IMAGE_DIMENSION
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
            limits.max_alloc = Some((bytes as u64).saturating_mul(2));
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
    // Inflate may overallocate beyond the admitted RGBA length. Trim that
    // spare capacity while decoder scratch is still reserved, before keeping
    // the pixels under the resource's retained CPU/GPU reservation.
    let rgba = rgba.into_boxed_slice().into_vec();
    let allocated_bytes = rgba.capacity();
    let render = RenderImage::new(SmallVec::from_elem(
        Frame::new(RgbaImage::from_raw(w, h, rgba)?),
        1,
    ));
    Some((Arc::new(render), w, h, allocated_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, atomic::AtomicUsize};

    fn apply_placement(worker: &DecodeWorker, update: GraphicsUpdate) {
        if let GraphicsUpdate::Place(placement) = &update {
            let metadata = {
                let inner = worker.store.0.state.lock().unwrap();
                !inner.placements.contains_key(&placement.key)
            };
            if metadata {
                worker.offer(placement.key, placement.key, 0, 0, 0, OWNER_METADATA_BYTES);
            }
        }
        worker.apply(update);
    }

    fn apply_resource(worker: &DecodeWorker, key: u64, red: u8) {
        let update = resource(key, red);
        worker.offer(
            key,
            key,
            1,
            1,
            update.encode_frame().len() as u64,
            OWNER_METADATA_BYTES,
        );
        worker.apply(update);
    }

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

    fn placement(key: u64, resource: u64, row: i64, col: u32) -> GraphicsUpdate {
        let mut placement = Placement::default();
        placement.key = key;
        placement.resource = resource;
        placement.w = 1;
        placement.h = 1;
        placement.anchor_row = row;
        placement.anchor_col = col;
        placement.span_rows = 1;
        placement.clip_bottom = 1;
        GraphicsUpdate::Place(placement)
    }

    fn establish_screen(worker: &DecodeWorker) {
        worker.apply(GraphicsUpdate::Screen {
            screen: 0,
            history_origin: 0,
            epoch: 0,
        });
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
    fn unknown_replay_origin_cannot_claim_a_decoded_placement_for_paint() {
        let store = ImageStore::new();
        let worker = DecodeWorker::spawn(store.clone(), || {});
        apply_resource(&worker, 1, 50);
        apply_placement(&worker, placement(10, 1, 10_005, 3));
        wait_for(&store, |images| images.len() == 1);
        let images = store.snapshot();
        assert!(
            !store.claim_for_paint(&images[0]),
            "a replayed resource cannot use the default origin before its authoritative Screen"
        );
        worker.apply(GraphicsUpdate::Screen {
            screen: 0,
            history_origin: 10_003,
            epoch: 0,
        });
        let current = store.snapshot();
        assert!(store.claim_for_paint(&current[0]));
        let (bounds, _) =
            current[0].paint_bounds(point(px(0.), px(0.)), px(10.), px(20.), 0, 0, 1.);
        assert_eq!(bounds.origin, point(px(30.), px(40.)));
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
        apply_resource(&worker, 1, 1);
        apply_placement(&worker, placement(10, 1, 3, 4));
        entered.wait();
        worker.apply(GraphicsUpdate::Remove {
            placements: vec![10],
            resources: vec![1],
        });
        apply_resource(&worker, 2, 90);
        apply_placement(&worker, placement(10, 2, 40, 8));
        release.wait();
        wait_for(&store, |images| images.len() == 1);
        let images = store.snapshot();
        assert_eq!((images[0].anchor_row, images[0].anchor_col), (40, 8));
        assert_eq!(
            images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
            &[30, 20, 90, 128]
        );
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
        apply_resource(&worker, 1, 1);
        apply_placement(&worker, placement(1, 1, 1, 0));
        entered.wait();
        for key in 2..=9 {
            apply_resource(&worker, key, key as u8);
            apply_placement(&worker, placement(key, key, key as i64, 0));
        }
        apply_placement(&worker, placement(1, 1, 77, 9));
        worker.apply(GraphicsUpdate::Remove {
            placements: vec![2],
            resources: vec![],
        });
        release.wait();
        wait_for(&store, |images| images.len() == 8);
        let images = store.snapshot();
        for image in &images {
            assert_eq!(
                image.data.as_ref().unwrap().as_bytes(0).unwrap(),
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
        apply_placement(&worker, placement(20, 2, 88, 0));
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
                .as_ref()
                .unwrap()
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
        apply_resource(&old, 1, 10);
        apply_placement(&old, placement(1, 1, 1, 2));
        entered.wait();
        // Relink retires the reader without joining it on the UI thread. Its
        // active decoder still owns the shared peak until real decode exits.
        store.retire_reader();
        store.clear();
        assert!(store.0.decode_scratch.decoder.try_lock().is_err());
        assert_eq!(
            store
                .0
                .decode_scratch
                .state
                .lock()
                .unwrap()
                .reservation
                .bytes,
            12
        );
        let screen_wakes = Arc::new(AtomicUsize::new(0));
        let new = DecodeWorker::spawn(store.clone(), {
            let wakes = screen_wakes.clone();
            move || {
                wakes.fetch_add(1, Ordering::SeqCst);
            }
        });
        apply_resource(&new, 2, 90);
        apply_placement(&new, placement(2, 2, 4, 5));
        assert!(
            store.snapshot().is_empty(),
            "successor cannot overlap the retired decoder"
        );
        assert!(store.0.decode_scratch.decoder.try_lock().is_err());
        release.wait();
        wait_for(&store, |images| images.len() == 1);
        let replaying = store.snapshot();
        assert!(
            !store.claim_for_paint(&replaying[0]),
            "relink invalidates the old reader's coordinates until its fresh Screen"
        );
        establish_screen(&new);
        assert_eq!(
            screen_wakes.load(Ordering::SeqCst),
            1,
            "the first zero-valued Screen wakes painting after relink"
        );
        let current = store.snapshot();
        assert_eq!(
            current[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
            &[30, 20, 90, 128]
        );
        assert!(store.claim_for_paint(&current[0]));
        assert!(
            !old.apply(GraphicsUpdate::Remove {
                placements: vec![2],
                resources: vec![2],
            }),
            "a retired reader cannot delete new link images"
        );
        apply_resource(&old, 3, 11);
        apply_placement(&old, placement(3, 3, 9, 9));
        drop(old);
        // Old Drop occurs after the successor is usable. It must neither land
        // its stale pixel nor close the new worker's wake/admission gate.
        apply_resource(&new, 4, 100);
        apply_placement(&new, placement(4, 4, 6, 7));
        wait_for(&store, |images| images.len() == 2);
        assert!(
            store
                .snapshot()
                .iter()
                .all(|image| image.placement.resource != 1 && image.placement.resource != 3)
        );
        new.apply(GraphicsUpdate::Remove {
            placements: vec![2, 4],
            resources: vec![2, 4],
        });
        assert!(
            store.snapshot().is_empty(),
            "successor deletion remains live"
        );
    }

    #[test]
    fn compressed_raw_image_survives_decode_and_releases_its_budget() {
        use base64::Engine as _;
        let store = ImageStore::new();
        let worker = DecodeWorker::spawn(store.clone(), || {});
        establish_screen(&worker);
        let update = GraphicsUpdate::Resource {
            key: 1,
            image: Arc::new(Image {
                width: 320,
                height: 160,
                format: WireFormat::Rgba,
                compressed: true,
                // Node zlib.deflateSync of [10, 20, 30, 128] repeated 320×160.
                data: base64::engine::general_purpose::STANDARD
                    .decode(format!(
                        "eJztw7ENAAAIA6C9s7O39nT/MJCQ2UZ{}dU3D4ff6I8=",
                        "V".repeat(262)
                    ))
                    .unwrap(),
            }),
        };
        assert!(worker.offer(
            1,
            1,
            320,
            160,
            update.encode_frame().len() as u64,
            OWNER_METADATA_BYTES,
        ));
        worker.apply(update);
        apply_placement(&worker, placement(10, 1, 0, 0));
        wait_for(&store, |images| images.len() == 1);
        let images = store.snapshot();
        assert_eq!(
            images[0].data.as_ref().unwrap().as_bytes(0).unwrap(),
            [30, 20, 10, 128].repeat(320 * 160),
        );
        drop(images);
        drop(worker);
        store.take_for_release();
        assert_eq!(store.0.pane_budget.used.load(Ordering::Acquire), 0);
        assert_eq!(store.0.pane_source.used.load(Ordering::Acquire), 0);
    }

    #[test]
    fn failed_real_decode_releases_peak_source_and_cancelled_offer_tickets() {
        let store = ImageStore::new();
        let (completed, completion) = std::sync::mpsc::channel();
        let worker = DecodeWorker::spawn_with_decoder(
            store.clone(),
            || {},
            move |image| {
                let decoded = decode(image);
                assert!(
                    decoded.is_none(),
                    "the real PNG decoder must reject invalid bytes"
                );
                completed.send(()).unwrap();
                decoded
            },
        );
        let failed = GraphicsUpdate::Resource {
            key: 1,
            image: Arc::new(Image {
                width: 1,
                height: 1,
                format: WireFormat::Png,
                compressed: false,
                data: vec![0; 8],
            }),
        };
        assert!(worker.offer(
            1,
            1,
            1,
            1,
            failed.encode_frame().len() as u64,
            OWNER_METADATA_BYTES
        ));
        worker.apply(failed);
        completion
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        drop(worker); // join proves every real decoder buffer has died
        assert_eq!(
            store
                .0
                .decode_scratch
                .state
                .lock()
                .unwrap()
                .reservation
                .bytes,
            0
        );
        assert_eq!(store.0.pane_budget.used.load(Ordering::Acquire), 0);
        assert_eq!(store.0.pane_source.used.load(Ordering::Acquire), 0);
        store.clear();
        assert_eq!(store.0.pane_metadata.used.load(Ordering::Acquire), 0);

        assert!(store.offer(2, 2, 3840, 2160, 1000, OWNER_METADATA_BYTES));
        assert!(store.offer(3, 3, 1920, 1080, 1000, OWNER_METADATA_BYTES));
        store.cancel_offer(2);
        assert_eq!(
            store
                .0
                .decode_scratch
                .state
                .lock()
                .unwrap()
                .reservation
                .bytes,
            24_883_200
        );
        store.cancel_unclaimed_offers();
        assert_eq!(
            store
                .0
                .decode_scratch
                .state
                .lock()
                .unwrap()
                .reservation
                .bytes,
            0
        );
        assert_eq!(store.0.pane_budget.used.load(Ordering::Acquire), 0);
        assert_eq!(store.0.pane_source.used.load(Ordering::Acquire), 0);
        assert_eq!(store.0.pane_metadata.used.load(Ordering::Acquire), 0);
    }

    #[test]
    fn shared_resource_has_one_atlas_retirement_and_stale_placement_cannot_paint() {
        let store = ImageStore::new();
        let worker = DecodeWorker::spawn(store.clone(), || {});
        establish_screen(&worker);
        apply_resource(&worker, 1, 50);
        apply_placement(&worker, placement(10, 1, 1, 0));
        apply_placement(&worker, placement(11, 1, 2, 0));
        wait_for(&store, |images| images.len() == 2);
        let images = store.snapshot();
        assert!(Arc::ptr_eq(
            images[0].data.as_ref().unwrap(),
            images[1].data.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(&images[0].painted, &images[1].painted));
        assert!(store.claim_for_paint(&images[0]));
        apply_placement(&worker, placement(10, 1, 9, 0));
        assert!(!store.claim_for_paint(&images[0]));
        worker.apply(GraphicsUpdate::Remove {
            placements: vec![10],
            resources: vec![],
        });
        assert!(
            store.take_retired().is_empty(),
            "other placement still uses the atlas tile"
        );
        assert!(store.claim_for_paint(&images[1]));
        worker.apply(GraphicsUpdate::Remove {
            placements: vec![11],
            resources: vec![1],
        });
        assert!(!store.claim_for_paint(&images[1]));
        let retired = store.take_retired();
        assert_eq!(retired.len(), 1);
        assert!(Arc::ptr_eq(
            &retired[0].data,
            images[0].data.as_ref().unwrap()
        ));
        assert!(store.take_for_release().is_empty());
        apply_resource(&worker, 2, 100);
        apply_placement(&worker, placement(20, 2, 0, 0));
        apply_placement(&worker, placement(21, 2, 0, 0));
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
        assert!(Arc::ptr_eq(
            &released[0].data,
            current[0].data.as_ref().unwrap()
        ));
        assert!(store.snapshot().is_empty());
        assert!(store.take_for_release().is_empty());
    }
}
