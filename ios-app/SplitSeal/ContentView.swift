import SwiftUI
import CoreImage.CIFilterBuiltins
import AVFoundation
import UIKit
import Contacts
import ContactsUI
import UserNotifications

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
struct Chat: Identifiable { let id = UUID(); let name: String; let last: String; let time: String; var unread: Int = 0; var devicePub: String = ""; var identityPub: String = ""; var isContact: Bool = false }
enum Kind { case text, image, voice }
enum MsgState { case plain, sealing, opened }
struct Msg: Identifiable {
    let id = UUID(); var text: String; let incoming: Bool; let time: String
    var kind: Kind = .text; var state: MsgState = .plain; var read: Bool = true; var mode: String = "STRICT"
    var sealedFor: String? = nil
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
struct Contact: Identifiable { let id = UUID(); let name: String; let address: String; let devicePub: String; let identityPub: String; let phone: String }

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
    /// Add a contact parsed from a card (from an inbound message) unless one with the same
    /// identity_pub already exists. Returns true if it was newly added.
    @discardableResult
    static func addContactFromCard(_ card: [String: Any]) -> Bool {
        let idp = card["identity_pub"] as? String ?? ""
        var a = contacts()
        if !idp.isEmpty, a.contains(where: { ($0["identity_pub"] as? String) == idp }) { return false }
        a.append([
            "name": card["name"] as? String ?? "Contact",
            "address": card["address"] as? String ?? "",
            "device_pub": card["device_pub"] as? String ?? "",
            "identity_pub": idp,
        ])
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "contacts")
        }
        return true
    }
    /// Remove a saved contact (match by identity_pub when present, else by name).
    static func removeContact(identityPub: String, name: String) {
        var a = contacts()
        a.removeAll { o in
            if !identityPub.isEmpty { return (o["identity_pub"] as? String) == identityPub }
            return (o["name"] as? String) == name
        }
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "contacts")
        }
    }

    // Demo placeholder chats aren't real contacts — hide them by name.
    static func hiddenChats() -> Set<String> { Set(d.stringArray(forKey: "hidden_chats") ?? []) }
    static func hideChat(_ name: String) {
        var s = hiddenChats(); s.insert(name); d.set(Array(s), forKey: "hidden_chats")
    }

    // received messages, persisted + deduped per mailbox tag (all my inbound share my tag).
    static func inbox(_ tag: String) -> [[String: Any]] {
        guard let s = d.string(forKey: "inbox_\(tag)"), let data = s.data(using: .utf8) else { return [] }
        return ((try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]) ?? []
    }
    /// Append a received message (tagged with the sender's identity_pub); false if already stored.
    @discardableResult
    static func addInbox(_ tag: String, _ sealId: String, _ text: String, _ sender: String) -> Bool {
        var a = inbox(tag)
        if a.contains(where: { ($0["id"] as? String) == sealId }) { return false }
        a.append(["id": sealId, "text": text, "sender": sender, "ts": Date().timeIntervalSince1970])
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "inbox_\(tag)")
        }
        return true
    }
    /// Drop every received message from a given sender (used when deleting that conversation).
    static func clearInbox(tag: String, sender: String) {
        guard !tag.isEmpty else { return }
        var a = inbox(tag)
        a.removeAll { ($0["sender"] as? String) == sender }
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "inbox_\(tag)")
        }
    }
}

func loadOrCreateIdentity() -> [String: Any] {
    if let j = Store.identity() { return j }
    let json = SealCore.newIdentity("Me")
    Store.saveIdentity(json)
    return ((try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]) ?? [:]
}
/// Parse a pasted/scanned contact code, tolerant of copy noise: strips whitespace/newlines and
/// auto-adds the "denvion:" prefix if the user copied only the card body. Returns nil if invalid.
func tryParseCard(_ code: String) -> [String: Any]? {
    let cleaned = code.components(separatedBy: .whitespacesAndNewlines).joined()
    guard !cleaned.isEmpty else { return nil }
    let candidates = cleaned.hasPrefix("denvion:") ? [cleaned] : [cleaned, "denvion:" + cleaned]
    for cand in candidates {
        let res = SealCore.parseCard(cand)
        if let d = (try? JSONSerialization.jsonObject(with: Data(res.utf8))) as? [String: Any], (d["ok"] as? Bool) == true {
            return d
        }
    }
    return nil
}

func loadContacts() -> [Contact] {
    Store.contacts().map {
        Contact(name: $0["name"] as? String ?? "", address: $0["address"] as? String ?? "",
                devicePub: $0["device_pub"] as? String ?? "", identityPub: $0["identity_pub"] as? String ?? "",
                phone: $0["phone"] as? String ?? "")
    }
}

func qrImage(_ text: String) -> Image? {
    let filter = CIFilter.qrCodeGenerator()
    filter.message = Data(text.utf8)
    guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 8, y: 8)),
          let cg = CIContext().createCGImage(output, from: output.extent) else { return nil }
    return Image(decorative: cg, scale: 1, orientation: .up)
}

// ── Configurable backend: relay + 3 gateways + directory all live on ONE host. ──
// Default = WCAHT node N6 (reachable from any device). Override it in Settings — e.g.
// "127.0.0.1" for services on the simulator's own Mac. Only a hostname/IP; ports are fixed.
// The servers only ever see ciphertext + hashes, never keys or plaintext.
// (http needs an NSAppTransportSecurity / NSAllowsLocalNetworking exception in Info.plist.)
enum Server {
    static let defaultHost = "51.79.176.134" // N6
    static var host: String {
        get { UserDefaults.standard.string(forKey: "server_host") ?? defaultHost }
        set { UserDefaults.standard.set(newValue.trimmingCharacters(in: .whitespaces), forKey: "server_host") }
    }
}
var directoryURL: String { "http://\(Server.host):9988" }

func directoryLookup(_ phone: String) async -> [String: Any]? {
    let commitJson = SealCore.phoneCommitment(phone)
    guard let cd = (try? JSONSerialization.jsonObject(with: Data(commitJson.utf8))) as? [String: Any],
          let commit = cd["phone_commitment"] as? String,
          let url = URL(string: "\(directoryURL)/lookup/\(commit)"),
          let (data, resp) = try? await URLSession.shared.data(from: url),
          (resp as? HTTPURLResponse)?.statusCode == 200,
          let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
          let card = body["card"] as? String
    else { return nil }
    let res = SealCore.parseCard(card)
    if let d = (try? JSONSerialization.jsonObject(with: Data(res.utf8))) as? [String: Any], (d["ok"] as? Bool) == true {
        return d
    }
    return nil
}

/// Publish my phone → my card to the directory (server stores only the hash).
func directoryPublish(_ phone: String, _ card: String) async -> Bool {
    let commitJson = SealCore.phoneCommitment(phone)
    guard let cd = (try? JSONSerialization.jsonObject(with: Data(commitJson.utf8))) as? [String: Any],
          let commit = cd["phone_commitment"] as? String,
          let url = URL(string: "\(directoryURL)/register") else { return false }
    var req = URLRequest(url: url)
    req.httpMethod = "POST"
    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
    req.httpBody = try? JSONSerialization.data(withJSONObject: ["commitment": commit, "card": card])
    guard let (_, resp) = try? await URLSession.shared.data(for: req) else { return false }
    return (resp as? HTTPURLResponse)?.statusCode == 200
}

// Delivery services (see Server above). Ship ciphertext to the relay + shares to gateways.
var relayURL: String { "http://\(Server.host):9200" }
var gatewayURLs: [String] { [9201, 9202, 9203].map { "http://\(Server.host):\($0)" } }

func httpPost(_ urlStr: String, _ body: String) async {
    guard let url = URL(string: urlStr) else { return }
    var req = URLRequest(url: url)
    req.httpMethod = "POST"
    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
    req.httpBody = body.data(using: .utf8)
    _ = try? await URLSession.shared.data(for: req)
}
func httpGet(_ urlStr: String) async -> String? {
    guard let url = URL(string: urlStr),
          let (data, resp) = try? await URLSession.shared.data(from: url),
          (resp as? HTTPURLResponse)?.statusCode == 200 else { return nil }
    return String(data: data, encoding: .utf8)
}
func shipSeal(_ ship: [String: Any]) async {
    let tag = ship["mailbox_tag"] as? String ?? ""
    let sealId = ship["seal_id"] as? String ?? ""
    // carry seal_id alongside the ciphertext so the recipient (who has neither) can collect shares.
    if let bundle = ship["bundle"] {
        let item: [String: Any] = ["seal_id": sealId, "bundle": bundle]
        if let d = try? JSONSerialization.data(withJSONObject: item), let s = String(data: d, encoding: .utf8) {
            await httpPost("\(relayURL)/inbox/\(tag)", s)
        }
    }
    if let shares = ship["shares"] as? [Any] {
        let gws = gatewayURLs
        for (i, sh) in shares.enumerated() where i < gws.count {
            if let d = try? JSONSerialization.data(withJSONObject: sh), let s = String(data: d, encoding: .utf8) {
                await httpPost("\(gws[i])/deposit", s)
                await httpPost("\(gws[i])/finalize/\(sealId)", "")
            }
        }
    }
}
/// Fetch every {seal_id, bundle} item delivered to a mailbox tag (recipient polls this).
func fetchInboxAll(_ tag: String) async -> [[String: Any]] {
    guard let body = await httpGet("\(relayURL)/inbox/\(tag)"),
          let arr = (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [[String: Any]] else { return [] }
    return arr
}
func collectShares(_ sealId: String) async -> String {
    var all: [Any] = []
    for gw in gatewayURLs {
        if let body = await httpGet("\(gw)/release/\(sealId)"), let arr = (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [Any] {
            all.append(contentsOf: arr)
        }
    }
    if let d = try? JSONSerialization.data(withJSONObject: all), let s = String(data: d, encoding: .utf8) { return s }
    return "[]"
}

// ── local notifications for inbound messages ──
// The delegate lets notifications show as a banner even while the app is foregrounded
// (the poller runs in-app), matching Android's heads-up behaviour.
final class NotifDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotifDelegate()
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }
}
func requestNotifs() {
    UNUserNotificationCenter.current().delegate = NotifDelegate.shared
    UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
}
func notifyMessage(_ title: String, _ body: String) {
    let c = UNMutableNotificationContent()
    c.title = title; c.body = body; c.sound = .default
    UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
}

// ─────────────────────────────── root nav ───────────────────────────────────
struct ContentView: View {
    @State private var identity: [String: Any] = loadOrCreateIdentity()
    @State private var contacts: [Contact] = loadContacts()
    @State private var hidden: Set<String> = Store.hiddenChats()
    @State private var openChat: Chat? = nil
    @State private var tab = 0
    @State private var showNew = false
    @State private var showScanner = false
    @State private var scanned: [String: Any]? = nil
    @State private var codeError = false
    @State private var seenGlobal = Set<String>()

    /// My own inbox tag — every seal addressed to me lands here; also purged when I delete a chat.
    private func myMailboxTag() -> String {
        let dp = identity["device_pub"] as? String ?? ""
        guard !dp.isEmpty else { return "" }
        let j = SealCore.mailboxTag(dp)
        return ((try? JSONSerialization.jsonObject(with: Data(j.utf8))) as? [String: Any])?["mailbox_tag"] as? String ?? ""
    }

    private func deleteChat(_ c: Chat) {
        if c.isContact {
            Store.removeContact(identityPub: c.identityPub, name: c.name)
            Store.clearInbox(tag: myMailboxTag(), sender: c.identityPub)
            contacts = loadContacts()
        } else {
            Store.hideChat(c.name)
            hidden = Store.hiddenChats()
        }
    }

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
        // tell the other side they were added (they'll get a real inbound → chat + notification)
        if let dp = scanned?["device_pub"] as? String, !dp.isEmpty { sendHello(to: dp) }
        showNew = false; scanned = nil
    }

    /// Ship a small "added you" hello so the peer's device surfaces the chat + a notification.
    private func sendHello(to devicePub: String) {
        let seed = identity["identity_seed"] as? String ?? ""
        let card = identity["card"] as? String ?? ""
        let myName = (identity["name"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Someone"
        Task.detached {
            let shipStr = SealCore.sealShippable(seed, card, devicePub, "👋 \(myName) added you on Denvion", false)
            if let ship = (try? JSONSerialization.jsonObject(with: Data(shipStr.utf8))) as? [String: Any], (ship["ok"] as? Bool) == true {
                await shipSeal(ship)
            }
        }
    }

    /// Rebuild my card under a new display name (identity_pub/address stay stable) and persist it.
    private func saveName(_ newName: String) {
        let nm = newName.trimmingCharacters(in: .whitespaces).isEmpty ? "Me" : newName.trimmingCharacters(in: .whitespaces)
        let rebuilt = SealCore.cardFor(identity["identity_seed"] as? String ?? "", identity["device_seed"] as? String ?? "", nm)
        guard let r = (try? JSONSerialization.jsonObject(with: Data(rebuilt.utf8))) as? [String: Any] else { return }
        var id = identity
        id["name"] = nm
        id["card"] = r["card"] ?? id["card"] ?? ""
        id["address"] = r["address"] ?? id["address"] ?? ""
        if let data = try? JSONSerialization.data(withJSONObject: id), let json = String(data: data, encoding: .utf8) {
            Store.saveIdentity(json)
            identity = id
        }
    }

    /// Always-on inbox poll while NOT in a conversation: receive even when the chat isn't open,
    /// auto-create the sender as a replyable contact (from the embedded card), store + notify.
    @MainActor private func globalPoll() async {
        let tag = myMailboxTag()
        guard openChat == nil, !tag.isEmpty else { return }
        for o in Store.inbox(tag) { if let id = o["id"] as? String { seenGlobal.insert(id) } }
        while !Task.isCancelled {
            let deviceSeed = identity["device_seed"] as? String ?? ""
            for item in await fetchInboxAll(tag) {
                guard let sealId = item["seal_id"] as? String, !sealId.isEmpty, !seenGlobal.contains(sealId),
                      let bundle = item["bundle"],
                      let bd = try? JSONSerialization.data(withJSONObject: bundle),
                      let bundleStr = String(data: bd, encoding: .utf8) else { continue }
                let shares = await collectShares(sealId)
                let openStr = SealCore.openReceived(deviceSeed, bundleStr, shares)
                if let r = (try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any],
                   (r["ok"] as? Bool) == true, let plain = r["plaintext"] as? String {
                    seenGlobal.insert(sealId)
                    let bmap = item["bundle"] as? [String: Any]
                    let sender = bmap?["sender_id_pub"] as? String ?? ""
                    var senderName = "New message"
                    if let cardCode = bmap?["sender_card"] as? String, !cardCode.isEmpty {
                        let parsed = SealCore.parseCard(cardCode)
                        if let pj = (try? JSONSerialization.jsonObject(with: Data(parsed.utf8))) as? [String: Any], (pj["ok"] as? Bool) == true {
                            if let n = pj["name"] as? String, !n.isEmpty { senderName = n }
                            if Store.addContactFromCard(pj) { contacts = loadContacts() }
                        }
                    }
                    if Store.addInbox(tag, sealId, plain, sender) { notifyMessage(senderName, plain) }
                }
            }
            try? await Task.sleep(nanoseconds: 3_000_000_000)
        }
    }

    var body: some View {
        Group {
            if let c = openChat {
                ConversationView(
                    chat: c,
                    myDeviceSeed: identity["device_seed"] as? String ?? "",
                    myDevicePub: identity["device_pub"] as? String ?? "",
                    myIdentitySeed: identity["identity_seed"] as? String ?? "",
                    myCard: identity["card"] as? String ?? "",
                    onBack: { openChat = nil }
                )
            } else if showNew {
                NewContactView(
                    scanned: scanned,
                    onScan: { showScanner = true },
                    onLookup: { phone in Task { if let card = await directoryLookup(phone) { scanned = card } } },
                    onPasteCode: { c in
                        if let d = tryParseCard(c) { scanned = d } else { codeError = true }
                    },
                    onCancel: { showNew = false; scanned = nil },
                    onSave: saveContact
                )
            } else if tab == 2 {
                ProfileView(identity: identity, tab: tab, onTab: { tab = $0 }, onSaveName: { saveName($0) }, onPublish: { phone in
                    var id = identity
                    id["phone"] = phone
                    if let data = try? JSONSerialization.data(withJSONObject: id), let json = String(data: data, encoding: .utf8) {
                        Store.saveIdentity(json)
                        identity = id
                    }
                    let card = identity["card"] as? String ?? ""
                    Task { _ = await directoryPublish(phone, card) }
                })
            } else {
                ChatListView(contacts: contacts, hidden: hidden, onOpen: { openChat = $0 }, onAdd: { showNew = true; scanned = nil }, onDelete: deleteChat, tab: tab, onTab: { tab = $0 })
            }
        }
        .fullScreenCover(isPresented: $showScanner) {
            QRScannerView(
                onFound: { code in
                    showScanner = false
                    if let d = tryParseCard(code) { scanned = d } else { codeError = true }
                },
                onClose: { showScanner = false }
            )
        }
        .alert("Not a contact code", isPresented: $codeError) {
            Button("OK", role: .cancel) {}
        } message: {
            Text("Copy the full \"denvion:…\" code from the other person's Settings (under \"Share this code\") — not their address.")
        }
        .onAppear { requestNotifs() }
        // run the always-on inbox poll whenever we're NOT inside a conversation
        .task(id: openChat == nil) { await globalPoll() }
    }
}

// ─────────────────────────────── chat list ──────────────────────────────────
struct ChatListView: View {
    let contacts: [Contact]
    let hidden: Set<String>
    let onOpen: (Chat) -> Void
    let onAdd: () -> Void
    let onDelete: (Chat) -> Void
    let tab: Int
    let onTab: (Int) -> Void
    @State private var pendingDelete: Chat? = nil
    var body: some View {
        let rows = contacts.map { c -> Chat in
            let sub = !c.address.isEmpty ? String(c.address.prefix(14)) + "… · tap to seal"
                : (!c.phone.isEmpty ? "+855 " + c.phone : "tap to seal")
            return Chat(name: c.name, last: sub, time: "", devicePub: c.devicePub, identityPub: c.identityPub, isContact: true)
        } + CHATS.filter { !hidden.contains($0.name) }
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
                            ChatRow(c).contentShape(Rectangle())
                                .onTapGesture { onOpen(c) }
                                .contextMenu {
                                    Button(role: .destructive) { pendingDelete = c } label: { Label("Delete Chat", systemImage: "trash") }
                                }
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
        .alert("Delete chat", isPresented: Binding(get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } })) {
            Button("Cancel", role: .cancel) { pendingDelete = nil }
            Button("Delete", role: .destructive) { if let c = pendingDelete { onDelete(c) }; pendingDelete = nil }
        } message: {
            Text("Delete your conversation with \(pendingDelete?.name ?? "")? This removes it from this device.")
        }
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
    let onSaveName: (String) -> Void
    let onPublish: (String) -> Void
    @State private var myPhone: String = ""
    @State private var myName: String = ""
    @State private var host: String = Server.host
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

                    VStack(spacing: 6) {
                        Text("Your display name").font(.system(size: 13)).foregroundColor(.dvSub)
                        Text("This is the name shown when others add you or get your messages.")
                            .font(.system(size: 11)).foregroundColor(.dvSub).multilineTextAlignment(.center)
                        TextField("your name", text: $myName)
                            .textInputAutocapitalization(.words).foregroundColor(.dvInk).multilineTextAlignment(.center)
                            .padding(12).background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 12))
                        Button(action: { onSaveName(myName) }) {
                            Text("Save name")
                                .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                                .frame(maxWidth: .infinity).padding(.vertical, 14)
                                .background(myName.trimmingCharacters(in: .whitespaces).isEmpty || myName == name ? Color(hex: 0xCBD3DA) : Color.dvBlue)
                                .clipShape(RoundedRectangle(cornerRadius: 12))
                        }.disabled(myName.trimmingCharacters(in: .whitespaces).isEmpty || myName == name)
                    }
                    .onAppear { if myName.isEmpty { myName = name } }

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
                    Button(action: { UIPasteboard.general.string = card }) {
                        Label("Copy my code", systemImage: "doc.on.doc")
                            .font(.system(size: 13, weight: .semibold)).foregroundColor(.white)
                            .padding(.horizontal, 18).padding(.vertical, 10)
                            .background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 10))
                    }

                    Divider().padding(.vertical, 8)
                    Text("Let others add you by phone number").font(.system(size: 13)).foregroundColor(.dvSub)
                    HStack {
                        Text("+855").foregroundColor(.dvInk)
                        TextField("your number", text: $myPhone).keyboardType(.numberPad).foregroundColor(.dvInk)
                    }
                    .padding(12).background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 12))
                    Button(action: { if !myPhone.isEmpty { onPublish(myPhone.trimmingCharacters(in: .whitespaces)) } }) {
                        Text("Publish my number to the directory")
                            .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                            .frame(maxWidth: .infinity).padding(.vertical, 14)
                            .background(myPhone.isEmpty ? Color(hex: 0xCBD3DA) : Color.dvBlue)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }.disabled(myPhone.isEmpty)
                    Text("Only a hash of your number is stored — never the number itself.")
                        .font(.system(size: 11)).foregroundColor(.dvSub).multilineTextAlignment(.center)

                    Divider().padding(.vertical, 8)
                    Text("Server").font(.system(size: 13)).foregroundColor(.dvSub)
                    Text("Host running the relay + gateways + directory. Both people must point at the same one.")
                        .font(.system(size: 11)).foregroundColor(.dvSub).multilineTextAlignment(.center)
                    TextField("server IP or hostname", text: $host)
                        .textInputAutocapitalization(.never).foregroundColor(.dvInk)
                        .padding(12).background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 12))
                    Button(action: {
                        let h = host.trimmingCharacters(in: .whitespaces)
                        if !h.isEmpty { Server.host = h; host = h }
                    }) {
                        Text("Save server")
                            .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                            .frame(maxWidth: .infinity).padding(.vertical, 14)
                            .background(host.isEmpty ? Color(hex: 0xCBD3DA) : Color.dvBlue)
                            .clipShape(RoundedRectangle(cornerRadius: 12))
                    }.disabled(host.isEmpty)
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
    let onLookup: (String) -> Void
    let onPasteCode: (String) -> Void
    let onCancel: () -> Void
    let onSave: (String, String, String) -> Void
    @State private var first = ""
    @State private var last = ""
    @State private var phone = ""
    @State private var sync = true
    @State private var country = iosCountries[0]
    @State private var showSync = false

    private func doSave() {
        onSave(first.trimmingCharacters(in: .whitespaces), last.trimmingCharacters(in: .whitespaces), phone.trimmingCharacters(in: .whitespaces))
    }

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
                    guard canSave else { return }
                    let n = [first, last].map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " ")
                    if sync && (!n.isEmpty || !phone.isEmpty) { showSync = true } else { doSave() }
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
                        Menu {
                            ForEach(iosCountries) { c in
                                Button("\(c.flag)  \(c.name)  +\(c.dial)") { country = c }
                            }
                        } label: {
                            HStack {
                                Text("\(country.flag)  \(country.name)").foregroundColor(.dvInk)
                                Spacer()
                                Text("+\(country.dial)").foregroundColor(.dvSub)
                                Image(systemName: "chevron.down").foregroundColor(.dvSub)
                            }.padding(16)
                        }
                        Divider().padding(.leading, 16)
                        HStack {
                            Text("+\(country.dial)").foregroundColor(.dvInk)
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

                    // Paste the denvion: contact code directly — one button, no editable field
                    // (an editable field pops the text-selection Magnifier, which crashes on the emulator).
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Or add by contact code").font(.system(size: 13)).foregroundColor(.dvSub)
                        Text("On the other phone: Settings ▸ Copy my code, then tap Paste here.")
                            .font(.system(size: 11)).foregroundColor(.dvSub)
                        Button(action: { onPasteCode(UIPasteboard.general.string ?? "") }) {
                            Label("Paste contact code", systemImage: "doc.on.clipboard")
                                .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                                .frame(maxWidth: .infinity).padding(.vertical, 14)
                                .background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 10))
                        }
                    }
                    .padding(16).background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))

                    if !phone.isEmpty && scanned == nil {
                        Button(action: { onLookup(phone) }) {
                            HStack {
                                Image(systemName: "magnifyingglass")
                                Text("Look up this number on the directory")
                                Spacer()
                            }.foregroundColor(.dvBlue).padding(16)
                        }
                        .background(Color.white).clipShape(RoundedRectangle(cornerRadius: 14))
                    }

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
        .sheet(isPresented: $showSync, onDismiss: { doSave() }) {
            AddToContactsSheet(
                name: [first, last].map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " "),
                phone: phone.isEmpty ? "" : "+\(country.dial)\(phone.filter { $0.isNumber })",
                onDone: { showSync = false }
            )
        }
    }
}

struct IOSCountry: Identifiable {
    let id = UUID(); let flag: String; let name: String; let dial: String
}
let iosCountries: [IOSCountry] = [
    IOSCountry(flag: "🇰🇭", name: "Cambodia", dial: "855"),
    IOSCountry(flag: "🇺🇸", name: "United States", dial: "1"),
    IOSCountry(flag: "🇬🇧", name: "United Kingdom", dial: "44"),
    IOSCountry(flag: "🇻🇳", name: "Vietnam", dial: "84"),
    IOSCountry(flag: "🇹🇭", name: "Thailand", dial: "66"),
    IOSCountry(flag: "🇸🇬", name: "Singapore", dial: "65"),
    IOSCountry(flag: "🇮🇳", name: "India", dial: "91"),
    IOSCountry(flag: "🇦🇺", name: "Australia", dial: "61"),
    IOSCountry(flag: "🇯🇵", name: "Japan", dial: "81"),
    IOSCountry(flag: "🇨🇳", name: "China", dial: "86"),
]

/// Presents the system "new contact" editor pre-filled (mirrors Android's ACTION_INSERT).
/// Presenting needs no permission; the user taps Done to save into their contacts.
struct AddToContactsSheet: UIViewControllerRepresentable {
    let name: String
    let phone: String
    let onDone: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onDone: onDone) }

    func makeUIViewController(context: Context) -> UINavigationController {
        let c = CNMutableContact()
        let parts = name.split(separator: " ", maxSplits: 1).map(String.init)
        c.givenName = parts.first ?? name
        if parts.count > 1 { c.familyName = parts[1] }
        if !phone.isEmpty {
            c.phoneNumbers = [CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(string: phone))]
        }
        let vc = CNContactViewController(forNewContact: c)
        vc.delegate = context.coordinator
        return UINavigationController(rootViewController: vc)
    }

    func updateUIViewController(_ vc: UINavigationController, context: Context) {}

    final class Coordinator: NSObject, CNContactViewControllerDelegate {
        let onDone: () -> Void
        init(onDone: @escaping () -> Void) { self.onDone = onDone }
        func contactViewController(_ vc: CNContactViewController, didCompleteWith contact: CNContact?) {
            onDone()
        }
    }
}

// ────────────────────────────── conversation ────────────────────────────────
struct ConversationView: View {
    let chat: Chat
    let myDeviceSeed: String
    let myDevicePub: String
    let myIdentitySeed: String
    let myCard: String
    let onBack: () -> Void
    @State private var messages: [Msg] = []
    @State private var draft = ""
    @State private var fastMode = false
    @State private var seen = Set<String>()
    @State private var polling = false
    @State private var myTag = ""

    // A "real" conversation is linked to a contact's device key (vs. the demo threads).
    private var real: Bool { !chat.devicePub.isEmpty }

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
        .task {
            // one-time: compute my inbox tag + restore persisted history, then live-poll
            if myTag.isEmpty, !myDevicePub.isEmpty {
                let j = SealCore.mailboxTag(myDevicePub)
                if let d = (try? JSONSerialization.jsonObject(with: Data(j.utf8))) as? [String: Any] {
                    myTag = d["mailbox_tag"] as? String ?? ""
                }
            }
            if real {
                if messages.isEmpty {
                    // restore only messages FROM this contact; seed `seen` with every id so poll skips them.
                    for o in Store.inbox(myTag) {
                        if let id = o["id"] as? String { seen.insert(id) }
                        if (o["sender"] as? String) == chat.identityPub {
                            messages.append(Msg(text: o["text"] as? String ?? "", incoming: true, time: "", state: .opened))
                        }
                    }
                }
                while !Task.isCancelled {
                    await poll()
                    try? await Task.sleep(nanoseconds: 3_000_000_000)
                }
            } else if messages.isEmpty {
                messages = seedThread()
            }
        }
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

    // Poll my inbox: fetch delivered ciphertext, collect released shares, open, show. One at a time.
    @MainActor private func poll() async {
        guard real, !myTag.isEmpty, !polling else { return }
        polling = true
        defer { polling = false }
        for item in await fetchInboxAll(myTag) {
            guard let sealId = item["seal_id"] as? String, !sealId.isEmpty, !seen.contains(sealId),
                  let bundle = item["bundle"],
                  let bd = try? JSONSerialization.data(withJSONObject: bundle),
                  let bundleStr = String(data: bd, encoding: .utf8) else { continue }
            let sender = (item["bundle"] as? [String: Any])?["sender_id_pub"] as? String ?? ""
            let shares = await collectShares(sealId)
            let openStr = SealCore.openReceived(myDeviceSeed, bundleStr, shares)
            if let r = (try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any],
               (r["ok"] as? Bool) == true, let plain = r["plaintext"] as? String {
                seen.insert(sealId)
                // persist under my inbox tagged by sender; only SHOW it in this contact's chat.
                if Store.addInbox(myTag, sealId, plain, sender) && sender == chat.identityPub {
                    messages.append(Msg(text: plain, incoming: true, time: nowTime(), state: .opened))
                }
            }
            // if not opened yet (shares still locked) leave it unseen to retry next poll
        }
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let fast = fastMode
        messages.append(Msg(text: text, incoming: false, time: nowTime(), state: .sealing, mode: fast ? "FAST" : "STRICT"))
        let idx = messages.count - 1
        draft = ""
        Task { @MainActor in
            if real {
                // seal + SHIP over the relay + gateways; the recipient's device polls + opens.
                var ok = false
                let shipStr = SealCore.sealShippable(myIdentitySeed, myCard, chat.devicePub, text, fast)
                if let ship = (try? JSONSerialization.jsonObject(with: Data(shipStr.utf8))) as? [String: Any], (ship["ok"] as? Bool) == true {
                    await shipSeal(ship)
                    ok = true
                }
                try? await Task.sleep(nanoseconds: fast ? 400_000_000 : 800_000_000)
                if messages.indices.contains(idx) {
                    messages[idx].state = ok ? .opened : .sealing
                    messages[idx].sealedFor = chat.name
                }
                await poll() // pick up a self-loopback / any pending inbound right away
            } else {
                let json = fast ? SealCore.runFastDemo() : SealCore.runDemo()
                try? await Task.sleep(nanoseconds: fast ? 550_000_000 : 1_150_000_000)
                let opened = json.contains("OPENED")
                if messages.indices.contains(idx) { messages[idx].state = opened ? .opened : .sealing }
            }
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
            if let who = m.sealedFor {
                Image(systemName: "lock.fill").font(.system(size: 10)).foregroundColor(.dvBlue)
                Text("Sealed for \(who)").font(.system(size: 10)).foregroundColor(.dvBlue)
                Spacer().frame(width: 4)
            }
            Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub)
            if !m.incoming && m.sealedFor == nil {
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
