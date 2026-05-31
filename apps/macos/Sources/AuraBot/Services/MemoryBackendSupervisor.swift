import Foundation
import Darwin

actor MemoryBackendSupervisor {
    private let config: MemoryConfig
    private var process: Process?
    private var lastLaunchAttempt: Date?

    init(config: MemoryConfig) {
        self.config = config
    }

    func start() async -> Bool {
        if await isHealthy() {
            return true
        }

        if let process, process.isRunning {
            return await waitForHealth()
        }

        if let lastLaunchAttempt, Date().timeIntervalSince(lastLaunchAttempt) < 10 {
            return false
        }
        lastLaunchAttempt = Date()

        guard let launch = resolveLaunchConfiguration() else {
            print("Memory backend is not bundled and no development service path was found.")
            return false
        }

        do {
            let process = Process()
            process.executableURL = launch.executableURL
            process.arguments = launch.arguments
            process.currentDirectoryURL = launch.workingDirectory
            process.environment = launch.environment

            let logHandle = try makeLogHandle()
            process.standardOutput = logHandle
            process.standardError = logHandle

            try process.run()
            self.process = process
            return await waitForHealth()
        } catch {
            print("Failed to launch memory backend: \(error)")
            return false
        }
    }

    func stop() async {
        guard let process, process.isRunning else {
            self.process = nil
            return
        }

        process.terminate()
        let exited = await waitForExit(process, timeoutSeconds: 5)
        if !exited, process.isRunning {
            kill(process.processIdentifier, SIGKILL)
            _ = await waitForExit(process, timeoutSeconds: 2)
        }
        self.process = nil
    }

    private func waitForExit(_ process: Process, timeoutSeconds: TimeInterval) async -> Bool {
        if !process.isRunning {
            return true
        }

        return await withCheckedContinuation { continuation in
            let box = ProcessWaitContinuationBox(continuation)
            DispatchQueue.global(qos: .utility).async {
                process.waitUntilExit()
                box.resume(true)
            }
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeoutSeconds) {
                box.resume(false)
            }
        }
    }

    private func waitForHealth() async -> Bool {
        for _ in 0..<40 {
            if await isHealthy() {
                return true
            }
            try? await Task.sleep(nanoseconds: 250_000_000)
        }
        return false
    }

    private func isHealthy() async -> Bool {
        guard let url = URL(string: "\(config.baseURL)/v2/health") else {
            return false
        }

        var request = URLRequest(url: url)
        request.timeoutInterval = 1

        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            guard let httpResponse = response as? HTTPURLResponse else {
                return false
            }
            return (200..<300).contains(httpResponse.statusCode)
        } catch {
            return false
        }
    }

    private func resolveLaunchConfiguration() -> MemoryBackendLaunchConfiguration? {
        guard let baseURL = URL(string: config.baseURL),
              let port = baseURL.port ?? defaultPort(for: baseURL.scheme) else {
            return nil
        }

        let candidates = bundledServiceDirectories() + developmentServiceDirectories()
        for serviceDirectory in candidates {
            let serverURL = serviceDirectory
                .appendingPathComponent("dist", isDirectory: true)
                .appendingPathComponent("src", isDirectory: true)
                .appendingPathComponent("server.js")

            if FileManager.default.fileExists(atPath: serverURL.path),
               let nodeURL = resolveNodeExecutable(serviceDirectory: serviceDirectory) {
                return MemoryBackendLaunchConfiguration(
                    executableURL: nodeURL.executableURL,
                    arguments: nodeURL.arguments + [serverURL.path],
                    workingDirectory: serviceDirectory,
                    environment: environment(port: port)
                )
            }

            let packageJSONURL = serviceDirectory.appendingPathComponent("package.json")
            if FileManager.default.fileExists(atPath: packageJSONURL.path),
               let npmURL = resolveNPMExecutable() {
                return MemoryBackendLaunchConfiguration(
                    executableURL: npmURL.executableURL,
                    arguments: npmURL.arguments + ["run", "serve"],
                    workingDirectory: serviceDirectory,
                    environment: environment(port: port)
                )
            }
        }

        return nil
    }

    private func bundledServiceDirectories() -> [URL] {
        [
            Bundle.main.resourceURL?.appendingPathComponent("MemoryPglite", isDirectory: true)
        ].compactMap { $0 }
    }

    private func developmentServiceDirectories() -> [URL] {
        let sourceFile = URL(fileURLWithPath: #filePath)
        let repoRoot = sourceFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()

        return [
            repoRoot
                .appendingPathComponent("services", isDirectory: true)
                .appendingPathComponent("memory-pglite", isDirectory: true)
        ]
    }

    private func resolveNodeExecutable(serviceDirectory: URL) -> (executableURL: URL, arguments: [String])? {
        let bundledNode = serviceDirectory
            .appendingPathComponent("node", isDirectory: true)
            .appendingPathComponent("bin", isDirectory: true)
            .appendingPathComponent("node")

        if nodeExecutableWorks(bundledNode, arguments: []) {
            return (bundledNode, [])
        }

        for path in ["/opt/homebrew/bin/node", "/usr/local/bin/node", "/usr/bin/node"] {
            let url = URL(fileURLWithPath: path)
            if nodeExecutableWorks(url, arguments: []) {
                return (url, [])
            }
        }

        let envURL = URL(fileURLWithPath: "/usr/bin/env")
        if nodeExecutableWorks(envURL, arguments: ["node"]) {
            return (envURL, ["node"])
        }

        return nil
    }

    private func nodeExecutableWorks(_ executableURL: URL, arguments: [String]) -> Bool {
        guard FileManager.default.isExecutableFile(atPath: executableURL.path) else {
            return false
        }

        let process = Process()
        process.executableURL = executableURL
        process.arguments = arguments + ["--version"]
        process.standardOutput = Pipe()
        process.standardError = Pipe()

        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        } catch {
            return false
        }
    }

    private func resolveNPMExecutable() -> (executableURL: URL, arguments: [String])? {
        for path in ["/opt/homebrew/bin/npm", "/usr/local/bin/npm", "/usr/bin/npm"] {
            if FileManager.default.isExecutableFile(atPath: path) {
                return (URL(fileURLWithPath: path), [])
            }
        }

        if FileManager.default.isExecutableFile(atPath: "/usr/bin/env") {
            return (URL(fileURLWithPath: "/usr/bin/env"), ["npm"])
        }

        return nil
    }

    private func environment(port: Int) -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        let auraHome = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aurabot", isDirectory: true)

        environment["AURABOT_HOME"] = auraHome.path
        environment["AURABOT_PGLITE_DIR"] = auraHome
            .appendingPathComponent("pglite", isDirectory: true)
            .appendingPathComponent("aurabot", isDirectory: true)
            .path
        environment["AURABOT_BRAIN_DIR"] = auraHome
            .appendingPathComponent("brain", isDirectory: true)
            .path
        environment["AURABOT_MEMORY_PGLITE_HOST"] = "127.0.0.1"
        environment["AURABOT_MEMORY_PGLITE_PORT"] = String(port)
        environment["AURABOT_MEMORY_USER_ID"] = config.userID
        environment["PATH"] = [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
            environment["PATH"] ?? ""
        ].joined(separator: ":")

        return environment
    }

    private func makeLogHandle() throws -> FileHandle {
        let logDirectory = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("Logs", isDirectory: true)
            .appendingPathComponent("AuraBot", isDirectory: true)
        try FileManager.default.createDirectory(at: logDirectory, withIntermediateDirectories: true)

        let logURL = logDirectory.appendingPathComponent("memory-pglite.log")
        if !FileManager.default.fileExists(atPath: logURL.path) {
            FileManager.default.createFile(atPath: logURL.path, contents: nil)
        }

        let handle = try FileHandle(forWritingTo: logURL)
        try handle.seekToEnd()
        return handle
    }

    private func defaultPort(for scheme: String?) -> Int? {
        switch scheme {
        case "https":
            return 443
        default:
            return nil
        }
    }
}

private final class ProcessWaitContinuationBox: @unchecked Sendable {
    private let lock = NSLock()
    private var didResume = false
    private let continuation: CheckedContinuation<Bool, Never>

    init(_ continuation: CheckedContinuation<Bool, Never>) {
        self.continuation = continuation
    }

    func resume(_ value: Bool) {
        lock.lock()
        defer { lock.unlock() }

        guard !didResume else { return }
        didResume = true
        continuation.resume(returning: value)
    }
}

private struct MemoryBackendLaunchConfiguration {
    let executableURL: URL
    let arguments: [String]
    let workingDirectory: URL
    let environment: [String: String]
}
