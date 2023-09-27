package org.equalitie.ouisync

import kotlinx.coroutines.test.runTest
import kotlin.io.path.createTempDirectory
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.test.fail
import java.io.File as JFile

class RepositoryTest {
    lateinit var tempDir: JFile
    lateinit var session: Session

    @BeforeTest
    fun setup() {
        tempDir = JFile(createTempDirectory().toString())
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

    @Test
    fun moveEntry() = runTest {
        withRepo { repo ->
            File.create(repo, "foo.txt").close()

            repo.moveEntry("foo.txt", "bar.txt")
            assertEquals(EntryType.FILE, repo.entryType("bar.txt"))
            assertNull(repo.entryType("foo.txt"))
        }
    }

    @Test
    fun fileWriteRead() = runTest {
        val charset = Charsets.UTF_8
        val contentW = "hello world"

        withRepo { repo ->
            val fileW = File.create(repo, "test.txt")
            fileW.write(0, contentW.toByteArray(charset))
            fileW.flush()
            fileW.close()

            val fileR = File.open(repo, "test.txt")
            val length = fileR.length()
            val contentR = fileR.read(0, length).toString(charset)

            assertEquals(contentW, contentR)
        }
    }

    @Test
    fun fileRemove() = runTest {
        val name = "test.txt"

        withRepo { repo ->
            assertNull(repo.entryType(name))

            val file = File.create(repo, name)
            file.close()
            assertEquals(EntryType.FILE, repo.entryType(name))

            File.remove(repo, name)
            assertNull(repo.entryType(name))
        }
    }

    @Test
    fun fileTruncate() = runTest {
        withRepo { repo ->
            val file = File.create(repo, "test.txt")
            file.write(0, "hello world".toByteArray(Charsets.UTF_8))
            file.flush()
            assertEquals(11, file.length())

            file.truncate(5)
            assertEquals(5, file.length())
            assertEquals("hello", file.read(0, 5).toString(Charsets.UTF_8))
        }
    }

    @Test
    fun fileProgress() = runTest {
        withRepo { repo ->
            val file = File.create(repo, "test.txt")
            file.write(0, "hello world".toByteArray(Charsets.UTF_8))
            file.flush()

            val length = file.length()
            val progress = file.progress()
            assertEquals(length, progress)
        }
    }

    @Test
    fun fileOpenError() = runTest {
        withRepo { repo ->
            try {
                File.open(repo, "missing.txt")
                fail("unexpected successs - expected 'entry not found'")
            } catch (e: Error) {
                assertEquals(ErrorCode.ENTRY_NOT_FOUND, e.code)
            }
        }
    }

    @Test
    fun directoryOperations() = runTest {
        val dirName = "dir"
        val fileName = "test.txt"

        withRepo { repo ->
            assertNull(repo.entryType(dirName))

            Directory.create(repo, dirName)
            assertEquals(EntryType.DIRECTORY, repo.entryType(dirName))

            val dir0 = Directory.open(repo, dirName)
            assertEquals(0, dir0.size)

            File.create(repo, "$dirName/$fileName").close()

            val dir1 = Directory.open(repo, dirName)
            assertEquals(1, dir1.size)
            assertEquals(fileName, dir1.elementAt(0).name)
            assertEquals(EntryType.FILE, dir1.elementAt(0).entryType)

            Directory.remove(repo, dirName, recursive = true)
            assertNull(repo.entryType(dirName))
        }
    }

    @Test
    fun shareTokenOperations() = runTest {
        withRepo { repo ->
            var token = repo.createShareToken(name = "foo")

            assertEquals(AccessMode.WRITE, token.accessMode())
            assertEquals("foo", token.suggestedName())
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
