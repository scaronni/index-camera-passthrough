Index camera passthrough
========================

**Warning: This is still a work in progress, you could get motion sickness if you try it now**

The problem that the Index camera doesn't work on Linux has been there for a long time, see [ValveSoftware/SteamVR-for-Linux#231](https://github.com/ValveSoftware/SteamVR-for-Linux/issues/231). And Valve doesn't seem to be willing to address it. So I decided to throw something together.

## Features

- Stereo overlay: the overlay in your game world that acts as a portal to real world. Meaning you see in 3D. A flat, non-3D view is also available, see [the example config file](index-camera-passthrough.toml).
- You can configure the overlay to be in one place, or stay in front of you.
- Use camera calibration data from your Steam installation.
- Show/hide passthrough with button presses

See also [the example config file](index-camera-passthrough.toml)

## Depth accuracy

The depth of the scene is estimated with stereo matching of the two cameras, as the first step towards the "3D" passthrough listed in the TODO; it is not used for the passthrough yet. It was checked against a tape measure on a Valve Index, measuring from the front of the headset to a flat printed box standing in front of it, at the 320x320 resolution used by the program:

| Distance (tape measure) | Estimated depth | Error |
|---|---|---|
| 0.45 m | 0.455 m | +0.5 cm (1.1%) |
| 0.80 m | 0.813 m | +1.3 cm (1.6%) |
| 1.36 m | 1.418 m | +5.8 cm (4.3%) |
| 2.58 m | 2.672 m | +9.2 cm (3.6%) |

The error grows with the distance because the farther a point is, the less its position differs between the two camera images: at 2.5 m the difference is only about 7 pixels at this resolution, so a fraction of a pixel is already a few centimeters. Most of the error is a constant offset of about 0.35 pixels, from a small inaccuracy of 0.14 degrees in the factory calibration of the cameras; corrected for it, the error is 3 mm RMS over the same range at full resolution. Plain surfaces, like white walls or cabinet doors, have no detail to match and get no depth, or a wrong one.

To check it on your headset, save a frame of the camera, for example with `ffmpeg -f v4l2 -input_format yuyv422 -video_size 1920x960 -i /dev/video0 -frames:v 1 frame.png`, then run `index-camera-passthrough --rectify frame.png --depth disparity.png`: the disparity map has the disparities in 1/16 pixels, the depth is 18.70 / disparity meters for a Valve Index.

## TODO

* Add option to make overlay follow controller.
* (Unrealistic) implement Valve's "3D" passthrough. To do this we essentially need to do 3D reconstruction from the stereo camera. There are existing methods, but will be really challenging to implement.

## Contribute

You can test this out and report your experience to help this improve.

If you have any suggestions about features, or how to make the passthrough look better, please let me know. I am not a graphics programmer and am trying my best to get things work, but solutions I came up with is definitely not going to be as good as things can be.

_Please_ help me out.

## Build instruction

Binaries for Fedora and Ubuntu are attached to the [releases](https://github.com/scaronni/index-camera-passthrough/releases).

To build this program, you need:

* Rust 1.89 or newer ([How to install](https://www.rust-lang.org/tools/install))
* clang, and the development packages of OpenVR, OpenCV, shaderc and udev:
  * Fedora: `clang-devel openvr-devel opencv-devel libshaderc-devel systemd-devel`
  * Debian / Ubuntu: `libclang-dev libopenvr-dev libopencv-dev libshaderc-dev libudev-dev`. Debian ships the shared shaderc library as `libshaderc.so`: point `SHADERC_LIB_DIR` to a directory with a `libshaderc_shared.so` link to it, as done in `.github/workflows/build.yaml`.

Then run, to build with both the OpenXR and the OpenVR backends:

```
cargo build --release --features openvr
```

## Usage

### Run from Steam library

After you have built the program, copy it to `/usr/local/bin`

```
cp ./target/release/index-camera-passthrough /usr/local/bin
```

And then add the `index-camera-passthrough.desktop` file to your Steam Library.

### Run directly

To run this program, you can either

```
cargo run
```

or run the binary directly

```
./target/release/index-camera-passthrough
```

## Configuration

On first run, the default configuration is written to `~/.config/index-camera-passthrough/index-camera-passthrough.toml` (`$XDG_CONFIG_HOME/index-camera-passthrough/` if set), unless it already exists. See [the example config file](index-camera-passthrough.toml), which is the same file, for all the options. The program has to be restarted after changing it.
