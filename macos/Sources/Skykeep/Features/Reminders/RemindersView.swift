@preconcurrency import EventKit
import SwiftUI

struct RemindersView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let ek = model.events
        if ek.canReadReminders {
            RemindersScreen()
        } else {
            PermissionGate(
                section: .reminders,
                message: "Veja e marque seus lembretes do iCloud aqui, junto com o resto do seu dia.",
                denied: ek.remindersDenied,
                privacyAnchor: "Privacy_Reminders",
                request: { await ek.requestReminders() }
            )
        }
    }
}

private struct RemindersScreen: View {
    @Environment(AppModel.self) private var model
    @State private var showCompleted = false
    /// Ticked in this visit: stays in view, struck through, until another list is opened.
    @State private var justCompleted: Set<String> = []
    @State private var draft = ""
    @FocusState private var addFocused: Bool

    var body: some View {
        let ek = model.events
        HStack(spacing: 0) {
            SourceColumn(width: 250) { sources }
            main
        }
        .onChange(of: ek.reminderSource) { justCompleted = [] }
        .onChange(of: model.newItemRequest) { addFocused = true }
    }

    // MARK: Sources

    private var sources: some View {
        let ek = model.events
        return ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                    ForEach([SmartList.today, .scheduled, .all, .important], id: \.self) { s in
                        SmartTile(list: s, count: ek.count(.smart(s)), selected: ek.reminderSource == .smart(s)) {
                            ek.reminderSource = .smart(s)
                        }
                    }
                }
                SmartTile(list: .completed, count: nil, selected: ek.reminderSource == .smart(.completed), wide: true) {
                    ek.reminderSource = .smart(.completed)
                }
                SourceHeading(text: "Minhas Listas")
                VStack(spacing: 2) {
                    ForEach(ek.reminderLists, id: \.calendarIdentifier) { list in
                        let selected = ek.reminderSource == .list(list.calendarIdentifier)
                        Button {
                            ek.reminderSource = .list(list.calendarIdentifier)
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "list.bullet")
                                    .font(.system(size: 11, weight: .bold))
                                    .foregroundStyle(.white)
                                    .frame(width: 24, height: 24)
                                    .background(list.tint.gradient, in: Circle())
                                Text(list.title).lineLimit(1)
                                Spacer()
                                Count(n: ek.count(.list(list.calendarIdentifier)))
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 6)
                            .background(selected ? list.tint.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .padding(12)
        }
    }

    // MARK: Main

    private var title: String {
        switch model.events.reminderSource {
        case .smart(let s): s.title
        case .list(let id): model.events.reminderLists.first { $0.calendarIdentifier == id }?.title ?? "Lista"
        }
    }

    private var tint: Color {
        switch model.events.reminderSource {
        case .smart(let s): s == .completed ? .gray : s.tint
        case .list(let id): model.events.reminderLists.first { $0.calendarIdentifier == id }?.tint ?? .blue
        }
    }

    private var targetList: EKCalendar? {
        if case .list(let id) = model.events.reminderSource {
            return model.events.reminderLists.first { $0.calendarIdentifier == id }
        }
        return model.events.defaultReminderList
    }

    private var main: some View {
        let ek = model.events
        let source = ek.reminderSource
        let items = ek.reminders(in: source, showCompleted: showCompleted, keep: justCompleted)
        let isList = { if case .list = source { return true } else { return false } }()

        return VStack(spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(.system(size: 30, weight: .bold)).foregroundStyle(tint)
                Spacer()
                Text("\(items.filter { !$0.isCompleted }.count)")
                    .font(.system(size: 30, weight: .semibold, design: .rounded))
                    .foregroundStyle(tint)
                    .monospacedDigit()
                    .contentTransition(.numericText())
            }
            .padding(.horizontal, 24)
            .padding(.top, 8)
            .padding(.bottom, 6)

            if !ek.remindersLoaded {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if items.isEmpty {
                QuietState(symbol: "checkmark.circle", title: source == .smart(.completed) ? "Nada concluído ainda" : "Tudo em dia",
                           message: source == .smart(.completed) ? nil : "Nenhum lembrete aqui. Adicione um abaixo.", tint: tint)
            } else {
                List {
                    ForEach(items, id: \.calendarItemIdentifier) { r in
                        ReminderRow(reminder: r, showList: !isList, highlighted: ek.highlightedReminderID == r.calendarItemIdentifier) {
                            if !r.isCompleted { justCompleted.insert(r.calendarItemIdentifier) }
                            withAnimation(.snappy) { ek.toggle(r) }
                        }
                    }
                }
                .listStyle(.inset)
                .scrollContentBackground(.hidden)
            }

            if source != .smart(.completed) {
                HStack(spacing: 10) {
                    Image(systemName: "plus.circle.fill").font(.title2).foregroundStyle(tint)
                    TextField("Novo lembrete — ex.: “ligar para o banco amanhã 10h”", text: $draft)
                        .textFieldStyle(.plain)
                        .focused($addFocused)
                        .onSubmit(add)
                    if !draft.isEmpty, let due = QuickParse.reminder(draft).due, let d = Calendar.current.date(from: due) {
                        Pill(text: Fmt.relativeDay(d) + (due.hour != nil ? ", " + Fmt.time(d) : ""), symbol: "calendar", tint: tint)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .glassEffect(.regular.interactive(), in: .capsule)
                .padding(16)
            }
        }
        .toolbar {
            ToolbarItem {
                Toggle(isOn: $showCompleted.animation()) {
                    Label("Mostrar concluídos", systemImage: showCompleted ? "eye" : "eye.slash")
                }
                .help("Mostrar lembretes concluídos")
            }
        }
    }

    private func add() {
        let parsed = QuickParse.reminder(draft)
        var due = parsed.due
        if due == nil, model.events.reminderSource == .smart(.today) {
            due = Calendar.current.dateComponents([.year, .month, .day], from: Date())
        }
        let priority = model.events.reminderSource == .smart(.important) ? 1 : 0
        if model.events.addReminder(title: parsed.title, list: targetList, due: due, priority: priority) != nil {
            draft = ""
        }
    }
}

private struct SmartTile: View {
    let list: SmartList
    let count: Int?
    let selected: Bool
    var wide = false
    let action: () -> Void
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let tint = list == .completed ? Color.gray : list.tint
        let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
        Button(action: action) {
            HStack(alignment: wide ? .center : .top, spacing: 8) {
                VStack(alignment: .leading, spacing: 7) {
                    Image(systemName: list.symbol)
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(selected ? tint : .white)
                        .frame(width: 26, height: 26)
                        .background(selected ? AnyShapeStyle(.white) : AnyShapeStyle(tint.gradient), in: Circle())
                    if !wide {
                        Text(list.title)
                            .font(.system(size: 12.5, weight: .semibold))
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                            .foregroundStyle(selected ? .white.opacity(0.95) : .secondary)
                    }
                }
                if wide {
                    Text(list.title)
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundStyle(selected ? .white.opacity(0.95) : .secondary)
                }
                Spacer(minLength: 0)
                if let count {
                    Text("\(count)")
                        .font(.system(size: 22, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(selected ? .white : .primary)
                        .contentTransition(.numericText())
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                if selected {
                    shape.fill(LinearGradient(colors: [tint.mix(with: .white, by: 0.12), tint.mix(with: .black, by: 0.08)], startPoint: .top, endPoint: .bottom))
                } else {
                    shape.fill(scheme == .dark ? Color.white.opacity(0.06) : Color.white)
                }
            }
            .overlay(shape.strokeBorder(selected ? Color.white.opacity(0.2) : Color.primary.opacity(0.07)))
            .shadow(color: selected ? tint.opacity(0.35) : .black.opacity(scheme == .dark ? 0 : 0.05), radius: selected ? 8 : 4, y: selected ? 4 : 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .animation(.snappy(duration: 0.2), value: selected)
    }
}

struct ReminderRow: View {
    @Environment(AppModel.self) private var model
    let reminder: EKReminder
    var showList = true
    var highlighted = false
    var compact = false
    let onToggle: () -> Void

    @State private var title = ""
    @State private var editing = false
    @State private var hovering = false

    var body: some View {
        let r = reminder
        let tint = r.calendar.tint
        HStack(alignment: .top, spacing: 10) {
            Button(action: onToggle) {
                ZStack {
                    Circle().strokeBorder(r.isCompleted ? tint : Color.secondary.opacity(0.55), lineWidth: 1.5)
                    if r.isCompleted { Circle().fill(tint).padding(3.5) }
                }
                .frame(width: 19, height: 19)
                .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .padding(.top, 1)

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 4) {
                    if (1...4).contains(r.priority) {
                        Text("!!!").font(.body.weight(.bold)).foregroundStyle(tint)
                    } else if r.priority == 5 {
                        Text("!!").font(.body.weight(.bold)).foregroundStyle(tint)
                    }
                    if compact {
                        Text(r.title ?? "").lineLimit(1)
                            .foregroundStyle(r.isCompleted ? .secondary : .primary)
                    } else {
                        TextField("", text: $title)
                            .textFieldStyle(.plain)
                            .foregroundStyle(r.isCompleted ? .secondary : .primary)
                            .onSubmit(commitTitle)
                    }
                }
                if !compact, let notes = r.notes, !notes.isEmpty {
                    Text(notes).font(.callout).foregroundStyle(.secondary).lineLimit(2)
                }
                meta
            }
            Spacer(minLength: 0)
            if !compact && (hovering || editing) {
                Button { editing = true } label: { Image(systemName: "info.circle") }
                    .buttonStyle(.plain)
                    .foregroundStyle(tint)
                    .popover(isPresented: $editing, arrowEdge: .trailing) { ReminderEditor(reminder: r) }
            }
        }
        .padding(.vertical, compact ? 2 : 4)
        .opacity(r.isCompleted ? 0.7 : 1)
        .background(highlighted ? tint.opacity(0.12) : .clear, in: RoundedRectangle(cornerRadius: 6))
        .onHover { hovering = $0 }
        .onAppear { title = r.title ?? "" }
        .onChange(of: model.events.revision) { if !editing { title = r.title ?? "" } }
        .contextMenu { menu }
    }

    @ViewBuilder
    private var meta: some View {
        let r = reminder
        let showMeta = r.dueDate != nil || showList || r.url != nil || r.location != nil
        if showMeta {
            HStack(spacing: 8) {
                if let d = r.dueDate {
                    Label(Fmt.relativeDay(d) + (r.dueHasTime ? ", " + Fmt.time(d) : ""), systemImage: r.hasRecurrenceRules ? "repeat" : "calendar")
                        .foregroundStyle(r.isOverdue ? Color.red : Color.secondary)
                }
                if showList {
                    Text(r.calendar.title).foregroundStyle(r.calendar.tint)
                }
                if let loc = r.location, !loc.isEmpty {
                    Label(loc, systemImage: "location").foregroundStyle(.secondary).lineLimit(1)
                }
                if let url = r.url {
                    Link(destination: url) { Label(url.host ?? "link", systemImage: "link") }
                }
            }
            .font(.caption)
            .labelStyle(.titleAndIcon)
        }
    }

    @ViewBuilder
    private var menu: some View {
        let ek = model.events
        let r = reminder
        Button(r.isCompleted ? "Marcar como não concluído" : "Concluir", action: onToggle)
        Divider()
        Menu("Data") {
            Button("Hoje") { ek.setDue(r, Date()) }
            Button("Amanhã") { ek.setDue(r, Date().adding(days: 1)) }
            Button("Próxima semana") { ek.setDue(r, Date().adding(days: 7)) }
            Divider()
            Button("Sem data") { ek.setDue(r, nil) }
        }
        Menu("Prioridade") {
            Button("Alta") { r.priority = 1; ek.persist(r) }
            Button("Média") { r.priority = 5; ek.persist(r) }
            Button("Baixa") { r.priority = 9; ek.persist(r) }
            Button("Nenhuma") { r.priority = 0; ek.persist(r) }
        }
        Menu("Mover para") {
            ForEach(ek.reminderLists.filter { $0.allowsContentModifications }, id: \.calendarIdentifier) { list in
                Button(list.title) { r.calendar = list; ek.persist(r) }
            }
        }
        Divider()
        Button("Apagar", role: .destructive) { ek.delete(r) }
    }

    private func commitTitle() {
        let t = title.trimmingCharacters(in: .whitespaces)
        guard !t.isEmpty, t != reminder.title else { return }
        reminder.title = t
        model.events.persist(reminder)
    }
}

private struct ReminderEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let reminder: EKReminder

    @State private var title = ""
    @State private var notes = ""
    @State private var hasDate = false
    @State private var hasTime = false
    @State private var date = Date()
    @State private var priority = 0
    @State private var listID = ""
    @State private var url = ""

    var body: some View {
        Form {
            TextField("Título", text: $title)
            TextField("Notas", text: $notes, axis: .vertical).lineLimit(2...6)
            TextField("URL", text: $url)
            Toggle("Data", isOn: $hasDate.animation())
            if hasDate {
                DatePicker("Dia", selection: $date, displayedComponents: .date)
                Toggle("Hora", isOn: $hasTime.animation())
                if hasTime { DatePicker("Hora", selection: $date, displayedComponents: .hourAndMinute) }
            }
            Picker("Prioridade", selection: $priority) {
                Text("Nenhuma").tag(0)
                Text("Baixa").tag(9)
                Text("Média").tag(5)
                Text("Alta").tag(1)
            }
            Picker("Lista", selection: $listID) {
                ForEach(model.events.reminderLists.filter { $0.allowsContentModifications }, id: \.calendarIdentifier) {
                    Text($0.title).tag($0.calendarIdentifier)
                }
            }
            HStack {
                Spacer()
                Button("Cancelar") { dismiss() }
                Button("OK", action: save).keyboardShortcut(.defaultAction)
            }
        }
        .formStyle(.grouped)
        .frame(width: 340)
        .onAppear {
            let r = reminder
            title = r.title ?? ""
            notes = r.notes ?? ""
            url = r.url?.absoluteString ?? ""
            hasDate = r.dueDate != nil
            hasTime = r.dueHasTime
            date = r.dueDate ?? Date()
            priority = r.priority == 0 ? 0 : (r.priority <= 4 ? 1 : (r.priority == 5 ? 5 : 9))
            listID = r.calendar.calendarIdentifier
        }
    }

    private func save() {
        let ek = model.events
        let r = reminder
        r.title = title
        r.notes = notes.isEmpty ? nil : notes
        r.url = url.isEmpty ? nil : URL(string: url)
        r.priority = priority
        if let list = ek.reminderLists.first(where: { $0.calendarIdentifier == listID }) { r.calendar = list }
        ek.setDue(r, hasDate ? date : nil, withTime: hasTime)
        dismiss()
    }
}
