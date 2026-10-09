import os

import imagepicker_ai  # noqa: F401


def test_opencv_is_told_to_run_single_threaded():
    # Nested / resized OpenCV thread pools crashed the worker on Windows (0xC000070A). The cap
    # is the environment variable OpenCV reads when it loads (`cv2.getNumThreads()` keeps
    # reporting the core count), so importing the package must have set it.
    assert os.environ["OPENCV_FOR_THREADS_NUM"] == "1"
