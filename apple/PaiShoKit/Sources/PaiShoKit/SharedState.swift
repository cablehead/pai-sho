import Foundation
import Security

/// The app's pai-sho key and peer list, copied into a keychain group the
/// share extension can read too, so the extension can run pai-sho as this
/// device: the same key, dialing the same peers. The app saves it whenever
/// it starts or goes to the background; the extension restores it into its
/// own container before it starts pai-sho.
///
/// A keychain group needs no registration with Apple, unlike an app group:
/// it only needs `keychain-access-groups` in both targets' entitlements, and
/// the group's full name, with the team prefix, in `PaiShoKeychainGroup` in
/// both Info.plists.
public enum SharedState {
    static let service = "stream.cross.pai-sho.state"
    /// The key file and what pai-sho keeps beside it, `<key>.<what>.json`.
    static let files = ["key", "key.peers.json", "key.grants.json", "key.surfaces.json", "key.ports.json"]

    static var group: String? {
        Bundle.main.object(forInfoDictionaryKey: "PaiShoKeychainGroup") as? String
    }

    static func query() -> [String: Any] {
        var q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "state",
        ]
        if let group, !group.hasPrefix("$(") { q[kSecAttrAccessGroup as String] = group }
        return q
    }

    /// Copy the files in `dir` into the keychain. Returns the keychain's
    /// status, `errSecSuccess` when saved.
    @discardableResult
    public static func save(from dir: URL) -> OSStatus {
        var state: [String: String] = [:]
        for name in files {
            if let data = try? Data(contentsOf: dir.appendingPathComponent(name)) {
                state[name] = data.base64EncodedString()
            }
        }
        guard state["key"] != nil, let data = try? JSONSerialization.data(withJSONObject: state) else {
            return errSecItemNotFound
        }
        SecItemDelete(query() as CFDictionary)
        var add = query()
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
        return SecItemAdd(add as CFDictionary, nil)
    }

    /// Write what the app saved into `dir`. False when the app has saved
    /// nothing yet, which is the case until Pai Sho has been opened once.
    public static func restore(to dir: URL) -> Bool {
        var q = query()
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        guard SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess,
              let data = out as? Data,
              let state = try? JSONSerialization.jsonObject(with: data) as? [String: String],
              state["key"] != nil
        else { return false }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        for (name, text) in state where files.contains(name) {
            guard let bytes = Data(base64Encoded: text) else { continue }
            try? bytes.write(to: dir.appendingPathComponent(name), options: .atomic)
        }
        return true
    }
}
