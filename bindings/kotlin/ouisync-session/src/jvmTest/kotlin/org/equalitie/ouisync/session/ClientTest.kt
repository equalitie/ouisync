package org.equalitie.ouisync.session

import kotlinx.coroutines.test.runTest
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.initLog
import java.io.File
import java.io.IOException
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.fail

class ClientTest {
    lateinit var tempDir: File

    @BeforeTest
    fun setup() {
        tempDir = File(createTempDirectory().toString())
        initLog()
    }

    @AfterTest
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
