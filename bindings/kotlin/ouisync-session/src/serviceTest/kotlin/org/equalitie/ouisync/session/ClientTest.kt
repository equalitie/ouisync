package org.equalitie.ouisync.session

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
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
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
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

    @Test
    fun customUnixSocketPath() = runTest {
        // Unix domain sockets are not supported by the service on Windows.
        if (System.getProperty("os.name").startsWith("Windows")) {
            return@runTest
        }

        val configDir = File(tempDir, "config").apply { mkdirs() }
        val socketDir = File(tempDir, "sockets").apply { mkdirs() }

        File(configDir, "local_endpoint.conf")
            .writeText("\"unix://${File(socketDir, "ouisync.sock").path}\"")

        val service = Service.start(configDir.path)

        try {
            val client = Client.connect(configDir.path)

            try {
                assertEquals(Response.Paths(emptyList()), client.invoke(Request.SessionGetStoreDirs))
            } finally {
                client.close()
            }
        } finally {
            service.stop()
        }

        assertFalse(File(configDir, "local_endpoint.sock").exists())
    }

    @Test
    fun connectTcpInvalidPort() = runTest {
        val configDir = File(tempDir, "config").apply { mkdirs() }
        val authKey = "00".repeat(32)

        for (endpoint in listOf("127.0.0.1", "127.0.0.1:0", "[::1]", "[::1]:0")) {
            File(configDir, "local_endpoint.conf")
                .writeText("\"tcp://$endpoint?auth_key=$authKey\"")

            assertFailsWith<IllegalArgumentException>("endpoint: $endpoint") {
                Client.connect(configDir.path)
            }
        }
    }

    @Test
    fun cancelSubscription() = runTest {
        val configDir = "$tempDir/config"
        val service = Service.start(configDir)
        val client = Client.connect(configDir)

        try {
            // Message ids are allocated sequentially starting from 0 and `connect` doesn't consume
            // any, so the ids of the requests below are known in advance.

            // id 0
            val job0 = launchSubscription(client)

            // Cancelling the collection sends `Cancel` for the subscription (id 1) and waits for the
            // response.
            job0.cancelAndJoin()

            // id 2: The subscription has already been cancelled so there is nothing to remove.
            assertEquals(Response.Bool(false), client.invoke(Request.Cancel(MessageId(0))))

            // Control case to verify the message id assumptions: cancelling an active subscription
            // removes it.

            // id 3
            val job3 = launchSubscription(client)

            // id 4
            assertEquals(Response.Bool(true), client.invoke(Request.Cancel(MessageId(3))))

            job3.cancelAndJoin()
        } finally {
            client.close()
            service.stop()
        }
    }

    // Subscribes to network events and returns the job collecting them, once the subscription has
    // been confirmed by the service.
    private suspend fun TestScope.launchSubscription(client: Client): Job {
        val subscribed = CompletableDeferred<Unit>()
        val job = launch {
            client.subscribe(Request.SessionSubscribeToNetwork).collect { subscribed.complete(Unit) }
        }

        subscribed.await()

        return job
    }
}
