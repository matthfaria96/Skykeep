import QuickLook
import SwiftUI
import UniformTypeIdentifiers

struct DriveView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.drive.available {
            DriveScreen()
        } else {
            ContentUnavailableView {
                Label("iCloud Drive Desligado", systemImage: "icloud.slash")
            } description: {
                Text("Ligue em Ajustes do Sistema › [seu nome] › iCloud › iCloud Drive.")
            } actions: {
                Button("Abrir Ajustes do iCloud…") {
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.systempreferences.AppleIDSettings:icloud")!)
                }
            }
        }
    }
}

private struct DriveScreen: View {
    @Environment(AppModel.self) private var model
    @AppStorage("driveGrid") private var grid = true
    @State private var query = ""
    @State private var preview: URL?
    @State private var renaming: DriveItem?
    @State private var newName = ""
    @State private var dropTargeted = false

    var body: some View {
        @Bindable var drive = model.drive
        let items = query.isEmpty ? drive.items : drive.items.filter { $0.name.localizedStandardContains(query) }

        VStack(spacing: 0) {
            pathBar
            Divider().opacity(0.6)
            Group {
                if let error = drive.error {
                    EmptyState(symbol: "exclamationmark.triangle", title: "Não deu para abrir esta pasta", message: error)
                } else if items.isEmpty {
                    EmptyState(symbol: "folder", title: query.isEmpty ? "Pasta vazia" : "Nada encontrado",
                               message: query.isEmpty ? "Arraste arquivos para cá para enviá-los ao iCloud Drive." : nil)
                } else if grid {
                    ScrollView {
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 120, maximum: 150), spacing: 14)], spacing: 18) {
                            ForEach(items) { item in
                                GridTile(item: item, selected: drive.selection.contains(item.url))
                                    .onTapGesture(count: 2) { activate(item) }
                                    .simultaneousGesture(TapGesture().onEnded { select(item) })
                                    .contextMenu { menu(item) }
                                    .draggable(item.url)
                            }
                        }
                        .padding(20)
                    }
                    .contentShape(Rectangle())
                    .onTapGesture { drive.selection = [] }
                } else {
                    Table(items, selection: $drive.selection) {
                        TableColumn("Nome") { item in
                            HStack(spacing: 8) {
                                FileIcon(item: item, side: 20)
                                Text(item.name).lineLimit(1)
                            }
                        }
                        .width(min: 220, ideal: 360)
                        TableColumn("Status") { item in StatusGlyph(state: item.state) }.width(50)
                        TableColumn("Modificado") { item in
                            Text(item.modified == .distantPast ? "—" : Fmt.short(item.modified) + " " + Fmt.time(item.modified)).foregroundStyle(.secondary)
                        }
                        .width(min: 120, ideal: 150)
                        TableColumn("Tamanho") { item in
                            Text(item.isFolder ? "—" : Fmt.bytes(item.size)).foregroundStyle(.secondary).monospacedDigit()
                        }
                        .width(min: 70, ideal: 90)
                        TableColumn("Tipo") { item in Text(item.kind).foregroundStyle(.secondary).lineLimit(1) }
                    }
                    .contextMenu(forSelectionType: URL.self) { urls in
                        if let u = urls.first, let item = drive.items.first(where: { $0.url == u }) { menu(item) }
                    } primaryAction: { urls in
                        if let u = urls.first, let item = drive.items.first(where: { $0.url == u }) { activate(item) }
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .overlay {
                if dropTargeted {
                    RoundedRectangle(cornerRadius: 14).strokeBorder(Color.cyan, style: StrokeStyle(lineWidth: 2, dash: [8, 5]))
                        .background(Color.cyan.opacity(0.06), in: RoundedRectangle(cornerRadius: 14))
                        .overlay { Label("Soltar para enviar ao iCloud Drive", systemImage: "icloud.and.arrow.up").font(.title3.weight(.semibold)).foregroundStyle(.cyan) }
                        .padding(12)
                        .allowsHitTesting(false)
                }
            }
            .dropDestination(for: URL.self) { urls, _ in
                drive.importFiles(urls.filter(\.isFileURL))
                return true
            } isTargeted: { dropTargeted = $0 }
        }
        .searchable(text: $query, placement: .toolbar, prompt: "Buscar nesta pasta")
        .toolbar {
            ToolbarItemGroup(placement: .navigation) {
                Button { drive.goBack() } label: { Image(systemName: "chevron.left") }.disabled(!drive.canGoBack)
                Button { drive.goForward() } label: { Image(systemName: "chevron.right") }.disabled(!drive.canGoForward)
            }
            ToolbarItemGroup {
                Picker("Visualização", selection: $grid) {
                    Image(systemName: "square.grid.2x2").tag(true)
                    Image(systemName: "list.bullet").tag(false)
                }
                .pickerStyle(.segmented)
                Button { drive.newFolder() } label: { Label("Nova pasta", systemImage: "folder.badge.plus") }
            }
        }
        .quickLookPreview($preview, in: items.filter { !$0.isFolder }.map(\.url))
        .onKeyPress(.space) {
            if let u = drive.selection.first, drive.items.first(where: { $0.url == u })?.isFolder == false { preview = u; return .handled }
            return .ignored
        }
        .onKeyPress(.return) {
            if let u = drive.selection.first, let item = drive.items.first(where: { $0.url == u }) { activate(item); return .handled }
            return .ignored
        }
        .onAppear { drive.start() }
        .navigationTitle(drive.displayName(drive.current))
        .navigationSubtitle("\(drive.items.count) itens")
        .onChange(of: model.newItemRequest) { drive.newFolder() }
        .alert("Renomear", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Nome", text: $newName)
            Button("Renomear") { if let r = renaming { _ = drive.rename(r.url, to: newName) } }
            Button("Cancelar", role: .cancel) {}
        }
    }

    private func select(_ item: DriveItem) {
        if NSEvent.modifierFlags.contains(.command) {
            if model.drive.selection.contains(item.url) { model.drive.selection.remove(item.url) } else { model.drive.selection.insert(item.url) }
        } else {
            model.drive.selection = [item.url]
        }
    }

    /// Folders open in place; files open in Quick Look, inside Skykeep.
    private func activate(_ item: DriveItem) {
        if item.isFolder { model.drive.open(item.url) } else { preview = item.url }
    }

    private var pathBar: some View {
        let drive = model.drive
        return HStack(spacing: 4) {
            ForEach(Array(drive.pathComponents.enumerated()), id: \.offset) { i, url in
                if i > 0 { Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary) }
                Button {
                    if url != drive.current { drive.open(url) }
                } label: {
                    HStack(spacing: 4) {
                        if i == 0 { Image(systemName: "icloud.fill").foregroundStyle(.cyan) }
                        Text(drive.displayName(url))
                    }
                }
                .buttonStyle(.plain)
                .font(i == drive.pathComponents.count - 1 ? .headline : .body)
                .foregroundStyle(i == drive.pathComponents.count - 1 ? .primary : .secondary)
            }
            Spacer()
            Text("\(drive.items.count) itens").font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
    }

    @ViewBuilder
    private func menu(_ item: DriveItem) -> some View {
        let drive = model.drive
        let targets = drive.selection.contains(item.url) ? Array(drive.selection) : [item.url]
        Button(item.isFolder ? "Abrir" : "Visualizar") { activate(item) }
        if !item.isFolder { Button("Abrir com o App Padrão") { NSWorkspace.shared.open(item.url) } }
        Button("Mostrar no Finder") { drive.reveal(targets) }
        Divider()
        if item.state == .cloud { Button("Baixar agora") { targets.forEach(drive.download) } }
        if item.state == .local && !item.isFolder { Button("Remover download") { targets.forEach(drive.evict) } }
        Button("Renomear…") { newName = item.name; renaming = item }
        Button("Copiar caminho") { Clipboard.copy(item.url.path) }
        ShareLink(item: item.url) { Text("Compartilhar…") }
        Divider()
        Button("Mover para o Lixo", role: .destructive) { drive.trash(targets) }
    }
}

private struct GridTile: View {
    let item: DriveItem
    let selected: Bool

    var body: some View {
        VStack(spacing: 6) {
            FileIcon(item: item, side: 76)
                .frame(width: 92, height: 80)
                .padding(4)
                .background(selected ? Color.primary.opacity(0.1) : .clear, in: RoundedRectangle(cornerRadius: 8))
            HStack(spacing: 3) {
                if item.state != .local { StatusGlyph(state: item.state) }
                Text(item.name)
                    .font(.callout)
                    .lineLimit(2)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .foregroundStyle(selected ? .white : .primary)
                    .background(selected ? Color.accentColor : .clear, in: RoundedRectangle(cornerRadius: 4))
            }
        }
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .help(item.name)
    }
}

struct FileIcon: View {
    @Environment(AppModel.self) private var model
    let item: DriveItem
    let side: CGFloat
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image).resizable().scaledToFit()
            } else {
                Image(nsImage: NSWorkspace.shared.icon(forFile: item.url.path)).resizable().scaledToFit()
            }
        }
        .frame(width: side, height: side)
        .task(id: item.url) { image = await model.drive.thumbnail(item, side: side) }
    }
}

private struct StatusGlyph: View {
    let state: DriveItem.CloudState

    var body: some View {
        switch state {
        case .local: EmptyView()
        case .cloud: Image(systemName: "icloud.and.arrow.down").foregroundStyle(.secondary).help("Só no iCloud — baixa ao abrir")
        case .downloading: ProgressView().controlSize(.mini).help("Baixando")
        case .uploading: Image(systemName: "icloud.and.arrow.up").foregroundStyle(.cyan).symbolEffect(.pulse).help("Enviando")
        }
    }
}
