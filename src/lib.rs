use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub mod events;

// ── Desktop-only modules (requires the native-cbl feature) ────────────────────
#[cfg(all(not(mobile), feature = "native-cbl"))]
pub mod commands;
#[cfg(all(not(mobile), feature = "native-cbl"))]
use couchbase_lite::{collection::CollectionChangeListener, Database, Listener, ListenerToken, Replicator};
#[cfg(all(not(mobile), feature = "native-cbl"))]
use commands::PredictionStateArc;
#[cfg(all(not(mobile), feature = "native-cbl"))]
use std::sync::{Arc, Mutex};

// ── Mobile-only modules ───────────────────────────────────────────────────────
#[cfg(mobile)]
mod mobile;
#[cfg(mobile)]
mod commands_mobile;

// ── Desktop plugin state ──────────────────────────────────────────────────────

/// Holds the open database, active listeners, and optional replicator.
///
/// `Database`, `Replicator`, and `ListenerToken` are not `Send` in the upstream
/// crate because CBL callbacks fire on arbitrary threads. The `unsafe impl`s
/// below are required for Tauri's `Mutex`-guarded state. All access is
/// serialised through the `Mutex`, so this is safe in practice.
#[cfg(all(not(mobile), feature = "native-cbl"))]
pub struct PluginState {
    pub db: Database,
    /// Opaque listener tokens kept alive for the plugin's lifetime.
    pub listeners: Vec<ListenerToken>,
    /// Collection change listeners (kept alive to receive callbacks).
    pub coll_listeners: Vec<Listener<CollectionChangeListener>>,
    pub replicator: Option<Replicator>,
}

#[cfg(all(not(mobile), feature = "native-cbl"))]
// SAFETY: all access is serialised through Arc<Mutex<Option<PluginState>>>.
unsafe impl Send for PluginState {}
#[cfg(all(not(mobile), feature = "native-cbl"))]
unsafe impl Sync for PluginState {}

#[cfg(all(not(mobile), feature = "native-cbl"))]
type PluginStateArc = Arc<Mutex<Option<PluginState>>>;

// ── Plugin init ───────────────────────────────────────────────────────────────

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    #[cfg(all(not(mobile), feature = "native-cbl"))]
    {
        Builder::new("cblite")
            .invoke_handler(tauri::generate_handler![
                commands::open_database,
                commands::close_database,
                commands::get_document,
                commands::save_document,
                commands::start_replication,
                commands::stop_replication,
                commands::execute_query,
                commands::create_fts_index,
                commands::register_predictive_model,
                commands::unregister_predictive_model,
                commands::save_blob,
                commands::get_blob_data,
            ])
            .setup(|app, _api| {
                let state: PluginStateArc = Arc::new(Mutex::new(None));
                app.manage(state);
                let pred_state: PredictionStateArc =
                    Arc::new(Mutex::new(std::collections::HashSet::new()));
                app.manage(pred_state);
                Ok(())
            })
            .build()
    }

    #[cfg(mobile)]
    {
        Builder::new("cblite")
            .invoke_handler(tauri::generate_handler![
                commands_mobile::open_database,
                commands_mobile::close_database,
                commands_mobile::get_document,
                commands_mobile::save_document,
                commands_mobile::start_replication,
                commands_mobile::stop_replication,
                commands_mobile::execute_query,
                commands_mobile::create_fts_index,
                commands_mobile::register_predictive_model,
                commands_mobile::unregister_predictive_model,
                commands_mobile::save_blob,
                commands_mobile::get_blob_data,
            ])
            .setup(|app, api| {
                let m = mobile::init(app, api)?;
                app.manage(m);
                Ok(())
            })
            .build()
    }
}
