package org.equalitie.ouisync.session

import kotlinx.coroutines.channels.produce
import kotlinx.coroutines.test.runTest
import org.equalitie.ouisync.service.Service
import org.equalitie.ouisync.service.initLog
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File
import kotlin.io.path.createTempDirectory

class SessionTest {
    lateinit var tempDir: File
    lateinit var session: Session
    lateinit var service: Service

    @BeforeEach
    fun setup() = runTest {
        tempDir = File(createTempDirectory().toString())
        val configDir = "$tempDir/config"

        initLog()

        service = Service.start(configDir)
        session = Session.create(configDir)
    }

    @AfterEach
    fun teardown() = runTest {
        session.close()
        service.stop()
        tempDir.deleteRecursively()
    }

    @Test
    fun storeDir() = runTest {
        val storeDir = "$tempDir/store"

        assertNull(session.getStoreDir())

        session.setStoreDir(storeDir)
        assertEquals(storeDir, session.getStoreDir())
    }

    @Test
    fun initNetwork() = runTest {
        session.initNetwork(
            NetworkDefaults(
                bind = emptyList(),
                portForwardingEnabled = false,
                localDiscoveryEnabled = false,
            ),
        )
    }

    @Test
    fun bindNetwork() = runTest {
        session.bindNetwork(listOf("quic/0.0.0.0:0"))

        val addrs = session.getLocalListenerAddrs()
        assertEquals(1, addrs.size)
        assertTrue(addrs[0].startsWith("quic/0.0.0.0"))
    }

    @Test
    fun addAndRemoveUserProvidedPeer() = runTest {
        val addr = "quic/127.0.0.1:1234"

        val peers0 = session.getPeers()
        assertTrue(peers0.isEmpty())

        // Convert the flow to ReceiveChannel so we can collect it one element at a time.
        @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
        val events = produce { session.subscribeToNetworkEvents().collect(::send) }

        // Wait for the initial event indicating that the subscription has been created
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        session.addUserProvidedPeers(listOf(addr))
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers1 = session.getPeers()
        assertEquals(1, peers1.size)
        assertEquals(addr, peers1[0].addr)

        session.removeUserProvidedPeers(listOf(addr))
        assertEquals(NetworkEvent.PEER_SET_CHANGE, events.receive())

        val peers2 = session.getPeers()
        assertTrue(peers2.isEmpty())

        events.cancel()
    }

    @Test fun runtimeId() = runTest { session.getRuntimeId() }

    @Test
    fun protocolVersion() = runTest {
        session.getCurrentProtocolVersion()
        session.getHighestSeenProtocolVersion()
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

    @Test
    fun multipleSessions() = runTest {
        val other = Session.create(configPath = "$tempDir/config")

        try {
            assertArrayEquals(session.getRuntimeId().value, other.getRuntimeId().value)
        } finally {
            other.close()
        }
    }
}
