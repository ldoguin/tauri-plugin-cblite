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
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_cblite)?;

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

// ── iOS Swift package entry-point (provided by ios/Sources/CblitePlugin.swift) ──

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_cblite);
