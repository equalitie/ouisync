package org.equalitie.ouisync

import com.android.build.gradle.BaseExtension
import com.android.build.gradle.BasePlugin
import com.android.build.gradle.LibraryPlugin
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.findByType
import java.io.File
import kotlin.text.lineSequence

class ConventionPlugin : Plugin<Project> {
    override fun apply(target: Project): Unit = with(target) {
        plugins.withType(BasePlugin::class.java) {
            configure(extensions.getByType(BaseExtension::class.java))
        }
    }

    private fun Project.configure(extension: BaseExtension) {
        val fileName = "ndk-version.txt"
        val file = findFile(projectDir, fileName)
        if (file == null) {
            logger.warn(
                "$name: Not setting NDK version because the NDK version file '$fileName' is missing in '$projectDir' or any of its ancestors",
            )
            return
        }

        val commonNdkVersion = parseVersion(file.readText())
        if (commonNdkVersion == null) {
            error("NDK version file is invalid: '$file'")
        }

        logger.info(
            "$name: Setting NDK version to $commonNdkVersion according to the NDK version file '$file'",
        )

        extension.ndkVersion = commonNdkVersion
    }
}

private fun parseVersion(input: String): String? = input
    .lineSequence()
    .map { it.trim() }
    .filterNot { it.isEmpty() || it.startsWith('#') }
    .firstOrNull()

private fun findFile(start: File, name: String): File? {
    var dir: File? = start

    while (dir != null) {
        val file = File(dir, name)

        if (file.exists()) {
            return file
        } else {
            dir = dir.parentFile
        }
    }

    return null
}
