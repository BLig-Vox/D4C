package site.blig.d4c;
import android.app.*;
import android.content.*;
import android.os.*;
import java.util.concurrent.atomic.AtomicBoolean;

/** User-visible native connection; no WebView, external push provider or credential copy. */
public final class D4cNotificationService extends Service {
    static {System.loadLibrary("vox_d4c");}
    private volatile boolean running;
    private native void watchNotifications(String storage);
    public boolean isRunning(){return running;}
    public void postAlert(String id,String title,String body,String session,boolean sound){D4cNotifications.alert(this,id,title,body,session,sound);}
    @Override public void onCreate(){
        super.onCreate();running=true;D4cNotifications.channels(this);
        startForeground(1,D4cNotifications.build(this,"d4c-connection","D4C","Background replies enabled",null,true));
        new Thread(()->watchNotifications(getFilesDir().getAbsolutePath()),"D4C notifications").start();
    }
    @Override public int onStartCommand(Intent intent,int flags,int startId){return START_STICKY;}
    @Override public void onDestroy(){running=false;super.onDestroy();}
    @Override public IBinder onBind(Intent intent){return null;}
}
