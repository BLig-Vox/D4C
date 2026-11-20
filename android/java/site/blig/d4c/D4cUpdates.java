package site.blig.d4c;

import android.app.AlertDialog;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;
import android.widget.Toast;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.*;
import java.net.*;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

final class D4cUpdates {
    private static final String API = "https://api.github.com/repos/BLig-Vox/D4C/releases";
    private static final Pattern VERSION = Pattern.compile("v?(\\d+)\\.(\\d+)\\.(\\d+)(?:-alpha\\.(\\d+))?");
    private final D4cActivity activity;
    private final SharedPreferences prefs;
    private boolean checking, downloading;
    private File awaitingPermission;

    D4cUpdates(D4cActivity app) {
        activity = app;
        prefs = app.getSharedPreferences("updates", 0);
    }

    static long[] version(String text) {
        Matcher m = VERSION.matcher(text);
        if (!m.matches()) return null;
        long[] v = new long[4];
        try {
            for (int i=0;i<4;i++) v[i] = m.group(i+1)==null ? Long.MAX_VALUE : Long.parseLong(m.group(i+1));
        } catch (NumberFormatException e) { return null; }
        return v;
    }

    static boolean newer(long[] a, long[] b) {
        for (int i=0;i<a.length;i++) if (a[i]!=b[i]) return a[i]>b[i];
        return false;
    }

    void menu() {
        boolean alpha = prefs.getBoolean("alpha", installed().contains("-alpha."));
        new AlertDialog.Builder(activity).setTitle("System update")
            .setSingleChoiceItems(new String[]{"Stable","Stable and alpha"},alpha?1:0,(d,i)->
                prefs.edit().putBoolean("alpha",i==1).apply())
            .setNegativeButton("Cancel",null).setPositiveButton("Check for updates",(d,i)->check(true)).show();
    }

    private String installed() {
        try { return activity.getPackageManager().getPackageInfo(activity.getPackageName(),0).versionName; }
        catch (PackageManager.NameNotFoundException e) { throw new IllegalStateException(e); }
    }

    void resume() {
        if (awaitingPermission!=null) {
            File apk=awaitingPermission;
            awaitingPermission=null;
            if (canInstall()) launch(apk);
            return;
        }
        if (System.currentTimeMillis()-prefs.getLong("last",0)>6*60*60*1000L) check(false);
    }

    private void tell(String msg) { Toast.makeText(activity,msg,Toast.LENGTH_LONG).show(); }

    private static HttpURLConnection open(String url) throws IOException {
        HttpURLConnection c=(HttpURLConnection)new URL(url).openConnection();
        c.setConnectTimeout(15000); c.setReadTimeout(30000);
        c.setRequestProperty("User-Agent","D4C-Android-Updates");
        return c;
    }

    private JSONObject find(boolean alpha, String current) throws Exception {
        long[] best=version(current);
        if (best==null) throw new IOException("Unknown installed version");
        JSONObject found=null;
        for (int page=1;;page++) {
            HttpURLConnection c=open(API+"?per_page=100&page="+page);
            JSONArray rows;
            try (BufferedReader in=new BufferedReader(new InputStreamReader(c.getInputStream(),"UTF-8"))) {
                StringBuilder json=new StringBuilder();
                for(String line;(line=in.readLine())!=null;) json.append(line);
                rows=new JSONArray(json.toString());
            } finally { c.disconnect(); }
            for(int i=0;i<rows.length();i++) {
                JSONObject r=rows.getJSONObject(i);
                String tag=r.optString("tag_name");
                long[] v=version(tag);
                if(v==null || !newer(v,best) || r.optBoolean("draft") ||
                    (!alpha && (r.optBoolean("prerelease") || v[3]!=Long.MAX_VALUE))) continue;
                String name="d4c-"+tag.replaceFirst("^v","")+"-android-arm64.apk";
                JSONArray assets=r.getJSONArray("assets");
                for(int j=0;j<assets.length();j++) {
                    JSONObject asset=assets.getJSONObject(j);
                    if(!name.equals(asset.optString("name"))) continue;
                    String url="https://github.com/BLig-Vox/D4C/releases/download/"+tag+"/"+name;
                    if(!url.equalsIgnoreCase(asset.optString("browser_download_url"))) continue;
                    best=v; found=new JSONObject().put("tag",tag).put("url",asset.getString("browser_download_url"))
                        .put("size",asset.getLong("size")).put("digest",asset.optString("digest"));
                }
            }
            if(rows.length()<100) return found;
        }
    }

    private void check(boolean manual) {
        if(checking || downloading) { if(manual) tell("Update check or download in progress"); return; }
        checking=true;
        String current=installed();
        boolean alpha=prefs.getBoolean("alpha",current.contains("-alpha."));
        new Thread(()->{
            JSONObject release=null; String failure=null;
            try { release=find(alpha,current); prefs.edit().putLong("last",System.currentTimeMillis()).apply(); }
            catch(Exception e) { failure=e.getMessage(); }
            final JSONObject result=release; final String error=failure;
            activity.runOnUiThread(()->{
                checking=false;
                if(activity.isFinishing() || activity.isDestroyed() || !activity.hasWindowFocus() && !manual) return;
                if(error!=null) { if(manual) tell("Update check failed: "+error); return; }
                if(result==null) { if(manual) tell("No newer APK in this channel"); return; }
                new AlertDialog.Builder(activity).setTitle("D4C "+result.optString("tag"))
                    .setMessage("Installed version: "+current).setNegativeButton("Later",null)
                    .setPositiveButton("Download and install",(d,i)->download(result)).show();
            });
        },"d4c-update-check").start();
    }

    private void download(JSONObject release) {
        if(downloading) return;
        downloading=true; tell("Downloading D4C update…");
        new Thread(()->{
            File tmp=null;
            try {
                File dir=new File(activity.getCacheDir(),"updates"); dir.mkdirs();
                tmp=File.createTempFile("download-", ".tmp",dir);
                long expected=release.getLong("size");
                if(expected<=0 || expected>256L*1024*1024) throw new IOException("Invalid APK size");
                MessageDigest sha=MessageDigest.getInstance("SHA-256");
                HttpURLConnection c=open(release.getString("url"));
                try(InputStream in=c.getInputStream(); OutputStream out=new FileOutputStream(tmp)) {
                    byte[] buf=new byte[32768]; long total=0;
                    for(int n;(n=in.read(buf))!=-1;) {
                        total+=n; if(total>expected) throw new IOException("APK size mismatch");
                        sha.update(buf,0,n); out.write(buf,0,n);
                    }
                    if(total!=expected) throw new IOException("Incomplete download");
                } finally { c.disconnect(); }
                String digest=release.optString("digest");
                if(digest.startsWith("sha256:")) {
                    StringBuilder hex=new StringBuilder("sha256:");
                    for(byte b:sha.digest()) hex.append(String.format(java.util.Locale.ROOT,"%02x",b&255));
                    if(!hex.toString().equalsIgnoreCase(digest)) throw new IOException("APK checksum mismatch");
                }
                PackageManager pm=activity.getPackageManager();
                PackageInfo old=pm.getPackageInfo(activity.getPackageName(),PackageManager.GET_SIGNATURES);
                PackageInfo next=pm.getPackageArchiveInfo(tmp.getPath(),PackageManager.GET_SIGNATURES);
                if(next==null || !old.packageName.equals(next.packageName) || next.versionCode<=old.versionCode ||
                    !release.getString("tag").replaceFirst("^v","").equals(next.versionName) ||
                    old.signatures==null || old.signatures.length==0 || !Arrays.equals(old.signatures,next.signatures))
                    throw new IOException("APK identity or version mismatch");
                File ready=new File(dir,"update.apk");
                if(!tmp.renameTo(ready)) throw new IOException("Could not save update");
                activity.runOnUiThread(()->{downloading=false; if(!activity.isDestroyed()&&!activity.isFinishing()) install(ready);});
            } catch(Exception e) {activity.runOnUiThread(()->{downloading=false;tell("Update failed: "+e.getMessage());});}
            finally {if(tmp!=null) tmp.delete();}
        },"d4c-update-download").start();
    }

    private boolean canInstall() {
        return Build.VERSION.SDK_INT<26 || activity.getPackageManager().canRequestPackageInstalls();
    }
    private void install(File apk) {
        if(canInstall()) { launch(apk); return; }
        awaitingPermission=apk;
        try { activity.startActivity(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,Uri.parse("package:"+activity.getPackageName()))); }
        catch(RuntimeException e) { awaitingPermission=null; tell("Allow installs from D4C in Android settings"); }
    }
    private void launch(File apk) {
        Uri uri=Uri.parse("content://"+activity.getPackageName()+".updates/update.apk");
        Intent intent=new Intent(Intent.ACTION_VIEW).setDataAndType(uri,"application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try { activity.startActivity(intent); } catch(RuntimeException e) { tell("Android installer unavailable"); }
    }
}
