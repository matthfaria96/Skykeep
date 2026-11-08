@preconcurrency import EventKit
import SwiftUI

private struct Hit: Identifiable {
    let id: String
    let section: AppSection
    let title: String
    let subtitle: String
    let go: @MainActor () -> Void
}

/// ⌘K: one field over everything — reminders, events, contacts, notes and iCloud Drive.
struct SearchPalette: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var files: [DriveItem] = []
    @State private var cursor = 0
    @FocusState private var focused: Bool

    var body: some View {
        let hits = results
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: "magnifyingglass").font(.title2).foregroundStyle(.secondary)
                TextField("Buscar e-mails, lembretes, eventos, contatos, notas e arquivos", text: $query)
                    .textFieldStyle(.plain)
                    .font(.title2)
                    .focused($focused)
                    .onSubmit { if hits.indices.contains(cursor) { open(hits[cursor]) } }
                    .onKeyPress(.downArrow) { cursor = min(cursor + 1, max(hits.count - 1, 0)); return .handled }
                    .onKeyPress(.upArrow) { cursor = max(cursor - 1, 0); return .handled }
                Text("esc").font(.caption.monospaced()).foregroundStyle(.tertiary)
            }
            .padding(18)
            Divider()
            if query.trimmingCharacters(in: .whitespaces).count < 2 {
                VStack(spacing: 10) {
                    HStack(spacing: 14) {
                        ForEach([AppSection.mail, .reminders, .calendar, .notes, .contacts, .drive], id: \.self) { s in
                            Button { model.go(s); dismiss() } label: {
                                VStack(spacing: 6) {
                                    IconTile(symbol: s.symbol, tint: s.tint, size: 38)
                                    Text(s.title).font(.caption)
                                }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    Text("Digite ao menos duas letras").font(.caption).foregroundStyle(.tertiary)
                }
                .frame(maxWidth: .infinity, minHeight: 160)
            } else if hits.isEmpty {
                Text("Nada encontrado para “\(query)”").foregroundStyle(.secondary).frame(maxWidth: .infinity, minHeight: 160)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(Array(hits.enumerated()), id: \.element.id) { i, h in
                                if i == 0 || hits[i - 1].section != h.section {
                                    Text(h.section.title.uppercased())
                                        .font(.caption2.weight(.semibold)).tracking(0.6)
                                        .foregroundStyle(.secondary)
                                        .padding(.horizontal, 12).padding(.top, i == 0 ? 4 : 12).padding(.bottom, 2)
                                }
                                Button { open(h) } label: {
                                    HStack(spacing: 10) {
                                        IconTile(symbol: h.section.symbol, tint: h.section.tint, size: 24)
                                        VStack(alignment: .leading, spacing: 1) {
                                            Text(h.title).lineLimit(1)
                                            if !h.subtitle.isEmpty {
                                                Text(h.subtitle).font(.caption).lineLimit(1)
                                                    .foregroundStyle(i == cursor ? .white.opacity(0.85) : .secondary)
                                            }
                                        }
                                        Spacer()
                                        if i == cursor { Text("↩").foregroundStyle(.white.opacity(0.8)) }
                                    }
                                    .padding(.horizontal, 12).padding(.vertical, 6)
                                    .foregroundStyle(i == cursor ? .white : .primary)
                                    .background(i == cursor ? Color.accentColor : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .id(h.id)
                                .onHover { if $0 { cursor = i } }
                            }
                        }
                        .padding(8)
                    }
                    .onChange(of: cursor) { if hits.indices.contains(cursor) { proxy.scrollTo(hits[cursor].id) } }
                }
                .frame(height: 380)
            }
        }
        .frame(width: 640)
        .onAppear { focused = true }
        .onChange(of: query) { cursor = 0 }
        .task(id: query) {
            let q = query.trimmingCharacters(in: .whitespaces)
            guard q.count >= 2, model.drive.available else { files = []; return }
            try? await Task.sleep(for: .milliseconds(220))
            guard !Task.isCancelled else { return }
            files = await model.drive.search(q, limit: 12)
        }
    }

    private func open(_ h: Hit) {
        h.go()
        dismiss()
    }

    private var results: [Hit] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard q.count >= 2 else { return [] }
        var out: [Hit] = []
        let model = self.model
        let ek = model.events

        if ek.canReadReminders {
            for r in ek.reminders.lazy.filter({ !$0.isCompleted && (($0.title ?? "").localizedStandardContains(q) || ($0.notes ?? "").localizedStandardContains(q)) }).prefix(6) {
                let listID = r.calendar.calendarIdentifier, rid = r.calendarItemIdentifier
                out.append(Hit(id: "r" + rid, section: .reminders, title: r.title ?? "", subtitle: r.calendar.title + (r.dueDate.map { " · " + Fmt.relativeDay($0) } ?? "")) {
                    ek.reminderSource = .list(listID)
                    ek.highlightedReminderID = rid
                    model.go(.reminders)
                })
            }
        }
        if ek.canReadEvents {
            let now = Date()
            let events = ek.events(from: now.adding(days: -365), to: now.adding(days: 365), allCalendars: true)
                .filter { ($0.title ?? "").localizedStandardContains(q) || ($0.location ?? "").localizedStandardContains(q) }
                .sorted { abs($0.startDate.timeIntervalSince(now)) < abs($1.startDate.timeIntervalSince(now)) }
            for e in events.prefix(6) {
                let start = e.startDate!
                out.append(Hit(id: "e" + occurrenceID(e), section: .calendar, title: e.title ?? "", subtitle: Fmt.weekdayDay(start) + (e.isAllDay ? "" : ", " + Fmt.time(start)) + " · " + e.calendar.title) {
                    ek.calendarFocus = start
                    model.go(.calendar)
                })
            }
        }
        if model.contacts.isAuthorized {
            for c in model.contacts.search(q).prefix(6) {
                let id = c.identifier
                out.append(Hit(id: "c" + id, section: .contacts, title: c.displayName,
                               subtitle: [c.organizationName, (c.phoneNumbers.first?.value.stringValue ?? ""), (c.emailAddresses.first?.value as String? ?? "")].filter { !$0.isEmpty }.joined(separator: " · ")) {
                    model.contacts.selectedID = id
                    model.go(.contacts)
                })
            }
        }
        if model.notes.connected {
            for n in model.notes.search(q).prefix(6) {
                let id = n.id
                out.append(Hit(id: "n\(id)", section: .notes, title: n.title, subtitle: n.preview) {
                    model.selection = .notes(nil)
                    model.notes.selectedID = id
                })
            }
        }
        if model.mail.connected {
            let hits = model.mail.messages.filter { $0.subject.localizedStandardContains(q) || $0.senderName.localizedStandardContains(q) || $0.senderEmail.localizedStandardContains(q) }
            for m in hits.prefix(6) {
                let id = m.id
                out.append(Hit(id: "m\(id)", section: .mail, title: m.subject, subtitle: m.senderName + " · " + Fmt.listDate(m.date)) {
                    model.mail.filter = .inbox
                    model.mail.selectedID = id
                    model.go(.mail)
                })
            }
        }
        for f in files {
            let url = f.url
            out.append(Hit(id: "f" + url.path, section: .drive, title: f.name, subtitle: url.deletingLastPathComponent().path.replacingOccurrences(of: DriveService.root.path, with: "iCloud Drive")) {
                model.drive.open(url.deletingLastPathComponent())
                model.drive.selection = [url]
                model.go(.drive)
            })
        }
        return out
    }
}
