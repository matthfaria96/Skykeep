@preconcurrency import EventKit
@preconcurrency import Photos
import SwiftUI

/// Início: one tile per app, in the sidebar's order and colours, each showing the preview that
/// matters for it — what's next on the calendar, what's due, unread mail, the latest notes,
/// people, pictures, files and the state of your passwords. Reads only; opens nothing.
struct TodayView: View {
    @Environment(AppModel.self) private var model
    @State private var capture = ""
    @State private var recentFiles: [DriveItem] = []
    @State private var photos: [PHAsset] = []
    @State private var photosAreMemories = false
    @State private var justDone: Set<String> = []
    @State private var width: CGFloat = 900
    @State private var generated = PasswordGenerator.apple()
    @State private var copied = false

    private let gap: CGFloat = 16
    private var col: CGFloat { max(220, (width - gap * 2) / 3) }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header
                captureBar
                VStack(spacing: gap) {
                    HStack(alignment: .top, spacing: gap) {
                        agendaTile.frame(maxWidth: .infinity)
                        remindersTile.frame(width: col)
                    }
                    .frame(height: 330)
                    HStack(alignment: .top, spacing: gap) {
                        mailTile.frame(width: col)
                        notesTile.frame(width: col)
                        contactsTile.frame(maxWidth: .infinity)
                    }
                    .frame(height: 262)
                    HStack(alignment: .top, spacing: gap) {
                        photosTile.frame(maxWidth: .infinity)
                        driveTile.frame(width: col)
                    }
                    .frame(height: 262)
                    passwordsStrip
                }
                .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = $0 }
            }
            .padding(.horizontal, 28)
            .padding(.top, 8)
            .padding(.bottom, 28)
            .frame(maxWidth: 1240)
            .frame(maxWidth: .infinity)
        }
        .scrollEdgeEffectStyle(.soft, for: .top)
        .task {
            let memories = model.photos.onThisDay(limit: 5)
            photosAreMemories = !memories.isEmpty
            photos = memories.isEmpty ? model.photos.recent(limit: 5) : memories
            if model.drive.available { recentFiles = await model.drive.recentFiles(limit: 4) }
            model.browsers.scan()
        }
    }

    // MARK: Header

    private var greeting: String {
        let h = Calendar.current.component(.hour, from: Date())
        let name = NSFullUserName().split(separator: " ").first.map(String.init) ?? ""
        let base = h < 5 ? "Boa noite" : h < 12 ? "Bom dia" : h < 18 ? "Boa tarde" : "Boa noite"
        return name.isEmpty ? base : "\(base), \(name)"
    }

    private var header: some View {
        let ek = model.events
        let eventsLeft = ek.canReadEvents ? ek.events(from: Date(), to: Date().startOfDay.adding(days: 1)).filter { !$0.isAllDay }.count : nil
        let due = ek.canReadReminders ? ek.count(.smart(.today)) : nil
        let unread = model.mail.connected ? model.mail.unread : nil
        return HStack(alignment: .bottom, spacing: 16) {
            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(Fmt.weekdayDay(Date()).uppercased())
                        .font(.system(size: 11, weight: .semibold)).tracking(0.9).foregroundStyle(Color.skyGold)
                    Text(greeting).font(.system(size: 32, weight: .bold))
                }
                HStack(spacing: 8) {
                    if let eventsLeft, eventsLeft > 0 { chip("\(eventsLeft)", eventsLeft == 1 ? "compromisso hoje" : "compromissos hoje", .red) { model.go(.calendar) } }
                    if let due, due > 0 { chip("\(due)", due == 1 ? "lembrete" : "lembretes", .indigo) { ek.reminderSource = .smart(.today); model.go(.reminders) } }
                    if let unread, unread > 0 { chip("\(unread)", unread == 1 ? "e-mail novo" : "e-mails novos", .blue) { model.mail.filter = .unread; model.go(.mail) } }
                    if (eventsLeft ?? 0) == 0 && (due ?? 0) == 0 && (unread ?? 0) == 0 {
                        chip("✓", "Tudo em dia por hoje", .green) {}
                    }
                }
            }
            Spacer()
        }
    }

    private func chip(_ number: String, _ label: String, _ tint: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Text(number).font(.system(size: 13, weight: .bold, design: .rounded)).monospacedDigit()
                Text(label).font(.system(size: 12.5, weight: .medium))
            }
            .foregroundStyle(tint.mix(with: .primary, by: 0.2))
            .padding(.horizontal, 11).padding(.vertical, 5)
            .background(tint.opacity(0.14), in: Capsule())
            .overlay(Capsule().strokeBorder(tint.opacity(0.22)))
        }
        .buttonStyle(.plain)
    }

    private var captureBar: some View {
        HStack(spacing: 12) {
            Image(systemName: "plus.circle.fill").font(.system(size: 20)).foregroundStyle(Color.skyGold)
            TextField("Anotar um lembrete — “pagar o aluguel dia 10”, “ligar pra mãe amanhã 19h”", text: $capture)
                .textFieldStyle(.plain)
                .font(.system(size: 15))
                .onSubmit(addCapture)
            if !capture.isEmpty, let due = QuickParse.reminder(capture).due, let d = Calendar.current.date(from: due) {
                Pill(text: Fmt.relativeDay(d) + (due.hour != nil ? ", " + Fmt.time(d) : ""), symbol: "calendar", tint: .skyGoldDeep)
                    .transition(.scale.combined(with: .opacity))
            }
            Text("↩").font(.callout).foregroundStyle(.tertiary)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
        .glassEffect(.regular.interactive(), in: .capsule)
        .animation(.snappy, value: capture.isEmpty)
    }

    private func addCapture() {
        let p = QuickParse.reminder(capture)
        guard !p.title.isEmpty else { return }
        if model.events.canReadReminders {
            if model.events.addReminder(title: p.title, list: nil, due: p.due) != nil { capture = "" }
        } else {
            Task { await model.events.requestReminders() }
        }
    }

    // MARK: Agenda

    private var agendaTile: some View {
        let ek = model.events
        let now = Date()
        let today = ek.canReadEvents ? ek.events(from: now.startOfDay, to: now.startOfDay.adding(days: 1)).filter { $0.endDate > now && !$0.isAllDay } : []
        return HomeTile(section: .calendar, title: "Agenda", count: today.isEmpty ? nil : "\(today.count) hoje") {
            ek.calendarFocus = Date(); model.go(.calendar)
        } content: {
            if !ek.canReadEvents {
                ConnectButton(title: "Mostrar sua agenda", tint: .red) { await ek.requestEvents() }
                Spacer()
            } else {
                if let next = today.first {
                    NextEvent(event: next)
                    VStack(spacing: 2) {
                        ForEach(today.dropFirst().prefix(2), id: \.self) { e in TimelineRow(event: e) }
                    }
                } else {
                    HStack(spacing: 12) {
                        Image(systemName: "sun.max.fill").font(.system(size: 26)).foregroundStyle(.orange.gradient)
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Dia livre").font(.headline)
                            Text("Nada mais na agenda hoje.").font(.callout).foregroundStyle(.secondary)
                        }
                    }
                    .padding(.vertical, 4)
                    let upcoming = ek.events(from: now.startOfDay.adding(days: 1), to: now.startOfDay.adding(days: 31)).prefix(3)
                    if !upcoming.isEmpty {
                        Text("EM BREVE").font(.system(size: 10, weight: .bold)).tracking(0.8).foregroundStyle(.secondary).padding(.top, 4)
                        VStack(spacing: 2) {
                            ForEach(Array(upcoming), id: \.self) { e in TimelineRow(event: e, showDay: true) }
                        }
                    }
                }
                Spacer(minLength: 0)
                WeekStrip()
            }
        }
    }

    // MARK: Reminders

    private var remindersTile: some View {
        let ek = model.events
        let due = ek.canReadReminders ? ek.reminders(in: .smart(.today), showCompleted: false, keep: justDone) : []
        let overdue = due.filter(\.isOverdue).count
        return HomeTile(section: .reminders, title: "Lembretes", count: due.isEmpty ? nil : "\(due.filter { !$0.isCompleted }.count)") {
            ek.reminderSource = .smart(.today); model.go(.reminders)
        } content: {
            if !ek.canReadReminders {
                ConnectButton(title: "Mostrar seus lembretes", tint: .indigo) { await ek.requestReminders() }
                Spacer()
            } else if due.isEmpty {
                Spacer()
                QuietState(symbol: "checkmark.seal.fill", title: "Tudo em dia", message: "Nada pendente para hoje.", tint: .green, compact: true)
                Spacer()
            } else {
                if overdue > 0 {
                    Pill(text: "\(overdue) atrasado\(overdue == 1 ? "" : "s")", symbol: "exclamationmark.circle.fill", tint: .red)
                }
                VStack(alignment: .leading, spacing: 7) {
                    ForEach(due.prefix(overdue > 0 ? 6 : 7), id: \.calendarItemIdentifier) { r in
                        ReminderRow(reminder: r, showList: false, compact: true) {
                            if !r.isCompleted { justDone.insert(r.calendarItemIdentifier) }
                            withAnimation(.snappy) { ek.toggle(r) }
                        }
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }

    // MARK: Mail

    private var mailTile: some View {
        let mail = model.mail
        let unread = mail.messages.filter { !$0.read }
        let rows = unread.isEmpty ? Array(mail.messages.prefix(3)) : Array(unread.prefix(3))
        return HomeTile(section: .mail, title: "E-mail", count: mail.connected && mail.unread > 0 ? "\(mail.unread) novos" : nil) {
            model.go(.mail)
        } content: {
            if mail.state == .signedOut {
                ConnectButton(title: "Conectar o e-mail do iCloud", tint: .blue) { model.go(.mail) }
                Spacer()
            } else if rows.isEmpty {
                Spacer()
                QuietState(symbol: "tray", title: "Caixa vazia", tint: .blue, compact: true)
                Spacer()
            } else {
                if unread.isEmpty {
                    Text("Tudo lido · últimas mensagens").font(.caption).foregroundStyle(.secondary)
                }
                VStack(spacing: 10) {
                    ForEach(rows) { m in
                        Button {
                            mail.filter = m.read ? .inbox : .unread
                            mail.selectedID = m.id
                            model.go(.mail)
                        } label: {
                            HStack(alignment: .top, spacing: 9) {
                                Avatar(name: m.senderName, imageData: nil, size: 28)
                                VStack(alignment: .leading, spacing: 1) {
                                    HStack {
                                        Text(m.senderName).font(.system(size: 12.5, weight: m.read ? .medium : .bold)).lineLimit(1)
                                        Spacer(minLength: 4)
                                        Text(Fmt.listDate(m.date)).font(.caption2).foregroundStyle(.secondary)
                                    }
                                    Text(m.subject).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                }
                            }
                            .opacity(m.read ? 0.75 : 1)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }

    // MARK: Notes

    private var notesTile: some View {
        let notes = model.notes
        let recent = Array(notes.allNotes.filter { !$0.locked }.prefix(3))
        return HomeTile(section: .notes, title: "Notas", count: notes.connected ? "\(notes.allNotes.count)" : nil) {
            model.go(.notes)
        } content: {
            if notes.state == .signedOut || notes.state == .needsCode {
                ConnectButton(title: "Entrar no iCloud", tint: .orange) { model.go(.notes) }
                Spacer()
            } else if recent.isEmpty {
                Spacer()
                QuietState(symbol: "note.text", title: "Nenhuma nota", tint: .yellow, compact: true)
                Spacer()
            } else {
                VStack(spacing: 6) {
                    ForEach(recent) { n in
                        Button {
                            notes.folderID = nil
                            notes.selectedID = n.id
                            model.go(.notes)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                HStack {
                                    Text(n.title).font(.system(size: 12.5, weight: .semibold)).lineLimit(1)
                                    Spacer(minLength: 4)
                                    Text(Fmt.relativeDay(n.modified)).font(.caption2).foregroundStyle(.secondary)
                                }
                                Text(n.preview.isEmpty ? "Sem texto adicional" : n.preview)
                                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                            }
                            .padding(.horizontal, 10).padding(.vertical, 7)
                            .background(Color.yellow.opacity(0.10), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .overlay(alignment: .leading) {
                                RoundedRectangle(cornerRadius: 2).fill(Color.yellow.gradient).frame(width: 3).padding(.vertical, 8)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }

    // MARK: Contacts

    private var contactsTile: some View {
        let svc = model.contacts
        let birthdays = svc.isAuthorized ? Array(svc.upcomingBirthdays(days: 30).prefix(2)) : []
        let faces = svc.contacts.filter { $0.thumbnailImageData != nil }.prefix(8)
        let people = faces.isEmpty ? svc.contacts.prefix(8) : faces
        return HomeTile(section: .contacts, title: "Contatos", count: svc.isAuthorized ? "\(svc.contacts.count)" : nil) {
            model.go(.contacts)
        } content: {
            if !svc.isAuthorized {
                ConnectButton(title: "Mostrar seus contatos", tint: .brown) { await svc.request() }
                Spacer()
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    if birthdays.isEmpty {
                        Label("Nenhum aniversário este mês", systemImage: "gift")
                            .font(.caption.weight(.medium)).foregroundStyle(.secondary).lineLimit(1)
                    } else {
                        Label("Próximos aniversários", systemImage: "gift.fill")
                            .font(.caption.weight(.semibold)).foregroundStyle(.pink)
                    }
                    ForEach(birthdays, id: \.0.identifier) { c, d in
                        Button { svc.selectedID = c.identifier; model.go(.contacts) } label: {
                            HStack(spacing: 9) {
                                Avatar(name: c.displayName, imageData: c.thumbnailImageData, size: 28)
                                Text(c.displayName).font(.callout).lineLimit(1)
                                Spacer()
                                if d.isToday { Pill(text: "Hoje", symbol: "party.popper.fill", tint: .pink) }
                                else { Text(Fmt.relativeDay(d)).font(.caption).foregroundStyle(.secondary) }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                Spacer(minLength: 0)
                Divider().opacity(0.5)
                HStack(spacing: -6) {
                    ForEach(Array(people), id: \.identifier) { c in
                        Button { svc.selectedID = c.identifier; model.go(.contacts) } label: {
                            Avatar(name: c.displayName, imageData: c.thumbnailImageData, size: 34)
                                .overlay(Circle().strokeBorder(Color(nsColor: .windowBackgroundColor), lineWidth: 2))
                        }
                        .buttonStyle(.plain)
                        .help(c.displayName)
                    }
                    Spacer()
                }
            }
        }
    }

    // MARK: Photos

    private var photosTile: some View {
        HomeTile(section: .photos, title: photosAreMemories ? "Neste dia" : "Fotos recentes", count: nil) {
            model.photos.source = .library; model.go(.photos)
        } content: {
            if !model.photos.isAuthorized {
                ConnectButton(title: "Mostrar suas fotos", tint: .orange) {
                    await model.photos.request()
                    photos = model.photos.recent(limit: 5)
                }
                Spacer()
            } else if photos.isEmpty {
                Spacer()
                QuietState(symbol: "photo", title: "Nenhuma foto ainda", tint: .orange, compact: true)
                Spacer()
            } else {
                GeometryReader { geo in
                    let big = geo.size.height
                    let small = (big - 6) / 2
                    HStack(spacing: 6) {
                        photo(photos[0], big)
                        if photos.count > 1 {
                            LazyHGrid(rows: [GridItem(.fixed(small), spacing: 6), GridItem(.fixed(small), spacing: 6)], spacing: 6) {
                                ForEach(photos.dropFirst().prefix(4), id: \.localIdentifier) { a in photo(a, small) }
                            }
                        }
                        Spacer(minLength: 0)
                    }
                }
            }
        }
    }

    private func photo(_ a: PHAsset, _ side: CGFloat) -> some View {
        AssetThumb(asset: a, side: side)
            .frame(width: side, height: side)
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(alignment: .bottomLeading) {
                if photosAreMemories, side > 120, let d = a.creationDate {
                    let years = Calendar.current.dateComponents([.year], from: d, to: Date()).year ?? 0
                    Text(years == 1 ? "Há 1 ano" : "Há \(years) anos")
                        .font(.caption.weight(.bold)).foregroundStyle(.white)
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .glassEffect(.clear, in: .capsule)
                        .padding(8)
                }
            }
            .contentShape(Rectangle())
            .onTapGesture { model.photos.source = .library; model.go(.photos) }
    }

    // MARK: Drive

    private var driveTile: some View {
        HomeTile(section: .drive, title: "iCloud Drive", count: nil) {
            model.go(.drive)
        } content: {
            if !model.drive.available {
                QuietState(symbol: "icloud.slash", title: "iCloud Drive desligado", tint: .cyan, compact: true)
                Spacer()
            } else if recentFiles.isEmpty {
                Spacer()
                QuietState(symbol: "doc", title: "Nenhum arquivo recente", tint: .cyan, compact: true)
                Spacer()
            } else {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
                    ForEach(recentFiles) { f in
                        Button {
                            model.drive.open(f.url.deletingLastPathComponent())
                            model.drive.selection = [f.url]
                            model.go(.drive)
                        } label: {
                            VStack(spacing: 5) {
                                FileIcon(item: f, side: 44).frame(height: 46)
                                Text(f.name).font(.caption2.weight(.medium)).lineLimit(1)
                                Text(Fmt.relativeDay(f.modified)).font(.caption2).foregroundStyle(.secondary)
                            }
                            .padding(8)
                            .frame(maxWidth: .infinity)
                            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help(f.name)
                    }
                }
                Spacer(minLength: 0)
            }
        }
    }

    // MARK: Passwords

    private var passwordsStrip: some View {
        let bridge = model.browsers
        let connected = bridge.browsers.filter { bridge.status[$0.id]?.connected == true }.map(\.name)
        let pending = bridge.browsers.filter { bridge.status[$0.id]?.connected != true }.map(\.name)
        return HStack(spacing: 14) {
            Button { model.go(.passwords) } label: {
                HStack(spacing: 10) {
                    IconTile(symbol: AppSection.passwords.symbol, tint: .green, size: 30)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Senhas").font(.system(size: 14, weight: .semibold))
                        Group {
                            if !connected.isEmpty {
                                Label("\(connected.joined(separator: ", ")) usando o app Senhas", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                            } else if !pending.isEmpty {
                                Label("Conecte o \(pending[0]) ao app Senhas", systemImage: "link").foregroundStyle(.orange)
                            } else {
                                Text("Protegidas pelo Touch ID").foregroundStyle(.secondary)
                            }
                        }
                        .font(.caption)
                        .lineLimit(1)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 8)
            HStack(spacing: 8) {
                Text(generated)
                    .font(.system(size: 13, weight: .medium, design: .monospaced))
                    .lineLimit(1)
                    .contentTransition(.numericText())
                Button { withAnimation(.snappy) { generated = PasswordGenerator.apple() } } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Gerar outra")
                Button {
                    Clipboard.copySecret(generated)
                    copied = true
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2) { copied = false }
                } label: { Label(copied ? "Copiada" : "Copiar", systemImage: copied ? "checkmark" : "doc.on.doc") }
                    .buttonStyle(.glassProminent).tint(.green).controlSize(.small)
                    .help("Senha forte, sai da área de transferência em 90 segundos")
            }
            .padding(.leading, 12).padding(.trailing, 4).padding(.vertical, 4)
            .background(Color.primary.opacity(0.05), in: Capsule())
        }
        .widget(tint: .green, radius: 18, padding: 12)
    }
}

/// One app's tile on Início: its icon and colour, a count, the preview, and the whole header
/// opens the app.
private struct HomeTile<Content: View>: View {
    let section: AppSection
    let title: String
    let count: String?
    let open: () -> Void
    @ViewBuilder var content: Content
    @State private var hover = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button(action: open) {
                HStack(spacing: 9) {
                    IconTile(symbol: section.symbol, tint: section.tint, size: 24)
                    Text(title).font(.system(size: 14, weight: .semibold))
                    Spacer(minLength: 6)
                    if let count {
                        Text(count)
                            .font(.system(size: 12, weight: .semibold, design: .rounded))
                            .monospacedDigit()
                            .foregroundStyle(section.tint.mix(with: .primary, by: 0.25))
                            .padding(.horizontal, 8).padding(.vertical, 2)
                            .background(section.tint.opacity(0.14), in: Capsule())
                    }
                    Image(systemName: "chevron.right")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(.secondary)
                        .opacity(hover ? 1 : 0.45)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .onHover { hover = $0 }
            .help("Abrir \(section.title)")
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .clipped()
        .widget(tint: section.tint, radius: 20, padding: 16)
    }
}

/// The next thing on the calendar, large: what, when, how long until, where, and a join
/// button for video calls.
private struct NextEvent: View {
    let event: EKEvent
    @State private var showing = false

    var body: some View {
        let e = event
        let now = Date()
        let live = e.startDate <= now
        let minutes = Int(e.startDate.timeIntervalSince(now) / 60)
        let when: String = live ? "Agora" : minutes < 60 ? "em \(minutes) min" : "às \(Fmt.time(e.startDate))"
        HStack(alignment: .top, spacing: 12) {
            RoundedRectangle(cornerRadius: 2).fill(e.calendar.tint.gradient).frame(width: 4)
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(live ? "ACONTECENDO AGORA" : "PRÓXIMO")
                        .font(.system(size: 10, weight: .bold)).tracking(0.8)
                        .foregroundStyle(e.calendar.tint)
                    Spacer()
                    Pill(text: when, tint: live ? .red : .orange)
                }
                Text(e.title ?? "Sem título").font(.system(size: 18, weight: .bold)).lineLimit(1)
                HStack(spacing: 10) {
                    Label("\(Fmt.time(e.startDate)) – \(Fmt.time(e.endDate))", systemImage: "clock")
                    if let loc = e.location, !loc.isEmpty, !loc.contains("://") {
                        Label(loc, systemImage: "mappin").lineLimit(1)
                    }
                }
                .font(.callout)
                .foregroundStyle(.secondary)
            }
            if let url = e.meetingURL {
                Button { NSWorkspace.shared.open(url) } label: { Label("Entrar", systemImage: "video.fill") }
                    .buttonStyle(.glassProminent).tint(.green)
            }
        }
        .padding(12)
        .background(e.calendar.tint.opacity(0.1), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .fixedSize(horizontal: false, vertical: true)
        .contentShape(Rectangle())
        .onTapGesture { showing = true }
        .popover(isPresented: $showing, arrowEdge: .trailing) { EventDetail(event: e) }
    }
}

/// The next seven days at a glance: weekday, date and a dot per event in its calendar's colour.
private struct WeekStrip: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let ek = model.events
        let start = Date().startOfDay
        let days = (0..<7).map { start.adding(days: $0) }
        let events = ek.events(from: start, to: start.adding(days: 7))
        HStack(spacing: 6) {
            ForEach(days, id: \.self) { d in
                let dayEvents = events.filter { $0.startDate < d.adding(days: 1) && $0.endDate > d }
                Button {
                    ek.calendarFocus = d
                    model.go(.calendar)
                } label: {
                    VStack(spacing: 4) {
                        Text(d.formatted(.dateTime.weekday(.abbreviated).locale(Fmt.locale)).prefix(3).capitalized)
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(d.isToday ? .red : .secondary)
                        Text(d.formatted(.dateTime.day()))
                            .font(.system(size: 15, weight: d.isToday ? .bold : .medium, design: .rounded))
                            .foregroundStyle(d.isToday ? .white : .primary)
                            .frame(width: 28, height: 28)
                            .background(d.isToday ? AnyShapeStyle(Color.red.gradient) : AnyShapeStyle(.clear), in: Circle())
                        HStack(spacing: 2) {
                            ForEach(Array(dayEvents.prefix(3).enumerated()), id: \.offset) { _, e in
                                Circle().fill(e.calendar.tint).frame(width: 5, height: 5)
                            }
                        }
                        .frame(height: 5)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 6)
                    .background(Color.primary.opacity(dayEvents.isEmpty ? 0.025 : 0.05), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(dayEvents.isEmpty ? "Nada marcado" : dayEvents.compactMap(\.title).joined(separator: ", "))
            }
        }
    }
}

/// "Permitir" inside a tile: quiet until hovered, in the tile's own colour.
struct ConnectButton: View {
    let title: String
    let tint: Color
    let action: () async -> Void
    @State private var hover = false

    var body: some View {
        Button { Task { await action() } } label: {
            HStack(spacing: 8) {
                Image(systemName: "hand.raised.fill").font(.system(size: 11, weight: .semibold))
                Text(title).font(.callout.weight(.semibold)).lineLimit(1)
                Spacer()
                Image(systemName: "arrow.right").font(.system(size: 11, weight: .bold)).opacity(hover ? 1 : 0.5)
            }
            .foregroundStyle(tint.mix(with: .primary, by: 0.15))
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(tint.opacity(hover ? 0.2 : 0.12), in: RoundedRectangle(cornerRadius: 11, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .animation(.easeOut(duration: 0.12), value: hover)
    }
}

/// One event in a list: time, a bar in the calendar's colour, title and place.
private struct TimelineRow: View {
    let event: EKEvent
    var showDay = false
    @State private var showing = false

    var body: some View {
        let e = event
        HStack(spacing: 10) {
            Text(showDay ? Fmt.relativeDay(e.startDate) : Fmt.time(e.startDate))
                .font(.callout.weight(.semibold).monospacedDigit())
                .lineLimit(1)
                .frame(width: showDay ? 72 : 44, alignment: .trailing)
            Capsule().fill(e.calendar.tint.gradient).frame(width: 3, height: 18)
            Text(e.title ?? "Sem título").font(.callout).lineLimit(1)
            if showDay, !e.isAllDay { Text(Fmt.time(e.startDate)).font(.caption).foregroundStyle(.secondary) }
            Spacer(minLength: 4)
            if let loc = e.location, !loc.isEmpty, !loc.contains("://") {
                Text(loc).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
        }
        .padding(.vertical, 4)
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .onTapGesture { showing = true }
        .popover(isPresented: $showing, arrowEdge: .trailing) { EventDetail(event: e) }
    }
}
