import AppKit
import SwiftUI

struct NoteFolder: Identifiable, Hashable {
    let id: String
    let name: String
    let parent: String?
    var count: Int
    let isTrash: Bool
}

struct NoteSummary: Identifiable, Hashable {
    let id: String
    let title: String
    let modified: Date
    let preview: String
    let folderID: String?
    let folderName: String
    let locked: Bool
    let pinned: Bool
    let editable: Bool
}

/// One run of Apple's note formatting, as the iCloud helper hands it over: a length of text
/// (UTF-16, as Notes counts) and how it is set.
struct NoteRun: Decodable, Hashable {
    var length: Int
    var style: Int?
    var indent: Int = 0
    var checked: Bool?
    var start: Int?
    var bold: Bool = false
    var italic: Bool = false
    var underline: Bool = false
    var strike: Bool = false
    var link: String?
    var attachment: String?
    var emphasis: Int?
    var quote: Bool = false

    enum CodingKeys: String, CodingKey { case length, style, indent, checked, start, bold, italic, underline, strike, link, attachment, emphasis, quote }

    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        length = try c.decodeIfPresent(Int.self, forKey: .length) ?? 0
        style = try c.decodeIfPresent(Int.self, forKey: .style)
        indent = try c.decodeIfPresent(Int.self, forKey: .indent) ?? 0
        checked = try c.decodeIfPresent(Bool.self, forKey: .checked)
        start = try c.decodeIfPresent(Int.self, forKey: .start)
        bold = try c.decodeIfPresent(Bool.self, forKey: .bold) ?? false
        italic = try c.decodeIfPresent(Bool.self, forKey: .italic) ?? false
        underline = try c.decodeIfPresent(Bool.self, forKey: .underline) ?? false
        strike = try c.decodeIfPresent(Bool.self, forKey: .strike) ?? false
        link = try c.decodeIfPresent(String.self, forKey: .link)
        attachment = try c.decodeIfPresent(String.self, forKey: .attachment)
        emphasis = try c.decodeIfPresent(Int.self, forKey: .emphasis)
        quote = try c.decodeIfPresent(Bool.self, forKey: .quote) ?? false
    }
}

/// Apple Notes, in iCloud — the same notes as the iPhone, read and written over CloudKit
/// through the iCloud helper, as Skykeep does on Windows and Linux. Checklists tick, text is
/// edited in place, notes are created, moved and deleted, and every change is on every device.
/// The Notes app is never opened.
@MainActor @Observable
final class NotesService {
    enum State: Equatable { case idle, signedOut, needsCode, loading, ready, failed(String) }

    var state: State = .idle
    var folders: [NoteFolder] = []
    var allNotes: [NoteSummary] = []
    var trashID = "TrashFolder-CloudKit"
    /// nil = every note outside Recently Deleted.
    var folderID: String?
    var selectedID: String?
    var syncing = false
    var lastError: String?
    /// Bumped after every pass, so an open note reads itself again.
    var revision = 0

    @ObservationIgnored private var timer: Timer?

    var connected: Bool { state == .ready }

    var notes: [NoteSummary] {
        let list: [NoteSummary]
        if let folderID { list = allNotes.filter { $0.folderID == folderID } }
        else { list = allNotes.filter { $0.folderID != trashID } }
        return list.sorted { a, b in a.pinned != b.pinned ? a.pinned : a.modified > b.modified }
    }

    // MARK: Session

    func load() async {
        if state == .loading { return }
        if state != .ready { state = .loading }
        do {
            let status = try await CloudBridge.shared.call("status") as? [String: Any]
            switch status?["state"] as? String {
            case "signedIn":
                try apply(await CloudBridge.shared.call("notes.list"))
                state = .ready
                startTimer()
                await sync()
            case "code": state = .needsCode
            default: state = .signedOut
            }
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    /// Answers nil when signed in, "code" when Apple wants the six digits, or an error to show.
    func signIn(appleID: String, password: String) async -> String? {
        do {
            let r = try await CloudBridge.shared.call("signIn", ["appleId": appleID, "password": password]) as? [String: Any]
            if r?["state"] as? String == "code" { state = .needsCode; return "code" }
            state = .idle
            await load()
            return nil
        } catch { return error.localizedDescription }
    }

    func submitCode(_ code: String) async -> String? {
        do {
            _ = try await CloudBridge.shared.call("submitCode", ["code": code])
            state = .idle
            await load()
            return nil
        } catch { return error.localizedDescription }
    }

    func resendCode() async { _ = try? await CloudBridge.shared.call("resendCode") }

    func signOut() async {
        _ = try? await CloudBridge.shared.call("signOut")
        allNotes = []
        folders = []
        state = .signedOut
    }

    private func startTimer() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, NSApp.isActive, self.connected else { return }
                await self.sync()
            }
        }
    }

    func sync() async {
        guard !syncing else { return }
        syncing = true
        defer { syncing = false }
        do {
            try apply(await CloudBridge.shared.call("notes.sync"))
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    private func apply(_ result: Any?) throws {
        guard let r = result as? [String: Any] else { return }
        if let t = r["trash"] as? String { trashID = t }
        let rawFolders = r["folders"] as? [[String: Any]] ?? []
        let rawNotes = r["notes"] as? [[String: Any]] ?? []
        var counts: [String: Int] = [:]
        for n in rawNotes { if let f = n["folder_guid"] as? String { counts[f, default: 0] += 1 } }
        var list = rawFolders.compactMap { f -> NoteFolder? in
            guard let id = f["guid"] as? String else { return nil }
            return NoteFolder(id: id, name: (f["title"] as? String) ?? "Notas", parent: f["parent_guid"] as? String,
                              count: counts[id] ?? 0, isTrash: id == trashID)
        }
        if !list.contains(where: { $0.id == trashID }), (counts[trashID] ?? 0) > 0 {
            list.append(NoteFolder(id: trashID, name: "Apagadas Recentemente", parent: nil, count: counts[trashID] ?? 0, isTrash: true))
        }
        list = list.map { f in f.isTrash ? NoteFolder(id: f.id, name: "Apagadas Recentemente", parent: nil, count: f.count, isTrash: true) : f }
        list.sort { a, b in
            if a.isTrash != b.isTrash { return !a.isTrash }
            if a.name == "Notes" || a.name == "Notas" { return true }
            if b.name == "Notes" || b.name == "Notas" { return false }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
        let names = Dictionary(list.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
        allNotes = rawNotes.compactMap { n in
            guard let id = n["guid"] as? String else { return nil }
            let folder = n["folder_guid"] as? String
            var t = (n["modified_at"] as? Double) ?? Double((n["modified_at"] as? Int) ?? 0)
            if t > 100_000_000_000 { t /= 1000 }
            let title = ((n["title"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return NoteSummary(id: id, title: title.isEmpty ? "Nova nota" : title,
                               modified: Date(timeIntervalSince1970: t),
                               preview: ((n["snippet"] as? String) ?? "").replacingOccurrences(of: "\n", with: " "),
                               folderID: folder, folderName: folder.flatMap { names[$0] } ?? "",
                               locked: (n["locked"] as? Bool) ?? false, pinned: (n["pinned"] as? Bool) ?? false,
                               editable: (n["editable"] as? Bool) ?? true)
        }
        folders = list
        revision += 1
    }

    // MARK: Notes

    func body(of id: String) async -> (String, [NoteRun])? {
        guard let r = try? await CloudBridge.shared.call("note.body", ["guid": id]) as? [String: Any],
              let body = r["body"] as? String else { return nil }
        var runs: [NoteRun] = []
        if let raw = r["runs"], let data = try? JSONSerialization.data(withJSONObject: raw) {
            runs = (try? JSONDecoder().decode([NoteRun].self, from: data)) ?? []
        }
        return (body, runs)
    }

    /// Tick or untick the checklist line beginning at `start`; `expect` is that line's text as
    /// shown, so a tick can never land on a line that moved there in the meantime.
    func setChecked(_ id: String, start: Int, checked: Bool, expect: String) async -> String? {
        await run("note.check", ["guid": id, "start": start, "checked": checked, "expect": expect])
    }

    /// Title, heading, subheading, body, monospaced, bulleted, dashed, numbered or checklist.
    func setStyle(_ id: String, start: Int, style: Int, expect: String) async -> String? {
        await run("note.check", ["guid": id, "start": start, "style": style, "expect": expect])
    }

    func save(_ id: String, body: String, base: String?) async -> String? {
        var args: [String: Any] = ["guid": id, "body": body]
        if let base { args["base"] = base }
        return await run("note.save", args)
    }

    func create(body: String) async -> String? {
        var args: [String: Any] = ["body": body]
        if let folderID, folderID != trashID { args["folder"] = folderID }
        do {
            let r = try await CloudBridge.shared.call("note.create", args) as? [String: Any]
            try? apply(await CloudBridge.shared.call("notes.list"))
            let guid = r?["guid"] as? String
            selectedID = guid
            return guid
        } catch {
            lastError = error.localizedDescription
            return nil
        }
    }

    func delete(_ id: String) async -> String? {
        let e = await run("note.delete", ["guid": id])
        if e == nil, selectedID == id { selectedID = notes.first?.id }
        return e
    }

    func move(_ id: String, to folder: String) async -> String? {
        await run("note.move", ["guid": id, "folder": folder])
    }

    private func run(_ cmd: String, _ args: [String: Any]) async -> String? {
        do {
            _ = try await CloudBridge.shared.call(cmd, args)
            try? apply(await CloudBridge.shared.call("notes.list"))
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func search(_ q: String) -> [NoteSummary] {
        allNotes.filter { $0.folderID != trashID && ($0.title.localizedStandardContains(q) || $0.preview.localizedStandardContains(q)) }
    }

    // MARK: Pictures

    /// A picture attached to a note, from the copy Notes keeps on this Mac (needs Full Disk
    /// Access; without it the picture shows as a labelled placeholder).
    nonisolated static func localImage(for identifier: String) -> NSImage? {
        let fm = FileManager.default
        let container = fm.homeDirectoryForCurrentUser.appending(path: "Library/Group Containers/group.com.apple.notes")
        let accounts = (try? fm.contentsOfDirectory(at: container.appending(path: "Accounts"), includingPropertiesForKeys: nil)) ?? []
        if let db = ReadOnlyDB(container.appending(path: "NoteStore.sqlite")),
           let a = try? db.rows("SELECT ZMEDIA FROM ZICCLOUDSYNCINGOBJECT WHERE ZIDENTIFIER = ?", [identifier]).first,
           let mediaPK = a["ZMEDIA"] as? Int,
           let m = try? db.rows("SELECT ZIDENTIFIER FROM ZICCLOUDSYNCINGOBJECT WHERE Z_PK = ?", [mediaPK]).first,
           let mediaID = m["ZIDENTIFIER"] as? String {
            for acct in accounts {
                guard let e = fm.enumerator(at: acct.appending(path: "Media/\(mediaID)"), includingPropertiesForKeys: nil) else { continue }
                while let u = e.nextObject() as? URL {
                    if let img = NSImage(contentsOf: u), img.isValid { return img }
                }
            }
        }
        for acct in accounts {
            for ext in ["png", "jpg", "jpeg"] {
                if let img = NSImage(contentsOf: acct.appending(path: "FallbackImages/\(identifier).\(ext)")) { return img }
            }
        }
        return nil
    }
}
