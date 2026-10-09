"""imagePicker AI worker."""

import os

# One OpenCV thread: work is already parallel per photo (pipeline decode pool, generation
# pools), and OpenCV 5.0's own parallel_for nested in those threads, or resized after first use,
# crashed the process on Windows with a fatal thread-pool exception (0xC000070A). Read by OpenCV
# when it loads, so it must be set before any module imports cv2.
os.environ.setdefault("OPENCV_FOR_THREADS_NUM", "1")

__version__ = "0.1.0"
PROTOCOL_VERSION = 1
