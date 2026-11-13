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
//! Plain and unauthenticated by default: `tls_identity: None` and
//! `authenticator: None`. That is right for two machines on a trusted LAN and
//! wrong for anything else, so it is **not** what you get unless you leave the
//! options below alone.
//!
//! ## Opt-in transport security
//!
//! `start_peer_listener` can additionally:
//!
//! * `tls: true` - serve over TLS (`wss://`) with a self-signed certificate that
//!   is made fresh on every start (the binding cannot persist an identity on
//!   Linux). The certificate is returned in `PeerListenerInfo::certificate` so a
//!   peer can pin it (`start_replication`'s `pinned_certificate`).
//! * `require_credentials: true` - let a peer in only if the host's admission
//!   check accepts the username and password it presents. The check is Rust code
//!   that the *host application* registers with [`set_peer_admission`]; it never
//!   crosses into JavaScript, so a webview cannot swap it out. Dialers present
//!   their credentials through `start_replication`'s `username` / `password`.
//!
//! The result tells the caller what is actually in force (`tls`,
//! `authenticated`). Check it: an older build of this plugin ignores arguments it
//! does not know, which would otherwise be a silent downgrade to plain.
//!
//! This is *connection* security. It says nothing about the documents that then
//! replicate; see the application for that.

use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{command, Runtime, State};

use couchbase_lite::tls_identity::TLSIdentity;
use couchbase_lite::url_endpoint_listener::{ListenerAuthenticator, ListenerConfiguration, UrlEndpointListener};
use couchbase_lite::MutableDict;

use crate::PluginStateArc;

/// Decides whether a connecting peer is let in: `(username, password) -> allowed`.
pub type AdmitFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// The running listener, if this process is currently hosting one.
///
/// `UrlEndpointListener` is not `Send` for the same reason the database is not:
/// Couchbase Lite callbacks arrive on arbitrary threads. Every access goes
/// through this mutex, exactly as the plugin already does for `PluginState`.
pub struct PeerState {
    pub listener: Option<UrlEndpointListener>,
    /// Whether the running listener serves TLS (its URLs are then `wss://`).
    pub tls: bool,
    /// The admission check used when a listener is started with `require_credentials`.
    pub admit: Option<AdmitFn>,
}

unsafe impl Send for PeerState {}
unsafe impl Sync for PeerState {}

pub type PeerStateArc = Arc<Mutex<PeerState>>;

pub fn new_peer_state() -> PeerStateArc {
    Arc::new(Mutex::new(PeerState { listener: None, tls: false, admit: None }))
}

/// Register (or clear) the admission check that `start_peer_listener` uses when it is
/// called with `require_credentials: true`. Call this from Rust, before the frontend
/// starts the listener; for example `set_peer_admission(&*app.state::<PeerStateArc>(), Some(check))`.
pub fn set_peer_admission(peer: &PeerStateArc, admit: Option<AdmitFn>) {
    if let Ok(mut g) = peer.lock() {
        g.admit = admit;
    }
}

#[derive(Serialize)]
pub struct PeerListenerInfo {
    /// The port actually bound. Asking for 0 lets the OS choose one.
    pub port: u16,
    /// Every address a peer could dial, as ws:// (or, with TLS, wss://) URLs.
    pub urls: Vec<String>,
    /// Whether the listener serves TLS. Check it when you asked for TLS.
    pub tls: bool,
    /// The listener's certificate (PEM), when it serves TLS: give it to peers that should pin it.
    pub certificate: Option<String>,
    /// Whether peers must present credentials that the admission check accepts.
    pub authenticated: bool,
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
    // Serve over TLS with a fresh self-signed certificate (see the module docs).
    tls: Option<bool>,
    // Common name for that certificate. Defaults to "cblite-peer".
    common_name: Option<String>,
    // Admit only peers the registered admission check accepts (see `set_peer_admission`).
    require_credentials: Option<bool>,
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

    let use_tls = tls.unwrap_or(false);
    let tls_identity = if use_tls {
        let mut attrs = MutableDict::new();
        attrs.at("CN").put_string(common_name.as_deref().filter(|s| !s.is_empty()).unwrap_or("cblite-peer"));
        Some(TLSIdentity::create(true, &attrs, None, None).map_err(|e| format!("TLS identity: {e}"))?)
    } else {
        None
    };
    let certificate = tls_identity
        .as_ref()
        .map(|i| String::from_utf8_lossy(&i.certificates().data(true)).into_owned());

    let authenticator = if require_credentials.unwrap_or(false) {
        let admit = peer
            .lock()
            .map_err(|e| e.to_string())?
            .admit
            .clone()
            .ok_or("require_credentials is set, but the host application has not registered an admission check (set_peer_admission)")?;
        Some(ListenerAuthenticator::password(move |user, password| admit(&user, &password)))
    } else {
        None
    };
    let authenticated = authenticator.is_some();

    let config = ListenerConfiguration {
        collections: open,
        port: port.unwrap_or(0),
        network_interface: None,
        // Plain and unauthenticated unless the caller asked otherwise: right for two
        // machines on a trusted LAN, nothing else. See the module comment.
        tls_identity,
        authenticator,
        read_only: read_only.unwrap_or(false),
        enable_delta_sync: false,
    };

    let listener = UrlEndpointListener::new(config).map_err(|e| e.to_string())?;
    listener.start().map_err(|e| e.to_string())?;

    let info = PeerListenerInfo {
        port: listener.port(),
        urls: peer_urls(&listener, use_tls),
        tls: use_tls,
        certificate,
        authenticated,
    };

    let mut peer_guard = peer.lock().map_err(|e| e.to_string())?;
    // Replacing an existing listener stops it: two listeners on one database is
    // not a configuration anybody wants by accident.
    if let Some(previous) = peer_guard.listener.take() {
        previous.stop();
    }
    peer_guard.listener = Some(listener);
    peer_guard.tls = use_tls;

    Ok(info)
}

/// The URLs a peer could dial. The library reports `ws://` even for a TLS listener; a dialer needs `wss://`.
fn peer_urls(listener: &UrlEndpointListener, tls: bool) -> Vec<String> {
    let urls = listener.urls();
    if tls {
        urls.into_iter().map(|u| u.replacen("ws://", "wss://", 1)).collect()
    } else {
        urls
    }
}

#[command]
pub async fn stop_peer_listener(peer: State<'_, PeerStateArc>) -> Result<(), String> {
    let mut guard = peer.lock().map_err(|e| e.to_string())?;
    if let Some(listener) = guard.listener.take() {
        listener.stop();
    }
    guard.tls = false;
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
                urls: peer_urls(listener, guard.tls),
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
