@preconcurrency import Photos
import AVFoundation
import LocalAuthentication
import SwiftUI

enum PhotoSource: Hashable {
    case library, favorites, videos, selfies, screenshots, recentlyAdded, hidden
    case album(String)

    var title: String {
        switch self {
        case .library: "Biblioteca"
        case .favorites: "Favoritos"
        case .videos: "Vídeos"
        case .selfies: "Selfies"
        case .screenshots: "Capturas de Tela"
        case .recentlyAdded: "Adicionados Recentemente"
        case .hidden: "Ocultos"
        case .album: "Álbum"
        }
    }

    var symbol: String {
        switch self {
        case .library: "photo.on.rectangle"
        case .favorites: "heart"
        case .videos: "video"
        case .selfies: "person.crop.square"
        case .screenshots: "camera.viewfinder"
        case .recentlyAdded: "clock"
        case .hidden: "eye.slash"
        case .album: "rectangle.stack"
        }
    }

    static let fixed: [PhotoSource] = [.library, .favorites, .recentlyAdded, .videos, .selfies, .screenshots, .hidden]
}

struct AlbumInfo: Identifiable, Hashable {
    let id: String
    let title: String
    let count: Int
}

/// The Mac's Photos library (iCloud Photos included) through PhotoKit. Thumbnails come from
/// the library's own cache; full pictures are fetched from iCloud only when opened.
@MainActor @Observable
final class PhotosService: NSObject, PHPhotoLibraryChangeObserver {
    var status = PHPhotoLibrary.authorizationStatus(for: .readWrite)
    var source: PhotoSource = .library { didSet { refetch() } }
    var albums: [AlbumInfo] = []
    var assets: PHFetchResult<PHAsset> = PHFetchResult()
    var revision = 0
    var hiddenUnlocked = false

    @ObservationIgnored let manager = PHCachingImageManager()
    @ObservationIgnored private let cache = NSCache<NSString, NSImage>()
    @ObservationIgnored private var registered = false

    override init() {
        super.init()
        cache.countLimit = 800
        if isAuthorized { start() }
    }

    var isAuthorized: Bool { status == .authorized || status == .limited }
    var denied: Bool { status == .denied || status == .restricted }

    func request() async {
        status = await PHPhotoLibrary.requestAuthorization(for: .readWrite)
        if isAuthorized { start() }
    }

    private func start() {
        if !registered { PHPhotoLibrary.shared().register(self); registered = true }
        loadAlbums()
        refetch()
    }

    nonisolated func photoLibraryDidChange(_ changeInstance: PHChange) {
        Task { @MainActor in
            if let details = changeInstance.changeDetails(for: self.assets) {
                self.assets = details.fetchResultAfterChanges
            }
            self.loadAlbums()
            self.revision += 1
        }
    }

    func loadAlbums() {
        var out: [AlbumInfo] = []
        let lists = PHAssetCollection.fetchAssetCollections(with: .album, subtype: .any, options: nil)
        lists.enumerateObjects { c, _, _ in
            let n = PHAsset.fetchAssets(in: c, options: nil).count
            if n > 0 { out.append(AlbumInfo(id: c.localIdentifier, title: c.localizedTitle ?? "Sem título", count: n)) }
        }
        albums = out.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
    }

    private func smart(_ subtype: PHAssetCollectionSubtype) -> PHAssetCollection? {
        PHAssetCollection.fetchAssetCollections(with: .smartAlbum, subtype: subtype, options: nil).firstObject
    }

    func refetch() {
        guard isAuthorized else { return }
        let opts = PHFetchOptions()
        opts.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        switch source {
        case .library:
            assets = PHAsset.fetchAssets(with: opts)
        case .favorites:
            opts.predicate = NSPredicate(format: "favorite == YES")
            assets = PHAsset.fetchAssets(with: opts)
        case .videos:
            assets = PHAsset.fetchAssets(with: .video, options: opts)
        case .recentlyAdded:
            opts.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
            opts.predicate = NSPredicate(format: "creationDate > %@", Date().adding(days: -30) as NSDate)
            assets = PHAsset.fetchAssets(with: opts)
        case .selfies:
            assets = smart(.smartAlbumSelfPortraits).map { PHAsset.fetchAssets(in: $0, options: opts) } ?? PHFetchResult()
        case .screenshots:
            assets = smart(.smartAlbumScreenshots).map { PHAsset.fetchAssets(in: $0, options: opts) } ?? PHFetchResult()
        case .hidden:
            guard hiddenUnlocked else { assets = PHFetchResult(); break }
            opts.includeHiddenAssets = true
            assets = smart(.smartAlbumAllHidden).map { PHAsset.fetchAssets(in: $0, options: opts) } ?? PHFetchResult()
        case .album(let id):
            let c = PHAssetCollection.fetchAssetCollections(withLocalIdentifiers: [id], options: nil).firstObject
            assets = c.map { PHAsset.fetchAssets(in: $0, options: opts) } ?? PHFetchResult()
        }
        revision += 1
    }

    func unlockHidden() async {
        let ctx = LAContext()
        let ok = (try? await ctx.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: "mostrar o álbum Ocultos")) ?? false
        hiddenUnlocked = ok
        refetch()
    }

    func lockHidden() {
        hiddenUnlocked = false
        if source == .hidden { refetch() }
    }

    // MARK: Pictures

    func thumbnail(_ asset: PHAsset, side: CGFloat) async -> NSImage? {
        let key = "\(asset.localIdentifier)|\(Int(side))" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        let opts = PHImageRequestOptions()
        opts.deliveryMode = .opportunistic
        opts.resizeMode = .fast
        opts.isNetworkAccessAllowed = true
        let image = await request(asset, size: CGSize(width: side, height: side), mode: .aspectFill, options: opts, acceptDegraded: true)
        if let image { cache.setObject(image, forKey: key) }
        return image
    }

    func fullImage(_ asset: PHAsset, size: CGSize) async -> NSImage? {
        let opts = PHImageRequestOptions()
        opts.deliveryMode = .highQualityFormat
        opts.isNetworkAccessAllowed = true
        opts.version = .current
        return await request(asset, size: size, mode: .aspectFit, options: opts, acceptDegraded: false)
    }

    /// One image per call. With `.opportunistic` the first (possibly degraded) frame is enough
    /// for a thumbnail; a later better one lands in the cache through the grid's next pass.
    private func request(_ asset: PHAsset, size: CGSize, mode: PHImageContentMode, options: PHImageRequestOptions, acceptDegraded: Bool) async -> NSImage? {
        let manager = self.manager
        final class Once: @unchecked Sendable { var done = false; var id: PHImageRequestID = 0 }
        let once = Once()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { (c: CheckedContinuation<NSImage?, Never>) in
                once.id = manager.requestImage(for: asset, targetSize: size, contentMode: mode, options: options) { image, info in
                    let degraded = (info?[PHImageResultIsDegradedKey] as? Bool) ?? false
                    let cancelled = (info?[PHImageCancelledKey] as? Bool) ?? false
                    let failed = info?[PHImageErrorKey] != nil
                    if once.done { return }
                    if degraded && !acceptDegraded && !cancelled && !failed { return }
                    once.done = true
                    c.resume(returning: image)
                }
            }
        } onCancel: {
            manager.cancelImageRequest(once.id)
        }
    }

    func playerItem(_ asset: PHAsset) async -> AVPlayerItem? {
        let opts = PHVideoRequestOptions()
        opts.isNetworkAccessAllowed = true
        opts.deliveryMode = .automatic
        return await withCheckedContinuation { c in
            PHImageManager.default().requestPlayerItem(forVideo: asset, options: opts) { item, _ in c.resume(returning: item) }
        }
    }

    func toggleFavorite(_ asset: PHAsset) {
        let newValue = !asset.isFavorite
        PHPhotoLibrary.shared().performChanges {
            PHAssetChangeRequest(for: asset).isFavorite = newValue
        }
    }

    /// Saves the original file to ~/Downloads.
    func export(_ asset: PHAsset) async -> URL? {
        guard let resource = PHAssetResource.assetResources(for: asset).first(where: { [.photo, .video, .fullSizePhoto, .fullSizeVideo].contains($0.type) })
                ?? PHAssetResource.assetResources(for: asset).first else { return nil }
        let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
        var target = downloads.appending(path: resource.originalFilename)
        var n = 2
        while FileManager.default.fileExists(atPath: target.path) {
            let base = (resource.originalFilename as NSString).deletingPathExtension
            let ext = (resource.originalFilename as NSString).pathExtension
            target = downloads.appending(path: "\(base) \(n).\(ext)")
            n += 1
        }
        let opts = PHAssetResourceRequestOptions()
        opts.isNetworkAccessAllowed = true
        let dest = target
        do {
            try await PHAssetResourceManager.default().writeData(for: resource, toFile: dest, options: opts)
            return dest
        } catch { return nil }
    }

    /// Pictures taken on this day in earlier years — "Neste dia".
    func onThisDay(limit: Int = 12) -> [PHAsset] {
        guard isAuthorized else { return [] }
        let cal = Calendar.current
        let today = Date().startOfDay
        var preds: [NSPredicate] = []
        for back in 1...20 {
            guard let start = cal.date(byAdding: .year, value: -back, to: today) else { continue }
            preds.append(NSPredicate(format: "creationDate >= %@ AND creationDate < %@", start as NSDate, start.adding(days: 1) as NSDate))
        }
        let opts = PHFetchOptions()
        opts.predicate = NSCompoundPredicate(orPredicateWithSubpredicates: preds)
        opts.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        opts.fetchLimit = limit
        let r = PHAsset.fetchAssets(with: .image, options: opts)
        return r.objects(at: IndexSet(0..<r.count))
    }

    func recent(limit: Int) -> [PHAsset] {
        guard isAuthorized else { return [] }
        let opts = PHFetchOptions()
        opts.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        opts.fetchLimit = limit
        let r = PHAsset.fetchAssets(with: .image, options: opts)
        return r.objects(at: IndexSet(0..<r.count))
    }
}
