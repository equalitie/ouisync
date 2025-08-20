package org.equalitie.ouisync.kotlin.android

import android.content.BroadcastReceiver
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.provider.DocumentsContract
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.util.concurrent.Semaphore
import kotlin.random.Random
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.runBlocking
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.close
import org.equalitie.ouisync.kotlin.client.create
import org.equalitie.ouisync.kotlin.server.initLog
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OuisyncProviderTest {
    companion object {
        private val TAG = OuisyncProviderTest::class.simpleName
        private const val AUTHORITY = "org.equalitie.ouisync.kotlin.android.test.documents"
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
            assertNotNull(cursor)
            assertEquals(1, cursor.count)

            assertTrue(cursor.moveToFirst())

            assertEquals(
                "default",
                cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_ROOT_ID)),
            )
            assertEquals(
                "repos",
                cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_DOCUMENT_ID)),
            )
            assertEquals(
                R.mipmap.ouisync_provider_root_icon,
                cursor.getInt(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_ICON)),
            )
            assertEquals(
                DocumentsContract.Root.MIME_TYPE_ITEM,
                cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_MIME_TYPES)),
            )
            assertEquals(
                "Ouisync",
                cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_TITLE)),
            )

            // TODO:
            // assertEquals(
            //     "???",
            //     cursor.getString(cursor.getColumnIndexOrThrow(DocumentsContract.Root.COLUMN_SUMMARY)),
            // )
        }
    }

    @Test
    fun testQueryRootDocument() {
        withSession { session ->
            session.setStoreDir(storeDir)
            session.createRepository("foo")
        }

        val uri = DocumentsContract.buildDocumentUri(AUTHORITY, "repos")

        contentResolver.query(uri, null, null, null, null)!!.use { cursor ->
            assertEquals(1, cursor.count)
            assertTrue(cursor.moveToFirst())

        }
    }

    // Ensure the OuisyncService is running, create a temporary Ouisync Session and execute the
    // given block with it.
    private fun <R> withSession(block: suspend (Session) -> R): R {
        val deferred = CompletableDeferred<Unit>()
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                deferred.complete(Unit)
            }
        }

        context.registerReceiver(
            receiver,
            IntentFilter(OuisyncService.ACTION_STARTED),
            Context.RECEIVER_NOT_EXPORTED,
        )

        try {
            context.startService(Intent(context, OuisyncService::class.java))

            return runBlocking {
                deferred.await()

                val session = Session.create(context.getConfigPath())

                try {
                    block(session)
                } finally {
                    session.close()
                }
            }
        } finally {
            context.unregisterReceiver(receiver)
        }
    }

    // Stat the OuisyncService
    private fun startService() {

    }

    // Stops the OuisyncService and wait until it's fully stopped. If the service wasn't running,
    // returns immediately.
    private fun stopService() {
        val semaphore = Semaphore(1).apply { acquire() }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                semaphore.release()
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

        semaphore.acquire()
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

