package org.equalitie.ouisync

import org.gradle.api.provider.Provider
import org.gradle.api.provider.ProviderFactory

/** Helpers for working with rust target triples. */
object RustTarget {
    /** Target triple of the host machine, as reported by `rustc -vV`. Evaluated lazily. */
    @JvmStatic
    fun host(providers: ProviderFactory): Provider<String> = providers
        .exec { commandLine("rustc", "-vV") }
        .standardOutput
        .asText
        .map { output ->
            output
                .lineSequence()
                .map { it.trim() }
                .firstOrNull { it.startsWith("host:") }
                ?.removePrefix("host:")
                ?.trim()
                ?: error("Failed to determine the host target from the output of `rustc -vV`")
        }

    /** File name of a dynamic library called [name] built for the given target. */
    @JvmStatic
    fun libraryFileName(target: String, name: String): String = when {
        target.contains("-windows") -> "$name.dll"
        target.contains("-apple-") -> "lib$name.dylib"
        else -> "lib$name.so"
    }

    private val androidAbis =
        mapOf(
            "aarch64-linux-android" to "arm64-v8a",
            "armv7-linux-androideabi" to "armeabi-v7a",
            "x86_64-linux-android" to "x86_64",
            "i686-linux-android" to "x86",
        )

    /** Android ABI (e.g., `arm64-v8a`) corresponding to the given target triple. */
    @JvmStatic
    fun toAndroidAbi(target: String): String = androidAbis[target] ?: error("Unsupported android target: $target")

    /**
     * Resource path prefix under which JNA looks for native libraries for the given target when
     * loading them from the classpath (see `com.sun.jna.Platform.RESOURCE_PREFIX`).
     */
    @JvmStatic
    fun jnaResourcePrefix(target: String): String {
        val arch = target.substringBefore('-')
        val os =
            when {
                target.contains("-linux-") -> "linux"
                target.contains("-apple-darwin") -> "darwin"
                target.contains("-windows") -> "win32"
                else -> error("Unsupported target for JNA: $target")
            }
        val jnaArch =
            when (arch) {
                "x86_64" -> "x86-64"
                "aarch64" -> "aarch64"
                else -> error("Unsupported target for JNA: $target")
            }

        return "$os-$jnaArch"
    }
}
