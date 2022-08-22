package zoxc.crusader;

import androidx.appcompat.app.AppCompatActivity;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

import com.google.androidgamesdk.GameActivity;

import android.os.Bundle;
import android.content.pm.PackageManager;
import android.os.Build.VERSION;
import android.os.Build.VERSION_CODES;
import android.os.Bundle;
import android.view.View;
import android.view.WindowManager;
import android.util.Log;
import android.content.Intent;
import android.net.Uri;
import android.app.Activity;
import android.view.inputmethod.InputMethodManager;
import android.provider.OpenableColumns;
import android.database.Cursor;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

public class MainActivity extends GameActivity {
    static {
        System.loadLibrary("main");
    }

    public void showKeyboard(boolean show) {
        InputMethodManager input = (InputMethodManager)getSystemService("input_method");
        if (show) {
            input.showSoftInput(getWindow().getDecorView().getRootView(), 0);
        } else {
            input.hideSoftInputFromWindow(getWindow().getDecorView().getRootView().getWindowToken(), 0);
        }
    }

    public void loadFile() {
        Intent intent = new Intent(Intent.ACTION_GET_CONTENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        startActivityForResult(intent, ACTIVITY_LOAD_FILE);
    }

    public void saveFile(boolean image, String name, byte[] data) {
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        if (image) {
            intent.setType("image/png");
        } else {
            intent.setType("application/octet-stream");
        }
        intent.putExtra(Intent.EXTRA_TITLE, name);
        saveFileData = data;
        saveImage = image;
        startActivityForResult(intent, ACTIVITY_CREATE_FILE);
    }

    private byte[] saveFileData = null;
    private boolean saveImage;

    private static final int ACTIVITY_LOAD_FILE = 1;
    private static final int ACTIVITY_CREATE_FILE = 2;

    static native void fileLoaded(String name, byte[] data); 

    static native void fileSaved(boolean image, String name); 

    @Override
    public void onActivityResult(int requestCode, int resultCode,
            Intent resultData) {
        if (requestCode == ACTIVITY_CREATE_FILE) {
            if (resultCode == Activity.RESULT_OK && resultData != null) {
                Uri uri = resultData.getData();
                String name = getName(uri);
                try {
                    OutputStream stream = getContentResolver().openOutputStream(uri);
                    stream.write(saveFileData);
                    stream.close();
                    fileSaved(saveImage, name);
                }
                catch(Exception e) {}
            }
            saveFileData = null;
        }

        if (requestCode == ACTIVITY_LOAD_FILE
        && resultCode == Activity.RESULT_OK
        && resultData != null) {
            Uri uri = resultData.getData();
            String name = getName(uri);

            try {
                InputStream stream = getContentResolver().openInputStream(uri);
                ByteArrayOutputStream buffer = new ByteArrayOutputStream();
                int read;
                byte[] byte_buffer = new byte[0x1000];
                while ((read = stream.read(byte_buffer, 0, byte_buffer.length)) != -1) {
                    buffer.write(byte_buffer, 0, read);
                }
                stream.close();
                byte[] data = buffer.toByteArray();
                fileLoaded(name, data);
            }
            catch(Exception e) {}
        }
    }

    public String getName(Uri uri) {
        Cursor cursor = getContentResolver().query(uri, null, null, null, null, null);
        String name = "";
        try {
            if (cursor != null && cursor.moveToFirst()) {
                name = cursor.getString(
                    cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                );
                return name;
            }
        } finally {
            cursor.close();
        }
        return name;
    }
}