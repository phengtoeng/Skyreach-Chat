package com.denvion.splitseal

import android.app.Activity
import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
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
private val AvatarColors = listOf(
    Color(0xFF5B8DEF), Color(0xFFEF6F6C), Color(0xFF3FBF8F), Color(0xFFF0A84A),
    Color(0xFF9B7EDE), Color(0xFF48B0C7), Color(0xFFE07AAE),
)

private fun now(): String = SimpleDateFormat("h:mm a", Locale.US).format(Date())
private fun avatarColor(name: String) = AvatarColors[(name.hashCode() and 0x7fffffff) % AvatarColors.size]
private fun initials(name: String) =
    name.trim().split(" ").filter { it.isNotEmpty() }.take(2).joinToString("") { it.first().uppercase() }

// ─────────────────────────────── models ─────────────────────────────────────
private data class Chat(val name: String, val last: String, val time: String, val unread: Int = 0, val devicePub: String = "", val identityPub: String = "")

private enum class Kind { TEXT, IMAGE, VOICE }
private enum class State { PLAIN, SEALING, OPENED }

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
}

private fun loadOrCreateIdentity(store: Store): JSONObject {
    store.identity()?.let { return it }
    val id = JSONObject(SealCore.newIdentity("Me"))
    store.saveIdentity(id)
    return id
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
    val directory: String get() = "http://$host:9988"
    val relay: String get() = "http://$host:9200"
    val gateways: List<String> get() = listOf(9201, 9202, 9203).map { "http://$host:$it" }
}

private fun directoryLookup(phone: String): JSONObject? {
    return try {
        val commit = JSONObject(SealCore.phoneCommitment(phone)).optString("phone_commitment")
        if (commit.isEmpty()) return null
        val conn = (java.net.URL("${Server.directory}/lookup/$commit").openConnection() as java.net.HttpURLConnection).apply {
            connectTimeout = 4000; readTimeout = 4000
        }
        if (conn.responseCode == 200) {
            val card = JSONObject(conn.inputStream.bufferedReader().use { it.readText() }).optString("card")
            val res = JSONObject(SealCore.parseCard(card))
            if (res.optBoolean("ok")) res else null
        } else null
    } catch (e: Exception) {
        null
    }
}

private fun directoryPublish(phone: String, cardCode: String): Boolean {
    return try {
        val commit = JSONObject(SealCore.phoneCommitment(phone)).optString("phone_commitment")
        val conn = (java.net.URL("${Server.directory}/register").openConnection() as java.net.HttpURLConnection).apply {
            requestMethod = "POST"; doOutput = true; connectTimeout = 4000; readTimeout = 4000
            setRequestProperty("Content-Type", "application/json")
        }
        conn.outputStream.use { it.write(JSONObject().put("commitment", commit).put("card", cardCode).toString().toByteArray()) }
        conn.responseCode == 200
    } catch (e: Exception) {
        false
    }
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

/** Ship a shippable seal: {seal_id,bundle} → relay inbox, each share → a gateway (+ finalize). */
private fun shipSeal(ship: JSONObject) {
    val tag = ship.optString("mailbox_tag")
    val sealId = ship.optString("seal_id")
    // carry seal_id alongside the ciphertext so the recipient (who has neither) can collect shares.
    val item = JSONObject().put("seal_id", sealId).put("bundle", ship.getJSONObject("bundle"))
    httpPost("${Server.relay}/inbox/$tag", item.toString())
    val shares = ship.getJSONArray("shares")
    val gateways = Server.gateways
    for (i in 0 until minOf(shares.length(), gateways.size)) {
        httpPost("${gateways[i]}/deposit", shares.getJSONObject(i).toString())
        httpPost("${gateways[i]}/finalize/$sealId", "")
    }
}

/** Fetch every {seal_id,bundle} item delivered to a mailbox tag (recipient polls this). */
private fun fetchInboxAll(tag: String): List<JSONObject> {
    val body = httpGet("${Server.relay}/inbox/$tag") ?: return emptyList()
    val arr = JSONArray(body)
    return (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
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
        val proto = runCatching {
            JSONObject(SealCore.version()).let { "DSCP-${it.getInt("version")} · chain ${it.getLong("chain_id")}" }
        }.getOrDefault("DSCP-2")
        setContent { MaterialTheme(colorScheme = lightColorScheme(primary = Blue)) { App(proto) } }
    }
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
    val identity = remember { loadOrCreateIdentity(store) }
    var contacts by remember { mutableStateOf(loadContacts(store)) }
    var openChat by remember { mutableStateOf<Chat?>(null) }
    var tab by remember { mutableStateOf(0) }
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
                val res = JSONObject(SealCore.parseCard(c))
                if (res.optBoolean("ok")) scanned = res
                else android.widget.Toast.makeText(ctx, res.optString("error", "invalid contact code"), android.widget.Toast.LENGTH_SHORT).show()
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
                showNew = false; scanned = null
            },
        )
        tab == 2 -> ProfileScreen(
            identity, tab,
            currentHost = Server.host,
            onTab = { tab = it },
            onPublish = { publishPhone(it) },
            onSaveHost = { h ->
                Server.host = h
                store.saveServerHost(h)
                android.widget.Toast.makeText(ctx, "Server set to $h", android.widget.Toast.LENGTH_SHORT).show()
            },
        )
        else -> ChatListScreen(
            contacts = contacts,
            onOpen = { openChat = it },
            onAdd = { showNew = true; scanned = null },
            tab = tab,
            onTab = { tab = it },
        )
    }
}

// ─────────────────────────────── chat list ──────────────────────────────────
@Composable
private fun ChatListScreen(
    contacts: List<Contact>,
    onOpen: (Chat) -> Unit,
    onAdd: () -> Unit,
    tab: Int,
    onTab: (Int) -> Unit,
) {
    SystemBars(Blue, darkIcons = false)
    // real contacts first (address / phone as subtitle), then the demo threads
    val rows = contacts.map {
        val sub = when {
            it.address.isNotBlank() -> it.address.take(14) + "… · tap to seal"
            it.phone.isNotBlank() -> "+855 " + it.phone
            else -> "tap to seal"
        }
        Chat(it.name, sub, "", devicePub = it.devicePub, identityPub = it.identityPub)
    } + CHATS
    Column(Modifier.fillMaxSize().background(ScreenBg)) {
        Row(
            Modifier.fillMaxWidth().background(Blue).padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Shield, null, tint = Color.White, modifier = Modifier.size(24.dp))
            Spacer(Modifier.width(8.dp))
            Text("Denvion", color = Color.White, fontSize = 21.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            Icon(Icons.Filled.Search, "Search", tint = Color.White, modifier = Modifier.size(24.dp))
        }
        Box(Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize()) {
                items(rows) { c ->
                    ChatRow(c) { onOpen(c) }
                    Box(Modifier.padding(start = 84.dp).fillMaxWidth().height(1.dp).background(Hair))
                }
            }
            FloatingActionButton(
                onClick = onAdd,
                containerColor = Blue,
                contentColor = Color.White,
                modifier = Modifier.align(Alignment.BottomEnd).padding(20.dp),
            ) { Icon(Icons.Filled.PersonAdd, "Add contact") }
        }
        BottomBar(tab, onTab)
    }
}

@Composable
private fun ChatRow(c: Chat, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 12.dp),
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

@Composable
private fun BottomBar(tab: Int, onTab: (Int) -> Unit) {
    Column(Modifier.fillMaxWidth().background(ScreenBg)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(Hair))
        Row(
            Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 10.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            NavItem(Icons.Filled.ChatBubble, "Chats", tab == 0) { onTab(0) }
            NavItem(Icons.Filled.Call, "Calls", tab == 1) { onTab(1) }
            NavItem(Icons.Filled.Settings, "Settings", tab == 2) { onTab(2) }
        }
    }
}

@Composable
private fun NavItem(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    active: Boolean,
    onClick: () -> Unit,
) {
    val c = if (active) Blue else Sub
    Column(
        horizontalAlignment = Alignment.CenterHorizontally,
        modifier = Modifier.clickable(onClick = onClick).padding(horizontal = 22.dp, vertical = 2.dp),
    ) {
        Icon(icon, null, tint = c, modifier = Modifier.size(24.dp))
        Spacer(Modifier.height(3.dp))
        Text(label, color = c, fontSize = 11.sp, fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal)
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
    onSaveHost: (String) -> Unit,
) {
    SystemBars(Blue, darkIcons = false)
    val name = identity.optString("name", "Me")
    val address = identity.optString("address")
    val card = identity.optString("card")
    val qr = remember(card) { runCatching { qrBitmap(card) }.getOrNull() }
    var myPhone by remember { mutableStateOf(identity.optString("phone")) }
    var host by remember { mutableStateOf(currentHost) }
    Column(Modifier.fillMaxSize().background(ScreenBg)) {
        Row(
            Modifier.fillMaxWidth().background(Blue).padding(horizontal = 16.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.Shield, null, tint = Color.White, modifier = Modifier.size(24.dp))
            Spacer(Modifier.width(8.dp))
            Text("My Denvion ID", color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        }
        Column(
            Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Avatar(name, 76.dp)
            Spacer(Modifier.height(10.dp))
            Text(name, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, color = Ink)
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
            Spacer(Modifier.height(16.dp))
        }
        BottomBar(tab, onTab)
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
    var code by remember { mutableStateOf("") }
    var country by remember { mutableStateOf(COUNTRIES[0]) }
    var countryMenu by remember { mutableStateOf(false) }
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

            // Add by the denvion: contact code — a Paste button reads the clipboard directly
            // (no long-press, so no emulator text-magnifier crash). Easiest between two emulators.
            Spacer(Modifier.height(18.dp))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).padding(16.dp)) {
                Text("Or add by contact code", color = Sub, fontSize = 13.sp)
                Spacer(Modifier.height(8.dp))
                FormField("denvion:…", code, KeyboardType.Text) { code = it }
                Spacer(Modifier.height(10.dp))
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    OutlinedButton(
                        onClick = {
                            val clip = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as? android.content.ClipboardManager
                            val t = clip?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()?.trim().orEmpty()
                            if (t.isNotEmpty()) code = t
                        },
                        modifier = Modifier.weight(1f),
                    ) { Text("Paste") }
                    Button(onClick = { onPasteCode(code.trim()) }, enabled = code.isNotBlank(), modifier = Modifier.weight(1f)) { Text("Add by code") }
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
    val messages = remember {
        mutableStateListOf<Msg>().apply {
            if (real) {
                // restore received messages, but only the ones FROM this contact (by sender identity).
                // seed `seen` with every stored id (any sender) so the poll never re-opens them.
                val stored = store.inbox(myTag)
                for (i in 0 until stored.length()) {
                    val o = stored.getJSONObject(i)
                    seen.add(o.optString("id"))
                    if (o.optString("sender") == chat.identityPub) {
                        add(Msg(System.nanoTime() + i, o.optString("text"), true, "", state = State.OPENED))
                    }
                }
            } else {
                addAll(seedThread())
            }
        }
    }
    var draft by remember { mutableStateOf("") }
    var fastMode by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val polling = remember { java.util.concurrent.atomic.AtomicBoolean(false) }

    LaunchedEffect(Unit) { if (messages.isNotEmpty()) listState.scrollToItem(messages.lastIndex) }

    // Poll my inbox: fetch delivered ciphertext, collect released shares, open, show. One at a time.
    suspend fun poll() {
        if (!real || myTag.isBlank()) return
        if (!polling.compareAndSet(false, true)) return
        try {
            val items = withContext(Dispatchers.IO) { fetchInboxAll(myTag) }
            for (item in items) {
                val sealId = item.optString("seal_id")
                val bundle = item.optJSONObject("bundle") ?: continue
                if (sealId.isBlank() || sealId in seen) continue
                val shares = withContext(Dispatchers.IO) { collectShares(sealId) }
                val opened = runCatching {
                    withContext(Dispatchers.IO) { JSONObject(SealCore.openReceived(myDeviceSeed, bundle.toString(), shares)) }
                }.getOrNull()
                if (opened != null && opened.optBoolean("ok")) {
                    seen.add(sealId)
                    val text = opened.optString("plaintext")
                    val sender = bundle.optString("sender_id_pub") // sender's stable identity_pub
                    // persist under my inbox tagged by sender; only SHOW it in this contact's chat.
                    if (store.addInbox(myTag, sealId, text, sender) && sender == chat.identityPub) {
                        messages.add(Msg(System.nanoTime(), text, true, now(), state = State.OPENED))
                        listState.animateScrollToItem(messages.lastIndex)
                    }
                }
                // if not opened yet (shares still locked) leave it unseen to retry next poll
            }
        } finally {
            polling.set(false)
        }
    }

    // live receive: poll every few seconds while the conversation is open
    LaunchedEffect(myTag, real) {
        if (real && myTag.isNotBlank()) while (true) { poll(); delay(3000) }
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty()) return
        val fast = fastMode
        messages.add(Msg(System.nanoTime(), text, false, now(), state = State.SEALING, mode = if (fast) "FAST" else "STRICT"))
        val idx = messages.lastIndex
        draft = ""
        scope.launch {
            listState.animateScrollToItem(messages.lastIndex)
            if (real) {
                // seal + SHIP over the relay + gateways; the recipient's device polls + opens.
                val ok = runCatching {
                    withContext(Dispatchers.IO) {
                        val ship = JSONObject(SealCore.sealShippable(myIdentitySeed, chat.devicePub, text, fast))
                        if (!ship.optBoolean("ok")) return@withContext false
                        shipSeal(ship)
                        true
                    }
                }.getOrDefault(false)
                delay(if (fast) 400 else 800)
                messages[idx] = messages[idx].copy(state = if (ok) State.OPENED else State.SEALING, sealedFor = chat.name)
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
            items(messages) { m -> Bubble(m) }
        }

        Composer(draft, { draft = it }, ::send)
    }
}

@Composable
private fun DateChip(text: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), horizontalArrangement = Arrangement.Center) {
        Box(
            Modifier.background(Color.White, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 4.dp),
        ) { Text(text, color = Sub, fontSize = 12.sp) }
    }
}

@Composable
private fun Bubble(m: Msg) {
    val align = if (m.incoming) Alignment.Start else Alignment.End
    val bg = if (m.incoming) Incoming else Outgoing
    val shape = RoundedCornerShape(
        topStart = 18.dp, topEnd = 18.dp,
        bottomStart = if (m.incoming) 4.dp else 18.dp,
        bottomEnd = if (m.incoming) 18.dp else 4.dp,
    )
    Column(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalAlignment = align) {
        Box(Modifier.widthIn(max = 300.dp).clip(shape).background(bg).padding(10.dp)) {
            when {
                m.state == State.SEALING -> SealingContent(m)
                m.kind == Kind.IMAGE -> ImageContent(m)
                m.kind == Kind.VOICE -> VoiceContent(m)
                else -> TextContent(m)
            }
        }
        if (m.state == State.SEALING) {
            Row(Modifier.padding(top = 3.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Filled.Autorenew, null, tint = Sub, modifier = Modifier.size(12.dp))
                Spacer(Modifier.width(4.dp))
                Text(if (m.mode == "FAST") "Pre-confirming…" else "Sealing…", color = Sub, fontSize = 11.sp)
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
    Column {
        Box(
            Modifier.size(width = 220.dp, height = 130.dp).clip(RoundedCornerShape(12.dp))
                .background(Brush.verticalGradient(listOf(Color(0xFFF6B26B), Color(0xFF6FA8DC), Color(0xFF2E5B8A)))),
            contentAlignment = Alignment.BottomEnd,
        ) {
            Row(Modifier.padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(m.time, color = Color.White, fontSize = 11.sp)
                Spacer(Modifier.width(3.dp))
                SealBadge()
            }
        }
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
private fun Composer(value: String, onChange: (String) -> Unit, onSend: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(Color.White).padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = value,
            onValueChange = onChange,
            placeholder = { Text("Message", color = Sub, fontSize = 15.sp) },
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
        val active = value.isNotBlank()
        Box(
            Modifier.size(46.dp).background(Blue, CircleShape).clickable(enabled = active, onClick = onSend),
            contentAlignment = Alignment.Center,
        ) {
            Icon(if (active) Icons.Filled.Send else Icons.Filled.Mic, "Send", tint = Color.White, modifier = Modifier.size(22.dp))
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
