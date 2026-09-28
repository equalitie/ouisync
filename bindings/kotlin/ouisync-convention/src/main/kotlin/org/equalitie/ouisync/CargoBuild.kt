package org.equalitie.ouisync

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.Property
import org.gradle.api.provider.ProviderFactory
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.File
import javax.inject.Inject

/**
 * Builds a dynamic library from a rust crate for a single target and copies it into [outputDir].
 */
abstract class CargoBuild
@Inject
constructor(
    private val execOperations: ExecOperations,
    providers: ProviderFactory,
) : DefaultTask() {
    /** The build tool to use: `cargo`, `cross` or `cargo-ndk`. Defaults to `cargo`. */
    @get:Input abstract val tool: Property<String>

    /** Target triple to build for. Defaults to the host target. */
    @get:Input abstract val target: Property<String>

    /** Name of the cargo package to build. */
    @get:Input abstract val packageName: Property<String>

    /** Name of the library (without the platform specific prefix and suffix). */
    @get:Input abstract val libraryName: Property<String>

    /** Whether to build with the release profile (otherwise debug). Defaults to `true`. */
    @get:Input abstract val release: Property<Boolean>

    /** Root directory of the cargo workspace. */
    @get:Internal abstract val workspaceDir: DirectoryProperty

    /** Directory where the built library is copied to. */
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    /** Android API level to build for. Used only with `cargo-ndk`. */
    @get:Input @get:Optional
    abstract val androidApiLevel: Property<Int>

    /** Android NDK directory. Used only with `cargo-ndk`. Resolved only when the task runs. */
    @get:Internal abstract val ndkDir: DirectoryProperty

    /** Target triple of the host machine. Detected automatically, normally doesn't need to be set. */
    @get:Internal abstract val hostTarget: Property<String>

    init {
        group = "rust"

        tool.convention("cargo")
        hostTarget.convention(RustTarget.host(providers))
        target.convention(hostTarget)
        release.convention(true)

        doNotTrackState("state is tracked by cargo")
    }

    @TaskAction
    fun build() {
        val tool = tool.get()
        val target = target.get()
        val release = release.get()
        val workspaceDir = workspaceDir.get().asFile

        // Pass `--target` only when needed so that building for the host target reuses the
        // artifacts from plain `cargo build`. `cross` and `cargo-ndk` always need it.
        val explicitTarget = tool != "cargo" || target != hostTarget.get()

        // cargo-ndk strips the library when copying it into its output dir, so take it from there
        // instead of from the cargo target dir.
        val ndkOutputDir = temporaryDir.resolve("ndk")
        ndkOutputDir.deleteRecursively()

        execOperations.exec {
            workingDir = workspaceDir

            if (tool == "cargo-ndk") {
                // cargo-ndk passes the target to cargo itself.
                executable = "cargo"
                args("ndk", "--target", target, "--output-dir", ndkOutputDir.absolutePath)

                if (androidApiLevel.isPresent) {
                    args("--platform", androidApiLevel.get().toString())
                }

                if (ndkDir.isPresent) {
                    // cargo-ndk uses the first of these that is set, but warns if the others are
                    // set to something else (e.g., the environment points to a different NDK than
                    // the one configured in the build). Set all of them to avoid that.
                    val ndkPath = ndkDir.get().asFile.absolutePath
                    for (
                    name in
                    listOf("ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK_PATH", "NDK_HOME")
                    ) {
                        environment(name, ndkPath)
                    }
                }

                args("build")
            } else {
                executable = tool
                args("build")

                if (explicitTarget) {
                    args("--target", target)
                }
            }

            args("--package", packageName.get(), "--lib")

            if (release) {
                args("--release")
            }
        }

        val fileName = RustTarget.libraryFileName(target, libraryName.get())
        val srcFile =
            if (tool == "cargo-ndk") {
                ndkOutputDir.resolve(RustTarget.toAndroidAbi(target)).resolve(fileName)
            } else {
                val targetDir =
                    System.getenv("CARGO_TARGET_DIR")?.let { File(it) } ?: workspaceDir.resolve("target")

                (if (explicitTarget) targetDir.resolve(target) else targetDir)
                    .resolve(if (release) "release" else "debug")
                    .resolve(fileName)
            }

        if (!srcFile.exists()) {
            throw GradleException("Built library not found at '$srcFile'")
        }

        val outputDir = outputDir.get().asFile
        outputDir.deleteRecursively()
        outputDir.mkdirs()

        srcFile.copyTo(outputDir.resolve(fileName))
    }
}
