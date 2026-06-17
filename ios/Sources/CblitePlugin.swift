import Foundation
import Tauri
import UIKit
import WebKit
import CouchbaseLiteSwift

// ---------------------------------------------------------------------------
// CblitePlugin — iOS implementation of tauri-plugin-cblite, backed by the
// CouchbaseLiteSwift SDK. Mirrors the Android Kotlin plugin command-for-command:
//   openDatabase, closeDatabase, getDocument, saveDocument, executeQuery (N1QL),
//   createFtsIndex, startReplication, stopReplication, saveBlob, getBlobData.
// Response shapes match Kotlin: queries resolve { rows: [...] }, blobs resolve
// { value: "..." }, documents resolve their JSON body directly.
// ---------------------------------------------------------------------------

// ── Arbitrary-JSON Decodable (for saveDocument bodies) ──────────────────────

enum JSONValue: Decodable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else if let o = try? c.decode([String: JSONValue].self) {
            self = .object(o)
        } else {
            throw DecodingError.dataCorruptedError(in: c, debugDescription: "unsupported JSON value")
        }
    }

    /// Convert to a Foundation object suitable for JSONSerialization.
    var anyValue: Any {
        switch self {
        case .null: return NSNull()
        case .bool(let b): return b
        case .number(let n):
            // Render integral numbers as Int so JSON round-trips without ".0".
            if n.truncatingRemainder(dividingBy: 1) == 0, n >= -9_007_199_254_740_991, n <= 9_007_199_254_740_991 {
                return Int64(n)
            }
            return n
        case .string(let s): return s
        case .array(let a): return a.map { $0.anyValue }
        case .object(let o): return o.mapValues { $0.anyValue }
        }
    }
}

// ── Command argument types (mirror commands_mobile.rs payloads) ─────────────

class OpenDatabaseArgs: Decodable {
    let path: String
    let name: String
    var encryptionPassword: String? = nil
    var collections: [String]? = nil
}

class DocArgs: Decodable {
    let collection: String
    let docId: String
}

class SaveDocumentArgs: Decodable {
    let collection: String
    let docId: String
    let body: JSONValue
    var encryptedFields: [String]? = nil
}

class ExecuteQueryArgs: Decodable {
    let language: String?
    let queryStr: String
    var parameters: [String: JSONValue]? = nil
}

class CreateFtsIndexArgs: Decodable {
    let collection: String
    let indexName: String
    let field: String
}

class StartReplicationArgs: Decodable {
    let url: String
    let collection: String
    var direction: String? = "both"
    var username: String? = nil
    var password: String? = nil
    var sessionId: String? = nil
    var cookieName: String? = nil
}

class SaveBlobArgs: Decodable {
    let dataB64: String
    var contentType: String? = "application/octet-stream"
}

class GetBlobArgs: Decodable {
    let digest: String
}

// ---------------------------------------------------------------------------

class CblitePlugin: Plugin {

    private var database: Database?
    private var replicator: Replicator?
    private var collectionListenerTokens: [ListenerToken] = []
    private var replListenerToken: ListenerToken? = nil

    // ── helpers ──────────────────────────────────────────────────────────────

    /// "scope.coll" or bare "coll" (=> _default scope), creating if needed.
    private func resolveCollection(_ db: Database, _ spec: String) throws -> Collection {
        let parts = spec.split(separator: ".", maxSplits: 1).map(String.init)
        let (scope, coll) = parts.count == 2 ? (parts[0], parts[1]) : ("_default", parts[0])
        return try db.createCollection(name: coll, scope: scope)
    }

    private func jsonStringToDict(_ s: String) throws -> [String: Any] {
        guard let data = s.data(using: .utf8),
              let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            throw NSError(domain: "cblite", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "invalid JSON from CBL"])
        }
        return obj
    }

    private func closeCurrentDatabase() throws {
        for t in collectionListenerTokens { t.remove() }
        collectionListenerTokens.removeAll()
        replListenerToken?.remove()
        replListenerToken = nil
        replicator?.stop()
        replicator = nil
        try database?.close()
        database = nil
    }

    /// One-shot config.json import, mirroring the Kotlin plugin: reads
    /// Documents/config.json, saves it to _default.config/app-config, then
    /// deletes the file. On the Simulator the Documents directory is a plain
    /// host directory, so this doubles as a config-injection mechanism.
    private func importConfigIfPresent(_ db: Database) {
        do {
            guard let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first
            else { return }
            let file = docs.appendingPathComponent("config.json")
            guard FileManager.default.fileExists(atPath: file.path) else { return }
            let json = try String(contentsOf: file, encoding: .utf8)
            let coll = try db.createCollection(name: "config", scope: "_default")
            let doc = MutableDocument(id: "app-config")
            try doc.setJSON(json)
            try coll.save(document: doc)
            try FileManager.default.removeItem(at: file)
            Logger.info("cblite: imported config.json into _default.config/app-config")
        } catch {
            Logger.error("cblite: importConfigIfPresent failed: \(error)")
        }
    }

    // ── openDatabase ─────────────────────────────────────────────────────────

    @objc public func openDatabase(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(OpenDatabaseArgs.self)
        do {
            try closeCurrentDatabase()

            var config = DatabaseConfiguration()
            config.directory = args.path
            let db = try Database(name: args.name, config: config)
            database = db

            for spec in args.collections ?? [] {
                let collection = try resolveCollection(db, spec)
                let token = collection.addChangeListener(listener: { [weak self] (change: CollectionChange) in
                    let payload = JSTypes.coerceDictionaryToJSObject(
                        ["docIds": change.documentIDs]) ?? [:]
                    DispatchQueue.main.async {
                        self?.trigger("cblite://collection-changed", data: payload)
                    }
                })
                collectionListenerTokens.append(token)
            }

            importConfigIfPresent(db)
            invoke.resolve()
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── closeDatabase ────────────────────────────────────────────────────────

    @objc public func closeDatabase(_ invoke: Invoke) throws {
        do {
            try closeCurrentDatabase()
            invoke.resolve()
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── getDocument ──────────────────────────────────────────────────────────

    @objc public func getDocument(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(DocArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        do {
            let coll = try resolveCollection(db, args.collection)
            guard let doc = try coll.document(id: args.docId) else {
                invoke.resolve([:] as JSObject)
                return
            }
            let dict = try jsonStringToDict(doc.toJSON())
            invoke.resolve(JSTypes.coerceDictionaryToJSObject(dict) ?? [:])
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── saveDocument ─────────────────────────────────────────────────────────

    @objc public func saveDocument(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(SaveDocumentArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        do {
            let coll = try resolveCollection(db, args.collection)

            // `_deleted` / `__deleted` are soft-delete sentinels — purge instead.
            if case .object(let o) = args.body {
                let deleted = { (k: String) -> Bool in
                    if case .bool(true)? = o[k] { return true }
                    return false
                }
                if deleted("_deleted") || deleted("__deleted") {
                    try? coll.purge(id: args.docId)
                    invoke.resolve()
                    return
                }
            }

            let data = try JSONSerialization.data(withJSONObject: args.body.anyValue)
            let json = String(data: data, encoding: .utf8) ?? "{}"
            let doc = MutableDocument(id: args.docId)
            try doc.setJSON(json)
            try coll.save(document: doc)
            invoke.resolve()
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── executeQuery ─────────────────────────────────────────────────────────

    @objc public func executeQuery(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(ExecuteQueryArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        let language = args.language ?? "N1QL"
        guard language.uppercased() == "N1QL" else {
            return invoke.reject("Only N1QL query language is supported on iOS")
        }
        do {
            let query = try db.createQuery(args.queryStr)

            if let params = args.parameters {
                let p = Parameters()
                for (k, v) in params {
                    switch v {
                    case .string(let s): _ = p.setString(s, forName: k)
                    case .bool(let b): _ = p.setBoolean(b, forName: k)
                    case .number(let n):
                        if n.truncatingRemainder(dividingBy: 1) == 0 {
                            _ = p.setInt64(Int64(n), forName: k)
                        } else {
                            _ = p.setDouble(n, forName: k)
                        }
                    case .null: _ = p.setValue(nil, forName: k)
                    default: _ = p.setString("\(v.anyValue)", forName: k)
                    }
                }
                query.parameters = p
            }

            var rows: [[String: Any]] = []
            for result in try query.execute() {
                rows.append(try jsonStringToDict(result.toJSON()))
            }
            invoke.resolve(JSTypes.coerceDictionaryToJSObject(["rows": rows]) ?? [:])
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── createFtsIndex ───────────────────────────────────────────────────────

    @objc public func createFtsIndex(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(CreateFtsIndexArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        do {
            let coll = try resolveCollection(db, args.collection)
            let config = FullTextIndexConfiguration([args.field])
            try coll.createIndex(withName: args.indexName, config: config)
            invoke.resolve()
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── startReplication ─────────────────────────────────────────────────────

    @objc public func startReplication(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(StartReplicationArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        guard let url = URL(string: args.url) else { return invoke.reject("invalid url: \(args.url)") }
        do {
            DispatchQueue.main.async { [weak self] in
                self?.trigger("cblite://replication-status", data: ["activity": "Connecting"])
            }

            replListenerToken?.remove()
            replListenerToken = nil
            replicator?.stop()
            replicator = nil

            let coll = try resolveCollection(db, args.collection)
            let collConfig = CollectionConfiguration(collection: coll)
            var config = ReplicatorConfiguration(collections: [collConfig], target: URLEndpoint(url: url))
            switch args.direction ?? "both" {
            case "push": config.replicatorType = .push
            case "pull": config.replicatorType = .pull
            default: config.replicatorType = .pushAndPull
            }
            if let sessionId = args.sessionId, !sessionId.isEmpty {
                config.authenticator = SessionAuthenticator(
                    sessionID: sessionId, cookieName: args.cookieName ?? "SyncGatewaySession")
            } else if let u = args.username, let pw = args.password, !u.isEmpty, !pw.isEmpty {
                config.authenticator = BasicAuthenticator(username: u, password: pw)
            }
            config.continuous = true
            config.heartbeat = 30
            // Allow pulling documents created by other devices.
            config.enableAutoPurge = false

            let repl = Replicator(config: config)
            replicator = repl

            replListenerToken = repl.addChangeListener { [weak self] (change: ReplicatorChange) in
                let label: String
                switch change.status.activity {
                case .stopped: label = "Stopped"
                case .offline: label = "Offline"
                case .connecting: label = "Connecting"
                case .idle: label = "Idle"
                case .busy: label = "Busy"
                @unknown default: label = "Unknown"
                }
                var payload: JSObject = ["activity": label]
                if let err = change.status.error {
                    payload["error"] = "\(err.localizedDescription)"
                }
                DispatchQueue.main.async {
                    self?.trigger("cblite://replication-status", data: payload)
                }
            }

            repl.start()
            invoke.resolve()
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── stopReplication ──────────────────────────────────────────────────────

    @objc public func stopReplication(_ invoke: Invoke) throws {
        replListenerToken?.remove()
        replListenerToken = nil
        replicator?.stop()
        replicator = nil
        invoke.resolve()
    }

    // ── saveBlob ─────────────────────────────────────────────────────────────

    @objc public func saveBlob(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(SaveBlobArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        guard let data = Data(base64Encoded: args.dataB64) else {
            return invoke.reject("invalid base64 data")
        }
        do {
            let blob = Blob(contentType: args.contentType ?? "application/octet-stream", data: data)
            try db.saveBlob(blob: blob)
            guard let digest = blob.digest else {
                return invoke.reject("Blob has no digest after save")
            }
            invoke.resolve(["value": digest] as JSObject)
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── getBlobData ──────────────────────────────────────────────────────────

    @objc public func getBlobData(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(GetBlobArgs.self)
        guard let db = database else { return invoke.reject("Database not open") }
        do {
            let props: [String: Any] = ["@type": "blob", "digest": args.digest]
            guard let blob = try db.getBlob(properties: props), let content = blob.content else {
                return invoke.reject("Blob not found: \(args.digest)")
            }
            invoke.resolve(["value": content.base64EncodedString()] as JSObject)
        } catch {
            invoke.reject("\(error.localizedDescription)")
        }
    }

    // ── predictive models (desktop-only feature) ─────────────────────────────

    @objc public func registerPredictiveModel(_ invoke: Invoke) throws {
        invoke.reject("Predictive models are not supported on iOS")
    }

    @objc public func unregisterPredictiveModel(_ invoke: Invoke) throws {
        invoke.reject("Predictive models are not supported on iOS")
    }
}

@_cdecl("init_plugin_cblite")
func initPluginCblite() -> Plugin {
    return CblitePlugin()
}
