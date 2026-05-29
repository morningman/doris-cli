use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct ProductCatalog {
    products: BTreeMap<String, ProductProfile>,
}

#[derive(Debug, Deserialize)]
pub struct ProductProfile {
    pub binary: String,
    pub about: String,
    pub config_dir: String,
    pub env_prefix: String,
}

impl ProductProfile {
    pub fn env_key(&self, suffix: &str) -> String {
        format!("{}_{}", self.env_prefix, suffix)
    }
}

static PRODUCT_CATALOG: Lazy<ProductCatalog> = Lazy::new(|| {
    toml::from_str(include_str!("../config/products.toml"))
        .expect("config/products.toml must be valid")
});

pub fn get_product(id: &str) -> &'static ProductProfile {
    PRODUCT_CATALOG
        .products
        .get(id)
        .unwrap_or_else(|| panic!("unknown product profile '{id}'"))
}
