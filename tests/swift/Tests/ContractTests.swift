import Foundation
import XCTest
import BarkBoltFFI

final class ContractTests: XCTestCase {
    func testLoggerRetainsThenReleasesForeignCallback() throws {
        weak var released: Logger?
        do {
            let logger = Logger()
            released = logger
            try setLogger(logger: logger, maxLevel: .info)
        }
        XCTAssertNotNil(released)
        clearLogger()
        XCTAssertNil(released)
    }

    func testMnemonicAndErrors() throws {
        XCTAssertTrue(try validateMnemonic(mnemonic: generateMnemonic()))
        XCTAssertFalse(try validateMnemonic(mnemonic: "invalid"))
        XCTAssertEqual(defaultVtxoKeyGapLimit(), 250)
        XCTAssertEqual(maxVtxoKeyGapLimit(), 100000)
    }

    func testCallbackFullRangeAndRelease() async throws {
        weak var released: Callbacks?
        do {
            let callback = Callbacks()
            released = callback
            let wallet = try OnchainWallet(custom: callback)
            do {
                _ = try await wallet.prepareSend(address: "unused", amount: .drainAll, feeRateSatPerVb: 10, confirmedOnly: true)
                XCTFail("Expected unsupported preparation")
            } catch let error as BarkError { XCTAssertEqual(error.code, .unsupportedOperation) }
            let balance = try await wallet.balance()
            XCTAssertEqual(balance.totalSats, UInt64.max)
            do {
                try await wallet.evictTx(txid: String(repeating: "11", count: 32))
                XCTFail("Expected callback error")
            } catch let error as BarkError { XCTAssertTrue(error.message.contains("callback failure 🦀\0")) }
            callback.enableUnexpectedError()
            do {
                try await wallet.evictTx(txid: String(repeating: "11", count: 32))
                XCTFail("Expected unexpected callback error conversion")
            } catch let error as BarkError { XCTAssertTrue(error.message.contains("unexpected callback failure")) }
            let again = try await wallet.balance()
            XCTAssertEqual(again.totalSats, UInt64.max)
        }
        XCTAssertNil(released)
    }
}

private final class Callbacks: CustomOnchainWalletCallbacks {
    private let lock = NSLock()
    private var unexpected = false
    func enableUnexpectedError() { lock.lock(); unexpected = true; lock.unlock() }
    func getBalance() throws -> UInt64 { UInt64.max }
    func prepareTx(destinations: [Destination], feeRateSatPerVb: UInt64) throws -> String { throw BarkError(message: "unexpected prepare") }
    func prepareDrainTx(address: String, feeRateSatPerVb: UInt64) throws -> String { throw BarkError(message: "unexpected drain") }
    func signPsbt(psbtBase64: String) throws -> String { throw BarkError(message: "unexpected sign") }
    func finishPsbt(psbtBase64: String) throws -> String { throw BarkError(message: "unexpected finish") }
    func isMine(scriptPubkeyHex: String) throws -> Bool { false }
    func registerTx(txHex: String) throws {}
    func evictTx(txid: String) throws {
        lock.lock(); let enabled = unexpected; lock.unlock()
        if enabled { throw NSError(domain: "BarkContract", code: 1, userInfo: [NSLocalizedDescriptionKey: "unexpected callback failure"]) }
        throw BarkError(message: "callback failure 🦀\0")
    }
    func makeSignedP2aCpfp(params: CpfpParams) throws -> String { throw BarkError(message: "unexpected cpfp") }
    func storeSignedP2aCpfp(txHex: String) throws {}
    func sync() throws {}
}

private final class Logger: BarkLogger {
    func log(level: LogLevel, target: String, message: String) {}
}

private var fixtures: URL {
    if let configured = ProcessInfo.processInfo.environment["BARK_CONTRACT_FIXTURES"] { return URL(fileURLWithPath: configured) }
    return URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("fixtures")
}
private struct NitroFixture: Decodable {
    struct Vector: Decodable {
        let index: UInt32
        let offlinePublicKey, offlineSignature, walletPublicKey, walletSignature: String
    }
    let mnemonic, message: String
    let vectors: [Vector]
    static func load() throws -> Self { try JSONDecoder().decode(Self.self, from: Data(contentsOf: fixtures.appendingPathComponent("nitro-v0.0.144-crypto.json"))) }
}

extension ContractTests {
    func testNitroOfflineGoldenVectors() throws {
        let fixture = try NitroFixture.load()
        for network: Network in [.bitcoin, .testnet, .signet, .regtest] {
            for vector in fixture.vectors {
                let key = try derivePublicKeyFromMnemonic(mnemonic: fixture.mnemonic, network: network, index: vector.index)
                let signature = try signMessageWithMnemonic(message: fixture.message, mnemonic: fixture.mnemonic, network: network, index: vector.index)
                XCTAssertEqual(key, vector.offlinePublicKey)
                XCTAssertEqual(signature, vector.offlineSignature)
                XCTAssertNotEqual(key, vector.walletPublicKey)
                XCTAssertTrue(try verifyMessage(message: fixture.message, signatureDerHex: signature, publicKeyHex: key))
                XCTAssertFalse(try verifyMessage(message: fixture.message + "!", signatureDerHex: signature, publicKeyHex: key))
            }
        }
        XCTAssertThrowsError(try derivePublicKeyFromMnemonic(mnemonic: "invalid", network: .regtest, index: 0))
        XCTAssertThrowsError(try verifyMessage(message: "test", signatureDerHex: "00", publicKeyHex: "00"))
    }

    func testSchema42SnapshotWithoutWallet() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let path = directory.appendingPathComponent("db.sqlite")
        try FileManager.default.copyItem(at: fixtures.appendingPathComponent("noah-v2-schema42.sqlite"), to: path)
        let before = try Data(contentsOf: path)
        let info = try await validateWalletSnapshot(path: path.path, expected: nil)
        XCTAssertEqual(info.schemaVersion, 42)
        XCTAssertEqual(info.network, "regtest")
        XCTAssertEqual(info.sizeBytes, UInt64(before.count))
        XCTAssertEqual(info.mailboxPubkey, "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5")
        XCTAssertEqual(try Data(contentsOf: path), before)
        do {
            _ = try await validateWalletSnapshot(path: path.path, expected: WalletSnapshotExpectation(network: "bitcoin"))
            XCTFail("Expected identity mismatch")
        } catch is BarkError {}
    }

    func testWalletNitroVectorsAndSnapshotObserver() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let fixture = try NitroFixture.load()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let env = ProcessInfo.processInfo.environment
        let config = Config(serverAddress: "http://127.0.0.1:\(env["ARK_PORT"] ?? "3635")", bitcoindAddress: "http://127.0.0.1:\(env["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark")
        let wallet = try await Wallet.open(network: .regtest, mnemonicOrSeed: fixture.mnemonic, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createWithoutServer: true, skipRecovery: true))
        do { _ = try await wallet.peekPublicKey(index: 0); XCTFail("Expected unstored key error") } catch is BarkError {}
        do { _ = try await wallet.signMessage(message: fixture.message, index: 0); XCTFail("Expected unstored key error") } catch is BarkError {}
        let holder = try wallet.stateChanges()
        defer { holder.stop() }
        XCTAssertTrue(holder.isActive())
        let initial = try await holder.nextChange()
        XCTAssertEqual(initial?.reason, "initial")
        let aborted = Task { try await holder.nextChange() }
        try await Task.sleep(nanoseconds: 50_000_000)
        aborted.cancel()
        _ = try? await aborted.value
        let cancelled = Task { try await holder.nextChange() }
        try await Task.sleep(nanoseconds: 50_000_000)
        do { _ = try await holder.nextChange(); XCTFail("Expected concurrent wait error") } catch is BarkError {}
        holder.cancelNextChangeWait()
        let cancelledValue = try await cancelled.value
        XCTAssertNil(cancelledValue)
        for index in UInt32(0)...7 {
            let stored = try await wallet.deriveStoreNextKeypair()
            XCTAssertEqual(stored.index, index)
            if let vector = fixture.vectors.first(where: { $0.index == index }) {
                XCTAssertEqual(stored.publicKey, vector.walletPublicKey)
                let key = try await wallet.peekPublicKey(index: index)
                XCTAssertEqual(key, stored.publicKey)
                let signature = try await wallet.signMessage(message: fixture.message, index: index)
                XCTAssertEqual(signature, vector.walletSignature)
                XCTAssertTrue(try verifyMessage(message: fixture.message, signatureDerHex: signature, publicKeyHex: key))
                XCTAssertFalse(try verifyMessage(message: fixture.message + "!", signatureDerHex: signature, publicKeyHex: key))
            }
        }
        let changed = try await holder.nextChange()
        XCTAssertEqual(changed?.reason, "databaseChanged")
        let snapshot = try await wallet.createSnapshot(destinationPath: directory.appendingPathComponent("snapshot.sqlite").path)
        let validated = try await validateWalletSnapshot(path: snapshot.path, expected: nil)
        XCTAssertEqual(snapshot.sha256, validated.sha256)
        do { _ = try await wallet.createSnapshot(destinationPath: snapshot.path); XCTFail("Expected no overwrite") } catch is BarkError {}
        do { _ = try await validateWalletSnapshot(path: directory.appendingPathComponent("db.sqlite").path, expected: nil); XCTFail("Expected live database refusal") } catch is BarkError {}
        let stopped = Task { try await holder.nextChange() }
        try await Task.sleep(nanoseconds: 50_000_000)
        holder.stop()
        let stoppedValue = try await stopped.value
        XCTAssertNil(stoppedValue)
        XCTAssertFalse(holder.isActive())
    }
}

private func regtestCommand(_ arguments: String...) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/bash")
    process.arguments = [fixtures.deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("scripts/ark-dev.sh").path] + arguments
    let output = Pipe()
    process.standardOutput = output
    try process.run()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0 else { throw BarkError(message: "RegTest command failed") }
    return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
}

extension ContractTests {
    func testPureVtxoDecode() throws {
        let hex = try String(contentsOf: fixtures.appendingPathComponent("board-vtxo.hex"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let vtxo = try decodeVtxo(hexOrBase64: hex)
        XCTAssertEqual(vtxo.amountSats, 10000)
        XCTAssertEqual(vtxo.state, .unknown)
        XCTAssertFalse(vtxo.registered)
        XCTAssertEqual(vtxo.serverPubkey.count, 66)
        XCTAssertEqual(vtxo.exitDelta, 2016)
        XCTAssertEqual(vtxo.anchorPoint.txid.count, 64)
        XCTAssertEqual(vtxo.point.txid.count, 64)
        let bytes = stride(from: 0, to: hex.count, by: 2).map { offset -> UInt8 in
            let a = hex.index(hex.startIndex, offsetBy: offset)
            let b = hex.index(a, offsetBy: 2)
            return UInt8(hex[a..<b], radix: 16)!
        }
        XCTAssertEqual(try decodeVtxo(hexOrBase64: Data(bytes).base64EncodedString()).id, vtxo.id)
        XCTAssertThrowsError(try decodeVtxo(hexOrBase64: "invalid"))
        XCTAssertThrowsError(try parseArkAddress(address: "invalid"))
    }

    func testLiveNoahFeatureParity() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let env = ProcessInfo.processInfo.environment
        let config = Config(serverAddress: "http://127.0.0.1:\(env["ARK_PORT"] ?? "3635")", bitcoindAddress: "http://127.0.0.1:\(env["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark", vtxoExitMargin: 2)
        let mnemonic = try generateMnemonic()
        let onchain = try await OnchainWallet.default(network: .regtest, mnemonic: mnemonic, config: config, datadir: directory.path)
        let wallet = try await Wallet.openWithOnchain(network: .regtest, mnemonicOrSeed: mnemonic, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createWithoutServer: true, skipRecovery: true), onchain: onchain)
        let address = try await onchain.newAddress()
        let mine = try await onchain.isMine(address: address)
        XCTAssertTrue(mine)
        let foreign = try await onchain.isMine(address: "1BoatSLRHtKNngkdXEeobR76b53LETtpyT")
        XCTAssertFalse(foreign)
        do { _ = try await onchain.isMine(address: "invalid"); XCTFail("Expected address error") } catch is BarkError {}
        do { _ = try await onchain.drain(address: address, feeRateSatPerVb: UInt64.max); XCTFail("Expected fee overflow") } catch is BarkError {}
        _ = try regtestCommand("send-to", address, "0.005")
        _ = try await onchain.sync()
        let board = try await wallet.boardAmount(amountSats: 100000)
        XCTAssertFalse(board.vtxoIds.isEmpty)
        _ = try regtestCommand("generate", "6")
        try await wallet.sync()
        let wholeWallet = try await wallet.estimateEmergencyExitFee(vtxoIds: [], feeRateSatPerVb: 1, destination: nil)
        XCTAssertTrue(wholeWallet.excludedVtxos.isEmpty)
        let history = try await wallet.history()
        let movement = try XCTUnwrap(history.first)
        try await wallet.updateHistoryMetadata(movementId: movement.id, patchJson: "{\"note\":\"Noah Swift\"}")
        let updated = try await wallet.history().first { $0.id == movement.id }
        XCTAssertTrue(try XCTUnwrap(updated).metadataJson.contains("Noah Swift"))
        do { try await wallet.updateHistoryMetadata(movementId: movement.id, patchJson: "[]"); XCTFail("Expected patch validation") } catch is BarkError {}
        do {
            let recipient = try await Wallet.open(network: .regtest, mnemonicOrSeed: generateMnemonic(), config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.appendingPathComponent("recipient").path, createWithoutServer: true, skipRecovery: true))
            let receiveAddress = try await recipient.newAddress()
            _ = try await wallet.sendArkoorPayment(arkAddress: receiveAddress, amountSats: 10000)
            try await recipient.sync()
            let incomingHistory = try await recipient.history()
            let received = incomingHistory.flatMap(\.receivedOnAddresses).filter { $0.paymentMethod.value == receiveAddress }
            XCTAssertEqual(received.reduce(UInt64(0)) { $0 + $1.amountSats }, 10000)
            XCTAssertTrue(received.allSatisfy { !$0.paymentMethod.methodType.isEmpty })
        }
        let arkAddress = try await wallet.newAddressWithIndex()
        let parsed = try parseArkAddress(address: arkAddress.address)
        let key = try await wallet.peekPublicKey(index: arkAddress.index)
        XCTAssertEqual(parsed.userPubkey, key)
        XCTAssertFalse(parsed.arkId.isEmpty)
        let info = try await wallet.arkInfo()
        XCTAssertFalse(try XCTUnwrap(info).mailboxPubkey.isEmpty)
        let auth = try wallet.mailboxAuthorization(authorizationExpiry: 2000000000)
        XCTAssertEqual(auth.expiry, 2000000000)
        XCTAssertEqual(auth.mailboxId, try wallet.mailboxAuthorizationAt(authorizationExpiry: 2000000000).mailboxId)
        XCTAssertThrowsError(try wallet.mailboxAuthorizationAt(authorizationExpiry: Int64.max))
        let rounds = try await wallet.syncPendingRounds()
        XCTAssertTrue(rounds.isEmpty)
        let invoice = try await wallet.bolt11Invoice(amountSats: 2000, description: "Noah parity", token: nil)
        let pending = try await wallet.lightningReceiveState(paymentHash: invoice.paymentHash)
        XCTAssertEqual(pending.invoice, invoice.invoice)
        XCTAssertEqual(pending.amountSats, 2000)
        XCTAssertTrue(pending.htlcVtxoIds.isEmpty)
        XCTAssertNil(pending.movementId)
        try await wallet.cancelLightningReceive(paymentHash: invoice.paymentHash)
        let id = try XCTUnwrap(board.vtxoIds.first)
        try await wallet.startExitForVtxos(vtxoIds: [id])
        let possible = try await wallet.getExitStatus(vtxoId: id, includeHistory: true, includeTransactions: true)
        let status = try XCTUnwrap(possible)
        XCTAssertFalse(status.transactions.isEmpty)
        XCTAssertEqual(Int(status.transactionCount), status.transactions.count)
        XCTAssertTrue(status.transactions.allSatisfy { !$0.parentTxHex.isEmpty })
        let compact = try await wallet.getExitStatus(vtxoId: id, includeHistory: false, includeTransactions: false)
        XCTAssertTrue(try XCTUnwrap(compact).transactions.isEmpty)
        _ = try await wallet.cancelExit(vtxoId: id)
        let allExits = try await wallet.listAllExits(includeHistory: true, includeTransactions: true)
        let canceled = try XCTUnwrap(allExits.first { $0.vtxoId == id })
        XCTAssertGreaterThan(canceled.amountSats, 0)
        XCTAssertFalse(try XCTUnwrap(canceled.history).isEmpty)
        XCTAssertFalse(canceled.transactions.isEmpty)
        try await wallet.dangerousDropVtxo(id: id)
        do { _ = try await wallet.getVtxoById(vtxoId: id); XCTFail("Expected missing VTXO") } catch is BarkError {}
        do {
            _ = try await wallet.estimateEmergencyExitFee(vtxoIds: [id], feeRateSatPerVb: 1, destination: nil)
            XCTFail("Expected unknown exit VTXO")
        } catch let error as BarkError {
            XCTAssertEqual(error.code, .exitUnknownVtxo)
            XCTAssertEqual(error.vtxoId, id)
        }
        do {
            _ = try await wallet.estimateEmergencyExitFee(vtxoIds: [], feeRateSatPerVb: UInt64.max, destination: nil)
            XCTFail("Expected typed invalid fee rate")
        } catch let error as BarkError { XCTAssertEqual(error.code, .invalidFeeRate) }
        do { try await wallet.dangerousDropVtxo(id: "invalid"); XCTFail("Expected invalid VTXO") } catch is BarkError {}
        _ = try await onchain.sync()
        let destination = try regtestCommand("bcli", "getnewaddress")
        let owned = try await onchain.isMine(address: destination)
        XCTAssertFalse(owned)
        do {
            _ = try await onchain.prepareSend(address: destination, amount: .drainAll, feeRateSatPerVb: 0, confirmedOnly: true)
            XCTFail("Expected typed invalid fee rate")
        } catch let error as BarkError { XCTAssertEqual(error.code, .invalidFeeRate) }
        let preview = try await onchain.prepareSend(address: destination, amount: .drainAll, feeRateSatPerVb: 10, confirmedOnly: true)
        let beforeBroadcast = try await onchain.transactions()
        XCTAssertFalse(beforeBroadcast.contains { $0.txid == preview.txid })
        XCTAssertFalse(preview.inputs.isEmpty)
        try await onchain.discardPrepared(id: preview.id)
        let prepared = try await onchain.prepareSend(address: destination, amount: .drainAll, feeRateSatPerVb: 10, confirmedOnly: true)
        let broadcast = try await onchain.broadcastPrepared(id: prepared.id)
        XCTAssertEqual(broadcast.txid, prepared.txid)
        XCTAssertEqual(broadcast.broadcast, .accepted)
        let txid = broadcast.txid
        let transactions = try await onchain.transactions()
        XCTAssertTrue(transactions.contains { $0.txid == txid })
        XCTAssertEqual(transactions.first { $0.txid == txid }?.onchainFeeSats, prepared.feeSats)
        try await onchain.discardPrepared(id: prepared.id)
        _ = try regtestCommand("generate", "1")
        _ = try await onchain.sync()
        let balance = try await onchain.balance()
        XCTAssertEqual(balance.totalSats, 0)
        _ = try regtestCommand("send-to", address, "0.002")
        _ = try await onchain.sync()
        let discarded = try await wallet.prepareBoard(amount: .drainAll, feeRateSatPerVb: 1, confirmedOnly: true)
        try await wallet.discardPreparedBoard(id: discarded.id)
        let boardPreview = try await wallet.prepareBoard(amount: .drainAll, feeRateSatPerVb: 1, confirmedOnly: true)
        XCTAssertEqual(boardPreview.grossSats, boardPreview.netSats + boardPreview.arkFeeSats)
        let beforeBoard = try await wallet.transactionStatus(txid: boardPreview.txid)
        XCTAssertEqual(beforeBoard, .notFound)
        let all = try await wallet.submitPreparedBoard(id: boardPreview.id)
        XCTAssertEqual(all.txid, boardPreview.txid)
        XCTAssertEqual(all.broadcast, .accepted)
        XCTAssertFalse(all.vtxoIds.isEmpty)
        let boardStatus = try await onchain.transactionStatus(txid: all.txid)
        XCTAssertEqual(boardStatus, .inMempool)
        try await wallet.discardPreparedBoard(id: boardPreview.id)
        _ = try regtestCommand("generate", "6")
        try await wallet.sync()
    }
}

extension ContractTests {
    func testTypedExitErrors() {
        let funds = BarkError(message: "needs fees", code: .exitInsufficientConfirmedFunds, neededSats: 5000, availableSats: 1200)
        XCTAssertEqual(funds.neededSats, 5000)
        XCTAssertEqual(funds.availableSats, 1200)
        XCTAssertNil(funds.vtxoId)
        XCTAssertNil(BarkError(message: "plain").vtxoId)
        let progress: (ExitProgressStatus) -> BarkErrorCode? = { $0.error?.code }
        _ = progress
        let permanent: Set<BarkErrorCode> = [.exitNonStandardVtxo, .exitDustLimit, .exitVtxoAlreadySpent, .exitVtxoAlreadyExited, .exitVtxoSwept, .exitUnknownVtxo, .exitCyclicTransactions, .exitMissingAnchorOutput]
        let transient: Set<BarkErrorCode> = [.exitTransactionRetrievalFailure, .exitTipRetrievalFailure, .exitBlockRetrievalFailure, .exitAncestorRetrievalFailure]
        XCTAssertEqual(permanent.union(transient).count, 12)
    }

    func testExcludedExitVtxos() {
        let skipped = ExcludedExitVtxo(vtxoId: "ab:0", amountSats: 200, code: .exitNonStandardVtxo, message: "dust")
        let estimate = EmergencyExitFeeEstimate(exitBroadcastFeeSats: 1, claimFeeSats: 2, totalFeeSats: 3, feeRateSatPerVb: 1, txsToBroadcast: 1, excludedVtxos: [skipped])
        XCTAssertEqual(estimate.excludedVtxos.first?.amountSats, 200)
    }

    func testWalletSyncOnSwiftWorkerStack() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let env = ProcessInfo.processInfo.environment
        let config = Config(serverAddress: "http://127.0.0.1:\(env["ARK_PORT"] ?? "3635")", bitcoindAddress: "http://127.0.0.1:\(env["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark")
        let wallet = try await Wallet.open(network: .regtest, mnemonicOrSeed: generateMnemonic(), config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createWithoutServer: true, skipRecovery: true))
        try await wallet.sync()
    }
}

extension ContractTests {
    func testExistingWalletOpenRejectsWithoutWrites() async throws {
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        let config = Config(serverAddress: "http://127.0.0.1:1", esploraAddress: "http://127.0.0.1:1")
        for scenario in ["empty", "foreign", "network", "mnemonic", "missing"] {
            let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            defer { try? FileManager.default.removeItem(at: directory) }
            let path = directory.appendingPathComponent("db.sqlite")
            if scenario == "empty" { try Data().write(to: path) }
            else if scenario != "missing" {
                try FileManager.default.copyItem(at: fixtures.appendingPathComponent(scenario == "foreign" ? "foreign.sqlite" : "noah-v2-schema42.sqlite"), to: path)
            }
            func contents() throws -> [String: Data] {
                try Dictionary(uniqueKeysWithValues: FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil).map { ($0.lastPathComponent, try Data(contentsOf: $0)) })
            }
            let before = try contents()
            let network: Network = scenario == "network" ? .bitcoin : .regtest
            let seed = scenario == "mnemonic" ? try generateMnemonic() : mnemonic
            do { _ = try await OnchainWallet.openExisting(network: network, mnemonic: seed, config: config, datadir: directory.path); XCTFail("Expected \(scenario) rejection") } catch let error as BarkError { XCTAssertFalse(error.message.isEmpty); if scenario == "empty" { XCTAssertTrue(error.message.contains("empty or truncated")) }; if scenario == "foreign" { XCTAssertTrue(error.message.contains("initialized Bark wallet properties")) } }
            XCTAssertEqual(try contents(), before, "onchain: \(scenario)")
            do { _ = try await Wallet.open(network: network, mnemonicOrSeed: seed, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createIfNotExists: false)); XCTFail("Expected \(scenario) rejection") } catch let error as BarkError { XCTAssertFalse(error.message.isEmpty); if scenario == "empty" { XCTAssertTrue(error.message.contains("empty or truncated")) }; if scenario == "foreign" { XCTAssertTrue(error.message.contains("initialized Bark wallet properties")) } }
            XCTAssertEqual(try contents(), before, "wallet: \(scenario)")
        }
    }

    func testSchema42ExistingWalletOpens() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.copyItem(at: fixtures.appendingPathComponent("noah-v2-schema42.sqlite"), to: directory.appendingPathComponent("db.sqlite"))
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        let config = Config(serverAddress: "http://127.0.0.1:\(ProcessInfo.processInfo.environment["ARK_PORT"] ?? "3635")", bitcoindAddress: "http://127.0.0.1:\(ProcessInfo.processInfo.environment["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark")
        let onchain = try await OnchainWallet.openExisting(network: .regtest, mnemonic: mnemonic, config: config, datadir: directory.path)
        let wallet = try await Wallet.openWithOnchain(network: .regtest, mnemonicOrSeed: mnemonic, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createIfNotExists: false), onchain: onchain)
        XCTAssertFalse(wallet.fingerprint().isEmpty)
    }
}

private func checkBalance(_ balance: Balance, file: StaticString = #filePath, line: UInt = #line) {
    XCTAssertEqual(balance.pendingSats, balance.pendingInRoundSats + balance.pendingExitSats + balance.pendingLightningSendSats + balance.claimableLightningReceiveSats + balance.pendingBoardSats + balance.pendingArkoorSendSats + balance.pendingOffboardSats, file: file, line: line)
    XCTAssertEqual(balance.totalSats, balance.spendableSats + balance.needsRefreshSats + balance.pendingSats, file: file, line: line)
}

extension ContractTests {
    func testLiveFullBalanceBuckets() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let proxy = Process()
        proxy.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        proxy.arguments = ["python3", fixtures.deletingLastPathComponent().appendingPathComponent("support/ark-proxy.py").path]
        let output = Pipe(); proxy.standardOutput = output
        try proxy.run()
        defer { proxy.terminate(); proxy.waitUntilExit() }
        var portBytes = Data()
        while let byte = try output.fileHandleForReading.read(upToCount: 1), !byte.isEmpty, byte[0] != 10 { portBytes.append(byte) }
        let port = String(decoding: portBytes, as: UTF8.self)
        let env = ProcessInfo.processInfo.environment
        let config = Config(serverAddress: "http://127.0.0.1:\(port)", bitcoindAddress: "http://127.0.0.1:\(env["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark", vtxoExitMargin: 2)
        let mnemonic = try generateMnemonic()
        let onchain = try await OnchainWallet.default(network: .regtest, mnemonic: mnemonic, config: config, datadir: directory.path)
        let wallet = try await Wallet.openWithOnchain(network: .regtest, mnemonicOrSeed: mnemonic, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, createWithoutServer: true, skipRecovery: true), onchain: onchain)
        let recipient = try await Wallet.open(network: .regtest, mnemonicOrSeed: generateMnemonic(), config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.appendingPathComponent("recipient").path, createWithoutServer: true, skipRecovery: true))
        let fundingAddress = try await onchain.newAddress()
        _ = try regtestCommand("send-to", fundingAddress, "0.005")
        _ = try await onchain.sync()
        let boarded = try await wallet.boardAmount(amountSats: 100000)
        let board = try await wallet.balance()
        checkBalance(board)
        XCTAssertEqual(board.pendingBoardSats, boarded.amountSats)
        _ = try regtestCommand("generate", "6")
        try await wallet.sync()
        let funded = try await wallet.balance()
        checkBalance(funded)
        XCTAssertEqual(funded.spendableSats, boarded.amountSats)
        let receiveAddress = try await recipient.newAddress()
        let send = Task { try await wallet.sendArkoorPayment(arkAddress: receiveAddress, amountSats: 10000) }
        func waitForBucket(_ bucket: KeyPath<Balance, UInt64>) async throws -> Balance {
            for _ in 0..<300 {
                let value = try await wallet.balance()
                if value[keyPath: bucket] > 0 { checkBalance(value); return value }
                try await Task.sleep(nanoseconds: 20_000_000)
            }
            throw BarkError(message: "Pending balance bucket never appeared")
        }
        let sending = try await waitForBucket(\.pendingArkoorSendSats)
        XCTAssertEqual(sending.pendingArkoorSendSats, funded.totalSats)
        XCTAssertEqual(sending.spendableSats, 0)
        XCTAssertEqual(sending.totalSats, funded.totalSats)
        _ = try await send.value
        try await recipient.sync()
        let before = try await wallet.balance()
        let destination = try await onchain.newAddress()
        let offboard = Task { try await wallet.offboardAll(bitcoinAddress: destination) }
        let offboarding = try await waitForBucket(\.pendingOffboardSats)
        XCTAssertEqual(offboarding.pendingOffboardSats, before.totalSats)
        XCTAssertEqual(offboarding.spendableSats, 0)
        XCTAssertEqual(offboarding.totalSats, before.totalSats)
        _ = try await offboard.value
        let vtxos = try await recipient.spendableVtxos()
        let vtxo = try XCTUnwrap(vtxos.first)
        let tip = try XCTUnwrap(UInt32(regtestCommand("bcli", "getblockcount")))
        _ = try regtestCommand("generate", String(vtxo.expiryHeight - tip + 1))
        try await recipient.sync()
        let expired = try await recipient.balance()
        checkBalance(expired)
        XCTAssertEqual(expired.needsRefreshSats, 10000)
        XCTAssertEqual(expired.spendableSats, 0)
        XCTAssertEqual(expired.pendingSats, 0)
        XCTAssertEqual(expired.totalSats, 10000)
    }
}

extension ContractTests {
    func testLivePaymentRequestsAndLightningOptions() async throws {
        guard ProcessInfo.processInfo.environment["BARK_REGTEST"] == "1" else { throw XCTSkip("requires live RegTest") }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let env = ProcessInfo.processInfo.environment
        let config = Config(serverAddress: "http://127.0.0.1:\(env["ARK_PORT"] ?? "3635")", bitcoindAddress: "http://127.0.0.1:\(env["BITCOIN_PORT"] ?? "19443")", bitcoindUser: "second", bitcoindPass: "ark")
        let mnemonic = try generateMnemonic()
        let onchain = try await OnchainWallet.default(network: .regtest, mnemonic: mnemonic, config: config, datadir: directory.path)
        let wallet = try await Wallet.openWithOnchain(network: .regtest, mnemonicOrSeed: mnemonic, config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.path, skipRecovery: true), onchain: onchain)
        let recipient = try await Wallet.open(network: .regtest, mnemonicOrSeed: generateMnemonic(), config: config, args: WalletOpenArgs(runDaemon: false, datadir: directory.appendingPathComponent("recipient").path, skipRecovery: true))
        _ = try regtestCommand("send-to", try await onchain.newAddress(), "0.005")
        _ = try await onchain.sync()
        _ = try await wallet.boardAmount(amountSats: 200000)
        _ = try regtestCommand("generate", "6")
        try await wallet.sync()
        let info = try await wallet.debugInfo()
        XCTAssertEqual(info.network, .regtest)
        XCTAssertEqual(info.mailboxId, try wallet.mailboxIdentifier())
        do {
            _ = try await wallet.parsePaymentRequest(input: "not a payment")
            XCTFail("Expected an invalid payment request")
        } catch let error as BarkError { XCTAssertEqual(error.code, .invalidPaymentRequest) }
        let uri = try await recipient.bip321Uri(options: Bip321UriOptions(amountSats: 3000, label: "coffee"))
        let request = try await wallet.parsePaymentRequest(input: uri.uri)
        XCTAssertEqual(request.amountSats, 3000)
        XCTAssertEqual(request.options.map(\.method.methodType), ["invoice", "ark"])
        let option = try XCTUnwrap(request.defaultOption)
        XCTAssertEqual(option.method.value, uri.arkAddress)
        let fees = try await wallet.estimatePaymentFees(request: request, amountSats: nil)
        XCTAssertEqual(fees.count, 2)
        guard case .ark(let movementId) = try await wallet.sendPayment(method: option.method, amountSats: 3000) else { return XCTFail("Expected an Ark payment") }
        let direct = try await wallet.sendArkoorPayment(arkAddress: try XCTUnwrap(uri.arkAddress), amountSats: 1000)
        let ids = Set(try await wallet.history().map(\.id))
        XCTAssertTrue(ids.isSuperset(of: [movementId, direct]))
        let mainnet = try await wallet.parsePaymentRequest(input: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
        XCTAssertEqual(mainnet.options.first?.errors, [.networkMismatch])
        _ = try regtestCommand("wait-lightning-sync")
        let added = try JSONSerialization.jsonObject(with: Data(regtestCommand("lncli", "addinvoice", "--amt=2000").utf8)) as? [String: Any]
        let invoice = try XCTUnwrap(added?["payment_request"] as? String)
        let status = try await wallet.payLightningInvoice(invoice: invoice, amountSats: nil, options: LightningSendOptions(wait: true, retryForSecs: 30))
        guard case .paid = status else { return XCTFail("Expected a paid invoice, got \(status)") }
    }
}
