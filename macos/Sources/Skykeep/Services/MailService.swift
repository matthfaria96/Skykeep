import AppKit
import SwiftUI

struct MailMessage: Identifiable, Hashable {
    /// The message's IMAP UID in its mailbox.
    let id: Int
    let mailbox: String
    let account: String
    let subject: String
    let senderName: String
    let senderEmail: String
    let date: Date
    var read: Bool
    var flagged: Bool
    let summary: String
}

/// A message opened in full.
struct MailBody {
    let html: String?
    let text: String?
    let attachments: [String]
    let messageID: String?
    let replyTo: String?
    let to: [String]
    let cc: [String]

    /// The page shown in the reader: the HTML as sent, or the text laid out as Mail does.
    var page: String {
        let style = """
            <meta charset="utf-8"><style>
            :root { color-scheme: light dark; }
            body { font: 14px -apple-system, system-ui; line-height: 1.5; margin: 22px 28px; word-wrap: break-word; }
            pre { font: 14px -apple-system, system-ui; white-space: pre-wrap; margin: 0; }
            img { max-width: 100%; height: auto; }
            blockquote { border-left: 3px solid #8885; margin-left: 0; padding-left: 12px; color: #888; }
            </style>
            """
        if let html { return style + html }
        let escaped = (text ?? "").replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;").replacingOccurrences(of: ">", with: "&gt;")
        return "\(style)<pre>\(escaped)</pre>"
    }

    /// The text to quote in a reply or forward.
    var quotable: String {
        if let text, !text.isEmpty { return text }
        guard let html, let data = html.data(using: .utf8),
              let a = try? NSAttributedString(data: data, options: [.documentType: NSAttributedString.DocumentType.html, .characterEncoding: String.Encoding.utf8.rawValue], documentAttributes: nil) else { return "" }
        return a.string
    }
}

/// iCloud Mail, online — the same mailbox as the iPhone and icloud.com, through the iCloud
/// helper (IMAP to read, flag, file and delete; SMTP to send). Read state, flags, deletions and
/// sent mail are on the server, so every device agrees. The Mail app is never opened.
@MainActor @Observable
final class MailService {
    enum State: Equatable { case idle, signedOut, loading, ready, failed(String) }
    enum Filter: String, CaseIterable, Hashable { case inbox = "Entrada", unread = "Não lidas", flagged = "Sinalizadas", sent = "Enviadas" }

    static let inbox = "INBOX", sent = "Sent Messages", trash = "Deleted Messages", archive = "Archive"

    var state: State = .idle
    var email: String?
    var messages: [MailMessage] = []
    var sentMessages: [MailMessage] = []
    var unread = 0
    var selectedID: Int?
    var filter: Filter = .inbox { didSet { if filter == .sent, oldValue != .sent { Task { await refresh() } } } }
    var refreshing = false
    var lastError: String?

    @ObservationIgnored private var timer: Timer?

    var connected: Bool { state == .ready }

    var shown: [MailMessage] {
        switch filter {
        case .inbox: messages
        case .unread: messages.filter { !$0.read }
        case .flagged: messages.filter(\.flagged)
        case .sent: sentMessages
        }
    }

    func message(_ id: Int?) -> MailMessage? { shown.first { $0.id == id } ?? messages.first { $0.id == id } }

    // MARK: Account

    func load() async {
        if state == .loading { return }
        if state != .ready { state = .loading }
        do {
            let r = try await CloudBridge.shared.call("mail.status") as? [String: Any]
            email = r?["email"] as? String
            if (r?["configured"] as? Bool) == true {
                state = .ready
                startTimer()
                await refresh()
            } else {
                state = .signedOut
            }
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    func setup(email: String, password: String) async -> String? {
        do {
            _ = try await CloudBridge.shared.call("mail.setup", ["email": email, "password": password])
            state = .idle
            await load()
            return nil
        } catch { return error.localizedDescription }
    }

    func signOut() async {
        _ = try? await CloudBridge.shared.call("mail.signOut")
        messages = []
        sentMessages = []
        unread = 0
        email = nil
        state = .signedOut
    }

    private func startTimer() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 90, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self, NSApp.isActive, self.connected else { return }
                await self.refresh()
            }
        }
    }

    func refresh() async {
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        do {
            let box = filter == .sent ? Self.sent : Self.inbox
            let r = try await CloudBridge.shared.call("mail.list", ["mailbox": box, "limit": 150]) as? [String: Any]
            let list = (r?["messages"] as? [[String: Any]] ?? []).compactMap { Self.message($0, box) }
            if box == Self.sent { sentMessages = list } else {
                messages = list
                unread = (r?["unread"] as? Int) ?? list.filter { !$0.read }.count
            }
            lastError = nil
        } catch {
            lastError = error.localizedDescription
        }
    }

    private static func message(_ m: [String: Any], _ box: String) -> MailMessage? {
        guard let uid = m["uid"] as? Int else { return nil }
        let subject = ((m["subject"] as? String) ?? "").trimmingCharacters(in: .whitespaces)
        return MailMessage(
            id: uid, mailbox: box, account: "",
            subject: subject.isEmpty ? "(sem assunto)" : subject,
            senderName: (m["fromName"] as? String) ?? "", senderEmail: (m["fromAddress"] as? String) ?? "",
            date: Date(timeIntervalSince1970: Double((m["date"] as? Int) ?? 0)),
            read: (m["seen"] as? Bool) ?? true, flagged: (m["flagged"] as? Bool) ?? false,
            summary: (m["preview"] as? String) ?? ""
        )
    }

    // MARK: A message

    func content(of m: MailMessage) async -> MailBody? {
        guard let r = try? await CloudBridge.shared.call("mail.body", ["mailbox": m.mailbox, "uid": m.id]) as? [String: Any] else { return nil }
        return MailBody(html: r["html"] as? String, text: r["text"] as? String, attachments: r["attachments"] as? [String] ?? [],
                        messageID: r["messageId"] as? String, replyTo: r["replyTo"] as? String,
                        to: r["to"] as? [String] ?? [], cc: r["cc"] as? [String] ?? [])
    }

    private func update(_ m: MailMessage, _ change: (inout MailMessage) -> Void) {
        if let i = messages.firstIndex(where: { $0.id == m.id && $0.mailbox == m.mailbox }) { change(&messages[i]) }
        if let i = sentMessages.firstIndex(where: { $0.id == m.id && $0.mailbox == m.mailbox }) { change(&sentMessages[i]) }
    }

    func setRead(_ m: MailMessage, _ read: Bool) {
        guard m.read != read else { return }
        update(m) { $0.read = read }
        if m.mailbox == Self.inbox { unread = max(0, unread + (read ? -1 : 1)) }
        Task { await perform("mail.flag", ["mailbox": m.mailbox, "uid": m.id, "seen": read]) }
    }

    func toggleFlag(_ m: MailMessage) {
        let on = !m.flagged
        update(m) { $0.flagged = on }
        Task { await perform("mail.flag", ["mailbox": m.mailbox, "uid": m.id, "flagged": on]) }
    }

    func delete(_ m: MailMessage) { file(m, to: Self.trash) }
    func archive(_ m: MailMessage) { file(m, to: Self.archive) }

    private func file(_ m: MailMessage, to target: String) {
        if !m.read, m.mailbox == Self.inbox { unread = max(0, unread - 1) }
        let next = shown.firstIndex(where: { $0.id == m.id }).flatMap { i in shown.indices.contains(i + 1) ? shown[i + 1].id : (i > 0 ? shown[i - 1].id : nil) }
        messages.removeAll { $0.id == m.id && $0.mailbox == m.mailbox }
        sentMessages.removeAll { $0.id == m.id && $0.mailbox == m.mailbox }
        if selectedID == m.id { selectedID = next }
        Task { await perform("mail.move", ["mailbox": m.mailbox, "uid": m.id, "target": target]) }
    }

    func send(to: [String], cc: [String], subject: String, text: String, inReplyTo: String?) async -> String? {
        var args: [String: Any] = ["to": to, "cc": cc, "subject": subject, "text": text, "fromName": NSFullUserName()]
        if let inReplyTo { args["inReplyTo"] = inReplyTo }
        do {
            _ = try await CloudBridge.shared.call("mail.send", args)
            if filter == .sent { await refresh() }
            return nil
        } catch { return error.localizedDescription }
    }

    private func perform(_ cmd: String, _ args: [String: Any]) async {
        do { _ = try await CloudBridge.shared.call(cmd, args) } catch {
            lastError = error.localizedDescription
            await refresh()
        }
    }
}
