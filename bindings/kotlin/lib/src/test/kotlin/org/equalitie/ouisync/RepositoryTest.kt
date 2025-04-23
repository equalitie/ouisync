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
    lateinit var server: Server

    @Before
    fun setup() = runTest {
        initLog { level, message -> println("[$level] $message") }

        tempDir = JFile(createTempDirectory().toString())
        val configDir = "$tempDir/config"

        server = Server.start(configDir)

        session = Session.create(configDir)
        session.setStoreDir("$tempDir/store")
    }

    @After
    fun teardown() = runTest {
        session.close()
        server.stop()
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
            repo = session.openRepository(repoName)
        } finally {
            repo.close()
        }
    }

    @Test
    fun list() = runTest {
        val ext = "ouisyncdb"
        val storeDir = session.getStoreDir()

        val repoA = createRepo(name = "a")
        assertEquals(mapOf("$storeDir/a.$ext" to repoA), session.listRepositories())

        val repoB = createRepo(name = "b")
        assertEquals(mapOf("$storeDir/a.$ext" to repoA, "$storeDir/b.$ext" to repoB), session.listRepositories())
    }

    // TODO: events

    @Test
    fun infoHash() = runTest {
        withRepo {
            val infoHash = it.getInfoHash()
            assertTrue(infoHash.isNotEmpty())

            val token = it.share(AccessMode.WRITE)
            assertEquals(infoHash, session.getShareTokenInfoHash(token))
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
            val credentials = repo.getCredentials()
            repo.close()
            repo = session.openRepository(repoName)
            repo.setCredentials(credentials)
        } finally {
            repo.close()
        }
    }

    @Test
    fun localPasswords() = runTest {
        suspend fun checkAccessMode(repo: Repository, set: AccessMode, expected: AccessMode) {
            repo.setAccessMode(AccessMode.BLIND)
            repo.setAccessMode(set)
            assertEquals(expected, repo.getAccessMode())
        }

        withRepo {
            checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
            checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.WRITE)

            it.setAccess(
                read = AccessChange.Enable(SetLocalSecret.Password(Password("banana"))),
                write = AccessChange.Enable(SetLocalSecret.Password(Password("banana")))
            )
            checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.BLIND)
            checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.BLIND)

            it.setAccessMode(AccessMode.READ, LocalSecret.Password(Password("banana")))
            it.setAccess(read = AccessChange.Enable(null), write = null)
            checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
            checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.READ)

            it.setAccessMode(AccessMode.WRITE, LocalSecret.Password(Password("banana")))
            it.setAccess(read = null, write = AccessChange.Enable(null))
            checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
            checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.WRITE)
        }
    }

    @Test
    fun accessMode() = runTest {
        withRepo {
            assertEquals(AccessMode.WRITE, it.getAccessMode())
        }
    }

    @Test
    fun syncProgress() = runTest {
        withRepo {
            val progress = it.getSyncProgress()
            assertEquals(0, progress.value)
            assertEquals(0, progress.total)
        }
    }

    @Test
    fun entryType() = runTest {
        withRepo {
            assertEquals(EntryType.DIRECTORY, it.getEntryType("/"))
            assertNull(it.getEntryType("missing.txt"))
        }
    }

    @Test
    fun moveEntry() = runTest {
        withRepo { repo ->
            repo.createFile("foo.txt").close()

            repo.moveEntry("foo.txt", "bar.txt")
            assertEquals(EntryType.FILE, repo.getEntryType("bar.txt"))
            assertNull(repo.getEntryType("foo.txt"))
        }
    }

    @Test
    fun fileWriteRead() = runTest {
        val charset = Charsets.UTF_8
        val contentW = "hello world"

        withRepo { repo ->
            val fileW = repo.createFile("test.txt")
            fileW.write(0, contentW.toByteArray(charset))
            fileW.flush()
            fileW.close()

            val fileR = repo.openFile("test.txt")
            val length = fileR.getLength()
            val contentR = fileR.read(0, length).toString(charset)

            assertEquals(contentW, contentR)
        }
    }

    @Test
    fun fileRemove() = runTest {
        val name = "test.txt"

        withRepo { repo ->
            assertNull(repo.getEntryType(name))

            val file = repo.createFile(name)
            file.close()
            assertEquals(EntryType.FILE, repo.getEntryType(name))

            repo.removeFile(name)
            assertNull(repo.getEntryType(name))
        }
    }

    @Test
    fun fileTruncate() = runTest {
        withRepo { repo ->
            val file = repo.createFile("test.txt")
            file.write(0, "hello world".toByteArray(Charsets.UTF_8))
            file.flush()
            assertEquals(11, file.getLength())

            file.truncate(5)
            assertEquals(5, file.getLength())
            assertEquals("hello", file.read(0, 5).toString(Charsets.UTF_8))
        }
    }

    @Test
    fun fileProgress() = runTest {
        withRepo { repo ->
            val file = repo.createFile("test.txt")
            file.write(0, "hello world".toByteArray(Charsets.UTF_8))
            file.flush()

            val length = file.getLength()
            val progress = file.getProgress()
            assertEquals(length, progress)
        }
    }

    @Test
    fun fileOpenError() = runTest {
        withRepo { repo ->
            try {
                repo.openFile("missing.txt")
                fail("unexpected successs - expected 'entry not found'")
            } catch (e: OuisyncException.NotFound) {
            }
        }
    }

    @Test
    fun directoryOperations() = runTest {
        val dirName = "dir"
        val fileName = "test.txt"

        withRepo { repo ->
            assertNull(repo.getEntryType(dirName))

            repo.createDirectory(dirName)
            assertEquals(EntryType.DIRECTORY, repo.getEntryType(dirName))

            val dir0 = repo.readDirectory(dirName)
            assertEquals(0, dir0.size)

            repo.createFile("$dirName/$fileName").close()

            val dir1 = repo.readDirectory(dirName)
            assertEquals(1, dir1.size)
            assertEquals(fileName, dir1.elementAt(0).name)
            assertEquals(EntryType.FILE, dir1.elementAt(0).entryType)

            repo.removeDirectory(dirName, recursive = true)
            assertNull(repo.getEntryType(dirName))
        }
    }

    @Test
    fun shareTokenOperations() = runTest {
        withRepo { repo ->
            var token = repo.share(AccessMode.WRITE)

            assertEquals(AccessMode.WRITE, session.getShareTokenAccessMode(token))
            assertEquals(repoName, session.getShareTokenSuggestedName(token))
        }
    }

    @Test
    fun shareTokenRoundTrip() = runTest {
        val origToken = session.validateShareToken(
            "https://ouisync.net/r#AwAgEZkrt6b9gW47Nb6hGQjsZRGeh9GKp3gTyhZrxfT03SE",
        )
        val repo = createRepo(token = origToken)

        try {
            val actualToken = repo.share(AccessMode.WRITE)
            assertEquals("${origToken.value}?name=$repoName", actualToken.value)
        } finally {
            repo.close()
        }
    }

    @Test
    fun repoWithLocalPasswords() = runTest {
        val tempDir = JFile(createTempDirectory().toString())
        val repoPath = "$tempDir/repo.db"

        val readPassword = Password("read_pwd")
        val writePassword = Password("write_pwd")

        session.createRepository(
            repoPath,
            readSecret = SetLocalSecret.Password(readPassword),
            writeSecret = SetLocalSecret.Password(writePassword),
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.getAccessMode())
            repo.close()
        }

        session.openRepository(
            repoPath,
            localSecret = LocalSecret.Password(readPassword)
        ).also { repo ->
            assertEquals(AccessMode.READ, repo.getAccessMode())
            repo.close()
        }

        session.openRepository(
            repoPath,
            localSecret = LocalSecret.Password(writePassword)
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.getAccessMode())
            repo.close()
        }
    }

    @Test
    fun repoWithLocalKeys() = runTest {
        val tempDir = JFile(createTempDirectory().toString())
        val repoPath = "$tempDir/repo.db"

        val readKey = session.generateSecretKey()
        val writeKey = session.generateSecretKey()

        val readSalt = session.generatePasswordSalt()
        val writeSalt = session.generatePasswordSalt()

        session.createRepository(
            repoPath,
            readSecret = SetLocalSecret.KeyAndSalt(readKey, readSalt),
            writeSecret = SetLocalSecret.KeyAndSalt(writeKey, writeSalt),
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.getAccessMode())
            repo.close()
        }

        session.openRepository(
            repoPath,
            localSecret = LocalSecret.SecretKey(readKey)
        ).also { repo ->
            assertEquals(AccessMode.READ, repo.getAccessMode())
            repo.close()
        }

        session.openRepository(
            repoPath,
            localSecret = LocalSecret.SecretKey(writeKey)
        ).also { repo ->
            assertEquals(AccessMode.WRITE, repo.getAccessMode())
            repo.close()
        }
    }

    @Test
    fun delete() = runTest {
        createRepo().apply { close() }

        val repo = session.openRepository(repoName)
        repo.delete()

        try {
            session.openRepository(repoName)
            fail("unexpected success")
        } catch (e: OuisyncException.StoreError) {
        } catch (e: Exception) {
            fail("unexpected exception: $e")
        }
    }

    private suspend fun createRepo(name: String? = null, token: ShareToken? = null): Repository =
        session.createRepository(
            name ?: repoName,
            token = token,
        )

    private suspend fun <R> withRepo(block: suspend (repo: Repository) -> R): R {
        val repo = createRepo()

        try {
            return block(repo)
        } finally {
            repo.close()
        }
    }

    private val repoName = "repo"
}
