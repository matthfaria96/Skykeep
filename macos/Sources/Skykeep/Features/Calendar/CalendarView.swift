@preconcurrency import EventKit
import SwiftUI

enum CalendarMode: String, CaseIterable {
    case day = "Dia", week = "Semana", month = "Mês", agenda = "Agenda"
}

struct CalendarView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let ek = model.events
        if ek.canReadEvents {
            CalendarScreen()
        } else {
            PermissionGate(
                section: .calendar,
                message: "Seus calendários do iCloud, Google e Exchange — os mesmos do app Calendário — num só lugar.",
                denied: ek.eventsDenied,
                privacyAnchor: "Privacy_Calendars",
                request: { await ek.requestEvents() }
            )
        }
    }
}

/// An occurrence of an event: recurring events share an identifier, so their start tells them apart.
func occurrenceID(_ e: EKEvent) -> String { "\(e.eventIdentifier ?? e.calendarItemIdentifier)|\(e.startDate.timeIntervalSince1970)" }

struct EventEditRequest: Identifiable {
    let id = UUID()
    let event: EKEvent?
    let start: Date
    var allDay = false
}

private struct CalendarScreen: View {
    @Environment(AppModel.self) private var model
    @AppStorage("calendarMode") private var mode: CalendarMode = .week
    @State private var editing: EventEditRequest?

    var body: some View {
        @Bindable var ek = model.events
        HStack(spacing: 0) {
            SourceColumn(width: 226) { sidebar }
            VStack(spacing: 0) {
                header
                Divider().opacity(0.6)
                Group {
                    switch mode {
                    case .day: TimeGrid(days: [ek.calendarFocus.startOfDay], onNew: newEvent, onEdit: edit)
                    case .week: TimeGrid(days: weekDays, onNew: newEvent, onEdit: edit)
                    case .month: MonthGrid(onNew: { newEvent($0, allDay: true) }, onEdit: edit, onOpenDay: { ek.calendarFocus = $0; mode = .day })
                    case .agenda: AgendaList(onEdit: edit)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .sheet(item: $editing) { req in EventEditor(request: req) }
        .onChange(of: model.newItemRequest) { newEvent(nextHalfHour(), allDay: false) }
        .onKeyPress(.leftArrow) { step(-1); return .handled }
        .onKeyPress(.rightArrow) { step(1); return .handled }
        .onKeyPress("t") { ek.calendarFocus = Date(); return .handled }
    }

    private var weekDays: [Date] {
        let cal = Calendar.current
        let start = cal.dateInterval(of: .weekOfYear, for: model.events.calendarFocus)?.start ?? model.events.calendarFocus.startOfDay
        return (0..<7).map { start.adding(days: $0) }
    }

    private func nextHalfHour() -> Date {
        let cal = Calendar.current
        let base = cal.isDate(model.events.calendarFocus, inSameDayAs: Date()) ? Date() : cal.date(bySettingHour: 9, minute: 0, second: 0, of: model.events.calendarFocus)!
        let m = cal.component(.minute, from: base)
        return cal.date(byAdding: .minute, value: m < 30 ? 30 - m : 60 - m, to: base)!.addingTimeInterval(-Double(cal.component(.second, from: base)))
    }

    private func newEvent(_ start: Date, allDay: Bool) { editing = EventEditRequest(event: nil, start: start, allDay: allDay) }
    private func newEvent(_ start: Date) { newEvent(start, allDay: false) }
    private func edit(_ e: EKEvent) { editing = EventEditRequest(event: e, start: e.startDate) }

    private func step(_ dir: Int) {
        let cal = Calendar.current
        let f = model.events.calendarFocus
        switch mode {
        case .day: model.events.calendarFocus = f.adding(days: dir)
        case .week: model.events.calendarFocus = f.adding(days: 7 * dir)
        case .month, .agenda: model.events.calendarFocus = cal.date(byAdding: .month, value: dir, to: f) ?? f
        }
    }

    private var title: String {
        let f = model.events.calendarFocus
        switch mode {
        case .day: return Fmt.weekdayDay(f)
        case .week:
            let d = weekDays
            let first = d.first!, last = d.last!
            if Calendar.current.isDate(first, equalTo: last, toGranularity: .month) { return Fmt.monthYear(first) }
            return first.formatted(.dateTime.month(.abbreviated).locale(Fmt.locale)).capitalizedFirst + " – " + Fmt.monthYear(last)
        case .month, .agenda: return Fmt.monthYear(f)
        }
    }


    /// "nov. 2026" — when the full title doesn't fit beside the controls.
    private var shortTitle: String {
        model.events.calendarFocus.formatted(.dateTime.month(.abbreviated).year().locale(Fmt.locale)).capitalizedFirst
    }

    private var monthOnly: String {
        model.events.calendarFocus.formatted(.dateTime.month(.wide).locale(Fmt.locale)).capitalizedFirst
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 12) {
            ViewThatFits(in: .horizontal) {
                Text(title).font(.system(size: 26, weight: .bold)).lineLimit(1)
                Text(shortTitle).font(.system(size: 24, weight: .bold)).lineLimit(1)
                Text(monthOnly).font(.system(size: 24, weight: .bold)).lineLimit(1)
            }
            .layoutPriority(1)
            Spacer(minLength: 8)
            Picker("", selection: $mode) {
                ForEach(CalendarMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            HStack(spacing: 0) {
                Button { step(-1) } label: { Image(systemName: "chevron.left").frame(width: 24, height: 22) }
                Button("Hoje") { model.events.calendarFocus = Date() }.padding(.horizontal, 4)
                Button { step(1) } label: { Image(systemName: "chevron.right").frame(width: 24, height: 22) }
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 6).padding(.vertical, 4)
            .fixedSize()
            .glassEffect(.regular.interactive(), in: .capsule)
            Button { newEvent(nextHalfHour(), allDay: false) } label: { Image(systemName: "plus").frame(width: 18, height: 18) }
                .buttonStyle(.glassProminent)
                .tint(.red)
                .help("Novo evento (⌘N)")
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 10)
        .fixedSize(horizontal: false, vertical: true)
        .animation(.snappy, value: title)
    }

    private var sidebar: some View {
        @Bindable var ek = model.events
        let grouped = Dictionary(grouping: ek.calendars) { $0.source.title }
        return ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                MiniMonth(focus: $ek.calendarFocus)
                    .padding(.bottom, 8)
                ForEach(grouped.keys.sorted(), id: \.self) { source in
                    SourceHeading(text: CalendarScreen.sourceName(source))
                    ForEach(grouped[source] ?? [], id: \.calendarIdentifier) { cal in
                        let on = !ek.hiddenCalendarIDs.contains(cal.calendarIdentifier)
                        Button {
                            if on { ek.hiddenCalendarIDs.insert(cal.calendarIdentifier) } else { ek.hiddenCalendarIDs.remove(cal.calendarIdentifier) }
                        } label: {
                            HStack(spacing: 9) {
                                ZStack {
                                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                                        .fill(on ? AnyShapeStyle(cal.tint.gradient) : AnyShapeStyle(.clear))
                                    RoundedRectangle(cornerRadius: 5, style: .continuous)
                                        .strokeBorder(cal.tint, lineWidth: on ? 0 : 1.5)
                                    if on { Image(systemName: "checkmark").font(.system(size: 9, weight: .heavy)).foregroundStyle(.white) }
                                }
                                .frame(width: 16, height: 16)
                                Text(cal.title).lineLimit(1).foregroundStyle(on ? .primary : .secondary)
                                Spacer()
                            }
                            .padding(.horizontal, 9).padding(.vertical, 5)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .padding(12)
        }
    }

    static func sourceName(_ s: String) -> String {
        switch s {
        case "Other", "Outros": "Neste Mac"
        case "Subscribed Calendars": "Assinados"
        default: s
        }
    }
}

/// A month as Calendar's sidebar draws it: today in red, the focused day in a soft ring,
/// days with events marked by a dot.
private struct MiniMonth: View {
    @Environment(AppModel.self) private var model
    @Binding var focus: Date
    @State private var shown = Date()

    var body: some View {
        let cal = Calendar.current
        let monthStart = cal.dateInterval(of: .month, for: shown)!.start
        let gridStart = cal.dateInterval(of: .weekOfYear, for: monthStart)!.start
        let days = (0..<42).map { gridStart.adding(days: $0) }
        let events = model.events.events(from: days.first!, to: days.last!.adding(days: 1))
        let busy = Set(events.flatMap { e -> [Date] in
            var out: [Date] = []
            var d = e.startDate.startOfDay
            while d < e.endDate && out.count < 42 { out.append(d); d = d.adding(days: 1) }
            return out
        })
        let symbols = Array(cal.veryShortWeekdaySymbols.rotated(by: cal.firstWeekday - 1))

        VStack(spacing: 6) {
            HStack {
                Text(Fmt.monthYear(shown)).font(.system(size: 13, weight: .semibold))
                Spacer()
                Button { shown = cal.date(byAdding: .month, value: -1, to: shown)! } label: { Image(systemName: "chevron.left") }
                Button { shown = Date(); focus = Date() } label: { Circle().frame(width: 6, height: 6) }
                Button { shown = cal.date(byAdding: .month, value: 1, to: shown)! } label: { Image(systemName: "chevron.right") }
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
            .font(.system(size: 11, weight: .semibold))
            .padding(.horizontal, 4)

            Grid(horizontalSpacing: 0, verticalSpacing: 2) {
                GridRow {
                    ForEach(Array(symbols.enumerated()), id: \.offset) { _, s in
                        Text(s).font(.system(size: 10, weight: .semibold)).foregroundStyle(.tertiary).frame(maxWidth: .infinity)
                    }
                }
                ForEach(0..<6, id: \.self) { row in
                    GridRow {
                        ForEach(0..<7, id: \.self) { col in
                            let d = days[row * 7 + col]
                            let inMonth = cal.isDate(d, equalTo: shown, toGranularity: .month)
                            let isFocus = cal.isDate(d, inSameDayAs: focus)
                            Button { focus = d } label: {
                                VStack(spacing: 1) {
                                    Text(d.formatted(.dateTime.day()))
                                        .font(.system(size: 11.5, weight: d.isToday || isFocus ? .bold : .regular))
                                        .foregroundStyle(d.isToday ? AnyShapeStyle(.white) : (inMonth ? AnyShapeStyle(.primary) : AnyShapeStyle(.tertiary)))
                                        .frame(width: 24, height: 24)
                                        .background {
                                            if d.isToday { Circle().fill(Color.red.gradient) }
                                            else if isFocus { Circle().fill(Color.red.opacity(0.16)) }
                                        }
                                    Circle().fill(busy.contains(d) && inMonth ? Color.secondary : .clear).frame(width: 3, height: 3)
                                }
                                .frame(maxWidth: .infinity)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            }
        }
        .padding(10)
        .widget(radius: 16, padding: 0)
        .onAppear { shown = focus }
        .onChange(of: focus) { _, f in if !cal.isDate(f, equalTo: shown, toGranularity: .month) { shown = f } }
    }
}


// MARK: - Time grid (day and week)

private struct Placed: Identifiable {
    let event: EKEvent
    var col: Int
    var cols: Int
    var id: String { occurrenceID(event) }
}

private func layout(_ events: [EKEvent]) -> [Placed] {
    var result: [Placed] = []
    var cluster: [Placed] = []
    var colEnds: [Date] = []
    var clusterEnd = Date.distantPast
    func flush() {
        for var p in cluster { p.cols = colEnds.count; result.append(p) }
        cluster = []; colEnds = []
    }
    for e in events.sorted(by: { $0.startDate < $1.startDate }) {
        if !cluster.isEmpty && e.startDate >= clusterEnd { flush() }
        let end = max(e.endDate, e.startDate.addingTimeInterval(20 * 60))
        if let idx = colEnds.firstIndex(where: { $0 <= e.startDate }) {
            colEnds[idx] = end
            cluster.append(Placed(event: e, col: idx, cols: 0))
        } else {
            colEnds.append(end)
            cluster.append(Placed(event: e, col: colEnds.count - 1, cols: 0))
        }
        clusterEnd = max(clusterEnd, end)
    }
    flush()
    return result
}

private struct TimeGrid: View {
    @Environment(AppModel.self) private var model
    let days: [Date]
    let onNew: (Date) -> Void
    let onEdit: (EKEvent) -> Void

    private let hour: CGFloat = 52
    private let gutter: CGFloat = 54

    var body: some View {
        let ek = model.events
        let start = days.first!.startOfDay
        let end = days.last!.startOfDay.adding(days: 1)
        let all = ek.events(from: start, to: end)
        let allDay = all.filter(\.isAllDay)
        let timed = all.filter { !$0.isAllDay }

        VStack(spacing: 0) {
            // Day headers
            HStack(spacing: 0) {
                Color.clear.frame(width: gutter)
                ForEach(days, id: \.self) { d in
                    HStack(spacing: 6) {
                        Text(d.formatted(.dateTime.weekday(.abbreviated).locale(Fmt.locale)).capitalizedFirst)
                            .foregroundStyle(.secondary)
                        Text(d.formatted(.dateTime.day()))
                            .fontWeight(.semibold)
                            .foregroundStyle(d.isToday ? .white : .primary)
                            .frame(minWidth: 24, minHeight: 24)
                            .background(d.isToday ? Color.red : .clear, in: Circle())
                    }
                    .frame(maxWidth: .infinity)
                }
            }
            .font(.callout)
            .padding(.vertical, 8)
            .fixedSize(horizontal: false, vertical: true)

            if !allDay.isEmpty {
                HStack(alignment: .top, spacing: 0) {
                    Text("dia todo").font(.caption2).foregroundStyle(.secondary).frame(width: gutter)
                    ForEach(days, id: \.self) { d in
                        VStack(spacing: 2) {
                            ForEach(allDay.filter { $0.startDate < d.adding(days: 1) && $0.endDate > d }, id: \.self) { e in
                                EventChip(event: e, style: .bar, onEdit: onEdit)
                            }
                        }
                        .padding(.horizontal, 2)
                        .frame(maxWidth: .infinity, alignment: .top)
                    }
                }
                .padding(.bottom, 6)
                .fixedSize(horizontal: false, vertical: true)
            }
            Divider()

            ScrollViewReader { proxy in
                ScrollView {
                    ZStack(alignment: .topLeading) {
                        // Hour lines and labels
                        VStack(spacing: 0) {
                            ForEach(0..<24, id: \.self) { h in
                                HStack(alignment: .top, spacing: 0) {
                                    Text(h == 0 ? "" : String(format: "%02d:00", h))
                                        .font(.caption2.monospacedDigit())
                                        .foregroundStyle(.secondary)
                                        .frame(width: gutter - 8, alignment: .trailing)
                                        .padding(.trailing, 8)
                                        .offset(y: -6)
                                    VStack { Divider() }.frame(maxWidth: .infinity, alignment: .top)
                                }
                                .frame(height: hour, alignment: .top)
                                .id(h)
                            }
                        }

                        GeometryReader { geo in
                            let colW = (geo.size.width - gutter) / CGFloat(days.count)
                            ForEach(Array(days.enumerated()), id: \.offset) { i, d in
                                let x = gutter + CGFloat(i) * colW
                                // Day column: double-click makes an event at that time
                                Rectangle()
                                    .fill(d.isToday ? Color.red.opacity(0.025) : .clear)
                                    .contentShape(Rectangle())
                                    .frame(width: colW, height: hour * 24)
                                    .overlay(alignment: .leading) { Rectangle().fill(Color.hairline).frame(width: 1) }
                                    .offset(x: x)
                                    .gesture(SpatialTapGesture(count: 2).onEnded { v in
                                        let minutes = Int(v.location.y / hour * 60) / 30 * 30
                                        onNew(Calendar.current.date(byAdding: .minute, value: minutes, to: d)!)
                                    })

                                let dayEvents = timed.filter { $0.startDate < d.adding(days: 1) && $0.endDate > d }
                                ForEach(layout(dayEvents)) { p in
                                    let s = max(p.event.startDate, d)
                                    let e = min(p.event.endDate, d.adding(days: 1))
                                    let y = CGFloat(s.timeIntervalSince(d) / 3600) * hour
                                    let h = max(22, CGFloat(e.timeIntervalSince(s) / 3600) * hour - 2)
                                    let w = colW / CGFloat(max(p.cols, 1))
                                    EventChip(event: p.event, style: .block, onEdit: onEdit)
                                        .frame(width: w - 3, height: h)
                                        .offset(x: x + CGFloat(p.col) * w + 2, y: y + 1)
                                }

                                if d.isToday {
                                    let now = Date()
                                    let y = CGFloat(now.timeIntervalSince(d) / 3600) * hour
                                    HStack(spacing: 0) {
                                        Circle().fill(.red).frame(width: 8, height: 8)
                                        Rectangle().fill(.red).frame(height: 1.5)
                                    }
                                    .frame(width: colW + 4)
                                    .offset(x: x - 4, y: y - 4)
                                    .allowsHitTesting(false)
                                }
                            }
                        }
                        .frame(height: hour * 24)
                    }
                    .padding(.top, 8)
                }
                .onAppear { proxy.scrollTo(max(0, Calendar.current.component(.hour, from: Date()) - 2), anchor: .top) }
            }
        }
    }
}

// MARK: - Month

private struct MonthGrid: View {
    @Environment(AppModel.self) private var model
    let onNew: (Date) -> Void
    let onEdit: (EKEvent) -> Void
    let onOpenDay: (Date) -> Void

    var body: some View {
        let cal = Calendar.current
        let focus = model.events.calendarFocus
        let monthStart = cal.dateInterval(of: .month, for: focus)!.start
        let gridStart = cal.dateInterval(of: .weekOfYear, for: monthStart)!.start
        let days = (0..<42).map { gridStart.adding(days: $0) }
        let events = model.events.events(from: days.first!, to: days.last!.adding(days: 1))
        let symbols = Array(cal.shortWeekdaySymbols.rotated(by: cal.firstWeekday - 1))

        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ForEach(symbols, id: \.self) { s in
                    Text(s.capitalizedFirst).font(.caption.weight(.semibold)).foregroundStyle(.secondary).frame(maxWidth: .infinity)
                }
            }
            .padding(.vertical, 6)
            Divider()
            Grid(horizontalSpacing: 0, verticalSpacing: 0) {
                ForEach(0..<6, id: \.self) { row in
                    GridRow {
                        ForEach(0..<7, id: \.self) { col in
                            let d = days[row * 7 + col]
                            let inMonth = cal.isDate(d, equalTo: focus, toGranularity: .month)
                            let dayEvents = events.filter { $0.startDate < d.adding(days: 1) && $0.endDate > d }
                            DayCell(day: d, inMonth: inMonth, events: dayEvents, onEdit: onEdit)
                                .contentShape(Rectangle())
                                .onTapGesture(count: 2) { onOpenDay(d) }
                                .contextMenu {
                                    Button("Novo evento") { onNew(d) }
                                    Button("Abrir dia") { onOpenDay(d) }
                                }
                        }
                    }
                }
            }
        }
    }
}

private struct DayCell: View {
    let day: Date
    let inMonth: Bool
    let events: [EKEvent]
    let onEdit: (EKEvent) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Spacer()
                Text(day.formatted(.dateTime.day()))
                    .font(.callout.weight(day.isToday ? .bold : .regular))
                    .foregroundStyle(day.isToday ? AnyShapeStyle(.white) : (inMonth ? AnyShapeStyle(.primary) : AnyShapeStyle(.tertiary)))
                    .frame(minWidth: 22, minHeight: 22)
                    .background(day.isToday ? Color.red : .clear, in: Circle())
            }
            ForEach(events.prefix(3), id: \.self) { e in
                EventChip(event: e, style: e.isAllDay ? .bar : .line, onEdit: onEdit)
            }
            if events.count > 3 {
                Text("mais \(events.count - 3)").font(.caption2).foregroundStyle(.secondary).padding(.leading, 4)
            }
            Spacer(minLength: 0)
        }
        .padding(4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(inMonth ? Color.clear : Color.primary.opacity(0.025))
        .overlay(alignment: .topLeading) {
            Rectangle().strokeBorder(Color.hairline, lineWidth: 0.5)
        }
    }
}

// MARK: - Agenda

private struct AgendaList: View {
    @Environment(AppModel.self) private var model
    let onEdit: (EKEvent) -> Void

    var body: some View {
        let start = model.events.calendarFocus.startOfDay
        let events = model.events.events(from: start, to: start.adding(days: 45))
        let byDay = Dictionary(grouping: events) { max($0.startDate, start).startOfDay }
        if events.isEmpty {
            EmptyState(symbol: "calendar", title: "Nada marcado", message: "Nenhum evento nos próximos 45 dias.")
        } else {
            List {
                ForEach(byDay.keys.sorted(), id: \.self) { d in
                    Section(Fmt.relativeDay(d) == d.formatted(.dateTime.day().month(.abbreviated).locale(Fmt.locale)) ? Fmt.weekdayDay(d) : "\(Fmt.relativeDay(d)) · \(Fmt.dayMonth(d))") {
                        ForEach(byDay[d] ?? [], id: \.self) { e in
                            AgendaRow(event: e, onEdit: onEdit)
                        }
                    }
                }
            }
            .listStyle(.inset)
        }
    }
}

struct AgendaRow: View {
    let event: EKEvent
    var onEdit: ((EKEvent) -> Void)? = nil
    @State private var showing = false

    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 2).fill(event.calendar.tint).frame(width: 4)
            VStack(alignment: .leading, spacing: 2) {
                Text(event.isAllDay ? "dia todo" : Fmt.time(event.startDate)).font(.callout.monospacedDigit())
                if !event.isAllDay { Text(Fmt.time(event.endDate)).font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
            }
            .frame(width: 64, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(event.title ?? "Sem título").fontWeight(.medium)
                if let loc = event.location, !loc.isEmpty {
                    Text(loc).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
            if let url = event.meetingURL {
                Button("Entrar") { NSWorkspace.shared.open(url) }.buttonStyle(.borderedProminent).controlSize(.small)
            }
        }
        .padding(.vertical, 3)
        .contentShape(Rectangle())
        .onTapGesture { showing = true }
        .popover(isPresented: $showing, arrowEdge: .trailing) { EventDetail(event: event, onEdit: onEdit) }
    }
}

// MARK: - Event chip and details

private struct EventChip: View {
    enum Style { case block, bar, line }
    let event: EKEvent
    let style: Style
    let onEdit: (EKEvent) -> Void
    @State private var showing = false

    var body: some View {
        let tint = event.calendar.tint
        Group {
            switch style {
            case .block:
                HStack(spacing: 0) {
                    Rectangle().fill(tint).frame(width: 3)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(event.title ?? "Sem título").font(.caption.weight(.semibold)).lineLimit(2)
                        Text(Fmt.time(event.startDate)).font(.caption2).opacity(0.8)
                        if let loc = event.location, !loc.isEmpty { Text(loc).font(.caption2).lineLimit(1).opacity(0.8) }
                    }
                    .padding(.horizontal, 5)
                    .padding(.vertical, 3)
                    Spacer(minLength: 0)
                }
                .frame(maxHeight: .infinity, alignment: .top)
                .foregroundStyle(tint.mix(with: .primary, by: 0.45))
                .background(tint.opacity(0.18), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                .clipShape(RoundedRectangle(cornerRadius: 5, style: .continuous))
            case .bar:
                Text(event.title ?? "Sem título")
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .foregroundStyle(.white)
                    .background(tint, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
            case .line:
                HStack(spacing: 4) {
                    Circle().fill(tint).frame(width: 6, height: 6)
                    Text(event.title ?? "Sem título").lineLimit(1)
                    Spacer(minLength: 2)
                    Text(Fmt.time(event.startDate)).foregroundStyle(.secondary)
                }
                .font(.caption)
                .padding(.horizontal, 4)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { showing = true }
        .popover(isPresented: $showing, arrowEdge: .trailing) { EventDetail(event: event, onEdit: onEdit) }
        .help(event.title ?? "")
    }
}

struct EventDetail: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var showMap = false
    let event: EKEvent
    var onEdit: ((EKEvent) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                RoundedRectangle(cornerRadius: 2).fill(event.calendar.tint).frame(width: 4, height: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(event.title ?? "Sem título").font(.title3.bold())
                    Text(event.calendar.title).font(.caption).foregroundStyle(event.calendar.tint)
                }
            }
            Label(timeText, systemImage: "clock")
            if let loc = event.location, !loc.isEmpty {
                if event.meetingURL != nil && loc.contains("://") {
                    Label(loc, systemImage: "video").lineLimit(1)
                } else {
                    Button { showMap.toggle() } label: { Label(loc, systemImage: "mappin.and.ellipse") }
                        .buttonStyle(.link)
                        .popover(isPresented: $showMap) { PlaceMap(query: loc) }
                }
            }
            if let attendees = event.attendees, !attendees.isEmpty {
                Label("\(attendees.count) convidado\(attendees.count == 1 ? "" : "s")", systemImage: "person.2")
                    .help(attendees.compactMap(\.name).joined(separator: ", "))
            }
            if let notes = event.notes, !notes.isEmpty {
                ScrollView { Text(notes).font(.callout).foregroundStyle(.secondary).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                    .frame(maxHeight: 120)
            }
            HStack {
                if let url = event.meetingURL {
                    Button { NSWorkspace.shared.open(url) } label: { Label("Entrar na reunião", systemImage: "video.fill") }
                        .buttonStyle(.borderedProminent)
                }
                Spacer()
                if let onEdit, event.calendar.allowsContentModifications {
                    Button("Editar") { dismiss(); onEdit(event) }
                }
            }
        }
        .padding(16)
        .frame(width: 320)
    }

    private var timeText: String {
        if event.isAllDay {
            let days = Calendar.current.dateComponents([.day], from: event.startDate, to: event.endDate).day ?? 0
            return days > 1 ? "\(Fmt.dayMonth(event.startDate)) – \(Fmt.dayMonth(event.endDate.adding(days: -1)))" : "\(Fmt.weekdayDay(event.startDate)), dia todo"
        }
        let sameDay = Calendar.current.isDate(event.startDate, inSameDayAs: event.endDate)
        return sameDay
            ? "\(Fmt.weekdayDay(event.startDate)), \(Fmt.time(event.startDate)) – \(Fmt.time(event.endDate))"
            : "\(Fmt.dayMonth(event.startDate)) \(Fmt.time(event.startDate)) – \(Fmt.dayMonth(event.endDate)) \(Fmt.time(event.endDate))"
    }
}

// MARK: - Editor

private struct EventEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let request: EventEditRequest

    @State private var title = ""
    @State private var location = ""
    @State private var allDay = false
    @State private var start = Date()
    @State private var end = Date()
    @State private var calendarID = ""
    @State private var alert = -1
    @State private var notes = ""
    @State private var url = ""
    @State private var error: String?
    @State private var askSpan: SpanAction?
    @State private var ready = false

    enum SpanAction: Identifiable { case save, delete; var id: Int { self == .save ? 0 : 1 } }

    private let alerts: [(String, Int)] = [("Nenhum", -1), ("Na hora", 0), ("5 min antes", 5), ("15 min antes", 15), ("30 min antes", 30), ("1 hora antes", 60), ("1 dia antes", 1440)]

    var body: some View {
        VStack(spacing: 0) {
            Form {
                TextField("Título", text: $title).font(.title3)
                TextField("Local ou chamada de vídeo", text: $location)
                Toggle("Dia todo", isOn: $allDay)
                DatePicker("Início", selection: $start, displayedComponents: allDay ? .date : [.date, .hourAndMinute])
                DatePicker("Fim", selection: $end, in: start..., displayedComponents: allDay ? .date : [.date, .hourAndMinute])
                Picker("Calendário", selection: $calendarID) {
                    ForEach(model.events.calendars.filter { $0.allowsContentModifications }, id: \.calendarIdentifier) { c in
                        Label { Text(c.title) } icon: { Image(systemName: "circle.fill").foregroundStyle(c.tint) }.tag(c.calendarIdentifier)
                    }
                }
                Picker("Alerta", selection: $alert) {
                    ForEach(alerts, id: \.1) { Text($0.0).tag($0.1) }
                }
                TextField("URL", text: $url)
                TextField("Notas", text: $notes, axis: .vertical).lineLimit(3...8)
            }
            .formStyle(.grouped)
            if let error { Text(error).foregroundStyle(.red).font(.caption).padding(.horizontal) }
            HStack {
                if request.event != nil {
                    Button("Apagar Evento", role: .destructive) {
                        if request.event?.hasRecurrenceRules == true { askSpan = .delete } else { delete(.thisEvent) }
                    }
                }
                Spacer()
                Button("Cancelar") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(request.event == nil ? "Adicionar" : "Salvar") {
                    if request.event?.hasRecurrenceRules == true { askSpan = .save } else { save(.thisEvent) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .padding(16)
        }
        .frame(width: 460)
        .onChange(of: start) { old, new in
            guard ready else { return }
            // Moving the start keeps the length, as Calendar does.
            end = end.addingTimeInterval(new.timeIntervalSince(old))
        }
        .confirmationDialog("Este é um evento que se repete.", isPresented: Binding(get: { askSpan != nil }, set: { if !$0 { askSpan = nil } }), presenting: askSpan) { action in
            Button(action == .save ? "Salvar só este" : "Apagar só este") { action == .save ? save(.thisEvent) : delete(.thisEvent) }
            Button(action == .save ? "Salvar este e os futuros" : "Apagar este e os futuros") { action == .save ? save(.futureEvents) : delete(.futureEvents) }
        }
        .onAppear(perform: load)
    }

    private func load() {
        if let e = request.event {
            title = e.title ?? ""
            location = e.location ?? ""
            allDay = e.isAllDay
            start = e.startDate
            end = e.isAllDay ? e.endDate.addingTimeInterval(-1) : e.endDate
            calendarID = e.calendar.calendarIdentifier
            alert = e.alarms?.first.map { Int(-$0.relativeOffset / 60) } ?? -1
            notes = e.notes ?? ""
            url = e.url?.absoluteString ?? ""
        } else {
            allDay = request.allDay
            start = request.allDay ? request.start.startOfDay : request.start
            end = request.allDay ? start : start.addingTimeInterval(3600)
            calendarID = model.events.defaultEventCalendar?.calendarIdentifier ?? ""
            alert = request.allDay ? -1 : 15
        }
        DispatchQueue.main.async { ready = true }
    }

    private func save(_ span: EKSpan) {
        let ek = model.events
        let e = request.event ?? EKEvent(eventStore: ek.store)
        e.title = title
        e.location = location.isEmpty ? nil : location
        e.isAllDay = allDay
        e.startDate = allDay ? start.startOfDay : start
        e.endDate = allDay ? end.startOfDay.adding(days: 1) : max(end, start.addingTimeInterval(60))
        if let c = ek.calendars.first(where: { $0.calendarIdentifier == calendarID }) { e.calendar = c }
        e.notes = notes.isEmpty ? nil : notes
        e.url = url.isEmpty ? nil : URL(string: url)
        e.alarms?.forEach { e.removeAlarm($0) }
        if alert >= 0 { e.addAlarm(EKAlarm(relativeOffset: TimeInterval(-alert * 60))) }
        if let err = ek.save(e, span: span) { error = err } else { dismiss() }
    }

    private func delete(_ span: EKSpan) {
        guard let e = request.event else { return }
        if let err = model.events.delete(e, span: span) { error = err } else { dismiss() }
    }
}

extension Array {
    func rotated(by n: Int) -> [Element] {
        guard !isEmpty else { return self }
        let k = ((n % count) + count) % count
        return Array(self[k...] + self[..<k])
    }
}
