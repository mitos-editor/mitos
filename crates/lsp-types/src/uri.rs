use std::{hash::Hash, ops::Deref, str::FromStr};

use serde::{de::Error, Deserialize, Serialize};

/// A URI reference serialized as a string, preserving its original spelling.
///
/// Wraps `fluent_uri::UriRef<String>` to retain support for relative references.
#[derive(Debug, Clone)]
pub struct Uri(fluent_uri::UriRef<String>);

impl Serialize for Uri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Uri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = String::deserialize(deserializer)?;
        fluent_uri::UriRef::parse(string)
            .map(Uri)
            .map_err(|(error, _)| Error::custom(error))
    }
}

impl Ord for Uri {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Uri {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Uri {
    type Err = fluent_uri::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl Deref for Uri {
    type Target = fluent_uri::UriRef<String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for Uri {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Uri {}

impl Hash for Uri {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}
