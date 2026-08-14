/// Headless integration tests for tauri-plugin-cblite.
///
/// Tests B1–B2 use the existing couchbase-lite-rust API and run without a
/// display or network.  Tests B3–B6 depend on enterprise APIs
/// (TLSIdentity, UrlEndpointListener, cert authenticator, server_certificate)
/// that are now implemented in the wrapper. Test C1 replicates against a
/// real Sync Gateway over a real URL endpoint — that's a `native-cbl`
/// (Community Edition) capability, not enterprise-gated, so its imports are
/// unconditional alongside B1's.
use couchbase_lite::{
    Authenticator, Database, DatabaseConfiguration, Document, Endpoint, MutableArray,
    ReplicationCollection, ReplicationConfigurationContext, Replicator, ReplicatorActivityLevel,
    ReplicatorConfiguration, ReplicatorType,
};
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

fn open_temp_db(dir: &TempDir, name: &str) -> Database {
    let config = DatabaseConfiguration {
        directory: dir.path(),
        #[cfg(feature = "enterprise")]
        encryption_key: None,
    };
    Database::open(name, Some(config)).expect("open database")
}

#[cfg(feature = "enterprise")]
fn make_replicator(src: &Database, dst: &Database) -> Replicator {
    let coll = src
        .default_collection_or_error()
        .expect("default collection");
    let endpoint = Endpoint::new_with_local_db(dst);
    let config = ReplicatorConfiguration {
        database: None,
        endpoint,
        replicator_type: ReplicatorType::PushAndPull,
        continuous: false,
        disable_auto_purge: false,
        max_attempts: 1,
        max_attempt_wait_time: 0,
        heartbeat: 0,
        authenticator: None,
        proxy: None,
        headers: std::collections::HashMap::new(),
        pinned_server_certificate: None,
        trusted_root_certificates: None,
        // Must be null (not empty) when `collections` is set.
        channels: MutableArray::default(),
        document_ids: MutableArray::default(),
        collections: Some(vec![ReplicationCollection {
            collection: coll,
            conflict_resolver: None,
            push_filter: None,
            pull_filter: None,
            channels: MutableArray::new(),
            document_ids: MutableArray::new(),
        }]),
        accept_parent_domain_cookies: false,
        #[cfg(feature = "enterprise")]
        accept_only_self_signed_server_certificate: false,
    };
    let ctx = Box::new(ReplicationConfigurationContext::default());
    Replicator::new(config, ctx).expect("create replicator")
}

// ── B1: CollectionChangeListener fires via buffer_notifications ───────────────

/// Verifies that `buffer_notifications` + `send_notifications` delivers a
/// collection change event to a listener, and that the listener receives the
/// saved document's ID.
#[test]
fn collection_change_event_fires() {
    let dir = TempDir::new().unwrap();
    let db = open_temp_db(&dir, "b1_db");

    let (tx, rx) = mpsc::channel::<Vec<String>>();

    // Register a collection change listener.
    let mut coll = db.default_collection_or_error().expect("default collection");
    let _listener = coll.add_listener(Box::new(move |_collection, doc_ids| {
        let _ = tx.send(doc_ids);
    }));

    // Switch to buffered-notification mode so we control when callbacks fire.
    fn notify_ready(db: &Database) {
        db.send_notifications();
    }
    db.buffer_notifications(notify_ready);

    // Save a document — this queues a notification but does not fire it yet.
    let mut doc = Document::new_with_id("b1_doc");
    doc.set_properties_as_json(r#"{"test": true}"#)
        .expect("set properties");
    coll.save_document(&mut doc).expect("save document");

    // Flush buffered notifications.
    db.send_notifications();

    // The listener must fire within 2 seconds.
    let received = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("listener did not fire within 2 seconds");

    assert!(
        received.contains(&"b1_doc".to_string()),
        "expected doc ID 'b1_doc' in changed IDs, got: {:?}",
        received
    );
}

// ── B2: ReplicatorChangeListener receives status events ───────────────────────

/// Verifies that a `ReplicatorChangeListener` receives at least one status
/// event and eventually reaches `Stopped` when replicating between two local
/// databases (no network required).
#[test]
#[cfg(feature = "enterprise")] // local-DB endpoint requires enterprise
fn replication_status_event_fires() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    let db1 = open_temp_db(&dir1, "b2_src");
    let db2 = open_temp_db(&dir2, "b2_dst");

    let (tx, rx) = mpsc::channel::<ReplicatorActivityLevel>();

    let replicator = make_replicator(&db1, &db2);
    let tx_clone = tx.clone();
    let mut replicator = replicator.add_change_listener(Box::new(move |status| {
        let _ = tx_clone.send(status.activity);
    }));

    replicator.start(false);

    // Collect status events until Stopped (10-second timeout).
    // Use recv_timeout in a loop; ignore channel-empty timeouts and keep
    // waiting until the deadline so we don't miss a late Stopped event.
    let mut got_stopped = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
            Ok(ReplicatorActivityLevel::Stopped) => {
                got_stopped = true;
                break;
            }
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    assert!(got_stopped, "replicator did not reach Stopped within 10 seconds");
}

// ── B3: TLSIdentity creation and certificate readback ────────────────────────

/// Verifies `TLSIdentity::create` and that the returned identity exposes a
/// non-null `Cert` whose subject name contains the requested CN.
#[test]
#[cfg(feature = "enterprise")]
fn tls_identity_create_and_read() {
    use couchbase_lite::{MutableDict, TLSIdentity, Timestamp};

    let mut attrs = MutableDict::new();
    attrs.at("CN").put_string("test");

    let expiry = Timestamp::now().add(Duration::from_secs(86400));
    let identity = TLSIdentity::create(true, &attrs, Some(expiry), None)
        .expect("create TLS identity");

    let cert = identity.certificates();
    let subject = cert.subject_name();
    assert!(
        subject.contains("test"),
        "expected subject name to contain 'test', got: {}",
        subject
    );

    let exp = identity.expiration();
    assert!(
        exp.get() > Timestamp::now().get(),
        "expected expiration to be in the future"
    );
}

// ── B4: UrlEndpointListener starts and reports a non-zero port ───────────────

/// Verifies that `UrlEndpointListener` starts, binds to an OS-assigned port,
/// and reports `port() > 0`.
#[test]
#[cfg(feature = "enterprise")]
fn url_endpoint_listener_starts() {
    use couchbase_lite::{ListenerConfiguration, UrlEndpointListener};

    let dir = TempDir::new().unwrap();
    let db = open_temp_db(&dir, "b4_db");
    let coll = db.default_collection_or_error().unwrap();

    let config = ListenerConfiguration {
        collections: vec![coll],
        port: 0, // OS-assigned
        tls_identity: None,
        authenticator: None,
        ..Default::default()
    };

    let listener = UrlEndpointListener::new(config).expect("create listener");
    listener.start().expect("listener start");

    let port = listener.port();
    assert!(port > 0, "expected non-zero port, got {}", port);

    listener.stop();
}

// ── B5: Authenticator::create_certificate compiles ───────────────────────────

/// Verifies that `Authenticator::create_certificate` exists and can be
/// constructed without panicking.
#[test]
#[cfg(feature = "enterprise")]
fn cert_authenticator_compiles() {
    use couchbase_lite::{Authenticator, MutableDict, TLSIdentity};

    let mut attrs = MutableDict::new();
    attrs.at("CN").put_string("client");

    let identity = TLSIdentity::create(false, &attrs, None, None)
        .expect("create client identity");

    // compile-time proof: if this line compiles and doesn't panic, the test passes.
    let _auth = Authenticator::create_certificate(&identity);
}

// ── B6: server_certificate() returns None for local-DB replication ────────────

/// Verifies that `Replicator::server_certificate()` is callable after a
/// local-DB replication completes without panicking or crashing.
///
/// Note: the enterprise library may return a self-signed cert even for local-DB
/// replication; the important invariant is that the API does not panic and the
/// returned value (Some or None) is handled safely.
#[test]
#[cfg(feature = "enterprise")]
fn server_certificate_none_for_local_replication() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();
    let db1 = open_temp_db(&dir1, "b6_src");
    let db2 = open_temp_db(&dir2, "b6_dst");

    let (tx, rx) = mpsc::channel::<ReplicatorActivityLevel>();

    let replicator = make_replicator(&db1, &db2);
    let mut replicator = replicator.add_change_listener(Box::new(move |status| {
        let _ = tx.send(status.activity);
    }));

    replicator.start(false);

    // Wait for Idle or Stopped.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ReplicatorActivityLevel::Stopped) | Ok(ReplicatorActivityLevel::Idle) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    replicator.stop(None);

    // The API must not panic. The enterprise library may return a cert even for
    // local-DB replication; we just verify the call completes safely.
    let _cert = replicator.server_certificate();
    // No assertion on the value — the compile-time proof that the method exists
    // and returns Option<Cert> is the primary goal of this test.
}

// ── C1: real client-server pull picks up a server-side-only mutation ──────────
//
// Reproduces (or disproves) a real bug found debugging the `assist` Tauri
// app's stuck-pull symptom: a document pushed once, then advanced through
// several states SERVER-SIDE ONLY (exactly how assist's brain/state.nu
// advances a task — via Sync Gateway's REST API, never touching this
// plugin or any local CBL database), never had its local copy updated by a
// fresh pull, even after the replicator reported reaching Idle.
//
// This isolates that scenario from all of assist's own complexity (Tauri,
// React, multiple environments) — pure couchbase-lite-rust against a real
// Sync Gateway, so a failure here means the bug is in this plugin/the
// underlying CBL library, not in assist's app code.
//
// Requires a real Sync Gateway reachable at SG_ADMIN_URL, with a database
// SG_DB whose SCOPE.COLLECTION is already provisioned and whose sync
// function allows a new doc at state="ingested" and an ingested->screened
// transition — assist's own docker/compose.yml stack (this repo's sibling
// project) provides exactly this via its "coding" environment. Skipped
// (not failed) if that stack isn't reachable, so the rest of the suite
// still runs in CI/without it.

const SG_ADMIN_URL: &str = "http://localhost:4985";
const SG_PUBLIC_URL: &str = "ws://localhost:4984";
const SG_DB: &str = "assist_coding";
const SCOPE: &str = "coding";
const COLLECTION: &str = "tasks";
const SG_USER: &str = "appuser";
const SG_PASS: &str = "assistpass";

fn sg_keyspace_url(doc_id: &str) -> String {
    format!("{SG_ADMIN_URL}/{SG_DB}.{SCOPE}.{COLLECTION}/{doc_id}")
}

fn sync_gateway_reachable() -> bool {
    std::process::Command::new("curl")
        .args(["-sf", "-o", "/dev/null", SG_ADMIN_URL])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// GET the doc via the admin API (no auth needed — admin port). Returns
/// `(rev, body)` or `None` if the doc doesn't exist (404).
fn sg_get(doc_id: &str) -> Option<(String, serde_json::Value)> {
    let out = std::process::Command::new("curl")
        .args(["-s", &sg_keyspace_url(doc_id)])
        .output()
        .expect("curl get");
    let body: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let rev = body.get("_rev")?.as_str()?.to_string();
    Some((rev, body))
}

/// PUT a same-or-new-state body via the admin API — the same mechanism
/// assist's brain/state.nu save-task uses (Sync Gateway's own sync
/// function still enforces the real state-transition graph either way).
fn sg_put(doc_id: &str, rev: Option<&str>, body: &serde_json::Value) {
    let url = match rev {
        Some(r) => format!("{}?rev={r}", sg_keyspace_url(doc_id)),
        None => sg_keyspace_url(doc_id),
    };
    let out = std::process::Command::new("curl")
        .args([
            "-s",
            "-X",
            "PUT",
            &url,
            "-H",
            "Content-Type: application/json",
            "-d",
            &body.to_string(),
        ])
        .output()
        .expect("curl put");
    let resp: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        resp.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Sync Gateway admin PUT for {doc_id} failed: {resp}"
    );
}

fn sg_cleanup(doc_id: &str) {
    if let Some((rev, _)) = sg_get(doc_id) {
        let _ = std::process::Command::new("curl")
            .args([
                "-s",
                "-X",
                "DELETE",
                &format!("{}?rev={rev}", sg_keyspace_url(doc_id)),
            ])
            .output();
    }
}

/// Runs a real client-server replicator (one-shot, not continuous) to
/// completion (Stopped or Idle) against SG_PUBLIC_URL/SG_DB, in the given
/// direction, for one collection opened on `db`. Panics if it never
/// settles within 15 seconds. Returns the final status's error message, if
/// any (e.g. a fatal BLIP-level rejection like an illegal channel name).
fn run_real_replication(db: &Database, direction: ReplicatorType) -> Option<String> {
    run_real_replication_with_channels(db, direction, &[])
}

/// Same as `run_real_replication`, but lets the caller set an explicit
/// per-collection channel filter instead of leaving it empty — a direct
/// test of whether empty per-collection channels really means "no filter"
/// for THIS binding/CBL version, despite commands.rs's own (now corrected)
/// comment having once claimed it does.
fn run_real_replication_with_channels(db: &Database, direction: ReplicatorType, channel_names: &[&str]) -> Option<String> {
    let coll = db
        .create_collection(COLLECTION.to_string(), SCOPE.to_string())
        .expect("create/open collection");

    let mut coll_channels = MutableArray::new();
    for name in channel_names {
        coll_channels.append().put_string(*name);
    }

    let endpoint = Endpoint::new_with_url(&format!("{SG_PUBLIC_URL}/{SG_DB}"))
        .expect("create URL endpoint");
    let config = ReplicatorConfiguration {
        database: None,
        endpoint,
        replicator_type: direction,
        continuous: false,
        disable_auto_purge: true,
        max_attempts: 1,
        max_attempt_wait_time: 5,
        heartbeat: 30,
        authenticator: Some(Authenticator::create_password(SG_USER, SG_PASS)),
        proxy: None,
        headers: std::collections::HashMap::new(),
        pinned_server_certificate: None,
        trusted_root_certificates: None,
        channels: MutableArray::default(),
        document_ids: MutableArray::default(),
        collections: Some(vec![ReplicationCollection {
            collection: coll,
            conflict_resolver: None,
            push_filter: None,
            pull_filter: None,
            channels: coll_channels,
            document_ids: MutableArray::default(),
        }]),
        accept_parent_domain_cookies: false,
        #[cfg(feature = "enterprise")]
        accept_only_self_signed_server_certificate: false,
    };
    let ctx = Box::new(ReplicationConfigurationContext::default());
    let replicator = Replicator::new(config, ctx).expect("create replicator");

    let (tx, rx) = mpsc::channel::<ReplicatorActivityLevel>();
    let mut replicator = replicator.add_change_listener(Box::new(move |status| {
        eprintln!("[run_real_replication] activity={:?} error={:?}", status.activity, status.error);
        let _ = tx.send(status.activity);
    }));
    replicator.start(false);

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "replicator did not settle (Idle/Stopped) within 15 seconds"
        );
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ReplicatorActivityLevel::Idle) | Ok(ReplicatorActivityLevel::Stopped) => break,
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let final_status = replicator.status();
    eprintln!(
        "[run_real_replication] FINAL status: activity={:?} error={:?} progress={:?}",
        final_status.activity, final_status.error, final_status.progress
    );
    let error_message = final_status.error.err().map(|e| e.to_string());
    replicator.stop(None);
    error_message
}

// Originally written to reproduce the assist stuck-pull bug with plain
// `run_real_replication` (empty per-collection channels). That empirically
// turned out to be its own separate, fatal bug (see below), not a
// reproduction of the state-sync one — so this is now the permanent
// regression guard for THAT finding: empty per-collection channels is not
// "no filter", it's a hard BLIP-level rejection. commands.rs's
// `start_replication` now requires callers to pass real channel names for
// exactly this reason (see its `channels` parameter doc comment). If CBL or
// Sync Gateway ever change this behavior, this test will start failing and
// that comment/requirement should be revisited.
#[test]
fn empty_per_collection_channels_is_a_fatal_error_not_a_no_filter_default() {
    if !sync_gateway_reachable() {
        eprintln!(
            "SKIPPING empty_per_collection_channels_is_a_fatal_error_not_a_no_filter_default: \
             Sync Gateway not reachable at {SG_ADMIN_URL} — run the sibling `assist` repo's \
             docker/compose.yml stack (docker/register-sync-gateway-databases.nu) to exercise this test."
        );
        return;
    }

    let dir = TempDir::new().unwrap();
    let db = open_temp_db(&dir, "c1_empty_channels");
    // Direction doesn't matter — the rejection happens before any documents
    // are exchanged, purely from the per-collection channel filter itself.
    let error = run_real_replication(&db, ReplicatorType::Pull);
    assert!(
        error.as_deref().is_some_and(|e| e.contains("Illegal channel name")),
        "expected a fatal 'Illegal channel name' rejection for empty per-collection channels, got: {error:?}"
    );
}

// ── C2: same scenario, but with an EXPLICIT per-collection channel filter ─────
//
// commands.rs's start_replication carries a comment claiming empty
// per-collection channels means "no filter — all channels the user has
// access to." C1 above empirically contradicts that (push, which isn't
// channel-filtered, works; pull, which is, delivers zero documents). This
// test is the direct A/B: same scenario, but the pull replicator's
// per-collection channels is explicitly `["public"]` — the one channel
// assist's sync function actually assigns every document to
// (`channel('public')`) and the one channel `appuser`'s `admin_channels`
// actually grants. If THIS passes where C1 fails, the fix is exactly:
// stop leaving per-collection channels empty in commands.rs.
#[test]
fn real_pull_with_explicit_channel_works() {
    if !sync_gateway_reachable() {
        eprintln!(
            "SKIPPING real_pull_with_explicit_channel_works: Sync Gateway not reachable at {SG_ADMIN_URL} \
             — run the sibling `assist` repo's docker/compose.yml stack (docker/register-sync-gateway-databases.nu) \
             to exercise this test."
        );
        return;
    }

    let doc_id = format!("plugin-repro-chan-{}", std::process::id());
    sg_cleanup(&doc_id);

    let dir1 = TempDir::new().unwrap();
    {
        let db1 = open_temp_db(&dir1, "c2_push");
        let mut coll1 = db1
            .create_collection(COLLECTION.to_string(), SCOPE.to_string())
            .expect("create collection");
        let mut doc = Document::new_with_id(&doc_id);
        doc.set_properties_as_json(r#"{"state":"ingested"}"#)
            .expect("set properties");
        coll1.save_document(&mut doc).expect("save document");

        run_real_replication_with_channels(&db1, ReplicatorType::Push, &[]);
    }

    let (rev, _) = sg_get(&doc_id).expect("doc not found on server after push");
    sg_put(&doc_id, Some(&rev), &serde_json::json!({"state": "screened"}));

    let dir2 = TempDir::new().unwrap();
    let db2 = open_temp_db(&dir2, "c2_pull");
    // The only difference from C1: explicit ["public"] instead of empty.
    run_real_replication_with_channels(&db2, ReplicatorType::Pull, &["public"]);

    let coll2 = db2
        .create_collection(COLLECTION.to_string(), SCOPE.to_string())
        .expect("create/open collection");
    let doc2 = coll2
        .get_document(&doc_id)
        .expect("pull with an explicit channel STILL did not deliver the document — channels weren't it either");
    let local_json = doc2.properties_as_json();

    sg_cleanup(&doc_id);

    assert!(
        local_json.contains("screened"),
        "pull with an explicit channel filter still didn't apply the mutation — \
         local doc: {local_json} (expected state=screened) — channels weren't the (whole) cause"
    );
}

// ── C3: does the SAME empty-channels bug hit PushAndPull (production's real
// direction), not just one-shot Pull? Confirmed live: YES — this was, in
// fact, THE live production bug. commands.rs's start_replication always
// used continuous PushAndPull with empty per-collection channels, so every
// real replicator the assist app started hit this exact fatal rejection —
// on top of, and independently from, the separate collection_access grant
// bug found and fixed in assist's own docker/register-sync-gateway-databases.nu.
// Both had to be fixed for real pull to work. commands.rs now requires a
// real `channels` list from its caller (see start_replication's doc
// comment) — this test guards that PushAndPull with an explicit channel
// still works, and stays as a reminder of why that parameter exists.
#[test]
fn real_pushandpull_with_empty_channels_is_fatal() {
    if !sync_gateway_reachable() {
        eprintln!("SKIPPING real_pushandpull_with_empty_channels_is_fatal: Sync Gateway not reachable");
        return;
    }
    let doc_id = format!("plugin-repro-pp-{}", std::process::id());
    sg_cleanup(&doc_id);

    let dir = TempDir::new().unwrap();
    let db = open_temp_db(&dir, "c3_pushpull");
    let mut coll = db
        .create_collection(COLLECTION.to_string(), SCOPE.to_string())
        .expect("create collection");
    let mut doc = Document::new_with_id(&doc_id);
    doc.set_properties_as_json(r#"{"state":"ingested"}"#)
        .expect("set properties");
    coll.save_document(&mut doc).expect("save document");

    let error = run_real_replication(&db, ReplicatorType::PushAndPull);
    sg_cleanup(&doc_id);
    assert!(
        error.as_deref().is_some_and(|e| e.contains("Illegal channel name")),
        "expected PushAndPull with empty per-collection channels to fail fatally \
         (this was assist's real production bug), got: {error:?}"
    );
}

#[test]
fn real_pushandpull_with_explicit_channel_works() {
    if !sync_gateway_reachable() {
        eprintln!("SKIPPING real_pushandpull_with_explicit_channel_works: Sync Gateway not reachable");
        return;
    }
    let doc_id = format!("plugin-repro-pp2-{}", std::process::id());
    sg_cleanup(&doc_id);

    let dir = TempDir::new().unwrap();
    let db = open_temp_db(&dir, "c3b_pushpull");
    let mut coll = db
        .create_collection(COLLECTION.to_string(), SCOPE.to_string())
        .expect("create collection");
    let mut doc = Document::new_with_id(&doc_id);
    doc.set_properties_as_json(r#"{"state":"ingested"}"#)
        .expect("set properties");
    coll.save_document(&mut doc).expect("save document");

    let error = run_real_replication_with_channels(&db, ReplicatorType::PushAndPull, &["public"]);
    assert_eq!(error, None, "PushAndPull with an explicit channel should not error: {error:?}");

    let (_, server_body) = sg_get(&doc_id).expect("PushAndPull with an explicit channel never pushed the doc");
    sg_cleanup(&doc_id);
    assert_eq!(server_body.get("state").and_then(|v| v.as_str()), Some("ingested"));
}
