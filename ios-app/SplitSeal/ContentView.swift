import SwiftUI

extension Color {
    static let ssBg     = Color(red: 0x0E/255, green: 0x16/255, blue: 0x21/255)
    static let ssPanel  = Color(red: 0x17/255, green: 0x21/255, blue: 0x2B/255)
    static let ssBubble = Color(red: 0x18/255, green: 0x25/255, blue: 0x33/255)
    static let ssAccent = Color(red: 0x2E/255, green: 0xA6/255, blue: 0xFF/255)
    static let ssLocked = Color(red: 0xF2/255, green: 0xB0/255, blue: 0x1E/255)
    static let ssOk     = Color(red: 0x4F/255, green: 0xCB/255, blue: 0x6B/255)
}

struct SealMsg: Identifiable {
    let id = UUID()
    let body: String
    var state: String = "DELIVERED_LOCKED" // -> FINALISING -> UNLOCKED / LOCKED
    var plaintext: String? = nil
    var shares: Int = 0
    var status: String? = nil
    var mode: String = "STRICT" // "STRICT" (finality) or "FAST" (pre-confirmations)
}

struct ContentView: View {
    @State private var messages: [SealMsg] = []
    @State private var draft = "Sealed by WCAHT before it opens."
    @State private var fastMode = false

    private let proto: String = {
        if let data = SealCore.version().data(using: .utf8),
           let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let v = j["version"], let c = j["chain_id"] {
            return "DSCP-\(v) · chain \(c)"
        }
        return "DSCP-1"
    }()

    var body: some View {
        VStack(spacing: 0) {
            header
            if messages.isEmpty { emptyState } else { conversation }
            composer
        }
        .background(Color.ssBg.ignoresSafeArea())
    }

    private var header: some View {
        VStack(spacing: 10) {
            HStack(spacing: 12) {
                Image(systemName: "shield.lefthalf.filled")
                    .foregroundColor(.white).padding(9)
                    .background(Color.ssAccent).clipShape(Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text("Denvion SplitSeal").font(.system(size: 16, weight: .semibold))
                    Text("Secured by WCAHT · \(proto)").font(.system(size: 11)).foregroundColor(.white.opacity(0.55))
                }
                Spacer()
            }
            Picker("Release mode", selection: $fastMode) {
                Text("StrictSeal · vault").tag(false)
                Text("FastSeal · instant").tag(true)
            }
            .pickerStyle(.segmented)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .background(Color.ssPanel)
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            Spacer()
            Image(systemName: "lock.rotation").font(.system(size: 52)).foregroundColor(.ssAccent)
            Text("Every message arrives locked").font(.system(size: 16, weight: .semibold))
            Text("It opens only when its WCAHT seal releases — hard L1 finality (StrictSeal) or a staked gateway pre-confirmation quorum (FastSeal).")
                .font(.system(size: 13)).foregroundColor(.white.opacity(0.55))
                .multilineTextAlignment(.center).padding(.horizontal, 40)
            Spacer()
        }.frame(maxWidth: .infinity)
    }

    private var conversation: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 8) {
                    ForEach(messages) { Bubble(m: $0).id($0.id) }
                }.padding(12)
            }
            .onChange(of: messages.count) { _ in
                if let last = messages.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
    }

    private var composer: some View {
        HStack(spacing: 8) {
            TextField("Send a sealed message…", text: $draft, axis: .vertical)
                .lineLimit(1...4).padding(12)
                .background(Color.ssBg).cornerRadius(20)
                .onSubmit(send)
            Button(action: send) {
                Image(systemName: "lock.open.fill").foregroundColor(.white)
                    .frame(width: 44, height: 44).background(Color.ssAccent).clipShape(Circle())
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(Color.ssPanel)
    }

    private func send() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let fast = fastMode
        messages.append(SealMsg(body: text, mode: fast ? "FAST" : "STRICT"))
        let idx = messages.count - 1
        draft = ""

        Task { @MainActor in
            let json = fast ? SealCore.runFastDemo() : SealCore.runDemo()
            guard let data = json.data(using: .utf8),
                  let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let steps = root["transcript"] as? [[String: Any]] else {
                try? await Task.sleep(nanoseconds: 700_000_000)
                update(idx) { $0.state = "FINALISING" }
                try? await Task.sleep(nanoseconds: 900_000_000)
                update(idx) { $0.state = "UNLOCKED"; $0.plaintext = text; $0.shares = 3; $0.status = "opened (fallback)" }
                return
            }
            // StrictSeal steps are before_finality/after_finality; FastSeal steps are
            // before_preconf/after_preconf_quorum. Handle both shapes uniformly.
            for step in steps {
                let name = step["step"] as? String ?? ""
                if name == "before_finality" || name == "before_preconf" {
                    try? await Task.sleep(nanoseconds: 650_000_000)
                    update(idx) { $0.state = "FINALISING" }
                } else if name == "after_finality" || name == "after_preconf_quorum" {
                    try? await Task.sleep(nanoseconds: fast ? 220_000_000 : 900_000_000)
                    let out = step["outcome"] as? [String: Any]
                    let count = (step["shares_released"] as? Int) ?? (step["preconfs"] as? Int) ?? 0
                    if (out?["result"] as? String) == "OPENED" {
                        update(idx) {
                            $0.state = "UNLOCKED"; $0.plaintext = text; $0.shares = count
                            $0.status = fast ? "pre-confirmed · no finality" : (step["status"] as? String ?? "finalised")
                        }
                    } else {
                        update(idx) { $0.state = "LOCKED"; $0.status = out?["reason"] as? String }
                    }
                }
            }
        }
    }

    private func update(_ idx: Int, _ f: (inout SealMsg) -> Void) {
        guard messages.indices.contains(idx) else { return }
        f(&messages[idx])
    }
}

private struct Bubble: View {
    let m: SealMsg
    private var unlocked: Bool { m.state == "UNLOCKED" }

    var body: some View {
        HStack {
            Spacer(minLength: 40)
            VStack(alignment: .leading, spacing: 6) {
                if unlocked {
                    Text(m.plaintext ?? m.body).font(.system(size: 15)).foregroundColor(.white)
                } else {
                    HStack(spacing: 8) {
                        Image(systemName: "lock.fill").font(.system(size: 13)).foregroundColor(.ssLocked)
                        Text(String(repeating: "•", count: min(max(m.body.count, 6), 22)))
                            .foregroundColor(.ssLocked).tracking(2)
                    }
                }
                statusChip
            }
            .padding(.init(top: 10, leading: 14, bottom: 8, trailing: 14))
            .background(unlocked ? Color.ssBubble : Color.ssPanel)
            .overlay(RoundedRectangle(cornerRadius: 14)
                .stroke((unlocked ? Color.ssOk : Color.ssLocked).opacity(0.4), lineWidth: 1))
            .cornerRadius(14)
            .frame(maxWidth: 300, alignment: .trailing)
        }
    }

    private var statusChip: some View {
        let fast = m.mode == "FAST"
        let (label, color, icon): (String, Color, String) = {
            switch m.state {
            case "DELIVERED_LOCKED":
                return (fast ? "Delivered · FastSeal" : "Delivered · locked", .ssLocked, "lock")
            case "FINALISING":
                return fast
                    ? ("Awaiting gateway pre-confs", .ssLocked, "bolt.horizontal")
                    : ("Securing seal · finalising", .ssLocked, "hourglass")
            case "UNLOCKED":
                return ("Opened · \(m.status ?? (fast ? "pre-confirmed" : "finalised"))", .ssOk, fast ? "bolt.fill" : "checkmark.seal.fill")
            default:
                return ("Locked", .ssLocked, "lock")
            }
        }()
        return HStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 11)).foregroundColor(color)
            Text(label).font(.system(size: 11)).foregroundColor(color)
        }
    }
}
