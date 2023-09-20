package org.equalitie.ouisync_kotlin

import java.io.File
import kotlin.io.path.createTempDirectory
import kotlin.test.Test
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.assertTrue
import kotlinx.coroutines.test.runTest

class SessionTest {
    lateinit var tempDir: File
    lateinit var session: Session

    @BeforeTest
    fun setup() {
        tempDir = File(createTempDirectory().toString())
        session = Session.create(
            configsPath = "${tempDir}/config",
            logPath = "${tempDir}/test.log",
        );
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
}
