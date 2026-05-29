/// Mobile command handlers — delegate to the Kotlin plugin via `MobileCblite`.
///
/// All Kotlin commands return JSObject; string values are wrapped as
/// `{ "value": "..." }`.  Object/array results are wrapped as
/// `{ "rows": [...] }` (for queries) or returned directly (for documents).
use serde::Deserialize;
use serde_json::Value;
use tauri::{AppHandle, Runtime, State};

use crate::mobile::MobileCblite;

type MobileState<'a, R> = State<'a, MobileCblite<R>>;

// Helper: single-string response from Kotlin (`{ "value": "..." }`)
#[derive(Deserialize)]
struct ValuePayload {
    value: String,
}

// Helper: query response from Kotlin (`{ "rows": [...] }`)
#[derive(Deserialize)]
struct RowsPayload {
    rows: Vec<Value>,
}

#[tauri::command]
pub async fn open_database<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    path: String,
    name: String,
    encryption_password: Option<String>,
    collections: Option<Vec<String>>,
) -> Result<(), String> {
    mobile
        .run::<_, ()>(
            "openDatabase",
            &serde_json::json!({
                "path": path,
                "name": name,
                "encryptionPassword": encryption_password,
                "collections": collections,
            }),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn close_database<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
) -> Result<(), String> {
    mobile
        .run::<_, ()>("closeDatabase", &serde_json::json!({}))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_document<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    collection: String,
    doc_id: String,
) -> Result<Value, String> {
    mobile
        .run::<_, Value>(
            "getDocument",
            &serde_json::json!({ "collection": collection, "docId": doc_id }),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_document<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    collection: String,
    doc_id: String,
    body: Value,
    encrypted_fields: Option<Vec<String>>,
) -> Result<(), String> {
    mobile
        .run::<_, ()>(
            "saveDocument",
            &serde_json::json!({
                "collection": collection,
                "docId": doc_id,
                "body": body,
                "encryptedFields": encrypted_fields,
            }),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_replication<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
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
    mobile
        .run::<_, ()>(
            "startReplication",
            &serde_json::json!({
                "url": url,
                "collection": collection,
                "direction": direction,
                "username": username,
                "password": password,
                "sessionId": session_id,
                "cookieName": cookie_name,
                "fieldEncryptionPassword": field_encryption_password,
                "fieldEncryptionSalt": field_encryption_salt,
            }),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_replication<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
) -> Result<(), String> {
    mobile
        .run::<_, ()>("stopReplication", &serde_json::json!({}))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn execute_query<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    language: String,
    query_str: String,
    parameters: Option<Value>,
) -> Result<Vec<Value>, String> {
    let payload: RowsPayload = mobile
        .run(
            "executeQuery",
            &serde_json::json!({
                "language": language,
                "queryStr": query_str,
                "parameters": parameters,
            }),
        )
        .map_err(|e| e.to_string())?;
    Ok(payload.rows)
}

#[tauri::command]
pub async fn create_fts_index<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    collection: String,
    index_name: String,
    field: String,
) -> Result<(), String> {
    mobile
        .run::<_, ()>(
            "createFtsIndex",
            &serde_json::json!({
                "collection": collection,
                "indexName": index_name,
                "field": field,
            }),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_blob<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    data_b64: String,
    content_type: String,
) -> Result<String, String> {
    let p: ValuePayload = mobile
        .run(
            "saveBlob",
            &serde_json::json!({ "dataB64": data_b64, "contentType": content_type }),
        )
        .map_err(|e| e.to_string())?;
    Ok(p.value)
}

#[tauri::command]
pub async fn get_blob_data<R: Runtime>(
    _app: AppHandle<R>,
    mobile: MobileState<'_, R>,
    digest: String,
) -> Result<String, String> {
    let p: ValuePayload = mobile
        .run("getBlobData", &serde_json::json!({ "digest": digest }))
        .map_err(|e| e.to_string())?;
    Ok(p.value)
}

#[tauri::command]
pub async fn register_predictive_model<R: Runtime>(
    _app: AppHandle<R>,
    _mobile: MobileState<'_, R>,
    _name: String,
    _onnx_path: Option<String>,
    _input_field: Option<String>,
    _output_field: Option<String>,
) -> Result<(), String> {
    Err("Predictive models are not supported on Android".into())
}

#[tauri::command]
pub async fn unregister_predictive_model<R: Runtime>(
    _app: AppHandle<R>,
    _mobile: MobileState<'_, R>,
    _name: String,
) -> Result<(), String> {
    Err("Predictive models are not supported on Android".into())
}
