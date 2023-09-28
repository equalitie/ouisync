package ie.equalit.ouisync_plugin

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
                OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
    }

    override fun query(
            uri: Uri,
            projection: Array<out String>?,
            selection: String?,
            selectionArgs: Array<out String>?,
            sortOrder: String?
    ): Cursor? {
        var projection = projection

        if (projection == null) {
            projection = OPENABLE_PROJECTION
        }

        val cursor = MatrixCursor(projection, 1)
        val b = cursor.newRow()
        for (col in projection!!) {
            when {
                OpenableColumns.DISPLAY_NAME == col -> {
                    b.add(getFileName(uri))
                    Log.d(TAG, "OpenableColumns.DISPLAY_NAME: ${getFileName(uri)}")
                }
                OpenableColumns.SIZE == col -> {
                    b.add(getDataLength(uri))
                    Log.d(TAG, "OpenableColumns.SIZE: ${getDataLength(uri)}")
                }
                else -> { // unknown, so just add null
                    b.add(null)
                    Log.d(TAG, "Unknown column $col. NULL")
                }
            }
        }

        return LegacyCompatCursorWrapper(cursor)
    }

    override fun getType(uri: Uri): String? {
        var type = URLConnection.guessContentTypeFromName(uri.toString());
        Log.d(TAG, "getType: $uri -> $type")
        return type
    }

    protected open fun getFileName(uri: Uri): String? {
        Log.d(TAG, "getFileName: ${uri.lastPathSegment}")
        return uri.lastPathSegment
    }

    protected open fun getDataLength(uri: Uri): Long {
        val segments = uri.pathSegments
        if (segments[0].toLongOrNull() != null) {
            Log.d(TAG, "getDataLength: ${segments[0].toLong()}")

            return segments[0].toLong()
        }
         
        Log.d(TAG, "getDataLength: File size couldn't be obtained. AssetFileDescriptor.UNKNOWN_LENGTH is returned")

        return AssetFileDescriptor.UNKNOWN_LENGTH
    }

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
