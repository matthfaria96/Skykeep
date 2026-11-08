import AppKit
import CommonCrypto
import Security
import SQLite3
import SwiftUI

/// Apple's Passwords, in every browser on the Mac.
///
/// Apple's "iCloud Passwords" extension (Chrome Web Store) fills and saves passwords straight
/// into the Passwords app, through a small helper macOS ships. macOS only tells Chrome and
/// Firefox where that helper is — a "native messaging host" file — so in Vivaldi, Brave, Edge,
/// Arc or Opera the extension installs but never connects. Connecting a browser here writes
/// that same file into the browser's own folder: from then on the browser and Passwords share
/// one set of passwords, both ways, with no copying.
///
/// For passwords already saved in a browser, Skykeep reads them (the browser's key comes from
/// the Mac's keychain, with the Mac's own "Allow"), writes a file in the format Passwords
/// imports, and deletes it once imported.
struct BrowserInfo: Identifiable, Hashable {
    let id: String
    let name: String
    let bundleID: String
    /// Under ~/Library/Application Support.
    let supportPath: String
    let safeStorage: (service: String, account: String)

    static func == (a: BrowserInfo, b: BrowserInfo) -> Bool { a.id == b.id }
    func hash(into h: inout Hasher) { h.combine(id) }

    static let chromium: [BrowserInfo] = [
        BrowserInfo(id: "vivaldi", name: "Vivaldi", bundleID: "com.vivaldi.Vivaldi", supportPath: "Vivaldi", safeStorage: ("Vivaldi Safe Storage", "Vivaldi")),
        BrowserInfo(id: "chrome", name: "Google Chrome", bundleID: "com.google.Chrome", supportPath: "Google/Chrome", safeStorage: ("Chrome Safe Storage", "Chrome")),
        BrowserInfo(id: "brave", name: "Brave", bundleID: "com.brave.Browser", supportPath: "BraveSoftware/Brave-Browser", safeStorage: ("Brave Safe Storage", "Brave")),
        BrowserInfo(id: "edge", name: "Microsoft Edge", bundleID: "com.microsoft.edgemac", supportPath: "Microsoft Edge", safeStorage: ("Microsoft Edge Safe Storage", "Microsoft Edge")),
        BrowserInfo(id: "arc", name: "Arc", bundleID: "company.thebrowser.Browser", supportPath: "Arc/User Data", safeStorage: ("Arc Safe Storage", "Arc")),
        BrowserInfo(id: "opera", name: "Opera", bundleID: "com.operasoftware.Opera", supportPath: "com.operasoftware.Opera", safeStorage: ("Opera Safe Storage", "Opera")),
        BrowserInfo(id: "chromium", name: "Chromium", bundleID: "org.chromium.Chromium", supportPath: "Chromium", safeStorage: ("Chromium Safe Storage", "Chromium")),
    ]

    var supportDir: URL {
        FileManager.default.homeDirectoryForCurrentUser.appending(path: "Library/Application Support").appending(path: supportPath)
    }
    var appURL: URL? { NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) }
    var hostFile: URL { supportDir.appending(path: "NativeMessagingHosts/\(BrowserBridge.hostName).json") }
    var icon: NSImage { appURL.map { NSWorkspace.shared.icon(forFile: $0.path) } ?? NSImage(systemSymbolName: "globe", accessibilityDescription: nil)! }
}

struct BrowserStatus: Hashable {
    var connected = false
    /// Connected by macOS itself (Chrome), nothing for Skykeep to write.
    var connectedBySystem = false
    var extensionInstalled = false
    var savedPasswords = 0
}

@MainActor @Observable
final class BrowserBridge {
    nonisolated static let hostName = "com.apple.passwordmanager"
    nonisolated static let extensionID = "pejdijmoenmkgeppbflobdenhhabjlaj"
    static let storeURL = URL(string: "https://chromewebstore.google.com/detail/icloud-passwords/\(extensionID)")!
    static let helperCandidates = [
        "/System/Cryptexes/App/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper",
        "/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper",
    ]

    var browsers: [BrowserInfo] = []
    var status: [String: BrowserStatus] = [:]
    var restartNeeded: Set<String> = []
    var working: String?
    var message: String?
    /// The import file waiting for the Passwords app, and when it will be deleted on its own.
    var pendingFile: URL?
    var pendingCount = 0
    var pendingBrowser = ""
    @ObservationIgnored private var cleanup: DispatchWorkItem?

    var helperAvailable: Bool { Self.helperCandidates.contains { FileManager.default.isExecutableFile(atPath: $0) } }
    var safariInstalled: Bool { NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Safari") != nil }

    func scan() {
        browsers = BrowserInfo.chromium.filter { $0.appURL != nil }
        for b in browsers {
            Task.detached(priority: .utility) {
                let s = BrowserBridge.readStatus(b)
                await MainActor.run { self.status[b.id] = s }
            }
        }
    }

    nonisolated static func readStatus(_ b: BrowserInfo) -> BrowserStatus {
        let fm = FileManager.default
        var s = BrowserStatus()
        s.connected = fm.fileExists(atPath: b.hostFile.path)
        if b.id == "chrome", fm.fileExists(atPath: "/Library/Google/Chrome/NativeMessagingHosts/\(hostName).json") {
            s.connected = true
            s.connectedBySystem = true
        }
        let profiles = profileDirs(b)
        s.extensionInstalled = profiles.contains { fm.fileExists(atPath: $0.appending(path: "Extensions/\(extensionID)").path) }
        s.savedPasswords = profiles.reduce(0) { $0 + countLogins($1.appending(path: "Login Data")) }
        return s
    }

    nonisolated static func profileDirs(_ b: BrowserInfo) -> [URL] {
        let fm = FileManager.default
        guard let dirs = try? fm.contentsOfDirectory(at: b.supportDir, includingPropertiesForKeys: nil) else { return [] }
        return dirs.filter { fm.fileExists(atPath: $0.appending(path: "Login Data").path) }
    }

    // MARK: Connect

    func connect(_ b: BrowserInfo) {
        guard let helper = Self.helperCandidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else {
            message = "O ajudante de senhas da Apple não está neste Mac. Ele vem com o macOS 14 ou mais novo."
            return
        }
        let manifest: [String: Any] = [
            "name": Self.hostName,
            "description": "PasswordManagerBrowserExtensionHelper",
            "path": helper,
            "type": "stdio",
            "allowed_origins": ["chrome-extension://\(Self.extensionID)/", "chrome-extension://mfbcdcnpokpoajjciilocoachedjkima/"],
        ]
        do {
            try FileManager.default.createDirectory(at: b.hostFile.deletingLastPathComponent(), withIntermediateDirectories: true)
            let data = try JSONSerialization.data(withJSONObject: manifest, options: [.prettyPrinted, .withoutEscapingSlashes])
            try data.write(to: b.hostFile, options: .atomic)
            status[b.id, default: BrowserStatus()].connected = true
            if isRunning(b) { restartNeeded.insert(b.id) }
            message = nil
        } catch {
            message = "Não deu para conectar o \(b.name): \(error.localizedDescription)"
        }
    }

    func disconnect(_ b: BrowserInfo) {
        try? FileManager.default.removeItem(at: b.hostFile)
        status[b.id, default: BrowserStatus()].connected = false
        if isRunning(b) { restartNeeded.insert(b.id) }
    }

    func installExtension(_ b: BrowserInfo) {
        guard let app = b.appURL else { return }
        NSWorkspace.shared.open([Self.storeURL], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
    }

    func isRunning(_ b: BrowserInfo) -> Bool { !NSRunningApplication.runningApplications(withBundleIdentifier: b.bundleID).isEmpty }

    /// Quits the browser politely (it restores its tabs) and opens it again.
    func restart(_ b: BrowserInfo) {
        restartNeeded.remove(b.id)
        let apps = NSRunningApplication.runningApplications(withBundleIdentifier: b.bundleID)
        apps.forEach { $0.terminate() }
        Task {
            for _ in 0..<40 {
                if NSRunningApplication.runningApplications(withBundleIdentifier: b.bundleID).isEmpty { break }
                try? await Task.sleep(for: .milliseconds(250))
            }
            if let app = b.appURL { _ = try? await NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration()) }
        }
    }

    // MARK: Import into Passwords

    func prepareImport(_ b: BrowserInfo) async {
        working = b.id
        message = nil
        defer { working = nil }
        let result = await Task.detached(priority: .userInitiated) { BrowserBridge.exportLogins(b) }.value
        switch result {
        case .failure(let e):
            message = e.text
        case .success(let logins):
            guard !logins.isEmpty else { message = "Nenhuma senha salva no \(b.name)."; return }
            do {
                let file = try Self.writeCSV(logins, browser: b.name)
                pendingFile = file
                pendingCount = logins.count
                pendingBrowser = b.name
                scheduleCleanup()
            } catch {
                message = "Não deu para preparar o arquivo: \(error.localizedDescription)"
            }
        }
    }

    func showPendingFile() {
        if let f = pendingFile { NSWorkspace.shared.activateFileViewerSelecting([f]) }
    }

    /// Overwrites the file before removing it: it held passwords in plain text.
    func deletePendingFile() {
        cleanup?.cancel()
        guard let f = pendingFile else { return }
        if let h = try? FileHandle(forWritingTo: f) {
            let size = (try? h.seekToEnd()) ?? 0
            try? h.seek(toOffset: 0)
            h.write(Data(repeating: 0, count: Int(size)))
            try? h.close()
        }
        try? FileManager.default.removeItem(at: f)
        pendingFile = nil
        pendingCount = 0
    }

    private func scheduleCleanup() {
        cleanup?.cancel()
        let work = DispatchWorkItem { [weak self] in Task { @MainActor in self?.deletePendingFile() } }
        cleanup = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 15 * 60, execute: work)
    }

    struct Login: Sendable { let url: String; let host: String; let username: String; let password: String }
    struct ExportError: Error { let text: String }

    nonisolated static func exportLogins(_ b: BrowserInfo) -> Result<[Login], ExportError> {
        guard let secret = safeStorageKey(b) else {
            return .failure(ExportError(text: "O Mac não liberou a chave do \(b.name). Tente de novo e escolha “Permitir” no aviso do Chaveiro."))
        }
        let key = deriveKey(secret)
        var seen = Set<String>()
        var out: [Login] = []
        for profile in profileDirs(b) {
            for file in ["Login Data", "Login Data For Account"] {
                for row in readLogins(profile.appending(path: file)) {
                    guard let pw = decrypt(row.blob, key: key), !pw.isEmpty,
                          let url = URL(string: row.origin), let scheme = url.scheme, scheme.hasPrefix("http"), let host = url.host else { continue }
                    let k = "\(host)|\(row.username)|\(pw)"
                    if seen.insert(k).inserted { out.append(Login(url: row.origin, host: host, username: row.username, password: pw)) }
                }
            }
        }
        return .success(out.sorted { $0.host < $1.host })
    }

    nonisolated private static func safeStorageKey(_ b: BrowserInfo) -> Data? {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: b.safeStorage.service,
            kSecAttrAccount as String: b.safeStorage.account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess else { return nil }
        return out as? Data
    }

    /// Chromium on macOS: PBKDF2-SHA1 over the keychain secret, salt "saltysalt", 1003 rounds, 16 bytes.
    nonisolated private static func deriveKey(_ secret: Data) -> Data {
        var key = Data(count: kCCKeySizeAES128)
        let salt = Array("saltysalt".utf8)
        _ = key.withUnsafeMutableBytes { k in
            secret.withUnsafeBytes { s in
                CCKeyDerivationPBKDF(CCPBKDFAlgorithm(kCCPBKDF2), s.baseAddress?.assumingMemoryBound(to: Int8.self), secret.count,
                                     salt, salt.count, CCPseudoRandomAlgorithm(kCCPRFHmacAlgSHA1), 1003,
                                     k.baseAddress?.assumingMemoryBound(to: UInt8.self), kCCKeySizeAES128)
            }
        }
        return key
    }

    /// "v10" + AES-128-CBC, IV of sixteen spaces, PKCS#7.
    nonisolated private static func decrypt(_ blob: Data, key: Data) -> String? {
        guard blob.count > 3, blob.prefix(3) == Data("v10".utf8) else { return nil }
        let cipher = blob.dropFirst(3)
        let iv = Data(repeating: 0x20, count: kCCBlockSizeAES128)
        var out = Data(count: cipher.count + kCCBlockSizeAES128)
        var moved = 0
        let outCount = out.count
        let status = out.withUnsafeMutableBytes { o in
            cipher.withUnsafeBytes { c in
                iv.withUnsafeBytes { v in
                    key.withUnsafeBytes { k in
                        CCCrypt(CCOperation(kCCDecrypt), CCAlgorithm(kCCAlgorithmAES), CCOptions(kCCOptionPKCS7Padding),
                                k.baseAddress, key.count, v.baseAddress, c.baseAddress, cipher.count, o.baseAddress, outCount, &moved)
                    }
                }
            }
        }
        guard status == kCCSuccess else { return nil }
        return String(data: out.prefix(moved), encoding: .utf8)
    }

    struct Row { let origin: String; let username: String; let blob: Data }

    /// Read-only and immutable: the browser can stay open, and its file is never touched.
    nonisolated private static func open(_ file: URL) -> OpaquePointer? {
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        let path = file.path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? file.path
        var db: OpaquePointer?
        guard sqlite3_open_v2("file:\(path)?immutable=1", &db, SQLITE_OPEN_READONLY | SQLITE_OPEN_URI, nil) == SQLITE_OK else {
            sqlite3_close(db)
            return nil
        }
        return db
    }

    nonisolated private static func countLogins(_ file: URL) -> Int {
        guard let db = open(file) else { return 0 }
        defer { sqlite3_close(db) }
        var stmt: OpaquePointer?
        guard sqlite3_prepare_v2(db, "SELECT count(*) FROM logins WHERE blacklisted_by_user = 0 AND length(password_value) > 0", -1, &stmt, nil) == SQLITE_OK else { return 0 }
        defer { sqlite3_finalize(stmt) }
        return sqlite3_step(stmt) == SQLITE_ROW ? Int(sqlite3_column_int(stmt, 0)) : 0
    }

    nonisolated private static func readLogins(_ file: URL) -> [Row] {
        guard let db = open(file) else { return [] }
        defer { sqlite3_close(db) }
        var stmt: OpaquePointer?
        let sql = "SELECT origin_url, username_value, password_value FROM logins WHERE blacklisted_by_user = 0"
        guard sqlite3_prepare_v2(db, sql, -1, &stmt, nil) == SQLITE_OK else { return [] }
        defer { sqlite3_finalize(stmt) }
        var rows: [Row] = []
        while sqlite3_step(stmt) == SQLITE_ROW {
            let origin = sqlite3_column_text(stmt, 0).map { String(cString: $0) } ?? ""
            let user = sqlite3_column_text(stmt, 1).map { String(cString: $0) } ?? ""
            let n = Int(sqlite3_column_bytes(stmt, 2))
            let blob = n > 0 ? Data(bytes: sqlite3_column_blob(stmt, 2), count: n) : Data()
            rows.append(Row(origin: origin, username: user, blob: blob))
        }
        return rows
    }

    /// Apple's import format: Title, URL, Username, Password, Notes, OTPAuth. Readable only by you.
    nonisolated static func writeCSV(_ logins: [Login], browser: String) throws -> URL {
        func q(_ s: String) -> String { "\"" + s.replacingOccurrences(of: "\"", with: "\"\"") + "\"" }
        var csv = "Title,URL,Username,Password,Notes,OTPAuth\n"
        for l in logins {
            csv += [q(l.host), q(l.url), q(l.username), q(l.password), q("Importada do \(browser) pelo Skykeep"), q("")].joined(separator: ",") + "\n"
        }
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
        let file = downloads.appending(path: "Senhas do \(browser) para importar.csv")
        try? FileManager.default.removeItem(at: file)
        FileManager.default.createFile(atPath: file.path, contents: nil, attributes: [.posixPermissions: 0o600])
        try Data(csv.utf8).write(to: file)
        return file
    }
}
