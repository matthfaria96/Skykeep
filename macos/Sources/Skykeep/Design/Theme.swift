import SwiftUI

// Skykeep's look on the Mac: one continuous surface per window, lit by the colour of the
// section in front; content sits on soft "widget" cards; glass is kept for chrome (sidebar,
// toolbar, floating controls). Everything here is static drawing — no live blur over
// scrolling content — so it costs nothing while scrolling.

extension Color {
    static let skyGold = Color(red: 0.784, green: 0.569, blue: 0.165)
    static let skyGoldDeep = Color(red: 0.663, green: 0.471, blue: 0.122)
    static let cardFill = Color(nsColor: .controlBackgroundColor)
    static let hairline = Color.primary.opacity(0.08)
}

enum AppSection: String, CaseIterable, Identifiable, Codable {
    case today, mail, reminders, calendar, notes, contacts, photos, drive, passwords

    var id: String { rawValue }

    var title: String {
        switch self {
        case .today: "Início"
        case .mail: "E-mail"
        case .reminders: "Lembretes"
        case .calendar: "Calendário"
        case .notes: "Notas"
        case .contacts: "Contatos"
        case .photos: "Fotos"
        case .drive: "iCloud Drive"
        case .passwords: "Senhas"
        }
    }

    var symbol: String {
        switch self {
        case .today: "house.fill"
        case .mail: "envelope.fill"
        case .reminders: "checklist"
        case .calendar: "calendar"
        case .notes: "note.text"
        case .contacts: "person.crop.circle.fill"
        case .photos: "photo.on.rectangle.angled"
        case .drive: "icloud.fill"
        case .passwords: "key.fill"
        }
    }

    var tint: Color {
        switch self {
        case .today: .skyGold
        case .mail: .blue
        case .reminders: .indigo
        case .calendar: .red
        case .notes: .yellow
        case .contacts: .brown
        case .photos: .orange
        case .drive: .cyan
        case .passwords: .green
        }
    }

    var shortcut: KeyEquivalent {
        KeyEquivalent(Character(String((AppSection.allCases.firstIndex(of: self) ?? 0) + 1)))
    }
}

enum PasswordPane: Hashable { case browsers, generator, keychain }

// MARK: - Surfaces

/// The window's background: the system's own colour with a quiet glow of the section's tint
/// coming from the top. Drawn once; nothing animates under scrolling content.
struct Backdrop: View {
    let tint: Color
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        ZStack {
            Color(nsColor: .windowBackgroundColor)
            RadialGradient(colors: [tint.opacity(scheme == .dark ? 0.16 : 0.10), .clear],
                           center: UnitPoint(x: 0.25, y: -0.1), startRadius: 0, endRadius: 720)
            LinearGradient(colors: [tint.opacity(scheme == .dark ? 0.05 : 0.03), .clear], startPoint: .top, endPoint: .center)
        }
        .ignoresSafeArea()
    }
}

/// A card in the style of macOS widgets: softly filled, lit from above with its tint, a fine
/// rim that is brighter on top, and in light mode a wide, low shadow.
struct Widget: ViewModifier {
    var tint: Color = .clear
    var radius: CGFloat = 20
    var padding: CGFloat = 18
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: radius, style: .continuous)
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                ZStack {
                    shape.fill(scheme == .dark ? Color.white.opacity(0.055) : Color.white)
                    shape.fill(LinearGradient(colors: [tint.opacity(scheme == .dark ? 0.16 : 0.09), .clear], startPoint: .top, endPoint: .center))
                }
            }
            .overlay {
                shape.strokeBorder(
                    LinearGradient(colors: scheme == .dark
                                   ? [Color.white.opacity(0.16), Color.white.opacity(0.04)]
                                   : [Color.black.opacity(0.05), Color.black.opacity(0.08)],
                                   startPoint: .top, endPoint: .bottom),
                    lineWidth: 1
                )
            }
            .shadow(color: .black.opacity(scheme == .dark ? 0 : 0.06), radius: 14, y: 6)
    }
}

extension View {
    func widget(tint: Color = .clear, radius: CGFloat = 20, padding: CGFloat = 18) -> some View {
        modifier(Widget(tint: tint, radius: radius, padding: padding))
    }
}

struct Card<Content: View>: View {
    var padding: CGFloat = 16
    var tint: Color = .clear
    @ViewBuilder var content: Content
    var body: some View { content.widget(tint: tint, radius: 16, padding: padding) }
}

/// A coloured rounded square with a white glyph, as System Settings draws its panes.
struct IconTile: View {
    let symbol: String
    let tint: Color
    var size: CGFloat = 22

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.5, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background {
                RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                    .fill(LinearGradient(colors: [tint.mix(with: .white, by: 0.22), tint.mix(with: .black, by: 0.06)], startPoint: .top, endPoint: .bottom))
                    .overlay {
                        RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                            .strokeBorder(Color.white.opacity(0.25), lineWidth: 0.5)
                    }
            }
            .shadow(color: tint.opacity(0.3), radius: size * 0.08, y: size * 0.04)
    }
}

/// What every widget opens with: tile, title, and a quiet chevron to go to the section.
struct CardHeader: View {
    let title: String
    let symbol: String
    let tint: Color
    var trailing: String? = nil
    var action: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: 9) {
            IconTile(symbol: symbol, tint: tint, size: 22)
            Text(title).font(.system(size: 14, weight: .semibold))
            Spacer()
            if let trailing {
                Text(trailing).font(.callout.weight(.medium)).foregroundStyle(.secondary).monospacedDigit()
            }
            if let action {
                Button(action: action) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(.secondary)
                        .frame(width: 22, height: 22)
                        .background(Color.primary.opacity(0.06), in: Circle())
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .help("Abrir \(title)")
            }
        }
    }
}

/// A section's big heading: an eyebrow line in the tint, a large title, and room for controls.
struct SectionHeader<Trailing: View>: View {
    let title: String
    var eyebrow: String? = nil
    var tint: Color = .primary
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .lastTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                if let eyebrow {
                    Text(eyebrow.uppercased())
                        .font(.system(size: 11, weight: .semibold))
                        .tracking(0.9)
                        .foregroundStyle(tint)
                }
                Text(title)
                    .font(.system(size: 28, weight: .bold))
                    .lineLimit(1)
            }
            Spacer(minLength: 12)
            trailing
        }
    }
}

extension SectionHeader where Trailing == EmptyView {
    init(title: String, eyebrow: String? = nil, tint: Color = .primary) {
        self.init(title: title, eyebrow: eyebrow, tint: tint) { EmptyView() }
    }
}

/// An empty state with presence: a large glyph in a lit disc, a title and a line.
struct QuietState: View {
    let symbol: String
    let title: String
    var message: String? = nil
    var tint: Color = .secondary
    var compact = false

    var body: some View {
        VStack(spacing: compact ? 8 : 14) {
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [tint.opacity(0.22), tint.opacity(0.04)], center: .top, startRadius: 0, endRadius: compact ? 34 : 56))
                    .frame(width: compact ? 52 : 92, height: compact ? 52 : 92)
                Image(systemName: symbol)
                    .font(.system(size: compact ? 22 : 38, weight: .regular))
                    .foregroundStyle(tint)
                    .symbolRenderingMode(.hierarchical)
            }
            Text(title).font(compact ? .headline : .title3.weight(.semibold))
            if let message {
                Text(message)
                    .font(compact ? .callout : .body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 340)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: compact ? nil : .infinity)
        .padding(.vertical, compact ? 10 : 0)
    }
}

struct EmptyState: View {
    let symbol: String
    let title: String
    var message: String? = nil
    var body: some View { QuietState(symbol: symbol, title: title, message: message) }
}

/// A search field drawn as Apple draws it in sidebars.
struct SearchField: View {
    let prompt: String
    @Binding var text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass").foregroundStyle(.secondary).font(.system(size: 12, weight: .medium))
            TextField(prompt, text: $text).textFieldStyle(.plain)
            if !text.isEmpty {
                Button { text = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary) }
                    .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 6)
        .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Color.primary.opacity(0.05)))
    }
}

/// A small rounded label: counts, due dates, states.
struct Pill: View {
    let text: String
    var symbol: String? = nil
    var tint: Color = .secondary

    var body: some View {
        HStack(spacing: 4) {
            if let symbol { Image(systemName: symbol) }
            Text(text)
        }
        .font(.caption.weight(.semibold))
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .foregroundStyle(tint)
        .background(tint.opacity(0.13), in: Capsule())
    }
}

/// The big, centred invitation every gate uses: a lit tile, a title, a line, actions.
private struct GateLayout<Actions: View>: View {
    let section: AppSection
    let message: String
    var detail: String? = nil
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(spacing: 18) {
            ZStack {
                Circle()
                    .fill(RadialGradient(colors: [section.tint.opacity(0.28), .clear], center: .center, startRadius: 10, endRadius: 90))
                    .frame(width: 180, height: 180)
                IconTile(symbol: section.symbol, tint: section.tint, size: 76)
            }
            .padding(.bottom, -20)
            Text(section.title).font(.system(size: 30, weight: .bold))
            Text(message)
                .font(.title3)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 460)
            if let detail {
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            }
            actions
            Label("Tudo fica no seu Mac. Nada passa por servidores do Skykeep.", systemImage: "lock.fill")
                .font(.footnote)
                .foregroundStyle(.tertiary)
                .padding(.top, 6)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// What a section shows until macOS has let it read the data behind it.
struct PermissionGate: View {
    let section: AppSection
    let message: String
    let denied: Bool
    let privacyAnchor: String
    let request: () async -> Void

    var body: some View {
        GateLayout(section: section, message: message,
                   detail: denied ? "O acesso foi recusado. Libere o Skykeep em Ajustes do Sistema › Privacidade e Segurança." : nil) {
            if denied {
                Button { openPrivacy(privacyAnchor) } label: { Text("Abrir Ajustes do Sistema").padding(.horizontal, 10) }
                    .buttonStyle(.glassProminent).tint(section.tint).controlSize(.extraLarge)
            } else {
                Button { Task { await request() } } label: { Text("Permitir acesso").padding(.horizontal, 10) }
                    .buttonStyle(.glassProminent).tint(section.tint).controlSize(.extraLarge)
            }
        }
    }
}

/// Notes and Mail have no public interface; reading their files directly — without opening
/// either app — needs Full Disk Access, which only System Settings can give.
struct FullDiskGate: View {
    let section: AppSection
    let message: String
    let retry: () async -> Void

    var body: some View {
        GateLayout(section: section, message: message,
                   detail: "Em Ajustes do Sistema › Privacidade e Segurança › Acesso Total ao Disco, ligue o Skykeep e volte aqui.") {
            HStack(spacing: 10) {
                Button { FullDiskAccess.openSettings() } label: { Text("Abrir Acesso Total ao Disco").padding(.horizontal, 10) }
                    .buttonStyle(.glassProminent).tint(section.tint).controlSize(.extraLarge)
                Button { Task { await retry() } } label: { Text("Já liguei").padding(.horizontal, 6) }
                    .buttonStyle(.glass).controlSize(.extraLarge)
            }
        }
    }
}

func openPrivacy(_ anchor: String) {
    if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)") {
        NSWorkspace.shared.open(url)
    }
}

/// An app's own column on the left (lists, folders, albums). No fill of its own: the window's
/// backdrop runs under it, a hairline sets it apart.
struct SourceColumn<Content: View>: View {
    var width: CGFloat = 230
    @ViewBuilder var content: Content

    var body: some View {
        VStack(spacing: 0) { content }
            .scrollContentBackground(.hidden)
            .frame(width: width)
            .frame(maxHeight: .infinity, alignment: .top)
            .background(Color.primary.opacity(0.018))
            .overlay(alignment: .trailing) { Rectangle().fill(Color.primary.opacity(0.07)).frame(width: 1) }
    }
}

/// A selectable row in a source column, highlighted in the section's tint.
struct SourceRow<Trailing: View>: View {
    let title: String
    let symbol: String
    var tint: Color = .accentColor
    let selected: Bool
    let action: () -> Void
    @ViewBuilder var trailing: Trailing

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Image(systemName: symbol)
                    .symbolVariant(selected ? .fill : .none)
                    .foregroundStyle(selected ? tint : .secondary)
                    .frame(width: 18)
                Text(title).lineLimit(1)
                Spacer(minLength: 4)
                trailing
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 6)
            .background(selected ? tint.opacity(0.16) : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

extension SourceRow where Trailing == EmptyView {
    init(title: String, symbol: String, tint: Color = .accentColor, selected: Bool, action: @escaping () -> Void) {
        self.init(title: title, symbol: symbol, tint: tint, selected: selected, action: action) { EmptyView() }
    }
}

/// A heading inside a source column.
struct SourceHeading: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 9)
            .padding(.top, 12)
            .padding(.bottom, 2)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// A row count, quiet, right-aligned.
struct Count: View {
    let n: Int
    var body: some View { Text("\(n)").foregroundStyle(.secondary).monospacedDigit().font(.callout) }
}

/// The person's picture, or their initials on a colour of their own (the same every time).
struct Avatar: View {
    let name: String
    let imageData: Data?
    var size: CGFloat = 32

    static let palette: [Color] = [
        Color(red: 0.98, green: 0.45, blue: 0.40), Color(red: 0.98, green: 0.62, blue: 0.25), Color(red: 0.95, green: 0.76, blue: 0.20),
        Color(red: 0.36, green: 0.78, blue: 0.47), Color(red: 0.25, green: 0.72, blue: 0.80), Color(red: 0.33, green: 0.56, blue: 0.98),
        Color(red: 0.55, green: 0.45, blue: 0.95), Color(red: 0.86, green: 0.42, blue: 0.85), Color(red: 0.55, green: 0.60, blue: 0.68),
    ]

    var body: some View {
        Group {
            if let imageData, let image = NSImage(data: imageData) {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                let c = Self.color(for: name)
                ZStack {
                    LinearGradient(colors: [c.mix(with: .white, by: 0.15), c.mix(with: .black, by: 0.12)], startPoint: .top, endPoint: .bottom)
                    Text(initials)
                        .font(.system(size: size * 0.4, weight: .semibold, design: .rounded))
                        .foregroundStyle(.white)
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .overlay(Circle().strokeBorder(Color.white.opacity(0.12), lineWidth: 0.5))
    }

    static func color(for name: String) -> Color {
        let h = name.unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0x7fffffff }
        return palette[h % palette.count]
    }

    private var initials: String {
        let parts = name.split(separator: " ").filter { $0.first?.isLetter ?? false }.prefix(2)
        let letters = parts.compactMap { $0.first }.map(String.init).joined()
        return letters.isEmpty ? String(name.prefix(1)).uppercased() : letters.uppercased()
    }
}

/// "Salvo em Downloads" and similar — said in the window, without opening anything.
struct Toast: View {
    let text: String
    var symbol = "checkmark.circle.fill"
    var body: some View {
        Label(text, systemImage: symbol)
            .font(.callout.weight(.medium))
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .glassEffect(.regular, in: .capsule)
            .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}

// MARK: - Dates

extension Date {
    var startOfDay: Date { Calendar.current.startOfDay(for: self) }
    func adding(days: Int) -> Date { Calendar.current.date(byAdding: .day, value: days, to: self) ?? self }
    var isToday: Bool { Calendar.current.isDateInToday(self) }
}

enum Fmt {
    static let locale = Locale(identifier: "pt_BR")

    static func time(_ d: Date) -> String { d.formatted(.dateTime.hour().minute().locale(locale)) }
    static func dayMonth(_ d: Date) -> String { d.formatted(.dateTime.day().month(.wide).locale(locale)) }
    static func weekdayDay(_ d: Date) -> String { d.formatted(.dateTime.weekday(.wide).day().month(.wide).locale(locale)).capitalizedFirst }
    static func monthYear(_ d: Date) -> String { d.formatted(.dateTime.month(.wide).year().locale(locale)).capitalizedFirst }
    static func short(_ d: Date) -> String { d.formatted(.dateTime.day().month(.abbreviated).year().locale(locale)) }

    static func relativeDay(_ d: Date) -> String {
        let cal = Calendar.current
        if cal.isDateInToday(d) { return "Hoje" }
        if cal.isDateInTomorrow(d) { return "Amanhã" }
        if cal.isDateInYesterday(d) { return "Ontem" }
        if let days = cal.dateComponents([.day], from: Date().startOfDay, to: d.startOfDay).day, (2...6).contains(days) {
            return d.formatted(.dateTime.weekday(.wide).locale(locale)).capitalizedFirst
        }
        return d.formatted(.dateTime.day().month(.abbreviated).locale(locale))
    }

    static func listDate(_ d: Date) -> String {
        let cal = Calendar.current
        if cal.isDateInToday(d) { return time(d) }
        if cal.isDateInYesterday(d) { return "Ontem" }
        if let days = cal.dateComponents([.day], from: d.startOfDay, to: Date().startOfDay).day, days < 7 {
            return d.formatted(.dateTime.weekday(.wide).locale(locale)).capitalizedFirst
        }
        return d.formatted(.dateTime.day(.twoDigits).month(.twoDigits).year(.twoDigits).locale(locale))
    }

    static func bytes(_ n: Int64) -> String { ByteCountFormatter.string(fromByteCount: n, countStyle: .file) }
}

extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
