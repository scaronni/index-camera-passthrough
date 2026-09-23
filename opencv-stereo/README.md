# opencv-stereo

Stereo matching of the Index camera frames for index-camera-passthrough: rectification with precomputed maps, contrast equalization and semi-global block matching with [OpenCV](https://opencv.org/), through a small C++ class exposed with [cxx](https://cxx.rs/).

It links the OpenCV library installed on the system, found through `pkg-config`, so the OpenCV development package is required:

- Fedora: `opencv-devel`
- Debian / Ubuntu: `libopencv-dev`
