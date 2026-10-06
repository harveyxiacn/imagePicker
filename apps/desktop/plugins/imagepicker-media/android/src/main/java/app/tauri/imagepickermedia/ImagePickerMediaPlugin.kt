package app.tauri.imagepickermedia

import android.Manifest
import android.app.Activity
import android.content.ContentUris
import android.content.ContentValues
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.util.Log
import android.view.WindowManager
import android.webkit.MimeTypeMap
import androidx.activity.result.ActivityResult
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class PathsArgs {
    var paths: Array<String> = emptyArray()
}

@InvokeArg
class ForegroundArgs {
    var title: String? = null
    var text: String? = null
    var progress: Int? = null
}

@InvokeArg
class KeepAwakeArgs {
    var on: Boolean = false
}

/**
 * Contract: docs/api-contract-m8.md section A.
 *
 * Aliases: the Android permission set differs per API level, so [requestPermission] picks one:
 *  - API 34+: READ_MEDIA_IMAGES + READ_MEDIA_VISUAL_USER_SELECTED (+ notifications) -> "Selected photos" possible
 *  - API 33:  READ_MEDIA_IMAGES (+ notifications)
 *  - <= 32:   READ_EXTERNAL_STORAGE
 */
@TauriPlugin(
    permissions = [
        Permission(
            strings = [
                Manifest.permission.READ_MEDIA_IMAGES,
                "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
                "android.permission.POST_NOTIFICATIONS",
            ],
            alias = "media34",
        ),
        Permission(
            strings = [Manifest.permission.READ_MEDIA_IMAGES, "android.permission.POST_NOTIFICATIONS"],
            alias = "media33",
        ),
        Permission(strings = [Manifest.permission.READ_EXTERNAL_STORAGE], alias = "media"),
    ],
)
class ImagePickerMediaPlugin(private val activity: Activity) : Plugin(activity) {
    private var pendingTrashCount = 0

    // ---- permissions -------------------------------------------------------------------------

    private fun has(permission: String) =
        ContextCompat.checkSelfPermission(activity, permission) == PackageManager.PERMISSION_GRANTED

    private fun fullAccess(): Boolean =
        if (Build.VERSION.SDK_INT >= 33) has(Manifest.permission.READ_MEDIA_IMAGES)
        else has(Manifest.permission.READ_EXTERNAL_STORAGE)

    private fun partialAccess(): Boolean =
        Build.VERSION.SDK_INT >= 34 && !fullAccess() && has("android.permission.READ_MEDIA_VISUAL_USER_SELECTED")

    private fun permissionState(): JSObject {
        val partial = partialAccess()
        return JSObject().put("granted", fullAccess() || partial).put("partial", partial)
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        if (fullAccess()) {
            invoke.resolve(permissionState())
            return
        }
        val alias = when {
            Build.VERSION.SDK_INT >= 34 -> "media34"
            Build.VERSION.SDK_INT >= 33 -> "media33"
            else -> "media"
        }
        requestPermissionForAlias(alias, invoke, "permissionResult")
    }

    @PermissionCallback
    fun permissionResult(invoke: Invoke) {
        invoke.resolve(permissionState())
    }

    // ---- albums ------------------------------------------------------------------------------

    @Command
    fun listAlbums(invoke: Invoke) {
        if (!fullAccess() && !partialAccess()) {
            invoke.reject("media permission not granted", "permission_denied")
            return
        }
        try {
            val rows = ArrayList<MediaRow>()
            val projection = arrayOf(
                MediaStore.Images.Media.BUCKET_ID,
                MediaStore.Images.Media.BUCKET_DISPLAY_NAME,
                MediaStore.Images.Media.DATA,
                MediaStore.Images.Media.DATE_TAKEN,
                MediaStore.Images.Media.DATE_MODIFIED,
            )
            activity.contentResolver.query(
                MediaStore.Images.Media.EXTERNAL_CONTENT_URI, projection, null, null, null,
            )?.use { c ->
                val iBucket = c.getColumnIndexOrThrow(MediaStore.Images.Media.BUCKET_ID)
                val iName = c.getColumnIndexOrThrow(MediaStore.Images.Media.BUCKET_DISPLAY_NAME)
                val iData = c.getColumnIndexOrThrow(MediaStore.Images.Media.DATA)
                val iTaken = c.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_TAKEN)
                val iMod = c.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_MODIFIED)
                while (c.moveToNext()) {
                    val path = c.getString(iData) ?: continue
                    val taken = c.getLong(iTaken)
                    val ms = if (taken > 0) taken else c.getLong(iMod) * 1000
                    rows.add(MediaRow(c.getString(iBucket) ?: "", c.getString(iName), path, ms))
                }
            }
            val arr = JSArray()
            for (a in AlbumAggregator.aggregate(rows)) {
                arr.put(
                    JSObject()
                        .put("id", a.id)
                        .put("name", a.name)
                        .put("path", a.path)
                        .put("count", a.count)
                        .put("cover_path", a.coverPath)
                        .put("latest_ms", a.latestMs),
                )
            }
            invoke.resolve(JSObject().put("albums", arr))
        } catch (e: Exception) {
            Log.e(TAG, "listAlbums failed", e)
            invoke.reject(e.message ?: "listAlbums failed")
        }
    }

    // ---- path <-> MediaStore ------------------------------------------------------------------

    private fun normalize(p: String): String = when {
        p.startsWith("/sdcard/") -> "/storage/emulated/0/" + p.removePrefix("/sdcard/")
        p.startsWith("/storage/self/primary/") -> "/storage/emulated/0/" + p.removePrefix("/storage/self/primary/")
        else -> p
    }

    /** MediaStore content URIs of the given file paths (unknown paths are skipped). */
    private fun urisForPaths(paths: Array<String>): List<Uri> {
        val out = ArrayList<Uri>()
        val wanted = paths.map { normalize(it) }.distinct()
        for (chunk in wanted.chunked(400)) {
            val sel = MediaStore.Images.Media.DATA + " IN (" + chunk.joinToString(",") { "?" } + ")"
            activity.contentResolver.query(
                MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                arrayOf(MediaStore.Images.Media._ID),
                sel,
                chunk.toTypedArray(),
                null,
            )?.use { c ->
                while (c.moveToNext()) {
                    out.add(ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, c.getLong(0)))
                }
            }
        }
        return out
    }

    // ---- share -------------------------------------------------------------------------------

    @Command
    fun share(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(PathsArgs::class.java)
            val uris = urisForPaths(args.paths)
            if (uris.isEmpty()) {
                invoke.reject("no shareable photos among the given paths")
                return
            }
            val intent = if (uris.size == 1) {
                Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
            } else {
                Intent(Intent.ACTION_SEND_MULTIPLE)
                    .putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
            }
            intent.type = "image/*"
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            val chooser = Intent.createChooser(intent, null)
            activity.runOnUiThread { activity.startActivity(chooser) }
            invoke.resolve(JSObject().put("shared", uris.size))
        } catch (e: Exception) {
            Log.e(TAG, "share failed", e)
            invoke.reject(e.message ?: "share failed")
        }
    }

    // ---- trash -------------------------------------------------------------------------------

    @Command
    fun trash(invoke: Invoke) {
        try {
            if (Build.VERSION.SDK_INT < 30) {
                invoke.reject("moving photos to the trash needs Android 11 or newer", "unsupported")
                return
            }
            val args = invoke.parseArgs(PathsArgs::class.java)
            val uris = urisForPaths(args.paths)
            if (uris.isEmpty()) {
                invoke.resolve(JSObject().put("trashed", 0))
                return
            }
            pendingTrashCount = uris.size
            val pi = MediaStore.createTrashRequest(activity.contentResolver, uris, true)
            val req = IntentSenderRequest.Builder(pi.intentSender).build()
            // The same call the AndroidX contract makes; the activity result registry routes
            // ACTION_INTENT_SENDER_REQUEST intents to startIntentSenderForResult.
            val intent = ActivityResultContracts.StartIntentSenderForResult().createIntent(activity, req)
            startActivityForResult(invoke, intent, "trashResult")
        } catch (e: Exception) {
            Log.e(TAG, "trash failed", e)
            invoke.reject(e.message ?: "trash failed")
        }
    }

    @ActivityCallback
    fun trashResult(invoke: Invoke, result: ActivityResult) {
        val n = if (result.resultCode == Activity.RESULT_OK) pendingTrashCount else 0
        pendingTrashCount = 0
        invoke.resolve(JSObject().put("trashed", n))
    }

    // ---- foreground service ------------------------------------------------------------------

    @Command
    fun startForeground(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(ForegroundArgs::class.java)
            val intent = Intent(activity, AnalysisService::class.java)
                .putExtra(AnalysisService.EXTRA_TITLE, args.title ?: "")
                .putExtra(AnalysisService.EXTRA_TEXT, args.text ?: "")
                .putExtra(AnalysisService.EXTRA_PROGRESS, args.progress ?: -1)
            ContextCompat.startForegroundService(activity, intent)
            invoke.resolve()
        } catch (e: Exception) {
            Log.e(TAG, "startForeground failed", e)
            invoke.reject(e.message ?: "startForeground failed")
        }
    }

    @Command
    fun stopForeground(invoke: Invoke) {
        try {
            activity.startService(
                Intent(activity, AnalysisService::class.java).setAction(AnalysisService.ACTION_STOP),
            )
        } catch (e: Exception) {
            // Service not running (or app in background): stopping is best effort.
            Log.w(TAG, "stopForeground: ${e.message}")
            activity.stopService(Intent(activity, AnalysisService::class.java))
        }
        invoke.resolve()
    }

    // ---- keep awake --------------------------------------------------------------------------

    @Command
    fun keepAwake(invoke: Invoke) {
        val args = invoke.parseArgs(KeepAwakeArgs::class.java)
        activity.runOnUiThread {
            if (args.on) activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            else activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
        invoke.resolve()
    }

    // ---- publish exports ---------------------------------------------------------------------

    @Command
    fun publishExports(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(PathsArgs::class.java)
            val uris = JSArray()
            var published = 0
            for (p in args.paths) {
                val f = File(p)
                if (!f.isFile) continue
                val mime = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(f.extension.lowercase()) ?: "image/jpeg"
                val values = ContentValues().apply {
                    put(MediaStore.Images.Media.DISPLAY_NAME, f.name)
                    put(MediaStore.Images.Media.MIME_TYPE, mime)
                    if (Build.VERSION.SDK_INT >= 29) {
                        put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/imagePicker")
                        put(MediaStore.Images.Media.IS_PENDING, 1)
                    }
                }
                val resolver = activity.contentResolver
                val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: continue
                try {
                    resolver.openOutputStream(uri)?.use { out -> f.inputStream().use { it.copyTo(out) } }
                        ?: throw java.io.IOException("no output stream")
                    if (Build.VERSION.SDK_INT >= 29) {
                        resolver.update(
                            uri, ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) }, null, null,
                        )
                    }
                    uris.put(uri.toString())
                    published++
                } catch (e: Exception) {
                    Log.e(TAG, "publish ${f.name} failed", e)
                    resolver.delete(uri, null, null)
                }
            }
            invoke.resolve(JSObject().put("published", published).put("uris", uris))
        } catch (e: Exception) {
            Log.e(TAG, "publishExports failed", e)
            invoke.reject(e.message ?: "publishExports failed")
        }
    }

    companion object {
        private const val TAG = "imagepicker-media"
    }
}
