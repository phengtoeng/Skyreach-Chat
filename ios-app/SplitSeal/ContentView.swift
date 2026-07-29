import SwiftUI
import CoreImage.CIFilterBuiltins

// ─────────────────────────────── palette ────────────────────────────────────
extension Color {
    init(hex: UInt) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xff) / 255,
                  green: Double((hex >> 8) & 0xff) / 255,
                  blue: Double(hex & 0xff) / 255,
                  opacity: 1)
    }
    static let dvBlue = Color(hex: 0x2E9BF6)
    static let dvConv = Color(hex: 0xE8F1F8)
    static let dvOut = Color(hex: 0xD3EAFB)
    static let dvInk = Color(hex: 0x101720)
    static let dvSub = Color(hex: 0x8C99A6)
    static let dvHair = Color(hex: 0xECEFF2)
    static let dvGreen = Color(hex: 0x33C15A)
}

private let avatarColors: [Color] =
    [0x5B8DEF, 0xEF6F6C, 0x3FBF8F, 0xF0A84A, 0x9B7EDE, 0x48B0C7, 0xE07AAE].map { Color(hex: $0) }
private func avatarColor(_ name: String) -> Color {
    let h = name.unicodeScalars.reduce(0) { $0 + Int($1.value) }
    return avatarColors[h % avatarColors.count]
}
private func initials(_ name: String) -> String {
    name.split(separator: " ").prefix(2).compactMap { $0.first }.map(String.init).joined().uppercased()
}
private func nowTime() -> String {
    let f = DateFormatter(); f.dateFormat = "h:mm a"; f.locale = Locale(identifier: "en_US"); return f.string(from: Date())
}

// ─────────────────────────────── models ─────────────────────────────────────
struct Chat: Identifiable { let id = UUID(); let name: String; let last: String; let time: String; var unread: Int = 0 }
enum Kind { case text, image, voice }
enum MsgState { case plain, sealing, opened }
struct Msg: Identifiable {
    let id = UUID(); var text: String; let incoming: Bool; let time: String
    var kind: Kind = .text; var state: MsgState = .plain; var read: Bool = true; var mode: String = "STRICT"
}

private let CHATS: [Chat] = [
    Chat(name: "Maya", last: "See you tonight!", time: "9:41 AM", unread: 2),
    Chat(name: "Ethan", last: "That works for me.", time: "9:32 AM"),
    Chat(name: "Lena", last: "Thanks for the update.", time: "Yesterday", unread: 1),
    Chat(name: "Weekend Plan", last: "You: Looking forward!", time: "Yesterday"),
    Chat(name: "Noah", last: "Let's catch up soon.", time: "Tue"),
    Chat(name: "Zoe", last: "Photo", time: "Mon", unread: 1),
    Chat(name: "Daniel", last: "Sounds good.", time: "Mon"),
]
private func seedThread() -> [Msg] {
    [
        Msg(text: "Hey! How was your day?", incoming: true, time: "9:24 AM"),
        Msg(text: "Pretty good! How about yours?", incoming: false, time: "9:25 AM"),
        Msg(text: "Busy, but productive.", incoming: true, time: "9:26 AM"),
        Msg(text: "Nice! Anything fun later?", incoming: false, time: "9:27 AM"),
        Msg(text: "Maybe dinner. I'll let you know!", incoming: true, time: "9:28 AM"),
    ]
}

// ─────────────────────── identity + contacts (persisted) ────────────────────
struct Contact: Identifiable { let id = UUID(); let name: String; let address: String; let devicePub: String }

enum Store {
    private static let d = UserDefaults.standard
    static func identity() -> [String: Any]? {
        guard let s = d.string(forKey: "identity"), let data = s.data(using: .utf8) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }
    static func saveIdentity(_ json: String) { d.set(json, forKey: "identity") }
    static func contacts() -> [[String: Any]] {
        guard let s = d.string(forKey: "contacts"), let data = s.data(using: .utf8) else { return [] }
        return ((try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]) ?? []
    }
    static func addContact(_ json: String) {
        guard let data = json.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return }
        var a = contacts(); a.append(obj)
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "contacts")
        }
    }
}

func loadOrCreateIdentity() -> [String: Any] {
    if let j = Store.identity() { return j }
    let json = SealCore.newIdentity("Me")
    Store.saveIdentity(json)
    return ((try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]) ?? [:]
}
func loadContacts() -> [Contact] {
    Store.contacts().map {
        Contact(name: $0["name"] as? String ?? "", address: $0["address"] as? String ?? "", devicePub: $0["device_pub"] as? String ?? "")
    }
}

func qrImage(_ text: String) -> Image? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(text.utf8)
    guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)),
          let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
    return Image(decorative: cg, scale: 1, orientation: .up)
}

// ─────────────────────────────── root nav ───────────────────────────────────
struct ContentView: View {
    @State private var identity: [String: Any] = loadOrCreateIdentity()
    @State private var contacts: [Contact] = loadContacts()
    @State private var openChat: Chat? = nil
    @State private var tab = 0
    @State private var showAdd = false

    var body: some View {
        Group {
            if let c = openChat {
                ConversationView(chat: c, onBack: { openChat = nil })
            } else if tab == 2 {
                ProfileView(identity: identity, tab: tab, onTab: { tab = $0 })
            } else {
                ChatListView(contacts: contacts, onOpen: { openChat = $0 }, onAdd: { showAdd = true }, tab: tab, onTab: { tab = $0 })
            }
        }
        .sheet(isPresented: $showAdd) {
            AddContactSheet(onCancel: { showAdd = false }, onAdd: { code in
                let res = SealCore.parseCard(code)
                let j = (try? JSONSerialization.jsonObject(with: Data(res.utf8))) as? [String: Any]
                if (j?["ok"] as? Bool) == true {
                    Store.addContact(res)
                    contacts = loadContacts()
                    showAdd = false
                    return nil
                }
                return (j?["error"] as? String) ?? "invalid code"
            })
        }
    }
}

// ─────────────────────────────── chat list ──────────────────────────────────
struct ChatListView: View {
    let contacts: [Contact]
    let onOpen: (Chat) -> Void
    let onAdd: () -> Void
    let tab: Int
    let onTab: (Int) -> Void
    var body: some View {
        let rows = contacts.map { Chat(name: $0.name, last: String($0.address.prefix(14)) + "… · tap to seal", time: "") } + CHATS
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "shield.fill").foregroundColor(.white).font(.system(size: 20))
                Text("Denvion").font(.system(size: 21, weight: .bold)).foregroundColor(.white)
                Spacer()
                Image(systemName: "magnifyingglass").foregroundColor(.white).font(.system(size: 20))
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
            .frame(maxWidth: .infinity)
            .background(Color.dvBlue.ignoresSafeArea(edges: .top))

            ZStack(alignment: .bottomTrailing) {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(rows.indices, id: \.self) { i in
                            let c = rows[i]
                            ChatRow(c).contentShape(Rectangle()).onTapGesture { onOpen(c) }
                            Rectangle().fill(Color.dvHair).frame(height: 1).padding(.leading, 84)
                        }
                    }
                }
                Button(action: onAdd) {
                    Image(systemName: "person.badge.plus").foregroundColor(.white).font(.system(size: 22))
                        .frame(width: 56, height: 56).background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 16))
                        .shadow(color: .dvBlue.opacity(0.4), radius: 6, y: 3)
                }.padding(20)
            }

            BottomBar(tab: tab, onTab: onTab)
        }
        .background(Color.white)
    }
}

private struct ChatRow: View {
    let c: Chat
    init(_ c: Chat) { self.c = c }
    var body: some View {
        HStack(spacing: 14) {
            Avatar(c.name, 52)
            VStack(alignment: .leading, spacing: 3) {
                Text(c.name).font(.system(size: 16, weight: .semibold)).foregroundColor(.dvInk)
                Text(c.last).font(.system(size: 14)).foregroundColor(.dvSub).lineLimit(1)
            }
            Spacer(minLength: 10)
            VStack(alignment: .trailing, spacing: 6) {
                Text(c.time).font(.system(size: 12)).foregroundColor(c.unread > 0 ? .dvBlue : .dvSub)
                if c.unread > 0 {
                    Text("\(c.unread)").font(.system(size: 11, weight: .bold)).foregroundColor(.white)
                        .frame(width: 20, height: 20).background(Color.dvBlue).clipShape(Circle())
                } else {
                    Color.clear.frame(width: 20, height: 20)
                }
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
    }
}

private struct BottomBar: View {
    let tab: Int
    let onTab: (Int) -> Void
    var body: some View {
        VStack(spacing: 0) {
            Rectangle().fill(Color.dvHair).frame(height: 1)
            HStack {
                NavItem(icon: "bubble.left.fill", label: "Chats", active: tab == 0) { onTab(0) }
                NavItem(icon: "phone.fill", label: "Calls", active: tab == 1) { onTab(1) }
                NavItem(icon: "gearshape.fill", label: "Settings", active: tab == 2) { onTab(2) }
            }
            .frame(maxWidth: .infinity)
            .padding(.top, 8).padding(.bottom, 4)
        }
        .background(Color.white)
    }
}

private struct NavItem: View {
    let icon: String; let label: String; let active: Bool; let onTap: () -> Void
    var body: some View {
        VStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 21))
            Text(label).font(.system(size: 11, weight: active ? .semibold : .regular))
        }
        .foregroundColor(active ? .dvBlue : .dvSub)
        .frame(maxWidth: .infinity)
        .contentShape(Rectangle())
        .onTapGesture(perform: onTap)
    }
}

// ───────────────────────────── profile / add ────────────────────────────────
struct ProfileView: View {
    let identity: [String: Any]
    let tab: Int
    let onTab: (Int) -> Void
    var body: some View {
        let name = identity["name"] as? String ?? "Me"
        let address = identity["address"] as? String ?? ""
        let card = identity["card"] as? String ?? ""
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "shield.fill").foregroundColor(.white).font(.system(size: 20))
                Text("My Denvion ID").font(.system(size: 20, weight: .bold)).foregroundColor(.white)
                Spacer()
            }
            .padding(16).frame(maxWidth: .infinity)
            .background(Color.dvBlue.ignoresSafeArea(edges: .top))

            ScrollView {
                VStack(spacing: 14) {
                    Avatar(name, 76)
                    Text(name).font(.system(size: 20, weight: .semibold)).foregroundColor(.dvInk)
                    VStack(spacing: 2) {
                        Text("WCAHT identity address").font(.system(size: 12)).foregroundColor(.dvSub)
                        Text(address).font(.system(size: 13)).foregroundColor(.dvBlue)
                            .multilineTextAlignment(.center).textSelection(.enabled)
                    }
                    if let qr = qrImage(card) {
                        qr.interpolation(.none).resizable().frame(width: 230, height: 230)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                            .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color.dvHair))
                    }
                    Text("Share this code so others can add you").font(.system(size: 13)).foregroundColor(.dvSub)
                    Text(card).font(.system(size: 11)).foregroundColor(.dvInk)
                        .multilineTextAlignment(.center).textSelection(.enabled)
                        .padding(12).background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 10))
                }
                .padding(20)
            }

            BottomBar(tab: tab, onTab: onTab)
        }
        .background(Color.white)
    }
}

struct AddContactSheet: View {
    let onCancel: () -> Void
    let onAdd: (String) -> String?
    @State private var code = ""
    @State private var error: String? = nil
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Add contact").font(.system(size: 20, weight: .bold)).foregroundColor(.dvInk)
            Text("Paste your contact's Denvion code.").font(.system(size: 13)).foregroundColor(.dvSub)
            TextField("denvion:…", text: $code, axis: .vertical).lineLimit(2...5)
                .padding(10).background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 10))
            if let e = error { Text(e).font(.system(size: 12)).foregroundColor(.red) }
            HStack {
                Spacer()
                Button("Cancel", action: onCancel).foregroundColor(.dvSub)
                Button("Add") { error = onAdd(code.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    .fontWeight(.semibold).foregroundColor(.dvBlue).padding(.leading, 16)
            }
        }
        .padding(20)
        .presentationDetents([.medium])
    }
}

// ────────────────────────────── conversation ────────────────────────────────
struct ConversationView: View {
    let chat: Chat
    let onBack: () -> Void
    @State private var messages: [Msg] = seedThread()
    @State private var draft = ""
    @State private var fastMode = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Button(action: onBack) { Image(systemName: "chevron.left").font(.system(size: 20, weight: .semibold)).foregroundColor(.dvBlue) }
                Avatar(chat.name, 40)
                VStack(alignment: .leading, spacing: 1) {
                    Text(chat.name).font(.system(size: 16, weight: .semibold)).foregroundColor(.dvInk)
                    Text("Online").font(.system(size: 12)).foregroundColor(.dvGreen)
                }
                Spacer()
                Button(action: { fastMode.toggle() }) {
                    Image(systemName: fastMode ? "bolt.fill" : "lock.fill").font(.system(size: 19)).foregroundColor(.dvBlue)
                }
                Button(action: {}) { Image(systemName: "phone.fill").font(.system(size: 19)).foregroundColor(.dvBlue) }
                    .padding(.leading, 4)
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            .frame(maxWidth: .infinity)
            .background(Color.white.ignoresSafeArea(edges: .top))
            Rectangle().fill(Color.dvHair).frame(height: 1)

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 0) {
                        DateChip("Today")
                        ForEach(messages) { m in Bubble(m).id(m.id) }
                    }
                    .padding(.horizontal, 12).padding(.vertical, 8)
                }
                .onChange(of: messages.count) { _ in
                    if let last = messages.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
                }
            }
            .background(Color.dvConv)

            composer
        }
        .background(Color.dvConv)
        .navigationBarHidden(true)
    }

    private var composer: some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "face.smiling").foregroundColor(.dvSub).font(.system(size: 20))
                TextField("Message", text: $draft, axis: .vertical)
                    .lineLimit(1...4).font(.system(size: 15)).foregroundColor(.dvInk)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 22))

            let active = !draft.trimmingCharacters(in: .whitespaces).isEmpty
            Button(action: send) {
                Image(systemName: active ? "paperplane.fill" : "mic.fill").foregroundColor(.white).font(.system(size: 19))
                    .frame(width: 46, height: 46).background(Color.dvBlue).clipShape(Circle())
            }.disabled(!active)
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(Color.white)
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let fast = fastMode
        messages.append(Msg(text: text, incoming: false, time: nowTime(), state: .sealing, mode: fast ? "FAST" : "STRICT"))
        let idx = messages.count - 1
        draft = ""
        Task { @MainActor in
            // drive the real Rust seal core
            let json = fast ? SealCore.runFastDemo() : SealCore.runDemo()
            try? await Task.sleep(nanoseconds: fast ? 550_000_000 : 1_150_000_000)
            let opened = json.contains("OPENED")
            if messages.indices.contains(idx) { messages[idx].state = opened ? .opened : .sealing }
        }
    }
}

private func DateChip(_ text: String) -> some View {
    HStack {
        Spacer()
        Text(text).font(.system(size: 12)).foregroundColor(.dvSub)
            .padding(.horizontal, 12).padding(.vertical, 4).background(Color.white).clipShape(Capsule())
        Spacer()
    }.padding(.vertical, 8)
}

// ─────────────────────────────── bubble ─────────────────────────────────────
private struct Bubble: View {
    let m: Msg
    init(_ m: Msg) { self.m = m }
    var body: some View {
        HStack {
            if !m.incoming { Spacer(minLength: 40) }
            VStack(alignment: m.incoming ? .leading : .trailing, spacing: 3) {
                content
                    .padding(10)
                    .background(m.incoming ? Color.white : Color.dvOut)
                    .clipShape(RoundedRectangle(cornerRadius: 18))
                    .frame(maxWidth: 300, alignment: m.incoming ? .leading : .trailing)
                if m.state == .sealing {
                    HStack(spacing: 4) {
                        Image(systemName: "arrow.triangle.2.circlepath").font(.system(size: 11))
                        Text(m.mode == "FAST" ? "Pre-confirming…" : "Sealing…").font(.system(size: 11))
                    }.foregroundColor(.dvSub)
                }
            }
            if m.incoming { Spacer(minLength: 40) }
        }.padding(.vertical, 3)
    }

    @ViewBuilder private var content: some View {
        switch m.state {
        case .sealing: sealing
        default:
            switch m.kind {
            case .image: imageContent
            case .voice: voiceContent
            case .text: textContent
            }
        }
    }

    private var textContent: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(m.text).font(.system(size: 15)).foregroundColor(.dvInk)
            metaRow
        }
    }

    private var sealing: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Image(systemName: "lock.fill").font(.system(size: 14)).foregroundColor(.dvSub)
                Text(m.mode == "FAST" ? "Waiting for pre-confirms" : "Waiting for finality")
                    .font(.system(size: 14)).foregroundColor(.dvSub)
            }
            HStack { Spacer(); Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub) }
        }
    }

    private var imageContent: some View {
        ZStack(alignment: .bottomTrailing) {
            LinearGradient(colors: [Color(hex: 0xF6B26B), Color(hex: 0x6FA8DC), Color(hex: 0x2E5B8A)],
                           startPoint: .top, endPoint: .bottom)
                .frame(width: 220, height: 130).clipShape(RoundedRectangle(cornerRadius: 12))
            HStack(spacing: 3) { Text(m.time).font(.system(size: 11)).foregroundColor(.white); sealBadge }.padding(6)
        }
    }

    private var voiceContent: some View {
        HStack(spacing: 6) {
            Image(systemName: "play.fill").font(.system(size: 20)).foregroundColor(.dvBlue)
            HStack(spacing: 2) {
                ForEach([8, 16, 11, 20, 13, 22, 9, 17, 12, 7, 15, 10], id: \.self) { h in
                    Capsule().fill(Color.dvBlue).frame(width: 3, height: CGFloat(h))
                }
            }
            Text("0:18").font(.system(size: 12)).foregroundColor(.dvSub)
            Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub)
            sealBadge
        }
    }

    private var metaRow: some View {
        HStack(spacing: 4) {
            Spacer()
            Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub)
            if !m.incoming {
                if m.state == .opened {
                    sealBadge
                } else {
                    Text("✓✓").font(.system(size: 12, weight: .bold)).foregroundColor(m.read ? .dvBlue : .dvSub)
                }
            }
        }
    }

    private var sealBadge: some View {
        Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundColor(.white)
            .frame(width: 16, height: 16).background(Color.dvGreen).clipShape(Circle())
    }
}

// ─────────────────────────────── avatar ─────────────────────────────────────
private struct Avatar: View {
    let name: String; let size: CGFloat
    init(_ name: String, _ size: CGFloat) { self.name = name; self.size = size }
    var body: some View {
        Circle().fill(avatarColor(name)).frame(width: size, height: size)
            .overlay(Text(initials(name)).foregroundColor(.white).font(.system(size: size / 2.6, weight: .semibold)))
    }
}
