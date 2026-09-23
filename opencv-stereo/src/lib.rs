//! Stereo matching of the Index camera frames with OpenCV.

#[cxx::bridge(namespace = "icp")]
mod ffi {
    unsafe extern "C++" {
        include!("opencv-stereo/include/stereo.h");

        type StereoMatcher;

        fn new_stereo_matcher(
            camera_size: i32,
            source_size: i32,
            size: i32,
            left_map: &[f32],
            right_map: &[f32],
            min_disparity: i32,
            num_disparities: i32,
        ) -> Result<UniquePtr<StereoMatcher>>;

        fn compute(
            self: Pin<&mut StereoMatcher>,
            frame: &[u8],
            disparity: &mut [i16],
        ) -> Result<()>;
    }
}

/// Fixed point scale of the disparities computed by [`StereoMatcher`].
pub const DISPARITY_SCALE: f32 = 16.0;

/// Rectifies the left and right camera images and computes the disparity of the left
/// image with semi-global block matching.
pub struct StereoMatcher {
    inner: cxx::UniquePtr<ffi::StereoMatcher>,
    camera_size: usize,
    size: usize,
}

// The matcher only owns OpenCV matrices, which can be moved to another thread.
unsafe impl Send for StereoMatcher {}

impl StereoMatcher {
    /// # Arguments
    ///
    /// - camera_size: size of each camera image in the input frames
    /// - source_size: size each camera image is scaled to before rectification
    /// - size: size of the rectified images and of the disparity map
    /// - left_map, right_map: for each pixel of the rectified images, row by row, the
    ///   x and y coordinates of the pixel to sample in the scaled camera image
    /// - min_disparity, num_disparities: disparity search range, in pixels, the number
    ///   must be a multiple of 16
    pub fn new(
        camera_size: usize,
        source_size: usize,
        size: usize,
        left_map: &[f32],
        right_map: &[f32],
        min_disparity: i32,
        num_disparities: i32,
    ) -> Result<Self, cxx::Exception> {
        let inner = ffi::new_stereo_matcher(
            camera_size as i32,
            source_size as i32,
            size as i32,
            left_map,
            right_map,
            min_disparity,
            num_disparities,
        )?;
        Ok(Self {
            inner,
            camera_size,
            size,
        })
    }

    /// Size of the disparity map.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Size of each camera image in the input frames.
    pub fn camera_size(&self) -> usize {
        self.camera_size
    }

    /// Compute the disparity map of `frame`, the left and right grayscale camera
    /// images side by side. Disparities are in pixels times [`DISPARITY_SCALE`], pixels
    /// without a match are below `min_disparity`.
    pub fn compute(&mut self, frame: &[u8], disparity: &mut [i16]) -> Result<(), cxx::Exception> {
        self.inner.pin_mut().compute(frame, disparity)
    }
}
