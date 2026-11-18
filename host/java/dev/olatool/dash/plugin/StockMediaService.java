package dev.olatool.dash.plugin;

import android.content.ComponentName;
import android.content.Context;
import android.media.AudioManager;
import android.media.MediaMetadata;
import android.media.session.MediaController;
import android.media.session.MediaSessionManager;
import android.media.session.PlaybackState;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.service.notification.NotificationListenerService;

import dev.olatool.dash.Trace;

import java.util.Collections;
import java.util.List;

/**
 * Built-in media source. Reads the active media session and sends it transport controls. Android
 * only exposes sessions to a granted notification listener, so the owner grants it once:
 *   adb shell cmd notification allow_listener dev.olatool.dash/dev.olatool.dash.plugin.StockMediaService
 * Runs on the main looper, so all state here is main-thread only.
 */
public final class StockMediaService extends NotificationListenerService {
    private static StockMediaService live;

    private final Handler main = new Handler(Looper.getMainLooper());
    private MediaSessionManager sessions;
    private MediaController current;
    private final Runnable progressTicker = new Runnable() {
        @Override public void run() {
            if (current == null) return;
            publish();
        }
    };

    private final MediaController.Callback callback = new MediaController.Callback() {
        @Override
        public void onPlaybackStateChanged(PlaybackState state) {
            publish();
        }

        @Override
        public void onMetadataChanged(MediaMetadata metadata) {
            publish();
        }

        @Override
        public void onSessionDestroyed() {
            pick(Collections.<MediaController>emptyList());
        }
    };

    private final MediaSessionManager.OnActiveSessionsChangedListener listener =
            this::pick;

    @Override
    public void onListenerConnected() {
        live = this;
        sessions = getSystemService(MediaSessionManager.class);
        try {
            sessions.addOnActiveSessionsChangedListener(listener, component());
            pick(sessions.getActiveSessions(component()));
        } catch (SecurityException e) {
            Trace.event("media: session access refused " + e.getMessage());
        }
    }

    @Override
    public void onListenerDisconnected() {
        live = null;
        main.removeCallbacks(progressTicker);
        if (sessions != null) {
            sessions.removeOnActiveSessionsChangedListener(listener);
        }
        pick(Collections.<MediaController>emptyList());
    }

    private ComponentName component() {
        return new ComponentName(this, StockMediaService.class);
    }

    /** Prefers a playing session, otherwise the first one. */
    private void pick(List<MediaController> list) {
        MediaController next = null;
        for (MediaController c : list) {
            if (next == null) {
                next = c;
            }
            if (isPlaying(c)) {
                next = c;
                break;
            }
        }
        if (next != current) {
            if (current != null) {
                current.unregisterCallback(callback);
            }
            current = next;
            if (current != null) {
                current.registerCallback(callback, main);
            }
        }
        publish();
    }

    private void publish() {
        main.removeCallbacks(progressTicker);
        if (current == null) {
            PluginHost.setStockMedia(null, null, false, 0L, 0L);
            return;
        }
        MediaMetadata md = current.getMetadata();
        String title = md == null ? null : md.getString(MediaMetadata.METADATA_KEY_TITLE);
        String artist = md == null ? null : md.getString(MediaMetadata.METADATA_KEY_ARTIST);
        long duration = md == null ? 0L : Math.max(0L, md.getLong(MediaMetadata.METADATA_KEY_DURATION));
        PlaybackState state = current.getPlaybackState();
        long position = 0L;
        if (state != null) {
            position = Math.max(0L, state.getPosition());
            if (state.getState() == PlaybackState.STATE_PLAYING && state.getLastPositionUpdateTime() > 0L) {
                long elapsed = Math.max(0L, SystemClock.elapsedRealtime() - state.getLastPositionUpdateTime());
                position += (long) (elapsed * state.getPlaybackSpeed());
            }
        }
        if (duration > 0L) position = Math.min(position, duration);
        boolean playing = isPlaying(current);
        PluginHost.setStockMedia(title, artist, playing, position, duration);
        if (playing) main.postDelayed(progressTicker, 500L);
    }

    private static boolean isPlaying(MediaController c) {
        PlaybackState s = c.getPlaybackState();
        return s != null && s.getState() == PlaybackState.STATE_PLAYING;
    }

    /** Adjusts Android's music stream volume in small system-controlled steps. */
    static void adjustVolume(Context context, String action) {
        AudioManager audio = (AudioManager) context.getSystemService(Context.AUDIO_SERVICE);
        if (audio == null) return;
        int direction = "media.volume-up".equals(action)
                ? AudioManager.ADJUST_RAISE : AudioManager.ADJUST_LOWER;
        audio.adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, AudioManager.FLAG_SHOW_UI);
    }

    /** Transport control for the active session. Called on the main thread by PluginHost. */
    static void control(String action) {
        StockMediaService s = live;
        if (s == null || s.current == null) {
            return;
        }
        MediaController.TransportControls t = s.current.getTransportControls();
        switch (action) {
            case "media.play-pause":
                if (isPlaying(s.current)) {
                    t.pause();
                } else {
                    t.play();
                }
                break;
            case "media.next":
                t.skipToNext();
                break;
            case "media.previous":
                t.skipToPrevious();
                break;
            default:
                break;
        }
    }
}
