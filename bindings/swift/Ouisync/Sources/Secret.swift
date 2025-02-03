import CryptoKit
import Foundation
import MessagePack

/** `Secret` is used to encrypt and decrypt "global" read and write keys stored inside repositories
 * which are consequently used to encrypt, decrypt and sign repository data.
 *
 * There may be two `Secret`s, one for decrypting the global read and one decrypting the global
 * write keys. Note the that decrypting the global write key will enable repository reading as well
 * because the global read key is derived from the global write key.
 *
 * When opening a repository with a `Secret` the library will attempt to gain the highest possible
 * access. That is, it will use the local secret to decrypt the global write key first and, if that
 * fails, it'll attempt to decrypt the global read key.
 *
 * `Secret` can be either a `Password` or a `SecretKey`. In case a `Password` is provided to the
 * library, it is internally converted to `SecretKey` using a KDF and a `Salt`.  Ouisync uses two
 * `Salt`s: one for the "read" and one for the "write" local secret keys and they are stored inside
 * each repository database individually.
 *
 * Since secrets should not be logged by default, we require (but provide a default implementation
 * for) `CustomDebugStringConvertible` conformance.
 */
public protocol Secret: CustomDebugStringConvertible {}
public extension Secret {
    var debugDescription: String { "\(Self.self)(***)" }
}
/// A secret that can be passed to `createRepository()` & friends
public protocol CreateSecret: Secret {
    var value: MessagePackValue { get }
}
/// A secret that can be passed to `openRepository()` & friends
public protocol OpenSecret: Secret {
    var value: MessagePackValue { get }
}

public struct Password: Secret, CreateSecret, OpenSecret {
    let string: String
    public var value: MessagePackValue { ["password": .string(string)] }
    public init(_ value: String) { string = value }
}

public struct SecretKey: Secret, OpenSecret {
    let bytes: Data
    public var value: MessagePackValue { ["secret_key": .binary(bytes)] }
    public init(_ value: Data) { bytes = value }
    /// Generates a random 256-bit key as required by the ChaCha20 implementation Ouisync is using.
    public static var random: Self { get throws { try Self(.secureRandom(32)) } }
}

public struct Salt: Secret {
    let bytes: Data
    public var value: MessagePackValue { .binary(bytes) }
    public init(_ value: Data) {bytes = value }
    /// Generates a random 128-bit nonce as recommended by the Argon2 KDF used by Ouisync.
    public static var random: Self { get throws { try Self(.secureRandom(16)) } }
}

public struct SaltedSecretKey: Secret, CreateSecret {
    public let key: SecretKey
    public let salt: Salt
    public var value: MessagePackValue { ["key_and_salt": ["key": .binary(key.bytes),
                                                           "salt": .binary(salt.bytes)]] }
    public init(_ key: SecretKey, _ salt: Salt) { self.key = key; self.salt = salt }

    /// Generates a random 256-bit key and a random 128-bit salt
    public static var random: Self { get throws { try Self(.random, .random) } }
}


extension Data {
    /// Returns `size` random bytes generated using a cryptographically secure algorithm
    static func secureRandom(_ size: Int) throws -> Self {
        guard let buff = malloc(size) else {
            throw CryptoKitError.underlyingCoreCryptoError(error: errSecMemoryError)
        }
        switch SecRandomCopyBytes(kSecRandomDefault, size, buff) {
        case errSecSuccess:
            return Data(bytesNoCopy: buff, count: size, deallocator: .free)
        case let code:
            free(buff)
            throw CryptoKitError.underlyingCoreCryptoError(error: code)
        }
    }
}


public extension Client {
    /// Remotely generate a password salt
    func generateSalt() async throws -> Salt {
        try await Salt(invoke("password_generate_salt").dataValue.orThrow)
    }

    /// Remotely derive a `SecretKey` from `password` and `salt` using a secure KDF
    func deriveSecretKey(from password: Password, with salt: Salt) async throws -> SaltedSecretKey {
        let key = try await SecretKey(invoke("password_derive_secret_key",
                                      with: ["password": .string(password.string),
                                             "salt": salt.value]).dataValue.orThrow)
        return SaltedSecretKey(key, salt)
    }
}
