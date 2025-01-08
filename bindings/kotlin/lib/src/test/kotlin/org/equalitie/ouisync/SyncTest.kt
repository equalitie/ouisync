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
    lateinit var sessionA: Session
    lateinit var sessionB: Session

    @Before
    fun setup() = runTest {
        tempDir = JFile(createTempDirectory().toString())

        sessionA = Session.create(
            configPath = "$tempDir/a/config",
        )

        sessionB = Session.create(
            configPath = "$tempDir/b/config",
        )
    }

    @After
    fun teardown() = runTest {
        sessionA.close()
        sessionB.close()
        tempDir.deleteRecursively()
    }

    @Test
    fun sync() = runTest {
        val repoA = Repository.create(
            sessionA,
            "$tempDir/a.ouisyncdb",
            readSecret = null,
            writeSecret = null,
        )

        val token = repoA.share()
        val repoB = Repository.create(
            sessionB,
            "$tempDir/b.ouisyncdb",
            readSecret = null,
            writeSecret = null,
            token = token,
        )

        sessionA.bindNetwork(listOf("quic/127.0.0.1:0"))
        sessionB.bindNetwork(listOf("quic/127.0.0.1:0"))

        val addrsA = sessionA.networkListenerAddrs()
        sessionB.addUserProvidedPeers(addrsA)

        repoA.setSyncEnabled(true)
        repoB.setSyncEnabled(true)

        val contentA = "hello world"
        val fileA = File.create(repoA, "test.txt")
        fileA.write(0, contentA.toByteArray())
        fileA.close()

        flow {
            emit(Unit)
            emitAll(repoB.subscribe())
        }
            .filter checkContent@{
                try {
                    val fileB = File.open(repoB, "test.txt")
                    try {
                        val length = fileB.length()
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
