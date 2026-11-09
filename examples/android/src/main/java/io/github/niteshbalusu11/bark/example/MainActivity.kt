package io.github.niteshbalusu11.bark.example

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Alignment
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.io.File

class MainActivity : ComponentActivity() {
    private val model by viewModels<WalletModel>()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (BuildConfig.DEBUG) {
            model.account = intent.getStringExtra("runId") ?: "wallet"
            intent.getStringExtra("command")?.let { command -> lifecycleScope.launch {
                val result = model.automate(JSONObject(command))
                File(filesDir, "automation-result.json").writeText(result.toString())
            } }
        }
        setContent {
            MaterialTheme(colorScheme = if (isSystemInDarkTheme()) darkColorScheme(primary = androidx.compose.ui.graphics.Color(0xFF70DBAF)) else lightColorScheme(primary = androidx.compose.ui.graphics.Color(0xFF006C4C))) {
                WalletScreen(model)
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun WalletScreen(model: WalletModel) {
    var restore by remember { mutableStateOf("") }
    var destination by remember { mutableStateOf("") }
    var amount by remember { mutableStateOf("10000") }
    var boardAmount by remember { mutableStateOf("1000000") }
    var exitDialog by remember { mutableStateOf(false) }
    var errorDetails by remember { mutableStateOf(false) }
    Scaffold(topBar = { TopAppBar(title = { Text("Bark Wallet") }) }, bottomBar = {
        Surface(tonalElevation = 3.dp) {
            Row(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                if (model.busy) CircularProgressIndicator(Modifier.size(24.dp))
                Text(if (model.busy) "Working…" else if (model.error != null) "Operation failed" else model.status,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.weight(1f).testTag("operation-feedback").semantics { liveRegion = LiveRegionMode.Polite })
                if (model.error != null) TextButton(onClick = { errorDetails = true }) { Text("Details") }
            }
        }
    }) { insets ->
        LazyColumn(Modifier.fillMaxSize().padding(insets).imePadding(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item { Text("RegTest · local funds only", style = MaterialTheme.typography.titleMedium) }
            item { Text(model.status, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.testTag("connection-status")) }
            if (model.busy) item { LinearProgressIndicator(Modifier.fillMaxWidth()) }
            model.error?.let { failure -> item { SelectionContainer { Text(failure, color = MaterialTheme.colorScheme.error, modifier = Modifier.testTag("wallet-error")) } } }
            if (!model.opened) {
                item { Heading("Connect") }
                item { OutlinedTextField(model.endpoint, { model.endpoint = it }, label = { Text("Ark server") }, modifier = Modifier.fillMaxWidth(), enabled = !model.busy) }
                item { OutlinedTextField(model.bitcoinEndpoint, { model.bitcoinEndpoint = it }, label = { Text("Bitcoin RPC") }, modifier = Modifier.fillMaxWidth(), enabled = !model.busy) }
                item { OutlinedTextField(restore, { restore = it }, label = { Text("Recovery phrase (optional)") }, visualTransformation = androidx.compose.ui.text.input.PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false), modifier = Modifier.fillMaxWidth(), enabled = !model.busy) }
                item { Button(onClick = { model.perform { model.open(restore); restore = "" } }, enabled = !model.busy, modifier = Modifier.testTag("open-wallet")) { Text("Create or open wallet") } }
            } else {
                item { Heading("Wallet") }
                item { Text("${model.spendable} sats available", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.testTag("spendable-balance")) }
                item { Text("${model.onchainBalance} sats onchain", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                item { Action("Sync wallet", model, "sync-wallet") { model.sync() } }
                item { Heading("Receive") }
                item { Address("Ark address", model.arkAddress) }
                item { Address("Onchain address", model.onchainAddress) }
                item { Heading("Send") }
                item { OutlinedTextField(destination, { destination = it }, label = { Text("Address, invoice or payment link") }, modifier = Modifier.fillMaxWidth().testTag("destination"), enabled = !model.busy) }
                item { OutlinedTextField(amount, { amount = it }, label = { Text("Amount in sats") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.fillMaxWidth().testTag("send-amount"), enabled = !model.busy) }
                item { Action("Send payment", model, "send-payment") { model.send(destination, amount) } }
                item { Heading("Lightning receive") }
                item { Action("Create 1-hour invoice for $amount sats", model) { model.createInvoice(amount) } }
                if (model.invoice.isNotEmpty()) item { Address("Invoice", model.invoice) }
                item { Action("Claim received payments", model) { model.claim() } }
                item { Heading("Activity") }
                if (model.activity.isEmpty()) item { Text("No saved activity yet.") }
                items(model.activity, key = { it.id.toString() }) { movement ->
                    Column {
                        Text(movement.subsystemKind.replaceFirstChar { it.uppercase() }, style = MaterialTheme.typography.titleMedium)
                        Text("${movement.status} · ${movement.effectiveBalanceSats} sats", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                item { Heading("Board & maintain") }
                item { OutlinedTextField(boardAmount, { boardAmount = it }, label = { Text("Board amount in sats") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.fillMaxWidth(), enabled = !model.busy) }
                item { Action("Board to Ark", model, "board-wallet") { model.board(boardAmount) } }
                item { Action("Refresh VTXOs", model) { model.refresh() } }
                item { Action("Offboard all to onchain address", model) { model.offboard() } }
                item { Action("Presign emergency exit", model, "presign-exit") { model.presignExit() } }
                if (model.presignedExit.isNotEmpty()) item { Address("Presigned exit plan", model.presignedExit) }
                item { OutlinedButton(onClick = { exitDialog = true }, enabled = !model.busy) { Text("Start unilateral exit", color = MaterialTheme.colorScheme.error) } }
                item { Action("Progress exits", model) { model.progressExit() } }
                item { Action("Claim matured exits", model) { model.drainExit() } }
            }
        }
    }
    if (errorDetails) AlertDialog(onDismissRequest = { errorDetails = false },
        title = { Text("Operation failed") },
        text = { SelectionContainer { Text(model.error ?: "No error details",
            modifier = Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState())) } },
        confirmButton = { TextButton(onClick = { errorDetails = false }) { Text("Done") } })
    if (exitDialog) AlertDialog(onDismissRequest = { exitDialog = false }, title = { Text("Exit every VTXO?") },
        text = { Text("This broadcasts onchain transactions and requires mining the exit delay.") },
        confirmButton = { TextButton(onClick = { exitDialog = false; model.perform { model.startExit() } }) { Text("Start exit") } },
        dismissButton = { TextButton(onClick = { exitDialog = false }) { Text("Cancel") } })
}

@Composable private fun Heading(title: String) {
    HorizontalDivider(Modifier.padding(top = 12.dp, bottom = 8.dp))
    Text(title, style = MaterialTheme.typography.titleLarge)
}
@Composable private fun Address(title: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title, style = MaterialTheme.typography.labelLarge)
        SelectionContainer { Text(value, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
    }
}
@Composable private fun Action(title: String, model: WalletModel, tag: String = title, action: suspend () -> Unit) {
    OutlinedButton(onClick = { model.perform(action) }, enabled = !model.busy, modifier = Modifier.testTag(tag)) { Text(title) }
}
