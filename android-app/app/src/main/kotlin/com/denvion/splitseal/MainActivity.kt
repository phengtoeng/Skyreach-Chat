package com.denvion.splitseal

import android.app.Activity
import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import org.json.JSONArray
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material.icons.outlined.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import dev.chrisbanes.haze.HazeState
import dev.chrisbanes.haze.HazeStyle
import dev.chrisbanes.haze.haze
import dev.chrisbanes.haze.hazeChild
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// ─────────────────────────────── palette ────────────────────────────────────
private val Blue = Color(0xFF2E9BF6)
private val ScreenBg = Color(0xFFFFFFFF)
private val ConvBg = Color(0xFFE8F1F8)
private val Incoming = Color(0xFFFFFFFF)
private val Outgoing = Color(0xFFD3EAFB)
private val Ink = Color(0xFF101720)
private val Sub = Color(0xFF8C99A6)
private val Hair = Color(0xFFECEFF2)
private val Green = Color(0xFF33C15A)
private val BarActive = Color(0x1A101720)  // pill behind the selected tab (soft grey on glass)
private val BarEdge = Color(0x99FFFFFF)    // hairline highlight around the glass
private val BarShadow = Color(0x26000000)  // soft drop shadow (API 28+ honours the colour)
private val BadgeRed = Color(0xFFF03D3D)

/**
 * Frosted-glass for the floating bar, matching iOS `.ultraThinMaterial`: whatever scrolls
 * underneath is blurred and tinted white. Real backdrop blur needs API 31+; below that haze
 * falls back to a plain translucent scrim, which still reads as glass.
 */
private val Glass = HazeStyle(
    tint = Color.White.copy(alpha = 0.72f),
    blurRadius = 20.dp,
    noiseFactor = 0.03f,
)

// Bottom-bar tabs, left to right.
private const val TAB_CONTACTS = 0
private const val TAB_CALLS = 1
private const val TAB_CHATS = 2
private const val TAB_SETTINGS = 3
private val AvatarColors = listOf(
    Color(0xFF5B8DEF), Color(0xFFEF6F6C), Color(0xFF3FBF8F), Color(0xFFF0A84A),
    Color(0xFF9B7EDE), Color(0xFF48B0C7), Color(0xFFE07AAE),
)

private fun now(): String = SimpleDateFormat("h:mm a", Locale.US).format(Date())
/** Short "time from now" label for a unix-seconds target, e.g. "10m", "1h", "1d". */
private fun relLabel(targetUnixSecs: Long): String {
    val s = targetUnixSecs - System.currentTimeMillis() / 1000
    if (s <= 0) return "now"
    return when {
        s < 3600 -> "${(s / 60).coerceAtLeast(1)}m"
        s < 86400 -> "${s / 3600}h"
        else -> "${s / 86400}d"
    }
}
private fun avatarColor(name: String) = AvatarColors[(name.hashCode() and 0x7fffffff) % AvatarColors.size]
private fun initials(name: String) =
    name.trim().split(" ").filter { it.isNotEmpty() }.take(2).joinToString("") { it.first().uppercase() }

// ─────────────────────────────── models ─────────────────────────────────────
private data class Chat(val name: String, val last: String, val time: String, val unread: Int = 0, val devicePub: String = "", val identityPub: String = "", val isContact: Boolean = false)

private enum class Kind { TEXT, IMAGE, VOICE }
private enum class State {
    PLAIN, SEALING, OPENED,
    /** Received but time-locked: the gateways withhold the shares until `revealAt`, so there is
     *  nothing to render yet. The bubble shows a lock + countdown, never a hint of the content. */
    LOCKED,
}

private data class Msg(
    val id: Long,
    val text: String,
    val incoming: Boolean,
    val time: String,
    val kind: Kind = Kind.TEXT,
    val state: State = State.PLAIN,
    val read: Boolean = true,
    val mode: String = "STRICT",
    val sealedFor: String? = null, // set when sealed to a real contact's device key
    val revealAt: Long = 0, // unix secs: timelocked to open at/after this time (0 = none)
    val destroyAt: Long = 0, // unix secs: self-destructs after this time (0 = none)
    val mediaPath: String = "", // decrypted file on THIS device only; never uploaded
    val mediaMime: String = "",
    /** Set on a LOCKED placeholder so the real item can replace it when the window opens. */
    val lockedSealId: String = "",
)

private val CHATS = listOf(
    Chat("Maya", "See you tonight!", "9:41 AM", 2),
    Chat("Ethan", "That works for me.", "9:32 AM"),
    Chat("Lena", "Thanks for the update.", "Yesterday", 1),
    Chat("Weekend Plan", "You: Looking forward!", "Yesterday"),
    Chat("Noah", "Let's catch up soon.", "Tue"),
    Chat("Zoe", "Photo", "Mon", 1),
    Chat("Daniel", "Sounds good.", "Mon"),
)

private fun seedThread(): List<Msg> = listOf(
    Msg(1, "Hey! How was your day?", true, "9:24 AM"),
    Msg(2, "Pretty good! How about yours?", false, "9:25 AM", read = true),
    Msg(3, "Busy, but productive.", true, "9:26 AM"),
    Msg(4, "Nice! Anything fun later?", false, "9:27 AM", read = true),
    Msg(5, "Maybe dinner. I'll let you know!", true, "9:28 AM"),
)

// ─────────────────────── identity + contacts (persisted) ────────────────────
private data class Contact(val name: String, val address: String, val devicePub: String, val identityPub: String, val phone: String)

private class Store(ctx: Context) {
    private val p = ctx.getSharedPreferences("denvion", Context.MODE_PRIVATE)
    fun identity(): JSONObject? = p.getString("identity", null)?.let { JSONObject(it) }
    fun saveIdentity(j: JSONObject) = p.edit().putString("identity", j.toString()).apply()
    fun contacts(): JSONArray = p.getString("contacts", null)?.let { JSONArray(it) } ?: JSONArray()
    fun addContact(c: JSONObject) {
        val a = contacts(); a.put(c); p.edit().putString("contacts", a.toString()).apply()
    }
    /** Add a contact parsed from a card (from an inbound message) unless one with the same
     *  identity_pub already exists. Returns true if it was newly added. */
    fun addContactFromCard(card: JSONObject): Boolean {
        val idp = card.optString("identity_pub")
        val a = contacts()
        if (idp.isNotBlank()) for (i in 0 until a.length()) if (a.getJSONObject(i).optString("identity_pub") == idp) return false
        a.put(
            JSONObject().put("name", card.optString("name", "Contact")).put("address", card.optString("address"))
                .put("device_pub", card.optString("device_pub")).put("identity_pub", idp),
        )
        p.edit().putString("contacts", a.toString()).apply()
        return true
    }
    /** Remove a saved contact (match by identity_pub when present, else by name). */
    fun removeContact(identityPub: String, name: String) {
        val a = contacts(); val out = JSONArray()
        for (i in 0 until a.length()) {
            val o = a.getJSONObject(i)
            val match = if (identityPub.isNotBlank()) o.optString("identity_pub") == identityPub else o.optString("name") == name
            if (!match) out.put(o)
        }
        p.edit().putString("contacts", out.toString()).apply()
    }

    // Demo placeholder chats can't be "deleted" (they aren't real contacts) — hide them by name.
    fun hiddenChats(): Set<String> = p.getString("hidden_chats", null)?.let {
        val a = JSONArray(it); (0 until a.length()).map { i -> a.getString(i) }.toSet()
    } ?: emptySet()
    fun hideChat(name: String) {
        val s = hiddenChats().toMutableSet(); s.add(name)
        p.edit().putString("hidden_chats", JSONArray(s.toList()).toString()).apply()
    }

    // backend host (relay + gateways + directory)
    fun serverHost(): String = p.getString("server_host", Server.DEFAULT_HOST) ?: Server.DEFAULT_HOST
    fun saveServerHost(h: String) = p.edit().putString("server_host", h.trim()).apply()

    // received messages, persisted + deduped per mailbox tag (all my inbound share my tag).
    fun inbox(tag: String): JSONArray = p.getString("inbox_$tag", null)?.let { JSONArray(it) } ?: JSONArray()
    /** Append a received message (tagged with the sender's identity_pub); false if already stored. */
    fun addInbox(tag: String, sealId: String, text: String, sender: String): Boolean {
        val a = inbox(tag)
        for (i in 0 until a.length()) if (a.getJSONObject(i).optString("id") == sealId) return false
        a.put(JSONObject().put("id", sealId).put("text", text).put("sender", sender).put("ts", System.currentTimeMillis()))
        p.edit().putString("inbox_$tag", a.toString()).apply()
        return true
    }
    /** Drop every received message from a given sender (used when deleting that conversation). */
    fun clearInboxFrom(tag: String, sender: String) {
        if (tag.isBlank()) return
        val a = inbox(tag); val out = JSONArray()
        for (i in 0 until a.length()) {
            val o = a.getJSONObject(i)
            if (o.optString("sender") != sender) out.put(o)
        }
        p.edit().putString("inbox_$tag", out.toString()).apply()
    }

    // Per-conversation transcript (BOTH directions), keyed by the peer's identity_pub — the durable
    // chat history. (inbox_<tag> above is only the opened-seal dedup set for incoming.)
    fun thread(peer: String): JSONArray =
        if (peer.isBlank()) JSONArray() else p.getString("thread_$peer", null)?.let { JSONArray(it) } ?: JSONArray()
    /** Append a message to a conversation (dedup by id); false if already stored. */
    fun addThreadMsg(
        peer: String,
        id: String,
        text: String,
        incoming: Boolean,
        media: String = "", // local path to the DECRYPTED file (this device only)
        mime: String = "",
        destroyAt: Long = 0, // persisted so a self-destruct still fires after an app restart
    ): Boolean {
        if (peer.isBlank()) return false
        val a = thread(peer)
        for (i in 0 until a.length()) if (a.getJSONObject(i).optString("id") == id) return false
        a.put(
            JSONObject().put("id", id).put("text", text).put("incoming", incoming)
                .put("ts", System.currentTimeMillis()).put("media", media).put("mime", mime)
                .put("destroy_at", destroyAt)
        )
        p.edit().putString("thread_$peer", a.toString()).apply()
        return true
    }
    fun clearThread(peer: String) { if (peer.isNotBlank()) p.edit().remove("thread_$peer").apply() }

    /** Drop a self-destructed item from the durable transcript so it never comes back on reopen. */
    fun removeThreadMedia(peer: String, mediaPath: String) {
        if (peer.isBlank() || mediaPath.isBlank()) return
        val a = thread(peer)
        val keep = JSONArray()
        for (i in 0 until a.length()) {
            val o = a.getJSONObject(i)
            if (o.optString("media") != mediaPath) keep.put(o)
        }
        p.edit().putString("thread_$peer", keep.toString()).apply()
    }
}

private fun loadOrCreateIdentity(store: Store): JSONObject {
    store.identity()?.let { return it }
    val id = JSONObject(SealCore.newIdentity("Me"))
    store.saveIdentity(id)
    return id
}

/** Parse a pasted/scanned contact code, tolerant of copy noise: strips whitespace/newlines and
 *  auto-adds the "denvion:" prefix if the user copied only the card body. Returns null if invalid. */
private fun tryParseCard(code: String): JSONObject? {
    val cleaned = code.trim().replace(Regex("\\s"), "")
    if (cleaned.isEmpty()) return null
    val candidates = if (cleaned.startsWith("denvion:")) listOf(cleaned) else listOf(cleaned, "denvion:$cleaned")
    for (cand in candidates) {
        val r = runCatching { JSONObject(SealCore.parseCard(cand)) }.getOrNull() ?: continue
        if (r.optBoolean("ok")) return r
    }
    return null
}

private fun loadContacts(store: Store): List<Contact> {
    val a = store.contacts()
    return (0 until a.length()).map {
        val o = a.getJSONObject(it)
        Contact(o.optString("name"), o.optString("address"), o.optString("device_pub"), o.optString("identity_pub"), o.optString("phone"))
    }
}

// ── Configurable backend: relay + 3 gateways + directory all live on ONE host. ──
// Default = WCAHT node N6 (reachable from any device). Override it in Settings — e.g.
// "10.0.2.2" for services on the emulator's own host machine. Only a hostname/IP: the
// ports are fixed. Nothing here is secret; the servers only ever see ciphertext + hashes.
object Server {
    const val DEFAULT_HOST = "51.79.176.134" // N6
    @Volatile var host: String = DEFAULT_HOST
    // The seal backbone runs on all 3 nodes (N5, N6, N7) so no single node is a point of failure.
    val nodeHosts = listOf("139.99.150.23", "51.79.176.134", "51.79.162.80")

    /** True when Settings points us at one specific machine instead of the live backbone. */
    private val pinned: Boolean get() = host.isNotBlank() && host != DEFAULT_HOST

    // Replicated relays: ship the ciphertext to ALL, read from ALL (merge) — delivery survives
    // any node outage as long as one relay that got the message is up.
    // Pinned to one host (dev/self-host), everything runs on that machine instead.
    val relays: List<String> get() = if (pinned) listOf("http://$host:9200") else nodeHosts.map { "http://$it:9200" }

    // 3 INDEPENDENT gateways, one per node (t=2 of 3): no single machine holds all key shares,
    // and any one gateway can be down and messages still open. Pinned to one host they sit on
    // consecutive ports — convenient for a local stack, but NOT independent, so dev only.
    val gateways: List<String> get() =
        if (pinned) listOf("http://$host:9201", "http://$host:9202", "http://$host:9203")
        else nodeHosts.map { "http://$it:9201" }
    // Replicated directories (gossip + persist server-side): register to ALL, look up on ANY.
    val directories: List<String> get() = nodeHosts.map { "http://$it:9988" }
}

private fun directoryLookup(phone: String): JSONObject? {
    val commit = runCatching { JSONObject(SealCore.phoneCommitment(phone)).optString("phone_commitment") }.getOrNull()
    if (commit.isNullOrEmpty()) return null
    // try each directory node until one answers (any replica resolves — survives a node outage).
    for (d in Server.directories) {
        val res = runCatching {
            val conn = (java.net.URL("$d/lookup/$commit").openConnection() as java.net.HttpURLConnection).apply {
                connectTimeout = 4000; readTimeout = 4000
            }
            if (conn.responseCode == 200) {
                val card = JSONObject(conn.inputStream.bufferedReader().use { it.readText() }).optString("card")
                JSONObject(SealCore.parseCard(card)).takeIf { it.optBoolean("ok") }
            } else null
        }.getOrNull()
        if (res != null) return res
    }
    return null
}

private fun directoryPublish(phone: String, cardCode: String): Boolean {
    val commit = runCatching { JSONObject(SealCore.phoneCommitment(phone)).optString("phone_commitment") }.getOrNull()
    if (commit.isNullOrEmpty()) return false
    // register to every directory node (they also gossip) — published as long as one accepts.
    var ok = false
    val body = JSONObject().put("commitment", commit).put("card", cardCode).toString()
    for (d in Server.directories) {
        val code = runCatching {
            val conn = (java.net.URL("$d/register").openConnection() as java.net.HttpURLConnection).apply {
                requestMethod = "POST"; doOutput = true; connectTimeout = 4000; readTimeout = 4000
                setRequestProperty("Content-Type", "application/json")
            }
            conn.outputStream.use { it.write(body.toByteArray()) }
            conn.responseCode
        }.getOrDefault(-1)
        if (code == 200) ok = true
    }
    return ok
}

private fun qrBitmap(text: String, size: Int = 480): androidx.compose.ui.graphics.ImageBitmap {
    val matrix = com.google.zxing.qrcode.QRCodeWriter().encode(text, com.google.zxing.BarcodeFormat.QR_CODE, size, size)
    val pixels = IntArray(size * size)
    for (y in 0 until size) for (x in 0 until size) {
        pixels[y * size + x] = if (matrix.get(x, y)) 0xFF000000.toInt() else 0xFFFFFFFF.toInt()
    }
    val bmp = android.graphics.Bitmap.createBitmap(size, size, android.graphics.Bitmap.Config.ARGB_8888)
    bmp.setPixels(pixels, 0, size, 0, 0, size, size)
    return bmp.asImageBitmap()
}

// Delivery services (see Server above). Ship ciphertext to the relay + shares to gateways.
private fun httpPost(url: String, body: String): Int = try {
    val conn = (java.net.URL(url).openConnection() as java.net.HttpURLConnection).apply {
        requestMethod = "POST"; doOutput = true; connectTimeout = 4000; readTimeout = 4000
        setRequestProperty("Content-Type", "application/json")
    }
    conn.outputStream.use { it.write(body.toByteArray()) }
    conn.responseCode
} catch (e: Exception) { -1 }

private fun httpGet(url: String): String? = try {
    val conn = (java.net.URL(url).openConnection() as java.net.HttpURLConnection).apply { connectTimeout = 4000; readTimeout = 4000 }
    if (conn.responseCode == 200) conn.inputStream.bufferedReader().use { it.readText() } else null
} catch (e: Exception) { null }

/** Ship a shippable seal: {seal_id,bundle} → ALL relays, each share → a gateway (+ finalize).
 *  Returns true if AT LEAST ONE relay accepted the ciphertext (delivery survives node outages). */
private fun shipSeal(ship: JSONObject): Boolean {
    val tag = ship.optString("mailbox_tag")
    val sealId = ship.optString("seal_id")
    // carry seal_id alongside the ciphertext so the recipient (who has neither) can collect shares.
    val item = JSONObject().put("seal_id", sealId).put("bundle", ship.getJSONObject("bundle"))
    // replicate the ciphertext to every relay so any one of them can serve the recipient.
    var relayOk = false
    for (r in Server.relays) if (httpPost("$r/inbox/$tag", item.toString()) in 200..299) relayOk = true
    val shares = ship.getJSONArray("shares")
    val gateways = Server.gateways
    // The timelock travels to the gateways as the SIGNED LEAF, not as bare numbers: the
    // gateway verifies the sender signature and reads the window out of the leaf, so nobody
    // else can install a different one.
    val bundle = ship.getJSONObject("bundle")
    val window = JSONObject()
        .put("signed_leaf", bundle.opt("signed_leaf"))
        .put("sender_id_pub", bundle.optString("sender_id_pub"))
        .toString()
    for (i in 0 until minOf(shares.length(), gateways.size)) {
        httpPost("${gateways[i]}/deposit", shares.getJSONObject(i).toString())
        httpPost("${gateways[i]}/finalize/$sealId", window)
    }
    return relayOk
}

/** Fetch every {seal_id,bundle} item for a mailbox tag from ALL relays, merged + deduped by
 *  seal_id — the recipient finds its messages on whichever relay(s) happen to be up. */
private fun fetchInboxAll(tag: String): List<JSONObject> {
    val byId = LinkedHashMap<String, JSONObject>()
    for (r in Server.relays) {
        val body = httpGet("$r/inbox/$tag") ?: continue
        val arr = runCatching { JSONArray(body) }.getOrNull() ?: continue
        for (i in 0 until arr.length()) {
            val o = arr.optJSONObject(i) ?: continue
            val id = o.optString("seal_id")
            if (id.isNotEmpty() && !byId.containsKey(id)) byId[id] = o
        }
    }
    return byId.values.toList()
}

/** The chain's current finalised slot, read from a WCAHT node's /health.
 *  Sealing uses it to put a chain-time floor in the leaf: the slot the chain must finalise
 *  past before the item may open. 0 means "couldn't reach a node" — the seal still carries
 *  its signed wall-clock window, it just has no chain floor. */
private fun finalizedSlot(): Long {
    for (h in Server.nodeHosts) {
        val body = httpGet("http://$h:8901/health") ?: continue
        val v = runCatching { JSONObject(body).optLong("finalized_slot") }.getOrNull() ?: continue
        if (v > 0) return v
    }
    return 0
}

/** Upload one encrypted media chunk to EVERY relay, addressed by its ciphertext hash.
 *  The relay verifies the hash matches the bytes, so it cannot substitute a chunk — and it
 *  holds no key, so it can never open one. True if at least one relay stored it. */
private fun uploadBlob(hashHex: String, bytes: ByteArray): Boolean {
    var ok = false
    for (r in Server.relays) {
        try {
            val conn = (java.net.URL("$r/blob/$hashHex").openConnection() as java.net.HttpURLConnection).apply {
                requestMethod = "PUT"; doOutput = true
                connectTimeout = 8000; readTimeout = 60000 // uploads are not 4-second work
                setRequestProperty("Content-Type", "application/octet-stream")
                setFixedLengthStreamingMode(bytes.size) // stream it; don't buffer a second copy
            }
            conn.outputStream.use { it.write(bytes) }
            if (conn.responseCode in 200..299) ok = true
        } catch (_: Exception) { /* try the next relay */ }
    }
    return ok
}

/** Fetch one encrypted chunk from whichever relay still has it. */
private fun downloadBlob(hashHex: String): ByteArray? {
    for (r in Server.relays) {
        try {
            val conn = (java.net.URL("$r/blob/$hashHex").openConnection() as java.net.HttpURLConnection).apply {
                connectTimeout = 8000; readTimeout = 60000
            }
            if (conn.responseCode == 200) return conn.inputStream.use { it.readBytes() }
        } catch (_: Exception) { /* try the next relay */ }
    }
    return null
}

// ───────────────────────────── media send / receive ─────────────────────────
//
// The picked file is copied into the app cache, sealed by Rust into encrypted chunks on
// disk, and only those chunks are uploaded. The readable original never leaves the device.

/** Copy a picked content:// item into our cache so Rust can read it by path. */
private fun cacheFromUri(ctx: Context, uri: android.net.Uri, name: String): java.io.File? = try {
    val f = java.io.File(ctx.cacheDir, name)
    ctx.contentResolver.openInputStream(uri)?.use { input -> f.outputStream().use { input.copyTo(it) } }
    if (f.length() > 0) f else null
} catch (_: Exception) { null }

/**
 * A deliberately TINY thumbnail: it is sealed inside the manifest, so it is only ever seen
 * by the recipient — but keeping it small is what makes it a blur rather than a preview
 * (spec §10.3: never a readable thumbnail for a locked item).
 */
private fun buildPreview(ctx: Context, src: java.io.File, isVideo: Boolean): java.io.File? = try {
    val bmp = if (isVideo) {
        android.media.MediaMetadataRetriever().use { it.setDataSource(src.path); it.getFrameAtTime(0) }
    } else {
        val opts = android.graphics.BitmapFactory.Options().apply { inSampleSize = 16 }
        android.graphics.BitmapFactory.decodeFile(src.path, opts)
    }
    if (bmp == null) null else {
        val small = android.graphics.Bitmap.createScaledBitmap(bmp, 32, 32, true)
        val out = java.io.File(ctx.cacheDir, "preview-${System.nanoTime()}.jpg")
        out.outputStream().use { small.compress(android.graphics.Bitmap.CompressFormat.JPEG, 60, it) }
        out
    }
} catch (_: Exception) { null }

/** Upload every encrypted chunk listed by `sealMediaFile`, then delete the local copies. */
private fun uploadChunks(sealed: JSONObject): Boolean {
    val chunks = sealed.optJSONArray("chunks") ?: return false
    for (i in 0 until chunks.length()) {
        val c = chunks.getJSONObject(i)
        val f = java.io.File(c.optString("path"))
        val bytes = runCatching { f.readBytes() }.getOrNull() ?: return false
        if (!uploadBlob(c.optString("hash"), bytes)) return false
        f.delete() // the relay has it now; no need to keep ciphertext on the sender
    }
    return true
}

/**
 * Receive one media seal: open the manifest, fetch every chunk the relay is holding, then
 * decrypt and reassemble. Returns (localPath, mime), or null while it is still locked or
 * the chunks have not all arrived — the caller simply retries on the next poll.
 */
private fun receiveMedia(ctx: Context, deviceSeed: String, sealId: String, bundle: String, shares: String, slot: Long): Triple<String, String, String>? {
    val previewPath = java.io.File(ctx.cacheDir, "pv-$sealId.jpg").path
    val info = runCatching { JSONObject(SealCore.openMediaInfo(deviceSeed, bundle, shares, previewPath, slot)) }.getOrNull() ?: return null
    if (!info.optBoolean("ok")) return null // locked, or not enough shares released yet

    val chunkDir = java.io.File(ctx.cacheDir, "chunks/$sealId").apply { mkdirs() }
    val hashes = info.optJSONArray("chunks") ?: return null
    for (i in 0 until hashes.length()) {
        val h = hashes.getString(i)
        val f = java.io.File(chunkDir, h)
        if (f.exists()) continue // resume: never re-download a chunk we already hold
        val bytes = downloadBlob(h) ?: return null
        f.writeBytes(bytes)
    }

    val mime = info.optString("mime_type", "application/octet-stream")
    val ext = when {
        mime.startsWith("video") -> "mp4"
        mime.contains("png") -> "png"
        else -> "jpg"
    }
    val mediaDir = java.io.File(ctx.filesDir, "media").apply { mkdirs() }
    val out = java.io.File(mediaDir, "$sealId.$ext")
    val done = runCatching {
        JSONObject(SealCore.openMediaFile(deviceSeed, bundle, shares, chunkDir.path, out.path, slot))
    }.getOrNull() ?: return null
    if (!done.optBoolean("ok")) return null
    chunkDir.deleteRecursively() // plaintext is assembled; the ciphertext copies are dead weight
    return Triple(out.path, mime, info.optString("caption"))
}

/**
 * The chain slot at which THIS seal was anchored, verified by us.
 *
 * Asks the gateways for the anchor tx signature, fetches that transaction from a WCAHT node,
 * and hands both to Rust — which recomputes the leaf hash from the bundle we already hold and
 * requires the anchor to have paid exactly that address. So the answer rests on a transaction
 * the chain confirmed, not on any gateway's claim. 0 = no verifiable anchor (yet).
 */
private fun verifiedAnchorSlot(sealId: String, bundle: String): Long {
    for (gw in Server.gateways) {
        val meta = httpGet("$gw/anchor/$sealId") ?: continue
        val sig = runCatching { JSONObject(meta).optString("signature") }.getOrNull().orEmpty()
        if (sig.isBlank()) continue
        for (h in Server.nodeHosts) {
            val txJson = httpGet("http://$h:8901/transaction/$sig") ?: continue
            val v = runCatching { JSONObject(SealCore.verifyAnchor(bundle, txJson)) }.getOrNull() ?: continue
            if (v.optBoolean("ok")) return v.optLong("anchor_slot")
            // An anchor that commits a DIFFERENT leaf is not a missing anchor — it means the
            // bundle we hold is not the one that was committed. Refuse it outright.
            if (v.optString("reason") == "anchor commits a different leaf") return -1L
        }
    }
    return 0
}

/** Collect released shares for a seal from all gateways. */
private fun collectShares(sealId: String): String {
    val all = JSONArray()
    for (gw in Server.gateways) {
        val body = httpGet("$gw/release/$sealId") ?: continue
        val arr = JSONArray(body)
        for (i in 0 until arr.length()) all.put(arr.getJSONObject(i))
    }
    return all.toString()
}

// ─────────────────────────────── activity ───────────────────────────────────
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Server.host = Store(this).serverHost() // apply the saved backend before any network call
        ensureNotifChannel(this)
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            runCatching { requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1001) }
        }
        val proto = runCatching {
            JSONObject(SealCore.version()).let { "DSCP-${it.getInt("version")} · chain ${it.getLong("chain_id")}" }
        }.getOrDefault("DSCP-2")
        setContent { MaterialTheme(colorScheme = lightColorScheme(primary = Blue)) { App(proto) } }
    }
}

// ── local notifications for inbound messages ──
private const val NOTIF_CHANNEL = "denvion_messages"
private var notifSeq = 2000

private fun ensureNotifChannel(ctx: Context) {
    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
        val ch = android.app.NotificationChannel(NOTIF_CHANNEL, "Messages", android.app.NotificationManager.IMPORTANCE_HIGH)
        ch.description = "New sealed messages"
        (ctx.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager).createNotificationChannel(ch)
    }
}

private fun notifyMessage(ctx: Context, title: String, text: String) {
    if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU &&
        androidx.core.content.ContextCompat.checkSelfPermission(ctx, android.Manifest.permission.POST_NOTIFICATIONS) !=
        android.content.pm.PackageManager.PERMISSION_GRANTED
    ) return
    val n = androidx.core.app.NotificationCompat.Builder(ctx, NOTIF_CHANNEL)
        .setSmallIcon(android.R.drawable.ic_dialog_email)
        .setContentTitle(title)
        .setContentText(text)
        .setAutoCancel(true)
        .setPriority(androidx.core.app.NotificationCompat.PRIORITY_HIGH)
        .build()
    runCatching { androidx.core.app.NotificationManagerCompat.from(ctx).notify(notifSeq++, n) }
}

@Composable
private fun SystemBars(color: Color, darkIcons: Boolean) {
    val view = LocalView.current
    if (!view.isInEditMode) {
        SideEffect {
            val window = (view.context as Activity).window
            window.statusBarColor = color.toArgb()
            WindowCompat.getInsetsController(window, view).isAppearanceLightStatusBars = darkIcons
        }
    }
}

@Composable
private fun App(proto: String) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { Store(ctx) }
    var identity by remember { mutableStateOf(loadOrCreateIdentity(store)) }
    var contacts by remember { mutableStateOf(loadContacts(store)) }
    var hidden by remember { mutableStateOf(store.hiddenChats()) }
    // My own inbox tag — every seal addressed to me lands here; also purged when I delete a chat.
    val myTag = remember {
        val dp = identity.optString("device_pub")
        if (dp.isBlank()) "" else runCatching { JSONObject(SealCore.mailboxTag(dp)).optString("mailbox_tag") }.getOrDefault("")
    }
    var openChat by remember { mutableStateOf<Chat?>(null) }

    // ── Always-on inbox poll (while on the chat list) ──
    // Receive messages even when the specific chat isn't open: fetch → open → auto-create the
    // sender as a replyable contact (from the card embedded in the bundle) → store → notify.
    val seenGlobal = remember { mutableSetOf<String>() }
    LaunchedEffect(openChat == null, myTag) {
        if (openChat != null || myTag.isBlank()) return@LaunchedEffect
        store.inbox(myTag).let { s -> for (i in 0 until s.length()) seenGlobal.add(s.getJSONObject(i).optString("id")) }
        while (openChat == null) {
            val items = withContext(Dispatchers.IO) { fetchInboxAll(myTag) }
            // read the chain once per pass so the core can check the seal's slot floor against
            // real finality rather than trusting that the gateways released
            val chainSlot = if (items.isEmpty()) 0L else withContext(Dispatchers.IO) { finalizedSlot() }
            for (item in items) {
                val sealId = item.optString("seal_id")
                val bundle = item.optJSONObject("bundle") ?: continue
                if (sealId.isBlank() || sealId in seenGlobal) continue
                val shares = withContext(Dispatchers.IO) { collectShares(sealId) }
                val opened = runCatching {
                    withContext(Dispatchers.IO) { JSONObject(SealCore.openReceived(identity.optString("device_seed"), bundle.toString(), shares)) }
                }.getOrNull()
                if (opened != null && opened.optBoolean("ok")) {
                    seenGlobal.add(sealId)
                    val text = opened.optString("plaintext")
                    val sender = bundle.optString("sender_id_pub")
                    var senderName = "New message"
                    val senderCard = bundle.optString("sender_card")
                    if (senderCard.isNotBlank()) {
                        val parsed = runCatching { JSONObject(SealCore.parseCard(senderCard)) }.getOrNull()
                        if (parsed != null && parsed.optBoolean("ok")) {
                            senderName = parsed.optString("name").ifBlank { senderName }
                            if (store.addContactFromCard(parsed)) contacts = loadContacts(store)
                        }
                    }
                    if (store.addInbox(myTag, sealId, text, sender)) {
                        store.addThreadMsg(sender, sealId, text, true)
                        notifyMessage(ctx, senderName, text)
                    }
                }
            }
            delay(3000)
        }
    }
    var tab by remember { mutableStateOf(TAB_CHATS) }
    var showNew by remember { mutableStateOf(false) }
    var scanned by remember { mutableStateOf<JSONObject?>(null) }

    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        result.contents?.let { code ->
            val res = JSONObject(SealCore.parseCard(code))
            if (res.optBoolean("ok")) scanned = res
            else android.widget.Toast.makeText(ctx, res.optString("error", "invalid code"), android.widget.Toast.LENGTH_SHORT).show()
        }
    }
    fun launchScan() {
        scanLauncher.launch(
            ScanOptions().apply {
                setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                setPrompt("Scan a Denvion contact code")
                setBeepEnabled(false)
                setOrientationLocked(false)
            }
        )
    }

    fun deleteChat(c: Chat) {
        if (c.isContact) {
            store.removeContact(c.identityPub, c.name)
            store.clearInboxFrom(myTag, c.identityPub)
            store.clearThread(c.identityPub)
            contacts = loadContacts(store)
        } else {
            store.hideChat(c.name)
            hidden = store.hiddenChats()
        }
    }

    // Tell a freshly-added contact they were added: ship a small hello so THEIR device gets a real
    // inbound event → the chat + a notification appear on their side ("X added you").
    fun sendHello(devicePub: String) {
        if (devicePub.isBlank()) return
        val seed = identity.optString("identity_seed")
        val card = identity.optString("card")
        val myName = identity.optString("name", "Someone").ifBlank { "Someone" }
        scope.launch(Dispatchers.IO) {
            val ship = runCatching {
                JSONObject(SealCore.sealShippable(seed, card, devicePub, "👋 $myName added you on Denvion", false))
            }.getOrNull()
            if (ship != null && ship.optBoolean("ok")) shipSeal(ship)
        }
    }

    fun publishPhone(phone: String) {
        identity.put("phone", phone)
        store.saveIdentity(identity)
        val card = identity.optString("card")
        scope.launch {
            val ok = withContext(Dispatchers.IO) { directoryPublish(phone, card) }
            android.widget.Toast.makeText(
                ctx,
                if (ok) "Published — others can add you by number" else "Publish failed (is the directory running?)",
                android.widget.Toast.LENGTH_SHORT,
            ).show()
        }
    }

    val chat = openChat
    when {
        chat != null -> ConversationScreen(
            chat,
            myDeviceSeed = identity.optString("device_seed"),
            myDevicePub = identity.optString("device_pub"),
            myIdentitySeed = identity.optString("identity_seed"),
            myCard = identity.optString("card"),
            store = store,
        ) { openChat = null }
        showNew -> NewContactScreen(
            scanned = scanned,
            onScan = { launchScan() },
            onLookup = { phone ->
                scope.launch {
                    val card = withContext(Dispatchers.IO) { directoryLookup(phone) }
                    if (card != null) {
                        scanned = card
                        android.widget.Toast.makeText(ctx, "Found ${card.optString("name")} in directory", android.widget.Toast.LENGTH_SHORT).show()
                    } else {
                        android.widget.Toast.makeText(ctx, "No directory match for that number", android.widget.Toast.LENGTH_SHORT).show()
                    }
                }
            },
            onPasteCode = { c ->
                val res = tryParseCard(c)
                if (res != null) {
                    scanned = res
                    android.widget.Toast.makeText(ctx, "Found ${res.optString("name", "contact")} — tap ✓ to save", android.widget.Toast.LENGTH_SHORT).show()
                } else {
                    android.widget.Toast.makeText(
                        ctx,
                        "That's not a contact code. Copy the full \"denvion:…\" code from the other person's Settings (under \"Share this code\") — not their address.",
                        android.widget.Toast.LENGTH_LONG,
                    ).show()
                }
            },
            onCancel = { showNew = false; scanned = null },
            onSave = { first, last, phone ->
                val nm = listOf(first, last).filter { it.isNotBlank() }.joinToString(" ")
                    .ifBlank { scanned?.optString("name") ?: "Contact" }
                val c = JSONObject().put("name", nm).put("phone", phone)
                scanned?.let {
                    c.put("address", it.optString("address"))
                        .put("device_pub", it.optString("device_pub"))
                        .put("identity_pub", it.optString("identity_pub"))
                }
                store.addContact(c)
                contacts = loadContacts(store)
                scanned?.optString("device_pub")?.let { if (it.isNotBlank()) sendHello(it) }
                showNew = false; scanned = null
            },
        )
        tab == TAB_SETTINGS -> ProfileScreen(
            identity, tab,
            currentHost = Server.host,
            onTab = { tab = it },
            onPublish = { publishPhone(it) },
            onSaveName = { newName ->
                val nm = newName.trim().ifBlank { "Me" }
                val rebuilt = runCatching {
                    JSONObject(SealCore.cardFor(identity.optString("identity_seed"), identity.optString("device_seed"), nm))
                }.getOrNull()
                if (rebuilt != null) {
                    // reassign a fresh object so the card/QR recompose; identity_pub/address stay stable.
                    val next = JSONObject(identity.toString())
                        .put("name", nm).put("card", rebuilt.optString("card")).put("address", rebuilt.optString("address"))
                    store.saveIdentity(next)
                    identity = next
                    android.widget.Toast.makeText(ctx, "Name saved — your contact card now shows \"$nm\"", android.widget.Toast.LENGTH_SHORT).show()
                }
            },
            onSaveHost = { h ->
                Server.host = h
                store.saveServerHost(h)
                android.widget.Toast.makeText(ctx, "Server set to $h", android.widget.Toast.LENGTH_SHORT).show()
            },
        )
        else -> ChatListScreen(
            contacts = contacts,
            hidden = hidden,
            onOpen = { openChat = it },
            onAdd = { showNew = true; scanned = null },
            onDelete = { deleteChat(it) },
            tab = tab,
            onTab = { tab = it },
        )
    }
}

// ─────────────────────────────── chat list ──────────────────────────────────
@Composable
private fun ChatListScreen(
    contacts: List<Contact>,
    hidden: Set<String>,
    onOpen: (Chat) -> Unit,
    onAdd: () -> Unit,
    onDelete: (Chat) -> Unit,
    tab: Int,
    onTab: (Int) -> Unit,
) {
    SystemBars(Blue, darkIcons = false)
    var pendingDelete by remember { mutableStateOf<Chat?>(null) }
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    // real contacts first (address / phone as subtitle), then the demo threads (minus hidden ones)
    val contactRows = contacts.map {
        val sub = when {
            it.address.isNotBlank() -> it.address.take(14) + "… · tap to seal"
            it.phone.isNotBlank() -> "+855 " + it.phone
            else -> "tap to seal"
        }
        Chat(it.name, sub, "", devicePub = it.devicePub, identityPub = it.identityPub, isContact = true)
    }
    // The Contacts tab lists only people you've actually saved; Chats/Calls also show the threads.
    val all = if (tab == TAB_CONTACTS) contactRows else contactRows + CHATS.filter { it.name !in hidden }
    val rows = if (query.isBlank()) all else all.filter { it.name.contains(query, ignoreCase = true) }
    val title = when (tab) {
        TAB_CONTACTS -> "Contacts"
        TAB_CALLS -> "Calls"
        else -> "Denvion"
    }
    val hazeState = remember { HazeState() }
    // The bar is a sibling of the hazed content, never a child of it — haze blurs a node's own
    // subtree, so nesting the bar inside would feed the bar back into its own backdrop.
    Box(Modifier.fillMaxSize()) {
    // The background belongs INSIDE the hazed node — haze blurs that node's own drawing, so a
    // backdrop painted by the parent would leave the glass sampling transparent pixels.
    Column(Modifier.fillMaxSize().background(ScreenBg).haze(state = hazeState, style = Glass)) {
        Row(
            Modifier.fillMaxWidth().background(Blue).padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Shield, null, tint = Color.White, modifier = Modifier.size(24.dp))
            Spacer(Modifier.width(8.dp))
            Text(title, color = Color.White, fontSize = 21.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
        }
        if (searching) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    Modifier.weight(1f).clip(RoundedCornerShape(22.dp)).background(Hair)
                        .padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Filled.Search, null, tint = Sub, modifier = Modifier.size(18.dp))
                    Spacer(Modifier.width(8.dp))
                    Box(Modifier.weight(1f)) {
                        if (query.isEmpty()) Text("Search", color = Sub, fontSize = 15.sp)
                        BasicTextField(
                            value = query,
                            onValueChange = { query = it },
                            singleLine = true,
                            textStyle = TextStyle(color = Ink, fontSize = 15.sp),
                            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
                Spacer(Modifier.width(10.dp))
                Text(
                    "Cancel", color = Blue, fontSize = 15.sp,
                    modifier = Modifier.clickable { searching = false; query = "" },
                )
            }
        }
        LazyColumn(
            Modifier.weight(1f),
            // clear the floating bar + its shadow so the last row stays tappable
            contentPadding = PaddingValues(bottom = 96.dp),
        ) {
            items(rows) { c ->
                ChatRow(c, onClick = { onOpen(c) }, onLongClick = { pendingDelete = c })
                Box(Modifier.padding(start = 84.dp).fillMaxWidth().height(1.dp).background(Hair))
            }
        }
    }
        FloatingActionButton(
            onClick = onAdd,
            containerColor = Blue,
            contentColor = Color.White,
            modifier = Modifier.align(Alignment.BottomEnd).padding(end = 20.dp, bottom = 104.dp),
        ) { Icon(Icons.Filled.PersonAdd, "Add contact") }
        Box(Modifier.align(Alignment.BottomCenter)) {
            BottomBar(
                hazeState, tab, onTab,
                chatBadge = all.sumOf { it.unread },
                onSearch = { searching = !searching; if (!searching) query = "" },
            )
        }
    }

    pendingDelete?.let { target ->
        AlertDialog(
            onDismissRequest = { pendingDelete = null },
            title = { Text("Delete chat") },
            text = { Text("Delete your conversation with ${target.name}? This removes it from this device.") },
            confirmButton = {
                TextButton(onClick = { onDelete(target); pendingDelete = null }) {
                    Text("Delete", color = Color(0xFFE0403A), fontWeight = FontWeight.SemiBold)
                }
            },
            dismissButton = { TextButton(onClick = { pendingDelete = null }) { Text("Cancel", color = Sub) } },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ChatRow(c: Chat, onClick: () -> Unit, onLongClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(c.name, 52.dp)
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(c.name, color = Ink, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Spacer(Modifier.height(3.dp))
            Text(c.last, color = Sub, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(10.dp))
        Column(horizontalAlignment = Alignment.End) {
            Text(c.time, color = if (c.unread > 0) Blue else Sub, fontSize = 12.sp)
            Spacer(Modifier.height(6.dp))
            if (c.unread > 0) {
                Box(
                    Modifier.size(20.dp).background(Blue, CircleShape),
                    contentAlignment = Alignment.Center,
                ) { Text("${c.unread}", color = Color.White, fontSize = 11.sp, fontWeight = FontWeight.Bold) }
            } else {
                Spacer(Modifier.size(20.dp))
            }
        }
    }
}

/**
 * Floating bottom bar: a white capsule holding the four tabs, plus a detached round
 * search button. It overlays the content (the lists pad their bottom for it) instead
 * of docking to the window edge.
 */
@Composable
private fun BottomBar(
    hazeState: HazeState,
    tab: Int,
    onTab: (Int) -> Unit,
    chatBadge: Int = 0,
    settingsAlert: Boolean = false,
    onSearch: () -> Unit = {},
) {
    Row(
        Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(
            Modifier.weight(1f)
                // Keep this faint: the surface is translucent, so the shadow shows THROUGH the
                // glass and a normal-strength one greys the whole bar out.
                .shadow(6.dp, CircleShape, clip = false, ambientColor = BarShadow, spotColor = BarShadow)
                .hazeChild(state = hazeState, shape = CircleShape, style = Glass)
                .border(1.dp, BarEdge, CircleShape)
                .padding(horizontal = 6.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            NavItem(Icons.Filled.Person, "Contacts", tab == TAB_CONTACTS) { onTab(TAB_CONTACTS) }
            NavItem(Icons.Filled.Call, "Calls", tab == TAB_CALLS) { onTab(TAB_CALLS) }
            NavItem(Icons.Filled.ChatBubble, "Chats", tab == TAB_CHATS, badge = chatBadge) { onTab(TAB_CHATS) }
            NavItem(Icons.Filled.Settings, "Settings", tab == TAB_SETTINGS, dot = settingsAlert) { onTab(TAB_SETTINGS) }
        }
        Box(
            Modifier.size(58.dp)
                // Keep this faint: the surface is translucent, so the shadow shows THROUGH the
                // glass and a normal-strength one greys the whole bar out.
                .shadow(6.dp, CircleShape, clip = false, ambientColor = BarShadow, spotColor = BarShadow)
                .hazeChild(state = hazeState, shape = CircleShape, style = Glass)
                .border(1.dp, BarEdge, CircleShape)
                .clip(CircleShape)
                .clickable(onClick = onSearch),
            contentAlignment = Alignment.Center,
        ) { Icon(Icons.Filled.Search, "Search", tint = Ink, modifier = Modifier.size(26.dp)) }
    }
}

@Composable
private fun NavItem(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    active: Boolean,
    badge: Int = 0,
    dot: Boolean = false,
    onClick: () -> Unit,
) {
    val c = if (active) Blue else Ink
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier
            .clip(CircleShape)
            .background(if (active) BarActive else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 7.dp),
    ) {
        Box {
            Icon(icon, null, tint = c, modifier = Modifier.size(23.dp))
            if (badge > 0 || dot) {
                Box(
                    Modifier.align(Alignment.TopEnd).offset(x = 9.dp, y = (-6).dp)
                        .defaultMinSize(minWidth = 16.dp, minHeight = 16.dp)
                        .clip(CircleShape).background(BadgeRed)
                        .padding(horizontal = 4.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (dot && badge == 0) "!" else "$badge",
                        color = Color.White, fontSize = 10.sp, fontWeight = FontWeight.Bold,
                    )
                }
            }
        }
        Spacer(Modifier.height(3.dp))
        Text(label, color = c, fontSize = 11.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Medium)
    }
}

// ───────────────────────────── profile / add ────────────────────────────────
@Composable
private fun ProfileScreen(
    identity: JSONObject,
    tab: Int,
    currentHost: String,
    onTab: (Int) -> Unit,
    onPublish: (String) -> Unit,
    onSaveName: (String) -> Unit,
    onSaveHost: (String) -> Unit,
) {
    SystemBars(Blue, darkIcons = false)
    val name = identity.optString("name", "Me")
    val address = identity.optString("address")
    val card = identity.optString("card")
    val qr = remember(card) { runCatching { qrBitmap(card) }.getOrNull() }
    var myName by remember(name) { mutableStateOf(name) }
    var myPhone by remember { mutableStateOf(identity.optString("phone")) }
    var host by remember { mutableStateOf(currentHost) }
    val hazeState = remember { HazeState() }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().background(ScreenBg).haze(state = hazeState, style = Glass)) {
        Row(
            Modifier.fillMaxWidth().background(Blue).padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Shield, null, tint = Color.White, modifier = Modifier.size(24.dp))
            Spacer(Modifier.width(8.dp))
            Text("My Denvion ID", color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        }
        Box(Modifier.weight(1f)) {
            Column(
                Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Avatar(name, 76.dp)
                Spacer(Modifier.height(10.dp))
                Text(name, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, color = Ink)

                Spacer(Modifier.height(16.dp))
                Text("Your display name", fontSize = 13.sp, color = Sub)
                Spacer(Modifier.height(4.dp))
                Text("This is the name shown when others add you or get your messages.", fontSize = 11.sp, color = Sub, textAlign = TextAlign.Center)
                Spacer(Modifier.height(8.dp))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Color(0xFFF1F4F7)).padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.weight(1f)) {
                        if (myName.isEmpty()) Text("your name", color = Sub, fontSize = 15.sp)
                        BasicTextField(
                            value = myName,
                            onValueChange = { myName = it },
                            singleLine = true,
                            textStyle = TextStyle(color = Ink, fontSize = 15.sp),
                            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
                Spacer(Modifier.height(10.dp))
                Box(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                        .background(if (myName.isNotBlank() && myName != name) Blue else Color(0xFFCBD3DA))
                        .clickable(enabled = myName.isNotBlank() && myName != name) { onSaveName(myName) }
                        .padding(vertical = 14.dp),
                    contentAlignment = Alignment.Center,
                ) { Text("Save name", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }

                Spacer(Modifier.height(18.dp))
                Text("WCAHT identity address", fontSize = 12.sp, color = Sub)
                Spacer(Modifier.height(2.dp))
                SelectionContainer { Text(address, fontSize = 13.sp, color = Blue, textAlign = TextAlign.Center) }
                Spacer(Modifier.height(20.dp))
                if (qr != null) {
                    Image(
                        bitmap = qr,
                        contentDescription = "My QR code",
                        modifier = Modifier.size(230.dp).clip(RoundedCornerShape(12.dp)).border(1.dp, Hair, RoundedCornerShape(12.dp)),
                    )
                }
                Spacer(Modifier.height(18.dp))
                Text("Share this code so others can add you", fontSize = 13.sp, color = Sub, textAlign = TextAlign.Center)
                Spacer(Modifier.height(6.dp))
                SelectionContainer {
                    Text(
                        card, fontSize = 11.sp, color = Ink, textAlign = TextAlign.Center,
                        modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(Color(0xFFF1F4F7)).padding(12.dp),
                    )
                }
                Spacer(Modifier.height(8.dp))
                val ctxCopy = LocalContext.current
                Box(
                    Modifier.clip(RoundedCornerShape(10.dp)).background(Blue)
                        .clickable {
                            val clip = ctxCopy.getSystemService(Context.CLIPBOARD_SERVICE) as? android.content.ClipboardManager
                            clip?.setPrimaryClip(android.content.ClipData.newPlainText("denvion code", card))
                            android.widget.Toast.makeText(ctxCopy, "Code copied — paste it on the other device", android.widget.Toast.LENGTH_SHORT).show()
                        }
                        .padding(horizontal = 18.dp, vertical = 10.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Filled.ContentCopy, null, tint = Color.White, modifier = Modifier.size(16.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("Copy my code", color = Color.White, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    }
                }

                Spacer(Modifier.height(24.dp))
                Box(Modifier.fillMaxWidth().height(1.dp).background(Hair))
                Spacer(Modifier.height(16.dp))
                Text("Let others add you by phone number", fontSize = 13.sp, color = Sub, textAlign = TextAlign.Center)
                Spacer(Modifier.height(10.dp))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Color(0xFFF1F4F7)).padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("+855", color = Ink, fontSize = 15.sp)
                    Spacer(Modifier.width(12.dp))
                    Box(Modifier.weight(1f)) {
                        if (myPhone.isEmpty()) Text("your number", color = Sub, fontSize = 15.sp)
                        BasicTextField(
                            value = myPhone,
                            onValueChange = { myPhone = it.filter { c -> c.isDigit() || c == ' ' } },
                            textStyle = TextStyle(color = Ink, fontSize = 15.sp),
                            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
                Spacer(Modifier.height(10.dp))
                Box(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                        .background(if (myPhone.isNotBlank()) Blue else Color(0xFFCBD3DA))
                        .clickable(enabled = myPhone.isNotBlank()) { onPublish(myPhone.trim()) }
                        .padding(vertical = 14.dp),
                    contentAlignment = Alignment.Center,
                ) { Text("Publish my number to the directory", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
                Spacer(Modifier.height(6.dp))
                Text("Only a hash of your number is stored — never the number itself.", fontSize = 11.sp, color = Sub, textAlign = TextAlign.Center)

                Spacer(Modifier.height(24.dp))
                Box(Modifier.fillMaxWidth().height(1.dp).background(Hair))
                Spacer(Modifier.height(16.dp))
                Text("Server", fontSize = 13.sp, color = Sub)
                Spacer(Modifier.height(4.dp))
                Text(
                    "Host running the relay + gateways + directory. Both people must point at the same one.",
                    fontSize = 11.sp, color = Sub, textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(10.dp))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(Color(0xFFF1F4F7)).padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.weight(1f)) {
                        if (host.isEmpty()) Text("server IP or hostname", color = Sub, fontSize = 15.sp)
                        BasicTextField(
                            value = host,
                            onValueChange = { host = it.trim() },
                            singleLine = true,
                            textStyle = TextStyle(color = Ink, fontSize = 15.sp),
                            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
                Spacer(Modifier.height(10.dp))
                Box(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
                        .background(if (host.isNotBlank()) Blue else Color(0xFFCBD3DA))
                        .clickable(enabled = host.isNotBlank()) { onSaveHost(host.trim()) }
                        .padding(vertical = 14.dp),
                    contentAlignment = Alignment.Center,
                ) { Text("Save server", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
                Spacer(Modifier.height(96.dp)) // clear the floating bar
            }
        }
        }
        Box(Modifier.align(Alignment.BottomCenter)) { BottomBar(hazeState, tab, onTab) }
    }
}

@Composable
private fun NewContactScreen(
    scanned: JSONObject?,
    onScan: () -> Unit,
    onLookup: (String) -> Unit,
    onPasteCode: (String) -> Unit,
    onCancel: () -> Unit,
    onSave: (String, String, String) -> Unit,
) {
    val bg = Color(0xFFF2F3F5)
    SystemBars(bg, darkIcons = true)
    val ctx = LocalContext.current
    var first by remember(scanned) { mutableStateOf(scanned?.optString("name") ?: "") }
    var last by remember { mutableStateOf("") }
    var phone by remember { mutableStateOf("") }
    var sync by remember { mutableStateOf(true) }
    var country by remember { mutableStateOf(COUNTRIES[0]) }
    var countryMenu by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") } // the denvion: contact code, typed or pasted
    val canSave = first.isNotBlank() || scanned != null

    fun syncToPhone() {
        if (!sync) return
        runCatching {
            val name = listOf(first, last).filter { it.isNotBlank() }.joinToString(" ").ifBlank { scanned?.optString("name") ?: "" }
            val i = android.content.Intent(android.content.Intent.ACTION_INSERT).apply {
                type = android.provider.ContactsContract.Contacts.CONTENT_TYPE
                if (name.isNotBlank()) putExtra(android.provider.ContactsContract.Intents.Insert.NAME, name)
                if (phone.isNotBlank()) putExtra(android.provider.ContactsContract.Intents.Insert.PHONE, "+${country.dial}${phone.filter { it.isDigit() }}")
                addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            ctx.startActivity(i)
        }
    }

    Column(Modifier.fillMaxSize().background(bg)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(36.dp).clip(CircleShape).background(Color.White).clickable(onClick = onCancel),
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Filled.Close, "Cancel", tint = Ink, modifier = Modifier.size(20.dp)) }
            Spacer(Modifier.weight(1f))
            Text("New Contact", fontSize = 17.sp, fontWeight = FontWeight.SemiBold, color = Ink)
            Spacer(Modifier.weight(1f))
            Box(
                Modifier.size(36.dp).clip(CircleShape).background(if (canSave) Blue else Color(0xFFCBD3DA))
                    .clickable(enabled = canSave) { syncToPhone(); onSave(first.trim(), last.trim(), phone.trim()) },
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Filled.Check, "Save", tint = Color.White, modifier = Modifier.size(20.dp)) }
        }

        Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp)) {
            FormField("First name", first, KeyboardType.Text) { first = it }
            Spacer(Modifier.height(10.dp))
            FormField("Last name", last, KeyboardType.Text) { last = it }
            Spacer(Modifier.height(18.dp))

            // working country picker
            Box {
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).border(1.dp, Hair, RoundedCornerShape(12.dp))
                        .clickable { countryMenu = true }.padding(16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("${country.flag}  ${country.name}", color = Ink, fontSize = 16.sp)
                    Spacer(Modifier.weight(1f))
                    Text("+${country.dial}", color = Sub, fontSize = 15.sp)
                    Icon(Icons.Filled.ArrowDropDown, null, tint = Sub)
                }
                DropdownMenu(expanded = countryMenu, onDismissRequest = { countryMenu = false }) {
                    COUNTRIES.forEach { c ->
                        DropdownMenuItem(text = { Text("${c.flag}  ${c.name}  +${c.dial}") }, onClick = { country = c; countryMenu = false })
                    }
                }
            }
            Spacer(Modifier.height(10.dp))
            FormField("Phone number", phone, KeyboardType.Phone) { phone = it.filter { c -> c.isDigit() || c == ' ' } }
            Spacer(Modifier.height(18.dp))

            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Sync Contact to Phone", color = Ink, fontSize = 16.sp)
                    Text("Also save it to your phone's contacts", color = Sub, fontSize = 12.sp)
                }
                Switch(checked = sync, onCheckedChange = { sync = it })
            }
            Spacer(Modifier.height(18.dp))

            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).clickable(onClick = onScan).padding(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.Filled.QrCode2, null, tint = Blue)
                Spacer(Modifier.width(12.dp))
                Text("Add via QR Code", color = Blue, fontSize = 16.sp)
            }

            // Add by the denvion: contact code. The field is EDITABLE so a code can be typed or
            // pasted with the keyboard, and so you can see/correct what actually arrived — the
            // clipboard button alone gave no way to recover from a truncated copy.
            Spacer(Modifier.height(18.dp))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).padding(16.dp)) {
                Text("Or add by contact code", color = Sub, fontSize = 13.sp)
                Spacer(Modifier.height(4.dp))
                Text("On the other phone: Settings ▸ Copy my code. Paste or type it here.", color = Sub, fontSize = 11.sp)
                Spacer(Modifier.height(10.dp))
                Box(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFFF1F4F7))
                        .padding(horizontal = 12.dp, vertical = 10.dp)
                ) {
                    if (code.isEmpty()) Text("denvion:…", color = Sub, fontSize = 12.sp)
                    BasicTextField(
                        value = code,
                        onValueChange = { code = it },
                        textStyle = TextStyle(color = Ink, fontSize = 12.sp),
                        cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                Spacer(Modifier.height(10.dp))
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Row(
                        Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).border(1.dp, Blue, RoundedCornerShape(12.dp))
                            .clickable {
                                val clip = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as? android.content.ClipboardManager
                                val t = clip?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString().orEmpty()
                                if (t.isBlank()) android.widget.Toast.makeText(ctx, "Clipboard is empty — copy the denvion: code first", android.widget.Toast.LENGTH_SHORT).show()
                                else code = t
                            }
                            .padding(vertical = 12.dp),
                        horizontalArrangement = Arrangement.Center,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.Filled.ContentPaste, null, tint = Blue, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("Paste", color = Blue, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    }
                    Spacer(Modifier.width(10.dp))
                    val ready = code.isNotBlank()
                    Box(
                        Modifier.weight(1f).clip(RoundedCornerShape(12.dp))
                            .background(if (ready) Blue else Color(0xFFCBD3DA))
                            .clickable(enabled = ready) { onPasteCode(code) }
                            .padding(vertical = 12.dp),
                        contentAlignment = Alignment.Center,
                    ) { Text("Use this code", color = Color.White, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
                }
            }

            if (phone.isNotBlank() && scanned == null) {
                Spacer(Modifier.height(12.dp))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).clickable { onLookup(phone) }.padding(16.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Filled.Search, null, tint = Blue)
                    Spacer(Modifier.width(12.dp))
                    Text("Look up this number on the directory", color = Blue, fontSize = 16.sp)
                }
            }

            if (scanned != null) {
                Spacer(Modifier.height(16.dp))
                Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color(0xFFE9F7EE)).padding(16.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Filled.CheckCircle, null, tint = Green, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Linked to WCAHT address", color = Ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    }
                    Spacer(Modifier.height(4.dp))
                    Text(scanned.optString("address"), color = Blue, fontSize = 12.sp)
                }
            }
        }
    }
}

private data class Country(val flag: String, val name: String, val dial: String)

private val COUNTRIES = listOf(
    Country("🇰🇭", "Cambodia", "855"),
    Country("🇺🇸", "United States", "1"),
    Country("🇬🇧", "United Kingdom", "44"),
    Country("🇻🇳", "Vietnam", "84"),
    Country("🇹🇭", "Thailand", "66"),
    Country("🇸🇬", "Singapore", "65"),
    Country("🇮🇳", "India", "91"),
    Country("🇦🇺", "Australia", "61"),
    Country("🇯🇵", "Japan", "81"),
    Country("🇨🇳", "China", "86"),
)

@Composable
private fun FormField(
    placeholder: String,
    value: String,
    keyboardType: KeyboardType = KeyboardType.Text,
    onChange: (String) -> Unit,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        placeholder = { Text(placeholder, color = Sub) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType),
        shape = RoundedCornerShape(12.dp),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = Blue,
            unfocusedBorderColor = Hair,
            focusedContainerColor = Color.White,
            unfocusedContainerColor = Color.White,
            cursorColor = Blue,
            focusedTextColor = Ink,
            unfocusedTextColor = Ink,
        ),
        modifier = Modifier.fillMaxWidth(),
    )
}

// ────────────────────────────── conversation ────────────────────────────────
@Composable
private fun ConversationScreen(
    chat: Chat,
    myDeviceSeed: String,
    myDevicePub: String,
    myIdentitySeed: String,
    myCard: String,
    store: Store,
    onBack: () -> Unit,
) {
    SystemBars(Color.White, darkIcons = true)
    BackHandler(onBack = onBack)

    // A "real" conversation is linked to a contact's device key (vs. the demo threads).
    val real = chat.devicePub.isNotBlank()
    // My own inbox tag: every message sealed TO me lands here, and I poll it to receive.
    val myTag = remember(myDevicePub) {
        if (myDevicePub.isBlank()) ""
        else runCatching { JSONObject(SealCore.mailboxTag(myDevicePub)).optString("mailbox_tag") }.getOrDefault("")
    }
    val seen = remember { mutableSetOf<String>() }
    val ctx = LocalContext.current
    val messages = remember {
        mutableStateListOf<Msg>().apply {
            if (real) {
                // seed `seen` with every opened-seal id so the poll never re-opens them.
                val stored = store.inbox(myTag)
                for (i in 0 until stored.length()) seen.add(stored.getJSONObject(i).optString("id"))
                // restore the durable transcript for THIS peer (both directions, in order).
                val t = store.thread(chat.identityPub)
                for (i in 0 until t.length()) {
                    val o = t.getJSONObject(i)
                    val media = o.optString("media")
                    add(
                        Msg(
                            System.nanoTime() + i, o.optString("text"), o.optBoolean("incoming"), "",
                            kind = if (media.isNotBlank()) Kind.IMAGE else Kind.TEXT,
                            state = State.OPENED,
                            mediaPath = media, mediaMime = o.optString("mime"),
                            destroyAt = o.optLong("destroy_at"),
                        )
                    )
                }
            } else {
                addAll(seedThread())
            }
        }
    }
    var draft by remember { mutableStateOf("") }
    var fastMode by remember { mutableStateOf(false) }
    // one-shot timelock for the NEXT message (unix secs, 0 = none); set via the clock in the composer.
    var revealAt by remember { mutableStateOf(0L) }
    var destroyAt by remember { mutableStateOf(0L) }
    var showTimer by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val polling = remember { java.util.concurrent.atomic.AtomicBoolean(false) }
    val nowSecs = rememberNowSeconds() // drives reveal countdowns and destroy deadlines
    // A picked photo/video waiting in the composer. It is NOT sent until the user taps send,
    // so a caption can be typed alongside it.
    var pending by remember { mutableStateOf<android.net.Uri?>(null) }

    LaunchedEffect(Unit) { if (messages.isNotEmpty()) listState.scrollToItem(messages.lastIndex) }

    // Poll my inbox: fetch delivered ciphertext, collect released shares, open, show. One at a time.
    suspend fun poll() {
        if (!real || myTag.isBlank()) return
        if (!polling.compareAndSet(false, true)) return
        try {
            val items = withContext(Dispatchers.IO) { fetchInboxAll(myTag) }
            val chainSlot = if (items.isEmpty()) 0L else withContext(Dispatchers.IO) { finalizedSlot() }
            for (item in items) {
                val sealId = item.optString("seal_id")
                val bundle = item.optJSONObject("bundle") ?: continue
                if (sealId.isBlank() || sealId in seen) continue
                val shares = withContext(Dispatchers.IO) { collectShares(sealId) }
                // If an anchor exists and contradicts this bundle, the bundle is not what was
                // committed on-chain — drop it rather than opening it.
                if (withContext(Dispatchers.IO) { verifiedAnchorSlot(sealId, bundle.toString()) } < 0) {
                    seen.add(sealId)
                    continue
                }

                // Media arrives as a manifest, not as bytes: open the manifest, pull the
                // chunks the relay is holding, then decrypt and reassemble locally.
                if (bundle.optJSONObject("signed_leaf")?.optJSONObject("leaf")?.optString("content_type") == "Media") {
                    // Time-locked? Show a locked placeholder with a countdown rather than
                    // nothing at all, and keep polling — it opens by itself at revealAt.
                    val gate = withContext(Dispatchers.IO) {
                        runCatching { JSONObject(SealCore.openMediaInfo(myDeviceSeed, bundle.toString(), shares, "", chainSlot)) }.getOrNull()
                    }
                    if (gate != null && !gate.optBoolean("ok")) {
                        when (gate.optString("reason")) {
                            "destroyed" -> { seen.add(sealId); continue } // gone before it was ever opened
                            "locked" -> {
                                val revealAt = gate.optLong("reveal_at")
                                val sender = bundle.optString("sender_id_pub")
                                if (sender == chat.identityPub && messages.none { it.lockedSealId == sealId }) {
                                    messages.add(
                                        Msg(
                                            System.nanoTime(), "Photo", true, now(), kind = Kind.IMAGE,
                                            state = State.LOCKED, revealAt = revealAt, lockedSealId = sealId,
                                        )
                                    )
                                    listState.animateScrollToItem(messages.lastIndex)
                                }
                                continue // NOT marked seen: the next poll opens it once the window does
                            }
                        }
                    }
                    val got = withContext(Dispatchers.IO) {
                        receiveMedia(ctx, myDeviceSeed, sealId, bundle.toString(), shares, chainSlot)
                    }
                    if (got != null) {
                        // replace the locked placeholder, if one is on screen
                        messages.indexOfFirst { it.lockedSealId == sealId }.takeIf { it >= 0 }?.let { messages.removeAt(it) }
                    }
                    if (got != null) {
                        seen.add(sealId)
                        val sender = bundle.optString("sender_id_pub")
                        val label = got.third.ifBlank { if (got.second.startsWith("video")) "Video" else "Photo" }
                        // carry the destroy deadline across: the recipient's copy must burn too,
                        // otherwise a "self-destruct" only ever removed the sender's side.
                        val dz = bundle.optLong("destroy_at")
                        if (store.addInbox(myTag, sealId, label, sender)) {
                            store.addThreadMsg(sender, sealId, label, true, media = got.first, mime = got.second, destroyAt = dz)
                            if (sender == chat.identityPub) {
                                messages.add(
                                    Msg(
                                        System.nanoTime(), label, true, now(), kind = Kind.IMAGE,
                                        state = State.OPENED, mediaPath = got.first, mediaMime = got.second,
                                        destroyAt = dz,
                                    )
                                )
                                listState.animateScrollToItem(messages.lastIndex)
                            }
                        }
                    }
                    continue // still locked / chunks not all there yet → retry on the next poll
                }

                val opened = runCatching {
                    withContext(Dispatchers.IO) { JSONObject(SealCore.openReceived(myDeviceSeed, bundle.toString(), shares, chainSlot)) }
                }.getOrNull()
                if (opened != null && opened.optBoolean("ok")) {
                    seen.add(sealId)
                    val text = opened.optString("plaintext")
                    val sender = bundle.optString("sender_id_pub") // sender's stable identity_pub
                    // dedup on the opened seal, persist to the sender's transcript, show if it's THIS chat.
                    if (store.addInbox(myTag, sealId, text, sender)) {
                        store.addThreadMsg(sender, sealId, text, true)
                        if (sender == chat.identityPub) {
                            messages.add(Msg(System.nanoTime(), text, true, now(), state = State.OPENED))
                            listState.animateScrollToItem(messages.lastIndex)
                        }
                    }
                } else if (opened?.optString("reason") == "destroyed") {
                    seen.add(sealId) // self-destructed before it was opened → stop retrying, never shows
                }
                // timelocked ("locked") or shares still gathering → leave unseen; it opens when the window does
            }
        } finally {
            polling.set(false)
        }
    }

    // live receive: poll every few seconds while the conversation is open
    LaunchedEffect(myTag, real) {
        if (real && myTag.isNotBlank()) while (true) { poll(); delay(3000) }
    }

    /**
     * Seal and ship a picked photo/video. The readable file is copied into our cache only so
     * Rust can chunk-encrypt it; ONLY the encrypted chunks are uploaded, and they go to the
     * relay addressed by ciphertext hash. The sender's own copy stays local for the bubble.
     */
    fun sendMedia(uri: android.net.Uri, caption: String) {
        if (!real) {
            android.widget.Toast.makeText(ctx, "Add this person as a contact first", android.widget.Toast.LENGTH_SHORT).show()
            return
        }
        val mime = ctx.contentResolver.getType(uri) ?: "image/jpeg"
        val isVideo = mime.startsWith("video")
        val label = caption.ifBlank { if (isVideo) "Video" else "Photo" }
        val fast = fastMode
        val rv = revealAt; val dz = destroyAt
        messages.add(
            Msg(
                System.nanoTime(), label, false, now(), kind = Kind.IMAGE, state = State.SEALING,
                mode = if (fast) "FAST" else "STRICT", revealAt = rv, destroyAt = dz,
            )
        )
        val idx = messages.lastIndex
        revealAt = 0L; destroyAt = 0L
        scope.launch {
            listState.animateScrollToItem(messages.lastIndex)
            val result = withContext(Dispatchers.IO) {
                val src = cacheFromUri(ctx, uri, "pick-${System.nanoTime()}") ?: return@withContext null
                val preview = buildPreview(ctx, src, isVideo)
                val chunkDir = java.io.File(ctx.cacheDir, "out-${System.nanoTime()}")
                val slot = if (rv > 0) finalizedSlot() else 0L // only needed for a reveal floor
                val sealed = runCatching {
                    JSONObject(
                        SealCore.sealMediaFile(
                            myIdentitySeed, myCard, chat.devicePub, src.path, mime,
                            if (isVideo) "video" else "image", caption,
                            preview?.path ?: "", chunkDir.path, fast, rv, dz, slot,
                        )
                    )
                }.getOrNull()
                preview?.delete()
                if (sealed == null || !sealed.optBoolean("ok")) {
                    chunkDir.deleteRecursively(); src.delete()
                    return@withContext null
                }
                // upload the opaque chunks, then the manifest bundle + key shares
                val up = uploadChunks(sealed)
                chunkDir.deleteRecursively()
                if (!up) { src.delete(); return@withContext null }
                if (!shipSeal(sealed)) { src.delete(); return@withContext null }
                // keep OUR readable copy locally so the sent bubble can render it
                val mediaDir = java.io.File(ctx.filesDir, "media").apply { mkdirs() }
                val mine = java.io.File(mediaDir, "sent-${sealed.optString("seal_id")}.${if (isVideo) "mp4" else "jpg"}")
                src.copyTo(mine, overwrite = true); src.delete()
                mine.path
            }
            if (result != null) {
                messages[idx] = messages[idx].copy(state = State.OPENED, sealedFor = chat.name, mediaPath = result, mediaMime = mime)
                store.addThreadMsg(chat.identityPub, "out-" + System.nanoTime(), label, false, media = result, mime = mime, destroyAt = dz)
            } else {
                messages.removeAt(idx)
                android.widget.Toast.makeText(ctx, "Couldn't send $label — check the server", android.widget.Toast.LENGTH_SHORT).show()
            }
        }
    }

    val pickMedia = rememberMediaPicker { uri -> pending = uri } // stage it; the user captions + sends

    fun send() {
        // An attached photo/video takes the typed text as its (sealed) caption, so the two
        // travel as ONE item rather than a picture followed by a stray message.
        pending?.let { uri ->
            val caption = draft.trim()
            draft = ""
            pending = null
            sendMedia(uri, caption)
            return
        }
        val text = draft.trim()
        if (text.isEmpty()) return
        val fast = fastMode
        val rv = revealAt; val dz = destroyAt // capture the one-shot timelock for THIS message
        messages.add(Msg(System.nanoTime(), text, false, now(), state = State.SEALING, mode = if (fast) "FAST" else "STRICT", revealAt = rv, destroyAt = dz))
        // persist the outgoing message immediately so it survives leaving/reopening the chat.
        if (real) store.addThreadMsg(chat.identityPub, "out-" + System.nanoTime(), text, false)
        val idx = messages.lastIndex
        draft = ""; revealAt = 0L; destroyAt = 0L // reset the timelock after each send
        scope.launch {
            listState.animateScrollToItem(messages.lastIndex)
            if (real) {
                // seal + SHIP over the relay + gateways; the recipient's device polls + opens.
                // `ok` now means the RELAY actually accepted the ciphertext (real delivery signal).
                val ok = runCatching {
                    withContext(Dispatchers.IO) {
                        val slot = if (rv > 0) finalizedSlot() else 0L // only needed for a reveal floor
                        val ship = JSONObject(SealCore.sealShippable(myIdentitySeed, myCard, chat.devicePub, text, fast, rv, dz, slot))
                        if (!ship.optBoolean("ok")) return@withContext false
                        shipSeal(ship)
                    }
                }.getOrDefault(false)
                delay(if (fast) 400 else 800)
                messages[idx] = messages[idx].copy(state = if (ok) State.OPENED else State.SEALING, sealedFor = chat.name)
                if (!ok) android.widget.Toast.makeText(ctx, "Couldn't reach the server — message not sent", android.widget.Toast.LENGTH_SHORT).show()
                poll() // pick up a self-loopback / any pending inbound right away
            } else {
                val transcript = runCatching {
                    withContext(Dispatchers.Default) { if (fast) SealCore.runFastDemo() else SealCore.runDemo() }
                }.getOrNull()
                delay(if (fast) 550 else 1150)
                val opened = transcript?.let { runCatching { JSONObject(it).getJSONArray("transcript").let { a -> a.toString().contains("OPENED") } }.getOrDefault(true) } ?: true
                messages[idx] = messages[idx].copy(state = if (opened) State.OPENED else State.SEALING)
            }
        }
    }

    Column(Modifier.fillMaxSize().background(ConvBg)) {
        // top bar (white)
        Row(
            Modifier.fillMaxWidth().background(Color.White).padding(horizontal = 8.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) { Icon(Icons.Filled.ArrowBackIosNew, "Back", tint = Blue, modifier = Modifier.size(22.dp)) }
            Avatar(chat.name, 40.dp)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                Text(chat.name, color = Ink, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Text("Online", color = Green, fontSize = 12.sp)
            }
            // subtle seal-mode toggle
            IconButton(onClick = { fastMode = !fastMode }) {
                Icon(
                    if (fastMode) Icons.Filled.Bolt else Icons.Filled.Lock,
                    if (fastMode) "FastSeal" else "StrictSeal",
                    tint = Blue, modifier = Modifier.size(21.dp),
                )
            }
            IconButton(onClick = {}) { Icon(Icons.Filled.Call, "Call", tint = Blue, modifier = Modifier.size(22.dp)) }
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Hair))

        // messages
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(horizontal = 12.dp, vertical = 10.dp),
        ) {
            item { DateChip("Today") }
            items(messages, key = { it.id }) { m ->
                Bubble(m, nowSecs) {
                    // burn finished — drop it from the transcript for good
                    messages.remove(m)
                    if (real) store.removeThreadMedia(chat.identityPub, m.mediaPath)
                    if (m.mediaPath.isNotBlank()) runCatching { java.io.File(m.mediaPath).delete() }
                }
            }
        }

        val timerLabel = when {
            revealAt > 0 -> "🔒 Opens in ${relLabel(revealAt)}"
            destroyAt > 0 -> "💥 Destroys in ${relLabel(destroyAt)}"
            else -> null
        }
        Composer(
            draft, { draft = it }, timerLabel, { showTimer = true }, { revealAt = 0L; destroyAt = 0L },
            onAttach = { pickMedia.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo)) },
            onSend = ::send,
            attachment = pending,
            onClearAttachment = { pending = null },
        )
    }

    if (showTimer) TimedSealDialog(
        onDismiss = { showTimer = false },
        onPick = { r, d -> revealAt = r; destroyAt = d; showTimer = false },
    )
}

/** Photo/video picker. `PickVisualMedia` needs no storage permission. */
@Composable
private fun rememberMediaPicker(onPicked: (android.net.Uri) -> Unit) =
    rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri -> uri?.let(onPicked) }

@Composable
@OptIn(ExperimentalMaterial3Api::class)
private fun TimedSealDialog(onDismiss: () -> Unit, onPick: (Long, Long) -> Unit) {
    var mode by remember { mutableStateOf(0) } // 0 = timelock (opens later), 1 = self-destruct
    var page by remember { mutableStateOf(0) }  // 0 = quick presets, 1 = calendar, 2 = clock
    val presets = listOf(
        "1 min" to 60L, "10 min" to 600L, "1 hour" to 3600L,
        "1 day" to 86400L, "3 days" to 259200L, "1 week" to 604800L,
    )
    val zone = remember { java.util.TimeZone.getDefault() }
    val datePickerState = rememberDatePickerState(
        initialSelectedDateMillis = System.currentTimeMillis(),
        // A seal can only be set in the FUTURE, so past days are not selectable at all.
        selectableDates = object : SelectableDates {
            override fun isSelectableDate(utcTimeMillis: Long): Boolean {
                val todayUtc = System.currentTimeMillis() - zone.getOffset(System.currentTimeMillis())
                return utcTimeMillis >= todayUtc - 86_400_000L
            }
        },
    )
    val timePickerState = rememberTimePickerState(is24Hour = false)

    /** Combine the picked day + time into a local unix timestamp. */
    fun chosenEpoch(): Long {
        val dayUtc = datePickerState.selectedDateMillis ?: return 0L
        // DatePicker hands back UTC midnight; rebuild the day in the LOCAL calendar so
        // "3pm" means 3pm where the user is, not 3pm UTC.
        val cal = java.util.Calendar.getInstance(java.util.TimeZone.getTimeZone("UTC"))
        cal.timeInMillis = dayUtc
        val local = java.util.Calendar.getInstance()
        local.set(
            cal.get(java.util.Calendar.YEAR), cal.get(java.util.Calendar.MONTH), cal.get(java.util.Calendar.DAY_OF_MONTH),
            timePickerState.hour, timePickerState.minute, 0,
        )
        local.set(java.util.Calendar.MILLISECOND, 0)
        return local.timeInMillis / 1000
    }

    fun commit(epoch: Long) {
        if (epoch <= System.currentTimeMillis() / 1000) return // never accept a past instant
        if (mode == 0) onPick(epoch, 0L) else onPick(0L, epoch)
    }

    // The calendar gets Material3's own DatePickerDialog: a DatePicker embedded in a plain
    // AlertDialog is squeezed to the alert's width and clips the last weekday column.
    if (page == 1) {
        DatePickerDialog(
            onDismissRequest = onDismiss,
            confirmButton = {
                TextButton(onClick = { page = 2 }) { Text("Next", color = Blue, fontWeight = FontWeight.SemiBold) }
            },
            dismissButton = { TextButton(onClick = { page = 0 }) { Text("Back", color = Sub) } },
        ) {
            DatePicker(state = datePickerState, showModeToggle = true)
        }
        return
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (page == 0) "Timed seal" else "Pick a time") },
        text = {
            // animateContentSize keeps the dialog from snapping between pages
            Column(Modifier.animateContentSize()) {
                if (page == 0) {
                    Text(
                        "The window is signed into the seal and anchored on-chain, and the gateways " +
                            "withhold the key outside it — so it can't be moved, and a patched app has " +
                            "nothing to open.",
                        color = Sub, fontSize = 12.sp,
                    )
                    Spacer(Modifier.height(14.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        ModeChip("🔒 Opens later", mode == 0) { mode = 0 }
                        ModeChip("💥 Self-destruct", mode == 1) { mode = 1 }
                    }
                    Spacer(Modifier.height(16.dp))
                    Text(
                        if (mode == 0) "Opens after" else "Destroys after",
                        color = Ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                    )
                    Spacer(Modifier.height(8.dp))
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        presets.chunked(2).forEach { row ->
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                row.forEach { (label, secs) ->
                                    Box(
                                        Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(Color(0xFFF1F4F7))
                                            .clickable { commit(System.currentTimeMillis() / 1000 + secs) }
                                            .padding(vertical = 12.dp),
                                        contentAlignment = Alignment.Center,
                                    ) { Text(label, color = Ink, fontSize = 14.sp) }
                                }
                            }
                        }
                    }
                    Spacer(Modifier.height(12.dp))
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
                            .border(1.dp, Blue, RoundedCornerShape(10.dp))
                            .clickable { page = 1 }.padding(vertical = 12.dp),
                        horizontalArrangement = Arrangement.Center,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Icon(Icons.Filled.CalendarMonth, null, tint = Blue, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Pick exact date & time", color = Blue, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    }
                } else {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
                        TimePicker(state = timePickerState)
                        Spacer(Modifier.height(6.dp))
                        val e = chosenEpoch()
                        if (e > 0) {
                            val fmt = SimpleDateFormat("EEE d MMM yyyy · h:mm a", Locale.getDefault())
                            Text(
                                (if (mode == 0) "Opens " else "Destroys ") + fmt.format(Date(e * 1000)),
                                color = if (e > System.currentTimeMillis() / 1000) Ink else Color(0xFFE0403A),
                                fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                            )
                            if (e <= System.currentTimeMillis() / 1000) {
                                Text("Pick a time in the future", color = Color(0xFFE0403A), fontSize = 11.sp)
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            if (page == 2) {
                TextButton(
                    onClick = { commit(chosenEpoch()) },
                    enabled = chosenEpoch() > System.currentTimeMillis() / 1000,
                ) { Text("Set", color = Blue, fontWeight = FontWeight.SemiBold) }
            }
        },
        dismissButton = {
            TextButton(onClick = { if (page == 0) onDismiss() else page = 1 }) {
                Text(if (page == 0) "Cancel" else "Back", color = Sub)
            }
        },
    )
}

@Composable
private fun ModeChip(label: String, active: Boolean, onClick: () -> Unit) {
    Box(
        Modifier.clip(RoundedCornerShape(20.dp)).background(if (active) Blue else Color(0xFFF1F4F7))
            .clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 8.dp),
    ) { Text(label, color = if (active) Color.White else Ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
private fun DateChip(text: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), horizontalArrangement = Arrangement.Center) {
        Box(
            Modifier.background(Color.White, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 4.dp),
        ) { Text(text, color = Sub, fontSize = 12.sp) }
    }
}

/** Wall-clock seconds, ticking once a second, for countdowns and destroy deadlines. */
@Composable
private fun rememberNowSeconds(): Long {
    var now by remember { mutableStateOf(System.currentTimeMillis() / 1000) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            now = System.currentTimeMillis() / 1000
        }
    }
    return now
}

/** mm:ss for a countdown; falls back to h/d for long windows. */
private fun countdown(secondsLeft: Long): String {
    val s = secondsLeft.coerceAtLeast(0)
    return when {
        s >= 86400 -> "${s / 86400}d ${(s % 86400) / 3600}h"
        s >= 3600 -> "${s / 3600}h ${(s % 3600) / 60}m"
        else -> "%d:%02d".format(s / 60, s % 60)
    }
}

/**
 * Burns the content away: it shrinks and fades while puffs of smoke rise off it, then
 * `onFinished` fires so the caller can drop the message for good.
 */
@Composable
private fun SmokeDestroy(onFinished: () -> Unit, content: @Composable () -> Unit) {
    val p = remember { androidx.compose.animation.core.Animatable(0f) }
    LaunchedEffect(Unit) {
        p.animateTo(1f, androidx.compose.animation.core.tween(1500, easing = androidx.compose.animation.core.LinearEasing))
        onFinished()
    }
    Box {
        Box(
            Modifier.graphicsLayer {
                alpha = (1f - p.value * 1.35f).coerceIn(0f, 1f)
                val shrink = 1f - 0.14f * p.value
                scaleX = shrink; scaleY = shrink
            }
        ) { content() }

        // puffs: deterministic per-index so they don't jitter between recompositions
        Canvas(Modifier.matchParentSize()) {
            val t = p.value
            for (i in 0 until 14) {
                val seed = i * 7919
                val fx = ((seed % 100) / 100f)                 // horizontal position 0..1
                val drift = (((seed / 100) % 100) / 100f - 0.5f) // sideways drift
                val delay = (i % 5) * 0.08f
                val local = ((t - delay) / (1f - delay)).coerceIn(0f, 1f)
                if (local <= 0f) continue
                val r = size.minDimension * (0.06f + 0.16f * local)
                val cx = size.width * fx + drift * size.width * 0.35f * local
                val cy = size.height * (0.85f - 1.05f * local)
                drawCircle(
                    color = Color(0xFF9AA5B1).copy(alpha = (0.42f * (1f - local)).coerceAtLeast(0f)),
                    radius = r,
                    center = androidx.compose.ui.geometry.Offset(cx, cy),
                )
            }
        }
    }
}

/** The locked card: a lock and a live countdown, and deliberately NO hint of the content —
 *  the preview is sealed inside the manifest, which cannot be opened before the window. */
@Composable
private fun LockedContent(m: Msg, now: Long, caption: String = "") {
    Column(horizontalAlignment = Alignment.Start) {
        Column(
            Modifier.size(width = 220.dp, height = 160.dp).clip(RoundedCornerShape(12.dp))
                .background(Color(0xFFE3E9EF)),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(Icons.Filled.Lock, null, tint = Sub, modifier = Modifier.size(30.dp))
            Spacer(Modifier.height(8.dp))
            Text(countdown(m.revealAt - now), color = Ink, fontSize = 26.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(4.dp))
            Text("Photo · opens in", color = Sub, fontSize = 12.sp)
        }
        // the sender's own caption stays readable — they wrote it; only the picture is sealed
        if (caption.isNotBlank() && caption != "Photo" && caption != "Video") {
            Spacer(Modifier.height(6.dp))
            Text(caption, color = Ink, fontSize = 15.sp, modifier = Modifier.widthIn(max = 220.dp))
        }
    }
}

@Composable
private fun Bubble(m: Msg, now: Long = 0L, onDestroyed: () -> Unit = {}) {
    val align = if (m.incoming) Alignment.Start else Alignment.End
    val bg = if (m.incoming) Incoming else Outgoing
    val shape = RoundedCornerShape(
        topStart = 18.dp, topEnd = 18.dp,
        bottomStart = if (m.incoming) 4.dp else 18.dp,
        bottomEnd = if (m.incoming) 18.dp else 4.dp,
    )
    // Self-destruct: the countdown is the SENDER's to watch, but the burn plays on both sides.
    val destroying = m.destroyAt > 0 && now > 0 && now >= m.destroyAt
    val showDestroyClock = !m.incoming && m.destroyAt > 0 && now > 0 && !destroying
    // The SENDER's own copy of an "opens later" item: they own the picture, but it must not
    // look like an ordinary sent photo — it is sealed shut for the recipient until revealAt.
    val stillSealed = !m.incoming && m.revealAt > 0 && now > 0 && now < m.revealAt

    Column(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalAlignment = align) {
        val body: @Composable () -> Unit = {
            Box(Modifier.widthIn(max = 300.dp).clip(shape).background(bg).padding(10.dp)) {
                when {
                    m.state == State.SEALING -> SealingContent(m)
                    m.state == State.LOCKED -> LockedContent(m, now)
                    // Sealed and not yet open: show the lock card on the SENDER's side too.
                    // A translucent scrim still let the picture read straight through it.
                    stillSealed && m.kind == Kind.IMAGE -> LockedContent(m, now, caption = m.text)
                    m.kind == Kind.IMAGE -> ImageContent(m)
                    m.kind == Kind.VOICE -> VoiceContent(m)
                    else -> TextContent(m)
                }
            }
        }
        if (destroying) SmokeDestroy(onFinished = onDestroyed, content = body) else body()

        if (m.state == State.SEALING) {
            Row(Modifier.padding(top = 3.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.Autorenew, null, tint = Sub, modifier = Modifier.size(12.dp))
                Spacer(Modifier.width(4.dp))
                Text(if (m.mode == "FAST") "Pre-confirming…" else "Sealing…", color = Sub, fontSize = 11.sp)
            }
        }
        if (stillSealed) {
            Row(Modifier.padding(top = 3.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.Lock, null, tint = Blue, modifier = Modifier.size(12.dp))
                Spacer(Modifier.width(4.dp))
                Text("Sealed · opens in ${countdown(m.revealAt - now)}", color = Blue, fontSize = 11.sp)
            }
        }
        if (showDestroyClock) {
            Row(Modifier.padding(top = 3.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.Whatshot, null, tint = Color(0xFFE0703A), modifier = Modifier.size(12.dp))
                Spacer(Modifier.width(4.dp))
                Text("Destroys in ${countdown(m.destroyAt - now)}", color = Color(0xFFE0703A), fontSize = 11.sp)
            }
        }
    }
}

@Composable
private fun TextContent(m: Msg) {
    Column {
        Text(m.text, color = Ink, fontSize = 15.sp)
        MetaRow(m)
    }
}

@Composable
private fun SealingContent(m: Msg) {
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Lock, null, tint = Sub, modifier = Modifier.size(15.dp))
            Spacer(Modifier.width(8.dp))
            Text(
                if (m.mode == "FAST") "Waiting for pre-confirms" else "Waiting for finality",
                color = Sub, fontSize = 14.sp,
            )
        }
        Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.End) {
            Text(m.time, color = Sub, fontSize = 11.sp)
        }
    }
}

@Composable
private fun ImageContent(m: Msg) {
    val ctx = LocalContext.current
    val isVideo = m.mediaMime.startsWith("video")
    // Decode the DECRYPTED local file. Nothing here ever touches the network — by this
    // point the bytes exist in readable form only on this device.
    val bmp = remember(m.mediaPath) {
        if (m.mediaPath.isBlank()) null
        else runCatching {
            if (isVideo) {
                android.media.MediaMetadataRetriever().use { it.setDataSource(m.mediaPath); it.getFrameAtTime(0) }
            } else {
                android.graphics.BitmapFactory.decodeFile(m.mediaPath, android.graphics.BitmapFactory.Options().apply { inSampleSize = 2 })
            }
        }.getOrNull()
    }
    Column {
        Box(
            Modifier.size(width = 220.dp, height = 160.dp).clip(RoundedCornerShape(12.dp))
                .background(Color(0xFFDDE4EA))
                .then(if (isVideo && m.mediaPath.isNotBlank()) Modifier.clickable { openExternally(ctx, m.mediaPath, m.mediaMime) } else Modifier),
            contentAlignment = Alignment.Center,
        ) {
            if (bmp != null) {
                Image(
                    bitmap = bmp.asImageBitmap(),
                    contentDescription = if (isVideo) "Video" else "Photo",
                    modifier = Modifier.fillMaxSize(),
                    contentScale = androidx.compose.ui.layout.ContentScale.Crop,
                )
            } else {
                // no decrypted file yet (placeholder / demo threads)
                Box(
                    Modifier.fillMaxSize()
                        .background(Brush.verticalGradient(listOf(Color(0xFFF6B26B), Color(0xFF6FA8DC), Color(0xFF2E5B8A))))
                )
            }
            if (isVideo) {
                Box(Modifier.size(46.dp).background(Color(0x99000000), CircleShape), contentAlignment = Alignment.Center) {
                    Icon(Icons.Filled.PlayArrow, "Play", tint = Color.White, modifier = Modifier.size(30.dp))
                }
            }
            Row(Modifier.align(Alignment.BottomEnd).padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(m.time, color = Color.White, fontSize = 11.sp)
                Spacer(Modifier.width(3.dp))
                SealBadge()
            }
        }
        // the caption travelled sealed inside the manifest, so it is as private as the pixels
        val caption = m.text
        if (caption.isNotBlank() && caption != "Photo" && caption != "Video") {
            Spacer(Modifier.height(6.dp))
            Text(caption, color = Ink, fontSize = 15.sp, modifier = Modifier.widthIn(max = 220.dp))
        }
    }
}

/** Hand a decrypted file to a player/viewer via a FileProvider-free content-less intent. */
private fun openExternally(ctx: Context, path: String, mime: String) {
    runCatching {
        val f = java.io.File(path)
        val uri = androidx.core.content.FileProvider.getUriForFile(ctx, "${ctx.packageName}.files", f)
        ctx.startActivity(
            android.content.Intent(android.content.Intent.ACTION_VIEW).apply {
                setDataAndType(uri, mime)
                addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        )
    }
}

@Composable
private fun VoiceContent(m: Msg) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.Filled.PlayArrow, null, tint = Blue, modifier = Modifier.size(26.dp))
        Spacer(Modifier.width(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            listOf(8, 16, 11, 20, 13, 22, 9, 17, 12, 7, 15, 10).forEach { h ->
                Box(Modifier.padding(horizontal = 1.dp).size(width = 3.dp, height = h.dp).background(Blue, RoundedCornerShape(2.dp)))
            }
        }
        Spacer(Modifier.width(8.dp))
        Text("0:18", color = Sub, fontSize = 12.sp)
        Spacer(Modifier.width(8.dp))
        Text(m.time, color = Sub, fontSize = 11.sp)
        Spacer(Modifier.width(3.dp))
        SealBadge()
    }
}

@Composable
private fun MetaRow(m: Msg) {
    Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
        if (m.revealAt > 0) {
            Icon(Icons.Filled.Schedule, null, tint = Blue, modifier = Modifier.size(11.dp))
            Spacer(Modifier.width(3.dp))
            Text("Opens in ${relLabel(m.revealAt)}", color = Blue, fontSize = 10.sp)
            Spacer(Modifier.width(6.dp))
        } else if (m.destroyAt > 0) {
            Text("💥 ${relLabel(m.destroyAt)}", color = Color(0xFFE0403A), fontSize = 10.sp)
            Spacer(Modifier.width(6.dp))
        }
        m.sealedFor?.let {
            Icon(Icons.Filled.Lock, null, tint = Blue, modifier = Modifier.size(11.dp))
            Spacer(Modifier.width(3.dp))
            Text("Sealed for $it", color = Blue, fontSize = 10.sp)
            Spacer(Modifier.width(6.dp))
        }
        Text(m.time, color = Sub, fontSize = 11.sp)
        if (!m.incoming && m.sealedFor == null) {
            Spacer(Modifier.width(4.dp))
            if (m.state == State.OPENED) {
                SealBadge()
            } else {
                Icon(Icons.Filled.DoneAll, null, tint = if (m.read) Blue else Sub, modifier = Modifier.size(15.dp))
            }
        }
    }
}

@Composable
private fun SealBadge() {
    Box(Modifier.size(16.dp).background(Green, CircleShape), contentAlignment = Alignment.Center) {
        Icon(Icons.Filled.Check, "Sealed", tint = Color.White, modifier = Modifier.size(11.dp))
    }
}

@Composable
private fun Composer(
    value: String,
    onChange: (String) -> Unit,
    timerLabel: String?,
    onClock: () -> Unit,
    onClearTimer: () -> Unit,
    onAttach: () -> Unit,
    onSend: () -> Unit,
    attachment: android.net.Uri? = null,
    onClearAttachment: () -> Unit = {},
) {
  val ctx = LocalContext.current
  Column(Modifier.fillMaxWidth().background(Color.White)) {
    // A staged photo/video slides up into the composer and waits there. Whatever is typed
    // becomes its caption, sealed inside the manifest, and both go in ONE item.
    AnimatedVisibility(
        visible = attachment != null,
        enter = slideInVertically { it / 2 } + fadeIn() + expandVertically(),
        exit = slideOutVertically { it / 2 } + fadeOut() + shrinkVertically(),
    ) {
        val uri = attachment
        val thumb = remember(uri) {
            uri?.let {
                runCatching {
                    val mime = ctx.contentResolver.getType(it).orEmpty()
                    if (mime.startsWith("video")) {
                        android.media.MediaMetadataRetriever().use { r ->
                            ctx.contentResolver.openFileDescriptor(it, "r")?.use { fd ->
                                r.setDataSource(fd.fileDescriptor); r.getFrameAtTime(0)
                            }
                        }
                    } else {
                        ctx.contentResolver.openInputStream(it)?.use { ins ->
                            android.graphics.BitmapFactory.decodeStream(
                                ins, null,
                                android.graphics.BitmapFactory.Options().apply { inSampleSize = 4 },
                            )
                        }
                    }
                }.getOrNull()
            }
        }
        Row(
            Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box {
                Box(Modifier.size(64.dp).clip(RoundedCornerShape(10.dp)).background(Hair)) {
                    thumb?.let {
                        Image(
                            bitmap = it.asImageBitmap(), contentDescription = "Attached",
                            modifier = Modifier.fillMaxSize(),
                            contentScale = androidx.compose.ui.layout.ContentScale.Crop,
                        )
                    }
                }
                Box(
                    Modifier.align(Alignment.TopEnd).offset(x = 6.dp, y = (-6).dp)
                        .size(22.dp).clip(CircleShape).background(Ink.copy(alpha = 0.75f))
                        .clickable(onClick = onClearAttachment),
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Filled.Close, "Remove", tint = Color.White, modifier = Modifier.size(14.dp)) }
            }
            Spacer(Modifier.width(12.dp))
            Column {
                Text("Ready to seal", color = Ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                Text("Add a caption, then send", color = Sub, fontSize = 11.sp)
            }
        }
    }
    if (timerLabel != null) {
        Row(
            Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.clip(RoundedCornerShape(12.dp)).background(Color(0xFFEAF3FF)).padding(horizontal = 10.dp, vertical = 5.dp),
            ) { Text(timerLabel, color = Blue, fontSize = 12.sp, fontWeight = FontWeight.SemiBold) }
            Spacer(Modifier.width(6.dp))
            Icon(Icons.Filled.Close, "Clear timer", tint = Sub, modifier = Modifier.size(16.dp).clickable(onClick = onClearTimer))
        }
    }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onClock) {
            Icon(Icons.Filled.Schedule, "Timed seal", tint = if (timerLabel != null) Blue else Sub, modifier = Modifier.size(23.dp))
        }
        IconButton(onClick = onAttach) {
            Icon(Icons.Filled.PhotoCamera, "Send photo or video", tint = Sub, modifier = Modifier.size(23.dp))
        }
        OutlinedTextField(
            value = value,
            onValueChange = onChange,
            placeholder = {
                Text(if (attachment != null) "Add a caption…" else "Message", color = Sub, fontSize = 15.sp)
            },
            leadingIcon = { Icon(Icons.Outlined.EmojiEmotions, null, tint = Sub, modifier = Modifier.size(22.dp)) },
            maxLines = 5,
            shape = RoundedCornerShape(24.dp),
            colors = OutlinedTextFieldDefaults.colors(
                focusedContainerColor = Color(0xFFF1F4F7),
                unfocusedContainerColor = Color(0xFFF1F4F7),
                focusedBorderColor = Color.Transparent,
                unfocusedBorderColor = Color.Transparent,
                cursorColor = Blue,
                focusedTextColor = Ink,
                unfocusedTextColor = Ink,
            ),
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.width(8.dp))
        val active = value.isNotBlank() || attachment != null
        Box(
            Modifier.size(46.dp).background(Blue, CircleShape).clickable(enabled = active, onClick = onSend),
            contentAlignment = Alignment.Center,
        ) {
            Icon(if (active) Icons.Filled.Send else Icons.Filled.Mic, "Send", tint = Color.White, modifier = Modifier.size(22.dp))
        }
    }
  }
}

// ─────────────────────────────── avatar ─────────────────────────────────────
@Composable
private fun Avatar(name: String, size: androidx.compose.ui.unit.Dp) {
    Box(
        Modifier.size(size).background(avatarColor(name), CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Text(initials(name), color = Color.White, fontSize = (size.value / 2.6f).sp, fontWeight = FontWeight.SemiBold)
    }
}
