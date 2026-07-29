import SwiftUI
import CoreImage.CIFilterBuiltins
import AVFoundation
import UIKit

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
struct Contact: Identifiable { let id = UUID(); let name: String; let address: String; let devicePub: String; let phone: String }

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
        Contact(name: $0["name"] as? String ?? "", address: $0["address"] as? String ?? "",
                devicePub: $0["device_pub"] as? String ?? "", phone: $0["phone"] as? String ?? "")
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
    @State private var showNew = false
    @State private var showScanner = false
    @State private var scanned: [String: Any]? = nil

    private func saveContact(_ first: String, _ last: String, _ phone: String) {
        let nm = [first, last].filter { !$0.isEmpty }.joined(separator: " ")
        let name = nm.isEmpty ? (scanned?["name"] as? String ?? "Contact") : nm
        var c: [String: Any] = ["name": name, "phone": phone]
        if let s = scanned {
            c["address"] = s["address"] ?? ""
            c["device_pub"] = s["device_pub"] ?? ""
            c["identity_pub"] = s["identity_pub"] ?? ""
        }
        if let data = try? JSONSerialization.data(withJSONObject: c), let json = String(data: data, encoding: .utf8) {
            Store.addContact(json)
            contacts = loadContacts()
        }
        showNew = false; scanned = nil
    }

    var body: some View {
        Group {
            if let c = openChat {
                ConversationView(chat: c, onBack: { openChat = nil })
            } else if showNew {
                NewContactView(scanned: scanned, onScan: { showScanner = true }, onCancel: { showNew = false; scanned = nil }, onSave: saveContact)
            } else if tab == 2 {
                ProfileView(identity: identity, tab: tab, onTab: { tab = $0 })
            } else {
                ChatListView(contacts: contacts, onOpen: { openChat = $0 }, onAdd: { showNew = true; scanned = nil }, tab: tab, onTab: { tab = $0 })
            }
        }
        .fullScreenCover(isPresented: $showScanner) {
            QRScannerView(
                onFound: { code in
                    showScanner = false
                    let res = SealCore.parseCard(code)
                    if let d = (try? JSONSerialization.jsonObject(with: Data(res.utf8))) as? [String: Any], (d["ok"] as? Bool) == true {
                        scanned = d
                    }
                },
                onClose: { showScanner = false }
            )
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
        let rows = contacts.map { c -> Chat in
            let sub = !c.address.isEmpty ? String(c.address.prefix(14)) + "… · tap to seal"
                : (!c.phone.isEmpty ? "+855 " + c.phone : "tap to seal")
            return Chat(name: c.name, last: sub, time: "")
        } + CHATS
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

struct NewContactView: View {
    let scanned: [String: Any]?
    let onScan: () -> Void
    let onCancel: () -> Void
    let onSave: (String, String, String) -> Void
    @State private var first = ""
    @State private var last = ""
    @State private var phone = ""
    @State private var sync = true

    var body: some View {
        let canSave = !first.trimmingCharacters(in: .whitespaces).isEmpty || scanned != nil
        VStack(spacing: 0) {
            HStack {
                Button(action: onCancel) {
                    Image(systemName: "xmark").foregroundColor(.dvInk)
                        .frame(width: 36, height: 36).background(Color.white).clipShape(Circle())
                }
                Spacer()
                Text("New Contact").font(.system(size: 17, weight: .semibold)).foregroundColor(.dvInk)
                Spacer()
                Button(action: {
                    if canSave {
                        onSave(first.trimmingCharacters(in: .whitespaces), last.trimmingCharacters(in: .whitespaces), phone.trimmingCharacters(in: .whitespaces))
                    }
                }) {
                    Image(systemName: "checkmark").foregroundColor(.white)
                        .frame(width: 36, height: 36).background(canSave ? Color.dvBlue : Color(hex: 0xCBD3DA)).clipShape(Circle())
                }.disabled(!canSave)
            }
            .padding(.horizontal, 14).padding(.vertical, 12)

            ScrollView {
                VStack(spacing: 18) {
                    VStack(spacing: 0) {
                        TextField("First Name", text: $first).foregroundColor(.dvInk).padding(16)
                        Divider().padding(.leading, 16)
                        TextField("Last Name", text: $last).foregroundColor(.dvInk).padding(16)
                    }
                    .background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))

                    VStack(spacing: 0) {
                        HStack {
                            Text("🇰🇭  Cambodia").foregroundColor(.dvInk)
                            Spacer()
                            Image(systemName: "chevron.right").foregroundColor(.dvSub)
                        }.padding(16)
                        Divider().padding(.leading, 16)
                        HStack {
                            Text("+855").foregroundColor(.dvInk)
                            Spacer().frame(width: 16)
                            TextField("00 000 0000", text: $phone).keyboardType(.numberPad).foregroundColor(.dvInk)
                        }.padding(16)
                    }
                    .background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))

                    HStack {
                        Text("Sync Contact to Phone").foregroundColor(.dvInk)
                        Spacer()
                        Toggle("", isOn: $sync).labelsHidden()
                    }
                    .padding(16).background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))

                    Button(action: onScan) {
                        HStack {
                            Image(systemName: "qrcode")
                            Text("Add via QR Code")
                            Spacer()
                        }.foregroundColor(.dvBlue).padding(16)
                    }
                    .background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))

                    if let s = scanned {
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Image(systemName: "checkmark.circle.fill").foregroundColor(.dvGreen)
                                Text("Linked to WCAHT address").font(.system(size: 14, weight: .semibold)).foregroundColor(.dvInk)
                            }
                            Text(s["address"] as? String ?? "").font(.system(size: 12)).foregroundColor(.dvBlue)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(16).background(Color(hex: 0xE9F7EE)).clipShape(RoundedRectangle(cornerRadius: 14))
                    }
                }
                .padding(16)
            }
        }
        .background(Color(hex: 0xF2F3F5).ignoresSafeArea())
        .onChange(of: (scanned?["address"] as? String) ?? "") { addr in
            if !addr.isEmpty, first.isEmpty, let n = scanned?["name"] as? String { first = n }
        }
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

// ─────────────────────────── camera QR scanner ──────────────────────────────
// Requires NSCameraUsageDescription in Info.plist (e.g. "Scan a contact's QR code").
struct QRScannerView: UIViewControllerRepresentable {
    let onFound: (String) -> Void
    let onClose: () -> Void
    func makeUIViewController(context: Context) -> ScannerVC {
        let vc = ScannerVC()
        vc.onFound = onFound
        vc.onClose = onClose
        return vc
    }
    func updateUIViewController(_ uiViewController: ScannerVC, context: Context) {}
}

final class ScannerVC: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onFound: ((String) -> Void)?
    var onClose: (() -> Void)?
    private let session = AVCaptureSession()
    private var handled = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black
        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.frame = view.layer.bounds
        preview.videoGravity = .resizeAspectFill
        view.layer.addSublayer(preview)

        let cancel = UIButton(type: .system)
        cancel.setTitle("Cancel", for: .normal)
        cancel.setTitleColor(.white, for: .normal)
        cancel.titleLabel?.font = .systemFont(ofSize: 17, weight: .semibold)
        cancel.frame = CGRect(x: 20, y: 56, width: 90, height: 40)
        cancel.addTarget(self, action: #selector(closeTapped), for: .touchUpInside)
        view.addSubview(cancel)

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in self?.session.startRunning() }
    }

    @objc private func closeTapped() {
        session.stopRunning()
        onClose?()
    }

    func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput metadataObjects: [AVMetadataObject], from connection: AVCaptureConnection) {
        guard !handled,
              let obj = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let value = obj.stringValue else { return }
        handled = true
        session.stopRunning()
        onFound?(value)
    }
}
