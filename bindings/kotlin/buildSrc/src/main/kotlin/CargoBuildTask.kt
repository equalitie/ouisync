import java.io.File
import org.gradle.api.DefaultTask
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.TaskAction

// Task to build rust library using cargo
abstract class CargoBuildTask : DefaultTask() {
    @get:Input
    abstract val command: Property<String>

    @get:Input
    abstract val workingDir: Property<File>

    @get:Input @get:Optional
    abstract val target: Property<String>

    @get:Input @get:Optional
    abstract val crate: Property<String>

    @get:Input
    abstract val libname: Property<String>

    @get:Input
    abstract val apiLevel: Property<Int>


    init {
        command.convention("cargo")
        workingDir.convention(project.rootDir)
        apiLevel.convention(33)
    }

    @TaskAction
    fun build() {
        val workingDir = workingDir.get()
        val command = command.get()
        val crate = crate.getOrNull()
        val target = target.getOrNull()
        val apiLevel = apiLevel.get()

        project.exec { spec ->
            spec.workingDir = workingDir
            spec.commandLine(command)

            spec.args("build", "--lib", "--release")

            if (target != null) {
                spec.args("--target", target)
            }

            if (crate != null) {
                spec.args("--package", crate)
            }

            if (target != null && target.contains("android")) {
                val compilerTriple = compilerTriple(target)

                val cc = "${compilerTriple}${apiLevel}-clang"
                val cxx = "${compilerTriple}${apiLevel}-clang++"
                val ar = "llvm-ar"
                val cargoTarget = target.replace("-", "_").uppercase()

                spec.environment("CC_${target}", cc)
                spec.environment("CXX_${target}", cxx)
                spec.environment("AR_${target}", ar)
                spec.environment("CARGO_TARGET_${cargoTarget}_LINKER", cc)
            }

        }.assertNormalExitValue()

        val targetDir = targetDir()
        val outputDir = project.layout.buildDirectory.dir("native").get()
        val libname = libname.get()

        project.copy { spec ->
            spec.from(targetDir).apply {
                include("lib${libname}.so")
                include("lib${libname}.dylib")
                include("${libname}.dll")
            }

            spec.into(outputDir)
        }
    }

    private fun targetDir(): String {
        val output = StringBuilder()
            .append(workingDir.get())
            .append("/")
            .append("target")

        if (target.isPresent) {
            output.append("/").append(target.get())
        }

        output.append("/").append("release")

        return output.toString()
    }
}

private fun compilerTriple(target: String) = when (target) {
    "armv7-linux-androideabi" -> "armv7a-linux-androideabi"
    else -> target
}
