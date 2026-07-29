import SwiftUI

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

// ─────────────────────────────── root nav ───────────────────────────────────
struct ContentView: View {
    @State private var openChat: Chat? = nil
    var body: some View {
        if let c = openChat {
            ConversationView(chat: c, onBack: { openChat = nil })
        } else {
            ChatListView(onOpen: { openChat = $0 })
        }
    }
}

// ─────────────────────────────── chat list ──────────────────────────────────
struct ChatListView: View {
    let onOpen: (Chat) -> Void
    var body: some View {
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
                        ForEach(CHATS) { c in
                            ChatRow(c).contentShape(Rectangle()).onTapGesture { onOpen(c) }
                            Rectangle().fill(Color.dvHair).frame(height: 1).padding(.leading, 84)
                        }
                    }
                }
                Button(action: { onOpen(CHATS[0]) }) {
                    Image(systemName: "square.and.pencil").foregroundColor(.white).font(.system(size: 22))
                        .frame(width: 56, height: 56).background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 16))
                        .shadow(color: .dvBlue.opacity(0.4), radius: 6, y: 3)
                }.padding(20)
            }

            BottomBar()
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
    var body: some View {
        VStack(spacing: 0) {
            Rectangle().fill(Color.dvHair).frame(height: 1)
            HStack {
                NavItem(icon: "bubble.left.fill", label: "Chats", active: true)
                NavItem(icon: "phone.fill", label: "Calls", active: false)
                NavItem(icon: "gearshape.fill", label: "Settings", active: false)
            }
            .frame(maxWidth: .infinity)
            .padding(.top, 8).padding(.bottom, 4)
        }
        .background(Color.white)
    }
}

private struct NavItem: View {
    let icon: String; let label: String; let active: Bool
    var body: some View {
        VStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 21))
            Text(label).font(.system(size: 11, weight: active ? .semibold : .regular))
        }
        .foregroundColor(active ? .dvBlue : .dvSub)
        .frame(maxWidth: .infinity)
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
