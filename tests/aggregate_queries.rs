/// Real integration test for the aggregate-query shapes callers run through
/// `execute_query` to compute on-device summaries: COUNT/SUM/AVG over a
/// collection, and the same aggregates narrowed by a boolean field.
///
/// This is the "it's a real query engine on the device, not just a sync
/// cache" path. COUNT(*) was already exercised by callers, but SUM/AVG and a
/// boolean WHERE were not — and a query that CBL rejects fails at runtime, in
/// the UI, with no build-time warning. Hence a headless test.
use couchbase_lite::{
    Database, DatabaseConfiguration, Document, Query, QueryLanguage,
};
use tempfile::TempDir;

fn save(coll: &mut couchbase_lite::collection::Collection, id: &str, price: f64, offline: bool) {
    let mut doc = Document::new_with_id(id);
    {
        let mut props = doc.mutable_properties();
        props.at("item").put_string("espresso");
        props.at("price").put_f64(price);
        props.at("lane").put_string("lane-1");
        props.at("capturedOffline").put_bool(offline);
    }
    coll.save_document(&mut doc).expect("save document");
}

#[test]
fn aggregates_and_boolean_filters_work_on_device() {
    let dir = TempDir::new().unwrap();
    let config = DatabaseConfiguration {
        directory: dir.path(),
        encryption_key: None,
    };
    let db = Database::open("agg_test", Some(config)).expect("open database");
    let mut coll = db.default_collection_or_error().unwrap();

    // 4 sales totalling 20.00; 2 of them captured while the uplink was down,
    // totalling 6.00.
    save(&mut coll, "s1", 3.00, true);
    save(&mut coll, "s2", 3.00, true);
    save(&mut coll, "s3", 7.00, false);
    save(&mut coll, "s4", 7.00, false);

    // The whole-shift summary: exactly the shape the apps' planSummary() builds.
    let query = Query::new(
        &db,
        QueryLanguage::N1QL,
        "SELECT COUNT(*) AS sales, SUM(price) AS revenue, AVG(price) AS basket \
         FROM _default WHERE lane IS NOT MISSING",
    )
    .expect("create summary query");
    let mut results = query.execute().expect("execute summary query");
    let row = results.next().expect("expected a summary row");
    assert_eq!(row.get(0).as_i64_or_0(), 4, "COUNT(*)");
    assert_eq!(row.get(1).as_f64_or_0(), 20.0, "SUM(price)");
    assert_eq!(row.get(2).as_f64_or_0(), 5.0, "AVG(price)");

    // The offline-captured subset: a boolean field in the WHERE clause.
    let query = Query::new(
        &db,
        QueryLanguage::N1QL,
        "SELECT COUNT(*) AS sales, SUM(price) AS revenue \
         FROM _default WHERE capturedOffline = true",
    )
    .expect("create offline query");
    let mut results = query.execute().expect("execute offline query");
    let row = results.next().expect("expected an offline row");
    assert_eq!(row.get(0).as_i64_or_0(), 2, "COUNT(*) where capturedOffline");
    assert_eq!(row.get(1).as_f64_or_0(), 6.0, "SUM(price) where capturedOffline");
}
