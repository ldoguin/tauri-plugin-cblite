/// Rust-side mobile layer. All CBL logic lives in the Kotlin plugin; this
/// module just holds a handle to call into it.
use serde::{de::DeserializeOwned, Serialize};
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<MobileCblite<R>, Box<dyn std::error::Error>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("com.plugin.cblite", "CblitePlugin")?;
    #[cfg(not(target_os = "android"))]
    // iOS is not yet implemented; the handle is provided by Tauri's no-op iOS shim.
    let handle = api.register_ios_plugin(noop_ios_init)?;

    Ok(MobileCblite(handle))
}

/// Wraps the `PluginHandle` used to call into the Kotlin (Android) plugin.
pub struct MobileCblite<R: Runtime>(pub PluginHandle<R>);

impl<R: Runtime> MobileCblite<R> {
    /// Call a command on the mobile plugin and deserialise the response.
    pub fn run<P: Serialize, T: DeserializeOwned>(
        &self,
        command: &str,
        payload: &P,
    ) -> Result<T, String> {
        self.0.run_mobile_plugin(command, payload).map_err(|e| e.to_string())
    }
}

// ── iOS stub ─────────────────────────────────────────────────────────────────

#[cfg(not(target_os = "android"))]
fn noop_ios_init(_webview: &tauri::WebviewWindow<impl Runtime>) {}
