//! Source support shown by health reports and generated language documentation.
use crate::queries::QueryFeature;
use loader::syntax::{QuerySource, Resources};

pub struct LanguageSupport {
    pub queries: Vec<(QueryFeature, QuerySource)>,
}

impl LanguageSupport {
    /// Inspect display features without loading grammars or compiling queries.
    pub fn inspect(resources: &Resources, language: &str) -> Self {
        Self {
            queries: QueryFeature::all()
                .iter()
                .map(|&feature| {
                    (
                        feature,
                        resources.query_source(language, feature.runtime_filename()),
                    )
                })
                .collect(),
        }
    }
}
