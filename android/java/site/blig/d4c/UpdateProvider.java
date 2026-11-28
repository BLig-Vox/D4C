package site.blig.d4c;

import android.content.*;
import android.database.*;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.*;

public final class UpdateProvider extends ContentProvider {
    public boolean onCreate(){return true;}
    private File apk(Uri u) {
        if(!"/update.apk".equals(u.getPath())) throw new IllegalArgumentException("Unknown update");
        return new File(getContext().getCacheDir(),"updates/update.apk");
    }
    public ParcelFileDescriptor openFile(Uri u,String mode) throws FileNotFoundException {
        if(!"r".equals(mode)) throw new FileNotFoundException("Read only");
        return ParcelFileDescriptor.open(apk(u),ParcelFileDescriptor.MODE_READ_ONLY);
    }
    public String getType(Uri u){apk(u);return "application/vnd.android.package-archive";}
    public Cursor query(Uri u,String[] projection,String where,String[] args,String order) {
        File f=apk(u);
        String[] cols=projection==null?new String[]{OpenableColumns.DISPLAY_NAME,OpenableColumns.SIZE}:projection;
        MatrixCursor c=new MatrixCursor(cols); MatrixCursor.RowBuilder row=c.newRow();
        for(String col:cols) row.add(OpenableColumns.SIZE.equals(col)?f.length():OpenableColumns.DISPLAY_NAME.equals(col)?f.getName():null);
        return c;
    }
    public Uri insert(Uri u,ContentValues v){throw new UnsupportedOperationException();}
    public int update(Uri u,ContentValues v,String s,String[] a){throw new UnsupportedOperationException();}
    public int delete(Uri u,String s,String[] a){throw new UnsupportedOperationException();}
}
