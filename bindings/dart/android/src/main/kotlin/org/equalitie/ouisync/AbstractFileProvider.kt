package org.equalitie.ouisync

import android.content.ContentProvider
import android.content.ContentValues
import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.URLConnection

abstract class AbstractFileProvider: ContentProvider() {
    private val TAG = javaClass.simpleName;

    companion object {
        private val OPENABLE_PROJECTION = arrayOf(
            OpenableColumns.DISPLAY_NAME,
            OpenableColumns.SIZE
        )
    }

    override fun query(
            uri: Uri,
            projection: Array<out String>?,
            selection: String?,
            selectionArgs: Array<out String>?,
            sortOrder: String?
    ): Cursor? {
        val projection = projection ?: OPENABLE_PROJECTION
        val cursor = MatrixCursor(projection, 1)
        val row = cursor.newRow()

        for (col in projection) {
            when (col) {
                OpenableColumns.DISPLAY_NAME -> row.add(getFileName(uri))
                OpenableColumns.SIZE         -> row.add(getDataLength(uri))
                else                         -> row.add(null) // unknown, just add null
            }
        }

        // TODO: Do we actually need the wrapper?
        return LegacyCompatCursorWrapper(cursor)
    }

    override fun getType(uri: Uri): String? =
        URLConnection.guessContentTypeFromName(uri.toString())

    protected open fun getFileName(uri: Uri): String? =
        uri.lastPathSegment

    protected open fun getDataLength(uri: Uri): Long =
        uri.pathSegments[0].toLongOrNull() ?: AssetFileDescriptor.UNKNOWN_LENGTH

    override fun insert(uri: Uri, values: ContentValues?): Uri? {
        TODO("Not yet implemented")
    }

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int {
        TODO("Not yet implemented")
    }

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?): Int {
        TODO("Not yet implemented")
    }
}
