package site.blig.d4c;

import android.graphics.Color;
import android.graphics.SurfaceTexture;
import android.graphics.drawable.ColorDrawable;
import android.media.MediaPlayer;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.Surface;
import android.view.TextureView;
import android.widget.FrameLayout;
import android.widget.PopupWindow;

/** Owned by one Activity. Every call and MediaPlayer callback runs on its main thread. */
final class D4cVideo implements TextureView.SurfaceTextureListener {
    static native void report(long id, int position, int duration, boolean playing, String error);
    final long id;
    private final D4cActivity activity;
    private final FrameLayout frame;
    private final TextureView texture;
    private final PopupWindow window;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private MediaPlayer player;
    private Surface surface;
    private boolean ready, playWhenReady = true, ended, suspended;
    private int pendingSeek = -1;
    private int[] bounds;

    D4cVideo(D4cActivity activity, long id, String path) {
        this.activity = activity;
        this.id = id;
        frame = new FrameLayout(activity);
        frame.setBackgroundColor(Color.BLACK);
        texture = new TextureView(activity);
        texture.setSurfaceTextureListener(this);
        frame.addView(texture);
        window = new PopupWindow(frame, 1, 1, false);
        window.setTouchable(false);
        window.setClippingEnabled(false);
        window.setAttachedInDecor(false);
        window.setBackgroundDrawable(new ColorDrawable(Color.BLACK));
        try {
            player = new MediaPlayer();
            player.setOnVideoSizeChangedListener((mp, w, h) -> layout());
            player.setOnErrorListener((mp, what, extra) -> {
                fail("Android could not decode this video (" + what + "/" + extra + ")");
                return true;
            });
            player.setOnPreparedListener(mp -> {
                ready = true;
                if (pendingSeek >= 0) seek(pendingSeek);
                if (playWhenReady && !suspended) mp.start();
                layout();
                handler.post(tick);
            });
            player.setOnCompletionListener(mp -> {
                ended = true;
                playWhenReady = false;
                status();
            });
            player.setDataSource(path);
            player.prepareAsync();
        } catch (Exception e) { fail("Could not open video"); }
    }

    private final Runnable tick = new Runnable() {
        public void run() {
            if (ready && !suspended) {
                status();
                handler.postDelayed(this, 150);
            }
        }
    };
    private void status() {
        if (ready) report(id, player.getCurrentPosition(), player.getDuration(), player.isPlaying(), "");
    }
    void play(boolean value) {
        playWhenReady = value;
        if (!ready || suspended) return;
        if (value) {
            if (ended) seek(0);
            player.start();
        } else if (player.isPlaying()) player.pause();
        status();
    }
    void seek(int ms) {
        if (!ready) { pendingSeek = ms; return; }
        pendingSeek = -1;
        ended = false;
        if (android.os.Build.VERSION.SDK_INT >= 26)
            player.seekTo(ms, MediaPlayer.SEEK_CLOSEST);
        else player.seekTo(ms);
        status();
    }
    void place(int[] bounds) {
        this.bounds = bounds;
        layout();
    }
    private void layout() {
        if (bounds == null || player == null || suspended) return;
        int w = bounds[2] - bounds[0], h = bounds[3] - bounds[1];
        int clipW = bounds[6] - bounds[4], clipH = bounds[7] - bounds[5];
        if (w <= 0 || h <= 0 || clipW <= 0 || clipH <= 0) {window.dismiss();return;}
        int vw = ready ? player.getVideoWidth() : 0, vh = ready ? player.getVideoHeight() : 0;
        float scale = vw > 0 && vh > 0 ? Math.min((float)w/vw, (float)h/vh) : 1f;
        int width = vw > 0 ? Math.max(1, Math.round(vw*scale)) : w;
        int height = vh > 0 ? Math.max(1, Math.round(vh*scale)) : h;
        FrameLayout.LayoutParams p = new FrameLayout.LayoutParams(width, height, Gravity.TOP|Gravity.LEFT);
        p.leftMargin = bounds[0]-bounds[4]+(w-width)/2;
        p.topMargin = bounds[1]-bounds[5]+(h-height)/2;
        texture.setLayoutParams(p);
        if (!window.isShowing()) {
            window.setWidth(clipW);
            window.setHeight(clipH);
            window.showAtLocation(activity.getWindow().getDecorView(), Gravity.TOP|Gravity.LEFT, bounds[4], bounds[5]);
        } else window.update(bounds[4], bounds[5], clipW, clipH);
    }
    void suspend() {
        if (ready) play(false);
        playWhenReady = false;
        suspended = true;
        handler.removeCallbacks(tick);
        window.dismiss();
    }
    void resume() {
        suspended = false;
        layout();
        handler.removeCallbacks(tick);
        if (ready) handler.post(tick);
    }
    public void onSurfaceTextureAvailable(SurfaceTexture st, int w, int h) {
        if (player == null) return;
        surface = new Surface(st);
        player.setSurface(surface);
    }
    public boolean onSurfaceTextureDestroyed(SurfaceTexture st) {
        if (player != null) player.setSurface(null);
        if (surface != null) { surface.release(); surface = null; }
        return true;
    }
    public void onSurfaceTextureSizeChanged(SurfaceTexture st, int w, int h) {}
    public void onSurfaceTextureUpdated(SurfaceTexture st) {}
    private void fail(String message) {
        close();
        report(id, 0, 0, false, message);
    }
    void close() {
        ready = false;
        handler.removeCallbacks(tick);
        if (player != null) {player.release(); player = null;}
        window.dismiss();
    }
}
