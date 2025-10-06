package org.equalitie.ouisync

import com.android.build.api.dsl.ApplicationExtension
import com.android.build.api.dsl.LibraryExtension
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.kotlin.dsl.findByType
import java.io.File
import kotlin.text.lineSequence

class ConventionPlugin : Plugin<Project> {
    override fun apply(target: Project) {
        val file = findFile(target.projectDir, "ndk-version.txt")
        if (file == null) {
            target.logger.info("${target.name}: Not setting NDK version because the NDK version file is missing")
            return
        }

        val commonNdkVersion = parseVersion(file.readText())
        if (commonNdkVersion == null) {
            error("NDK version file is invalid: '$file'")
        }

        target.logger.info("${target.name}: Setting NDK version to $commonNdkVersion according to the NDK version file '$file'")

        with(target) {
            with(pluginManager) {
                extensions.findByType<ApplicationExtension>()?.apply { ndkVersion = commonNdkVersion }
                extensions.findByType<LibraryExtension>()?.apply { ndkVersion = commonNdkVersion }
            }
        }
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
