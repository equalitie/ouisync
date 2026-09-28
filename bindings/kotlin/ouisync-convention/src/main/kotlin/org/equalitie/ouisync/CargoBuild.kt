package org.equalitie.ouisync

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.Property
import org.gradle.api.provider.ProviderFactory
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Internal
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
    /** The build tool to use: `cargo` or `cross`. Defaults to `cargo`. */
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
        // artifacts from plain `cargo build`. `cross` always needs it.
        val explicitTarget = tool != "cargo" || target != hostTarget.get()

        execOperations.exec {
            workingDir = workspaceDir
            executable = tool
            args("build", "--package", packageName.get(), "--lib")

            if (explicitTarget) {
                args("--target", target)
            }

            if (release) {
                args("--release")
            }
        }

        val targetDir =
            System.getenv("CARGO_TARGET_DIR")?.let { File(it) } ?: workspaceDir.resolve("target")
        val profileDir =
            (if (explicitTarget) targetDir.resolve(target) else targetDir).resolve(
                if (release) "release" else "debug",
            )

        val fileName = RustTarget.libraryFileName(target, libraryName.get())
        val srcFile = profileDir.resolve(fileName)

        if (!srcFile.exists()) {
            throw GradleException("Built library not found at '$srcFile'")
        }

        val outputDir = outputDir.get().asFile
        outputDir.deleteRecursively()
        outputDir.mkdirs()

        srcFile.copyTo(outputDir.resolve(fileName))
    }
}
