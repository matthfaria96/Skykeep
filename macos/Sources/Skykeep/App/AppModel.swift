import SwiftUI

/// What the sidebar can select: an app, or one of its lists (a mailbox filter, a reminders
/// list, a notes folder, a photo album, a passwords pane).
enum SidebarItem: Hashable {
    case today, calendar, contacts, drive
    case mail(MailService.Filter)
    case reminders(ReminderSource)
    case notes(String?)
    case photos(PhotoSource)
    case passwords(PasswordPane)

    var section: AppSection {
        switch self {
        case .today: .today
        case .calendar: .calendar
        case .contacts: .contacts
        case .drive: .drive
        case .mail: .mail
        case .reminders: .reminders
        case .notes: .notes
        case .photos: .photos
        case .passwords: .passwords
        }
    }
}

/// One model for the whole app: which section is in front and the services behind each.
/// Services live as long as the app, so switching is instant. Nothing here ever opens
/// another app: every service reads the Mac's data directly.
@MainActor @Observable
final class AppModel {
    var section: AppSection
    var showSearch = false
    /// Bumped by ⌘N; the section in front starts a new item when it changes.
    var newItemRequest = 0
    var passwordPane: PasswordPane = .browsers

    let events = EventKitService()
    let contacts = ContactsService()
    let photos = PhotosService()
    let notes = NotesService()
    let mail = MailService()
    let drive = DriveService()
    let keychain = KeychainService()
    let browsers = BrowserBridge()

    init() {
        let saved = UserDefaults.standard.string(forKey: "startSection").flatMap(AppSection.init(rawValue:))
        let last = UserDefaults.standard.string(forKey: "lastSection").flatMap(AppSection.init(rawValue:))
        section = saved ?? last ?? .today
    }

    func go(_ s: AppSection) {
        section = s
        UserDefaults.standard.set(s.rawValue, forKey: "lastSection")
    }

    /// The sidebar's selection, kept in the services so search and Today can point at a list too.
    var selection: SidebarItem {
        get {
            switch section {
            case .today: .today
            case .calendar: .calendar
            case .contacts: .contacts
            case .drive: .drive
            case .mail: .mail(mail.filter)
            case .reminders: .reminders(events.reminderSource)
            case .notes: .notes(notes.folderID)
            case .photos: .photos(photos.source)
            case .passwords: .passwords(passwordPane)
            }
        }
        set {
            switch newValue {
            case .mail(let f): mail.filter = f
            case .reminders(let s): events.reminderSource = s
            case .notes(let f): notes.folderID = f; notes.selectedID = nil
            case .photos(let p): if photos.source != p { photos.source = p }
            case .passwords(let p): passwordPane = p
            default: break
            }
            go(newValue.section)
        }
    }

    func badge(for s: AppSection) -> Int {
        switch s {
        case .reminders: events.canReadReminders ? events.count(.smart(.today)) : 0
        case .mail: mail.unread
        default: 0
        }
    }
}
