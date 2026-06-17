package com.plugin.cblite

import android.app.Activity
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.couchbase.lite.BasicAuthenticator
import com.couchbase.lite.Blob
import com.couchbase.lite.CouchbaseLite
import com.couchbase.lite.Collection as CblCollection
import com.couchbase.lite.Database
import com.couchbase.lite.DatabaseConfiguration
import com.couchbase.lite.ListenerToken
import com.couchbase.lite.MutableDocument
import com.couchbase.lite.Parameters
import com.couchbase.lite.Replicator
import com.couchbase.lite.ReplicatorActivityLevel
import com.couchbase.lite.ReplicatorConfiguration
import com.couchbase.lite.ReplicatorType
import com.couchbase.lite.CollectionConfiguration
import android.util.Log
import com.couchbase.lite.FullTextIndexConfiguration
import com.couchbase.lite.SessionAuthenticator
import com.couchbase.lite.URLEndpoint
import java.net.URI

@TauriPlugin
class CblitePlugin(private val activity: Activity) : Plugin(activity) {

    init {
        CouchbaseLite.init(activity)
    }

    private var database: Database? = null
    @Volatile private var replicator: Replicator? = null
    private val collectionListenerTokens: MutableList<ListenerToken> = mutableListOf()
    private var replListenerToken: ListenerToken? = null

    // ── open_database ─────────────────────────────────────────────────────────

    @Command
    fun openDatabase(invoke: Invoke) {
        val args = invoke.getArgs()
        val path = args.optString("path").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("path is required", null as JSObject?)
        val name = args.optString("name").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("name is required", null as JSObject?)

        try {
            closeCurrentDatabase()

            val config = DatabaseConfiguration().apply { directory = path }
            val db = Database(name, config)
            database = db

            // For each requested collection: ensure it exists and register a change listener.
            val collectionsArg = args.getJSONArray("collections")
            val collectionNames: List<String> = if (collectionsArg != null) {
                (0 until collectionsArg.length()).mapNotNull { collectionsArg.optString(it) }
            } else {
                emptyList()
            }

            for (collSpec in collectionNames) {
                val dotIdx = collSpec.indexOf('.')
                val (scope, coll) = if (dotIdx >= 0) {
                    collSpec.substring(0, dotIdx) to collSpec.substring(dotIdx + 1)
                } else {
                    "_default" to collSpec
                }
                val collection = db.createCollection(coll, scope)
                val token = collection.addChangeListener { change ->
                    val arr = JSArray()
                    change.documentIDs.forEach { arr.put(it) }
                    val obj = JSObject()
                    obj.put("docIds", arr)
                    activity.runOnUiThread { trigger("cblite://collection-changed", obj) }
                }
                collectionListenerTokens.add(token)
            }

            // Auto-import config.json from external storage (one-shot, deleted after import).
            // This runs here because Kotlin has reliable access to external storage while
            // Rust std::fs cannot access /sdcard paths on Android.
            importConfigIfPresent(db)

            invoke.resolve()
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    /** Reads config.json from external files dir, saves it to CBL, then deletes the file. */
    private fun importConfigIfPresent(db: Database) {
        try {
            val extDir = activity.getExternalFilesDir(null) ?: return
            val file = java.io.File(extDir, "config.json")
            if (!file.exists()) return

            val json = file.readText()
            // Parse and save to _default.config/app-config
            val parsed = org.json.JSONObject(json)
            val coll = db.createCollection("config", "_default")
            val doc = MutableDocument("app-config")
            doc.setJSON(parsed.toString())
            coll.save(doc)
            file.delete()
        } catch (e: Exception) {
            android.util.Log.e("CblitePlugin", "importConfigIfPresent error: ${e.message}", e)
        }
    }

    // ── create_fts_index ──────────────────────────────────────────────────────

    /** Creates (or idempotently ensures) an FTS index on a collection field. */
    @Command
    fun createFtsIndex(invoke: Invoke) {
        val args = invoke.getArgs()
        val collectionSpec = args.optString("collection").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("collection is required", null as JSObject?)
        val indexName = args.optString("indexName").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("indexName is required", null as JSObject?)
        val fieldArg = args.optString("field").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("field is required", null as JSObject?)
        val fields = fieldArg.trim().split("\\s+".toRegex()).filter { it.isNotBlank() }
        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val dotIdx = collectionSpec.indexOf('.')
            val (scope, coll) = if (dotIdx >= 0) {
                collectionSpec.substring(0, dotIdx) to collectionSpec.substring(dotIdx + 1)
            } else {
                "_default" to collectionSpec
            }
            val collection = db.createCollection(coll, scope)
            android.util.Log.d("CblitePlugin", "createFtsIndex: $indexName on $scope.$coll fields=$fields")
            collection.createIndex(indexName, FullTextIndexConfiguration(*fields.toTypedArray()))
            val after: Set<String> = collection.indexes
            android.util.Log.d("CblitePlugin", "createFtsIndex done, indexes now: $after")
            invoke.resolve()
        } catch (e: Throwable) {
            android.util.Log.e("CblitePlugin", "createFtsIndex FAILED: ${e.message}", e)
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    /** Returns the names of all indexes on a collection (used to verify FTS creation). */
    @Command
    fun listIndexes(invoke: Invoke) {
        val args = invoke.getArgs()
        val collectionSpec = args.optString("collection").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("collection is required", null as JSObject?)
        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val dotIdx = collectionSpec.indexOf('.')
            val (scope, coll) = if (dotIdx >= 0) {
                collectionSpec.substring(0, dotIdx) to collectionSpec.substring(dotIdx + 1)
            } else {
                "_default" to collectionSpec
            }
            val collection = db.createCollection(coll, scope)
            val indexNames: Set<String> = collection.indexes
            android.util.Log.d("CblitePlugin", "listIndexes $scope.$coll: $indexNames")
            val arr = JSArray()
            for (n in indexNames) { arr.put(n) }
            val result = JSObject()
            result.put("indexes", arr)
            invoke.resolve(result)
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── write_export_file ─────────────────────────────────────────────────────

    /** Writes a text file to the app's external files dir (ADB-accessible). */
    @Command
    fun writeExportFile(invoke: Invoke) {
        val args = invoke.getArgs()
        val filename = args.optString("filename").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("filename is required", null as JSObject?)
        val data = args.optString("data").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("data is required", null as JSObject?)
        try {
            val extDir = activity?.getExternalFilesDir(null)
                ?: return invoke.reject("External storage unavailable", null as JSObject?)
            val file = java.io.File(extDir.absolutePath, filename)
            file.parentFile?.mkdirs()
            file.writeText(data)
            val result = JSObject()
            result.put("value", file.absolutePath)
            invoke.resolve(result)
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── close_database ────────────────────────────────────────────────────────

    @Command
    fun closeDatabase(invoke: Invoke) {
        try {
            closeCurrentDatabase()
            invoke.resolve()
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    private fun closeCurrentDatabase() {
        collectionListenerTokens.forEach { it.close() }
        collectionListenerTokens.clear()
        replListenerToken?.close()
        replListenerToken = null
        replicator?.stop()
        replicator = null
        database?.close()
        database = null
    }

    // ── get_document ──────────────────────────────────────────────────────────

    @Command
    fun getDocument(invoke: Invoke) {
        val args = invoke.getArgs()
        val collection = args.optString("collection").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("collection is required", null as JSObject?)
        val docId = args.optString("docId").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("docId is required", null as JSObject?)

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val coll = resolveCollection(db, collection)
            val doc = coll.getDocument(docId)
            if (doc == null) {
                invoke.resolve(JSObject())
            } else {
                val json = doc.toJSON() ?: "{}"
                invoke.resolve(JSObject(json))
            }
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── save_document ─────────────────────────────────────────────────────────

    @Command
    fun saveDocument(invoke: Invoke) {
        val args = invoke.getArgs()
        val collection = args.optString("collection").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("collection is required", null as JSObject?)
        val docId = args.optString("docId").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("docId is required", null as JSObject?)
        val body = args.getJSObject("body")
            ?: return invoke.reject("body is required", null as JSObject?)

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val coll = resolveCollection(db, collection)
            // `{ _deleted: true }` or `{ __deleted: true }` are soft-delete sentinels — purge the document.
            // `_deleted` is a CBL reserved property and cannot be stored in a document body.
            // `__deleted` is the preferred non-reserved tombstone used by JS deleteKnowledgeChunk.
            if (body.optBoolean("_deleted", false) || body.optBoolean("__deleted", false)) {
                // Ignore NotFound — if the doc doesn't exist, the desired state is achieved.
                try { coll.purge(docId) } catch (_: Exception) {}
                invoke.resolve()
                return
            }
            val doc = MutableDocument(docId as String)
            doc.setJSON(body.toString())
            coll.save(doc)
            invoke.resolve()
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── execute_query ─────────────────────────────────────────────────────────

    @Command
    fun executeQuery(invoke: Invoke) {
        val args = invoke.getArgs()
        val language = args.optString("language").takeIf { it.isNotEmpty() } ?: "N1QL"
        val queryStr = args.optString("queryStr").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("queryStr is required", null as JSObject?)
        val parameters = args.getJSObject("parameters")

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)

            if (!language.equals("N1QL", ignoreCase = true)) {
                return invoke.reject("Only N1QL query language is supported on Android", null as JSObject?)
            }

            val query = db.createQuery(queryStr)

            if (parameters != null) {
                val params = Parameters()
                val keys = parameters.keys()
                while (keys.hasNext()) {
                    val key = keys.next()
                    when (val v = parameters.get(key)) {
                        is String  -> params.setString(key, v)
                        is Int     -> params.setInt(key, v)
                        is Long    -> params.setLong(key, v)
                        is Double  -> params.setDouble(key, v)
                        is Boolean -> params.setBoolean(key, v)
                        else       -> params.setString(key, v?.toString() ?: "")
                    }
                }
                query.parameters = params
            }

            val resultSet = query.execute()
            val rows = JSArray()
            for (result in resultSet) {
                val json = result.toJSON() ?: "{}"
                rows.put(JSObject(json))
            }

            val response = JSObject()
            response.put("rows", rows)
            invoke.resolve(response)
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── start_replication ─────────────────────────────────────────────────────

    @Command
    fun startReplication(invoke: Invoke) {
        val args = invoke.getArgs()
        val url        = args.optString("url").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("url is required", null as JSObject?)
        val collection = args.optString("collection").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("collection is required", null as JSObject?)
        val direction  = args.optString("direction").takeIf { it.isNotEmpty() } ?: "both"
        val username   = args.optString("username").takeIf { it.isNotEmpty() }
        val password   = args.optString("password").takeIf { it.isNotEmpty() }
        val sessionId  = args.optString("sessionId").takeIf { it.isNotEmpty() }
        val cookieName = args.optString("cookieName").takeIf { it.isNotEmpty() }

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)

            // Initial connecting status event
            val obj = JSObject()
            obj.put("activity", "Connecting")
            activity.runOnUiThread { trigger("cblite://replication-status", obj) }

            replListenerToken?.close()
            replListenerToken = null
            replicator?.stop()
            replicator = null

            val endpoint = URLEndpoint(URI(url))
            // CBL 4.0: CollectionConfiguration takes the Collection; constructor takes list + endpoint.
            // Each collection gets its own CollectionConfiguration — do NOT share instances.
            // channels = null (default) means "all channels the user has access to" per SG.
            val coll = resolveCollection(db, collection)
            val config = ReplicatorConfiguration(listOf(CollectionConfiguration(coll)), endpoint).apply {
                setType(when (direction) {
                    "push" -> ReplicatorType.PUSH
                    "pull" -> ReplicatorType.PULL
                    else   -> ReplicatorType.PUSH_AND_PULL
                })
                setAuthenticator(when {
                    !sessionId.isNullOrEmpty() ->
                        SessionAuthenticator(sessionId, cookieName ?: "SyncGatewaySession")
                    !username.isNullOrEmpty() && !password.isNullOrEmpty() ->
                        BasicAuthenticator(username, password.toCharArray())
                    else -> null
                })
                setContinuous(true)
                // Enable heartbeat to keep connection alive
                heartbeat = 30
                // ✅ CRITICAL: Disable auto purge to allow pulling documents created by OTHER DEVICES
                setAutoPurgeEnabled(false)
            }

            // Assign FIRST to prevent GC collection while starting - critical on Android!
            replicator = Replicator(config)

            replListenerToken = replicator!!.addChangeListener { change ->
                val label = when (change.status.activityLevel) {
                    ReplicatorActivityLevel.STOPPED     -> "Stopped"
                    ReplicatorActivityLevel.OFFLINE     -> "Offline"
                    ReplicatorActivityLevel.CONNECTING  -> "Connecting"
                    ReplicatorActivityLevel.IDLE        -> "Idle"
                    ReplicatorActivityLevel.BUSY        -> "Busy"
                    else                                -> "Unknown"
                }
                val obj = JSObject()
                obj.put("activity", label)
                val err = change.status.error
                if (err != null) obj.put("error", err.message ?: err.toString())
                activity.runOnUiThread { trigger("cblite://replication-status", obj) }
            }
            
            replicator!!.start()

            invoke.resolve()
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── stop_replication ──────────────────────────────────────────────────────

    @Command
    fun stopReplication(invoke: Invoke) {
        replListenerToken?.close()
        replListenerToken = null
        replicator?.stop()
        replicator = null
        invoke.resolve()
    }

    // ── save_blob ─────────────────────────────────────────────────────────────

    @Command
    fun saveBlob(invoke: Invoke) {
        val args = invoke.getArgs()
        val dataB64     = args.optString("dataB64").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("dataB64 is required", null as JSObject?)
        val contentType = args.optString("contentType").takeIf { it.isNotEmpty() }
            ?: "application/octet-stream"

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val data = Base64.decode(dataB64 as String, Base64.NO_WRAP)
            val blob = Blob(contentType as String, data as ByteArray)
            db.saveBlob(blob)
            val digest = blob.digest() ?: return invoke.reject("Blob has no digest after save", null as JSObject?)

            val result = JSObject()
            result.put("value", digest as String)
            invoke.resolve(result)
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── get_blob_data ─────────────────────────────────────────────────────────

    @Command
    fun getBlobData(invoke: Invoke) {
        val args = invoke.getArgs()
        val digest = args.optString("digest").takeIf { it.isNotEmpty() }
            ?: return invoke.reject("digest is required", null as JSObject?)

        try {
            val db = database ?: return invoke.reject("Database not open", null as JSObject?)
            val props = mapOf("@type" to "blob", "digest" to digest)
            val blob = db.getBlob(props)
                ?: return invoke.reject("Blob not found: $digest", null as JSObject?)
            val content = blob.content
                ?: return invoke.reject("Blob has no content: $digest", null as JSObject?)
            val b64 = Base64.encodeToString(content, Base64.NO_WRAP)

            val result = JSObject()
            result.put("value", b64 as String)
            invoke.resolve(result)
        } catch (e: Throwable) {
            invoke.reject(e.message ?: e.toString(), null as JSObject?)
        }
    }

    // ── register_predictive_model ─────────────────────────────────────────────

    @Command
    fun registerPredictiveModel(invoke: Invoke) {
        invoke.reject("Predictive models are not supported on Android", null as JSObject?)
    }

    @Command
    fun unregisterPredictiveModel(invoke: Invoke) {
        invoke.reject("Predictive models are not supported on Android", null as JSObject?)
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    private fun resolveCollection(db: Database, name: String): CblCollection {
        val dotIdx = name.indexOf('.')
        val (scope, coll) = if (dotIdx >= 0) {
            name.substring(0, dotIdx) to name.substring(dotIdx + 1)
        } else {
            "_default" to name
        }
        return db.createCollection(coll, scope)
    }
}
