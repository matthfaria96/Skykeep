import AppKit
import SwiftUI

struct NotesView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let notes = model.notes
        switch notes.state {
        case .ready:
            NotesScreen()
        case .idle, .loading:
            VStack(spacing: 12) {
                ProgressView()
                Text("Abrindo suas notas do iCloud…").foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .task { if notes.state == .idle { await notes.load() } }
        case .signedOut, .needsCode:
            ICloudSignIn(section: .notes, message: "Suas notas do iCloud, as mesmas do iPhone: edite, marque checklists e crie notas aqui, direto no iCloud.")
        case .failed(let msg):
            VStack(spacing: 14) {
                QuietState(symbol: "icloud.slash", title: "Não deu para abrir as Notas", message: msg, tint: .yellow)
                    .frame(maxHeight: 260)
                Button("Tentar de novo") { Task { await notes.load() } }.buttonStyle(.glass)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }
}

private struct NotesScreen: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var confirmDelete: NoteSummary?

    var body: some View {
        @Bindable var notes = model.notes
        let shown = query.isEmpty ? notes.notes : notes.notes.filter { $0.title.localizedStandardContains(query) || $0.preview.localizedStandardContains(query) }
        let visibleFolders = notes.folders.filter { !$0.isTrash && ($0.count > 0 || $0.id == notes.folderID) }
        let trash = notes.folders.first(where: \.isTrash)

        HStack(spacing: 0) {
            SourceColumn(width: 210) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        SourceRow(title: "Todas as Notas", symbol: "tray.full", tint: .yellow, selected: notes.folderID == nil) {
                            notes.folderID = nil
                        } trailing: { Count(n: notes.allNotes.filter { $0.folderID != notes.trashID }.count) }
                        SourceHeading(text: "iCloud")
                        ForEach(visibleFolders) { f in
                            SourceRow(title: f.name, symbol: "folder", tint: .yellow, selected: notes.folderID == f.id) {
                                notes.folderID = f.id
                            } trailing: { Count(n: f.count) }
                        }
                        if let trash {
                            SourceRow(title: trash.name, symbol: "trash", tint: .yellow, selected: notes.folderID == trash.id) {
                                notes.folderID = trash.id
                            } trailing: { Count(n: trash.count) }
                            .padding(.top, 8)
                        }
                    }
                    .padding(10)
                }
                HStack(spacing: 6) {
                    if notes.syncing { ProgressView().controlSize(.mini) }
                    Text(notes.syncing ? "Sincronizando…" : (notes.lastError == nil ? "Em dia com o iCloud" : "Sem conexão agora"))
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                }
                .padding(.horizontal, 14).padding(.vertical, 10)
            }

            VStack(spacing: 0) {
                SearchField(prompt: "Buscar notas", text: $query)
                    .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 6)
                List(selection: $notes.selectedID) {
                    ForEach(groupByAge(shown), id: \.0) { group, items in
                        Section(group) {
                            ForEach(items) { n in
                                VStack(alignment: .leading, spacing: 3) {
                                    HStack(spacing: 4) {
                                        if n.pinned { Image(systemName: "pin.fill").font(.caption2).foregroundStyle(.orange) }
                                        if n.locked { Image(systemName: "lock.fill").font(.caption).foregroundStyle(.secondary) }
                                        Text(n.title).font(.headline).lineLimit(1)
                                    }
                                    HStack(spacing: 6) {
                                        Text(n.modified.isToday ? Fmt.time(n.modified) : Fmt.listDate(n.modified))
                                            .foregroundStyle(.primary.opacity(0.75))
                                        Text(n.locked ? "Nota trancada" : (n.preview.isEmpty ? "Sem texto adicional" : n.preview))
                                            .foregroundStyle(.secondary).lineLimit(1)
                                    }
                                    .font(.callout)
                                    if notes.folderID == nil, !n.folderName.isEmpty {
                                        Label(n.folderName, systemImage: "folder").font(.caption).foregroundStyle(.tertiary)
                                    }
                                }
                                .padding(.vertical, 4)
                                .tag(n.id)
                                .contextMenu { menu(n) }
                            }
                        }
                    }
                }
                .listStyle(.inset)
                .scrollContentBackground(.hidden)
                .overlay {
                    if shown.isEmpty {
                        QuietState(symbol: "note.text", title: query.isEmpty ? "Nenhuma nota" : "Nada encontrado", tint: .yellow, compact: true)
                    }
                }
            }
            .frame(width: 300)
            .overlay(alignment: .trailing) { Rectangle().fill(Color.primary.opacity(0.07)).frame(width: 1) }

            Group {
                if let id = notes.selectedID, let n = notes.allNotes.first(where: { $0.id == id }) {
                    NoteDetail(note: n).id(id)
                } else {
                    QuietState(symbol: "note.text", title: "Nenhuma nota selecionada", tint: .yellow)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .toolbar {
            ToolbarItemGroup {
                Button { Task { await notes.sync() } } label: { Label("Sincronizar", systemImage: "arrow.clockwise") }
                    .help("Buscar mudanças no iCloud")
                Button { newNote() } label: { Label("Nova nota", systemImage: "square.and.pencil") }
                    .help("Nova nota (⌘N)")
            }
        }
        .onChange(of: model.newItemRequest) { newNote() }
        .onAppear { if notes.selectedID == nil { notes.selectedID = shown.first?.id } }
        .onChange(of: notes.folderID) { notes.selectedID = notes.notes.first?.id }
        .confirmationDialog("Apagar “\(confirmDelete?.title ?? "")”?", isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } })) {
            Button("Apagar", role: .destructive) {
                if let n = confirmDelete { Task { _ = await notes.delete(n.id) } }
            }
        } message: {
            Text(confirmDelete?.folderID == notes.trashID ? "Ela será apagada de vez, em todos os seus dispositivos." : "Ela vai para Apagadas Recentemente, em todos os seus dispositivos.")
        }
    }

    @ViewBuilder
    private func menu(_ n: NoteSummary) -> some View {
        let notes = model.notes
        Menu("Mover para") {
            ForEach(notes.folders.filter { !$0.isTrash && $0.id != n.folderID }) { f in
                Button(f.name) { Task { _ = await notes.move(n.id, to: f.id) } }
            }
        }
        Divider()
        Button("Apagar…", role: .destructive) { confirmDelete = n }
    }

    private func newNote() {
        Task { _ = await model.notes.create(body: "Nova nota\n") }
    }

    private func groupByAge(_ notes: [NoteSummary]) -> [(String, [NoteSummary])] {
        let cal = Calendar.current
        var buckets: [(String, [NoteSummary])] = []
        func put(_ key: String, _ n: NoteSummary) {
            if let i = buckets.firstIndex(where: { $0.0 == key }) { buckets[i].1.append(n) } else { buckets.append((key, [n])) }
        }
        let now = Date()
        for n in notes {
            if n.pinned { put("Fixadas", n); continue }
            let days = cal.dateComponents([.day], from: n.modified.startOfDay, to: now.startOfDay).day ?? 0
            if days == 0 { put("Hoje", n) }
            else if days == 1 { put("Ontem", n) }
            else if days < 7 { put("Últimos 7 dias", n) }
            else if days < 30 { put("Últimos 30 dias", n) }
            else if cal.isDate(n.modified, equalTo: now, toGranularity: .year) { put(n.modified.formatted(.dateTime.month(.wide).locale(Fmt.locale)).capitalizedFirst, n) }
            else { put(String(cal.component(.year, from: n.modified)), n) }
        }
        return buckets
    }
}

/// A note open for reading and writing: edits are saved to iCloud a moment after typing stops,
/// a click on a checkbox ticks it in iCloud, and the Aa menu sets the paragraph's style.
private struct NoteDetail: View {
    @Environment(AppModel.self) private var model
    let note: NoteSummary

    @State private var content = NSAttributedString()
    @State private var version = 0
    @State private var loaded = false
    /// The text as iCloud last had it: what an edit is made over.
    @State private var base: String?
    @State private var baseRuns: [NoteRun] = []
    @State private var pending: String?
    @State private var saveTask: Task<Void, Never>?
    @State private var status: Status = .idle
    @State private var handle = NoteEditorHandle()

    enum Status: Equatable { case idle, saving, saved, failed(String) }

    var body: some View {
        Group {
            if note.locked {
                QuietState(symbol: "lock.fill", title: "Nota trancada", message: "Notas trancadas com senha ficam criptografadas pela Apple e só abrem no app Notas.", tint: .yellow)
            } else if !loaded {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                VStack(spacing: 0) {
                    HStack(spacing: 8) {
                        Text(note.modified.formatted(.dateTime.day().month(.wide).year().hour().minute().locale(Fmt.locale)))
                        Spacer()
                        statusLabel
                    }
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 24)
                    .padding(.top, 12)
                    NoteEditor(text: content, version: version, editable: note.editable && note.folderID != model.notes.trashID,
                               handle: handle, onChange: edited, onToggle: toggle)
                }
            }
        }
        .toolbar {
            ToolbarItem {
                Menu {
                    Button("Título") { style(NoteStyle.title) }
                    Button("Cabeçalho") { style(NoteStyle.heading) }
                    Button("Subcabeçalho") { style(NoteStyle.subheading) }
                    Button("Corpo") { style(NoteStyle.body) }
                    Button("Monoespaçado") { style(NoteStyle.mono) }
                    Divider()
                    Button("Lista com marcadores") { style(NoteStyle.dotted) }
                    Button("Lista com traços") { style(NoteStyle.dashed) }
                    Button("Lista numerada") { style(NoteStyle.numbered) }
                    Divider()
                    Button("Checklist") { style(NoteStyle.checklist) }
                } label: {
                    Label("Formato", systemImage: "textformat")
                }
                .help("Estilo do parágrafo")
                .disabled(!note.editable || note.locked)
            }
            ToolbarItem {
                Button { style(NoteStyle.checklist) } label: { Label("Checklist", systemImage: "checklist") }
                    .help("Transformar o parágrafo em item de checklist")
                    .disabled(!note.editable || note.locked)
            }
        }
        .task(id: note.id) { await reload() }
        .onChange(of: model.notes.revision) {
            // Changes from another device: read again, unless something typed here is on its way.
            if pending == nil, status != .saving { Task { await reload() } }
        }
        .onDisappear { flush() }
    }

    @ViewBuilder
    private var statusLabel: some View {
        switch status {
        case .idle: if !note.editable { Label("Só leitura", systemImage: "eye") }
        case .saving: HStack(spacing: 4) { ProgressView().controlSize(.mini); Text("Salvando…") }
        case .saved: Label("Salvo no iCloud", systemImage: "checkmark.icloud")
        case .failed(let e): Label(e, systemImage: "exclamationmark.icloud").foregroundStyle(.orange).lineLimit(1)
        }
    }

    private func reload() async {
        guard let (body, runs) = await model.notes.body(of: note.id) else { loaded = true; return }
        // Same text and same formatting (a tick on the phone changes only the latter): nothing to redraw.
        if loaded, body == base, runs == baseRuns { return }
        base = body
        baseRuns = runs
        content = NoteFormatter.attributed(body: body, runs: runs)
        version += 1
        loaded = true
    }

    private func edited(_ text: String) {
        pending = text
        saveTask?.cancel()
        saveTask = Task {
            try? await Task.sleep(for: .milliseconds(1100))
            guard !Task.isCancelled else { return }
            await save()
        }
    }

    private func save() async {
        guard let text = pending else { return }
        pending = nil
        status = .saving
        if let e = await model.notes.save(note.id, body: text, base: base) {
            status = .failed(e)
            pending = text
            return
        }
        base = text
        status = .saved
        // Apple may have merged an edit from another device, or carried a list style onto a
        // new line: read the note back while nothing more has been typed.
        if pending == nil, handle.textView?.string == text {
            await reload()
        }
    }

    private func flush() {
        guard pending != nil else { return }
        saveTask?.cancel()
        Task { await save() }
    }

    private func toggle(_ start: Int, _ checked: Bool, _ text: String) {
        Task {
            if pending != nil { saveTask?.cancel(); await save() }
            status = .saving
            if let e = await model.notes.setChecked(note.id, start: start, checked: checked, expect: text) {
                status = .failed(e)
                await reload()
            } else {
                status = .saved
            }
        }
    }

    private func style(_ s: Int) {
        guard let para = handle.textView?.currentParagraph else { return }
        Task {
            if pending != nil { saveTask?.cancel(); await save() }
            status = .saving
            if let e = await model.notes.setStyle(note.id, start: para.start, style: s, expect: para.text) {
                status = .failed(e)
            } else {
                status = .saved
                base = nil
                await reload()
            }
        }
    }
}
