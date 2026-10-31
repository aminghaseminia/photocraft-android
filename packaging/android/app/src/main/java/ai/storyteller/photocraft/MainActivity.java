package ai.storyteller.photocraft;

import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.view.View;
import android.view.WindowManager;

import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.google.androidgamesdk.GameActivity;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;

/**
 * The Android shell's Java half.
 *
 * <p>The Rust side calls the public {@code photocraft*} methods from the {@code android_main}
 * thread. They use the Storage Access Framework instead of paths: Android gives an app content
 * URIs, not filesystem paths. A picker is asynchronous, so its result is stored here and polled by
 * the Rust side once per frame ({@code Services::os_events}), which also keeps the JNI surface to
 * plain method calls.
 */
public class MainActivity extends GameActivity {
    static {
        // The cdylib built by cargo-ndk from apps/photocraft-android.
        System.loadLibrary("photocraft_android");
    }

    private static final int REQUEST_OPEN = 0x5043;
    private static final int REQUEST_SAVE = 0x5044;

    /** "" (none), "cancel", "open:<uri>\n<uri>…" or "save:<uri>". Written on the UI thread. */
    private volatile String pendingResult = "";

    /** Set by the back gesture; drained by photocraftTakeResult on the native thread. */
    private volatile boolean backPressed = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // A drawing session should not be interrupted by the screen timeout.
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        hideSystemUi();
        super.onCreate(savedInstanceState);
    }

    @Override
    protected void onResume() {
        super.onResume();
        hideSystemUi();
    }

    /**
     * An editor wants the whole screen. Insets are not wired yet (winit has no safe-area API), so
     * drawing behind the cutout at all is the less surprising of the two options.
     */
    private void hideSystemUi() {
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            WindowManager.LayoutParams attributes = getWindow().getAttributes();
            attributes.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(attributes);
        }
        View decor = getWindow().getDecorView();
        WindowInsetsControllerCompat controller = new WindowInsetsControllerCompat(getWindow(), decor);
        controller.hide(WindowInsetsCompat.Type.systemBars());
        controller.hide(WindowInsetsCompat.Type.displayCutout());
        controller.setSystemBarsBehavior(WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
    }

    /**
     * Back goes to the app, which asks about unsaved documents before closing. Without this,
     * GameActivity finishes the activity and Android may kill the process with the work unsaved.
     */
    @Override
    public void onBackPressed() {
        backPressed = true;
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (resultCode != RESULT_OK || data == null) {
            pendingResult = "cancel";
            return;
        }
        if (requestCode == REQUEST_OPEN) {
            ArrayList<String> uris = new ArrayList<>();
            ClipData clip = data.getClipData();
            if (clip != null) {
                for (int i = 0; i < clip.getItemCount(); i++) {
                    Uri uri = clip.getItemAt(i).getUri();
                    if (uri != null) {
                        uris.add(uri.toString());
                    }
                }
            } else if (data.getData() != null) {
                uris.add(data.getData().toString());
            }
            if (uris.isEmpty()) {
                pendingResult = "cancel";
                return;
            }
            for (String uri : uris) {
                try {
                    getContentResolver().takePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION);
                } catch (Exception ignored) {
                    // Some providers grant only for this session; the open still works.
                }
            }
            StringBuilder joined = new StringBuilder("open:");
            for (int i = 0; i < uris.size(); i++) {
                if (i > 0) {
                    joined.append('\n');
                }
                joined.append(uris.get(i));
            }
            pendingResult = joined.toString();
        } else if (requestCode == REQUEST_SAVE) {
            Uri uri = data.getData();
            pendingResult = uri == null ? "cancel" : "save:" + uri;
        }
    }

    // ---- Called from Rust over JNI. -------------------------------------------------------------

    public String photocraftFilesDir() {
        return getFilesDir().getAbsolutePath();
    }

    public void photocraftOpenDocuments(boolean multiple) {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType("*/*");
            intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION);
            startActivityForResult(intent, REQUEST_OPEN);
        });
    }

    public void photocraftCreateDocument(String name, String mime) {
        runOnUiThread(() -> {
            Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            intent.addCategory(Intent.CATEGORY_OPENABLE);
            intent.setType(mime);
            intent.putExtra(Intent.EXTRA_TITLE, name);
            intent.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
            startActivityForResult(intent, REQUEST_SAVE);
        });
    }

    /** One pending event per call; empty when there is nothing new. */
    public String photocraftTakeResult() {
        if (backPressed) {
            backPressed = false;
            return "back";
        }
        String result = pendingResult;
        if (result != null && !result.isEmpty()) {
            pendingResult = "";
            return result;
        }
        return "";
    }

    /** The provider's display name (with its extension), which the importer uses to pick a codec. */
    public String photocraftDocumentName(String uri) {
        try (Cursor cursor = getContentResolver().query(Uri.parse(uri), null, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (column >= 0) {
                    String name = cursor.getString(column);
                    if (name != null) {
                        return name;
                    }
                }
            }
        } catch (Exception ignored) {
            // Fall through to the caller's own name.
        }
        return "";
    }

    /**
     * Reads a whole document into memory. Large photos are copied once here and once into Rust;
     * moving this to a detached file descriptor is the next step if memory becomes a problem.
     */
    public byte[] photocraftReadAll(String uri) {
        try (InputStream in = getContentResolver().openInputStream(Uri.parse(uri));
             ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            if (in == null) {
                return null;
            }
            byte[] buffer = new byte[1 << 16];
            int read;
            while ((read = in.read(buffer)) > 0) {
                out.write(buffer, 0, read);
            }
            return out.toByteArray();
        } catch (Exception error) {
            return null;
        }
    }

    /** Writes a document back to the URI the create-document picker returned. */
    public void photocraftWriteAll(String uri, byte[] data) throws Exception {
        try (OutputStream out = getContentResolver().openOutputStream(Uri.parse(uri), "wt")) {
            if (out == null) {
                throw new java.io.IOException("no output stream for " + uri);
            }
            out.write(data);
            out.flush();
        }
    }

    public void photocraftOpenUrl(String url) {
        runOnUiThread(() -> {
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
            } catch (Exception ignored) {
                // No browser installed; links are not essential.
            }
        });
    }
}
