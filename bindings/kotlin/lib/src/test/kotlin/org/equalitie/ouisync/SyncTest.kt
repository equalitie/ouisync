package org.equalitie.ouisync.lib

import kotlinx.coroutines.flow.emitAll
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Before
import org.junit.Test
import kotlin.io.path.createTempDirectory
import java.io.File as JFile

class SyncTest {
    lateinit var tempDir: JFile

    lateinit var serverA: Server
    lateinit var sessionA: Session

    lateinit var serverB: Server
    lateinit var sessionB: Session

    @Before
    fun setup() = runTest {
        initLog { level, message -> println("[$level] $message") }

        tempDir = JFile(createTempDirectory().toString())

        val configDirA = "$tempDir/a/config"
        serverA = Server.start(configDirA, "A")
        sessionA = Session.create(configDirA)

        val configDirB = "$tempDir/b/config"
        serverB = Server.start(configDirB, "B")
        sessionB = Session.create(configDirB)
    }

    @After
    fun teardown() = runTest {
        sessionA.close()
        serverA.stop()

        sessionB.close()
        serverB.stop()

        tempDir.deleteRecursively()
    }

    @Test
    fun sync() = runTest {
        val repoA = sessionA.createRepository("$tempDir/a.ouisyncdb")

        val token = repoA.share(AccessMode.WRITE)
        val repoB = sessionB.createRepository(
            "$tempDir/b.ouisyncdb",
            token = token,
        )

        sessionA.bindNetwork(listOf("quic/127.0.0.1:0"))
        sessionB.bindNetwork(listOf("quic/127.0.0.1:0"))

        val addrsA = sessionA.getLocalListenerAddrs()
        sessionB.addUserProvidedPeers(addrsA)

        repoA.setSyncEnabled(true)
        repoB.setSyncEnabled(true)

        val contentA = "hello world"
        val fileA = repoA.createFile("test.txt")
        fileA.write(0, contentA.toByteArray())
        fileA.close()

        flow {
            emit(Unit)
            emitAll(repoB.subscribe())
        }
            .filter checkContent@{
                try {
                    val fileB = repoB.openFile("test.txt")
                    try {
                        val length = fileB.getLength()
                        val contentB = fileB.read(0, length).decodeToString()

                        if (contentB == contentA) {
                            return@checkContent true
                        }
                    } finally {
                        fileB.close()
                    }
                } catch (e: Exception) {
                }

                return@checkContent false
            }
            .first()

        repoA.close()
        repoB.close()
    }
}
