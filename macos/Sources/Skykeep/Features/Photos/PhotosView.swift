@preconcurrency import Photos
import AVKit
import SwiftUI

struct PhotosView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let svc = model.photos
        if svc.isAuthorized {
            PhotosScreen()
        } else {
            PermissionGate(
                section: .photos,
                message: "Sua biblioteca do Fotos do iCloud, com álbuns, favoritos e vídeos. As fotos originais só são baixadas quando você abre.",
                denied: svc.denied,
                privacyAnchor: "Privacy_Photos",
                request: { await svc.request() }
            )
        }
    }
}

private struct PhotosScreen: View {
    @Environment(AppModel.self) private var model
    @AppStorage("photoTile") private var tile: Double = 150
    @State private var viewer: Int?
    @State private var saved: String?

    var body: some View {
        let svc = model.photos
        HStack(spacing: 0) {
            SourceColumn(width: 220) { sources }
            ZStack {
            content
            if let i = viewer, i < svc.assets.count {
                PhotoViewer(index: i, close: { withAnimation(.smooth(duration: 0.2)) { viewer = nil } }, move: { viewer = $0 }, onSaved: notifySaved)
                    .transition(.opacity)
                    .zIndex(1)
            }
            }
        }
        .overlay(alignment: .bottom) {
            if let saved { Toast(text: "\(saved) salvo em Downloads").padding(20) }
        }
        .toolbar {
            ToolbarItemGroup {
                if svc.source == .hidden && svc.hiddenUnlocked {
                    Button { svc.lockHidden() } label: { Label("Trancar", systemImage: "lock") }
                }
                Slider(value: $tile, in: 80...300) {
                    Text("Tamanho")
                } minimumValueLabel: {
                    Image(systemName: "photo").imageScale(.small)
                } maximumValueLabel: {
                    Image(systemName: "photo").imageScale(.large)
                }
                .frame(width: 140)
                .help("Tamanho das miniaturas")
            }
        }
        .onChange(of: svc.source) { viewer = nil }
    }

    private var sources: some View {
        let svc = model.photos
        return ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                SourceHeading(text: "Fotos")
                ForEach(PhotoSource.fixed, id: \.self) { s in
                    SourceRow(title: s.title, symbol: s.symbol, tint: .orange, selected: svc.source == s) { svc.source = s } trailing: {
                        if s == .hidden { Image(systemName: svc.hiddenUnlocked ? "lock.open" : "lock").foregroundStyle(.secondary) }
                    }
                }
                if !svc.albums.isEmpty {
                    SourceHeading(text: "Álbuns")
                    ForEach(svc.albums) { a in
                        SourceRow(title: a.title, symbol: "rectangle.stack", tint: .orange, selected: svc.source == .album(a.id)) {
                            svc.source = .album(a.id)
                        } trailing: { Count(n: a.count) }
                    }
                }
            }
            .padding(10)
        }
    }

    private var title: String {
        let svc = model.photos
        if case .album(let id) = svc.source { return svc.albums.first { $0.id == id }?.title ?? "Álbum" }
        return svc.source.title
    }

    private func notifySaved(_ url: URL?) {
        guard let url else { return }
        withAnimation(.snappy) { saved = url.lastPathComponent }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { withAnimation(.snappy) { saved = nil } }
    }

    @ViewBuilder
    private var content: some View {
        let svc = model.photos
        if svc.source == .hidden && !svc.hiddenUnlocked {
            VStack(spacing: 14) {
                QuietState(symbol: "lock.fill", title: "O álbum Ocultos está trancado", message: "Use o Touch ID ou a senha do Mac para ver.", tint: .orange)
                    .frame(maxHeight: 220)
                Button { Task { await svc.unlockHidden() } } label: { Text("Ver álbum").padding(.horizontal, 8) }
                    .buttonStyle(.glassProminent).tint(.orange).controlSize(.large)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            let count = svc.assets.count
            ScrollView {
                SectionHeader(title: title, eyebrow: "\(count.formatted()) \(count == 1 ? "item" : "itens")", tint: .orange)
                    .padding(.horizontal, 20).padding(.top, 8).padding(.bottom, 12)
                LazyVGrid(columns: [GridItem(.adaptive(minimum: tile, maximum: tile * 1.6), spacing: 2)], spacing: 2) {
                    ForEach(0..<count, id: \.self) { i in
                        let asset = svc.assets.object(at: i)
                        AssetThumb(asset: asset, side: tile)
                            .aspectRatio(1, contentMode: .fill)
                            .onTapGesture(count: 2) { withAnimation(.smooth(duration: 0.2)) { viewer = i } }
                            .contextMenu {
                                Button("Abrir") { viewer = i }
                                Button(asset.isFavorite ? "Remover dos Favoritos" : "Favoritar") { svc.toggleFavorite(asset) }
                                Button("Salvar Original em Downloads") { Task { notifySaved(await svc.export(asset)) } }
                            }
                    }
                }
                .padding(2)
            }
            .overlay {
                if count == 0 { QuietState(symbol: "photo", title: "Nada aqui", tint: .orange) }
            }
        }
    }
}

struct AssetThumb: View {
    @Environment(AppModel.self) private var model
    let asset: PHAsset
    let side: CGFloat
    @State private var image: NSImage?

    var body: some View {
        GeometryReader { geo in
            ZStack {
                Rectangle().fill(Color.primary.opacity(0.06))
                if let image {
                    Image(nsImage: image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: geo.size.width, height: geo.size.height)
                        .clipped()
                        .transition(.opacity)
                }
            }
            .overlay(alignment: .bottom) {
                if asset.mediaType == .video || asset.isFavorite {
                    HStack {
                        if asset.isFavorite { Image(systemName: "heart.fill") }
                        Spacer()
                        if asset.mediaType == .video {
                            Text(Duration.seconds(asset.duration).formatted(.time(pattern: .minuteSecond)))
                        }
                    }
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.white)
                    .shadow(radius: 2)
                    .padding(6)
                    .background(LinearGradient(colors: [.clear, .black.opacity(0.35)], startPoint: .top, endPoint: .bottom))
                }
            }
        }
        .clipped()
        .contentShape(Rectangle())
        .task(id: "\(asset.localIdentifier)\(Int(side / 60))") {
            let scale = NSScreen.main?.backingScaleFactor ?? 2
            let img = await model.photos.thumbnail(asset, side: max(side * 1.3, 160) * scale)
            withAnimation(.easeOut(duration: 0.15)) { image = img }
        }
    }
}

private struct PhotoViewer: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let close: () -> Void
    let move: (Int) -> Void
    var onSaved: (URL?) -> Void = { _ in }
    @State private var showMap = false

    @State private var image: NSImage?
    @State private var player: AVPlayer?
    @State private var showInfo = false
    @State private var saving = false
    @FocusState private var focused: Bool

    var body: some View {
        let svc = model.photos
        let asset = svc.assets.object(at: index)
        ZStack {
            Color.black.ignoresSafeArea()
            if let player {
                VideoPlayer(player: player)
            } else if let image {
                Image(nsImage: image).resizable().scaledToFit()
            } else {
                AssetThumb(asset: asset, side: 300).scaledToFit().blur(radius: 6).opacity(0.6)
                ProgressView().controlSize(.large).tint(.white)
            }
        }
        .overlay(alignment: .top) { topBar(asset) }
        .overlay(alignment: .leading) {
            if index > 0 { arrow("chevron.left") { move(index - 1) } }
        }
        .overlay(alignment: .trailing) {
            if index < svc.assets.count - 1 { arrow("chevron.right") { move(index + 1) } }
        }
        .overlay(alignment: .bottomLeading) {
            if showInfo { info(asset).padding(16).transition(.move(edge: .bottom).combined(with: .opacity)) }
        }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear { focused = true }
        .onKeyPress(.leftArrow) { if index > 0 { move(index - 1) }; return .handled }
        .onKeyPress(.rightArrow) { if index < svc.assets.count - 1 { move(index + 1) }; return .handled }
        .onKeyPress(.escape) { close(); return .handled }
        .onKeyPress(.space) { close(); return .handled }
        .task(id: asset.localIdentifier) {
            image = nil
            player?.pause()
            player = nil
            if asset.mediaType == .video {
                if let item = await svc.playerItem(asset) {
                    let p = AVPlayer(playerItem: item)
                    player = p
                    p.play()
                }
            } else {
                let scale = NSScreen.main?.backingScaleFactor ?? 2
                let size = (NSScreen.main?.frame.size ?? CGSize(width: 1600, height: 1000))
                image = await svc.fullImage(asset, size: CGSize(width: size.width * scale, height: size.height * scale))
            }
        }
        .onDisappear { player?.pause() }
    }

    private func topBar(_ asset: PHAsset) -> some View {
        HStack(spacing: 10) {
            glassButton("xmark", help: "Fechar (Esc)", action: close)
            VStack(alignment: .leading, spacing: 0) {
                if let d = asset.creationDate {
                    Text(Fmt.weekdayDay(d)).font(.headline)
                    Text(Fmt.time(d) + " · " + d.formatted(.dateTime.year())).font(.caption).opacity(0.8)
                }
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .glassEffect(.clear, in: .capsule)
            Spacer()
            glassButton(asset.isFavorite ? "heart.fill" : "heart", help: "Favorito") { model.photos.toggleFavorite(asset) }
            glassButton("info.circle", help: "Informações") { withAnimation(.smooth) { showInfo.toggle() } }
            glassButton(saving ? "hourglass" : "square.and.arrow.down", help: "Salvar original em Downloads") {
                saving = true
                Task {
                    onSaved(await model.photos.export(asset))
                    saving = false
                }
            }
        }
        .padding(14)
    }

    private func glassButton(_ symbol: String, help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: 34, height: 34)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .glassEffect(.clear.interactive(), in: .circle)
        .help(help)
    }

    private func arrow(_ symbol: String, action: @escaping () -> Void) -> some View {
        glassButton(symbol, help: "", action: action).padding(16)
    }

    private func info(_ a: PHAsset) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let d = a.creationDate { Label(Fmt.weekdayDay(d) + ", " + Fmt.time(d), systemImage: "calendar") }
            Label("\(a.pixelWidth) × \(a.pixelHeight) · \(String(format: "%.1f", Double(a.pixelWidth * a.pixelHeight) / 1_000_000)) MP", systemImage: "photo")
            if let resource = PHAssetResource.assetResources(for: a).first {
                Label(resource.originalFilename, systemImage: "doc")
            }
            if let loc = a.location {
                Button { showMap.toggle() } label: { Label("Ver no Mapa", systemImage: "mappin.and.ellipse") }
                    .buttonStyle(.link)
                    .popover(isPresented: $showMap) { PlaceMap(coordinate: loc.coordinate, title: "Foto") }
            }
        }
        .font(.callout)
        .foregroundStyle(.white)
        .padding(14)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .environment(\.colorScheme, .dark)
    }
}
