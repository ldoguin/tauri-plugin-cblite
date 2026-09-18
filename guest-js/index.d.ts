export declare const COLLECTION_CHANGED_EVENT = "cblite://collection-changed";
export declare const REPLICATION_STATUS_EVENT = "cblite://replication-status";
export declare function openDatabase(path: string, name: string, encryptionPassword?: string, collections?: string[]): Promise<void>;
export declare function closeDatabase(): Promise<void>;
export declare function getDocument(collection: string, docId: string): Promise<unknown>;
export declare function saveDocument(collection: string, docId: string, body: unknown, encryptedFields?: string[]): Promise<void>;
export declare function startReplication(url: string, collection: string, direction: "push" | "pull" | "both", auth?: {
    username: string;
    password: string;
} | {
    sessionId: string;
    cookieName?: string;
}, fieldEncryption?: {
    password: string;
    salt: string;
}, extraCollections?: string[], channels?: string[], label?: string): Promise<void>;
/** `label` defaults to `"default"`, matching `startReplication`. */
export declare function stopReplication(label?: string): Promise<void>;
export declare function executeQuery(language: "N1QL" | "JSON", queryStr: string, parameters?: Record<string, unknown>): Promise<unknown[]>;
/**
 * Create (or idempotently ensure) a full-text search index on a collection field.
 * Safe to call on every app start — CBL is a no-op if the identical index exists.
 */
export declare function createFtsIndex(collection: string, indexName: string, field: string): Promise<void>;
export declare function listIndexes(collection: string): Promise<string[]>;
/**
 * Register a predictive model for use in PREDICTION() queries.
 */
export declare function registerPredictiveModel(name: string, options?: {
    onnxPath?: string;
    inputField?: string;
    outputField?: string;
}): Promise<void>;
/** Unregister a previously registered predictive model. */
export declare function unregisterPredictiveModel(name: string): Promise<void>;
/**
 * Save binary data as a CBL blob. `dataB64` is the base64-encoded content.
 * Returns the blob's digest string (e.g. "sha1-abc123...") for later retrieval.
 */
export declare function saveBlob(dataB64: string, contentType: string): Promise<string>;
/**
 * Retrieve blob bytes by digest. Returns base64-encoded content.
 */
export declare function getBlobData(digest: string): Promise<string>;
/**
 * Write a text file to user-accessible storage (~/Downloads or $HOME on
 * desktop, the app's external files dir on Android). Returns the absolute
 * path of the written file.
 */
export declare function writeExportFile(filename: string, data: string): Promise<string>;
export declare function onCollectionChanged(handler: (docIds: string[]) => void): Promise<() => void>;
/**
 * `replicator` (4th argument) is the label passed to `startReplication`
 * (`"default"` if none was given) — added so a device running an uplink and
 * a peer replicator at once can tell which one just changed. Appended after
 * the original two arguments rather than inserted before them, so an
 * existing `(activity, error) => ...` handler keeps compiling and working
 * unchanged; it simply never looks at the 3rd argument.
 */
export declare function onReplicationStatus(handler: (activity: string, error?: string, replicator?: string) => void): Promise<() => void>;
export interface PeerListenerInfo {
    /** The port actually bound; asking for 0 lets the OS choose. */
    port: number;
    /** Every address a peer could dial, as ws:// URLs. */
    urls: string[];
}
export interface PeerConnectionStatus {
    listening: boolean;
    port: number;
    connections: number;
    /** serde serialises the Rust field as active_connections. */
    active_connections: number;
    urls: string[];
}
/** Start accepting replication connections from other Couchbase Lite instances. */
export declare function startPeerListener(collections: string[], port?: number, readOnly?: boolean): Promise<PeerListenerInfo>;
export declare function stopPeerListener(): Promise<void>;
export declare function peerListenerStatus(): Promise<PeerConnectionStatus>;
/**
 * Whether this build can host peers.
 *
 * A Community build has no peer commands at all, so the invoke rejects. Asking
 * once and remembering is kinder than letting a replication quietly never
 * connect.
 */
export declare function peerSupported(): Promise<boolean>;
