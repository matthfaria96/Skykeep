@preconcurrency import EventKit
import SwiftUI

/// The menu bar: today at a glance and a quick reminder, without opening the window.
struct MenuBarView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var draft = ""
    @State private var done: Set<String> = []

    var body: some View {
        let ek = model.events
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 0) {
                    Text(Date().formatted(.dateTime.weekday(.wide).locale(Fmt.locale)).capitalizedFirst)
                        .font(.caption.weight(.semibold)).foregroundStyle(.red)
                    Text(Fmt.dayMonth(Date())).font(.title3.bold())
                }
                Spacer()
                Button { show(.today) } label: { Image(systemName: "macwindow") }
                    .buttonStyle(.borderless).help("Abrir Skykeep")
            }

            if ek.canReadEvents {
                let events = ek.events(from: Date(), to: Date().startOfDay.adding(days: 1))
                if events.isEmpty {
                    Text("Sem mais eventos hoje").font(.callout).foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(events.prefix(4), id: \.self) { e in
                            HStack(spacing: 8) {
                                RoundedRectangle(cornerRadius: 1.5).fill(e.calendar.tint).frame(width: 3, height: 26)
                                VStack(alignment: .leading, spacing: 0) {
                                    Text(e.title ?? "").lineLimit(1)
                                    Text(e.isAllDay ? "dia todo" : "\(Fmt.time(e.startDate)) – \(Fmt.time(e.endDate))").font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if let url = e.meetingURL {
                                    Button("Entrar") { NSWorkspace.shared.open(url) }.controlSize(.small).buttonStyle(.borderedProminent)
                                }
                            }
                        }
                    }
                }
            }

            if ek.canReadReminders {
                Divider()
                let due = ek.reminders(in: .smart(.today), showCompleted: false, keep: done)
                ForEach(due.prefix(6), id: \.calendarItemIdentifier) { r in
                    ReminderRow(reminder: r, showList: false, compact: true) {
                        if !r.isCompleted { done.insert(r.calendarItemIdentifier) }
                        ek.toggle(r)
                    }
                }
                HStack(spacing: 8) {
                    Image(systemName: "plus.circle.fill").foregroundStyle(.blue)
                    TextField("Novo lembrete", text: $draft)
                        .textFieldStyle(.plain)
                        .onSubmit {
                            let p = QuickParse.reminder(draft)
                            if ek.addReminder(title: p.title, list: nil, due: p.due ?? Calendar.current.dateComponents([.year, .month, .day], from: Date())) != nil { draft = "" }
                        }
                }
                .padding(8)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
            }

            Divider()
            HStack(spacing: 14) {
                ForEach([AppSection.mail, .calendar, .reminders, .notes, .contacts, .photos, .drive, .passwords], id: \.self) { s in
                    Button { show(s) } label: { IconTile(symbol: s.symbol, tint: s.tint, size: 24) }
                        .buttonStyle(.plain).help(s.title)
                }
            }
            .frame(maxWidth: .infinity)
        }
        .padding(14)
        .frame(width: 320)
    }

    private func show(_ s: AppSection) {
        model.go(s)
        openWindow(id: "main")
        NSApp.activate()
    }
}
