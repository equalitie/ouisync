package org.equalitie.ouisync.session

import kotlinx.coroutines.test.runTest
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.initLog
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.fail
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File
import java.io.IOException
import kotlin.io.path.createTempDirectory

class ClientTest {
    lateinit var tempDir: File

    @BeforeEach
    fun setup() {
        tempDir = File(createTempDirectory().toString())
        initLog()
    }

    @AfterEach
    fun teardown() {
        tempDir.deleteRecursively()
    }

    @Test
    fun disconnect() = runTest {
        val configDir = "$tempDir/config"
        val service = Service.start(configDir)
        val client = Client.connect(configDir)

        val response = client.invoke(Request.SessionGetStoreDirs)
        assertEquals(Response.Paths(emptyList()), response)

        service.stop()

        try {
            client.invoke(Request.SessionGetStoreDirs)
            fail("unexpected success")
        } catch (e: IOException) {}
    }
}
