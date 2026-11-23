package org.equalitie.ouisync.session

import kotlinx.coroutines.async
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
import java.net.InetAddress
import java.net.ServerSocket
import kotlin.concurrent.thread
import kotlin.io.path.createTempDirectory
import kotlin.time.Duration.Companion.seconds

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

    // While connecting, the client re-reads the endpoint file on every attempt, so it connects to
    // the port the service actually listens on even when the file was stale at first.
    @OptIn(kotlin.ExperimentalStdlibApi::class)
    @Test
    fun connectWithStaleEndpoint() = runTest {
        val configDir = "$tempDir/config"
        File(configDir).mkdirs()

        // Port 0 can't be connected to. The service binds to a random port instead and writes it to
        // the file (keeping the auth key).
        val authKey = ByteArray(32) { it.toByte() }.toHexString()
        File("$configDir/local_endpoint.conf").writeText("""{"port":0,"auth_key":"$authKey"}""")

        val client = async { Client.connect(configDir, timeout = 10.seconds) }
        // Let the client read the (stale) endpoint file before the service rewrites it.
        testScheduler.runCurrent()

        val service = Service.start(configDir)

        try {
            val response = client.await().invoke(Request.SessionGetStoreDirs)
            assertEquals(Response.Paths(emptyList()), response)
        } finally {
            service.stop()
        }
    }

    // Connecting before the service has been started for the first time (so the endpoint file
    // doesn't exist yet) waits for the service.
    @Test
    fun connectBeforeServiceStarted() = runTest {
        val configDir = "$tempDir/config"

        val client = async { Client.connect(configDir, timeout = 10.seconds) }
        testScheduler.runCurrent()

        val service = Service.start(configDir)

        try {
            val response = client.await().invoke(Request.SessionGetStoreDirs)
            assertEquals(Response.Paths(emptyList()), response)
        } finally {
            service.stop()
        }
    }

    // A connection closed by the service during authentication (e.g. because the service is being
    // stopped) is a connection error, so it's retried like any other.
    @OptIn(kotlin.ExperimentalStdlibApi::class)
    @Test
    fun connectionClosedDuringAuthentication() = runTest {
        val configDir = "$tempDir/config"
        File(configDir).mkdirs()

        // Fake service which reads the client's challenge and then closes the connection. Reading it
        // first makes the close clean, so the client sees the end of the stream (not a reset).
        val server = ServerSocket(0, 50, InetAddress.getLoopbackAddress())
        val serverThread = thread {
            try {
                while (true) {
                    server.accept().use { socket -> socket.getInputStream().readNBytes(256) }
                }
            } catch (e: IOException) {}
        }

        val authKey = ByteArray(32) { it.toByte() }.toHexString()
        File("$configDir/local_endpoint.conf")
            .writeText("""{"port":${server.localPort},"auth_key":"$authKey"}""")

        try {
            Client.connect(configDir, timeout = 1.seconds)
            fail("unexpected success")
        } catch (e: IOException) {} finally {
            server.close()
            serverThread.join()
        }
    }
}
