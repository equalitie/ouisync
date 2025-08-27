package org.equalitie.ouisync.kotlin.android

import android.content.BroadcastReceiver
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.database.ContentObserver
import android.database.Cursor
import android.net.Uri
import android.os.Handler
import android.provider.DocumentsContract
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.random.Random
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.AccessMode
import org.equalitie.ouisync.kotlin.client.EntryType
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.close
import org.equalitie.ouisync.kotlin.client.create
import org.equalitie.ouisync.kotlin.server.initLog
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OuisyncProviderTest {
    companion object {
        private val TAG = OuisyncProviderTest::class.simpleName
        private const val AUTHORITY = "org.equalitie.ouisync.kotlin.android.test.provider"
    }

    private lateinit var context: Context
    private lateinit var contentResolver: ContentResolver
    private lateinit var tempDir: File

    private val configDir: String
        get() = "${tempDir.path}/config"

    private val storeDir: String
        get() = "${tempDir.path}/store"

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        contentResolver = context.contentResolver

        tempDir = context.getDir(randomString(16), 0)

        runBlocking {
            context.setConfigPath(configDir)
        }

        initLog()
        startService()
    }

    @After
    fun tearDown() {
        stopService()
        tempDir.deleteRecursively()
    }

    @Test
    fun testQueryRoots() {
        val uri = DocumentsContract.buildRootsUri(AUTHORITY)

        contentResolver.query(uri, null, null, null, null)!!.use { cursor ->
            assertEquals(1, cursor.count)

            assertTrue(cursor.moveToFirst())

            assertEquals(
                "default",
                cursor.getString(DocumentsContract.Root.COLUMN_ROOT_ID),
            )
            assertEquals(
                "repos",
                cursor.getString(DocumentsContract.Root.COLUMN_DOCUMENT_ID),
            )
            assertEquals(
                R.mipmap.ouisync_provider_root_icon,
                cursor.getInt(DocumentsContract.Root.COLUMN_ICON),
            )
            assertEquals(
                DocumentsContract.Root.MIME_TYPE_ITEM,
                cursor.getString(DocumentsContract.Root.COLUMN_MIME_TYPES),
            )
            assertEquals(
                "Ouisync",
                cursor.getString(DocumentsContract.Root.COLUMN_TITLE),
            )
        }
    }

    @Test
    fun testQueryRepos() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo")
            createRepository("bar")
        }

        contentResolver.query(
            DocumentsContract.buildDocumentUri(AUTHORITY, "repos"),
            null,
            null,
            null,
            null
        )!!.use { cursor ->
            assertEquals(1, cursor.count)

            assertTrue(cursor.moveToFirst())
            assertEquals(
                "repos",
                cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID),
            )
            assertEquals(
                "Repositories",
                cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            )
            assertEquals(
                DocumentsContract.Document.MIME_TYPE_DIR,
                cursor.getString(DocumentsContract.Document.COLUMN_MIME_TYPE),
            )
        }

        contentResolver.query(
            DocumentsContract.buildChildDocumentsUri(AUTHORITY, "repos"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            assertEquals(2, cursor.count)

            assertTrue(cursor.moveToFirst())
            assertEquals(
                "bar/",
                cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID),
            )
            assertEquals(
                "bar",
                cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            )
            assertEquals(
                DocumentsContract.Document.MIME_TYPE_DIR,
                cursor.getString(DocumentsContract.Document.COLUMN_MIME_TYPE),
            )

            assertTrue(cursor.moveToNext())
            assertEquals(
                "foo/",
                cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID),
            )
            assertEquals(
                "foo",
                cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
            )
            assertEquals(
                DocumentsContract.Document.MIME_TYPE_DIR,
                cursor.getString(DocumentsContract.Document.COLUMN_MIME_TYPE),
            )
        }
    }

    @Test
    fun testQueryEmptyDirectory() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo")
        }

        contentResolver.query(
            DocumentsContract.buildChildDocumentsUri(AUTHORITY, "foo/"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            assertEquals(0, cursor.count)
        }
    }

    @Test
    fun testQueryNonEmptyDirectory() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo").apply {
                createDirectory("a")
                createFile("b.txt").apply {
                    write(0, "hello world".toByteArray())
                    close()
                }
            }
        }

        contentResolver.query(
            DocumentsContract.buildChildDocumentsUri(AUTHORITY, "foo/"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            assertEquals(2, cursor.count)

            assertTrue(cursor.moveToFirst())
            assertEquals("a", cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME))
            assertEquals("foo/a", cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID))
            assertEquals(DocumentsContract.Document.MIME_TYPE_DIR, cursor.getString(DocumentsContract.Document.COLUMN_MIME_TYPE))

            assertTrue(cursor.moveToNext())
            assertEquals("b.txt", cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME))
            assertEquals("foo/b.txt", cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID))
            assertEquals("text/plain", cursor.getString(DocumentsContract.Document.COLUMN_MIME_TYPE))
        }
    }

    @Test
    fun testQueryBlindRepo() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo").setAccessMode(AccessMode.BLIND)
        }

        contentResolver.query(
            DocumentsContract.buildChildDocumentsUri(AUTHORITY, "foo/"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            assertEquals(0, cursor.count)
            assertEquals("This repository is locked", cursor.getExtras().getString(DocumentsContract.EXTRA_INFO))
        }
    }

    @Test
    fun testReadFile() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo").apply {
                createFile("a.txt").apply {
                    write(0, "hello world".toByteArray())
                    close()
                }
            }
        }

        val uri = DocumentsContract.buildDocumentUri(AUTHORITY, "foo/a.txt")
        contentResolver.openInputStream(uri)!!.use { stream ->
            val content = stream.readAllBytes().decodeToString()
            assertEquals("hello world", content)
        }
    }

    @Test
    fun testCreateDirectory() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo")
        }

        // Check creating documents is supported by the root...
        contentResolver.query(
            DocumentsContract.buildRootsUri(AUTHORITY),
            null,
            null,
            null,
            null
        )!!.use { cursor ->
            assertTrue(cursor.moveToFirst())
            assertNotEquals(
                0,
                cursor.getInt(DocumentsContract.Root.COLUMN_FLAGS) and
                    DocumentsContract.Root.FLAG_SUPPORTS_CREATE
            )
        }

        // ...and also by the parent directory.
        contentResolver.query(
            DocumentsContract.buildDocumentUri(AUTHORITY, "foo/"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            assertTrue(cursor.moveToFirst())
            assertNotEquals(
                0,
                cursor.getInt(DocumentsContract.Document.COLUMN_FLAGS) and
                    DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE
            )
        }

        contentResolver.query(
            DocumentsContract.buildChildDocumentsUri(AUTHORITY, "foo/"),
            null,
            null,
            null,
            null,
        )!!.use { cursor ->
            // Create the directory and wait for the notification
            val newUri = waitForNotification(cursor) {
                DocumentsContract.createDocument(
                    contentResolver,
                    DocumentsContract.buildDocumentUri(AUTHORITY, "foo/"),
                    DocumentsContract.Document.MIME_TYPE_DIR,
                    "bar",
                )
            }

            assertEquals(
                DocumentsContract.buildDocumentUri(AUTHORITY, "foo/bar"),
                newUri
            )
        }

        withSession {
            findRepository("foo").apply {
                val entries = readDirectory("")
                assertEquals(1, entries.size)
                assertEquals("bar", entries[0].name)
                assertEquals(EntryType.DIRECTORY, entries[0].entryType)
            }
        }
    }

    @Test
    fun testDeleteFile() {
        withSession {
            setStoreDir(storeDir)
            createRepository("foo").apply {
                createFile("a.txt").apply {
                    write(0, "this is a".toByteArray())
                    close()
                }

                createFile("b.txt").apply {
                    write(0, "this is b".toByteArray())
                    close()
                }
            }
        }

        val parentUri = DocumentsContract.buildChildDocumentsUri(AUTHORITY, "foo/")

        contentResolver.query(
            parentUri,
            null,
            null,
            null,
            null
        )!!.use { cursor ->
            assertTrue(cursor.moveToFirst())
            assertEquals("a.txt", cursor.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME))

            // Check the file supports delete
            assertNotEquals(
                0,
                cursor.getInt(DocumentsContract.Document.COLUMN_FLAGS) and
                    DocumentsContract.Document.FLAG_SUPPORTS_DELETE
            )

            val fileId = cursor.getString(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
            val fileUri = DocumentsContract.buildDocumentUri(AUTHORITY, fileId)

            // Delete the file and wait for the notification
            val result = waitForNotification(cursor) {
                DocumentsContract.deleteDocument(contentResolver, fileUri)
            }

            assertTrue(result)
        }

        withSession {
            findRepository("foo").apply {
                val entries = readDirectory("")
                assertEquals(1, entries.size)
                assertEquals("b.txt", entries[0].name)
            }
        }
    }

    // Creates a temporary Ouisync Session and pass it to the given block.
    private fun <R> withSession(block: suspend Session.() -> R): R = runBlocking {
        val session = Session.create(context.getConfigPath())

        try {
            session.block()
        } finally {
            session.close()
        }
    }

    // Starts the OuisyncService
    private fun startService() {
        val latch = CountDownLatch(1)
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                latch.countDown()
            }
        }

        context.registerReceiver(
            receiver,
            IntentFilter(OuisyncService.ACTION_STARTED),
            Context.RECEIVER_NOT_EXPORTED,
        )

        try {
            context.startService(Intent(context, OuisyncService::class.java))
            latch.await()
        } finally {
            context.unregisterReceiver(receiver)
        }
    }

    // Stops the OuisyncService and wait until it's fully stopped. If the service wasn't running,
    // returns immediately.
    private fun stopService() {
        val latch = CountDownLatch(1)
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                assertEquals(1, resultCode)
                latch.countDown()
            }
        }

        context.sendOrderedBroadcast(
            Intent(OuisyncService.ACTION_STOP).setPackage(context.getPackageName()),
            null,
            receiver,
            null,
            0,
            null,
            null,
        )

        latch.await()
    }

    // Execute the given block and wait until a notification of changes in the cursor's data is
    // delivered.
    private fun <T> waitForNotification(cursor: Cursor, block: () -> T): T {
        var latch = CountDownLatch(1)
        cursor.registerContentObserver(object : ContentObserver(Handler(context.mainLooper)) {
            override fun onChange(selfChange: Boolean) {
                latch.countDown()
            }
        })

        val result = block()

        assertTrue(latch.await(10, TimeUnit.SECONDS))

        return result
    }
}

private fun randomString(size: Int): String {
    val rng = Random.Default
    val alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
    val builder = StringBuilder(size)

    for (i in 0 ..< size) {
        builder.append(alphabet[rng.nextInt(alphabet.length)])
    }

    return builder.toString()
}

private fun Cursor.getString(columnName: String): String = getString(getColumnIndexOrThrow(columnName))
private fun Cursor.getInt(columnName: String): Int = getInt(getColumnIndexOrThrow(columnName))

