// Tauri v2 plugin guest API
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export const COLLECTION_CHANGED_EVENT = "cblite://collection-changed";
export const REPLICATION_STATUS_EVENT = "cblite://replication-status";
/**
 * True when running on Android/iOS. On mobile, plugin events come through
 * Tauri Channels (registerListener) rather than the global event bus (listen).
 */
function isMobile() {
    return /android|iphone|ipad|ipod/i.test(navigator.userAgent);
}
export function openDatabase(path, name, encryptionPassword, collections) {
    return invoke("plugin:cblite|open_database", {
        path,
        name,
        encryptionPassword: encryptionPassword ?? null,
        collections: collections ?? null,
    });
}
export function closeDatabase() {
    return invoke("plugin:cblite|close_database");
}
export function getDocument(collection, docId) {
    return invoke("plugin:cblite|get_document", {
        collection,
        docId,
    });
}
export function saveDocument(collection, docId, body, encryptedFields) {
    return invoke("plugin:cblite|save_document", {
        collection,
        docId,
        body,
        encryptedFields: encryptedFields ?? null,
    });
}
export function startReplication(url, collection, direction, auth, fieldEncryption, extraCollections, 
// Sync Gateway channel filter applied to every collection in this
// replicator. REQUIRED in practice: an empty/omitted list is NOT "no
// filter" — confirmed live, Sync Gateway rejects it outright with a
// fatal `400 Illegal channel name ""` that kills the whole replicator
// (push included). Pass every channel the authenticated user needs.
channels, 
// Distinguishes this replicator from any others running on the same
// database at once — e.g. `"uplink"` (a server) and `"peer"` (a direct
// peer-to-peer leg). Defaults to `"default"`: starting a second
// replication with the same (or no) label stops and replaces the first,
// same as before this parameter existed. Pass distinct labels to run
// more than one replicator concurrently.
label) {
    const isSession = auth && "sessionId" in auth;
    return invoke("plugin:cblite|start_replication", {
        url,
        collection,
        direction,
        username: !isSession && auth ? auth.username : null,
        password: !isSession && auth ? auth.password : null,
        sessionId: isSession ? auth.sessionId : null,
        cookieName: isSession ? (auth.cookieName ?? null) : null,
        fieldEncryptionPassword: fieldEncryption?.password ?? null,
        fieldEncryptionSalt: fieldEncryption?.salt ?? null,
        extraCollections: extraCollections ?? null,
        channels: channels ?? null,
        label: label ?? null,
    });
}
/** `label` defaults to `"default"`, matching `startReplication`. */
export function stopReplication(label) {
    return invoke("plugin:cblite|stop_replication", { label: label ?? null });
}
export function executeQuery(language, queryStr, parameters) {
    return invoke("plugin:cblite|execute_query", {
        language,
        queryStr,
        parameters: parameters ?? null,
    });
}
/**
 * Create (or idempotently ensure) a full-text search index on a collection field.
 * Safe to call on every app start — CBL is a no-op if the identical index exists.
 */
export function createFtsIndex(collection, indexName, field) {
    return invoke("plugin:cblite|create_fts_index", { collection, indexName, field });
}
export function listIndexes(collection) {
    return invoke("plugin:cblite|list_indexes", { collection });
}
/**
 * Loads the Vector Search extension. Enterprise only, and must be called
 * before `openDatabase` — Couchbase Lite loads it once, globally.
 * `extensionPath` is the directory containing the platform's real
 * extension library (e.g. `CouchbaseLiteVectorSearch.so` on Linux),
 * downloaded separately — see
 * https://docs.couchbase.com/couchbase-lite/current/c/gs-downloads.html.
 */
export function enableVectorSearch(extensionPath) {
    return invoke("plugin:cblite|enable_vector_search", { extensionPath });
}
/**
 * Create (or idempotently replace) a vector index on a collection field.
 * `expression` is an N1QL expression evaluating to the vector array per
 * document (e.g. `"vector"` for a plain top-level field); `dimensions` is
 * the vector length; `centroids` is recommended as `sqrt(number of
 * vectors)`, any positive number works for a small dataset.
 */
export function createVectorIndex(collection, indexName, expression, dimensions, centroids) {
    return invoke("plugin:cblite|create_vector_index", {
        collection,
        indexName,
        expression,
        dimensions,
        centroids,
    });
}
/**
 * Register a predictive model for use in PREDICTION() queries.
 */
export function registerPredictiveModel(name, options) {
    return invoke("plugin:cblite|register_predictive_model", {
        name,
        onnxPath: options?.onnxPath ?? null,
        inputField: options?.inputField ?? null,
        outputField: options?.outputField ?? null,
    });
}
/** Unregister a previously registered predictive model. */
export function unregisterPredictiveModel(name) {
    return invoke("plugin:cblite|unregister_predictive_model", { name });
}
/**
 * Save binary data as a CBL blob. `dataB64` is the base64-encoded content.
 * Returns the blob's digest string (e.g. "sha1-abc123...") for later retrieval.
 */
export function saveBlob(dataB64, contentType) {
    return invoke("plugin:cblite|save_blob", { dataB64, contentType });
}
/**
 * Retrieve blob bytes by digest. Returns base64-encoded content.
 */
export function getBlobData(digest) {
    return invoke("plugin:cblite|get_blob_data", { digest });
}
/**
 * Write a text file to user-accessible storage (~/Downloads or $HOME on
 * desktop, the app's external files dir on Android). Returns the absolute
 * path of the written file.
 */
export function writeExportFile(filename, data) {
    return invoke("plugin:cblite|write_export_file", { filename, data });
}
export function onCollectionChanged(handler) {
    if (isMobile()) {
        // Android: plugin events go through Tauri Channels, not the global event bus.
        const ch = new Channel();
        ch.onmessage = (payload) => handler(payload.docIds ?? []);
        return invoke("plugin:cblite|registerListener", {
            event: COLLECTION_CHANGED_EVENT,
            handler: ch,
        }).then(() => () => {
            invoke("plugin:cblite|removeListener", {
                event: COLLECTION_CHANGED_EVENT,
                channelId: ch.id,
            }).catch(() => { });
        });
    }
    // Desktop: emitted via Rust app_handle.emit()
    return listen(COLLECTION_CHANGED_EVENT, (event) => {
        handler(event.payload);
    });
}
/**
 * `replicator` (4th argument) is the label passed to `startReplication`
 * (`"default"` if none was given) — added so a device running an uplink and
 * a peer replicator at once can tell which one just changed. Appended after
 * the original two arguments rather than inserted before them, so an
 * existing `(activity, error) => ...` handler keeps compiling and working
 * unchanged; it simply never looks at the 3rd argument.
 */
export function onReplicationStatus(handler) {
    if (isMobile()) {
        // Android: plugin events go through Tauri Channels. Mobile does not yet
        // have multiple concurrent replicators, so there is no label to report.
        const ch = new Channel();
        ch.onmessage = (payload) => handler(payload.activity ?? "", payload.error);
        return invoke("plugin:cblite|registerListener", {
            event: REPLICATION_STATUS_EVENT,
            handler: ch,
        }).then(() => () => {
            invoke("plugin:cblite|removeListener", {
                event: REPLICATION_STATUS_EVENT,
                channelId: ch.id,
            }).catch(() => { });
        });
    }
    // Desktop: emitted via Rust app_handle.emit() as { replicator, activity }.
    return listen(REPLICATION_STATUS_EVENT, (event) => {
        handler(event.payload.activity, event.payload.error ?? undefined, event.payload.replicator);
    });
}
/** Start accepting replication connections from other Couchbase Lite instances. */
export function startPeerListener(collections, port, readOnly) {
    return invoke("plugin:cblite|start_peer_listener", {
        collections,
        port: port ?? 0,
        readOnly: readOnly ?? false,
    });
}
export function stopPeerListener() {
    return invoke("plugin:cblite|stop_peer_listener");
}
export function peerListenerStatus() {
    return invoke("plugin:cblite|peer_listener_status");
}
/**
 * Whether this build can host peers.
 *
 * A Community build has no peer commands at all, so the invoke rejects. Asking
 * once and remembering is kinder than letting a replication quietly never
 * connect.
 */
export async function peerSupported() {
    try {
        await peerListenerStatus();
        return true;
    }
    catch {
        return false;
    }
}
