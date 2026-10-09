//! Rust guest adapters and reexports of Mitos's runtime-neutral plugin protocol.
//!
//! Plugins are ordinary core WebAssembly modules, without WASI or host imports.
//! Use [`export_plugin!`] to export the allocation and request entry points.
//! Document offsets count Unicode scalar values; they are not UTF-8 byte offsets.

pub use plugin_api::protocol::*;

#[cfg(any(target_arch = "wasm32", test))]
fn dispatch_request(input: &[u8], handler: fn(Request) -> Response) -> Response {
    match serde_json::from_slice::<Request>(input) {
        Ok(request) if request.abi_version == ABI_VERSION => handler(request),
        Ok(request) => Response {
            error: Some(format!(
                "Unsupported plugin ABI version {}",
                request.abi_version
            )),
            ..Response::default()
        },
        Err(error) => Response {
            error: Some(format!("Invalid plugin request: {error}")),
            ..Response::default()
        },
    }
}

/// Export the Mitos memory ABI for a `fn(Request) -> Response` handler.
///
/// ```
/// use plugin_sdk::{Request, Response, export_plugin};
///
/// fn handle(_request: Request) -> Response {
///     Response::default()
/// }
///
/// export_plugin!(handle);
/// ```
///
/// The exports are emitted only for `wasm32`, allowing the handler to be tested
/// natively. Compile a `cdylib` for `wasm32-unknown-unknown` to produce a plugin.
#[macro_export]
macro_rules! export_plugin {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn mitos_alloc(length: i32) -> i32 {
            $crate::guest::allocate(length)
        }

        /// # Safety
        /// The pointer and length must describe a live allocation returned by
        /// `mitos_alloc` or `mitos_call`, and may be freed only once.
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn mitos_dealloc(pointer: i32, length: i32) {
            // SAFETY: The host owns the buffer and fulfills the ABI contract.
            unsafe { $crate::guest::deallocate(pointer, length) }
        }

        /// # Safety
        /// The pointer and length must describe a live `mitos_alloc` buffer
        /// containing the JSON request. The host retains ownership of it.
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn mitos_call(pointer: i32, length: i32) -> i64 {
            // SAFETY: The host supplies a readable buffer for this invocation.
            unsafe { $crate::guest::call(pointer, length, $handler) }
        }
    };
}

/// Implementation details used by [`export_plugin!`].
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod guest {
    use super::{dispatch_request, Request, Response};

    pub fn allocate(length: i32) -> i32 {
        if length <= 0 {
            return 0;
        }
        let buffer = vec![0_u8; length as usize].into_boxed_slice();
        Box::into_raw(buffer).cast::<u8>() as usize as i32
    }

    /// # Safety
    /// `pointer` and `length` must identify a live boxed byte buffer returned by
    /// this module. It must not have been freed, and may be deallocated once.
    pub unsafe fn deallocate(pointer: i32, length: i32) {
        if pointer == 0 || length <= 0 {
            return;
        }
        let pointer = pointer as u32 as *mut u8;
        let buffer = std::ptr::slice_from_raw_parts_mut(pointer, length as usize);
        // SAFETY: The ABI passes the exact allocation address and length. Every
        // buffer exported by this module was allocated as a boxed byte slice.
        drop(unsafe { Box::from_raw(buffer) });
    }

    /// # Safety
    /// `pointer` and `length` must identify a readable allocation returned by
    /// `allocate`. The host keeps that allocation alive until this call returns.
    pub unsafe fn call(pointer: i32, length: i32, handler: fn(Request) -> Response) -> i64 {
        let response = if pointer == 0 || length <= 0 {
            Response {
                error: Some("Invalid plugin request buffer".into()),
                ..Response::default()
            }
        } else {
            // SAFETY: The host follows the documented allocation contract. We
            // borrow only for deserialization and never free the input buffer.
            let input =
                unsafe { std::slice::from_raw_parts(pointer as u32 as *const u8, length as usize) };
            dispatch_request(input, handler)
        };
        // Response contains only JSON-representable values; serialization cannot
        // encounter unsupported map keys, custom serializers, or non-finite floats.
        let output = serde_json::to_vec(&response).expect("plugin response is JSON serializable");
        if output.len() > i32::MAX as usize {
            return 0;
        }
        let length = output.len() as u32;
        let pointer = Box::into_raw(output.into_boxed_slice()).cast::<u8>() as usize as u32;
        ((u64::from(pointer) << 32) | u64::from(length)) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_default_optional_invocation_fields() {
        let request: Request = serde_json::from_value(json!({
            "abi_version": ABI_VERSION,
            "event": "document-changed",
            "editor": {"generation": 1, "mode": "normal", "document": null, "view": null}
        }))
        .unwrap();
        assert_eq!(request.event, Event::DocumentChanged);
        assert_eq!(request.command, None);
        assert!(request.args.is_empty());
        assert_eq!(request.config, serde_json::Value::Null);
        assert_eq!(request.data, serde_json::Value::Null);
        assert_eq!(
            serde_json::from_str::<Response>("{}").unwrap(),
            Response::default()
        );
    }

    #[test]
    fn invalid_json_and_unsupported_abi_do_not_invoke_the_handler() {
        fn unexpected_handler(_request: Request) -> Response {
            panic!("Invalid request must not be dispatched")
        }

        let malformed = dispatch_request(b"not JSON", unexpected_handler);
        assert!(malformed.error.unwrap().contains("Invalid plugin request"));
        assert!(malformed.actions.is_empty());

        let unsupported = dispatch_request(
            br#"{"abi_version":1,"event":"init","editor":{"generation":1,"mode":"normal","document":null,"view":null}}"#,
            unexpected_handler,
        );
        assert_eq!(
            unsupported.error.as_deref(),
            Some("Unsupported plugin ABI version 1")
        );
        assert!(unsupported.actions.is_empty());
    }

    #[test]
    fn response_uses_language_neutral_tagged_actions() {
        let response = Response {
            actions: vec![
                Action::Edit {
                    document: 7,
                    version: 3,
                    edits: vec![TextEdit {
                        start: 1,
                        end: 2,
                        text: "É".into(),
                    }],
                },
                Action::SetSelection {
                    document: 7,
                    version: 3,
                    view: 9,
                    binding_revision: 1,
                    selection_revision: 2,
                    ranges: vec![SelectionRange { anchor: 2, head: 1 }],
                    primary: 0,
                },
            ],
            error: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["actions"][0]["type"], "edit");
        assert_eq!(json["actions"][1]["type"], "set-selection");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
    }
}
