package org.equalitie.ouisync.lib

import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import kotlin.io.path.createTempDirectory
import java.io.File as JFile

class RepositoryTest {
    lateinit var tempDir: JFile
    lateinit var session: Session

    @Before
    fun setup() {
        tempDir = JFile(createTempDirectory().toString())
        session = Session.create(
            configsPath = "$tempDir/config",
            logPath = "$tempDir/test.log",
        )
    }

    @After
    fun teardown() = runTest {
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
            // Syncing is required to access DHT status
            it.setSyncEnabled(true)

            assertFalse(it.isDhtEnabled())
            it.setDhtEnabled(true)
            assertTrue(it.isDhtEnabled())
        }
    }

    @Test
    fun pex() = runTest {
        withRepo {
            // Syncing is required to access PEX status
            it.setSyncEnabled(true)

            assertFalse(it.isPexEnabled())
            it.setPexEnabled(true)
            assertTrue(it.isPexEnabled())
        }
    }

    @Test
    fun reopen() = runTest {
        var repo = createRepo()

        try {
            val credentials = repo.credentials()
            repo.close()
            repo = Repository.open(session, repoPath)
            repo.setCredentials(credentials)
        } finally {
            repo.close()
        }
    }

    @Test
    fun localPasswords() = runTest {
        withRepo {
            assertFalse(it.requiresLocalSecretForReading())
            assertFalse(it.requiresLocalSecretForWriting())

            it.setAccess(read = EnableAccess(LocalPassword("banana")), write = EnableAccess(LocalPassword("banana")))
            assertTrue(it.requiresLocalSecretForReading())
            assertTrue(it.requiresLocalSecretForWriting())

            it.setAccess(read = EnableAccess(null))
            assertFalse(it.requiresLocalSecretForReading())
            assertTrue(it.requiresLocalSecretForWriting())

            it.setAccess(write = EnableAccess(null))
            assertFalse(it.requiresLocalSecretForReading())
            assertFalse(it.requiresLocalSecretForWriting())
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

    @Test
    fun shareTokenRoundTrip() = runTest {
        val origToken = ShareToken.fromString(
            session,
            "https://ouisync.net/r#AwAgEZkrt6b9gW47Nb6hGQjsZRGeh9GKp3gTyhZrxfT03SE",
        )
        val repo = createRepo(shareToken = origToken)

        try {
            val actualToken = repo.createShareToken()
            assertEquals(origToken, actualToken)
        } finally {
            repo.close()
        }
    }

    @Test
    fun repoWithLocalPasswords() = runTest {
        val tempDir = JFile(createTempDirectory().toString())
        val repoPath = "$tempDir/repo.db"

        val readPassword = LocalPassword("read_pwd")
        val writePassword = LocalPassword("write_pwd")

        Repository.create(
            session,
            repoPath,
            readSecret = readPassword,
            writeSecret = writePassword,
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.accessMode())
            repo.close()
        }

        Repository.open(session, repoPath, secret = readPassword).also { repo ->
            assertEquals(AccessMode.READ, repo.accessMode())
            repo.close()
        }

        Repository.open(session, repoPath, secret = writePassword).also { repo ->
            assertEquals(AccessMode.WRITE, repo.accessMode())
            repo.close()
        }
    }

    @Test
    fun repoWithLocalKeys() = runTest {
        val tempDir = JFile(createTempDirectory().toString())
        val repoPath = "$tempDir/repo.db"

        val readKey = LocalSecretKey.random()
        val writeKey = LocalSecretKey.random()

        val readSalt = PasswordSalt.random()
        val writeSalt = PasswordSalt.random()

        Repository.create(
            session,
            repoPath,
            readSecret = LocalSecretKeyAndSalt(readKey, readSalt),
            writeSecret = LocalSecretKeyAndSalt(writeKey, writeSalt),
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.accessMode())
            repo.close()
        }

        Repository.open(session, repoPath, secret = readKey).also { repo ->
            assertEquals(AccessMode.READ, repo.accessMode())
            repo.close()
        }

        Repository.open(session, repoPath, secret = writeKey).also { repo ->
            assertEquals(AccessMode.WRITE, repo.accessMode())
            repo.close()
        }
    }

    private suspend fun createRepo(shareToken: ShareToken? = null): Repository =
        Repository.create(
            session,
            repoPath,
            readSecret = null,
            writeSecret = null,
            shareToken = shareToken,
        )

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
