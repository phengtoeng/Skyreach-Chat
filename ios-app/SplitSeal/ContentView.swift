import SwiftUI
import CoreImage.CIFilterBuiltins
import AVFoundation
import AVKit
import PhotosUI
import UniformTypeIdentifiers
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
    static let dvBarActive = Color(hex: 0x101720).opacity(0.10)  // pill behind the selected tab
    static let dvBarEdge = Color.white.opacity(0.60)             // hairline highlight on the glass
    static let dvBadge = Color(hex: 0xF03D3D)
}

// Bottom-bar tabs, left to right.
let TAB_CONTACTS = 0
let TAB_CALLS = 1
let TAB_CHATS = 2
let TAB_SETTINGS = 3

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
enum MsgState {
    case plain, sealing, opened
    /// Received but time-locked: the gateways withhold the shares until `revealAt`, so there is
    /// nothing to render yet. The bubble shows a lock + countdown, never a hint of the content.
    case locked
}
struct Msg: Identifiable {
    let id = UUID(); var text: String; let incoming: Bool; let time: String
    var kind: Kind = .text; var state: MsgState = .plain; var read: Bool = true; var mode: String = "STRICT"
    var sealedFor: String? = nil
    var revealAt: Int64 = 0  // unix secs: timelocked to open at/after this time (0 = none)
    var destroyAt: Int64 = 0 // unix secs: self-destructs after this time (0 = none)
    var mediaPath: String = "" // decrypted file on THIS device only; never uploaded
    var mediaMime: String = ""
    /// Set on a LOCKED placeholder so the real item can replace it when the window opens.
    var lockedSealId: String = ""
    /// The seal this bubble came from, so the live poll can tell "already on screen" apart from
    /// "already in the store". Those are NOT the same: two poll loops run, and whichever gets
    /// there first consumes the store's one-shot dedup — which used to mean the message never
    /// appeared until you left the chat and came back. Dedup the UI on this, not on the store.
    var sealId: String = ""
    /// True only for the bubble that replaced a LOCKED placeholder this session — it earns the
    /// gradient halo. Not persisted: a timelock opens once, and reopening the chat later should
    /// show an ordinary photo, not a fresh celebration.
    var justRevealed: Bool = false
}

/// Give up opening a seal after this many *permanent* failures (see `openFailures`).
let MAX_OPEN_ATTEMPTS = 5

/// True only when a seal can NEVER open, so it is safe to stop retrying it.
///
/// Being wrong in the "permanent" direction silently loses a real message, so this is a
/// whitelist of transient states rather than a blacklist: anything about the timelock, missing
/// key shares, chain finality or quorum is a gate that clears on its own and must keep retrying.
/// Only failures no amount of waiting can fix count against the give-up budget.
func isPermanentOpenFailure(_ reason: String) -> Bool {
    let r = reason.lowercased()
    let transient = ["locked", "share", "final", "pending", "quorum", "not yet", "await"]
    return !transient.contains { r.contains($0) }
}

/// Short "time from now" label for a unix-seconds target, e.g. "10m", "1h", "1d".
func relLabel(_ target: Int64) -> String {
    let s = target - Int64(Date().timeIntervalSince1970)
    if s <= 0 { return "now" }
    if s < 3600 { return "\(max(1, s / 60))m" }
    if s < 86400 { return "\(s / 3600)h" }
    return "\(s / 86400)d"
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

    // Per-conversation transcript (BOTH directions), keyed by the peer's identity_pub — the durable
    // chat history. (inbox_<tag> above is only the opened-seal dedup set for incoming.)
    static func thread(_ peer: String) -> [[String: Any]] {
        guard !peer.isEmpty, let s = d.string(forKey: "thread_\(peer)"), let data = s.data(using: .utf8) else { return [] }
        return ((try? JSONSerialization.jsonObject(with: data)) as? [[String: Any]]) ?? []
    }
    /// Append a message to a conversation (dedup by id); false if already stored.
    @discardableResult
    static func addThreadMsg(
        _ peer: String, _ id: String, _ text: String, _ incoming: Bool,
        media: String = "", // local path to the DECRYPTED file (this device only)
        mime: String = "",
        destroyAt: Int64 = 0 // persisted so a self-destruct still fires after an app restart
    ) -> Bool {
        guard !peer.isEmpty else { return false }
        var a = thread(peer)
        if a.contains(where: { ($0["id"] as? String) == id }) { return false }
        a.append(["id": id, "text": text, "incoming": incoming, "ts": Date().timeIntervalSince1970,
                  "media": media, "mime": mime, "destroy_at": destroyAt])
        if let out = try? JSONSerialization.data(withJSONObject: a), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "thread_\(peer)")
        }
        return true
    }
    static func clearThread(_ peer: String) { if !peer.isEmpty { d.removeObject(forKey: "thread_\(peer)") } }

    /// Drop a self-destructed item from the durable transcript so it never comes back on reopen.
    static func removeThreadMedia(_ peer: String, _ mediaPath: String) {
        guard !peer.isEmpty, !mediaPath.isEmpty else { return }
        let keep = thread(peer).filter { ($0["media"] as? String) != mediaPath }
        if let out = try? JSONSerialization.data(withJSONObject: keep), let s = String(data: out, encoding: .utf8) {
            d.set(s, forKey: "thread_\(peer)")
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
// Replicated directories (gossip + persist server-side): register to ALL, look up on ANY.
var directoryURLs: [String] { nodeHosts.map { "http://\($0):9988" } }
// Seal batchers: every message's leaf goes here to be committed under a SEAL_ROOT, and
// recipients fetch the merkle proof that their message is inside that finalised root.
var batcherURLs: [String] {
    serverPinned ? ["http://\(Server.host):9300"] : nodeHosts.map { "http://\($0):9300" }
}

func directoryLookup(_ phone: String) async -> [String: Any]? {
    let commitJson = SealCore.phoneCommitment(phone)
    guard let cd = (try? JSONSerialization.jsonObject(with: Data(commitJson.utf8))) as? [String: Any],
          let commit = cd["phone_commitment"] as? String else { return nil }
    // try each directory node until one answers (any replica resolves — survives a node outage).
    for base in directoryURLs {
        guard let url = URL(string: "\(base)/lookup/\(commit)"),
              let (data, resp) = try? await URLSession.shared.data(from: url),
              (resp as? HTTPURLResponse)?.statusCode == 200,
              let body = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let card = body["card"] as? String else { continue }
        let res = SealCore.parseCard(card)
        if let d = (try? JSONSerialization.jsonObject(with: Data(res.utf8))) as? [String: Any], (d["ok"] as? Bool) == true {
            return d
        }
    }
    return nil
}

/// Publish my phone → my card to every directory (server stores only the hash; they also gossip).
func directoryPublish(_ phone: String, _ card: String) async -> Bool {
    let commitJson = SealCore.phoneCommitment(phone)
    guard let cd = (try? JSONSerialization.jsonObject(with: Data(commitJson.utf8))) as? [String: Any],
          let commit = cd["phone_commitment"] as? String,
          let payload = try? JSONSerialization.data(withJSONObject: ["commitment": commit, "card": card]) else { return false }
    var ok = false
    for base in directoryURLs {
        guard let url = URL(string: "\(base)/register") else { continue }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = payload
        if let (_, resp) = try? await URLSession.shared.data(for: req), (resp as? HTTPURLResponse)?.statusCode == 200 {
            ok = true
        }
    }
    return ok
}

// The seal backbone runs on all 3 nodes (N5, N6, N7) so no single node is a point of failure.
let nodeHosts = ["139.99.150.23", "51.79.176.134", "51.79.162.80"]
/// True when Settings points us at one specific machine instead of the live backbone.
private var serverPinned: Bool { !Server.host.isEmpty && Server.host != Server.defaultHost }

// Replicated relays: ship the ciphertext to ALL, read from ALL (merge) — delivery survives any
// node outage as long as one relay that got the message is up.
// Pinned to one host (dev/self-host), everything runs on that machine instead.
var relayURLs: [String] { serverPinned ? ["http://\(Server.host):9200"] : nodeHosts.map { "http://\($0):9200" } }
// 3 INDEPENDENT gateways, one per node (t=2 of 3): no single machine holds all key shares,
// and any one gateway can be down and messages still open. Pinned to one host they sit on
// consecutive ports — convenient for a local stack, but NOT independent, so dev only.
var gatewayURLs: [String] {
    serverPinned
        ? ["http://\(Server.host):9201", "http://\(Server.host):9202", "http://\(Server.host):9203"]
        : nodeHosts.map { "http://\($0):9201" }
}

// ───────────────────────── media blob transport ─────────────────────────────

/// Upload one encrypted media chunk to EVERY relay, addressed by its ciphertext hash.
/// The relay verifies the hash matches the bytes, so it cannot substitute a chunk — and it
/// holds no key, so it can never open one. True if at least one relay stored it.
func uploadBlob(_ hashHex: String, _ bytes: Data) async -> Bool {
    var ok = false
    for r in relayURLs {
        guard let url = URL(string: "\(r)/blob/\(hashHex)") else { continue }
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.timeoutInterval = 60 // uploads are not 4-second work
        req.setValue("application/octet-stream", forHTTPHeaderField: "Content-Type")
        if let (_, resp) = try? await URLSession.shared.upload(for: req, from: bytes),
           (200..<300).contains((resp as? HTTPURLResponse)?.statusCode ?? -1) { ok = true }
    }
    return ok
}

/// Fetch one encrypted chunk from whichever relay still has it.
func downloadBlob(_ hashHex: String) async -> Data? {
    for r in relayURLs {
        guard let url = URL(string: "\(r)/blob/\(hashHex)") else { continue }
        var req = URLRequest(url: url)
        req.timeoutInterval = 60
        if let (data, resp) = try? await URLSession.shared.data(for: req),
           (resp as? HTTPURLResponse)?.statusCode == 200 { return data }
    }
    return nil
}

@discardableResult
func httpPost(_ urlStr: String, _ body: String) async -> Int {
    guard let url = URL(string: urlStr) else { return -1 }
    var req = URLRequest(url: url)
    req.httpMethod = "POST"
    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
    req.httpBody = body.data(using: .utf8)
    guard let (_, resp) = try? await URLSession.shared.data(for: req) else { return -1 }
    return (resp as? HTTPURLResponse)?.statusCode ?? -1
}
func httpGet(_ urlStr: String) async -> String? {
    guard let url = URL(string: urlStr),
          let (data, resp) = try? await URLSession.shared.data(from: url),
          (resp as? HTTPURLResponse)?.statusCode == 200 else { return nil }
    return String(data: data, encoding: .utf8)
}
/// Ship a seal; returns true only if the relay actually accepted the ciphertext (delivery signal).
@discardableResult
func shipSeal(_ ship: [String: Any]) async -> Bool {
    let tag = ship["mailbox_tag"] as? String ?? ""
    let sealId = ship["seal_id"] as? String ?? ""
    var relayOk = false
    // the leaf also goes to the batchers, so this message ends up inside a committed SEAL_ROOT
    if let b = ship["bundle"] as? [String: Any] { await submitLeafForBatching(b) }
    // carry seal_id alongside the ciphertext so the recipient (who has neither) can collect shares.
    // replicate to every relay so any one of them can serve the recipient.
    if let bundle = ship["bundle"] {
        let item: [String: Any] = ["seal_id": sealId, "bundle": bundle]
        if let d = try? JSONSerialization.data(withJSONObject: item), let s = String(data: d, encoding: .utf8) {
            for r in relayURLs {
                let code = await httpPost("\(r)/inbox/\(tag)", s)
                if (200..<300).contains(code) { relayOk = true }
            }
        }
    }
    // The timelock travels to the gateways as the SIGNED LEAF, not as bare numbers: the
    // gateway verifies the sender signature and reads the window out of the leaf, so nobody
    // else can install a different one.
    var window = "{}"
    if let b = ship["bundle"] as? [String: Any], let leaf = b["signed_leaf"] {
        let payload: [String: Any] = ["signed_leaf": leaf, "sender_id_pub": b["sender_id_pub"] as? String ?? ""]
        window = (try? JSONSerialization.data(withJSONObject: payload))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }
    if let shares = ship["shares"] as? [Any] {
        let gws = gatewayURLs
        for (i, sh) in shares.enumerated() where i < gws.count {
            if let d = try? JSONSerialization.data(withJSONObject: sh), let s = String(data: d, encoding: .utf8) {
                await httpPost("\(gws[i])/deposit", s)
                await httpPost("\(gws[i])/finalize/\(sealId)", window)
            }
        }
    }
    return relayOk
}
// ───────────────────────────── media send / receive ─────────────────────────
//
// The picked file is written into the app's cache so Rust can chunk-encrypt it by path, and
// ONLY the encrypted chunks are uploaded. The readable original never leaves the device.

private var cacheDir: URL { FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0] }
private var mediaDir: URL {
    let d = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("media")
    try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
    return d
}

/// A deliberately TINY thumbnail: it is sealed inside the manifest, so only the recipient ever
/// sees it — and keeping it small is what makes it a blur rather than a readable preview
/// (spec §10.3: never a readable thumbnail for a locked item).
func buildPreview(_ src: URL, isVideo: Bool) -> URL? {
    var image: UIImage?
    if isVideo {
        let asset = AVURLAsset(url: src)
        let gen = AVAssetImageGenerator(asset: asset)
        gen.appliesPreferredTrackTransform = true
        if let cg = try? gen.copyCGImage(at: .zero, actualTime: nil) { image = UIImage(cgImage: cg) }
    } else {
        image = UIImage(contentsOfFile: src.path)
    }
    guard let img = image else { return nil }
    let size = CGSize(width: 32, height: 32)
    let small = UIGraphicsImageRenderer(size: size).image { _ in img.draw(in: CGRect(origin: .zero, size: size)) }
    guard let data = small.jpegData(compressionQuality: 0.6) else { return nil }
    let out = cacheDir.appendingPathComponent("preview-\(UUID().uuidString).jpg")
    try? data.write(to: out)
    return out
}

/// Upload every encrypted chunk listed by `sealMediaFile`, then delete the local copies.
func uploadChunks(_ sealed: [String: Any]) async -> Bool {
    guard let chunks = sealed["chunks"] as? [[String: Any]] else { return false }
    for c in chunks {
        guard let path = c["path"] as? String, let hash = c["hash"] as? String,
              let bytes = FileManager.default.contents(atPath: path) else { return false }
        let stored = await uploadBlob(hash, bytes)
        if !stored { return false }
        try? FileManager.default.removeItem(atPath: path) // the relay has it; don't keep ciphertext
    }
    return true
}

/// A video picked from the library, delivered as a FILE rather than as `Data` — a 40 MB
/// clip loaded as `Data` would sit in memory in full, and Rust wants a path anyway.
struct PickedMovie: Transferable {
    let url: URL
    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(contentType: .movie) { movie in
            SentTransferredFile(movie.url)
        } importing: { received in
            let dest = FileManager.default.temporaryDirectory.appendingPathComponent("pick-\(UUID().uuidString).mov")
            try? FileManager.default.removeItem(at: dest)
            try FileManager.default.copyItem(at: received.file, to: dest)
            return Self(url: dest)
        }
    }
}

/// Decoded thumbnails, cached by path. A bubble's body re-runs on every scroll pass, and
/// re-decoding a multi-megabyte image each time makes the list crawl.
enum MediaThumbs {
    private static let cache = NSCache<NSString, UIImage>()

    static func thumb(path: String, isVideo: Bool) -> UIImage? {
        guard !path.isEmpty else { return nil }
        if let hit = cache.object(forKey: path as NSString) { return hit }
        var image: UIImage?
        if isVideo {
            let gen = AVAssetImageGenerator(asset: AVURLAsset(url: URL(fileURLWithPath: path)))
            gen.appliesPreferredTrackTransform = true
            if let cg = try? gen.copyCGImage(at: .zero, actualTime: nil) { image = UIImage(cgImage: cg) }
        } else {
            image = UIImage(contentsOfFile: path)
        }
        if let img = image { cache.setObject(img, forKey: path as NSString) }
        return image
    }
}

/// Receive one media seal: open the manifest, fetch every chunk the relay is holding, then
/// decrypt and reassemble. Returns (localPath, mime, caption), or nil while it is still locked or the
/// chunks have not all arrived — the caller simply retries on the next poll.
func receiveMedia(_ deviceSeed: String, _ sealId: String, _ bundle: String, _ shares: String, _ slot: Int64) async -> (String, String, String)? {
    let previewPath = cacheDir.appendingPathComponent("pv-\(sealId).jpg").path
    let infoStr = SealCore.openMediaInfo(deviceSeed, bundle, shares, previewPath, slot)
    guard let info = (try? JSONSerialization.jsonObject(with: Data(infoStr.utf8))) as? [String: Any],
          (info["ok"] as? Bool) == true,
          let hashes = info["chunks"] as? [String] else { return nil } // locked, or shares pending

    let chunkDir = cacheDir.appendingPathComponent("chunks/\(sealId)")
    try? FileManager.default.createDirectory(at: chunkDir, withIntermediateDirectories: true)
    for h in hashes {
        let f = chunkDir.appendingPathComponent(h)
        if FileManager.default.fileExists(atPath: f.path) { continue } // resume: never refetch
        guard let bytes = await downloadBlob(h) else { return nil }
        try? bytes.write(to: f)
    }

    let mime = info["mime_type"] as? String ?? "application/octet-stream"
    let ext = mime.hasPrefix("video") ? "mp4" : (mime.contains("png") ? "png" : "jpg")
    let out = mediaDir.appendingPathComponent("\(sealId).\(ext)")
    let doneStr = SealCore.openMediaFile(deviceSeed, bundle, shares, chunkDir.path, out.path, slot)
    guard let done = (try? JSONSerialization.jsonObject(with: Data(doneStr.utf8))) as? [String: Any],
          (done["ok"] as? Bool) == true else { return nil }
    try? FileManager.default.removeItem(at: chunkDir) // plaintext assembled; ciphertext is dead weight
    return (out.path, mime, info["caption"] as? String ?? "")
}

/// Fetch every {seal_id, bundle} item for a mailbox tag from ALL relays, merged + deduped by
/// seal_id — the recipient finds its messages on whichever relay(s) happen to be up.
func fetchInboxAll(_ tag: String) async -> [[String: Any]] {
    var byId = [String: [String: Any]]()
    var order = [String]()
    for r in relayURLs {
        guard let body = await httpGet("\(r)/inbox/\(tag)"),
              let arr = (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [[String: Any]] else { continue }
        for o in arr {
            if let id = o["seal_id"] as? String, !id.isEmpty, byId[id] == nil { byId[id] = o; order.append(id) }
        }
    }
    return order.compactMap { byId[$0] }
}
/// The chain's current finalised slot, read from a WCAHT node's /health. Sealing uses it to
/// put a chain-time floor in the leaf. 0 means "couldn't reach a node" — the seal still carries
/// its signed wall-clock window, it just has no chain floor.
func finalizedSlot() async -> Int64 {
    for h in nodeHosts {
        guard let body = await httpGet("http://\(h):8901/health"),
              let d = (try? JSONSerialization.jsonObject(with: Data(body.utf8))) as? [String: Any],
              let v = (d["finalized_slot"] as? NSNumber)?.int64Value, v > 0 else { continue }
        return v
    }
    return 0
}

/// The chain slot at which THIS seal was anchored, verified by us.
///
/// Asks the gateways for the anchor tx signature, fetches that transaction from a WCAHT node,
/// and hands both to Rust — which recomputes the leaf hash from the bundle we already hold and
/// requires the anchor to have paid exactly that address. So the answer rests on a transaction
/// the chain confirmed, not on any gateway's claim. 0 = no verifiable anchor (yet).
func verifiedAnchorSlot(_ sealId: String, _ bundle: String) async -> Int64 {
    for gw in gatewayURLs {
        guard let meta = await httpGet("\(gw)/anchor/\(sealId)"),
              let d = (try? JSONSerialization.jsonObject(with: Data(meta.utf8))) as? [String: Any],
              let sig = d["signature"] as? String, !sig.isEmpty else { continue }
        for h in nodeHosts {
            guard let txJson = await httpGet("http://\(h):8901/transaction/\(sig)") else { continue }
            let r = SealCore.verifyAnchor(bundle, txJson)
            if let v = (try? JSONSerialization.jsonObject(with: Data(r.utf8))) as? [String: Any] {
                if (v["ok"] as? Bool) == true {
                    return (v["anchor_slot"] as? NSNumber)?.int64Value ?? 0
                }
                // An anchor that commits a DIFFERENT leaf is not a missing anchor — it means the
                // bundle we hold is not the one that was committed. Refuse it outright.
                if (v["reason"] as? String) == "anchor commits a different leaf" { return -1 }
            }
        }
    }
    return 0
}

/// Hand this message's signed leaf to the batchers so it gets committed under a SEAL_ROOT.
/// Fire-and-forget: a batcher being down must never stop a message being delivered.
func submitLeafForBatching(_ bundle: [String: Any]) async {
    guard let leaf = bundle["signed_leaf"] else { return }
    let payload: [String: Any] = ["signed_leaf": leaf, "sender_id_pub": bundle["sender_id_pub"] as? String ?? ""]
    guard let d = try? JSONSerialization.data(withJSONObject: payload),
          let s = String(data: d, encoding: .utf8) else { return }
    for b in batcherURLs { _ = await httpPost("\(b)/leaf", s) }
}

/// The batched proof for this seal, verified against our OWN leaf. nil while no batcher has
/// anchored it yet — a 425 pending is normal, not an error.
func fetchVerifiedProof(_ sealId: String, _ bundle: String) async -> Int64? {
    for b in batcherURLs {
        guard let body = await httpGet("\(b)/proof/\(sealId)") else { continue }
        let r = SealCore.verifySealProof(bundle, body)
        if let v = (try? JSONSerialization.jsonObject(with: Data(r.utf8))) as? [String: Any],
           (v["ok"] as? Bool) == true {
            return (v["finalized_slot"] as? NSNumber)?.int64Value
        }
    }
    return nil
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
    @State private var tab = TAB_CHATS
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
            Store.clearThread(c.identityPub)
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
                    if Store.addInbox(tag, sealId, plain, sender) {
                        // Carry the destroy deadline, or a self-destructing text received while
                        // the chat is closed is persisted with destroyAt = 0 and never burns.
                        Store.addThreadMsg(sender, sealId, plain, true,
                                           destroyAt: (bmap?["destroy_at"] as? NSNumber)?.int64Value ?? 0)
                        notifyMessage(senderName, plain)
                    }
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
            } else if tab == TAB_SETTINGS {
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
    @State private var searching = false
    @State private var query = ""
    var body: some View {
        let contactRows = contacts.map { c -> Chat in
            let sub = !c.address.isEmpty ? String(c.address.prefix(14)) + "… · tap to seal"
                : (!c.phone.isEmpty ? "+855 " + c.phone : "tap to seal")
            return Chat(name: c.name, last: sub, time: "", devicePub: c.devicePub, identityPub: c.identityPub, isContact: true)
        }
        // The Contacts tab lists only people you've actually saved; Chats/Calls also show the threads.
        let all = tab == TAB_CONTACTS ? contactRows : contactRows + CHATS.filter { !hidden.contains($0.name) }
        let rows = query.isEmpty ? all : all.filter { $0.name.localizedCaseInsensitiveContains(query) }
        let title = tab == TAB_CONTACTS ? "Contacts" : (tab == TAB_CALLS ? "Calls" : "Denvion")
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "shield.fill").foregroundColor(.white).font(.system(size: 20))
                Text(title).font(.system(size: 21, weight: .bold)).foregroundColor(.white)
                Spacer()
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
            .frame(maxWidth: .infinity)
            .background(Color.dvBlue.ignoresSafeArea(edges: .top))

            if searching {
                HStack(spacing: 10) {
                    HStack(spacing: 8) {
                        Image(systemName: "magnifyingglass").foregroundColor(.dvSub).font(.system(size: 15))
                        TextField("Search", text: $query)
                            .font(.system(size: 15)).foregroundColor(.dvInk)
                            .autocorrectionDisabled()
                    }
                    .padding(.horizontal, 14).padding(.vertical, 10)
                    .background(Color.dvHair).clipShape(Capsule())
                    Text("Cancel").font(.system(size: 15)).foregroundColor(.dvBlue)
                        .onTapGesture { searching = false; query = "" }
                }
                .padding(.horizontal, 16).padding(.vertical, 10)
            }

            ZStack(alignment: .bottom) {
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
                            // clear the floating bar + its shadow so the last row stays tappable
                            Color.clear.frame(height: 96)
                        }
                    }
                    Button(action: onAdd) {
                        Image(systemName: "person.badge.plus").foregroundColor(.white).font(.system(size: 22))
                            .frame(width: 56, height: 56).background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 16))
                            .shadow(color: .dvBlue.opacity(0.4), radius: 6, y: 3)
                    }.padding(.trailing, 20).padding(.bottom, 104)
                }

                BottomBar(
                    tab: tab, onTab: onTab,
                    chatBadge: all.reduce(0) { $0 + $1.unread },
                    onSearch: { searching.toggle(); if !searching { query = "" } }
                )
            }
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

/// Floating bottom bar: a frosted-glass capsule holding the four tabs, plus a detached
/// round search button. `.ultraThinMaterial` blurs whatever scrolls underneath. It overlays
/// the content (the lists pad their bottom for it) instead of docking to the window edge.
///
/// Shadows stay faint here on purpose: the surface is translucent, so a normal-strength
/// shadow shows through the glass and greys the whole bar out.
private struct BottomBar: View {
    let tab: Int
    let onTab: (Int) -> Void
    var chatBadge: Int = 0
    var settingsAlert: Bool = false
    var onSearch: () -> Void = {}
    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 0) {
                NavItem(icon: "person.fill", label: "Contacts", active: tab == TAB_CONTACTS) { onTab(TAB_CONTACTS) }
                NavItem(icon: "phone.fill", label: "Calls", active: tab == TAB_CALLS) { onTab(TAB_CALLS) }
                NavItem(icon: "bubble.left.fill", label: "Chats", active: tab == TAB_CHATS, badge: chatBadge) { onTab(TAB_CHATS) }
                NavItem(icon: "gearshape.fill", label: "Settings", active: tab == TAB_SETTINGS, dot: settingsAlert) { onTab(TAB_SETTINGS) }
            }
            .padding(6)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(Capsule().strokeBorder(Color.dvBarEdge, lineWidth: 1))
            .shadow(color: .black.opacity(0.10), radius: 10, y: 4)

            Image(systemName: "magnifyingglass")
                .font(.system(size: 22, weight: .medium))
                .foregroundColor(.dvInk)
                .frame(width: 58, height: 58)
                .background(.ultraThinMaterial, in: Circle())
                .overlay(Circle().strokeBorder(Color.dvBarEdge, lineWidth: 1))
                .shadow(color: .black.opacity(0.10), radius: 10, y: 4)
                .contentShape(Circle())
                .onTapGesture(perform: onSearch)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 6)
    }
}

private struct NavItem: View {
    let icon: String; let label: String; let active: Bool
    var badge: Int = 0
    var dot: Bool = false
    let onTap: () -> Void
    var body: some View {
        VStack(spacing: 3) {
            Image(systemName: icon).font(.system(size: 21))
                .overlay(alignment: .topTrailing) {
                    if badge > 0 || dot {
                        Text(dot && badge == 0 ? "!" : "\(badge)")
                            .font(.system(size: 10, weight: .bold)).foregroundColor(.white)
                            .padding(.horizontal, 4).frame(minWidth: 16, minHeight: 16)
                            .background(Color.dvBadge).clipShape(Capsule())
                            .offset(x: 11, y: -8)
                    }
                }
            Text(label).font(.system(size: 11, weight: active ? .semibold : .medium))
        }
        .foregroundColor(active ? .dvBlue : .dvInk)
        .padding(.horizontal, 14).padding(.vertical, 7)
        .background(active ? Color.dvBarActive : Color.clear)
        .clipShape(Capsule())
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

            ZStack(alignment: .bottom) {
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
                        Color.clear.frame(height: 96) // clear the floating bar
                    }
                    .padding(20)
                }

                BottomBar(tab: tab, onTab: onTab)
            }
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
    @State private var codeInput = ""

    private var codeReady: Bool { !codeInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

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

                    // Add by the denvion: contact code. The field is EDITABLE so a code can be
                    // typed, or pasted with the keyboard's own long-press ▸ Paste.
                    //
                    // The clipboard button is SwiftUI's `PasteButton`, not a plain button reading
                    // UIPasteboard: since iOS 16 a programmatic `UIPasteboard.general.string` read
                    // pops an "Allow Paste?" prompt and returns nil unless the user confirms it,
                    // which surfaced here as "Not a contact code" for a perfectly good code.
                    // PasteButton is granted access without any prompt.
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Or add by contact code").font(.system(size: 13)).foregroundColor(.dvSub)
                        Text("On the other phone: Settings ▸ Copy my code. Paste or type it here.")
                            .font(.system(size: 11)).foregroundColor(.dvSub)

                        TextField("denvion:…", text: $codeInput, axis: .vertical)
                            .lineLimit(1...3)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(.dvInk)
                            .autocorrectionDisabled()
                            .textInputAutocapitalization(.never)
                            .padding(12)
                            .background(Color(hex: 0xF1F4F7))
                            .clipShape(RoundedRectangle(cornerRadius: 10))

                        HStack(spacing: 10) {
                            PasteButton(payloadType: String.self) { items in
                                if let s = items.first { codeInput = s }
                            }
                            .labelStyle(.titleAndIcon)
                            .tint(.dvBlue)

                            Button(action: { onPasteCode(codeInput) }) {
                                Text("Use this code")
                                    .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                                    .frame(maxWidth: .infinity).padding(.vertical, 12)
                                    .background(codeReady ? Color.dvBlue : Color(hex: 0xCBD3DA))
                                    .clipShape(RoundedRectangle(cornerRadius: 10))
                            }.disabled(!codeReady)
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
            c.phoneNumbers = [CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(stringValue: phone))]
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
    /// How many times each seal has been tried and failed to open. Only a SUCCESSFUL open used to
    /// mark a seal `seen`, so a seal that can never open (sealed to a device key we no longer
    /// hold) was retried on every poll forever at ~15 HTTP calls each — starving the loop so newer
    /// messages were never reached. The inbox looked frozen while the app was in fact busy.
    @State private var openFailures: [String: Int] = [:]
    @State private var polling = false
    @State private var myTag = ""
    // one-shot timelock for the NEXT message (unix secs, 0 = none); set via the clock in the composer.
    @State private var revealAt: Int64 = 0
    @State private var destroyAt: Int64 = 0
    @State private var showTimer = false
    @State private var pickedItem: PhotosPickerItem?
    @State private var mediaError: String?
    @State private var playing: URL?
    /// A picked photo/video waiting in the composer. It is NOT sent until the user taps send,
    /// so a caption can be typed alongside it.
    @State private var pending: (url: URL, mime: String)?
    /// Wall-clock seconds, ticking once a second: drives reveal countdowns and destroy deadlines.
    @State private var nowSecs: Int64 = Int64(Date().timeIntervalSince1970)
    private let tick = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

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
                        ForEach(messages) { m in
                            Bubble(m, now: nowSecs) {
                                // burn finished — drop it from the transcript for good
                                messages.removeAll { $0.id == m.id }
                                if real { Store.removeThreadMedia(chat.identityPub, m.mediaPath) }
                                if !m.mediaPath.isEmpty {
                                    try? FileManager.default.removeItem(atPath: m.mediaPath)
                                }
                            }
                            .id(m.id)
                        }
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
        // Load the picked item's raw bytes and seal them. Videos come back as a movie
        // transferable; images as Data.
        .onReceive(tick) { _ in nowSecs = Int64(Date().timeIntervalSince1970) }
        .onChange(of: pickedItem) { _, item in
            guard let item else { return }
            Task {
                let isVideo = item.supportedContentTypes.contains { $0.conforms(to: .movie) }
                var src: URL?
                if isVideo {
                    // as a FILE: a long clip loaded as Data would sit in memory in full
                    src = (try? await item.loadTransferable(type: PickedMovie.self))?.url
                } else if let data = try? await item.loadTransferable(type: Data.self) {
                    let f = cacheDir.appendingPathComponent("pick-\(UUID().uuidString).jpg")
                    try? data.write(to: f)
                    src = f
                }
                await MainActor.run {
                    if let src { pending = (src, isVideo ? "video/mp4" : "image/jpeg") }
                    else { mediaError = "Couldn't read that item" }
                    pickedItem = nil
                }
            }
        }
        .alert("Media", isPresented: Binding(get: { mediaError != nil }, set: { if !$0 { mediaError = nil } })) {
            Button("OK", role: .cancel) { mediaError = nil }
        } message: { Text(mediaError ?? "") }
        // Play a DECRYPTED video from local storage — nothing here touches the network.
        .fullScreenCover(item: Binding(get: { playing.map { PlayerItem(url: $0) } }, set: { if $0 == nil { playing = nil } })) { item in
            VideoPlayer(player: AVPlayer(url: item.url))
                .ignoresSafeArea()
                .overlay(alignment: .topTrailing) {
                    Button(action: { playing = nil }) {
                        Image(systemName: "xmark.circle.fill").font(.system(size: 30)).foregroundColor(.white.opacity(0.9))
                    }.padding()
                }
        }
        .environment(\.playVideo, { url in playing = url })
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
                    // seed `seen` with every opened-seal id so poll skips them...
                    for o in Store.inbox(myTag) { if let id = o["id"] as? String { seen.insert(id) } }
                    // ...then restore the durable transcript for THIS peer (both directions, in order).
                    for o in Store.thread(chat.identityPub) {
                        let media = o["media"] as? String ?? ""
                        messages.append(Msg(
                            text: o["text"] as? String ?? "",
                            incoming: (o["incoming"] as? Bool) ?? false, time: "",
                            kind: media.isEmpty ? .text : .image, state: .opened,
                            destroyAt: (o["destroy_at"] as? NSNumber)?.int64Value ?? 0,
                            mediaPath: media, mediaMime: o["mime"] as? String ?? ""
                        ))
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
        .sheet(isPresented: $showTimer) {
            TimedSealSheet(onPick: { r, d in revealAt = r; destroyAt = d; showTimer = false },
                           onCancel: { showTimer = false })
        }
    }

    private var composer: some View {
        VStack(spacing: 0) {
            // A staged photo/video slides up into the composer and waits there. Whatever is
            // typed becomes its caption, sealed inside the manifest, and both go in ONE item.
            if let p = pending {
                HStack(spacing: 12) {
                    ZStack(alignment: .topTrailing) {
                        Group {
                            if let img = MediaThumbs.thumb(path: p.url.path, isVideo: p.mime.hasPrefix("video")) {
                                Image(uiImage: img).resizable().aspectRatio(contentMode: .fill)
                            } else {
                                Color.dvHair
                            }
                        }
                        .frame(width: 64, height: 64).clipped()
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                        Button(action: { withAnimation(.easeInOut(duration: 0.2)) { pending = nil } }) {
                            Image(systemName: "xmark").font(.system(size: 10, weight: .bold))
                                .foregroundColor(.white).frame(width: 22, height: 22)
                                .background(Color.dvInk.opacity(0.75)).clipShape(Circle())
                        }
                        .offset(x: 7, y: -7)
                    }
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Ready to seal").font(.system(size: 13, weight: .semibold)).foregroundColor(.dvInk)
                        Text("Add a caption, then send").font(.system(size: 11)).foregroundColor(.dvSub)
                    }
                    Spacer()
                }
                .padding(.horizontal, 14).padding(.top, 10)
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if revealAt > 0 || destroyAt > 0 {
                HStack(spacing: 6) {
                    Text(revealAt > 0 ? "🔒 Opens in \(relLabel(revealAt))" : "💥 Destroys in \(relLabel(destroyAt))")
                        .font(.system(size: 12, weight: .semibold)).foregroundColor(.dvBlue)
                        .padding(.horizontal, 10).padding(.vertical, 5)
                        .background(Color(hex: 0xEAF3FF)).clipShape(RoundedRectangle(cornerRadius: 12))
                    Button(action: { revealAt = 0; destroyAt = 0 }) {
                        Image(systemName: "xmark").font(.system(size: 12)).foregroundColor(.dvSub)
                    }
                    Spacer()
                }.padding(.horizontal, 14).padding(.top, 8)
            }
            HStack(spacing: 8) {
                Button(action: { showTimer = true }) {
                    Image(systemName: "clock").font(.system(size: 21))
                        .foregroundColor(revealAt > 0 || destroyAt > 0 ? .dvBlue : .dvSub)
                }
                // PhotosPicker needs no photo-library permission for the items the user picks.
                PhotosPicker(selection: $pickedItem, matching: .any(of: [.images, .videos])) {
                    Image(systemName: "camera.fill").font(.system(size: 20)).foregroundColor(.dvSub)
                }
                HStack(spacing: 8) {
                    Image(systemName: "face.smiling").foregroundColor(.dvSub).font(.system(size: 20))
                    TextField(pending == nil ? "Message" : "Add a caption…", text: $draft, axis: .vertical)
                        .lineLimit(1...4).font(.system(size: 15)).foregroundColor(.dvInk)
                }
                .padding(.horizontal, 12).padding(.vertical, 10)
                .background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 22))

                let active = !draft.trimmingCharacters(in: .whitespaces).isEmpty || pending != nil
                Button(action: send) {
                    Image(systemName: active ? "paperplane.fill" : "mic.fill").foregroundColor(.white).font(.system(size: 19))
                        .frame(width: 46, height: 46).background(Color.dvBlue).clipShape(Circle())
                }.disabled(!active)
            }
            .padding(.horizontal, 10).padding(.vertical, 8)
        }
        .background(Color.white)
        .animation(.easeInOut(duration: 0.22), value: pending?.url)
    }

    // Poll my inbox: fetch delivered ciphertext, collect released shares, open, show. One at a time.
    @MainActor private func poll() async {
        guard real, !myTag.isEmpty, !polling else { return }
        polling = true
        defer { polling = false }
        let inbox = await fetchInboxAll(myTag)
        // (The chain slot used to be read once per pass and handed to the open calls. It is no
        // longer passed — a slot behind the seal's finality floor made the open fail outright,
        // see the note below — so the read is dropped rather than left as dead work.)
        for item in inbox {
            guard let sealId = item["seal_id"] as? String, !sealId.isEmpty, !seen.contains(sealId),
                  let bundle = item["bundle"],
                  let bd = try? JSONSerialization.data(withJSONObject: bundle),
                  let bundleStr = String(data: bd, encoding: .utf8) else { continue }
            let sender = (item["bundle"] as? [String: Any])?["sender_id_pub"] as? String ?? ""
            let shares = await collectShares(sealId)
            // If an anchor exists and contradicts this bundle, the bundle is not what was
            // committed on-chain — drop it rather than opening it.
            if await verifiedAnchorSlot(sealId, bundleStr) < 0 { seen.insert(sealId); continue }
            // No slot is passed to the open calls any more (see the note on the text open below),
            // so the batched-proof fetch that used to feed it is dropped rather than left
            // dangling: it was 3 HTTP calls per message per poll for a value nothing reads, on
            // the loop that most needs to stay fast.

            // Media arrives as a manifest, not as bytes: open the manifest, pull the chunks
            // the relay is holding, then decrypt and reassemble locally.
            let leaf = ((item["bundle"] as? [String: Any])?["signed_leaf"] as? [String: Any])?["leaf"] as? [String: Any]
            if (leaf?["content_type"] as? String) == "Media" {
                // Time-locked? Show a locked placeholder with a countdown rather than nothing at
                // all, and keep polling — it opens by itself at revealAt.
                // no currentSlot — same reason as the text path below: a lagging chain slot makes
                // this refuse a photo the chat-list poll would open fine.
                let gateStr = SealCore.openMediaInfo(myDeviceSeed, bundleStr, shares, "")
                if let gate = (try? JSONSerialization.jsonObject(with: Data(gateStr.utf8))) as? [String: Any],
                   (gate["ok"] as? Bool) != true {
                    let reason = gate["reason"] as? String ?? ""
                    if reason == "destroyed" { seen.insert(sealId); continue } // gone before ever opened
                    if reason == "locked" {
                        let revealAt = (gate["reveal_at"] as? NSNumber)?.int64Value ?? 0
                        if sender == chat.identityPub, !messages.contains(where: { $0.lockedSealId == sealId }) {
                            messages.append(Msg(text: "Photo", incoming: true, time: nowTime(), kind: .image,
                                                state: .locked, revealAt: revealAt, lockedSealId: sealId))
                        }
                        continue // NOT marked seen: the next poll opens it once the window does
                    }
                }
                if let got = await receiveMedia(myDeviceSeed, sealId, bundleStr, shares, 0) {
                    seen.insert(sealId)
                    let label = got.2.isEmpty ? (got.1.hasPrefix("video") ? "Video" : "Photo") : got.2
                    // carry the destroy deadline across: the recipient's copy must burn too,
                    // otherwise "self-destruct" only ever removed the sender's side.
                    let dz = ((item["bundle"] as? [String: Any])?["destroy_at"] as? NSNumber)?.int64Value ?? 0
                    // Was this photo sitting behind a countdown a moment ago? Has to be read
                    // BEFORE the placeholder is dropped, and it is what earns the reveal halo.
                    let cameOutOfLock = messages.contains { $0.lockedSealId == sealId }
                    messages.removeAll { $0.lockedSealId == sealId } // replace the locked placeholder
                    // persist once …
                    if Store.addInbox(myTag, sealId, label, sender) {
                        Store.addThreadMsg(sender, sealId, label, true, media: got.0, mime: got.1, destroyAt: dz)
                    }
                    // … and append independently of that dedup, or a photo the chat-list poll
                    // persisted first never shows up live.
                    if sender == chat.identityPub, !messages.contains(where: { $0.sealId == sealId }) {
                        messages.append(Msg(text: label, incoming: true, time: nowTime(),
                                            kind: .image, state: .opened, destroyAt: dz,
                                            mediaPath: got.0, mediaMime: got.1, sealId: sealId,
                                            justRevealed: cameOutOfLock))
                    }
                } else {
                    // Media that did not open. A timelocked item returned "locked" above and
                    // already continued, so reaching here means the open genuinely failed.
                    // Count only permanent reasons — a photo still gathering chunks must retry.
                    let why = ((try? JSONSerialization.jsonObject(with: Data(gateStr.utf8))) as? [String: Any])?["reason"] as? String ?? "no result"
                    if isPermanentOpenFailure(why) {
                        let n = (openFailures[sealId] ?? 0) + 1
                        openFailures[sealId] = n
                        if n >= MAX_OPEN_ATTEMPTS { seen.insert(sealId) }
                    }
                }
                continue // still gathering chunks → retry on the next poll
            }

            // NOTE: no currentSlot, deliberately — matching the chat-list poll (~line 736).
            // Passing a slot makes the core enforce the seal's finality floor against THAT slot.
            // Chain finality routinely runs a minute or more behind wall clock, so the slot we
            // just read is older than the floor and the open is refused — while the chat-list
            // poll, which passes none, opens the same message fine. That poll is paused while a
            // chat is open, so the message only appeared after leaving the chat and returning.
            // The gateways withholding shares are the primary release gate (the FFI calls this
            // check "defense-in-depth"), so the two paths agreeing matters more than one being
            // selectively stricter. Found and fixed on Android first — see SKYREACH_DOC §9b.
            let openStr = SealCore.openReceived(myDeviceSeed, bundleStr, shares)
            if let r = (try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any],
               (r["ok"] as? Bool) == true, let plain = r["plaintext"] as? String {
                seen.insert(sealId)
                // Carry the destroy deadline across, exactly as the media path above does.
                // Without it the recipient's copy of a self-destructing TEXT lands with
                // destroyAt = 0 and never burns — the countdown only ever ran on the sender.
                let dz = ((item["bundle"] as? [String: Any])?["destroy_at"] as? NSNumber)?.int64Value ?? 0
                // Persist once (addInbox is a one-shot dedup) …
                if Store.addInbox(myTag, sealId, plain, sender) {
                    Store.addThreadMsg(sender, sealId, plain, true, destroyAt: dz)
                }
                // … but decide the on-screen append SEPARATELY. The chat-list poll and this one
                // both run, and if the list persists the message first it eats the dedup — so
                // gating the UI on addInbox() meant the bubble never appeared until the user
                // left the chat and came back.
                // a timelocked text put a sealed placeholder on screen — swap it for the real
                // message now the window has opened, rather than leaving both.
                messages.removeAll { $0.lockedSealId == sealId }
                if sender == chat.identityPub, !messages.contains(where: { $0.sealId == sealId }) {
                    messages.append(Msg(text: plain, incoming: true, time: nowTime(), state: .opened,
                                        destroyAt: dz, sealId: sealId))
                }
            } else if let r = (try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any],
                      (r["reason"] as? String) == "destroyed" {
                seen.insert(sealId) // self-destructed before it was opened → stop retrying, never shows
            } else if let r = (try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any],
                      (r["reason"] as? String) == "locked" {
                // Timelocked text: show the same sealed card a locked photo gets, so the
                // recipient sees SOMETHING is waiting and when it opens. Previously a
                // timed-reveal text rendered nothing at all until the window passed, so it
                // looked like the message had never arrived.
                // NOT marked seen — the next poll opens it once the window does.
                let rv = (r["reveal_at"] as? NSNumber)?.int64Value ?? 0
                if sender == chat.identityPub, !messages.contains(where: { $0.lockedSealId == sealId }) {
                    messages.append(Msg(text: "Message", incoming: true, time: nowTime(),
                                        state: .locked, revealAt: rv, lockedSealId: sealId))
                }
            } else {
                // Not opened. Count only failures that can NEVER clear; a seal waiting on the
                // timelock, on shares or on finality must keep retrying untouched.
                let why = ((try? JSONSerialization.jsonObject(with: Data(openStr.utf8))) as? [String: Any])?["reason"] as? String ?? "no result"
                if isPermanentOpenFailure(why) {
                    let n = (openFailures[sealId] ?? 0) + 1
                    openFailures[sealId] = n
                    if n >= MAX_OPEN_ATTEMPTS { seen.insert(sealId) }
                }
            }
        }
    }

    /// Seal and ship a picked photo/video. The readable file is written to our cache only so
    /// Rust can chunk-encrypt it; ONLY the encrypted chunks are uploaded, addressed by
    /// ciphertext hash. Our own copy stays local so the sent bubble can render it.
    private func sendMedia(_ src: URL, _ mime: String, _ caption: String) {
        guard real else { return }
        let isVideo = mime.hasPrefix("video")
        let label = caption.isEmpty ? (isVideo ? "Video" : "Photo") : caption
        let fast = fastMode
        let rv = revealAt, dz = destroyAt
        messages.append(Msg(text: label, incoming: false, time: nowTime(), kind: .image,
                            state: .sealing, mode: fast ? "FAST" : "STRICT",
                            revealAt: rv, destroyAt: dz))
        let idx = messages.count - 1
        revealAt = 0; destroyAt = 0
        Task {
            let preview = buildPreview(src, isVideo: isVideo)
            let chunkDir = cacheDir.appendingPathComponent("out-\(UUID().uuidString)")

            let sealedStr = SealCore.sealMediaFile(
                myIdentitySeed, myCard, chat.devicePub, src.path, mime,
                isVideo ? "video" : "image", caption, preview?.path ?? "", chunkDir.path, fast, rv, dz,
                rv > 0 ? await finalizedSlot() : 0
            )
            if let p = preview { try? FileManager.default.removeItem(at: p) }
            guard let sealed = (try? JSONSerialization.jsonObject(with: Data(sealedStr.utf8))) as? [String: Any],
                  (sealed["ok"] as? Bool) == true else {
                try? FileManager.default.removeItem(at: chunkDir)
                try? FileManager.default.removeItem(at: src)
                await MainActor.run { failMedia(idx, label) }
                return
            }
            // upload the opaque chunks, then the manifest bundle + key shares
            let up = await uploadChunks(sealed)
            try? FileManager.default.removeItem(at: chunkDir)
            let shipped = up ? await shipSeal(sealed) : false
            guard shipped else {
                try? FileManager.default.removeItem(at: src)
                await MainActor.run { failMedia(idx, label) }
                return
            }
            // keep OUR readable copy locally so the sent bubble can render it
            let mine = mediaDir.appendingPathComponent("sent-\(sealed["seal_id"] as? String ?? UUID().uuidString).\(isVideo ? "mp4" : "jpg")")
            try? FileManager.default.removeItem(at: mine)
            try? FileManager.default.moveItem(at: src, to: mine)
            await MainActor.run {
                guard idx < messages.count else { return }
                messages[idx].state = .opened
                messages[idx].sealedFor = chat.name
                messages[idx].mediaPath = mine.path
                messages[idx].mediaMime = mime
                Store.addThreadMsg(chat.identityPub, "out-\(UUID().uuidString)", label, false, media: mine.path, mime: mime, destroyAt: dz)
            }
        }
    }

    @MainActor private func failMedia(_ idx: Int, _ label: String) {
        if idx < messages.count { messages.remove(at: idx) }
        mediaError = "Couldn't send \(label) — check the server"
    }

    private func send() {
        // An attached photo/video takes the typed text as its (sealed) caption, so the two
        // travel as ONE item rather than a picture followed by a stray message.
        if let p = pending {
            let caption = draft.trimmingCharacters(in: .whitespacesAndNewlines)
            draft = ""
            pending = nil
            sendMedia(p.url, p.mime, caption)
            return
        }
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let fast = fastMode
        let rv = revealAt, dz = destroyAt // capture the one-shot timelock for THIS message
        messages.append(Msg(text: text, incoming: false, time: nowTime(), state: .sealing, mode: fast ? "FAST" : "STRICT", revealAt: rv, destroyAt: dz))
        // persist the outgoing message immediately so it survives leaving/reopening the chat.
        if real { Store.addThreadMsg(chat.identityPub, "out-\(UUID().uuidString)", text, false) }
        let idx = messages.count - 1
        draft = ""; revealAt = 0; destroyAt = 0 // reset the timelock after each send
        Task { @MainActor in
            if real {
                // seal + SHIP over the relay + gateways; the recipient's device polls + opens.
                // `ok` now means the RELAY actually accepted the ciphertext (real delivery signal).
                var ok = false
                let slot: Int64 = rv > 0 ? await finalizedSlot() : 0 // only needed for a reveal floor
                let shipStr = SealCore.sealShippable(myIdentitySeed, myCard, chat.devicePub, text, fast, rv, dz, slot)
                if let ship = (try? JSONSerialization.jsonObject(with: Data(shipStr.utf8))) as? [String: Any], (ship["ok"] as? Bool) == true {
                    ok = await shipSeal(ship)
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

// Timed-seal picker: choose timelock (opens later) or self-destruct + a preset duration.
struct TimedSealSheet: View {
    let onPick: (Int64, Int64) -> Void   // (revealAt, destroyAt) unix secs
    let onCancel: () -> Void
    @State private var mode = 0           // 0 = opens later, 1 = self-destruct
    @State private var exact = false      // false = quick presets, true = calendar + clock
    @State private var when = Date().addingTimeInterval(3600)
    private let presets: [(String, Int64)] = [
        ("1 min", 60), ("10 min", 600), ("1 hour", 3600),
        ("1 day", 86400), ("3 days", 259200), ("1 week", 604800),
    ]

    private func commit(_ epoch: Int64) {
        guard epoch > Int64(Date().timeIntervalSince1970) else { return } // never a past instant
        mode == 0 ? onPick(epoch, 0) : onPick(0, epoch)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text(exact ? "Pick date & time" : "Timed seal")
                    .font(.system(size: 18, weight: .bold)).foregroundColor(.dvInk)
                Spacer()
                Button(exact ? "Back" : "Cancel") {
                    if exact { withAnimation(.easeInOut(duration: 0.22)) { exact = false } } else { onCancel() }
                }.foregroundColor(.dvSub)
            }
            Text("The window is signed into the seal and anchored on-chain, and the gateways withhold the key outside it — so it can't be moved, and a patched app has nothing to open.")
                .font(.system(size: 12)).foregroundColor(.dvSub)
            Picker("", selection: $mode) {
                Text("🔒 Opens later").tag(0)
                Text("💥 Self-destruct").tag(1)
            }.pickerStyle(.segmented)

            if exact {
                // graphical style gives a real calendar + clock in one control
                DatePicker(
                    "",
                    selection: $when,
                    in: Date().addingTimeInterval(60)...,   // future only
                    displayedComponents: [.date, .hourAndMinute]
                )
                .datePickerStyle(.graphical)
                .labelsHidden()
                .tint(.dvBlue)

                Button(action: { commit(Int64(when.timeIntervalSince1970)) }) {
                    Text((mode == 0 ? "Opens " : "Destroys ") + when.formatted(date: .abbreviated, time: .shortened))
                        .font(.system(size: 15, weight: .semibold)).foregroundColor(.white)
                        .frame(maxWidth: .infinity).padding(.vertical, 14)
                        .background(Color.dvBlue).clipShape(RoundedRectangle(cornerRadius: 10))
                }
            } else {
                Text(mode == 0 ? "Opens after" : "Destroys after")
                    .font(.system(size: 13, weight: .semibold)).foregroundColor(.dvInk)
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 10) {
                    ForEach(presets, id: \.0) { (label, secs) in
                        Button(action: { commit(Int64(Date().timeIntervalSince1970) + secs) }) {
                            Text(label).font(.system(size: 15)).foregroundColor(.dvInk)
                                .frame(maxWidth: .infinity).padding(.vertical, 14)
                                .background(Color(hex: 0xF1F4F7)).clipShape(RoundedRectangle(cornerRadius: 10))
                        }
                    }
                }
                Button(action: { withAnimation(.easeInOut(duration: 0.22)) { exact = true } }) {
                    HStack {
                        Image(systemName: "calendar")
                        Text("Pick exact date & time").font(.system(size: 15, weight: .semibold))
                    }
                    .foregroundColor(.dvBlue)
                    .frame(maxWidth: .infinity).padding(.vertical, 13)
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(Color.dvBlue, lineWidth: 1))
                }
            }
            Spacer(minLength: 0)
        }
        .padding(20)
        .animation(.easeInOut(duration: 0.22), value: exact)
        .presentationDetents([.height(exact ? 620 : 420)])
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
/// Wraps a local file URL so it can drive a `fullScreenCover(item:)`.
struct PlayerItem: Identifiable {
    let url: URL
    var id: String { url.path }
}

/// Lets a deeply-nested bubble ask the conversation view to play a decrypted video,
/// without threading a binding through every intermediate view.
private struct PlayVideoKey: EnvironmentKey {
    static let defaultValue: (URL) -> Void = { _ in }
}
extension EnvironmentValues {
    var playVideo: (URL) -> Void {
        get { self[PlayVideoKey.self] }
        set { self[PlayVideoKey.self] = newValue }
    }
}

/// mm:ss for a countdown; falls back to h/d for long windows.
func countdownLabel(_ secondsLeft: Int64) -> String {
    let s = max(0, secondsLeft)
    if s >= 86400 { return "\(s / 86400)d \((s % 86400) / 3600)h" }
    if s >= 3600 { return "\(s / 3600)h \((s % 3600) / 60)m" }
    return String(format: "%d:%02d", s / 60, s % 60)
}

/// Burns the content away: it shrinks and fades while puffs of smoke rise off it, then
/// `onFinished` fires so the caller can drop the message for good.
private struct SmokeDestroy<Content: View>: View {
    let onFinished: () -> Void
    @ViewBuilder var content: Content
    @State private var p: Double = 0

    var body: some View {
        ZStack {
            content
                .opacity(max(0, 1 - p * 1.35))
                .scaleEffect(1 - 0.14 * p)
            Canvas { ctx, size in
                for i in 0..<14 {
                    let seed = i * 7919
                    let fx = Double(seed % 100) / 100.0
                    let drift = Double((seed / 100) % 100) / 100.0 - 0.5
                    let delay = Double(i % 5) * 0.08
                    let local = max(0, min(1, (p - delay) / (1 - delay)))
                    if local <= 0 { continue }
                    let r = min(size.width, size.height) * (0.06 + 0.16 * local)
                    let cx = size.width * fx + drift * size.width * 0.35 * local
                    let cy = size.height * (0.85 - 1.05 * local)
                    let rect = CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2)
                    ctx.fill(Path(ellipseIn: rect),
                             with: .color(Color(hex: 0x9AA5B1).opacity(max(0, 0.42 * (1 - local)))))
                }
            }
            .allowsHitTesting(false)
        }
        .onAppear {
            withAnimation(.linear(duration: 1.5)) { p = 1 }
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.55) { onFinished() }
        }
    }
}

/// How long the reveal halo runs before fading out.
private let revealGlowSeconds: Double = 9

/// A band of colour that travels behind a photo for a few seconds after its timelock opens.
///
/// A gradient three times the picture's width is slid sideways and clipped back to its footprint,
/// then blurred — so what shows is a moving coloured shadow rather than a hard edge. The colours
/// repeat across the run, which is what makes the travel read as continuous instead of looping.
private struct RevealHalo: View {
    let width: CGFloat
    let height: CGFloat
    @State private var travel: CGFloat = -1
    @State private var lit = true

    var body: some View {
        LinearGradient(
            colors: [Color(hex: 0x2E9BF6), Color(hex: 0x7B61FF), Color(hex: 0xFF6FB5),
                     Color(hex: 0xFFC46B), Color(hex: 0x2E9BF6), Color(hex: 0x7B61FF),
                     Color(hex: 0xFF6FB5)],
            startPoint: .leading, endPoint: .trailing
        )
        .frame(width: width * 3, height: height)
        .offset(x: travel * width)
        .frame(width: width, height: height)
        .clipShape(RoundedRectangle(cornerRadius: 18))
        .blur(radius: 14)
        .opacity(lit ? 0.85 : 0)
        .onAppear {
            withAnimation(.linear(duration: 2.6).repeatForever(autoreverses: false)) { travel = 0 }
            withAnimation(.easeOut(duration: 1.4).delay(revealGlowSeconds)) { lit = false }
        }
    }
}

private struct Bubble: View {
    let m: Msg
    var now: Int64 = 0
    var onDestroyed: () -> Void = {}
    @Environment(\.playVideo) private var playVideo
    init(_ m: Msg, now: Int64 = 0, onDestroyed: @escaping () -> Void = {}) {
        self.m = m; self.now = now; self.onDestroyed = onDestroyed
    }
    var body: some View {
        // Self-destruct: the countdown is the SENDER's to watch, but the burn plays on both sides.
        let destroying = m.destroyAt > 0 && now > 0 && now >= m.destroyAt
        let showDestroyClock = !m.incoming && m.destroyAt > 0 && now > 0 && !destroying
        // The SENDER's own copy of an "opens later" item: they own the picture, but it must not
        // look like an ordinary sent photo — it is sealed shut for the recipient until revealAt.
        let stillSealed = !m.incoming && m.revealAt > 0 && now > 0 && now < m.revealAt
        return HStack {
            if !m.incoming { Spacer(minLength: 40) }
            VStack(alignment: m.incoming ? .leading : .trailing, spacing: 3) {
                Group {
                    if destroying {
                        SmokeDestroy(onFinished: onDestroyed) { bubbleBody }
                    } else {
                        bubbleBody
                    }
                }
                if m.state == .sealing {
                    HStack(spacing: 4) {
                        Image(systemName: "arrow.triangle.2.circlepath").font(.system(size: 11))
                        Text(m.mode == "FAST" ? "Pre-confirming…" : "Sealing…").font(.system(size: 11))
                    }.foregroundColor(.dvSub)
                }
                if stillSealed {
                    HStack(spacing: 4) {
                        Image(systemName: "lock.fill").font(.system(size: 11))
                        Text("Sealed · opens in \(countdownLabel(m.revealAt - now))").font(.system(size: 11))
                    }.foregroundColor(.dvBlue)
                }
                if showDestroyClock {
                    HStack(spacing: 4) {
                        Image(systemName: "flame.fill").font(.system(size: 11))
                        Text("Destroys in \(countdownLabel(m.destroyAt - now))").font(.system(size: 11))
                    }.foregroundColor(Color(hex: 0xE0703A))
                }
            }
            if m.incoming { Spacer(minLength: 40) }
        }.padding(.vertical, 3)
    }

    private var bubbleBody: some View {
        content
            .padding(10)
            .background(m.incoming ? Color.white : Color.dvOut)
            .clipShape(RoundedRectangle(cornerRadius: 18))
            .frame(maxWidth: 300, alignment: m.incoming ? .leading : .trailing)
    }

    /// The locked card: a lock and a live countdown, and deliberately NO hint of the content —
    /// the preview is sealed inside the manifest, which cannot be opened before the window.
    private var lockedContent: some View {
        VStack(alignment: .leading, spacing: 6) {
            VStack(spacing: 6) {
                Image(systemName: "lock.fill").font(.system(size: 26)).foregroundColor(.dvSub)
                Text(countdownLabel(m.revealAt - now))
                    .font(.system(size: 26, weight: .bold)).foregroundColor(.dvInk)
                // A timelocked TEXT gets the same sealed card as a photo — the recipient sees
                // that something is waiting and when it opens, but never a hint of what it says.
                Text(m.kind == .image ? "Photo · opens in" : "Message · opens in")
                    .font(.system(size: 12)).foregroundColor(.dvSub)
            }
            .frame(width: 220, height: 160)
            .background(Color(hex: 0xE3E9EF))
            .clipShape(RoundedRectangle(cornerRadius: 12))
            // the sender's own caption stays readable — they wrote it; only the picture is sealed
            if !m.text.isEmpty, m.text != "Photo", m.text != "Video" {
                Text(m.text).font(.system(size: 15)).foregroundColor(.dvInk)
                    .frame(maxWidth: 220, alignment: .leading)
            }
        }
    }

    @ViewBuilder private var content: some View {
        switch m.state {
        case .sealing: sealing
        case .locked: lockedContent
        // Sealed and not yet open: show the lock card on the SENDER's side too. A translucent
        // scrim still let the picture read straight through it.
        case _ where sealedNow && m.kind == .image: lockedContent
        default:
            switch m.kind {
            case .image: imageContent
            case .voice: voiceContent
            case .text: textContent
            }
        }
    }

    private var textContent: some View {
        // One paragraph, not a stack: concatenated Text runs let the timestamp and read marker
        // sit at the end of the last line when there is room, and wrap onto their own line only
        // when there isn't. It also means the bubble hugs the content — the old version held a
        // leading Spacer() that expanded to the full 300pt proposal, so a five-letter message
        // got the same slab as a paragraph.
        (Text(m.text).font(.system(size: 15)).foregroundColor(.dvInk) + Text("  ") + metaText)
            .fixedSize(horizontal: false, vertical: true)
    }

    private var sealing: some View {
        VStack(alignment: .trailing, spacing: 4) {
            HStack(spacing: 8) {
                Image(systemName: "lock.fill").font(.system(size: 14)).foregroundColor(.dvSub)
                Text(m.mode == "FAST" ? "Waiting for pre-confirms" : "Waiting for finality")
                    .font(.system(size: 14)).foregroundColor(.dvSub)
            }
            Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub)
        }
    }

    private var imageContent: some View {
        VStack(alignment: .leading, spacing: 6) {
            imageOnly
            // the caption travelled sealed inside the manifest, so it is as private as the pixels
            if !m.text.isEmpty, m.text != "Photo", m.text != "Video" {
                Text(m.text).font(.system(size: 15)).foregroundColor(.dvInk).frame(maxWidth: 220, alignment: .leading)
            }
        }
    }

    /// True while THIS bubble is the sender's copy of an item still inside its reveal window.
    private var sealedNow: Bool { !m.incoming && m.revealAt > 0 && now > 0 && now < m.revealAt }

    /// True while this photo should wear the reveal halo.
    private var showRevealGlow: Bool {
        if m.justRevealed { return true }
        // The sender watches the same countdown, so their own copy earns the halo the moment it
        // opens. Derived from the clock rather than a flag — there is no locked placeholder to
        // replace on this side, the bubble simply stops being sealed.
        guard !m.incoming, m.revealAt > 0, now > 0 else { return false }
        let since = now - m.revealAt
        return since >= 0 && since < Int64(revealGlowSeconds)
    }

    private var imageOnly: some View {
        // Renders the DECRYPTED local file. By this point the bytes exist in readable form
        // only on this device — nothing here touches the network.
        let isVideo = m.mediaMime.hasPrefix("video")
        return ZStack {
        // The halo sits BEHIND the picture and slightly proud of it, so what you see is colour
        // moving around the edges rather than a tint laid over the photo itself.
        if showRevealGlow { RevealHalo(width: 238, height: 178) }
        ZStack(alignment: .bottomTrailing) {
            Group {
                if let img = decodedMedia {
                    Image(uiImage: img).resizable().aspectRatio(contentMode: .fill)
                } else {
                    LinearGradient(colors: [Color(hex: 0xF6B26B), Color(hex: 0x6FA8DC), Color(hex: 0x2E5B8A)],
                                   startPoint: .top, endPoint: .bottom)
                }
            }
            .frame(width: 220, height: 160).clipped().clipShape(RoundedRectangle(cornerRadius: 12))

            if isVideo {
                Image(systemName: "play.circle.fill")
                    .font(.system(size: 44)).foregroundColor(.white.opacity(0.85))
                    .frame(width: 220, height: 160)
            }
            HStack(spacing: 3) {
                Text(m.time).font(.system(size: 11)).foregroundColor(.white)
                // Photos overlay their own status corner rather than using the meta run, so the
                // read indicator has to be repeated here — otherwise a sent photo keeps showing
                // the green "sealed" badge and never reports read, unlike every text message.
                if !m.incoming {
                    Text("R").font(.system(size: 12, weight: .bold))
                        .foregroundColor(m.read ? .dvBlue : .white)
                } else {
                    sealBadge
                }
            }.padding(6)
        }
        }
        .contentShape(Rectangle())
        .onTapGesture {
            if isVideo, !m.mediaPath.isEmpty { playVideo(URL(fileURLWithPath: m.mediaPath)) }
        }
    }

    /// First frame for a video, or the image itself — cached, since a bubble's body re-runs
    /// on every scroll pass.
    private var decodedMedia: UIImage? {
        MediaThumbs.thumb(path: m.mediaPath, isVideo: m.mediaMime.hasPrefix("video"))
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

    /// The trailing run: badges, timestamp, read marker. A `Text` rather than a `View` so it can
    /// be concatenated onto the message body — that is what makes it flow at the end of the last
    /// line instead of claiming a row of its own.
    private var metaText: Text {
        var t = Text("")
        if m.revealAt > 0 {
            t = t + Text(Image(systemName: "clock")).font(.system(size: 10)).foregroundColor(.dvBlue)
                  + Text(" Opens in \(relLabel(m.revealAt))  ").font(.system(size: 10)).foregroundColor(.dvBlue)
        } else if m.destroyAt > 0 {
            t = t + Text("💥 \(relLabel(m.destroyAt))  ").font(.system(size: 10)).foregroundColor(Color(hex: 0xE0403A))
        }
        if let who = m.sealedFor {
            t = t + Text(Image(systemName: "lock.fill")).font(.system(size: 10)).foregroundColor(.dvBlue)
                  + Text(" Sealed for \(who)  ").font(.system(size: 10)).foregroundColor(.dvBlue)
        }
        t = t + Text(m.time).font(.system(size: 11)).foregroundColor(.dvSub)
        // Read status on our own messages: grey R once delivered, blue once the recipient has
        // opened it. An R rather than ticks, because ticks read as "delivered" everywhere else
        // and this is specifically about being READ.
        if !m.incoming {
            t = t + Text(" R").font(.system(size: 12, weight: .bold)).foregroundColor(m.read ? .dvBlue : .dvSub)
        }
        return t
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
