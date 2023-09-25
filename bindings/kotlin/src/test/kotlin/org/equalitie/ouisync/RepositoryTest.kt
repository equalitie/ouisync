package org.equalitie.ouisync

import kotlinx.coroutines.test.runTest
import java.io.File
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class RepositoryTest {
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
        session.close()
        tempDir.deleteRecursively()
    }

    @Test
    fun create() = runTest {
        val repo = createRepo()
        repo.close()
    }

    @Test
    fun open() = runTest {
        var repo = createRepo()

        try {
            repo.close()
            repo = Repository.open(session, "$tempDir/repo.db")
        } finally {
            repo.close()
        }
    }

    // TODO: events

    @Test
    fun infoHash() = runTest {
        withRepo {
            assertTrue(it.infoHash().isNotEmpty())
        }
    }

    @Test
    fun dht() = runTest {
        withRepo {
            assertFalse(it.isDhtEnabled())
            it.setDhtEnabled(true)
            assertTrue(it.isDhtEnabled())
        }
    }

    @Test
    fun pex() = runTest {
        withRepo {
            assertFalse(it.isPexEnabled())
            it.setPexEnabled(true)
            assertTrue(it.isPexEnabled())
        }
    }

    @Test
    fun shareToken() = runTest {
        withRepo {
            assertTrue(it.createShareToken().isNotEmpty())
        }
    }

    private suspend fun createRepo(): Repository =
        Repository.create(session, "$tempDir/repo.db", readPassword = null, writePassword = null)

    private suspend fun <R> withRepo(block: suspend (repo: Repository) -> R): R {
        val repo = createRepo()

        try {
            return block(repo)
        } finally {
            repo.close()
        }
    }
}
