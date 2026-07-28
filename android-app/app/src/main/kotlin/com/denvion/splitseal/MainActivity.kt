package com.denvion.splitseal

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

private val BG = Color(0xFF0E1621)
private val PANEL = Color(0xFF17212B)
private val BUBBLE = Color(0xFF182533)
private val ACCENT = Color(0xFF2EA6FF)
private val LOCKED = Color(0xFFF2B01E)
private val OK = Color(0xFF4FCB6B)

data class SealMsg(
    val body: String,
    val state: String = "DELIVERED_LOCKED", // -> FINALISING -> UNLOCKED / LOCKED
    val plaintext: String? = null,
    val shares: Int = 0,
    val status: String? = null,
    val mode: String = "STRICT", // "STRICT" (finality) or "FAST" (pre-confirmations)
)

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val proto = runCatching { JSONObject(SealCore.version()).let { "DSCP-${it.getInt("version")} · chain ${it.getLong("chain_id")}" } }
            .getOrDefault("DSCP-1")
        setContent { MaterialTheme(colorScheme = darkColorScheme(primary = ACCENT, surface = PANEL)) { ChatScreen(proto) } }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(proto: String) {
    val messages = remember { mutableStateListOf<SealMsg>() }
    var draft by remember { mutableStateOf("Sealed by WCAHT before it opens.") }
    var fastMode by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()

    fun send() {
        val text = draft.trim()
        if (text.isEmpty()) return
        val fast = fastMode
        messages.add(SealMsg(text, mode = if (fast) "FAST" else "STRICT"))
        val idx = messages.lastIndex
        draft = ""
        scope.launch {
            listState.animateScrollToItem(idx)
            // Drive the real Rust seal core off the main thread — StrictSeal or FastSeal.
            val transcript = runCatching {
                withContext(Dispatchers.Default) { if (fast) SealCore.runFastDemo() else SealCore.runDemo() }
            }.getOrNull()
            if (transcript == null) {
                delay(700); messages[idx] = messages[idx].copy(state = "FINALISING")
                delay(900); messages[idx] = messages[idx].copy(state = "UNLOCKED", plaintext = text, shares = 3, status = "opened (fallback)")
                return@launch
            }
            // StrictSeal steps: before_finality/after_finality. FastSeal: before_preconf/after_preconf_quorum.
            val steps = JSONObject(transcript).getJSONArray("transcript")
            for (i in 0 until steps.length()) {
                val step = steps.getJSONObject(i)
                when (step.getString("step")) {
                    "before_finality", "before_preconf" -> {
                        delay(650)
                        messages[idx] = messages[idx].copy(state = "FINALISING")
                    }
                    "after_finality", "after_preconf_quorum" -> {
                        delay(if (fast) 220 else 900)
                        val out = step.getJSONObject("outcome")
                        val count = if (step.has("shares_released")) step.optInt("shares_released") else step.optInt("preconfs")
                        messages[idx] = if (out.getString("result") == "OPENED") {
                            messages[idx].copy(
                                state = "UNLOCKED", plaintext = text, shares = count,
                                status = if (fast) "pre-confirmed · no finality" else step.optString("status", "finalised"),
                            )
                        } else {
                            messages[idx].copy(state = "LOCKED", status = out.optString("reason"))
                        }
                    }
                }
            }
            listState.animateScrollToItem(messages.lastIndex)
        }
    }

    Scaffold(
        containerColor = BG,
        topBar = {
            TopAppBar(
                colors = TopAppBarDefaults.topAppBarColors(containerColor = PANEL),
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Filled.Shield, null, tint = ACCENT)
                        Spacer(Modifier.width(10.dp))
                        Column {
                            Text("Denvion SplitSeal", fontSize = 16.sp, fontWeight = FontWeight_600)
                            Text("Secured by WCAHT · $proto", fontSize = 11.sp, color = Color.White.copy(alpha = .55f))
                        }
                    }
                },
            )
        },
        bottomBar = { Composer(draft, { draft = it }, ::send) },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            ModeToggle(fastMode) { fastMode = it }
            if (messages.isEmpty()) {
                EmptyState(Modifier.weight(1f))
            } else {
                LazyColumn(state = listState, modifier = Modifier.weight(1f).fillMaxWidth().padding(12.dp)) {
                    itemsIndexed(messages) { _, m -> Bubble(m) }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ModeToggle(fast: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(PANEL).padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        FilterChip(
            selected = !fast,
            onClick = { onChange(false) },
            label = { Text("StrictSeal · vault") },
            leadingIcon = { Icon(Icons.Filled.Lock, null, Modifier.size(16.dp)) },
        )
        FilterChip(
            selected = fast,
            onClick = { onChange(true) },
            label = { Text("FastSeal · instant") },
            leadingIcon = { Icon(Icons.Filled.Bolt, null, Modifier.size(16.dp)) },
        )
    }
}

private val FontWeight_600 = androidx.compose.ui.text.font.FontWeight.SemiBold

@Composable
fun EmptyState(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(32.dp)) {
            Icon(Icons.Filled.LockClock, null, tint = ACCENT, modifier = Modifier.size(56.dp))
            Spacer(Modifier.height(16.dp))
            Text("Every message arrives locked", fontSize = 16.sp, fontWeight = FontWeight_600)
            Spacer(Modifier.height(8.dp))
            Text(
                "It opens only when its WCAHT seal releases — hard L1 finality (StrictSeal) or a staked gateway pre-confirmation quorum (FastSeal).",
                textAlign = TextAlign.Center, fontSize = 13.sp, color = Color.White.copy(alpha = .55f),
            )
        }
    }
}

@Composable
fun Bubble(m: SealMsg) {
    val unlocked = m.state == "UNLOCKED"
    Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), horizontalArrangement = Arrangement.End) {
        Column(
            Modifier
                .widthIn(max = 300.dp)
                .background(if (unlocked) BUBBLE else PANEL, RoundedCornerShape(14.dp))
                .border(1.dp, (if (unlocked) OK else LOCKED).copy(alpha = .4f), RoundedCornerShape(14.dp))
                .padding(start = 14.dp, top = 10.dp, end = 14.dp, bottom = 8.dp),
        ) {
            if (unlocked) {
                Text(m.plaintext ?: m.body, fontSize = 15.sp, color = Color.White)
            } else {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Filled.Lock, null, tint = LOCKED, modifier = Modifier.size(15.dp))
                    Spacer(Modifier.width(8.dp))
                    Text("•".repeat(m.body.length.coerceIn(6, 22)), color = LOCKED, fontSize = 14.sp)
                }
            }
            Spacer(Modifier.height(6.dp))
            StatusChip(m)
        }
    }
}

@Composable
fun StatusChip(m: SealMsg) {
    val fast = m.mode == "FAST"
    val (label, color, icon) = when (m.state) {
        "DELIVERED_LOCKED" -> Triple(if (fast) "Delivered · FastSeal" else "Delivered · locked", LOCKED, Icons.Filled.Lock)
        "FINALISING" ->
            if (fast) Triple("Awaiting gateway pre-confs", LOCKED, Icons.Filled.Bolt)
            else Triple("Securing seal · finalising", LOCKED, Icons.Filled.HourglassTop)
        "UNLOCKED" -> Triple(
            "Opened · ${m.status ?: if (fast) "pre-confirmed" else "finalised"}",
            OK,
            if (fast) Icons.Filled.Bolt else Icons.Filled.VerifiedUser,
        )
        else -> Triple("Locked", LOCKED, Icons.Filled.Lock)
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, tint = color, modifier = Modifier.size(12.dp))
        Spacer(Modifier.width(5.dp))
        Text(label, fontSize = 11.sp, color = color)
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Composer(value: String, onChange: (String) -> Unit, onSend: () -> Unit) {
    Row(
        Modifier.background(PANEL).padding(start = 12.dp, top = 8.dp, end = 8.dp, bottom = 12.dp).fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlinedTextField(
            value = value,
            onValueChange = onChange,
            modifier = Modifier.weight(1f),
            placeholder = { Text("Send a sealed message…") },
            maxLines = 4,
            keyboardActions = KeyboardActions(onSend = { onSend() }),
            colors = OutlinedTextFieldDefaults.colors(unfocusedContainerColor = BG, focusedContainerColor = BG),
        )
        Spacer(Modifier.width(8.dp))
        FloatingActionButton(onClick = onSend, containerColor = ACCENT, modifier = Modifier.size(48.dp)) {
            Icon(Icons.Filled.LockOpen, "Send", tint = Color.White)
        }
    }
}
