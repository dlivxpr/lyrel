//! 桌面本地的非秘密统计设置；公共目录请求只接收代理，不接收任何历史数据。
use lyrel_core::statistics::{pricing::Pricing, sources::SourceBindings};
use serde::{Deserialize, Serialize};
use std::{io, path::Path, time::Duration};

pub(super) const CATALOG_URL: &str = lyrel_core::statistics::pricing::CATALOG_URL;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct StoredStatistics {
    pub pricing: Pricing,
    pub sources: SourceBindings,
}

pub(super) type SettingsStamp = Option<(u64, std::time::SystemTime)>;

pub(super) fn settings_stamp(path: &Path) -> io::Result<SettingsStamp> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some((metadata.len(), metadata.modified()?))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
impl StoredStatistics {
    pub fn load(path: &Path) -> io::Result<Self> {
        let value: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        value.pricing.validate().map_err(io::Error::other)?;
        Ok(value)
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.pricing.validate().map_err(io::Error::other)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        lyrel_core::core::config::write_atomic_private(path, &bytes)
    }
}
/// 唯一可替换的外部 HTTP 边界；没有配置 URL、账户或会话参数。
pub(super) trait CatalogHttp: Send + Sync {
    fn get(&self, proxy: Option<&str>) -> Result<Vec<u8>, String>;
}
pub(super) struct PublicCatalogHttp;
impl CatalogHttp for PublicCatalogHttp {
    fn get(&self, proxy: Option<&str>) -> Result<Vec<u8>, String> {
        let mut builder = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("lyrel/", env!("CARGO_PKG_VERSION")));
        if let Some(proxy) = lyrel_core::daemon::install::proxy::resolve(CATALOG_URL, proxy) {
            builder = builder.proxy(Some(proxy));
        }
        let agent: ureq::Agent = builder.build().into();
        let mut response = agent
            .get(CATALOG_URL)
            .header("Accept", "application/json")
            .call()
            .map_err(|_| "Public catalog network request failed".to_string())?;
        if response.status().as_u16() != 200 {
            return Err(format!(
                "Public catalog HTTP {}",
                response.status().as_u16()
            ));
        }
        response
            .body_mut()
            .with_config()
            .limit(32 * 1024 * 1024)
            .read_to_vec()
            .map_err(|_| "Public catalog response is unreadable or exceeds 32 MiB".into())
    }
}
pub(super) fn check_catalog(
    pricing: &mut Pricing,
    http: &dyn CatalogHttp,
    proxy: Option<&str>,
    now: i64,
) -> Result<(), String> {
    if !pricing.public_enabled {
        return Err("Public catalog is disabled".into());
    }
    match http
        .get(proxy)
        .and_then(|bytes| pricing.update_catalog(&bytes, now))
    {
        Ok(()) => Ok(()),
        Err(error) => {
            pricing.catalog_failed(now, error.clone());
            Err(error)
        }
    }
}
