#![allow(missing_docs)]
use enum_dispatch::enum_dispatch;
use vector_config::{configurable_component, NamedComponent};

#[cfg(feature = "moogsoft-pipelines")]
use crate::providers::moogsoft::provider::MoogsoftHttpConfig;
use crate::{
    config::{ConfigBuilder, ProviderConfig},
    signal,
};

pub mod http;
#[cfg(feature = "moogsoft-pipelines")]
pub mod moogsoft;

pub type BuildResult = std::result::Result<ConfigBuilder, Vec<String>>;

/// Configurable providers in Vector.
#[configurable_component]
#[derive(Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[enum_dispatch(ProviderConfig)]
pub enum Providers {
    /// HTTP.
    Http(http::HttpConfig),
    /// Moogsoft
    #[cfg(feature = "moogsoft-pipelines")]
    Moogsoft(MoogsoftHttpConfig),
}

// TODO: Use `enum_dispatch` here.
impl NamedComponent for Providers {
    fn get_component_name(&self) -> &'static str {
        match self {
            Self::Http(config) => config.get_component_name(),
            #[cfg(feature = "moogsoft-pipelines")]
            Self::Moogsoft(config) => config.get_component_name(),
        }
    }
}
