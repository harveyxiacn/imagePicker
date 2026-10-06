package app.tauri.imagepickermedia

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AlbumAggregatorTest {
    private fun row(bucket: String, name: String?, path: String, ms: Long) = MediaRow(bucket, name, path, ms)

    @Test
    fun groupsByBucketAndPicksNewestAsCover() {
        val albums = AlbumAggregator.aggregate(
            listOf(
                row("1", "Camera", "/storage/emulated/0/DCIM/Camera/a.jpg", 100),
                row("2", "Screenshots", "/storage/emulated/0/Pictures/Screenshots/s.png", 500),
                row("1", "Camera", "/storage/emulated/0/DCIM/Camera/b.jpg", 300),
                row("1", "Camera", "/storage/emulated/0/DCIM/Camera/c.jpg", 200),
            ),
        )
        assertEquals(2, albums.size)
        // newest album first
        assertEquals("Screenshots", albums[0].name)
        val cam = albums[1]
        assertEquals(3, cam.count)
        assertEquals("/storage/emulated/0/DCIM/Camera", cam.path)
        assertEquals("/storage/emulated/0/DCIM/Camera/b.jpg", cam.coverPath)
        assertEquals(300L, cam.latestMs)
    }

    @Test
    fun fallsBackToDirectoryNameWhenBucketNameMissing() {
        val albums = AlbumAggregator.aggregate(listOf(row("9", null, "/sdcard/Foo/x.jpg", 1)))
        assertEquals("Foo", albums.single().name)
    }

    @Test
    fun emptyInputGivesNoAlbums() {
        assertTrue(AlbumAggregator.aggregate(emptyList()).isEmpty())
    }
}
