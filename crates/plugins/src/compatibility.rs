//! Manifest/service versions are distinct from engine and editor releases. The
//! host supports one frozen WIT world; linking verifies artifact imports/exports.

use plugin_api::{ErrorCode, ServiceError};

pub const MANIFEST_VERSION: u32 = 1;
pub const API_VERSION: &str = plugin_api::SERVICE_API_VERSION;

fn release(value: &str) -> Result<(u32, u32, u32), ServiceError> {
    let invalid = || {
        ServiceError::new(
            ErrorCode::InvalidRequest,
            "release requirement must use major.minor.patch numeric format",
        )
    };
    if value.len() > 32 {
        return Err(invalid());
    }
    let mut parts = value.split('.');
    let mut part = || {
        parts
            .next()
            .filter(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|part| part.parse::<u32>().ok())
            .ok_or_else(&invalid)
    };
    let version = (part()?, part()?, part()?);
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(version)
}

pub fn validate(manifest: u32, api: &str, minimum_host: Option<&str>) -> Result<(), ServiceError> {
    if manifest != MANIFEST_VERSION {
        return Err(ServiceError::new(
            ErrorCode::UnsupportedInterface,
            format!("unsupported manifest version {manifest}; host supports {MANIFEST_VERSION}"),
        ));
    }
    if api != API_VERSION {
        return Err(ServiceError::new(
            ErrorCode::UnsupportedInterface,
            format!(
                "unsupported service API '{api}'; host supports frozen WIT world {API_VERSION}"
            ),
        ));
    }
    if let Some(minimum) = minimum_host {
        let current = release(env!("CARGO_PKG_VERSION"))?;
        if release(minimum)? > current {
            return Err(ServiceError::new(
                ErrorCode::UnsupportedInterface,
                format!(
                    "package requires host {minimum}; running {}",
                    env!("CARGO_PKG_VERSION")
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_api_and_host_requirements_reject_unsupported_packages() {
        assert!(validate(1, API_VERSION, Some("0.0.0")).is_ok());
        assert_eq!(
            validate(2, API_VERSION, None).unwrap_err().code,
            ErrorCode::UnsupportedInterface
        );
        assert_eq!(
            validate(1, "0.2.0", None).unwrap_err().code,
            ErrorCode::UnsupportedInterface
        );
        assert!(validate(1, API_VERSION, Some("999.0.0")).is_err());
        assert!(validate(1, API_VERSION, Some("../0.3")).is_err());
    }
}
