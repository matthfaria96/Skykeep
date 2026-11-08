@preconcurrency import EventKit
import SwiftUI

/// Calendar and Reminders, straight from the Mac's own store — the same one Apple's apps use,
/// already in sync with iCloud. Nothing is copied or cached by Skykeep.
@MainActor @Observable
final class EventKitService {
    @ObservationIgnored let store = EKEventStore()

    var eventsStatus = EKEventStore.authorizationStatus(for: .event)
    var remindersStatus = EKEventStore.authorizationStatus(for: .reminder)
    var calendars: [EKCalendar] = []
    var reminderLists: [EKCalendar] = []
    var reminders: [EKReminder] = []
    var remindersLoaded = false
    /// Bumped whenever the store changes or an object is edited in place, so views re-read.
    var revision = 0

    var hiddenCalendarIDs: Set<String> {
        didSet { UserDefaults.standard.set(Array(hiddenCalendarIDs), forKey: "hiddenCalendars") }
    }

    // Navigation targets other screens (search, Today) can set.
    var reminderSource: ReminderSource = .smart(.today)
    var highlightedReminderID: String?
    var calendarFocus = Date()
    var selectedEventID: String?

    @ObservationIgnored private var observer: NSObjectProtocol?

    init() {
        hiddenCalendarIDs = Set(UserDefaults.standard.stringArray(forKey: "hiddenCalendars") ?? [])
        observer = NotificationCenter.default.addObserver(forName: .EKEventStoreChanged, object: store, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.reload() }
        }
        reload()
    }

    var canReadEvents: Bool { eventsStatus == .fullAccess }
    var canReadReminders: Bool { remindersStatus == .fullAccess }
    var eventsDenied: Bool { eventsStatus == .denied || eventsStatus == .restricted || eventsStatus == .writeOnly }
    var remindersDenied: Bool { remindersStatus == .denied || remindersStatus == .restricted || remindersStatus == .writeOnly }

    func requestEvents() async {
        _ = try? await store.requestFullAccessToEvents()
        eventsStatus = EKEventStore.authorizationStatus(for: .event)
        reload()
    }

    func requestReminders() async {
        _ = try? await store.requestFullAccessToReminders()
        remindersStatus = EKEventStore.authorizationStatus(for: .reminder)
        reload()
    }

    func reload() {
        if canReadEvents {
            calendars = store.calendars(for: .event).sorted { ($0.source.title, $0.title) < ($1.source.title, $1.title) }
        }
        if canReadReminders {
            reminderLists = store.calendars(for: .reminder).sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
            Task { await loadReminders() }
        }
        revision += 1
    }

    func loadReminders() async {
        let predicate = store.predicateForReminders(in: nil)
        let store = self.store
        let all: [EKReminder] = await withCheckedContinuation { c in
            store.fetchReminders(matching: predicate) { c.resume(returning: $0 ?? []) }
        }
        reminders = all
        remindersLoaded = true
        revision += 1
    }

    // MARK: Events

    var visibleCalendars: [EKCalendar] { calendars.filter { !hiddenCalendarIDs.contains($0.calendarIdentifier) } }

    func events(from start: Date, to end: Date, allCalendars: Bool = false) -> [EKEvent] {
        _ = revision
        guard canReadEvents else { return [] }
        let cals = allCalendars ? calendars : visibleCalendars
        guard !cals.isEmpty else { return [] }
        let predicate = store.predicateForEvents(withStart: start, end: end, calendars: cals)
        return store.events(matching: predicate).sorted {
            if $0.isAllDay != $1.isAllDay { return $0.isAllDay }
            return $0.startDate < $1.startDate
        }
    }

    func event(id: String) -> EKEvent? { store.event(withIdentifier: id) }

    var defaultEventCalendar: EKCalendar? { store.defaultCalendarForNewEvents ?? calendars.first { $0.allowsContentModifications } }

    @discardableResult
    func save(_ event: EKEvent, span: EKSpan = .thisEvent) -> String? {
        do { try store.save(event, span: span, commit: true); revision += 1; return nil }
        catch { return error.localizedDescription }
    }

    @discardableResult
    func delete(_ event: EKEvent, span: EKSpan = .thisEvent) -> String? {
        do { try store.remove(event, span: span, commit: true); revision += 1; return nil }
        catch { return error.localizedDescription }
    }

    // MARK: Reminders

    var defaultReminderList: EKCalendar? { store.defaultCalendarForNewReminders() ?? reminderLists.first }

    func toggle(_ reminder: EKReminder) {
        reminder.isCompleted.toggle()
        persist(reminder)
    }

    func persist(_ reminder: EKReminder) {
        try? store.save(reminder, commit: true)
        revision += 1
    }

    @discardableResult
    func addReminder(title: String, list: EKCalendar?, due: DateComponents? = nil, priority: Int = 0, notes: String? = nil) -> EKReminder? {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let calendar = list ?? defaultReminderList else { return nil }
        let r = EKReminder(eventStore: store)
        r.title = trimmed
        r.calendar = calendar
        r.dueDateComponents = due
        r.priority = priority
        r.notes = notes
        if let due, due.hour != nil, let date = Calendar.current.date(from: due) {
            r.addAlarm(EKAlarm(absoluteDate: date))
        }
        do {
            try store.save(r, commit: true)
            reminders.append(r)
            revision += 1
            return r
        } catch { return nil }
    }

    func delete(_ reminder: EKReminder) {
        try? store.remove(reminder, commit: true)
        reminders.removeAll { $0.calendarItemIdentifier == reminder.calendarItemIdentifier }
        revision += 1
    }

    func setDue(_ reminder: EKReminder, _ date: Date?, withTime: Bool = false) {
        reminder.alarms?.forEach { reminder.removeAlarm($0) }
        if let date {
            let comps: Set<Calendar.Component> = withTime ? [.year, .month, .day, .hour, .minute] : [.year, .month, .day]
            reminder.dueDateComponents = Calendar.current.dateComponents(comps, from: date)
            if withTime { reminder.addAlarm(EKAlarm(absoluteDate: date)) }
        } else {
            reminder.dueDateComponents = nil
        }
        persist(reminder)
    }

    func reminders(in source: ReminderSource, showCompleted: Bool, keep: Set<String> = []) -> [EKReminder] {
        _ = revision
        let endOfToday = Date().startOfDay.adding(days: 1)
        let list: [EKReminder]
        switch source {
        case .smart(.today):
            list = reminders.filter { r in
                guard let d = r.dueDate else { return false }
                return (!r.isCompleted || keep.contains(r.calendarItemIdentifier)) && d < endOfToday
            }
        case .smart(.scheduled):
            list = reminders.filter { ($0.dueDate != nil) && (!$0.isCompleted || keep.contains($0.calendarItemIdentifier) || showCompleted) }
        case .smart(.all):
            list = reminders.filter { !$0.isCompleted || showCompleted || keep.contains($0.calendarItemIdentifier) }
        case .smart(.important):
            list = reminders.filter { (1...4).contains($0.priority) && (!$0.isCompleted || showCompleted || keep.contains($0.calendarItemIdentifier)) }
        case .smart(.completed):
            return reminders.filter(\.isCompleted).sorted { ($0.completionDate ?? .distantPast) > ($1.completionDate ?? .distantPast) }
        case .list(let id):
            list = reminders.filter { $0.calendar.calendarIdentifier == id && (!$0.isCompleted || showCompleted || keep.contains($0.calendarItemIdentifier)) }
        }
        return list.sorted { a, b in
            if a.isCompleted != b.isCompleted { return !a.isCompleted }
            switch (a.dueDate, b.dueDate) {
            case let (x?, y?) where x != y: return x < y
            case (_?, nil): return true
            case (nil, _?): return false
            default: return (a.creationDate ?? .distantPast) < (b.creationDate ?? .distantPast)
            }
        }
    }

    func count(_ source: ReminderSource) -> Int { reminders(in: source, showCompleted: false).count }
}

enum SmartList: String, CaseIterable, Hashable {
    case today, scheduled, all, important, completed

    var title: String {
        switch self {
        case .today: "Hoje"
        case .scheduled: "Agendados"
        case .all: "Todos"
        case .important: "Importantes"
        case .completed: "Concluídos"
        }
    }

    var symbol: String {
        switch self {
        case .today: "calendar.circle.fill"
        case .scheduled: "calendar.badge.clock"
        case .all: "tray.fill"
        case .important: "exclamationmark.circle.fill"
        case .completed: "checkmark.circle.fill"
        }
    }

    var tint: Color {
        switch self {
        case .today: .blue
        case .scheduled: .red
        case .all: .gray
        case .important: .orange
        case .completed: .secondary
        }
    }
}

enum ReminderSource: Hashable {
    case smart(SmartList)
    case list(String)
}

extension EKReminder {
    var dueDate: Date? {
        guard let c = dueDateComponents else { return nil }
        return Calendar.current.date(from: c)
    }
    var dueHasTime: Bool { dueDateComponents?.hour != nil }
    var isOverdue: Bool {
        guard !isCompleted, let d = dueDate else { return false }
        return dueHasTime ? d < Date() : d < Date().startOfDay
    }
}

extension EKCalendar {
    var tint: Color { Color(nsColor: color ?? .systemBlue) }
}

extension EKEvent {
    /// A video-call link in the event, if it has one — Meet, Zoom, Teams, FaceTime, Webex.
    var meetingURL: URL? {
        let hosts = ["meet.google.com", "zoom.us", "teams.microsoft.com", "teams.live.com", "facetime.apple.com", "webex.com", "whereby.com"]
        var candidates: [URL] = []
        if let url { candidates.append(url) }
        let text = [location, notes].compactMap { $0 }.joined(separator: "\n")
        if let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) {
            detector.enumerateMatches(in: text, range: NSRange(text.startIndex..., in: text)) { m, _, _ in
                if let u = m?.url { candidates.append(u) }
            }
        }
        return candidates.first { u in hosts.contains { (u.host ?? "").hasSuffix($0) } }
    }
}

/// "pagar conta amanhã 9h" → title "pagar conta", due tomorrow at 09:00.
enum QuickParse {
    static func reminder(_ text: String) -> (title: String, due: DateComponents?) {
        var title = text
        var day: Date?
        var hour: Int?
        var minute = 0
        let lower = text.lowercased()

        let dayWords: [(String, Int)] = [("depois de amanhã", 2), ("amanhã", 1), ("amanha", 1), ("hoje", 0)]
        for (word, offset) in dayWords where lower.contains(word) {
            day = Date().startOfDay.adding(days: offset)
            if let r = title.range(of: word, options: .caseInsensitive) { title.removeSubrange(r) }
            break
        }

        // "às 9", "as 14:30", "9h", "9h30", "14:30"
        let patterns = [#"\b(?:às|as)\s+(\d{1,2})(?:[:h](\d{2}))?h?\b"#, #"\b(\d{1,2})(?:h(\d{2})?|:(\d{2}))(?!\d)"#]
        for p in patterns {
            guard let re = try? NSRegularExpression(pattern: p, options: .caseInsensitive),
                  let m = re.firstMatch(in: title, range: NSRange(title.startIndex..., in: title)),
                  let hr = Range(m.range(at: 1), in: title), let h = Int(title[hr]), h < 24 else { continue }
            hour = h
            for g in 2..<m.numberOfRanges {
                if let mr = Range(m.range(at: g), in: title), let mm = Int(title[mr]), mm < 60 { minute = mm }
            }
            if let whole = Range(m.range, in: title) { title.removeSubrange(whole) }
            break
        }

        if day == nil, hour == nil,
           let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.date.rawValue),
           let m = detector.firstMatch(in: text, range: NSRange(text.startIndex..., in: text)), let date = m.date,
           let r = Range(m.range, in: text) {
            title = text
            title.removeSubrange(r)
            let comps = Calendar.current.dateComponents([.hour, .minute], from: date)
            day = date.startOfDay
            if comps.hour != 0 || comps.minute != 0 { hour = comps.hour; minute = comps.minute ?? 0 }
        }

        if hour != nil, day == nil { day = Date().startOfDay }
        title = title.replacingOccurrences(of: "  ", with: " ").trimmingCharacters(in: .whitespacesAndNewlines.union(.punctuationCharacters))
        if title.isEmpty { title = text }
        guard let day else { return (title, nil) }
        var comps = Calendar.current.dateComponents([.year, .month, .day], from: day)
        if let hour { comps.hour = hour; comps.minute = minute }
        return (title, comps)
    }
}
