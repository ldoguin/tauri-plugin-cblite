use crate::{events, PluginState};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime, State};

type PluginStateArc = Arc<Mutex<Option<PluginState>>>;

#[tauri::command]
pub async fn open_database<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    path: String,
    name: String,
    encryption_password: Option<String>,
    collections: Option<Vec<String>>,
) -> Result<(), String> {
    use couchbase_lite::{collection::CollectionChangeListener, Database, DatabaseConfiguration};
    #[cfg(feature = "enterprise")]
    use couchbase_lite::{EncryptionAlgorithm, EncryptionKey};
    use std::path::Path;

    let dir = Path::new(&path);

    #[cfg(feature = "enterprise")]
    let encryption_key: Option<EncryptionKey> = encryption_password
        .as_deref()
        .filter(|p| !p.is_empty())
        .and_then(|p| EncryptionKey::new_from_password(EncryptionAlgorithm::AES256, p));

    let config = DatabaseConfiguration {
        directory: dir,
        #[cfg(feature = "enterprise")]
        encryption_key,
    };
    let db = Database::open(&name, Some(config)).map_err(|e| e.to_string())?;

    // For each requested collection: ensure it exists and register a change
    // listener so the frontend receives cblite://collection-changed events.
    let mut coll_listeners = vec![];
    for coll_spec in collections.unwrap_or_default() {
        let (scope_name, coll_name) = if let Some((s, c)) = coll_spec.split_once('.') {
            (s.to_string(), c.to_string())
        } else {
            ("_default".to_string(), coll_spec)
        };
        let mut coll = db
            .create_collection(coll_name, scope_name)
            .map_err(|e| e.to_string())?;
        let app_handle = app.clone();
        let listener = coll.add_listener(Box::new(move |_coll, doc_ids| {
            let _ = app_handle.emit(events::COLLECTION_CHANGED, doc_ids);
        }) as CollectionChangeListener);
        coll_listeners.push(listener);
    }

    let mut guard = state.lock().map_err(|e| e.to_string())?;
    *guard = Some(PluginState {
        db,
        listeners: vec![],
        coll_listeners,
        replicator: None,
    });
    Ok(())
}

#[tauri::command]
pub async fn close_database<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if let Some(plugin_state) = guard.take() {
        drop(plugin_state.listeners);
        drop(plugin_state.replicator);
        plugin_state.db.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_document<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    collection: String,
    doc_id: String,
) -> Result<Value, String> {
    let guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_ref().ok_or("Database not open")?;

    let (scope_name, coll_name) = parse_collection(&collection);
    let coll = plugin_state
        .db
        .create_collection(coll_name.to_string(), scope_name.to_string())
        .map_err(|e| e.to_string())?;

    let doc = coll.get_document(&doc_id).map_err(|e| e.to_string())?;
    let json_str = doc.properties_as_json();
    serde_json::from_str(&json_str).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_document<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    collection: String,
    doc_id: String,
    body: Value,
    encrypted_fields: Option<Vec<String>>,
) -> Result<(), String> {
    use couchbase_lite::Document;

    let mut guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_mut().ok_or("Database not open")?;

    let (scope_name, coll_name) = parse_collection(&collection);
    let mut coll = plugin_state
        .db
        .create_collection(coll_name.to_string(), scope_name.to_string())
        .map_err(|e| e.to_string())?;

    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let mut doc = Document::new_with_id(&doc_id);
    doc.set_properties_as_json(&json)
        .map_err(|e| e.to_string())?;

    // Wrap specified string fields as CBL Encryptable values so the replicator
    // encrypts them before pushing to the server.
    #[cfg(feature = "enterprise")]
    if let Some(fields) = &encrypted_fields {
        use couchbase_lite::encryptable::Encryptable;
        let mut props = doc.mutable_properties();
        for field in fields {
            if let Some(val) = body.get(field).and_then(|v| v.as_str()) {
                let enc = Encryptable::create_with_string(val);
                props.at(field).put_encrypt(&enc);
            }
        }
    }

    coll.save_document(&mut doc).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn start_replication<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    url: String,
    collection: String,
    direction: String,
    username: Option<String>,
    password: Option<String>,
    session_id: Option<String>,
    cookie_name: Option<String>,
    field_encryption_password: Option<String>,
    field_encryption_salt: Option<String>,
) -> Result<(), String> {
    use couchbase_lite::{
        Authenticator, Endpoint, MutableArray, ReplicationCollection,
        ReplicationConfigurationContext, Replicator, ReplicatorConfiguration, ReplicatorType,
    };
    use log::{debug, info, error};

    debug!("Starting replication to URL: {}", url);
    debug!("Collection: {}", collection);
    debug!("Direction: {}", direction);
    debug!("Username: {:?}", username);
    debug!("Session ID: {:?}", session_id);
    debug!("Field encryption password: {:?}", field_encryption_password);
    debug!("Field encryption salt: {:?}", field_encryption_salt);

    let mut guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_mut().ok_or("Database not open")?;

    let (scope_name, coll_name) = parse_collection(&collection);
    let coll = plugin_state
        .db
        .create_collection(coll_name.to_string(), scope_name.to_string())
        .map_err(|e| e.to_string())?;

    let endpoint = Endpoint::new_with_url(&url).map_err(|e| e.to_string())?;
    debug!("Created endpoint for URL: {}", url);

    let repl_type = match direction.as_str() {
        "push" => {
            debug!("Replication direction: push");
            ReplicatorType::Push
        }
        "pull" => {
            debug!("Replication direction: pull");
            ReplicatorType::Pull
        }
        _ => {
            debug!("Replication direction: push and pull");
            ReplicatorType::PushAndPull
        }
    };

// Prefer session auth (SG session cookie) over basic auth
    let authenticator = match session_id.as_deref().filter(|s| !s.is_empty()) {
        Some(sid) => {
            let cookie = cookie_name.as_deref().unwrap_or("SyncGatewaySession");
            debug!("Using session authentication with session ID: {}", sid);
            Some(Authenticator::create_session(sid, cookie))
        }
        _ => match (username.as_deref(), password.as_deref()) {
            (Some(u), Some(p)) if !u.is_empty() => {
                debug!("Using basic authentication with username: {}", u);
                Some(Authenticator::create_password(u, p))
            }
            _ => {
                debug!("No authentication configured");
                None
            }
        },
    };

    // Empty channels = no filter = "all channels the user has access to" (per SG).
    // channels: ["*"] requests the SG star channel explicitly, which requires
    // admin_channels:["*"]. Our users only have user.{username}, so ["*"] returns nothing.
    // GLOBAL channels ARE NOT IMPLEMENTED. ONLY PER-COLLECTION channels work.
    // This has been verified in the replicator binding source code at line 771.

    // Replicate the requested collection only
    // No hardcoded collection names in this plugin
    let mut collections = Vec::new();

    collections.push(ReplicationCollection {
        collection: coll,
        conflict_resolver: None,
        push_filter: None,
        pull_filter: None,
        channels: MutableArray::default(), // empty = pull from all user-accessible channels
        document_ids: MutableArray::default(),
    });
    
    // Required for Sync Gateway: accept cookies from parent domain
    let mut headers = std::collections::HashMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    
    let config = ReplicatorConfiguration {
        database: None,
        endpoint,
        replicator_type: repl_type.clone(),
        continuous: true,
        disable_auto_purge: true,
        max_attempts: 5,
        max_attempt_wait_time: 10,
        heartbeat: 30,
        authenticator,
        proxy: None,
        headers,
        pinned_server_certificate: None,
        trusted_root_certificates: None,
        channels: MutableArray::default(), // GLOBAL channels ARE NOT IMPLEMENTED! DO NOT USE. Only PER-COLLECTION channels work.
        document_ids: MutableArray::default(),
        collections: Some(collections),
        accept_parent_domain_cookies: true,
        #[cfg(feature = "enterprise")]
        accept_only_self_signed_server_certificate: false,
    };

    debug!("Created replicator configuration with endpoint: {}", url);
    debug!("Replication type: {:?}", repl_type);
    debug!("Collections configured: {} collection(s)", config.collections.as_ref().map_or(0, |c| c.len()));

    let mut context = Box::new(ReplicationConfigurationContext::default());

    // Derive AES-256-GCM field-encryption key from password + base64 salt via PBKDF2-SHA256.
    #[cfg(feature = "enterprise")]
    if let (Some(pass), Some(salt_b64)) = (
        field_encryption_password.as_deref().filter(|s| !s.is_empty()),
        field_encryption_salt.as_deref().filter(|s| !s.is_empty()),
    ) {
        use base64::{Engine, engine::general_purpose};
        use hmac::Hmac;
        use pbkdf2::pbkdf2;
        use sha2::Sha256;

        let salt = general_purpose::STANDARD
            .decode(salt_b64)
            .map_err(|e| format!("invalid salt: {e}"))?;
        let mut key = [0u8; 32];
        pbkdf2::<Hmac<Sha256>>(pass.as_bytes(), &salt, 100_000, &mut key)
            .map_err(|e| format!("pbkdf2 error: {e}"))?;
        context.field_encryption_key = Some(key);
        debug!("Field encryption key derived successfully");
    }

    let app_handle = app.clone();
    let mut replicator = Replicator::new(config, context)
        .map_err(|e| {
            error!("Failed to create replicator: {}", e);
            e.to_string()
        })?
        .add_change_listener(Box::new(move |status| {
            use couchbase_lite::ReplicatorActivityLevel;
            let label = match status.activity {
                ReplicatorActivityLevel::Stopped => {
                    info!("Replication stopped");
                    "Stopped"
                }
                ReplicatorActivityLevel::Offline => {
                    info!("Replication offline");
                    "Offline"
                }
                ReplicatorActivityLevel::Connecting => {
                    info!("Replication connecting");
                    "Connecting"
                }
                ReplicatorActivityLevel::Idle => {
                    info!("Replication idle");
                    "Idle"
                }
                ReplicatorActivityLevel::Busy => {
                    info!("Replication busy");
                    "Busy"
                }
            };
            let _ = app_handle.emit(events::REPLICATION_STATUS, label);
        }));
    replicator.start(false);
    info!("Replication started successfully");
    plugin_state.replicator = Some(replicator);
    Ok(())
}

#[tauri::command]
pub async fn stop_replication<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    if let Some(plugin_state) = guard.as_mut() {
        if let Some(mut replicator) = plugin_state.replicator.take() {
            replicator.stop(None);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn execute_query<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    language: String,
    query_str: String,
    parameters: Option<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    use couchbase_lite::{FleeceReference, MutableDict, Query, QueryLanguage};

    let guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_ref().ok_or("Database not open")?;

    let lang = if language.eq_ignore_ascii_case("json") {
        QueryLanguage::JSON
    } else {
        QueryLanguage::N1QL
    };

    log::debug!("[execute_query] lang={language} query={query_str:?}");

    let query = Query::new(&plugin_state.db, lang, &query_str)
        .map_err(|e| { log::error!("[execute_query] compile error: {e}"); e.to_string() })?;

    if let Some(explain) = query.explain().ok() {
        log::debug!("[execute_query] explain:\n{explain}");
    }

    if let Some(serde_json::Value::Object(map)) = parameters {
        let mut params = MutableDict::new();
        for (key, val) in &map {
            log::debug!("[execute_query] param ${key} = {val}");
            match val {
                serde_json::Value::String(s) => { params.at(key).put_string(s); }
                serde_json::Value::Number(n)  => { params.at(key).put_f64(n.as_f64().unwrap_or(0.0)); }
                serde_json::Value::Bool(b)    => { params.at(key).put_bool(*b); }
                _                             => { params.at(key).put_null(); }
            }
        }
        query.set_parameters(&params);
    }

    let result_set = query.execute()
        .map_err(|e| { log::error!("[execute_query] execute error: {e}"); e.to_string() })?;
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for row in result_set {
        let json_str = row.as_dict().to_json();
        log::debug!("[execute_query] row: {json_str}");
        match serde_json::from_str(&json_str) {
            Ok(v) => rows.push(v),
            Err(e) => log::warn!("[execute_query] failed to parse row JSON: {e} — raw: {json_str}"),
        }
    }
    log::debug!("[execute_query] total rows returned: {}", rows.len());
    Ok(rows)
}

#[tauri::command]
pub async fn save_blob<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    data_b64: String,
    content_type: String,
) -> Result<String, String> {
    use base64::{Engine, engine::general_purpose};
    use couchbase_lite::Blob;

    let data = general_purpose::STANDARD
        .decode(&data_b64)
        .map_err(|e| format!("base64 decode error: {e}"))?;

    let mut guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_mut().ok_or("Database not open")?;

    let mut blob = Blob::new_from_data(&data, &content_type);
    blob.save_to_database(&plugin_state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_blob_data<R: Runtime>(
    _app: AppHandle<R>,
    state: State<'_, PluginStateArc>,
    digest: String,
) -> Result<String, String> {
    use base64::{Engine, engine::general_purpose};
    use couchbase_lite::Blob;

    let guard = state.lock().map_err(|e| e.to_string())?;
    let plugin_state = guard.as_ref().ok_or("Database not open")?;

    let blob = Blob::get_from_database(&plugin_state.db, &digest)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Blob not found: {digest}"))?;

    let bytes = blob.load_content().map_err(|e| e.to_string())?;
    Ok(general_purpose::STANDARD.encode(&bytes))
}

fn parse_collection(s: &str) -> (&str, &str) {
    if let Some((scope, coll)) = s.split_once('.') {
        (scope, coll)
    } else {
        ("_default", s)
    }
}

// ── Predictive model commands ─────────────────────────────────────────────────

/// Tracks names of registered predictive models so they can be unregistered.
pub type PredictionStateArc = Arc<Mutex<std::collections::HashSet<String>>>;

/// ONNX-backed predictive model using tract.
///
/// Input: the dict passed to `PREDICTION()` must contain a key matching
/// `input_field` (default `"input"`) whose value is a JSON array of f32.
/// Output: first output element exposed as `"score"`; full array as `output_field`.
#[cfg(feature = "enterprise")]
struct OnnxModel {
    plan: std::sync::Arc<tract_onnx::prelude::TypedRunnableModel<tract_onnx::prelude::TypedModel>>,
    input_field: String,
    output_field: String,
}

#[cfg(feature = "enterprise")]
impl couchbase_lite::PredictiveModel for OnnxModel {
    fn predict(
        &self,
        input: couchbase_lite::Dict,
    ) -> Option<couchbase_lite::MutableDict> {
        use couchbase_lite::FleeceReference;
        use tract_onnx::prelude::*;

        let json = input.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
        let field = parsed.get(&self.input_field)?;

        // Nu/JS lists may be stored as JSON strings in Fleece
        let floats: Vec<f32> = if let Some(arr) = field.as_array() {
            arr.iter().filter_map(|v| v.as_f64().map(|f| f as f32)).collect()
        } else if let Some(s) = field.as_str() {
            let inner: serde_json::Value = serde_json::from_str(s).ok()?;
            inner.as_array()?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        } else {
            return None;
        };

        if floats.is_empty() {
            return None;
        }

        let len = floats.len();
        let tensor = tract_ndarray::Array2::from_shape_vec((1, len), floats)
            .ok()?
            .into_tensor()
            .into_tvalue();

        let outputs = self.plan.run(tvec![tensor]).ok()?;
        let out = outputs.into_iter().next()?.into_tensor();
        let output_floats: Vec<f64> = out
            .to_array_view::<f32>()
            .ok()?
            .iter()
            .map(|&f| f as f64)
            .collect();

        let mut dict = couchbase_lite::MutableDict::new();
        let json_out = serde_json::to_string(&output_floats).unwrap_or_default();
        dict.at(&self.output_field).put_string(&json_out);
        if let Some(&first) = output_floats.first() {
            dict.at("score").put_f64(first);
        }
        Some(dict)
    }
}

/// Register a predictive model by name.
///
/// Without `onnx_path` registers a stub that always returns `{score: 1.0}`.
/// With `onnx_path` loads the ONNX file via tract and runs real inference.
#[tauri::command]
pub async fn register_predictive_model<R: Runtime>(
    _app: AppHandle<R>,
    pred_state: State<'_, PredictionStateArc>,
    name: String,
    onnx_path: Option<String>,
    input_field: Option<String>,
    output_field: Option<String>,
) -> Result<(), String> {
    #[cfg(feature = "enterprise")]
    {
        use couchbase_lite::{register_predictive_model, MutableDict, PredictiveModel, Dict};

        let input_field  = input_field.unwrap_or_else(|| "input".into());
        let output_field = output_field.unwrap_or_else(|| "output".into());

        match onnx_path {
            Some(path) => {
                use tract_onnx::prelude::*;
                let plan = tract_onnx::onnx()
                    .model_for_path(&path)
                    .map_err(|e| format!("Failed to load ONNX model: {e}"))?
                    .into_optimized()
                    .map_err(|e| format!("Failed to optimise ONNX model: {e}"))?
                    .into_runnable()
                    .map_err(|e| format!("Failed to compile ONNX model: {e}"))?;

                register_predictive_model(&name, OnnxModel {
                    plan: std::sync::Arc::new(plan),
                    input_field,
                    output_field,
                });
            }
            None => {
                struct FixedScoreModel;
                impl PredictiveModel for FixedScoreModel {
                    fn predict(&self, _input: Dict) -> Option<MutableDict> {
                        let mut out = MutableDict::new();
                        out.at("score").put_f64(1.0);
                        Some(out)
                    }
                }
                register_predictive_model(&name, FixedScoreModel);
            }
        }

        pred_state.lock().map_err(|e| e.to_string())?.insert(name);
        return Ok(());
    }

    #[cfg(not(feature = "enterprise"))]
    Err("Predictive queries require the enterprise feature".into())
}

/// Unregister a previously registered predictive model.
#[tauri::command]
pub async fn unregister_predictive_model<R: Runtime>(
    _app: AppHandle<R>,
    pred_state: State<'_, PredictionStateArc>,
    name: String,
) -> Result<(), String> {
    #[cfg(feature = "enterprise")]
    {
        couchbase_lite::unregister_predictive_model(&name);
        pred_state.lock().map_err(|e| e.to_string())?.remove(&name);
        return Ok(());
    }

    #[cfg(not(feature = "enterprise"))]
    Err("Predictive queries require the enterprise feature".into())
}
