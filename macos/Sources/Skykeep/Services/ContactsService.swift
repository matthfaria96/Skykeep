@preconcurrency import Contacts
import SwiftUI

/// The Mac's address book — iCloud contacts included, already in sync.
@MainActor @Observable
final class ContactsService {
    @ObservationIgnored let store = CNContactStore()

    var status = CNContactStore.authorizationStatus(for: .contacts)
    var contacts: [CNContact] = []
    var loaded = false
    var selectedID: String?

    @ObservationIgnored private var observer: NSObjectProtocol?

    nonisolated static let keys: [CNKeyDescriptor] = [
        CNContactFormatter.descriptorForRequiredKeys(for: .fullName),
        CNContactIdentifierKey, CNContactGivenNameKey, CNContactFamilyNameKey, CNContactMiddleNameKey,
        CNContactNicknameKey, CNContactOrganizationNameKey, CNContactJobTitleKey, CNContactDepartmentNameKey,
        CNContactPhoneNumbersKey, CNContactEmailAddressesKey, CNContactPostalAddressesKey, CNContactUrlAddressesKey,
        CNContactBirthdayKey, CNContactThumbnailImageDataKey, CNContactImageDataAvailableKey, CNContactTypeKey,
    ].map { $0 as! CNKeyDescriptor }

    init() {
        observer = NotificationCenter.default.addObserver(forName: .CNContactStoreDidChange, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in await self?.load() }
        }
        if isAuthorized { Task { await load() } }
    }

    var isAuthorized: Bool { status == .authorized }
    var denied: Bool { status == .denied || status == .restricted }

    func request() async {
        _ = try? await store.requestAccess(for: .contacts)
        status = CNContactStore.authorizationStatus(for: .contacts)
        if isAuthorized { await load() }
    }

    func load() async {
        guard isAuthorized else { return }
        let store = self.store
        let result: [CNContact] = await Task.detached(priority: .userInitiated) {
            var out: [CNContact] = []
            let req = CNContactFetchRequest(keysToFetch: ContactsService.keys)
            req.sortOrder = .userDefault
            try? store.enumerateContacts(with: req) { c, _ in out.append(c) }
            return out
        }.value
        contacts = result
        loaded = true
    }

    func contact(id: String) -> CNContact? { contacts.first { $0.identifier == id } }

    func search(_ q: String) -> [CNContact] {
        let q = q.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return contacts }
        return contacts.filter { c in
            c.displayName.localizedStandardContains(q)
                || c.organizationName.localizedStandardContains(q)
                || c.emailAddresses.contains { ($0.value as String).localizedStandardContains(q) }
                || c.phoneNumbers.contains { $0.value.stringValue.filter(\.isNumber).contains(q.filter(\.isNumber)) && !q.filter(\.isNumber).isEmpty }
        }
    }

    /// Birthdays in the next `days` days, soonest first.
    func upcomingBirthdays(days: Int = 30) -> [(CNContact, Date)] {
        let cal = Calendar.current
        let today = Date().startOfDay
        return contacts.compactMap { c -> (CNContact, Date)? in
            guard let b = c.birthday, let month = b.month, let day = b.day else { return nil }
            var comps = DateComponents(year: cal.component(.year, from: today), month: month, day: day)
            guard var next = cal.date(from: comps) else { return nil }
            if next < today { comps.year! += 1; next = cal.date(from: comps) ?? next }
            guard let diff = cal.dateComponents([.day], from: today, to: next).day, diff <= days else { return nil }
            return (c, next)
        }
        .sorted { $0.1 < $1.1 }
    }

    func save(_ draft: ContactDraft) -> String? {
        let mutable: CNMutableContact
        let request = CNSaveRequest()
        if let id = draft.id, let existing = contact(id: id) {
            mutable = existing.mutableCopy() as! CNMutableContact
            draft.apply(to: mutable)
            request.update(mutable)
        } else {
            mutable = CNMutableContact()
            draft.apply(to: mutable)
            request.add(mutable, toContainerWithIdentifier: nil)
        }
        do {
            try store.execute(request)
            selectedID = mutable.identifier
            Task { await load() }
            return nil
        } catch { return error.localizedDescription }
    }

    func delete(_ contact: CNContact) {
        let request = CNSaveRequest()
        request.delete(contact.mutableCopy() as! CNMutableContact)
        try? store.execute(request)
        contacts.removeAll { $0.identifier == contact.identifier }
        if selectedID == contact.identifier { selectedID = nil }
    }
}

extension CNContact {
    var displayName: String {
        let name = CNContactFormatter.string(from: self, style: .fullName) ?? ""
        if !name.isEmpty { return name }
        if !organizationName.isEmpty { return organizationName }
        return (emailAddresses.first?.value as String?) ?? phoneNumbers.first?.value.stringValue ?? "Sem nome"
    }

    var sortLetter: String {
        let key = familyName.isEmpty ? displayName : familyName
        let first = key.folding(options: [.diacriticInsensitive, .caseInsensitive], locale: .current).prefix(1).uppercased()
        return first.rangeOfCharacter(from: .letters) != nil ? first : "#"
    }
}

func labelName(_ label: String?) -> String {
    guard let label else { return "outro" }
    return CNLabeledValue<NSString>.localizedString(forLabel: label)
}

struct LabeledEntry: Identifiable, Hashable {
    var id = UUID()
    var label: String
    var value: String
}

struct ContactDraft {
    var id: String?
    var givenName = ""
    var familyName = ""
    var organization = ""
    var jobTitle = ""
    var phones: [LabeledEntry] = []
    var emails: [LabeledEntry] = []
    var hasBirthday = false
    var birthday = Date()

    init() {}

    init(_ c: CNContact) {
        id = c.identifier
        givenName = c.givenName
        familyName = c.familyName
        organization = c.organizationName
        jobTitle = c.jobTitle
        phones = c.phoneNumbers.map { LabeledEntry(label: $0.label ?? CNLabelPhoneNumberMobile, value: $0.value.stringValue) }
        emails = c.emailAddresses.map { LabeledEntry(label: $0.label ?? CNLabelHome, value: $0.value as String) }
        if let b = c.birthday, let d = Calendar.current.date(from: DateComponents(year: b.year ?? 2000, month: b.month, day: b.day)) {
            hasBirthday = true
            birthday = d
        }
    }

    func apply(to c: CNMutableContact) {
        c.givenName = givenName
        c.familyName = familyName
        c.organizationName = organization
        c.jobTitle = jobTitle
        c.phoneNumbers = phones.filter { !$0.value.isEmpty }.map { CNLabeledValue(label: $0.label, value: CNPhoneNumber(stringValue: $0.value)) }
        c.emailAddresses = emails.filter { !$0.value.isEmpty }.map { CNLabeledValue(label: $0.label, value: $0.value as NSString) }
        c.birthday = hasBirthday ? Calendar.current.dateComponents([.year, .month, .day], from: birthday) : nil
    }
}
