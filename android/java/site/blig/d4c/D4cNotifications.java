package site.blig.d4c;
import android.app.*;
import android.content.*;
import android.os.Build;

final class D4cNotifications {
    static void channels(Context context) {
        if(Build.VERSION.SDK_INT>=26){
            NotificationManager manager=context.getSystemService(NotificationManager.class);
            manager.createNotificationChannel(new NotificationChannel("d4c-connection","D4C connection",NotificationManager.IMPORTANCE_LOW));
            manager.createNotificationChannel(new NotificationChannel("d4c-replies","Replies and tasks",NotificationManager.IMPORTANCE_HIGH));
            NotificationChannel silent=new NotificationChannel("d4c-silent","Replies without sound",NotificationManager.IMPORTANCE_DEFAULT);silent.setSound(null,null);manager.createNotificationChannel(silent);
        }
    }
    static Notification build(Context context,String channel,String title,String body,String session,boolean ongoing) {
        Intent intent=new Intent(context,D4cActivity.class).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP|Intent.FLAG_ACTIVITY_CLEAR_TOP);
        if(session!=null&&!session.isEmpty())intent.putExtra("d4c.session",session);
        PendingIntent pending=PendingIntent.getActivity(context,session==null?0:session.hashCode(),intent,PendingIntent.FLAG_UPDATE_CURRENT|PendingIntent.FLAG_IMMUTABLE);
        Notification.Builder builder=Build.VERSION.SDK_INT>=26?new Notification.Builder(context,channel):new Notification.Builder(context);
        builder.setSmallIcon(android.R.drawable.ic_dialog_email).setContentTitle(title).setContentText(body).setContentIntent(pending).setAutoCancel(!ongoing).setOngoing(ongoing).setVisibility(Notification.VISIBILITY_PRIVATE).setCategory(ongoing?Notification.CATEGORY_SERVICE:Notification.CATEGORY_MESSAGE);
        if(!ongoing){builder.setStyle(new Notification.BigTextStyle().bigText(body));if(Build.VERSION.SDK_INT<26&&channel.equals("d4c-replies"))builder.setDefaults(Notification.DEFAULT_SOUND);}
        return builder.build();
    }
    static void alert(Context context,String id,String title,String body,String session,boolean sound){
        channels(context);
        NotificationManager manager=context.getSystemService(NotificationManager.class);
        if(Build.VERSION.SDK_INT>=24&&!manager.areNotificationsEnabled())return;
        manager.notify(id,2,build(context,sound?"d4c-replies":"d4c-silent",title,body,session,false));
    }
}
