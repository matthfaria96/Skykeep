import AppKit
import QuickLookThumbnailing
import SwiftUI

struct DriveItem: Identifiable, Hashable {
    enum CloudState { case local, cloud, downloading, uploading }

    var id: URL { url }
    let url: URL
    let name: String
    let isFolder: Bool
    let size: Int64
    let modified: Date
    let state: CloudState
    let kind: String
}

/// iCloud Drive is a folder on the Mac; macOS keeps it in sync. Skykeep shows it with the
/// download state of each file and the Mac's own previews.
@MainActor @Observable
final class DriveService {
    static let mobileDocuments = URL(fileURLWithPath: NSHomeDirectory()).appending(path: "Library/Mobile Documents")
    static let root = mobileDocuments.appending(path: "com~apple~CloudDocs")

    var current: URL = DriveService.root
    var items: [DriveItem] = []
    var error: String?
    var selection: Set<URL> = []
    @ObservationIgnored private var back: [URL] = []
    @ObservationIgnored private var forward: [URL] = []
    var canGoBack = false
    var canGoForward = false

    @ObservationIgnored private var watcher: DispatchSourceFileSystemObject?
    @ObservationIgnored private var poll: Timer?
    @ObservationIgnored private let thumbs = NSCache<NSURL, NSImage>()

    var available: Bool { FileManager.default.fileExists(atPath: Self.root.path) }

    init() { thumbs.countLimit = 400 }

    /// iCloud Drive › … › the folder shown. A folder inside an app's container (Pages…) hangs
    /// off iCloud Drive as Finder shows it.
    var pathComponents: [URL] {
        let cur = current.standardizedFileURL.pathComponents
        let rootParts = Self.root.standardizedFileURL.pathComponents
        let mobileParts = Self.mobileDocuments.standardizedFileURL.pathComponents
        var base: URL
        var rest: ArraySlice<String>
        var out: [URL] = [Self.root]
        if cur.starts(with: rootParts) {
            base = Self.root
            rest = cur.dropFirst(rootParts.count)
        } else if cur.starts(with: mobileParts), cur.count >= mobileParts.count + 2 {
            base = Self.mobileDocuments.appending(path: cur[mobileParts.count]).appending(path: "Documents")
            out.append(base)
            rest = cur.dropFirst(mobileParts.count + 2)
        } else {
            return [current]
        }
        for part in rest {
            base = base.appending(path: part)
            out.append(base)
        }
        return out
    }

    func displayName(_ url: URL) -> String {
        if url.standardizedFileURL == Self.root.standardizedFileURL { return "iCloud Drive" }
        if isAppContainer(url) { return FileManager.default.displayName(atPath: url.deletingLastPathComponent().path) }
        return FileManager.default.displayName(atPath: url.path)
    }

    private func isAppContainer(_ url: URL) -> Bool {
        url.lastPathComponent == "Documents" && url.deletingLastPathComponent().deletingLastPathComponent().standardizedFileURL == Self.mobileDocuments.standardizedFileURL
    }

    func open(_ url: URL) {
        back.append(current)
        forward.removeAll()
        go(url)
    }

    func goBack() {
        guard let u = back.popLast() else { return }
        forward.append(current)
        go(u)
    }

    func goForward() {
        guard let u = forward.popLast() else { return }
        back.append(current)
        go(u)
    }

    private func go(_ url: URL) {
        current = url
        selection = []
        canGoBack = !back.isEmpty
        canGoForward = !forward.isEmpty
        reload()
        watch()
    }

    func start() {
        reload()
        watch()
    }

    func reload() {
        let keys: [URLResourceKey] = [
            .isDirectoryKey, .fileSizeKey, .totalFileAllocatedSizeKey, .contentModificationDateKey, .localizedTypeDescriptionKey,
            .ubiquitousItemDownloadingStatusKey, .ubiquitousItemIsDownloadingKey, .ubiquitousItemIsUploadingKey, .isPackageKey,
        ]
        do {
            let urls = try FileManager.default.contentsOfDirectory(at: current, includingPropertiesForKeys: keys, options: [.skipsHiddenFiles])
            var list = urls.map { Self.item($0) }
            if current.standardizedFileURL == Self.root.standardizedFileURL { list += appContainers() }
            items = list.sorted { a, b in
                if a.isFolder != b.isFolder { return a.isFolder }
                return a.name.localizedStandardCompare(b.name) == .orderedAscending
            }
            error = nil
        } catch {
            items = []
            self.error = error.localizedDescription
        }
        let busy = items.contains { $0.state == .downloading || $0.state == .uploading }
        poll?.invalidate()
        if busy {
            poll = Timer.scheduledTimer(withTimeInterval: 2, repeats: false) { [weak self] _ in Task { @MainActor in self?.reload() } }
        }
    }

    /// Pages, Numbers, Keynote, Preview… each keeps its iCloud files in its own container,
    /// which Finder shows as a folder at the top of iCloud Drive.
    private func appContainers() -> [DriveItem] {
        let fm = FileManager.default
        guard let dirs = try? fm.contentsOfDirectory(at: Self.mobileDocuments, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { return [] }
        return dirs.compactMap { dir in
            guard dir.lastPathComponent != "com~apple~CloudDocs" else { return nil }
            let docs = dir.appending(path: "Documents")
            guard let contents = try? fm.contentsOfDirectory(atPath: docs.path), contents.contains(where: { !$0.hasPrefix(".") }) else { return nil }
            let modified = (try? docs.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast
            return DriveItem(url: docs, name: fm.displayName(atPath: dir.path), isFolder: true, size: 0, modified: modified, state: .local, kind: "Pasta de app")
        }
    }

    nonisolated static func item(_ url: URL) -> DriveItem {
        let v = try? url.resourceValues(forKeys: [
            .isDirectoryKey, .fileSizeKey, .contentModificationDateKey, .localizedTypeDescriptionKey,
            .ubiquitousItemDownloadingStatusKey, .ubiquitousItemIsDownloadingKey, .ubiquitousItemIsUploadingKey, .isPackageKey,
        ])
        let isFolder = (v?.isDirectory ?? false) && !(v?.isPackage ?? false)
        var state: DriveItem.CloudState = .local
        if v?.ubiquitousItemIsUploading == true { state = .uploading }
        else if v?.ubiquitousItemIsDownloading == true { state = .downloading }
        else if let s = v?.ubiquitousItemDownloadingStatus, s == .notDownloaded { state = .cloud }
        return DriveItem(
            url: url,
            name: FileManager.default.displayName(atPath: url.path),
            isFolder: isFolder,
            size: Int64(v?.fileSize ?? 0),
            modified: v?.contentModificationDate ?? .distantPast,
            state: isFolder ? .local : state,
            kind: v?.localizedTypeDescription ?? ""
        )
    }

    private func watch() {
        watcher?.cancel()
        let fd = Darwin.open(current.path, O_EVTONLY)
        guard fd >= 0 else { return }
        let src = DispatchSource.makeFileSystemObjectSource(fileDescriptor: fd, eventMask: [.write, .rename, .delete], queue: .main)
        src.setEventHandler { [weak self] in Task { @MainActor in self?.reload() } }
        src.setCancelHandler { close(fd) }
        src.resume()
        watcher = src
    }

    // MARK: Actions

    func openFile(_ item: DriveItem) {
        if item.isFolder { open(item.url) } else { NSWorkspace.shared.open(item.url) }
    }

    func reveal(_ urls: [URL]) { NSWorkspace.shared.activateFileViewerSelecting(urls) }

    func download(_ url: URL) {
        try? FileManager.default.startDownloadingUbiquitousItem(at: url)
        reload()
    }

    func evict(_ url: URL) {
        try? FileManager.default.evictUbiquitousItem(at: url)
        reload()
    }

    func trash(_ urls: [URL]) {
        for u in urls { try? FileManager.default.trashItem(at: u, resultingItemURL: nil) }
        reload()
    }

    func rename(_ url: URL, to name: String) -> String? {
        let clean = name.trimmingCharacters(in: .whitespaces)
        guard !clean.isEmpty, !clean.contains("/") else { return "Nome inválido" }
        do {
            try FileManager.default.moveItem(at: url, to: url.deletingLastPathComponent().appending(path: clean))
            reload()
            return nil
        } catch { return error.localizedDescription }
    }

    func newFolder() {
        var name = "Pasta sem título"
        var n = 2
        while FileManager.default.fileExists(atPath: current.appending(path: name).path) { name = "Pasta sem título \(n)"; n += 1 }
        try? FileManager.default.createDirectory(at: current.appending(path: name), withIntermediateDirectories: false)
        reload()
    }

    /// Copies files dropped onto the window into the folder being shown.
    func importFiles(_ urls: [URL]) {
        for u in urls {
            var target = current.appending(path: u.lastPathComponent)
            var n = 2
            while FileManager.default.fileExists(atPath: target.path) {
                let base = u.deletingPathExtension().lastPathComponent
                let ext = u.pathExtension
                target = current.appending(path: ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)")
                n += 1
            }
            try? FileManager.default.copyItem(at: u, to: target)
        }
        reload()
    }

    func thumbnail(_ item: DriveItem, side: CGFloat) async -> NSImage {
        if let hit = thumbs.object(forKey: item.url as NSURL) { return hit }
        let fallback = NSWorkspace.shared.icon(forFile: item.url.path)
        guard !item.isFolder else { return fallback }
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        let req = QLThumbnailGenerator.Request(fileAt: item.url, size: CGSize(width: side, height: side), scale: scale, representationTypes: .thumbnail)
        if let rep = try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: req) {
            thumbs.setObject(rep.nsImage, forKey: item.url as NSURL)
            return rep.nsImage
        }
        return fallback
    }

    /// Recently changed files anywhere in iCloud Drive, for Today.
    func recentFiles(limit: Int = 8) async -> [DriveItem] {
        let root = Self.root
        return await Task.detached(priority: .utility) { DriveService.scanRecent(root, limit: limit) }.value
    }

    /// Names matching a search, anywhere in iCloud Drive.
    func search(_ q: String, limit: Int = 30) async -> [DriveItem] {
        let root = Self.root
        return await Task.detached(priority: .userInitiated) { DriveService.scanNames(root, q, limit: limit) }.value
    }

    nonisolated private static func scanRecent(_ root: URL, limit: Int) -> [DriveItem] {
        let keys: [URLResourceKey] = [.contentModificationDateKey, .isDirectoryKey, .isPackageKey]
        guard let e = FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys, options: [.skipsHiddenFiles, .skipsPackageDescendants]) else { return [] }
        var found: [(URL, Date)] = []
        var seen = 0
        while let url = e.nextObject() as? URL {
            seen += 1
            if seen > 6000 { break }
            let v = try? url.resourceValues(forKeys: Set(keys))
            if v?.isDirectory == true && v?.isPackage != true { continue }
            if !isDocument(url) { continue }
            found.append((url, v?.contentModificationDate ?? .distantPast))
        }
        return found.sorted { $0.1 > $1.1 }.prefix(limit).map { item($0.0) }
    }

    /// Things a person made or saved — not an app's preferences, caches or databases that
    /// happen to live in iCloud Drive.
    nonisolated static func isDocument(_ url: URL) -> Bool {
        let ext = url.pathExtension.lowercased()
        if ext.isEmpty { return false }
        let noise: Set<String> = ["wal", "shm", "db", "sqlite", "sqlite3", "plist", "json", "xml", "log", "lock", "tmp", "temp", "cache",
                                  "prefs", "ini", "dat", "bak", "lck", "ds_store", "localized", "icloud", "prmdc2", "cfg", "conf", "yaml", "yml"]
        if noise.contains(ext) || ext.hasSuffix("-wal") || ext.hasSuffix("-shm") || ext.hasSuffix("-journal") { return false }
        let path = url.path
        if path.contains("/Library/") || path.contains("/.") { return false }
        return true
    }

    nonisolated private static func scanNames(_ root: URL, _ q: String, limit: Int) -> [DriveItem] {
        guard let e = FileManager.default.enumerator(at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles, .skipsPackageDescendants]) else { return [] }
        var out: [URL] = []
        var seen = 0
        while let url = e.nextObject() as? URL {
            seen += 1
            if seen > 20000 || out.count >= limit { break }
            if url.lastPathComponent.localizedStandardContains(q) { out.append(url) }
        }
        return out.map { item($0) }
    }
}
