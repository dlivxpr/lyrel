//! 显式单价与公共目录缓存；核心不发起网络请求。
use super::{Tokens, UsageRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CATALOG_URL: &str = "https://models.dev/api.json";

/// 每百万 Token 的单价；缺失类别不是零，也不能借用其他类别。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnitPrice {
    pub provider: String,
    pub model: String,
    pub currency: String,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub reasoning: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}
impl UnitPrice {
    pub fn validate(&self) -> Result<(), String> {
        if self.provider.trim().is_empty() || self.provider.trim() != self.provider
            || self.model.trim().is_empty() || self.model.trim() != self.model
        {
            return Err("provider and model must be nonempty exact identifiers".into());
        }
        if self.currency.len() != 3 || !self.currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err("currency must be a three-letter uppercase code".into());
        }
        if self.rates().into_iter().flatten().any(|v| !v.is_finite() || v < 0.0) {
            return Err("unit prices must be finite and nonnegative".into());
        }
        Ok(())
    }
    fn rates(&self) -> [Option<f64>; 5] {
        [self.input, self.output, self.reasoning, self.cache_read, self.cache_write]
    }
    fn same_key(&self, other: &Self) -> bool {
        self.provider == other.provider && self.model == other.model && self.currency == other.currency
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pricing {
    pub public_enabled: bool,
    pub custom: Vec<UnitPrice>,
    pub catalog: Vec<UnitPrice>,
    pub checked_at: Option<i64>,
    pub fetched_at: Option<i64>,
    pub last_error: Option<String>,
}
impl Pricing {
    pub fn validate(&self) -> Result<(), String> {
        for rows in [&self.custom, &self.catalog] {
            for (index, row) in rows.iter().enumerate() {
                row.validate()?;
                if rows[..index].iter().any(|previous| previous.same_key(row)) {
                    return Err("duplicate provider/model/currency price".into());
                }
            }
        }
        if self.catalog.iter().any(|row| row.currency != "USD") {
            return Err("models.dev catalog prices must use USD".into());
        }
        Ok(())
    }
    pub fn upsert_custom(&mut self, price: UnitPrice) -> Result<(), String> {
        price.validate()?;
        if let Some(existing) = self.custom.iter_mut().find(|row| row.same_key(&price)) {
            *existing = price;
        } else {
            self.custom.push(price);
        }
        Ok(())
    }
    pub fn remove_custom(&mut self, provider: &str, model: &str, currency: &str) -> bool {
        let before = self.custom.len();
        self.custom.retain(|row| !(row.provider == provider && row.model == model && row.currency == currency));
        before != self.custom.len()
    }
    /// Applies a downloaded provider-specific api.json only after explicit opt-in.
    /// Parse failures retain the last successful catalog and its fetched_at.
    pub fn update_catalog(&mut self, bytes: &[u8], checked_at: i64) -> Result<(), String> {
        if !self.public_enabled {
            return Err("public pricing catalog is disabled".into());
        }
        match parse_catalog(bytes) {
            Ok(catalog) => {
                self.catalog = catalog;
                self.checked_at = Some(checked_at);
                self.fetched_at = Some(checked_at);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.catalog_failed(checked_at, error.clone());
                Err(error)
            }
        }
    }
    pub fn catalog_failed(&mut self, checked_at: i64, error: impl Into<String>) {
        self.checked_at = Some(checked_at);
        self.last_error = Some(error.into());
    }
}

/// 每币种分别呈现估算与覆盖；custom/catalog 均不是账单。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyEstimate {
    pub custom: Option<f64>,
    pub catalog: Option<f64>,
    pub priced_tokens: u64,
    pub unpriced_tokens: u64,
    pub unknown_categories: bool,

    pub calls: u64,
    pub partial_calls: u64,
}
#[derive(Deserialize)]
struct CatalogProvider {
    id: String,
    models: BTreeMap<String, CatalogModel>,
}
#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    cost: Option<CatalogCost>,
}
#[derive(Deserialize)]
struct CatalogCost {
    input: Option<f64>,
    output: Option<f64>,
    reasoning: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}
fn parse_catalog(bytes: &[u8]) -> Result<Vec<UnitPrice>, String> {
    let providers: BTreeMap<String, CatalogProvider> = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid models.dev api.json: {error}"))?;
    let mut prices = Vec::new();
    for (provider_id, provider) in providers {
        if provider_id != provider.id {
            return Err("models.dev provider key does not match id".into());
        }
        for (model_id, model) in provider.models {
            if model_id != model.id {
                return Err("models.dev model key does not match id".into());
            }
            let Some(cost) = model.cost else { continue };
            let price = UnitPrice {
                provider: provider_id.clone(),
                model: model_id,
                currency: "USD".into(),
                input: cost.input,
                output: cost.output,
                reasoning: cost.reasoning,
                cache_read: cost.cache_read,
                cache_write: cost.cache_write,
            };
            price.validate()?;
            prices.push(price);
        }
    }
    if prices.is_empty() {
        return Err("models.dev api.json contains no usable prices".into());
    }
    Ok(prices)
}
impl CurrencyEstimate {
    pub fn total(&self) -> Option<f64> {
        match (self.custom, self.catalog) {
            (None, None) => None,
            (custom, catalog) => Some(custom.unwrap_or(0.0) + catalog.unwrap_or(0.0)),
        }
    }
    pub fn is_complete(&self) -> bool {
        self.partial_calls == 0 && !self.unknown_categories && self.unpriced_tokens == 0
    }
}

pub(super) struct CurrencyCost<'a> {
    pub currency: &'a str,
    pub estimate: CurrencyEstimate,
}
pub(super) struct RecordCost<'a> {
    pub currencies: smallvec::SmallVec<[CurrencyCost<'a>; 2]>,
    pub unpriced: bool,
}
type PricePair<'a> = (&'a str, Option<&'a UnitPrice>, Option<&'a UnitPrice>);
pub(super) struct PriceIndex<'a> {
    rows: std::collections::HashMap<(&'a str, &'a str), smallvec::SmallVec<[PricePair<'a>; 2]>>,
}
impl<'a> PriceIndex<'a> {
    pub fn new(pricing: &'a Pricing) -> Self {
        let mut index = Self { rows: std::collections::HashMap::new() };
        for (rows, catalog) in [(&pricing.custom, false), (&pricing.catalog, true)] {
            if catalog && !pricing.public_enabled {
                continue;
            }
            for row in rows {
                if row.validate().is_err() || (catalog && row.currency != "USD") {
                    continue;
                }
                let currencies = index.rows.entry((&row.provider, &row.model)).or_default();
                let position = currencies.iter().position(|(currency, _, _)| *currency == row.currency);
                let position = position.unwrap_or_else(|| {
                    currencies.push((&row.currency, None, None));
                    currencies.len() - 1
                });
                let pair = &mut currencies[position];
                let target = if catalog { &mut pair.2 } else { &mut pair.1 };
                target.get_or_insert(row);
            }
        }
        index
    }
    pub fn resolve(&self, record: &UsageRecord) -> RecordCost<'a> {
        let mut result = RecordCost { currencies: smallvec::SmallVec::new(), unpriced: record.recorded_usd.is_none() };
        if record.recorded_usd.is_some() {
            return result;
        }
        if let Some(rows) = self.rows.get(&(record.provider.as_str(), record.model.as_str())) {
            for &(currency, custom, catalog) in rows {
                let estimate = estimate(&record.tokens, custom, catalog);
                if estimate.is_complete() {
                    result.unpriced = false;
                }
                result.currencies.push(CurrencyCost { currency, estimate });
            }
        }
        result
    }
}
fn estimate(tokens: &Tokens, custom: Option<&UnitPrice>, catalog: Option<&UnitPrice>) -> CurrencyEstimate {
    let mut result = CurrencyEstimate { calls: 1, ..CurrencyEstimate::default() };
    let counts = [tokens.input, tokens.output, tokens.reasoning, tokens.cache_read, tokens.cache_write];
    let custom = custom.map(UnitPrice::rates).unwrap_or([None; 5]);
    let catalog = catalog.map(UnitPrice::rates).unwrap_or([None; 5]);
    let known = counts.into_iter().flatten().try_fold(0u64, u64::checked_add);
    result.unknown_categories = tokens.total.zip(known).is_none_or(|(total, known)| total != known);
    result.unpriced_tokens = tokens.total.zip(known)
        .map(|(total, known)| total.saturating_sub(known)).unwrap_or(0);
    for ((count, custom), catalog) in counts.into_iter().zip(custom).zip(catalog) {
        let Some(count) = count.filter(|count| *count > 0) else { continue };
        let amount = custom.or(catalog).map(|rate| count as f64 * (rate / 1_000_000.0));
        if let Some(amount) = amount.filter(|amount| amount.is_finite()) {
            let target = if custom.is_some() { &mut result.custom } else { &mut result.catalog };
            *target = Some(target.unwrap_or(0.0) + amount);
            result.priced_tokens = result.priced_tokens.saturating_add(count);
        } else {
            result.unpriced_tokens = result.unpriced_tokens.saturating_add(count);
        }
    }
    result.partial_calls = u64::from(result.unknown_categories || result.unpriced_tokens > 0);
    result
}

pub(super) fn add_estimates(target: &mut BTreeMap<String, CurrencyEstimate>, cost: &RecordCost<'_>) {
    for row in &cost.currencies {
        let total = if target.contains_key(row.currency) {
            target.get_mut(row.currency).unwrap()
        } else {
            target.entry(row.currency.to_owned()).or_default()
        };
        let value = &row.estimate;
        for (target, value) in [(&mut total.custom, value.custom), (&mut total.catalog, value.catalog)] {
            if let Some(value) = value {
                *target = Some(target.unwrap_or(0.0) + value);
            }
        }
        total.priced_tokens = total.priced_tokens.saturating_add(value.priced_tokens);
        total.unpriced_tokens = total.unpriced_tokens.saturating_add(value.unpriced_tokens);
        total.unknown_categories |= value.unknown_categories;
        total.calls += value.calls;
        total.partial_calls += value.partial_calls;
    }
}
