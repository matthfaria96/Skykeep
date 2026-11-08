import AppKit
import Compression
import SQLite3

/// Reads another app's SQLite store without ever writing to it, and without launching that app.
/// Opened read-only (WAL readers never block the app that owns the file), closed after each pass.
final class ReadOnlyDB {
    private var db: OpaquePointer?

    init?(_ url: URL) {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let path = url.path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? url.path
        guard sqlite3_open_v2("file:\(path)?mode=ro", &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_URI, nil) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
        sqlite3_busy_timeout(db, 1500)
    }

    deinit { sqlite3_close(db) }

    /// Whether the file could actually be read (macOS answers "authorization denied" on the
    /// first query when Full Disk Access is missing).
    var readable: Bool { (try? rows("SELECT 1 FROM sqlite_master LIMIT 1")) != nil }

    func columns(_ table: String) -> Set<String> {
        Set(((try? rows("PRAGMA table_info(\(table))")) ?? []).compactMap { $0["name"] as? String })
    }

    func tables() -> Set<String> {
        Set(((try? rows("SELECT name FROM sqlite_master WHERE type = 'table'")) ?? []).compactMap { $0["name"] as? String })
    }

    struct Failure: Error { let message: String }

    func rows(_ sql: String, _ args: [Any] = []) throws -> [[String: Any]] {
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else {
            throw Failure(message: String(cString: sqlite3_errmsg(db)))
        }
        defer { sqlite3_finalize(stmt) }
        for (i, a) in args.enumerated() {
            let idx = Int32(i + 1)
            switch a {
            case let v as Int: sqlite3_bind_int64(stmt, idx, Int64(v))
            case let v as Int64: sqlite3_bind_int64(stmt, idx, v)
            case let v as Double: sqlite3_bind_double(stmt, idx, v)
            case let v as String: sqlite3_bind_text(stmt, idx, v, -1, unsafeBitCast(-1, to: sqlite3_destructor_type.self))
            default: sqlite3_bind_null(stmt, idx)
            }
        }
        var out: [[String: Any]] = []
        while true {
            let rc = sqlite3_step(stmt)
            if rc == SQLITE_DONE { break }
            guard rc == SQLITE_ROW else { throw Failure(message: String(cString: sqlite3_errmsg(db))) }
            var row: [String: Any] = [:]
            for c in 0..<sqlite3_column_count(stmt) {
                let name = String(cString: sqlite3_column_name(stmt, c))
                switch sqlite3_column_type(stmt, c) {
                case SQLITE_INTEGER: row[name] = Int(sqlite3_column_int64(stmt, c))
                case SQLITE_FLOAT: row[name] = sqlite3_column_double(stmt, c)
                case SQLITE_TEXT: row[name] = String(cString: sqlite3_column_text(stmt, c))
                case SQLITE_BLOB:
                    let n = Int(sqlite3_column_bytes(stmt, c))
                    row[name] = n > 0 ? Data(bytes: sqlite3_column_blob(stmt, c), count: n) : Data()
                default: break
                }
            }
            out.append(row)
        }
        return out
    }
}

/// Full Disk Access: what lets Skykeep read Notes' and Mail's own files directly, so neither
/// app has to be opened. macOS has no prompt for it; the person adds Skykeep in System Settings.
enum FullDiskAccess {
    static let notesStore = FileManager.default.homeDirectoryForCurrentUser
        .appending(path: "Library/Group Containers/group.com.apple.notes/NoteStore.sqlite")

    static var granted: Bool {
        // Mail's folder is the plainest test: listing it fails without the permission.
        let mail = FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Mail")
        if (try? FileManager.default.contentsOfDirectory(atPath: mail.path)) != nil { return true }
        return ReadOnlyDB(notesStore)?.readable ?? false
    }

    static func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            NSWorkspace.shared.open(url)
        }
    }
}

enum Gzip {
    /// gzip → bytes. The header is read by hand; the deflate stream goes to Compression.
    static func decompress(_ data: Data) -> Data? {
        guard data.count > 18, data[data.startIndex] == 0x1f, data[data.startIndex + 1] == 0x8b else {
            return zlibOrRaw(data)
        }
        let b = [UInt8](data)
        let flags = b[3]
        var i = 10
        if flags & 0x04 != 0 { guard i + 2 <= b.count else { return nil }; i += 2 + Int(b[i]) | Int(b[i + 1]) << 8 }
        if flags & 0x08 != 0 { while i < b.count && b[i] != 0 { i += 1 }; i += 1 }
        if flags & 0x10 != 0 { while i < b.count && b[i] != 0 { i += 1 }; i += 1 }
        if flags & 0x02 != 0 { i += 2 }
        guard i < b.count - 8 else { return nil }
        let isize = Int(b[b.count - 4]) | Int(b[b.count - 3]) << 8 | Int(b[b.count - 2]) << 16 | Int(b[b.count - 1]) << 24
        return inflate(Data(b[i..<(b.count - 8)]), expected: isize)
    }

    private static func zlibOrRaw(_ data: Data) -> Data? {
        if data.count > 2, data[data.startIndex] == 0x78 { return inflate(data.dropFirst(2), expected: data.count * 8) }
        return nil
    }

    private static func inflate(_ deflated: Data, expected: Int) -> Data? {
        let capacity = max(expected, 64) + 64
        var out = Data(count: capacity)
        let n = out.withUnsafeMutableBytes { dst in
            deflated.withUnsafeBytes { src in
                compression_decode_buffer(dst.bindMemory(to: UInt8.self).baseAddress!, capacity,
                                           src.bindMemory(to: UInt8.self).baseAddress!, deflated.count, nil, COMPRESSION_ZLIB)
            }
        }
        guard n > 0 else { return nil }
        return out.prefix(n)
    }
}

/// Just enough Protocol Buffers to walk a message: varints and length-delimited fields.
struct ProtoReader {
    enum Value { case varint(UInt64), bytes(Data), fixed64(UInt64), fixed32(UInt32) }
    let fields: [(Int, Value)]

    init(_ data: Data) {
        var out: [(Int, Value)] = []
        let b = [UInt8](data)
        var i = 0
        func varint() -> UInt64? {
            var r: UInt64 = 0, shift: UInt64 = 0
            while i < b.count {
                let byte = b[i]; i += 1
                r |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 { return r }
                shift += 7
                if shift > 63 { return nil }
            }
            return nil
        }
        while i < b.count {
            guard let key = varint() else { break }
            let field = Int(key >> 3), wire = key & 7
            switch wire {
            case 0: guard let v = varint() else { break }; out.append((field, .varint(v)))
            case 1:
                guard i + 8 <= b.count else { i = b.count; break }
                var v: UInt64 = 0; for k in 0..<8 { v |= UInt64(b[i + k]) << (8 * UInt64(k)) }
                i += 8; out.append((field, .fixed64(v)))
            case 2:
                guard let len = varint(), i + Int(len) <= b.count else { i = b.count; break }
                out.append((field, .bytes(Data(b[i..<(i + Int(len))])))); i += Int(len)
            case 5:
                guard i + 4 <= b.count else { i = b.count; break }
                var v: UInt32 = 0; for k in 0..<4 { v |= UInt32(b[i + k]) << (8 * UInt32(k)) }
                i += 4; out.append((field, .fixed32(v)))
            default: i = b.count
            }
        }
        fields = out
    }

    func message(_ f: Int) -> ProtoReader? {
        for (n, v) in fields where n == f { if case .bytes(let d) = v { return ProtoReader(d) } }
        return nil
    }
    func messages(_ f: Int) -> [ProtoReader] {
        fields.compactMap { n, v in if n == f, case .bytes(let d) = v { return ProtoReader(d) }; return nil }
    }
    func string(_ f: Int) -> String? {
        for (n, v) in fields where n == f { if case .bytes(let d) = v { return String(data: d, encoding: .utf8) } }
        return nil
    }
    func int(_ f: Int) -> Int? {
        for (n, v) in fields where n == f { if case .varint(let x) = v { return Int(truncatingIfNeeded: x) } }
        return nil
    }
}
