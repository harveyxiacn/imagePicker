package app.tauri.imagepickermedia

/** One MediaStore image row (only what the album list needs). */
data class MediaRow(val bucketId: String, val bucketName: String?, val path: String, val dateMs: Long)

data class Album(
    val id: String,
    val name: String,
    val path: String,
    val count: Int,
    val coverPath: String,
    val latestMs: Long,
)

/** Pure aggregation of MediaStore rows into albums (one per bucket); no Android dependencies. */
object AlbumAggregator {
    fun parentDir(path: String): String {
        val i = path.lastIndexOf('/')
        return if (i <= 0) "/" else path.substring(0, i)
    }

    /** Albums sorted by newest photo first. The newest row of a bucket is its cover. */
    fun aggregate(rows: Iterable<MediaRow>): List<Album> {
        class Acc(val id: String, var name: String, var cover: MediaRow, var count: Int)

        val byBucket = LinkedHashMap<String, Acc>()
        for (r in rows) {
            val acc = byBucket[r.bucketId]
            if (acc == null) {
                val name = r.bucketName?.takeIf { it.isNotBlank() } ?: parentDir(r.path).substringAfterLast('/')
                byBucket[r.bucketId] = Acc(r.bucketId, name, r, 1)
            } else {
                acc.count++
                if (r.dateMs > acc.cover.dateMs) acc.cover = r
            }
        }
        return byBucket.values
            .map { Album(it.id, it.name, parentDir(it.cover.path), it.count, it.cover.path, it.cover.dateMs) }
            .sortedWith(compareByDescending<Album> { it.latestMs }.thenBy { it.name })
    }
}
