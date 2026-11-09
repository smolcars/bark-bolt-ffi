import SwiftUI
import BarkBoltFFI

@main
struct BarkExampleApp: App {
    @StateObject private var model = WalletModel(account: ProcessInfo.processInfo.environment["BARK_RUN_ID"] ?? "wallet")
    var body: some Scene {
        WindowGroup {
            WalletView(model: model)
                .task {
                    #if DEBUG
                    if let command = ProcessInfo.processInfo.environment["BARK_COMMAND"],
                       let data = command.data(using: .utf8),
                       let request = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                        let result = await model.automate(request)
                        if let output = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
                            let file = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("automation-result.json")
                            try? output.write(to: file, options: .atomic)
                            model.status = (result["ok"] as? Bool == true) ? "Test completed · RegTest" : "Test failed · RegTest"
                            model.error = result["error"] as? String
                        }
                    }
                    #endif
                }
        }
    }
}

struct WalletView: View {
    @ObservedObject var model: WalletModel
    @State private var restore = ""
    @State private var destination = ""
    @State private var amount = "10000"
    @State private var boardAmount = "1000000"
    @State private var confirmExit = false
    @State private var errorDetails = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Label("RegTest · local funds only", systemImage: "network")
                    Text(model.status).foregroundStyle(.secondary).accessibilityIdentifier("connection-status")
                    if model.busy { ProgressView("Working…") }
                    if let error = model.error { Text(error).foregroundStyle(.red).textSelection(.enabled).accessibilityIdentifier("wallet-error") }
                }
                if !model.opened {
                    Section("Connect") {
                        LabeledContent("Ark server") { TextField("Ark server", text: $model.endpoint).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled() }
                        LabeledContent("Bitcoin RPC") { TextField("Bitcoin RPC", text: $model.bitcoinEndpoint).keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled() }
                        SecureField("Recovery phrase (optional)", text: $restore).textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("Create or open wallet") { model.perform { try await model.open(restore: restore); restore = "" } }
                            .accessibilityIdentifier("open-wallet")
                    }
                } else {
                    Section("Wallet") {
                        LabeledContent("Available", value: "\(model.spendable) sats").accessibilityIdentifier("spendable-balance")
                        LabeledContent("Onchain", value: "\(model.onchainBalance) sats")
                        Button { model.perform { try await model.sync() } } label: { Label("Sync wallet", systemImage: "arrow.clockwise") }.accessibilityIdentifier("sync-wallet")
                    }
                    Section("Receive") {
                        address("Ark address", model.arkAddress)
                        address("Onchain address", model.onchainAddress)
                        Button("New Ark address") { model.perform { model.arkAddress = try await model.wallet!.newAddress() } }
                    }
                    Section("Send") {
                        TextField("Address, invoice or payment link", text: $destination, axis: .vertical)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().accessibilityIdentifier("destination")
                        LabeledContent("Amount in sats") { TextField("Amount in sats", text: $amount).keyboardType(.numberPad).accessibilityIdentifier("send-amount") }
                        Button("Send payment") { model.perform { try await model.send(destination: destination, amount: amount) } }.accessibilityIdentifier("send-payment")
                    }
                    Section("Lightning receive") {
                        Button("Create 1-hour invoice for \(amount) sats") { model.perform { try await model.createInvoice(amount: amount) } }
                        if !model.invoice.isEmpty { address("Invoice", model.invoice) }
                        Button("Claim received payments") { model.perform { _ = try await model.wallet!.tryClaimAllLightningReceives(wait: true) } }
                    }
                    Section("Activity") {
                        if model.activity.isEmpty { Text("No saved activity yet.").foregroundStyle(.secondary) }
                        ForEach(Array(model.activity.enumerated()), id: \.offset) { _, movement in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(movement.subsystemKind.capitalized).font(.headline)
                                Text("\(movement.status) · \(movement.id)").font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                    Section("Board & maintain") {
                        LabeledContent("Board amount in sats") { TextField("Board amount in sats", text: $boardAmount).keyboardType(.numberPad) }
                        Button("Board to Ark") { model.perform { _ = try await model.wallet!.boardAmount(amountSats: model.amount(boardAmount)); model.status = "Board submitted. Mine a block, then sync." } }.accessibilityIdentifier("board-wallet")
                        Button("Refresh VTXOs") { model.perform { let ids = try await model.wallet!.spendableVtxos().map(\.id); _ = try await model.wallet!.refreshVtxos(vtxoIds: ids) } }
                        Button("Offboard all to onchain address") { model.perform { _ = try await model.wallet!.offboardAll(bitcoinAddress: model.onchainAddress) } }
                        Button("Presign emergency exit") { model.perform { try await model.presignExit() } }.accessibilityIdentifier("presign-exit")
                        if !model.presignedExit.isEmpty { address("Presigned exit plan", model.presignedExit) }
                        Button("Start unilateral exit", role: .destructive) { confirmExit = true }
                        Button("Progress exits") { model.perform { _ = try await model.wallet!.progressExits(feeRateSatPerVb: nil) } }
                        Button("Claim matured exits") {
                            model.perform {
                                let claim = try await model.wallet!.drainExits(vtxoIds: [], drainAll: true, address: model.onchainAddress, feeRateSatPerVb: nil)
                                let signed = try await model.wallet!.signExitClaimInputs(psbtBase64: claim.psbtBase64)
                                _ = try await model.wallet!.broadcastTx(txHex: extractTxFromPsbt(psbtBase64: signed))
                            }
                        }
                    }
                }
            }
            .disabled(model.busy)
            .safeAreaInset(edge: .bottom) {
                HStack {
                    if model.busy { ProgressView() }
                    Text(model.busy ? "Working…" : model.error == nil ? model.status : "Operation failed")
                        .font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("operation-feedback")
                    if model.error != nil { Button("Details") { errorDetails = true } }
                }.padding().background(.bar)
            }
            .onChange(of: model.busy) { if $0 { UIAccessibility.post(notification: .announcement, argument: "Working") } }
            .onChange(of: model.status) { UIAccessibility.post(notification: .announcement, argument: $0) }
            .onChange(of: model.error) { if let message = $0 { UIAccessibility.post(notification: .announcement, argument: message) } }
            .sheet(isPresented: $errorDetails) {
                NavigationStack {
                    ScrollView { Text(model.error ?? "No error details").textSelection(.enabled).padding() }
                        .navigationTitle("Operation failed")
                        .toolbar { Button("Done") { errorDetails = false } }
                }
            }
            .navigationTitle("Bark Wallet")
            .tint(Color(uiColor: UIColor { traits in
                traits.userInterfaceStyle == .dark
                    ? UIColor(red: 0.44, green: 0.86, blue: 0.69, alpha: 1)
                    : UIColor(red: 0, green: 0.42, blue: 0.30, alpha: 1)
            }))
            .confirmationDialog("Exit every VTXO? This broadcasts onchain transactions and requires mining the exit delay.", isPresented: $confirmExit, titleVisibility: .visible) {
                Button("Start exit", role: .destructive) { model.perform { try await model.wallet!.startExitForEntireWallet() } }
            }
        }
    }

    private func address(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.subheadline).foregroundStyle(.secondary)
            Text(value).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            ShareLink(item: value) { Label("Share", systemImage: "square.and.arrow.up") }
        }.padding(.vertical, 4)
    }
}
