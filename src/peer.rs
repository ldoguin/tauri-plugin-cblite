//! Peer-to-peer replication.
//!
//! Couchbase Lite can accept replication connections directly, with no Sync
//! Gateway and no server, through `CBLURLEndpointListener`. The Rust binding
//! exposes it; this plugin did not, so these commands add it.
//!
//! What makes this different from replication through Sync Gateway is that
//! **there is no arbiter**. Sync Gateway can decide who is allowed to write
//! what, and a transaction can decide who wins a contended update — here two
//! peers agree with each other and with nobody else, and any conflict that
//! produces is the caller's to resolve.
//!
//! ## This is an Enterprise feature
//!
//! `CBLURLEndpointListener` lives behind the `enterprise` feature of the
//! Couchbase Lite crate, so these commands are compiled only when the plugin is
//! built with `--features enterprise`. A Community build has no listener at
//! all - not a stub that fails at runtime, but no such command - and the
//! frontend finds that out by asking, rather than by a replication that quietly
//! never connects.
//!
//! Deliberately unauthenticated and unencrypted: `tls_identity: None` and
//! `authenticator: None`. That is right for two machines on a trusted LAN and
//! wrong for anything else, and this is stated here rather than hidden behind
//! a flag that looks production-ready.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{command, Runtime, State};

use couchbase_lite::url_endpoint_listener::{ListenerConfiguration, UrlEndpointListener};

use crate::PluginStateArc;

/// The running listener, if this process is currently hosting one.
///
/// `UrlEndpointListener` is not `Send` for the same reason the database is not:
/// Couchbase Lite callbacks arrive on arbitrary threads. Every access goes
/// through this mutex, exactly as the plugin already does for `PluginState`.
pub struct PeerState {
    pub listener: Option<UrlEndpointListener>,
}

unsafe impl Send for PeerState {}
unsafe impl Sync for PeerState {}

pub type PeerStateArc = Arc<Mutex<PeerState>>;

pub fn new_peer_state() -> PeerStateArc {
    Arc::new(Mutex::new(PeerState { listener: None }))
}

#[derive(Serialize)]
pub struct PeerListenerInfo {
    /// The port actually bound. Asking for 0 lets the OS choose one.
    pub port: u16,
    /// Every address a peer could dial, as ws:// URLs.
    pub urls: Vec<String>,
}

#[derive(Serialize)]
pub struct PeerConnectionStatus {
    pub listening: bool,
    pub port: u16,
    pub connections: u64,
    pub active_connections: u64,
    pub urls: Vec<String>,
}

/// Start accepting replication connections from other Couchbase Lite instances.
///
/// Returns the port and URLs, because with `port: 0` the caller cannot know
/// where it ended up, and a peer has to be told where to dial.
#[command]
pub async fn start_peer_listener<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginStateArc>,
    peer: State<'_, PeerStateArc>,
    collections: Vec<String>,
    port: Option<u16>,
    read_only: Option<bool>,
) -> Result<PeerListenerInfo, String> {
    let db_guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = db_guard.as_ref().ok_or("Database not open")?;

    if collections.is_empty() {
        return Err("start_peer_listener needs at least one collection".to_string());
    }

    let mut open = Vec::with_capacity(collections.len());
    for spec in &collections {
        let (scope_name, coll_name) = crate::commands::parse_collection_public(spec);
        open.push(crate::commands::open_collection_public(
            &plugin_state.db,
            scope_name,
            coll_name,
        )?);
    }

    let config = ListenerConfiguration {
        collections: open,
        port: port.unwrap_or(0),
        network_interface: None,
        // No TLS and no authenticator: two machines on a trusted LAN. See the
        // module comment - this is not a shippable configuration on its own.
        tls_identity: None,
        authenticator: None,
        read_only: read_only.unwrap_or(false),
        enable_delta_sync: false,
    };

    let listener = UrlEndpointListener::new(config).map_err(|e| e.to_string())?;
    listener.start().map_err(|e| e.to_string())?;

    let info = PeerListenerInfo {
        port: listener.port(),
        urls: listener.urls(),
    };

    let mut peer_guard = peer.lock().map_err(|e| e.to_string())?;
    // Replacing an existing listener stops it: two listeners on one database is
    // not a configuration anybody wants by accident.
    if let Some(previous) = peer_guard.listener.take() {
        previous.stop();
    }
    peer_guard.listener = Some(listener);

    Ok(info)
}

#[command]
pub async fn stop_peer_listener(peer: State<'_, PeerStateArc>) -> Result<(), String> {
    let mut guard = peer.lock().map_err(|e| e.to_string())?;
    if let Some(listener) = guard.listener.take() {
        listener.stop();
    }
    Ok(())
}

/// Whether this process is hosting, and how many peers are connected.
#[command]
pub async fn peer_listener_status(
    peer: State<'_, PeerStateArc>,
) -> Result<PeerConnectionStatus, String> {
    let guard = peer.lock().map_err(|e| e.to_string())?;

    Ok(match guard.listener.as_ref() {
        Some(listener) => {
            let status = listener.status();
            PeerConnectionStatus {
                listening: true,
                port: listener.port(),
                connections: status.connection_count,
                active_connections: status.active_connection_count,
                urls: listener.urls(),
            }
        }
        None => PeerConnectionStatus {
            listening: false,
            port: 0,
            connections: 0,
            active_connections: 0,
            urls: vec![],
        },
    })
}
