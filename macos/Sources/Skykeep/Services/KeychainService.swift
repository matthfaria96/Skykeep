import AppKit
import LocalAuthentication
import Security
import SwiftUI

struct KeychainItem: Identifiable, Hashable {
    let id: String
    let server: String
    let account: String
    let label: String
    let modified: Date?
    let ref: Data
    let createdBySkykeep: Bool

    var title: String { label.isEmpty ? server : label }
}

/// Passwords in the Mac's login keychain, behind Touch ID or the Mac's password.
///
/// What macOS lets an app see: the login keychain's internet passwords (each password itself
/// still needs the keychain's own "Allow" the first time). Passwords in iCloud Keychain are
/// sealed to Apple's apps — the Passwords app and Safari — so Skykeep opens Passwords for those.
@MainActor @Observable
final class KeychainService {
    var unlocked = false
    var items: [KeychainItem] = []
    var error: String?
    @ObservationIgnored private var resignObserver: NSObjectProtocol?
    @ObservationIgnored private var resignedAt: Date?

    nonisolated static let marker = "Skykeep"

    init() {
        resignObserver = NotificationCenter.default.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.resignedAt = Date() }
        }
        NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in
                // Locks again after five minutes away from Skykeep.
                if let t = self?.resignedAt, Date().timeIntervalSince(t) > 300 { self?.lock() }
            }
        }
    }

    func unlock() async {
        let ctx = LAContext()
        var err: NSError?
        guard ctx.canEvaluatePolicy(.deviceOwnerAuthentication, error: &err) else {
            error = err?.localizedDescription
            return
        }
        let ok = (try? await ctx.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "mostrar suas senhas")) ?? false
        unlocked = ok
        if ok { await load() }
    }

    func lock() {
        unlocked = false
        items = []
    }

    func load() async {
        items = await Task.detached(priority: .userInitiated) { KeychainService.fetchAll() }.value
    }

    nonisolated private static func fetchAll() -> [KeychainItem] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassInternetPassword,
            kSecMatchLimit as String: kSecMatchLimitAll,
            kSecReturnAttributes as String: true,
            kSecReturnPersistentRef as String: true,
        ]
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let rows = result as? [[String: Any]] else { return [] }
        return rows.compactMap { a in
            guard let ref = a[kSecValuePersistentRef as String] as? Data else { return nil }
            let server = a[kSecAttrServer as String] as? String ?? ""
            let account = a[kSecAttrAccount as String] as? String ?? ""
            guard !server.isEmpty || !account.isEmpty else { return nil }
            return KeychainItem(
                id: ref.base64EncodedString(),
                server: server,
                account: account,
                label: a[kSecAttrLabel as String] as? String ?? "",
                modified: a[kSecAttrModificationDate as String] as? Date,
                ref: ref,
                createdBySkykeep: (a[kSecAttrComment as String] as? String) == marker
            )
        }
        .sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
    }

    /// The password itself. macOS may ask once more, in its own keychain dialog.
    func password(for item: KeychainItem) async -> String? {
        let ref = item.ref
        return await Task.detached(priority: .userInitiated) {
            let q: [String: Any] = [
                kSecClass as String: kSecClassInternetPassword,
                kSecValuePersistentRef as String: ref,
                kSecReturnData as String: true,
                kSecMatchLimit as String: kSecMatchLimitOne,
            ]
            var out: CFTypeRef?
            guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
            return String(data: data, encoding: .utf8)
        }.value
    }

    func add(server: String, account: String, password: String) async -> String? {
        let host = URL(string: server.contains("://") ? server : "https://\(server)")?.host ?? server
        let q: [String: Any] = [
            kSecClass as String: kSecClassInternetPassword,
            kSecAttrServer as String: host,
            kSecAttrAccount as String: account,
            kSecAttrProtocol as String: kSecAttrProtocolHTTPS,
            kSecAttrLabel as String: host,
            kSecAttrComment as String: Self.marker,
            kSecValueData as String: Data(password.utf8),
        ]
        let status = SecItemAdd(q as CFDictionary, nil)
        await load()
        if status == errSecSuccess { return nil }
        if status == errSecDuplicateItem { return "Já existe uma senha para essa conta nesse site." }
        return SecCopyErrorMessageString(status, nil) as String? ?? "Erro \(status)"
    }

    func delete(_ item: KeychainItem) async {
        let q: [String: Any] = [kSecClass as String: kSecClassInternetPassword, kSecValuePersistentRef as String: item.ref]
        SecItemDelete(q as CFDictionary)
        await load()
    }

    static func openPasswordsApp() {
        let candidates = ["/System/Applications/Passwords.app", "/Applications/Passwords.app"]
        if let path = candidates.first(where: { FileManager.default.fileExists(atPath: $0) }) {
            NSWorkspace.shared.open(URL(fileURLWithPath: path))
        } else if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Passwords") {
            NSWorkspace.shared.open(url)
        }
    }
}

enum PasswordGenerator {
    /// Apple's strong-password shape: three groups of six, one capital and one digit — easy to
    /// type on a phone, 71 bits.
    static func apple() -> String {
        let consonants = Array("bcdfghjkmnpqrstvwxz"), vowels = Array("aeiouy")
        var groups: [[Character]] = (0..<3).map { _ in
            (0..<6).map { i in i % 2 == 0 ? consonants.randomElement()! : vowels.randomElement()! }
        }
        let g = Int.random(in: 0..<3), i = Int.random(in: 0..<6)
        groups[g][i] = Character(groups[g][i].uppercased())
        var dg = Int.random(in: 0..<3), di = Int.random(in: 0..<6)
        while dg == g && di == i { dg = Int.random(in: 0..<3); di = Int.random(in: 0..<6) }
        groups[dg][di] = Character(String(Int.random(in: 0...9)))
        return groups.map { String($0) }.joined(separator: "-")
    }

    static func random(length: Int, symbols: Bool) -> String {
        var pool = Array("abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789")
        if symbols { pool += Array("!@#$%&*-_=+?") }
        var bytes = [UInt8](repeating: 0, count: length)
        _ = SecRandomCopyBytes(kSecRandomDefault, length, &bytes)
        return String(bytes.map { pool[Int($0) % pool.count] })
    }
}

enum Clipboard {
    /// Copies, then clears it after 90 seconds if nothing else was copied meanwhile.
    static func copySecret(_ s: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(s, forType: .string)
        // Ask clipboard managers not to keep it.
        pb.setString("", forType: NSPasteboard.PasteboardType("org.nspasteboard.ConcealedType"))
        let count = pb.changeCount
        DispatchQueue.main.asyncAfter(deadline: .now() + 90) {
            if NSPasteboard.general.changeCount == count { NSPasteboard.general.clearContents() }
        }
    }

    static func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }
}
