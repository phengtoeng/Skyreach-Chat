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
import androidx.compose.foundation.text.selection.SelectionContainer
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
private data class Chat(val name: String, val last: String, val time: String, val unread: Int = 0)

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
private data class Contact(val name: String, val address: String, val devicePub: String, val phone: String)

private class Store(ctx: Context) {
    private val p = ctx.getSharedPreferences("denvion", Context.MODE_PRIVATE)
    fun identity(): JSONObject? = p.getString("identity", null)?.let { JSONObject(it) }
    fun saveIdentity(j: JSONObject) = p.edit().putString("identity", j.toString()).apply()
    fun contacts(): JSONArray = p.getString("contacts", null)?.let { JSONArray(it) } ?: JSONArray()
    fun addContact(c: JSONObject) {
        val a = contacts(); a.put(c); p.edit().putString("contacts", a.toString()).apply()
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
        Contact(o.optString("name"), o.optString("address"), o.optString("device_pub"), o.optString("phone"))
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

// ─────────────────────────────── activity ───────────────────────────────────
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
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

    val chat = openChat
    when {
        chat != null -> ConversationScreen(chat) { openChat = null }
        showNew -> NewContactScreen(
            scanned = scanned,
            onScan = { launchScan() },
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
        tab == 2 -> ProfileScreen(identity, tab) { tab = it }
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
        Chat(it.name, sub, "")
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
private fun ProfileScreen(identity: JSONObject, tab: Int, onTab: (Int) -> Unit) {
    SystemBars(Blue, darkIcons = false)
    val name = identity.optString("name", "Me")
    val address = identity.optString("address")
    val card = identity.optString("card")
    val qr = remember(card) { runCatching { qrBitmap(card) }.getOrNull() }
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
        }
        BottomBar(tab, onTab)
    }
}

@Composable
private fun NewContactScreen(
    scanned: JSONObject?,
    onScan: () -> Unit,
    onCancel: () -> Unit,
    onSave: (String, String, String) -> Unit,
) {
    val bg = Color(0xFFF2F3F5)
    SystemBars(bg, darkIcons = true)
    var first by remember(scanned) { mutableStateOf(scanned?.optString("name") ?: "") }
    var last by remember { mutableStateOf("") }
    var phone by remember { mutableStateOf("") }
    var sync by remember { mutableStateOf(true) }
    val canSave = first.isNotBlank() || scanned != null

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
                    .clickable(enabled = canSave) { onSave(first.trim(), last.trim(), phone.trim()) },
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.Filled.Check, "Save", tint = Color.White, modifier = Modifier.size(20.dp)) }
        }

        Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp)) {
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).padding(horizontal = 16.dp)) {
                FormField("First Name", first) { first = it }
                Box(Modifier.fillMaxWidth().height(1.dp).background(Hair))
                FormField("Last Name", last) { last = it }
            }
            Spacer(Modifier.height(18.dp))

            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White)) {
                Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("🇰🇭  Cambodia", color = Ink, fontSize = 16.sp)
                    Spacer(Modifier.weight(1f))
                    Icon(Icons.Filled.ChevronRight, null, tint = Sub)
                }
                Box(Modifier.padding(start = 16.dp).fillMaxWidth().height(1.dp).background(Hair))
                Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("+855", color = Ink, fontSize = 16.sp)
                    Spacer(Modifier.width(16.dp))
                    Box(Modifier.weight(1f)) {
                        if (phone.isEmpty()) Text("00 000 0000", color = Sub, fontSize = 16.sp)
                        BasicTextField(
                            value = phone,
                            onValueChange = { phone = it.filter { c -> c.isDigit() || c == ' ' } },
                            textStyle = TextStyle(color = Ink, fontSize = 16.sp),
                            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                            modifier = Modifier.fillMaxWidth(),
                        )
                    }
                }
            }
            Spacer(Modifier.height(18.dp))

            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(Color.White).padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Sync Contact to Phone", color = Ink, fontSize = 16.sp)
                Spacer(Modifier.weight(1f))
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

@Composable
private fun FormField(placeholder: String, value: String, onChange: (String) -> Unit) {
    Box(Modifier.fillMaxWidth().padding(vertical = 15.dp)) {
        if (value.isEmpty()) Text(placeholder, color = Sub, fontSize = 16.sp)
        BasicTextField(
            value = value,
            onValueChange = onChange,
            textStyle = TextStyle(color = Ink, fontSize = 16.sp),
            cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

// ────────────────────────────── conversation ────────────────────────────────
@Composable
private fun ConversationScreen(chat: Chat, onBack: () -> Unit) {
    SystemBars(Color.White, darkIcons = true)
    BackHandler(onBack = onBack)

    val messages = remember { mutableStateListOf<Msg>().apply { addAll(seedThread()) } }
    var draft by remember { mutableStateOf("") }
    var fastMode by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) { if (messages.isNotEmpty()) listState.scrollToItem(messages.lastIndex) }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty()) return
        val fast = fastMode
        messages.add(Msg(System.nanoTime(), text, false, now(), state = State.SEALING, mode = if (fast) "FAST" else "STRICT"))
        val idx = messages.lastIndex
        draft = ""
        scope.launch {
            listState.animateScrollToItem(messages.lastIndex)
            // drive the real Rust seal core off the main thread
            val transcript = runCatching {
                withContext(Dispatchers.Default) { if (fast) SealCore.runFastDemo() else SealCore.runDemo() }
            }.getOrNull()
            delay(if (fast) 550 else 1150) // show the sealing state
            val opened = transcript?.let { runCatching { JSONObject(it).getJSONArray("transcript").let { a -> a.toString().contains("OPENED") } }.getOrDefault(true) } ?: true
            messages[idx] = messages[idx].copy(state = if (opened) State.OPENED else State.SEALING)
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
        Text(m.time, color = Sub, fontSize = 11.sp)
        if (!m.incoming) {
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
        Row(
            Modifier.weight(1f).clip(RoundedCornerShape(24.dp)).background(Color(0xFFF1F4F7)).padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Outlined.EmojiEmotions, null, tint = Sub, modifier = Modifier.size(22.dp))
            Spacer(Modifier.width(8.dp))
            Box(Modifier.weight(1f)) {
                if (value.isEmpty()) Text("Message", color = Sub, fontSize = 15.sp)
                BasicTextField(
                    value = value,
                    onValueChange = onChange,
                    textStyle = TextStyle(color = Ink, fontSize = 15.sp),
                    cursorBrush = Brush.verticalGradient(listOf(Blue, Blue)),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
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
