# openvr-sys

Raw bindings to the parts of [OpenVR](https://github.com/ValveSoftware/openvr) used by index-camera-passthrough, generated with [autocxx](https://github.com/google/autocxx).

The binding code comes from [openvr-sys2](https://crates.io/crates/openvr-sys2) 0.1.3 by Yuxuan Shui, released under the MIT license like the rest of this repository (see `LICENSE` at the top level). It is kept here so that it can follow newer OpenVR SDK releases.

## Building

The bindings link the OpenVR client library installed on the system, found through `pkg-config`, so the OpenVR development package is required:

- Fedora: `openvr-devel`
- Debian / Ubuntu: `libopenvr-dev`

## OpenVR SDK

The bindings are generated from `openvr/headers/openvr.h`, vendored from the OpenVR SDK. The SDK is released under the BSD 3-Clause license, see `openvr/LICENSE`.

Current version: **2.15.6**.

The header can be newer than the client library installed on the system: the library only provides the loader entry points (`VR_InitInternal2`, `VR_GetGenericInterface` and so on), while the interfaces are requested from the runtime by the version strings defined in the header.

`TryFrom<u32> for EVREventType` is generated from the header by `build.rs`, so it always matches the SDK.

To update the SDK, copy the header and the license from the new release:

```
git clone --depth 1 --branch vX.Y.Z https://github.com/ValveSoftware/openvr /tmp/openvr
cp /tmp/openvr/LICENSE openvr/
cp /tmp/openvr/headers/openvr.h openvr/headers/
```

Then check that the hand-written `VREvent_Data_t` union in `src/lib.rs` still has the same members as the one in `openvr.h`.
