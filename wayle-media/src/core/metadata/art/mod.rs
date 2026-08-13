mod error;
mod resolver;
mod youtube;

pub(crate) use error::ArtResolverError;
pub(crate) use resolver::{ArtResolver, ResolveResult};
pub(crate) use youtube::thumbnail_for_page_url;
