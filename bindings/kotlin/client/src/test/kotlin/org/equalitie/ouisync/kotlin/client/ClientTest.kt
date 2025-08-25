package org.equalitie.ouisync.kotlin.client

import kotlinx.coroutines.channels.produce
import kotlinx.coroutines.test.runTest
import org.equalitie.ouisync.kotlin.server.Server
import org.equalitie.ouisync.kotlin.server.initLog
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Assertions.fail
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File
import java.io.EOFException
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
        val server = Server.start(configDir)
        val client = Client.connect(configDir)

        val response = client.invoke(Request.SessionGetStoreDir)
        assertEquals(Response.None, response)

        server.stop()

        try {
            client.invoke(Request.SessionGetStoreDir)
            fail("unexpected success")
        } catch (e: EOFException) {
        }
    }
}