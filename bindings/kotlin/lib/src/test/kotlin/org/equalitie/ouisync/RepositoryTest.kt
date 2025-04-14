package org.equalitie.ouisync.lib

// import kotlinx.coroutines.test.runTest
// import org.junit.After
// import org.junit.Assert.assertEquals
// import org.junit.Assert.assertFalse
// import org.junit.Assert.assertNull
// import org.junit.Assert.assertTrue
// import org.junit.Assert.fail
// import org.junit.Before
// import org.junit.Test
// import kotlin.io.path.createTempDirectory
// import java.io.File as JFile

// class RepositoryTest {
//     lateinit var tempDir: JFile
//     lateinit var session: Session

//     @Before
//     fun setup() = runTest {
//         tempDir = JFile(createTempDirectory().toString())

//         initLog("$tempDir/test.log")

//         session = Session.create(configPath = "$tempDir/config")
//         session.setStoreDir("$tempDir/store")
//     }

//     @After
//     fun teardown() = runTest {
//         session.close()
//         tempDir.deleteRecursively()
//     }

//     @Test
//     fun create() = runTest {
//         val repo = createRepo()
//         repo.close()
//     }

//     @Test
//     fun open() = runTest {
//         var repo = createRepo()

//         try {
//             repo.close()
//             repo = Repository.open(session, repoName)
//         } finally {
//             repo.close()
//         }
//     }

//     @Test
//     fun list() = runTest {
//         val ext = "ouisyncdb"
//         val storeDir = session.storeDir()

//         val repoA = createRepo(name = "a")
//         assertEquals(mapOf("$storeDir/a.$ext" to repoA), Repository.list(session))

//         val repoB = createRepo(name = "b")
//         assertEquals(mapOf("$storeDir/a.$ext" to repoA, "$storeDir/b.$ext" to repoB), Repository.list(session))
//     }

//     // TODO: events

//     @Test
//     fun infoHash() = runTest {
//         withRepo {
//             val infoHash = it.infoHash()
//             assertTrue(infoHash.isNotEmpty())

//             val token = it.share()
//             assertEquals(infoHash, token.infoHash())
//         }
//     }

//     @Test
//     fun dht() = runTest {
//         withRepo {
//             // Syncing is required to access DHT status
//             it.setSyncEnabled(true)

//             assertFalse(it.isDhtEnabled())
//             it.setDhtEnabled(true)
//             assertTrue(it.isDhtEnabled())
//         }
//     }

//     @Test
//     fun pex() = runTest {
//         withRepo {
//             // Syncing is required to access PEX status
//             it.setSyncEnabled(true)

//             assertFalse(it.isPexEnabled())
//             it.setPexEnabled(true)
//             assertTrue(it.isPexEnabled())
//         }
//     }

//     @Test
//     fun reopen() = runTest {
//         var repo = createRepo()

//         try {
//             val credentials = repo.credentials()
//             repo.close()
//             repo = Repository.open(session, repoName)
//             repo.setCredentials(credentials)
//         } finally {
//             repo.close()
//         }
//     }

//     @Test
//     fun localPasswords() = runTest {
//         suspend fun checkAccessMode(repo: Repository, set: AccessMode, expected: AccessMode) {
//             repo.setAccessMode(AccessMode.BLIND)
//             repo.setAccessMode(set)
//             assertEquals(expected, repo.accessMode())
//         }

//         withRepo {
//             checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
//             checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.WRITE)

//             it.setAccess(read = EnableAccess(LocalPassword("banana")), write = EnableAccess(LocalPassword("banana")))
//             checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.BLIND)
//             checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.BLIND)

//             it.setAccessMode(AccessMode.READ, LocalPassword("banana"))
//             it.setAccess(read = EnableAccess(null))
//             checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
//             checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.READ)

//             it.setAccessMode(AccessMode.WRITE, LocalPassword("banana"))
//             it.setAccess(write = EnableAccess(null))
//             checkAccessMode(it, set = AccessMode.READ, expected = AccessMode.READ)
//             checkAccessMode(it, set = AccessMode.WRITE, expected = AccessMode.WRITE)
//         }
//     }

//     @Test
//     fun accessMode() = runTest {
//         withRepo {
//             assertEquals(AccessMode.WRITE, it.accessMode())
//         }
//     }

//     @Test
//     fun syncProgress() = runTest {
//         withRepo {
//             val progress = it.syncProgress()
//             assertEquals(0, progress.value)
//             assertEquals(0, progress.total)
//         }
//     }

//     @Test
//     fun entryType() = runTest {
//         withRepo {
//             assertEquals(EntryType.DIRECTORY, it.entryType("/"))
//             assertNull(it.entryType("missing.txt"))
//         }
//     }

//     @Test
//     fun moveEntry() = runTest {
//         withRepo { repo ->
//             File.create(repo, "foo.txt").close()

//             repo.moveEntry("foo.txt", "bar.txt")
//             assertEquals(EntryType.FILE, repo.entryType("bar.txt"))
//             assertNull(repo.entryType("foo.txt"))
//         }
//     }

//     @Test
//     fun fileWriteRead() = runTest {
//         val charset = Charsets.UTF_8
//         val contentW = "hello world"

//         withRepo { repo ->
//             val fileW = File.create(repo, "test.txt")
//             fileW.write(0, contentW.toByteArray(charset))
//             fileW.flush()
//             fileW.close()

//             val fileR = File.open(repo, "test.txt")
//             val length = fileR.length()
//             val contentR = fileR.read(0, length).toString(charset)

//             assertEquals(contentW, contentR)
//         }
//     }

//     @Test
//     fun fileRemove() = runTest {
//         val name = "test.txt"

//         withRepo { repo ->
//             assertNull(repo.entryType(name))

//             val file = File.create(repo, name)
//             file.close()
//             assertEquals(EntryType.FILE, repo.entryType(name))

//             File.remove(repo, name)
//             assertNull(repo.entryType(name))
//         }
//     }

//     @Test
//     fun fileTruncate() = runTest {
//         withRepo { repo ->
//             val file = File.create(repo, "test.txt")
//             file.write(0, "hello world".toByteArray(Charsets.UTF_8))
//             file.flush()
//             assertEquals(11, file.length())

//             file.truncate(5)
//             assertEquals(5, file.length())
//             assertEquals("hello", file.read(0, 5).toString(Charsets.UTF_8))
//         }
//     }

//     @Test
//     fun fileProgress() = runTest {
//         withRepo { repo ->
//             val file = File.create(repo, "test.txt")
//             file.write(0, "hello world".toByteArray(Charsets.UTF_8))
//             file.flush()

//             val length = file.length()
//             val progress = file.progress()
//             assertEquals(length, progress)
//         }
//     }

//     @Test
//     fun fileOpenError() = runTest {
//         withRepo { repo ->
//             try {
//                 File.open(repo, "missing.txt")
//                 fail("unexpected successs - expected 'entry not found'")
//             } catch (e: Error) {
//                 assertEquals(ErrorCode.NOT_FOUND, e.code)
//             }
//         }
//     }

//     @Test
//     fun directoryOperations() = runTest {
//         val dirName = "dir"
//         val fileName = "test.txt"

//         withRepo { repo ->
//             assertNull(repo.entryType(dirName))

//             Directory.create(repo, dirName)
//             assertEquals(EntryType.DIRECTORY, repo.entryType(dirName))

//             val dir0 = Directory.read(repo, dirName)
//             assertEquals(0, dir0.size)

//             File.create(repo, "$dirName/$fileName").close()

//             val dir1 = Directory.read(repo, dirName)
//             assertEquals(1, dir1.size)
//             assertEquals(fileName, dir1.elementAt(0).name)
//             assertEquals(EntryType.FILE, dir1.elementAt(0).entryType)

//             Directory.remove(repo, dirName, recursive = true)
//             assertNull(repo.entryType(dirName))
//         }
//     }

//     @Test
//     fun shareTokenOperations() = runTest {
//         withRepo { repo ->
//             var token = repo.share()

//             assertEquals(AccessMode.WRITE, token.accessMode())
//             assertEquals(repoName, token.suggestedName())
//         }
//     }

//     @Test
//     fun shareTokenRoundTrip() = runTest {
//         val origToken = ShareToken.fromString(
//             session,
//             "https://ouisync.net/r#AwAgEZkrt6b9gW47Nb6hGQjsZRGeh9GKp3gTyhZrxfT03SE",
//         )
//         val repo = createRepo(token = origToken)

//         try {
//             val actualToken = repo.share()
//             assertEquals("$origToken?name=$repoName", actualToken.toString())
//         } finally {
//             repo.close()
//         }
//     }

//     @Test
//     fun repoWithLocalPasswords() = runTest {
//         val tempDir = JFile(createTempDirectory().toString())
//         val repoPath = "$tempDir/repo.db"

//         val readPassword = LocalPassword("read_pwd")
//         val writePassword = LocalPassword("write_pwd")

//         Repository.create(
//             session,
//             repoPath,
//             readSecret = readPassword,
//             writeSecret = writePassword,
//         ).also { repo ->
//             assertEquals(AccessMode.WRITE, repo.accessMode())
//             repo.close()
//         }

//         Repository.open(session, repoPath, secret = readPassword).also { repo ->
//             assertEquals(AccessMode.READ, repo.accessMode())
//             repo.close()
//         }

//         Repository.open(session, repoPath, secret = writePassword).also { repo ->
//             assertEquals(AccessMode.WRITE, repo.accessMode())
//             repo.close()
//         }
//     }

//     @Test
//     fun repoWithLocalKeys() = runTest {
//         val tempDir = JFile(createTempDirectory().toString())
//         val repoPath = "$tempDir/repo.db"

//         val readKey = LocalSecretKey.random()
//         val writeKey = LocalSecretKey.random()

//         val readSalt = PasswordSalt.random()
//         val writeSalt = PasswordSalt.random()

//         Repository.create(
//             session,
//             repoPath,
//             readSecret = LocalSecretKeyAndSalt(readKey, readSalt),
//             writeSecret = LocalSecretKeyAndSalt(writeKey, writeSalt),
//         ).also { repo ->
//             assertEquals(AccessMode.WRITE, repo.accessMode())
//             repo.close()
//         }

//         Repository.open(session, repoPath, secret = readKey).also { repo ->
//             assertEquals(AccessMode.READ, repo.accessMode())
//             repo.close()
//         }

//         Repository.open(session, repoPath, secret = writeKey).also { repo ->
//             assertEquals(AccessMode.WRITE, repo.accessMode())
//             repo.close()
//         }
//     }

//     @Test
//     fun delete() = runTest {
//         createRepo().apply { close() }

//         val repo = Repository.open(session, repoName)
//         repo.delete()

//         try {
//             Repository.open(session, repoName)
//             fail("unexpected success")
//         } catch (e: Error.StoreError) {
//         } catch (e: Exception) {
//             fail("unexpected exception: $e")
//         }
//     }

//     private suspend fun createRepo(name: String? = null, token: ShareToken? = null): Repository =
//         Repository.create(
//             session,
//             name ?: repoName,
//             readSecret = null,
//             writeSecret = null,
//             token = token,
//         )

//     private suspend fun <R> withRepo(block: suspend (repo: Repository) -> R): R {
//         val repo = createRepo()

//         try {
//             return block(repo)
//         } finally {
//             repo.close()
//         }
//     }

//     private val repoName = "repo"
// }
