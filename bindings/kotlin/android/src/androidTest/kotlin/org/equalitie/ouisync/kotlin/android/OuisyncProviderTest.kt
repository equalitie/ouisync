package org.equalitie.ouisync.kotlin.android

import android.content.ContentResolver
import android.content.Context
import android.provider.DocumentsContract
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
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

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        contentResolver = context.contentResolver
    }

    // @After
    // fun tearDown() {
    // }

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
        val uri = DocumentsContract.buildDocumentUri(AUTHORITY, "repos")

        Log.d(TAG, "FOOOOOOOO: $uri")

        contentResolver.query(uri, null, null, null, null)!!.use { cursor ->
            assertNotNull(cursor)
            assertEquals(1, cursor.count)

            assertTrue(cursor.moveToFirst())

        }
    }

}
