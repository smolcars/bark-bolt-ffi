import Foundation
import BarkBoltFFI

@MainActor
final class WalletModel: ObservableObject {
    @Published var status = "Open your RegTest wallet"
    @Published var error: String?
    @Published var busy = false
    @Published var spendable: UInt64 = 0
    @Published var onchainBalance: UInt64 = 0
    @Published var arkAddress = ""
    @Published var onchainAddress = ""
    @Published var invoice = ""
    @Published var presignedExit = ""
    @Published var activity: [Movement] = []
    @Published var opened = false
    @Published var notificationsReceived = 0
    @Published var endpoint = "http://127.0.0.1:3635"
    @Published var bitcoinEndpoint = "http://127.0.0.1:19443"
    var wallet: Wallet?
    var onchain: OnchainWallet?
    var notification: NotificationHolder?
    var notificationTask: Task<Void, Never>?
    private var seed = ""
    private var directory = ""
    private let account: String

    init(account: String = "wallet") { self.account = account }

    static func walletDirectory(account: String, restored: Bool) throws -> URL {
        let root = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        var directory = root.appendingPathComponent(account + (restored ? "-restored-" + UUID().uuidString : ""), isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var resources = URLResourceValues()
        resources.isExcludedFromBackup = true
        try directory.setResourceValues(resources)
        return directory
    }

    func open(restore: String = "", restoredDirectory: Bool = false) async throws {
        let restoring = !restore.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        if seed.isEmpty || restoring { seed = try SeedStore.loadOrCreate(account: account, restore: restore) }
        notification?.cancelNextNotificationWait()
        notificationTask?.cancel()
        if let wallet { try await wallet.stopDaemonWait() }
        notification = nil; wallet = nil; onchain = nil; opened = false
        directory = try Self.walletDirectory(account: account, restored: restoredDirectory).path
        let config = Config(serverAddress: endpoint, bitcoindAddress: bitcoinEndpoint,
                            bitcoindUser: "second", bitcoindPass: "ark", vtxoExitMargin: 2,
                            fallbackFeeRate: 250, daemonSyncIntervalSecs: 2, offboardRequiredConfirmations: 1)
        let chain = try await OnchainWallet.default(network: .regtest, mnemonic: seed, config: config, datadir: directory)
        let openedWallet = try await Wallet.openWithOnchain(network: .regtest, mnemonicOrSeed: seed,
            config: config, args: WalletOpenArgs(datadir: directory), onchain: chain)
        self.onchain = chain; self.wallet = openedWallet
        if restoring { _ = try await chain.initialScan(birthdayHeight: 0) }
        arkAddress = try await openedWallet.newAddress()
        onchainAddress = try await chain.newAddress()
        try await sync()
        opened = true
        status = "Connected · RegTest"
        let holder = openedWallet.notifications()
        notification = holder
        notificationTask = Task { [weak self] in
            do {
                while !Task.isCancelled {
                    guard try await holder.nextNotification() != nil else { break }
                    guard let self else { break }
                    self.notificationsReceived += 1
                    try await self.readBalances()
                }
            } catch { if !Task.isCancelled { self?.error = error.localizedDescription } }
        }
    }

    func readBalances() async throws {
        guard let wallet, let onchain else { return }
        spendable = try await wallet.balance().spendableSats
        onchainBalance = try await onchain.balance().totalSats
        activity = try await wallet.history()
    }

    func sync() async throws {
        guard let wallet, let onchain else { throw AppError.message("Open a wallet first.") }
        _ = try await onchain.sync()
        try await wallet.sync()
        try await readBalances()
    }

    func perform(_ action: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        let previousStatus = status
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                try await action(); if opened { try await readBalances() }
                if status == previousStatus { status = "Wallet updated · RegTest" }
            }
            catch let failure as BarkError { error = failure.message }
            catch { self.error = error.localizedDescription }
        }
    }

    func amount(_ text: String) throws -> UInt64 {
        guard let value = UInt64(text), value > 0 else { throw AppError.message("Enter a positive whole number of sats.") }
        return value
    }

    func send(destination: String, amount: String) async throws {
        guard let wallet else { throw AppError.message("Open a wallet first.") }
        let request = try await wallet.parsePaymentRequest(input: destination)
        guard let option = request.defaultOption else { throw AppError.message("No usable payment method in this request.") }
        // Pay the requested amount; ask for one only when the destination carries none.
        let sats: UInt64? = try option.requiresAmount ? request.amountSats ?? self.amount(amount) : nil
        _ = try await wallet.sendPayment(method: option.method, amountSats: sats, options: LightningSendOptions(wait: true))
        status = "Payment sent"
    }

    func createInvoice(amount: String) async throws {
        guard let wallet else { throw AppError.message("Open a wallet first.") }
        invoice = try await wallet.bolt11InvoiceWith(amountSats: self.amount(amount),
            options: Bolt11InvoiceOptions(description: "Bark example", expirySecs: 3600)).invoice
    }

    /// Signs every spendable VTXO's exit children now, so the exit can be broadcast later without this wallet.
    @discardableResult
    func presignExit() async throws -> (plan: PresignedExits, remaining: RemainingExitTransactions) {
        guard let wallet, let onchain else { throw AppError.message("Open a wallet first.") }
        // Children only spend coins the onchain wallet already sees as confirmed.
        _ = try await onchain.sync()
        let ids = try await wallet.spendableVtxos().map(\.id)
        guard !ids.isEmpty else { throw AppError.message("No spendable VTXOs to exit.") }
        let remaining = try await wallet.remainingExitTransactions(vtxoIds: ids)
        let plan = try await wallet.presignEmergencyExit(vtxoIds: ids, feeRateSatPerVb: nil)
        presignedExit = plan.waves.enumerated().map { index, wave in
            (["Wave \(index + 1): broadcast each exit with its child, then wait for confirmation"]
                + wave.packages.map { "\($0.exit.txHex)\n\($0.child.txHex)" }).joined(separator: "\n")
        }.joined(separator: "\n\n")
        let packages = plan.waves.reduce(0) { $0 + $1.packages.count }
        status = "Presigned \(packages) of \(remaining.transactions.count) exit transactions in \(plan.waves.count) wave\(plan.waves.count == 1 ? "" : "s")"
        return (plan, remaining)
    }

    #if DEBUG
    // Test orchestration uses the same SDK handles as the visible wallet.
    func automate(_ request: [String: Any]) async -> [String: Any] {
        do {
            endpoint = request["server"] as? String ?? endpoint
            bitcoinEndpoint = request["bitcoin"] as? String ?? bitcoinEndpoint
            if wallet == nil { try await open() }
            guard let wallet, let onchain else { throw AppError.message("Wallet unavailable") }
            let action = request["action"] as? String ?? "open"
            var extra: [String: Any] = [:]
            let sats = (request["amount"] as? NSNumber)?.uint64Value ?? 10000
            let destination = request["destination"] as? String ?? ""
            switch action {
            case "open": break
            case "sync": try await sync()
            case "board": extra["txid"] = try await wallet.boardAmount(amountSats: sats).txid
            case "send": try await send(destination: destination, amount: String(sats))
            case "invoice":
                let result = try await wallet.bolt11InvoiceWith(amountSats: sats, options: Bolt11InvoiceOptions(description: "SDK RegTest", expirySecs: 3600))
                extra["invoice"] = result.invoice; extra["paymentHash"] = result.paymentHash
            case "claim": _ = try await wallet.tryClaimAllLightningReceives(wait: true)
            case "refresh": extra["txid"] = try await wallet.refreshVtxos(vtxoIds: wallet.spendableVtxos().map(\.id)) ?? ""
            case "offboard": extra["txid"] = try await wallet.offboardVtxos(vtxoIds: wallet.spendableVtxos().prefix(1).map(\.id), bitcoinAddress: destination).txid
            case "estimate":
                let estimate = try await wallet.estimateEmergencyExitFee(vtxoIds: [], feeRateSatPerVb: 1, destination: onchainAddress)
                extra["totalFee"] = estimate.totalFeeSats
                extra["broadcastFee"] = estimate.exitBroadcastFeeSats
                extra["claimFee"] = estimate.claimFeeSats
            case "presignExit":
                let (plan, remaining) = try await presignExit()
                extra["packages"] = plan.waves.reduce(0) { $0 + $1.packages.count }
                extra["waves"] = plan.waves.count
                extra["remaining"] = remaining.transactions.count
                extra["fullySigned"] = plan.exits.allSatisfy { if case .fullySigned = $0.status { true } else { false } }
            case "startExit": try await wallet.startExitForVtxos(vtxoIds: wallet.spendableVtxos().prefix(1).map(\.id))
            case "progressExit":
                _ = try await wallet.progressExits(feeRateSatPerVb: 1)
                extra["claimable"] = try await wallet.listClaimableExits().count
                extra["claimHeight"] = try await wallet.allExitsClaimableAtHeight().map { Int($0) } ?? -1
            case "drainExit":
                let claim = try await wallet.drainExits(vtxoIds: [], drainAll: true, address: destination, feeRateSatPerVb: 1)
                let signed = try await wallet.signExitClaimInputs(psbtBase64: claim.psbtBase64)
                extra["txid"] = try await wallet.broadcastTx(txHex: extractTxFromPsbt(psbtBase64: signed))
            case "restore":
                try await open(restore: seed, restoredDirectory: true)
                extra["databaseName"] = URL(fileURLWithPath: directory).lastPathComponent
                extra["recoveryComplete"] = self.wallet?.recoveryReport()?.isComplete == true
            case "cancel":
                let holder = wallet.notifications()
                let wait = Task { try await holder.nextNotification() }
                try await Task.sleep(nanoseconds: 100_000_000)
                wait.cancel(); _ = try? await wait.value
                let again = Task { try await holder.nextNotification() }
                try await Task.sleep(nanoseconds: 100_000_000)
                holder.cancelNextNotificationWait(); _ = try await again.value
                extra["reused"] = true
            default: throw AppError.message("Unknown automation action")
            }
            if action == "board" {
                for _ in 0..<100 where notificationsReceived == 0 { try await Task.sleep(nanoseconds: 20_000_000) }
            }
            try await readBalances()
            return ["notifications": notificationsReceived, "ok": true, "arkAddress": arkAddress, "onchainAddress": onchainAddress,
                    "spendable": spendable, "onchain": onchainBalance, "historyCount": activity.count,
                    "fingerprint": self.wallet!.fingerprint()].merging(extra) { _, value in value }
        } catch let error as BarkError { return ["ok": false, "error": error.message] }
        catch { return ["ok": false, "error": error.localizedDescription] }
    }
    #endif
}
