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
channels) {
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
    });
}
export function stopReplication() {
    return invoke("plugin:cblite|stop_replication");
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
export function onReplicationStatus(handler) {
    if (isMobile()) {
        // Android: plugin events go through Tauri Channels.
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
    // Desktop: emitted via Rust app_handle.emit()
    return listen(REPLICATION_STATUS_EVENT, (event) => {
        handler(event.payload);
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
