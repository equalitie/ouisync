package org.equalitie.ouisync.session

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.initLog
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.nio.ByteBuffer
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertTrue

class SocketTest {
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
    fun cancelReadTcp() = runTest {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val addr = SocketAddress.Tcp(InetSocketAddress(server.inetAddress, server.localPort))

            Socket.connect(addr).use { socket -> server.accept().use { checkCancelRead(socket) } }
        }
    }

    @Test
    fun cancelReadUnix() = runTest {
        val configDir = "$tempDir/config"
        val service = Service.start(configDir)

        try {
            val path = File(configDir, "local_endpoint.sock")
            assertTrue(path.exists())

            Socket.connect(SocketAddress.Unix(path.path)).use { socket -> checkCancelRead(socket) }
        } finally {
            service.stop()
        }
    }

    // Starts a read which never completes (the peer never sends anything), cancels it and checks the
    // cancellation completes promptly.
    private suspend fun checkCancelRead(socket: Socket) = withContext(Dispatchers.Default) {
        // Use a separate scope so that if the cancellation doesn't work, the test fails on the
        // timeout
        // below instead of hanging forever.
        val job =
            CoroutineScope(Dispatchers.Default).launch(start = CoroutineStart.UNDISPATCHED) {
                socket.read(ByteBuffer.allocate(1))
            }

        // Give the read some time to actually block.
        delay(100)

        withTimeout(5000) { job.cancelAndJoin() }

        assertTrue(job.isCancelled)
    }
}
