package org.equalitie.ouisync_kotlin

import java.io.File
import kotlin.io.path.createTempDirectory
import kotlin.test.Test
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.assertTrue

class SessionTest {
    lateinit var tempDir: File

    @BeforeTest
    fun setup() {
        tempDir = File(createTempDirectory().toString())
    }

    @AfterTest
    fun teardown() {
        tempDir.deleteRecursively()
    }

    @Test
    fun createAndDispose() {
        try {
            val session = Session.create(
                configsPath = "${tempDir}/config",
                logPath = "${tempDir}/test.log",
            );

            session.dispose()
        } catch (e: Exception) {
            e.printStackTrace()
            throw e
        } catch (e: java.lang.Error) {
            e.printStackTrace()
            throw e
        }
    }
}
