import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @State private var columns: NavigationSplitViewVisibility = .all

    var body: some View {
        @Bindable var model = model
        NavigationSplitView(columnVisibility: $columns) {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 216, max: 260)
        } detail: {
            ZStack {
                content(for: model.section)
                    .id(model.section)
                    .transition(.opacity)
            }
            .animation(.smooth(duration: 0.2), value: model.section)
            .background { Backdrop(tint: model.section.tint).animation(.smooth(duration: 0.35), value: model.section) }
            .navigationTitle(model.section.title)
            .toolbar(removing: .title)
            .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
        }
        .tint(model.section.tint)
        .sheet(isPresented: $model.showSearch) {
            SearchPalette().environment(model)
        }
        .task {
            // Notes and Mail come from iCloud, online. Neither app is ever opened.
            await model.notes.load()
            await model.mail.load()
        }
    }

    @ViewBuilder
    private func content(for s: AppSection) -> some View {
        switch s {
        case .today: TodayView()
        case .mail: MailView()
        case .reminders: RemindersView()
        case .calendar: CalendarView()
        case .notes: NotesView()
        case .contacts: ContactsView()
        case .photos: PhotosView()
        case .drive: DriveView()
        case .passwords: PasswordsView()
        }
    }
}

struct Sidebar: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let selection = Binding<AppSection?>(get: { model.section }, set: { if let s = $0 { model.go(s) } })
        List(selection: selection) {
            row(.today)
            Section("Comunicação") {
                row(.mail)
                row(.contacts)
            }
            Section("Organização") {
                row(.reminders)
                row(.calendar)
                row(.notes)
            }
            Section("Arquivos") {
                row(.photos)
                row(.drive)
            }
            Section("Segurança") {
                row(.passwords)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            Button {
                model.showSearch = true
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                    Text("Buscar em tudo")
                    Spacer()
                    Text("⌘K").font(.caption.monospaced()).foregroundStyle(.tertiary)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .glassEffect(.regular.interactive(), in: .capsule)
            .padding(12)
        }
    }

    private func row(_ s: AppSection) -> some View {
        Label {
            Text(s.title)
        } icon: {
            IconTile(symbol: s.symbol, tint: s.tint, size: 22)
        }
        .badge(model.badge(for: s))
        .tag(s)
    }
}
