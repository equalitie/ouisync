package org.equalitie.ouisync

import java.io.File
import kotlin.io.path.createTempDirectory
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertFalse
import org.junit.After
import org.junit.Before
import org.junit.Test

class SessionTest {
    lateinit var tempDir: File
    lateinit var session: Session

    @Before
    fun setup() {
        tempDir = File(createTempDirectory().toString())
        session = Session.create(
            configsPath = "$tempDir/config",
            logPath = "$tempDir/test.log",
        )
    }

    @After
    fun teardown() {
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
        session.bindNetwork(quicV4 = "0.0.0.0:0")

        assertNotNull(session.quicListenerLocalAddrV4())
        assertNull(session.quicListenerLocalAddrV6())
        assertNull(session.tcpListenerLocalAddrV4())
        assertNull(session.tcpListenerLocalAddrV6())
    }

    @Test
    fun addAndRemoveUserProvidedPeer() = runTest {
        val addr = "quic/127.0.0.1:1234"
        val events = session.subscribeToNetworkEvents()

        val peers0 = session.peers()
        assertTrue(peers0.isEmpty())

        session.addUserProvidedPeer(addr)
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers1 = session.peers()
        assertEquals(1, peers1.size)
        assertEquals(addr, "quic/${peers1[0].ip}:${peers1[0].port}")

        session.removeUserProvidedPeer(addr)
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers2 = session.peers()
        assertTrue(peers2.isEmpty())
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
        session.bindNetwork(quicV4 = "0.0.0.0:0")

        assertFalse(session.isLocalDiscoveryEnabled())
        session.setLocalDiscoveryEnabled(true)
        assertTrue(session.isLocalDiscoveryEnabled())
    }
}
