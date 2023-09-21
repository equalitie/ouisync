package org.equalitie.ouisync

import kotlinx.coroutines.test.runTest
import java.io.File
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class SessionTest {
    lateinit var tempDir: File
    lateinit var session: Session

    @BeforeTest
    fun setup() {
        tempDir = File(createTempDirectory().toString())
        session = Session.create(
            configsPath = "$tempDir/config",
            logPath = "$tempDir/test.log",
        )
    }

    @AfterTest
    fun teardown() {
        session.dispose()
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
        session.addUserProvidedPeer(addr)
        session.removeUserProvidedPeer(addr)

        // TODO: check peer list
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
