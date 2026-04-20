/// Event emitted when documents in a collection change.
/// Payload: list of changed document IDs.
pub const COLLECTION_CHANGED: &str = "cblite://collection-changed";

/// Event emitted when the replicator status changes.
/// Payload: activity level string ("Idle", "Busy", "Connecting", "Stopped").
pub const REPLICATION_STATUS: &str = "cblite://replication-status";
