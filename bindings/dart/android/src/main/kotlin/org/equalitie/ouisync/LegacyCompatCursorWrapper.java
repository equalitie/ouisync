package org.equalitie.ouisync;

import android.database.Cursor;
import android.database.CursorWrapper;
import android.net.Uri;
import java.util.Arrays;
import static android.provider.MediaStore.MediaColumns.DATA;
import static android.provider.MediaStore.MediaColumns.MIME_TYPE;

/**
 * Wraps the Cursor returned by an ordinary FileProvider,
 * StreamProvider, or other ContentProvider. If the query()
 * requests _DATA or MIME_TYPE, adds in some values for
 * that column, so the client getting this Cursor is less
 * likely to crash. Of course, clients should not be requesting
 * either of these columns in the first place...
 */
public class LegacyCompatCursorWrapper extends CursorWrapper {
    final private int fakeDataColumn;
    final private int fakeMimeTypeColumn;
    final private String mimeType;
    final private Uri uriForDataColumn;

    /**
     * Constructor.
     *
     * @param cursor the Cursor to be wrapped
     */
    public LegacyCompatCursorWrapper(Cursor cursor) {
        this(cursor, null);
    }

    /**
     * Constructor.
     *
     * @param cursor the Cursor to be wrapped
     * @param mimeType the MIME type of the content represented
     *                 by the Uri that generated this Cursor, should
     *                 we need it
     */
    public LegacyCompatCursorWrapper(Cursor cursor, String mimeType) {
        this(cursor, mimeType, null);
    }

    /**
     * Constructor.
     *
     * @param cursor the Cursor to be wrapped
     * @param mimeType the MIME type of the content represented
     *                 by the Uri that generated this Cursor, should
     *                 we need it
     * @param uriForDataColumn Uri to return for the _DATA column
     */
    public LegacyCompatCursorWrapper(Cursor cursor, String mimeType,
                                     Uri uriForDataColumn) {
        super(cursor);

        this.uriForDataColumn=uriForDataColumn;

        if (cursor.getColumnIndex(DATA)>=0) {
            fakeDataColumn=-1;
        }
        else {
            fakeDataColumn=cursor.getColumnCount();
        }

        if (cursor.getColumnIndex(MIME_TYPE)>=0) {
            fakeMimeTypeColumn=-1;
        }
        else if (fakeDataColumn==-1) {
            fakeMimeTypeColumn=cursor.getColumnCount();
        }
        else {
            fakeMimeTypeColumn=fakeDataColumn+1;
        }

        this.mimeType=mimeType;
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public int getColumnCount() {
        int count=super.getColumnCount();

        if (!cursorHasDataColumn()) {
            count+=1;
        }

        if (!cursorHasMimeTypeColumn()) {
            count+=1;
        }

        return(count);
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public int getColumnIndex(String columnName) {
        if (!cursorHasDataColumn() && DATA.equalsIgnoreCase(
                columnName)) {
            return(fakeDataColumn);
        }

        if (!cursorHasMimeTypeColumn() && MIME_TYPE.equalsIgnoreCase(
                columnName)) {
            return(fakeMimeTypeColumn);
        }

        return(super.getColumnIndex(columnName));
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public String getColumnName(int columnIndex) {
        if (columnIndex==fakeDataColumn) {
            return(DATA);
        }

        if (columnIndex==fakeMimeTypeColumn) {
            return(MIME_TYPE);
        }

        return(super.getColumnName(columnIndex));
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public String[] getColumnNames() {
        if (cursorHasDataColumn() && cursorHasMimeTypeColumn()) {
            return(super.getColumnNames());
        }

        String[] orig=super.getColumnNames();
        String[] result=Arrays.copyOf(orig, getColumnCount());

        if (!cursorHasDataColumn()) {
            result[fakeDataColumn]=DATA;
        }

        if (!cursorHasMimeTypeColumn()) {
            result[fakeMimeTypeColumn]=MIME_TYPE;
        }

        return(result);
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public String getString(int columnIndex) {
        if (!cursorHasDataColumn() && columnIndex==fakeDataColumn) {
            if (uriForDataColumn!=null) {
                return(uriForDataColumn.toString());
            }

            return(null);
        }

        if (!cursorHasMimeTypeColumn() && columnIndex==fakeMimeTypeColumn) {
            return(mimeType);
        }

        return(super.getString(columnIndex));
    }

    /**
     * {@inheritDoc}
     */
    @Override
    public int getType(int columnIndex) {
        if (!cursorHasDataColumn() && columnIndex==fakeDataColumn) {
            return(Cursor.FIELD_TYPE_STRING);
        }

        if (!cursorHasMimeTypeColumn() && columnIndex==fakeMimeTypeColumn) {
            return(Cursor.FIELD_TYPE_STRING);
        }

        return(super.getType(columnIndex));
    }

    /**
     * @return true if the Cursor has a _DATA column, false otherwise
     */
    private boolean cursorHasDataColumn() {
        return(fakeDataColumn==-1);
    }

    /**
     * @return true if the Cursor has a MIME_TYPE column, false
     * otherwise
     */
    private boolean cursorHasMimeTypeColumn() {
        return(fakeMimeTypeColumn==-1);
    }
}
