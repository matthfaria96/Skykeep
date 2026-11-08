import Foundation

/// The iCloud half of Skykeep: `skykeep-cloud`, a small helper inside the app bundle built from
/// the same Rust code as Skykeep for Windows and Linux. It signs in to iCloud the way
/// icloud.com does and reads and writes Notes over CloudKit. One JSON message per line, each
/// way; replies are matched to calls by id. Nothing here opens another app.
@MainActor
final class CloudBridge {
    static let shared = CloudBridge()

    struct Failure: Error, LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    private var process: Process?
    private var input: FileHandle?
    private var buffer = Data()
    private var pending: [Int: CheckedContinuation<Any?, Error>] = [:]
    private var nextID = 1

    private var executable: URL? {
        let fm = FileManager.default
        let candidates = [
            Bundle.main.bundleURL.appending(path: "Contents/MacOS/skykeep-cloud"),
            Bundle.main.executableURL?.deletingLastPathComponent().appending(path: "skykeep-cloud"),
        ].compactMap { $0 }
        return candidates.first { fm.isExecutableFile(atPath: $0.path) }
    }

    private func start() throws {
        if let process, process.isRunning { return }
        guard let url = executable else { throw Failure(message: "O componente do iCloud não está no app. Reinstale o Skykeep.") }
        let p = Process()
        p.executableURL = url
        let inPipe = Pipe(), outPipe = Pipe()
        p.standardInput = inPipe
        p.standardOutput = outPipe
        p.standardError = FileHandle.nullDevice
        outPipe.fileHandleForReading.readabilityHandler = { handle in
            let data = handle.availableData
            Task { @MainActor in CloudBridge.shared.received(data) }
        }
        p.terminationHandler = { _ in
            Task { @MainActor in CloudBridge.shared.ended() }
        }
        try p.run()
        process = p
        input = inPipe.fileHandleForWriting
    }

    private func received(_ data: Data) {
        guard !data.isEmpty else { return }
        buffer.append(data)
        while let nl = buffer.firstIndex(of: 0x0a) {
            let line = buffer[buffer.startIndex..<nl]
            buffer.removeSubrange(buffer.startIndex...nl)
            guard let obj = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
                  let id = obj["id"] as? Int, let c = pending.removeValue(forKey: id) else { continue }
            if (obj["ok"] as? Bool) == true {
                c.resume(returning: obj["result"])
            } else {
                c.resume(throwing: Failure(message: Self.friendly(obj["error"] as? String ?? "Erro desconhecido")))
            }
        }
    }

    private func ended() {
        process = nil
        input = nil
        let waiting = pending
        pending = [:]
        for c in waiting.values { c.resume(throwing: Failure(message: "O componente do iCloud parou. Tente de novo.")) }
    }

    func call(_ cmd: String, _ args: [String: Any] = [:]) async throws -> Any? {
        try start()
        let id = nextID
        nextID += 1
        let message: [String: Any] = ["id": id, "cmd": cmd, "args": args]
        var data = try JSONSerialization.data(withJSONObject: message)
        data.append(0x0a)
        return try await withCheckedThrowingContinuation { c in
            pending[id] = c
            do { try input?.write(contentsOf: data) } catch {
                pending.removeValue(forKey: id)
                c.resume(throwing: error)
            }
        }
    }

    /// Apple's and the library's messages are in English; the ones people meet most, in Portuguese.
    private static func friendly(_ s: String) -> String {
        let l = s.lowercased()
        if l.contains("password") && (l.contains("incorrect") || l.contains("wrong") || l.contains("401")) { return "Conta Apple ou senha incorretas." }
        if l.contains("code") && (l.contains("incorrect") || l.contains("invalid") || l.contains("wrong")) { return "Código incorreto. Confira e tente de novo." }
        if l.contains("network") || l.contains("offline") || l.contains("connect") { return "Sem conexão com o iCloud agora. Tente de novo em instantes." }
        if l.contains("advanced data protection") || l.contains("pcs") { return "Sua conta usa a Proteção Avançada de Dados: aprove o acesso na web do iCloud nos seus dispositivos." }
        return s
    }
}
