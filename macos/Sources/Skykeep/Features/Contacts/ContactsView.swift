@preconcurrency import Contacts
import SwiftUI

struct ContactsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let svc = model.contacts
        if svc.isAuthorized {
            ContactsScreen()
        } else {
            PermissionGate(
                section: .contacts,
                message: "Seus contatos do iCloud, com um clique para ligar, mandar mensagem ou FaceTime.",
                denied: svc.denied,
                privacyAnchor: "Privacy_Contacts",
                request: { await svc.request() }
            )
        }
    }
}

private struct ContactsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var query = ""
    @State private var editing: ContactDraft?
    @State private var confirmDelete: CNContact?

    var body: some View {
        @Bindable var svc = model.contacts
        let list = svc.search(query)
        let groups = Dictionary(grouping: list, by: \.sortLetter)
        let letters = groups.keys.sorted { a, b in a == "#" ? false : (b == "#" ? true : a < b) }

        HStack(spacing: 0) {
            SourceColumn(width: 290) {
                SearchField(prompt: "Buscar contatos", text: $query)
                    .padding(.horizontal, 12).padding(.top, 12).padding(.bottom, 6)
                List(selection: $svc.selectedID) {
                    ForEach(letters, id: \.self) { letter in
                        Section(letter) {
                            ForEach(groups[letter] ?? [], id: \.identifier) { c in
                                HStack(spacing: 10) {
                                    Avatar(name: c.displayName, imageData: c.thumbnailImageData, size: 28)
                                    VStack(alignment: .leading, spacing: 0) {
                                        Text(c.displayName).lineLimit(1)
                                        if !c.organizationName.isEmpty && c.displayName != c.organizationName {
                                            Text(c.organizationName).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                                        }
                                    }
                                }
                                .tag(c.identifier)
                                .contextMenu {
                                    Button("Editar") { editing = ContactDraft(c) }
                                    Button("Apagar…", role: .destructive) { confirmDelete = c }
                                }
                            }
                        }
                    }
                }
                .listStyle(.sidebar)
                .overlay {
                    if !svc.loaded { ProgressView() }
                    else if list.isEmpty { QuietState(symbol: "person.crop.circle", title: query.isEmpty ? "Nenhum contato" : "Nada encontrado", tint: .brown, compact: true) }
                }
                Text("\(svc.contacts.count) contatos").font(.caption).foregroundStyle(.secondary).padding(8)
            }
            Group {
                if let id = svc.selectedID, let c = svc.contact(id: id) {
                    ContactCard(contact: c, onEdit: { editing = ContactDraft(c) }, onDelete: { confirmDelete = c })
                } else {
                    QuietState(symbol: "person.crop.circle", title: "Nenhum contato selecionado", tint: .brown)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .toolbar {
            ToolbarItem {
                Button { editing = ContactDraft() } label: { Label("Novo contato", systemImage: "person.badge.plus") }
            }
        }
        .onChange(of: model.newItemRequest) { editing = ContactDraft() }
        .onAppear { if svc.selectedID == nil { svc.selectedID = svc.contacts.first?.identifier } }
        .onChange(of: svc.loaded) { if svc.selectedID == nil { svc.selectedID = svc.contacts.first?.identifier } }
        .sheet(isPresented: Binding(get: { editing != nil }, set: { if !$0 { editing = nil } })) {
            if let editing { ContactEditor(draft: editing) }
        }
        .confirmationDialog("Apagar \(confirmDelete?.displayName ?? "")?", isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } })) {
            Button("Apagar Contato", role: .destructive) { if let c = confirmDelete { svc.delete(c) } }
        } message: {
            Text("O contato será apagado de todos os seus dispositivos.")
        }
    }
}

private struct ContactCard: View {
    let contact: CNContact
    let onEdit: () -> Void
    let onDelete: () -> Void
    @State private var mapFor: String?

    var body: some View {
        let c = contact
        let phone = c.phoneNumbers.first.map { digits($0.value.stringValue) }
        let email = c.emailAddresses.first.map { $0.value as String }
        ScrollView {
            VStack(spacing: 20) {
                VStack(spacing: 10) {
                    Avatar(name: c.displayName, imageData: c.thumbnailImageData, size: 96)
                        .shadow(color: .black.opacity(0.15), radius: 10, y: 4)
                    Text(c.displayName).font(.system(size: 26, weight: .bold)).textSelection(.enabled)
                    let sub = [c.jobTitle, c.departmentName, c.organizationName].filter { !$0.isEmpty && $0 != c.displayName }.joined(separator: " · ")
                    if !sub.isEmpty { Text(sub).foregroundStyle(.secondary) }
                }
                .padding(.top, 28)

                // Calling and messaging are FaceTime's and Messages' job: these open them, on your click.
                HStack(spacing: 12) {
                    action("message.fill", "mensagem", url: phone.map { "sms:\($0)" } ?? email.map { "imessage:\($0)" })
                    action("phone.fill", "ligar", url: phone.map { "facetime-audio:\($0)" })
                    action("video.fill", "FaceTime", url: (phone ?? email).map { "facetime:\($0)" })
                    action("envelope.fill", "e-mail", url: email.map { "mailto:\($0)" })
                }

                VStack(spacing: 12) {
                    group(c.phoneNumbers.map { (labelName($0.label), $0.value.stringValue) })
                    group(c.emailAddresses.map { (labelName($0.label), $0.value as String) })
                    if !c.postalAddresses.isEmpty {
                        Card(padding: 0) {
                            VStack(spacing: 0) {
                                ForEach(Array(c.postalAddresses.enumerated()), id: \.offset) { i, a in
                                    if i > 0 { Divider().padding(.leading, 16) }
                                    let text = CNPostalAddressFormatter.string(from: a.value, style: .mailingAddress)
                                    let oneLine = text.replacingOccurrences(of: "\n", with: ", ")
                                    field(labelName(a.label), text) {
                                        Button { mapFor = oneLine } label: { Image(systemName: "map") }
                                            .buttonStyle(.plain).foregroundStyle(.secondary).help("Ver no mapa")
                                            .popover(isPresented: Binding(get: { mapFor == oneLine }, set: { if !$0 { mapFor = nil } })) { PlaceMap(query: oneLine) }
                                    }
                                }
                            }
                        }
                    }
                    group(c.urlAddresses.map { (labelName($0.label), $0.value as String) })
                    if let b = c.birthday, let month = b.month, let day = b.day {
                        let d = Calendar.current.date(from: DateComponents(year: b.year ?? 2000, month: month, day: day)) ?? Date()
                        group([("aniversário", b.year != nil ? Fmt.short(d) : Fmt.dayMonth(d))])
                    }
                }
                .frame(maxWidth: 520)

                HStack {
                    Button("Editar", action: onEdit)
                    Button("Apagar…", role: .destructive, action: onDelete)
                }
                .padding(.bottom, 28)
            }
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 24)
        }
    }

    @ViewBuilder
    private func group(_ rows: [(String, String)]) -> some View {
        if !rows.isEmpty {
            Card(padding: 0) {
                VStack(spacing: 0) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { i, r in
                        if i > 0 { Divider().padding(.leading, 16) }
                        field(r.0, r.1) {
                            Button { Clipboard.copy(r.1) } label: { Image(systemName: "doc.on.doc") }
                                .buttonStyle(.plain).foregroundStyle(.tertiary).help("Copiar")
                        }
                    }
                }
            }
        }
    }

    private func field<Accessory: View>(_ label: String, _ value: String, @ViewBuilder accessory: () -> Accessory) -> some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                Text(label).font(.caption).foregroundStyle(.secondary)
                Text(value).textSelection(.enabled)
            }
            Spacer()
            accessory()
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
    }

    private func action(_ symbol: String, _ title: String, url: String?) -> some View {
        Button {
            if let url, let u = URL(string: url) { NSWorkspace.shared.open(u) }
        } label: {
            VStack(spacing: 4) {
                Image(systemName: symbol).font(.system(size: 17))
                    .frame(width: 44, height: 44)
                    .background(url == nil ? Color.secondary.opacity(0.12) : Color.accentColor.opacity(0.14), in: Circle())
                    .foregroundStyle(url == nil ? Color.secondary : Color.accentColor)
                Text(title).font(.caption).foregroundStyle(url == nil ? .tertiary : .secondary)
            }
            .frame(width: 70)
        }
        .buttonStyle(.plain)
        .disabled(url == nil)
    }

    private func digits(_ s: String) -> String { s.filter { $0.isNumber || $0 == "+" } }
}

private struct ContactEditor: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State var draft: ContactDraft
    @State private var error: String?

    private let phoneLabels = [CNLabelPhoneNumberMobile, CNLabelPhoneNumberiPhone, CNLabelHome, CNLabelWork, CNLabelPhoneNumberMain, CNLabelOther]
    private let emailLabels = [CNLabelHome, CNLabelWork, CNLabelEmailiCloud, CNLabelOther]

    var body: some View {
        VStack(spacing: 0) {
            Form {
                Section {
                    TextField("Nome", text: $draft.givenName)
                    TextField("Sobrenome", text: $draft.familyName)
                    TextField("Empresa", text: $draft.organization)
                    TextField("Cargo", text: $draft.jobTitle)
                }
                Section("Telefones") {
                    ForEach($draft.phones) { $p in
                        HStack {
                            Picker("", selection: $p.label) { ForEach(phoneLabels, id: \.self) { Text(labelName($0)).tag($0) } }
                                .labelsHidden().frame(width: 110)
                            TextField("Telefone", text: $p.value)
                            Button { draft.phones.removeAll { $0.id == p.id } } label: { Image(systemName: "minus.circle.fill") }
                                .buttonStyle(.plain).foregroundStyle(.red)
                        }
                    }
                    Button("Adicionar telefone") { draft.phones.append(LabeledEntry(label: CNLabelPhoneNumberMobile, value: "")) }
                }
                Section("E-mails") {
                    ForEach($draft.emails) { $e in
                        HStack {
                            Picker("", selection: $e.label) { ForEach(emailLabels, id: \.self) { Text(labelName($0)).tag($0) } }
                                .labelsHidden().frame(width: 110)
                            TextField("E-mail", text: $e.value)
                            Button { draft.emails.removeAll { $0.id == e.id } } label: { Image(systemName: "minus.circle.fill") }
                                .buttonStyle(.plain).foregroundStyle(.red)
                        }
                    }
                    Button("Adicionar e-mail") { draft.emails.append(LabeledEntry(label: CNLabelHome, value: "")) }
                }
                Section {
                    Toggle("Aniversário", isOn: $draft.hasBirthday)
                    if draft.hasBirthday { DatePicker("Data", selection: $draft.birthday, displayedComponents: .date) }
                }
            }
            .formStyle(.grouped)
            if let error { Text(error).foregroundStyle(.red).font(.caption) }
            HStack {
                Spacer()
                Button("Cancelar") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(draft.id == nil ? "Adicionar" : "Salvar") {
                    if let e = model.contacts.save(draft) { error = e } else { dismiss() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(draft.givenName.isEmpty && draft.familyName.isEmpty && draft.organization.isEmpty)
            }
            .padding(16)
        }
        .frame(width: 480, height: 560)
    }
}
