package org.equalitie.ouisync

import kotlinx.coroutines.test.runTest
import java.io.File
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
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
            repo = Repository.open(session, repoPath)
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

    @Test
    fun reopen() = runTest {
        var repo = createRepo()

        try {
            val token = repo.createReopenToken()
            repo.close()
            repo = Repository.reopen(session, repoPath, token)
        } finally {
            repo.close()
        }
    }

    @Test
    fun localPasswords() = runTest {
        withRepo {
            assertFalse(it.requiresLocalPasswordForReading())
            assertFalse(it.requiresLocalPasswordForWriting())

            it.setReadAndWriteAccess(oldPassword = null, newPassword = "banana")
            assertTrue(it.requiresLocalPasswordForReading())
            assertTrue(it.requiresLocalPasswordForWriting())

            it.setReadAccess(password = null)
            assertFalse(it.requiresLocalPasswordForReading())
            assertTrue(it.requiresLocalPasswordForWriting())

            it.setReadAndWriteAccess(oldPassword = null, newPassword = null)
            assertFalse(it.requiresLocalPasswordForReading())
            assertFalse(it.requiresLocalPasswordForWriting())
        }
    }

    @Test
    fun accessMode() = runTest {
        withRepo {
            assertEquals(AccessMode.WRITE, it.accessMode())
        }
    }

    @Test
    fun databaseId() = runTest {
        withRepo {
            assertTrue(it.databaseId().isNotEmpty())
        }
    }

    @Test
    fun syncProgress() = runTest {
        withRepo {
            val progress = it.syncProgress()
            assertEquals(0, progress.value)
            assertEquals(0, progress.total)
        }
    }

    @Test
    fun entryType() = runTest {
        withRepo {
            assertEquals(EntryType.DIRECTORY, it.entryType("/"))
            assertNull(it.entryType("missing.txt"))
        }
    }

    private suspend fun createRepo(): Repository =
        Repository.create(session, repoPath, readPassword = null, writePassword = null)

    private suspend fun <R> withRepo(block: suspend (repo: Repository) -> R): R {
        val repo = createRepo()

        try {
            return block(repo)
        } finally {
            repo.close()
        }
    }

    private val repoPath: String
        get() = "$tempDir/repo.db"
}
