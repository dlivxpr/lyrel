use gpui_kit::RenderImage;
use markdown::mdast::Node;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Image {
    pub url: String,
    pub alt: String,
    pub title: Option<String>,
}
pub(super) type Definitions = HashMap<String, (String, Option<String>)>;
pub(super) fn definitions(root: &Node) -> Definitions {
    let mut values = HashMap::new();
    fn walk(node: &Node, values: &mut Definitions) {
        if let Node::Definition(value) = node {
            values
                .entry(value.identifier.to_uppercase())
                .or_insert_with(|| (value.url.clone(), value.title.clone()));
        }
        for child in node.children().into_iter().flatten() {
            walk(child, values);
        }
    }
    walk(root, &mut values);
    values
}
pub(super) fn image(node: &Node, definitions: &Definitions) -> Option<Image> {
    match node {
        Node::Image(value) => Some(Image {
            url: value.url.clone(),
            alt: value.alt.clone(),
            title: value.title.clone(),
        }),
        Node::ImageReference(value) => {
            definitions
                .get(&value.identifier.to_uppercase())
                .map(|(url, title)| Image {
                    url: url.clone(),
                    alt: value.alt.clone(),
                    title: title.clone(),
                })
        }
        _ => None,
    }
}
#[derive(Clone)]
pub(super) enum State {
    Unloaded,
    Loading(u64),
    Ready(Arc<RenderImage>),
    Failed(String),
}
#[derive(Default)]
pub(super) struct Images {
    pub states: HashMap<String, State>,
    tasks: HashMap<String, gpui_kit::Task<()>>,
    generation: u64,
}
const MAX_BYTES: u64 = 32 * 1024 * 1024;
pub(super) fn external(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}
fn decode(bytes: &[u8]) -> anyhow::Result<Arc<RenderImage>> {
    // Decode off the UI thread; the image hook returns already prepared pixels.
    let pixels = match image::load_from_memory(bytes) {
        Ok(image) => image.into_rgba8(),
        Err(raster_error) => {
            use resvg::{tiny_skia, usvg};
            static FONTS: std::sync::LazyLock<Arc<usvg::fontdb::Database>> =
                std::sync::LazyLock::new(|| {
                    let mut fonts = usvg::fontdb::Database::new();
                    fonts.load_system_fonts();
                    Arc::new(fonts)
                });
            let denied = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let rejected = denied.clone();
            let options = usvg::Options {
                fontdb: FONTS.clone(),
                image_href_resolver: usvg::ImageHrefResolver {
                    resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
                    resolve_string: Box::new(move |_, _| {
                        rejected.store(true, std::sync::atomic::Ordering::Relaxed);
                        None
                    }),
                },
                ..Default::default()
            };
            let tree = usvg::Tree::from_data(bytes, &options)
                .map_err(|_| anyhow::anyhow!("{raster_error}"))?;
            anyhow::ensure!(
                !denied.load(std::sync::atomic::Ordering::Relaxed),
                "SVG references external resources; those resources were not loaded"
            );
            let size = tree.size();
            let scale = (2048. / size.width().max(size.height())).min(1.);
            let width = (size.width() * scale).ceil().max(1.) as u32;
            let height = (size.height() * scale).ceil().max(1.) as u32;
            let mut pixmap = tiny_skia::Pixmap::new(width, height)
                .ok_or_else(|| anyhow::anyhow!("image allocation failed"))?;
            resvg::render(
                &tree,
                tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );
            let mut bytes = pixmap.take();
            for pixel in bytes.chunks_exact_mut(4) {
                let alpha = pixel[3] as u32;
                if alpha != 0 {
                    for channel in &mut pixel[..3] {
                        *channel = ((*channel as u32 * 255) / alpha).min(255) as u8;
                    }
                }
            }
            image::RgbaImage::from_raw(width, height, bytes).unwrap()
        }
    };
    let mut pixels = pixels;
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(pixels)
    ])))
}
impl super::MarkdownDocument {
    pub(super) fn sync_images(
        &mut self,
        urls: Vec<String>,
        consent: bool,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let previous_len = self.images.states.len();
        self.images.states.retain(|url, _| urls.contains(url));
        self.images.tasks.retain(|url, _| urls.contains(url));
        let mut changed = self.images.states.len() != previous_len;
        let Some(resources) = self.resources().cloned() else {
            if changed { cx.emit(super::DocumentEvent::ImagesChanged); }
            return;
        };
        let revision = self.revision();
        for url in urls {
            let state = self
                .images
                .states
                .entry(url.clone())
                .or_insert_with(|| {
                    changed = true;
                    State::Unloaded
                });
            if matches!(state, State::Ready(_) | State::Loading(_)) || (external(&url) && !consent)
            {
                continue;
            }
            if matches!(state, State::Failed(_)) && !consent {
                continue;
            }
            self.images.generation = self.images.generation.wrapping_add(1);
            let generation = self.images.generation;
            *state = State::Loading(generation);
            changed = true;
            let resources = resources.clone();
            let client = cx.http_client();
            let task_url = url.clone();
            let task = cx.spawn(async move |document, cx| {
                let load_url = url.clone();
                let owner = resources.clone();
                let result = if external(&url) {
                    use smol::io::AsyncReadExt as _;
                    async {
                        use gpui_kit::http_client::{AsyncBody, HttpRequestExt as _, RedirectPolicy};
                        let request = gpui_kit::http_client::Request::get(url.as_str())
                            .follow_redirects(RedirectPolicy::FollowAll).body(AsyncBody::default())?;
                        let mut response = client.send(request).await?;
                        anyhow::ensure!(response.status().is_success(), "HTTP {}", response.status());
                        let mut bytes = Vec::new();
                        response.body_mut().take(MAX_BYTES + 1).read_to_end(&mut bytes).await?;
                        anyhow::ensure!(bytes.len() as u64 <= MAX_BYTES, "image exceeds 32 MiB");
                        cx.background_executor().spawn(async move { decode(&bytes) }).await
                    }.await
                } else {
                    crate::ui::host_ops::off_thread(move || {
                        let file_url = url::Url::parse(&load_url).ok().filter(|url| url.scheme() == "file");
                        anyhow::ensure!(file_url.as_ref().is_none_or(|url| url.host_str().is_none_or(|host| host == "localhost")), "file URI names a different host");
                        let decoded = percent_encoding::percent_decode_str(file_url.as_ref().map_or(load_url.as_str(), |url| url.path())).decode_utf8()?;
                        let value = if owner.host().separator() == '\\' && file_url.is_some() { decoded.strip_prefix('/').unwrap_or(&decoded) } else { &decoded };
                        let host = owner.host();
                        let path = std::path::Path::new(value);
                        let path = if host.is_absolute(path) { path.to_path_buf() } else {
                            anyhow::ensure!(!value.contains("://") && !value.starts_with("//") && !value.contains(':'), "unsupported image scheme");
                            let document = owner.path().ok_or_else(|| anyhow::anyhow!("save the document before loading relative images"))?;
                            let name = document.to_string_lossy();
                            let parent = name.rsplit_once(host.separator()).map_or("", |(parent, _)| parent);
                            host.join(std::path::Path::new(parent), value)
                        };
                        decode(&host.read_file(&path, MAX_BYTES)?)
                    }).await.unwrap_or_else(|| Err(anyhow::anyhow!("image Host operation stopped")))
                };
                let _ = document.update(cx, |document, cx| {
                    if !matches!(document.images.states.get(&url), Some(State::Loading(id)) if *id == generation) { return; }
                    if !resources.is_current(document.resources()) || document.revision() != revision {
                        if resources.is_current(document.resources()) {
                            document.images.states.remove(&url);
                            cx.emit(super::DocumentEvent::ImagesChanged);
                        }
                        return;
                    }
                    document.images.states.insert(url, match result {
                        Ok(image) => State::Ready(image),
                        Err(error) => State::Failed(error.to_string()),
                    });
                    cx.emit(super::DocumentEvent::ImagesChanged);
                    cx.notify();
                });
            });
            self.images.tasks.insert(task_url, task);
        }
        if changed {
            cx.emit(super::DocumentEvent::ImagesChanged);
        }
    }
}
