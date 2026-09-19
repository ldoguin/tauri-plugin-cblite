/// Event emitted when documents in a collection change.
/// Payload: list of changed document IDs.
pub const COLLECTION_CHANGED: &str = "cblite://collection-changed";

/// Event emitted when a replicator's status changes.
/// Payload: `{ replicator: string, activity: "Idle" | "Busy" | "Connecting" | "Offline" | "Stopped", error: string | null }`.
/// `replicator` is the label passed to `start_replication` (`"default"` if
/// none was given), so a device running an uplink and a peer replicator at
/// once can tell which one just changed. `error` is the underlying
/// LiteCore/network error when the activity change was caused by one (e.g.
/// a `Stopped` following a failed handshake looks identical to a
/// deliberate `stop_replication` without this).
pub const REPLICATION_STATUS: &str = "cblite://replication-status";
