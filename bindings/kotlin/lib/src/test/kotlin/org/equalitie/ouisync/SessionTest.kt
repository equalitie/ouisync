package org.equalitie.ouisync.lib

import kotlinx.coroutines.channels.produce
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.yield
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import kotlin.io.path.createTempDirectory

class SessionTest {
    lateinit var tempDir: File
    lateinit var session: Session

    @Before
    fun setup() = runTest {
        tempDir = File(createTempDirectory().toString())

        initLog("$tempDir/test.log")

        session = Session.create(
            configPath = "$tempDir/config",
        )
    }

    @After
    fun teardown() = runTest {
        session.close()
        tempDir.deleteRecursively()
    }

    @Test
    fun initNetwork() = runTest {
        session.initNetwork(
            defaultPortForwardingEnabled = false,
            defaultLocalDiscoveryEnabled = false,
        )
    }

    @Test
    fun bindNetwork() = runTest {
        session.bindNetwork(listOf("quic/0.0.0.0:0"))

        val addrs = session.networkListenerAddrs()
        assertEquals(1, addrs.size)
        assertTrue(addrs[0].startsWith("quic/0.0.0.0"))
    }

    @Test
    fun addAndRemoveUserProvidedPeer() = runTest {
        val addr = "quic/127.0.0.1:1234"

        val peers0 = session.peers()
        assertTrue(peers0.isEmpty())

        // Convert the flow to ReceiveChannel so we can collect it one element at a time.
        @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
        val events = produce {
            session.subscribeToNetworkEvents().collect(::send)
        }

        yield()

        session.addUserProvidedPeers(listOf(addr))
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers1 = session.peers()
        assertEquals(1, peers1.size)
        assertEquals(addr, peers1[0].addr)

        session.removeUserProvidedPeers(listOf(addr))
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers2 = session.peers()
        assertTrue(peers2.isEmpty())

        events.cancel()
    }

    @Test
    fun thisRuntimeId() = runTest {
        val runtimeId = session.thisRuntimeId()
        assertTrue(runtimeId.isNotEmpty())
    }

    @Test
    fun protocolVersion() = runTest {
        session.currentProtocolVersion()
        session.highestSeenProtocolVersion()
    }

    @Test
    fun portForwarding() = runTest {
        assertFalse(session.isPortForwardingEnabled())
        session.setPortForwardingEnabled(true)
        assertTrue(session.isPortForwardingEnabled())
    }

    @Test
    fun localDiscovery() = runTest {
        // Local discovery requires a running listener
        session.bindNetwork(listOf("quic/0.0.0.0:0"))

        assertFalse(session.isLocalDiscoveryEnabled())
        session.setLocalDiscoveryEnabled(true)
        assertTrue(session.isLocalDiscoveryEnabled())
    }
}
