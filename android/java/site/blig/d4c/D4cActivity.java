package site.blig.d4c;

import android.app.NativeActivity;
import android.content.Intent;
import android.content.ClipData;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;
import java.io.*;
import java.util.*;
import org.json.*;

/** Thin Android integration; the UI, connection and workspace remain Rust. */
public final class D4cActivity extends NativeActivity {
    // NativeActivity's own dlopen doesn't register the library with this Java
    // class loader, which is required to resolve our JNI result callback.
    static { System.loadLibrary("vox_d4c"); }
    private android.widget.EditText composer;
    private android.widget.PopupWindow composerWindow;
    private String composerSession="";
    private long composerRevision=0;
    private boolean composerUpdating=false;
    private boolean composerActive=false;
    private native void composerWindowHidden();
    @Override protected void onResume(){super.onResume();composerActive=true;composerWindowHidden();}
    private native void finishNativeUi();
    @Override protected void onPause(){
        composerActive=false;hideComposer();
        // A new Activity can be created before the old one's onDestroy. Stop
        // the old native UI before that overlap, while keeping the service alive.
        if(isFinishing())finishNativeUi();
        super.onPause();
    }
    private native void visibleBottomChanged(int bottom);
    private native void composerChanged(String session,String text,long revision);
    public void renderComposer(String session,String text,int left,int top,int width,int height,long expectedRevision,boolean focus) {
        if(!composerActive)return;
        if(composer==null){
            composer=new android.widget.EditText(this);
            composer.setBackgroundColor(android.graphics.Color.TRANSPARENT);
            composer.setTextColor(android.graphics.Color.rgb(235,235,240));
            composer.setHintTextColor(android.graphics.Color.rgb(135,135,143));
            composer.setHint("Message…  / for commands");
            composer.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP,14);
            composer.setPadding(0,0,0,0);
            composer.setGravity(android.view.Gravity.TOP|android.view.Gravity.START);
            composer.setInputType(android.text.InputType.TYPE_CLASS_TEXT|android.text.InputType.TYPE_TEXT_FLAG_MULTI_LINE|android.text.InputType.TYPE_TEXT_FLAG_CAP_SENTENCES|android.text.InputType.TYPE_TEXT_FLAG_AUTO_CORRECT);
            composer.setImeOptions(android.view.inputmethod.EditorInfo.IME_FLAG_NO_EXTRACT_UI);
            composer.setFilters(new android.text.InputFilter[]{new android.text.InputFilter.LengthFilter(131072)});
            composer.setSelectAllOnFocus(false);
            composer.addTextChangedListener(new android.text.TextWatcher(){
                public void beforeTextChanged(CharSequence s,int start,int count,int after){}
                public void onTextChanged(CharSequence s,int start,int before,int count){}
                public void afterTextChanged(android.text.Editable s){if(!composerUpdating){composerRevision++;composerChanged(composerSession,s.toString(),composerRevision);}}
            });
            // NativeActivity owns the EGL window surface: a normal child View is
            // painted underneath it. Use a tiny native subwindow over just the editor.
            composerWindow=new android.widget.PopupWindow(composer,1,1,true);
            composerWindow.setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(android.graphics.Color.TRANSPARENT));
            composerWindow.setElevation(0);
            composerWindow.setOnDismissListener(()->composerWindowHidden());
            composerWindow.setInputMethodMode(android.widget.PopupWindow.INPUT_METHOD_NEEDED);
            composerWindow.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING);
            composerWindow.setClippingEnabled(false);
            composerWindow.setAttachedInDecor(false);
            if(android.os.Build.VERSION.SDK_INT>=29)composerWindow.setTouchModal(false);
            else composerWindow.setOutsideTouchable(true);
        }
        boolean changedSession=!composerSession.equals(session);
        if(changedSession || (expectedRevision==composerRevision && !composer.getText().toString().equals(text))){
            composerUpdating=true;composerSession=session;composer.setText(text);composer.setSelection(composer.length());composerUpdating=false;
        }
        // Session changes are programmatic, so TextWatcher is deliberately muted.
        // Acknowledge the new session with its actual text and revision together.
        if(changedSession)composerChanged(composerSession,composer.getText().toString(),composerRevision);
        composer.setVisibility(android.view.View.VISIBLE);
        if(!composerWindow.isShowing())composerWindow.showAtLocation(getWindow().getDecorView(),android.view.Gravity.TOP|android.view.Gravity.LEFT,left,top);
        composerWindow.update(left,top,width,height);
        if(focus){composer.requestFocus();composer.post(()->((android.view.inputmethod.InputMethodManager)getSystemService(INPUT_METHOD_SERVICE)).showSoftInput(composer,android.view.inputmethod.InputMethodManager.SHOW_IMPLICIT));}
    }
    public void hideComposer(){if(composer!=null&&composerWindow.isShowing()){composer.clearFocus();((android.view.inputmethod.InputMethodManager)getSystemService(INPUT_METHOD_SERVICE)).hideSoftInputFromWindow(composer.getWindowToken(),0);composerWindow.dismiss();}}

    public void copyText(String text){
        android.content.ClipboardManager clipboard=(android.content.ClipboardManager)getSystemService(CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(android.content.ClipData.newPlainText("D4C",text));
        // Android 13+ already displays its own copy confirmation.
        if(android.os.Build.VERSION.SDK_INT<33)android.widget.Toast.makeText(this,"Copied",android.widget.Toast.LENGTH_SHORT).show();
    }

    // Draw complete grapheme clusters with Android's font fallback/emoji shaper.
    private android.graphics.Typeface bundledEmoji;
    public int[] rasterizeEmoji(String cluster,float size,boolean color){
        if(cluster.length()>64 || !Float.isFinite(size)||size<1||size>256)return new int[0];
        android.graphics.Paint paint=new android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG);
        paint.setTextSize(size);paint.setTypeface(android.graphics.Typeface.DEFAULT);paint.setColor(android.graphics.Color.WHITE);
        if(color){
            if(bundledEmoji==null){try{bundledEmoji=android.graphics.Typeface.createFromAsset(getAssets(),"NotoColorEmoji.ttf");}catch(RuntimeException ignored){}}
            if(bundledEmoji!=null){paint.setTypeface(bundledEmoji);if(!paint.hasGlyph(cluster))paint.setTypeface(android.graphics.Typeface.DEFAULT);}
        }
        if(!cluster.equals("🗐")&&!paint.hasGlyph(cluster))return new int[0];
        android.graphics.Paint.FontMetrics metrics=paint.getFontMetrics();float advance=paint.measureText(cluster);
        int width=Math.max(1,(int)Math.ceil(advance)+8),height=Math.max(1,(int)Math.ceil(metrics.descent-metrics.ascent)+8);
        if(width>1024||height>1024)return new int[0];
        android.graphics.Bitmap bitmap=android.graphics.Bitmap.createBitmap(width,height,android.graphics.Bitmap.Config.ARGB_8888);
        try{
            android.graphics.Canvas canvas=new android.graphics.Canvas(bitmap);
            if(cluster.equals("🗐")){
                // CommonMark's copy symbol is absent from some Android fonts.
                // Draw its two-page outline without relying on a Unicode glyph.
                float x=4,y=4-metrics.ascent-size*0.8f;
                paint.setStyle(android.graphics.Paint.Style.STROKE);paint.setStrokeWidth(Math.max(1,size/13));
                canvas.drawRoundRect(x+size*.24f,y,x+size*.78f,y+size*.62f,size*.06f,size*.06f,paint);
                canvas.drawRoundRect(x,y+size*.22f,x+size*.54f,y+size*.84f,size*.06f,size*.06f,paint);
            }else{canvas.drawText(cluster,4,4-metrics.ascent,paint);}
            int[] result=new int[5+width*height];result[0]=width;result[1]=height;result[2]=Math.round(advance*1000);result[3]=-4;result[4]=(int)Math.floor(metrics.ascent)-4;bitmap.getPixels(result,5,width,0,0,width,height);return result;}finally{bitmap.recycle();}
    }
    private static final int PICK_FILES = 4104;
    private String pickerToken;
    private native void openSession(String session);
    private native void filesPicked(String token, String result);

    @Override protected void onCreate(android.os.Bundle state){
        super.onCreate(state);
        getWindow().getDecorView().getViewTreeObserver().addOnGlobalLayoutListener(()->{android.graphics.Rect frame=new android.graphics.Rect();getWindow().getDecorView().getWindowVisibleDisplayFrame(frame);if(frame.bottom>0)visibleBottomChanged(frame.bottom);});
        deliverSession(getIntent());
        if(android.os.Build.VERSION.SDK_INT>=33&&checkSelfPermission("android.permission.POST_NOTIFICATIONS")!=android.content.pm.PackageManager.PERMISSION_GRANTED){requestPermissions(new String[]{"android.permission.POST_NOTIFICATIONS"},4105);}
        Intent service=new Intent(this,D4cNotificationService.class);
        if(android.os.Build.VERSION.SDK_INT>=26)startForegroundService(service);else startService(service);
    }
    private void deliverSession(Intent intent){String session=intent.getStringExtra("d4c.session");if(session!=null&&!session.isEmpty()){openSession(session);intent.removeExtra("d4c.session");}}
    @Override protected void onNewIntent(Intent intent){super.onNewIntent(intent);setIntent(intent);deliverSession(intent);}
    public String takeNotificationSession(){String session=getIntent().getStringExtra("d4c.session");getIntent().removeExtra("d4c.session");return session==null?"":session;}
    public void notifyLocal(String title,String body,boolean sound){D4cNotifications.alert(this,"local:"+System.currentTimeMillis(),title,body,"",sound);}
    public void pickFiles(String token) {
        if (pickerToken != null) { reply(token, "A file picker is already open"); return; }
        pickerToken = token;
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.setType("*/*");
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try { startActivityForResult(intent, PICK_FILES); }
        catch (Exception error) { pickerToken = null; reply(token, "Could not open Android files: " + error.getMessage()); }
    }
    private void reply(String token, String error) {
        try {
            JSONObject result = new JSONObject();
            if (error != null) result.put("error", error); else result.put("files", new JSONArray());
            filesPicked(token, result.toString());
        } catch (JSONException ignored) { }
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != PICK_FILES || pickerToken == null) return;
        final String token = pickerToken;
        pickerToken = null;
        if (result != RESULT_OK || data == null) { reply(token, null); return; }
        final ArrayList<Uri> uris = new ArrayList<>();
        ClipData clip = data.getClipData();
        if (clip != null) {
            if (clip.getItemCount() > 8) { reply(token, "Choose up to 8 files at a time"); return; }
            for (int i=0; i<clip.getItemCount(); i++) uris.add(clip.getItemAt(i).getUri());
        } else if (data.getData() != null) uris.add(data.getData());
        // Stream content URIs off the UI thread. Never resolve a provider URI to a shared path.
        new Thread(() -> copySelected(token, uris), "D4C attachments").start();
    }
    private void copySelected(String token, ArrayList<Uri> uris) {
        File directory = new File(new File(getCacheDir(), "d4c-attachments"), UUID.randomUUID().toString());
        try {
            if (!directory.mkdirs()) throw new IOException("Could not create attachment cache");
            JSONArray files = new JSONArray();
            long total = 0;
            for (Uri uri: uris) {
                String name = "Attachment";
                try (Cursor cursor = getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
                    if (cursor != null && cursor.moveToFirst()) {
                        int n = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                        if (n >= 0 && !cursor.isNull(n)) name = cursor.getString(n);
                        int size = cursor.getColumnIndex(OpenableColumns.SIZE);
                        if (size >= 0 && !cursor.isNull(size) && cursor.getLong(size) > 6000000) throw new IOException("Choose a file under 6 MB");
                    }
                }
                File output = new File(directory, UUID.randomUUID().toString());
                long count = 0;
                try (InputStream input = getContentResolver().openInputStream(uri); OutputStream stream = new FileOutputStream(output)) {
                    if (input == null) throw new IOException("Could not read selected file");
                    byte[] buffer = new byte[16384];
                    int read;
                    while ((read = input.read(buffer)) != -1) {
                        count += read; total += read;
                        if (count > 6000000 || total > 12000000) throw new IOException("Attachment limit: 6 MB per file, 12 MB total");
                        stream.write(buffer, 0, read);
                    }
                }
                String mime = getContentResolver().getType(uri);
                JSONObject file = new JSONObject();
                file.put("name", name); file.put("mime", mime == null ? "" : mime); file.put("path", output.getAbsolutePath());
                files.put(file);
            }
            JSONObject response = new JSONObject(); response.put("files", files);
            // Rust reads bounded bytes synchronously on this worker before returning.
            filesPicked(token, response.toString());
        } catch (Exception error) { reply(token, error.getMessage() == null ? "Could not attach selected file" : error.getMessage()); }
        finally {
            File[] temporary = directory.listFiles();
            if (temporary != null) for (File file: temporary) file.delete();
            directory.delete();
        }
    }
}
