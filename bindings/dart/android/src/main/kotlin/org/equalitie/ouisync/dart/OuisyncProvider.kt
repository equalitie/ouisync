package org.equalitie.ouisync.dart

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.DocumentsProvider
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import org.equalitie.ouisync.kotlin.client.Session
import org.equalitie.ouisync.kotlin.client.close
import org.equalitie.ouisync.kotlin.client.create

// Provider that exposes Ouisync repositories to other apps.
class OuisyncProvider : DocumentsProvider() {
    companion object {
        val DEFAULT_ROOT_PROJECTION = arrayOf(
            DocumentsContract.Root.COLUMN_DOCUMENT_ID,
            DocumentsContract.Root.COLUMN_FLAGS,
            DocumentsContract.Root.COLUMN_ICON,
            DocumentsContract.Root.COLUMN_ROOT_ID,
            DocumentsContract.Root.COLUMN_SUMMARY,
            DocumentsContract.Root.COLUMN_TITLE,
        )

        val DEFAULT_DOCUMENT_PROJECTION = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            // DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            // DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            // DocumentsContract.Document.COLUMN_FLAGS,
            // DocumentsContract.Document.COLUMN_SIZE
        )

        val ROOT_ID = "root"
    }

    private val scope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private val session = MutableStateFlow<Session?>(null)

    private val receiver =
        object : BroadcastReceiver() {
            override fun onReceive(
                context: Context,
                intent: Intent,
            ) {
                scope.launch {
                    session.value?.close()
                    session.value = Session.create(context.getConfigPath())
                }
            }
        }

    override fun onCreate(): Boolean {
        Log.d(TAG, "${this::class.simpleName}.onCreate")

        return true
    }

    override fun queryRoots(projection: Array<out String>?): Cursor {
        Log.d(TAG, "${this::class.simpleName}.queryRoots($projection)");

        val result = MatrixCursor(projection ?: DEFAULT_ROOT_PROJECTION)
        val row = result.newRow()

        row.add(DocumentsContract.Root.COLUMN_DOCUMENT_ID, "$ROOT_ID:")
        row.add(DocumentsContract.Root.COLUMN_FLAGS, DocumentsContract.Root.FLAG_SUPPORTS_IS_CHILD)
        row.add(DocumentsContract.Root.COLUMN_ICON, R.mipmap.ouisync_provider_root_icon)
        row.add(DocumentsContract.Root.COLUMN_MIME_TYPES, DocumentsContract.Root.MIME_TYPE_ITEM)
        row.add(DocumentsContract.Root.COLUMN_ROOT_ID, ROOT_ID)
        row.add(DocumentsContract.Root.COLUMN_SUMMARY, "Blah blah blah")
        row.add(DocumentsContract.Root.COLUMN_TITLE, "Ouisync")

        return result
    }

    override fun queryChildDocuments(
        parentDocumentId: String?,
        projection: Array<out String>?,
        sortOrder: String?
    ): Cursor {
        Log.d(TAG, "${this::class.simpleName}.queryChildDocuments($parentDocumentId, $projection, $sortOrder)")

        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)

        return result
    }

    override fun queryDocument(documentId: String?, projection: Array<out String>?): Cursor {
        Log.d(TAG, "${this::class.simpleName}.queryDocument($documentId, $projection)")

        val locator = if (documentId != null) Locator.parse(documentId) else Locator.ROOT
        Log.d(TAG, "${this::class.simpleName}.queryDocument locator=$locator")

        val result = MatrixCursor(projection ?: DEFAULT_DOCUMENT_PROJECTION)
        // val row = result


        return result
    }

    override fun openDocument(
        documentId: String,
        mode: String,
        signal: CancellationSignal?
    ): ParcelFileDescriptor {
        Log.d(TAG, "${this::class.simpleName}.openDocument($documentId, $mode, ..)")

        throw NotImplementedError()
    }

    private data class Locator(val repo: String, val path: String) {
        companion object {
            fun parse(documentId: String): Locator {
                val rootSepIndex = documentId.indexOf(':')
                require(rootSepIndex >= 0) { "invalid document id" }

                val rootId = documentId.substring(0, rootSepIndex)
                require(rootId == ROOT_ID) { "invalid root id" }

                val rest = documentId.substring(rootSepIndex + 1)

                if (rest.isEmpty()) {
                    return ROOT
                }

                val repoSepIndex = rest.indexOf('/')
                if (repoSepIndex < 0) {
                    return Locator(rest, "")
                } else {
                    return Locator(
                        repo = rest.substring(0, repoSepIndex),
                        path = rest.substring(repoSepIndex + 1)
                    )
                }
            }

            val ROOT = Locator("", "")
        }

        override fun toString() = if (repo.isEmpty()) "" else "$repo/$path"
    }
}


