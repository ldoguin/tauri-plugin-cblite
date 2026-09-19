/// Real integration test for the non-lazy vector index path — the exact
/// shape `commands::create_vector_index` uses (`VectorIndexConfiguration::new`,
/// no `IndexUpdater`), as opposed to couchbase-lite-rust's own test of the
/// lazy-index path.
///
/// Requires the real Couchbase Lite Vector Search extension, downloaded
/// separately (not vendored — see commands.rs's `enable_vector_search` doc
/// comment). Skips, rather than fails, if it isn't available at
/// `CBLITE_VECTOR_SEARCH_PATH` — same convention couchbase-lite-rust's own
/// vector index tests use.
use couchbase_lite::{
    enable_vector_search, Database, DatabaseConfiguration, Document, MutableArray, Query,
    QueryLanguage, VectorIndexConfiguration,
};
use tempfile::TempDir;

#[test]
#[cfg(feature = "enterprise")]
fn non_lazy_vector_index_matches_create_vector_index_command() {
    let ext_path = std::env::var("CBLITE_VECTOR_SEARCH_PATH").unwrap_or_default();
    if let Err(e) = enable_vector_search(&ext_path) {
        eprintln!("SKIP: vector search extension not available: {e}");
        return;
    }

    let dir = TempDir::new().unwrap();
    let config = DatabaseConfiguration {
        directory: dir.path(),
        encryption_key: None,
    };
    let db = Database::open("vs_test", Some(config)).expect("open database");
    let mut coll = db.default_collection_or_error().unwrap();

    // Documents carry the vector as a plain field — the pattern every
    // tauri-* app in kubecondemo writes via save_document, and the reason
    // create_vector_index's config is non-lazy: no IndexUpdater step, CBL
    // reads "vector" directly off each document.
    let vectors: &[(&str, [f32; 4])] = &[
        ("doc_0", [1.0, 0.0, 0.0, 0.0]),
        ("doc_1", [0.0, 1.0, 0.0, 0.0]),
        ("doc_2", [0.0, 0.0, 1.0, 0.0]),
    ];
    for (id, v) in vectors {
        let mut doc = Document::new_with_id(id);
        let mut arr = MutableArray::new();
        for x in v.iter() {
            arr.append().put_f64(*x as f64);
        }
        doc.mutable_properties().at("vector").put_value(&arr);
        coll.save_document(&mut doc).unwrap();
    }

    // Exactly commands::create_vector_index's config: VectorIndexConfiguration::new,
    // no lazy override.
    let config = VectorIndexConfiguration::new("vector", 4, 2);
    coll.create_vector_index("vec_idx", &config)
        .expect("create vector index");

    let query = Query::new(
        &db,
        QueryLanguage::N1QL,
        "SELECT META().id FROM _default._default \
         ORDER BY APPROX_VECTOR_DISTANCE(vector, [1,0,0,0]) LIMIT 1",
    )
    .expect("create query");
    let mut results = query.execute().expect("execute query");
    let row = results.next().expect("expected at least one result row");
    let id = row.get(0).as_string().unwrap_or_default().to_string();
    assert_eq!(id, "doc_0", "nearest neighbour to [1,0,0,0] should be doc_0");
}
