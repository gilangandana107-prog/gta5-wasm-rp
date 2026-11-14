//! Thin host wrapper delegating zero-extract asset downloads to `rp_client_core::sync`.

use anyhow::Result;
use std::path::Path;

pub fn sync_game_data(
    server_input: &str,
    token: Option<&str>,
    dest: &Path,
    keep_code: bool,
    progress: impl Fn(u64, u64) + Send + Sync,
) -> Result<()> {
    rp_client_core::sync::sync_game_data(
        server_input,
        token,
        dest,
        keep_code,
        crate::config::game_ready(),
        progress,
    )
}

#[cfg(target_os = "ios")]
mod ffi {
    use std::ffi::{c_char, CStr, CString};
    use std::sync::Mutex;

    static SYNC_ERROR: Mutex<Option<CString>> = Mutex::new(None);

    #[no_mangle]
    pub extern "C" fn muguet_sync(
        server: *const c_char,
        token: *const c_char,
        progress: extern "C" fn(u64, u64),
    ) -> bool {
        let srv = if server.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(server) }.to_string_lossy().into_owned()
        };
        let tok = if token.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(token) }.to_string_lossy().into_owned())
        };
        match super::sync_game_data(
            &srv,
            tok.as_deref(),
            &crate::config::game_dir(),
            false,
            |d, t| progress(d, t),
        ) {
            Ok(()) => true,
            Err(e) => {
                *SYNC_ERROR.lock().unwrap() = CString::new(format!("{e:#}")).ok();
                false
            }
        }
    }

    #[no_mangle]
    pub extern "C" fn muguet_sync_cancel() {
        rp_client_core::sync::request_cancel();
    }

    #[no_mangle]
    pub extern "C" fn muguet_sync_error() -> *const c_char {
        SYNC_ERROR
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null())
    }

    #[no_mangle]
    pub extern "C" fn muguet_default_server() -> *const c_char {
        static SRV: std::sync::OnceLock<CString> = std::sync::OnceLock::new();
        SRV.get_or_init(|| CString::new(crate::config::default_server()).unwrap_or_default())
            .as_ptr()
    }
}
