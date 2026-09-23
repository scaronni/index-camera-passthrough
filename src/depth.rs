//! Depth of the scene, from stereo matching of the rectified camera images.

use anyhow::Result;
use opencv_stereo::{StereoMatcher, DISPARITY_SCALE};

use crate::{
    rectification::{Rectification, RECTIFIED_FOV},
    vrapi::StereoCamera,
    CAMERA_SIZE,
};

/// Size of the disparity map. Matching at this size takes a few milliseconds, and
/// measured depth is within a few percent up to about 2.5 m.
pub const DEPTH_SIZE: usize = 320;
/// Size the camera images are scaled to before they are rectified.
const SOURCE_SIZE: usize = 480;
/// Nearest distance that can be measured, in meters.
const MIN_DEPTH: f64 = 0.25;
/// The factory calibration can be off by a fraction of a pixel, which makes very far
/// points have slightly negative disparities.
const MIN_DISPARITY: i32 = -2;

/// Estimates the depth of the left rectified camera image.
pub struct DepthEstimator {
    matcher: StereoMatcher,
    /// Focal length of the rectified images times the baseline, in pixels * meters.
    focal_baseline: f64,
    disparity: Vec<i16>,
}

impl DepthEstimator {
    pub fn new(calib: &StereoCamera) -> Result<Self> {
        let rectification = Rectification::new(calib, RECTIFIED_FOV);
        let maps = [&calib.left.intrinsics, &calib.right.intrinsics]
            .into_iter()
            .enumerate()
            .map(|(eye, intrinsics)| {
                let mut map = Vec::with_capacity(DEPTH_SIZE * DEPTH_SIZE * 2);
                for y in 0..DEPTH_SIZE {
                    for x in 0..DEPTH_SIZE {
                        let pixel_center = |i: usize| (i as f64 + 0.5) / DEPTH_SIZE as f64;
                        let position = rectification.camera_position(
                            eye,
                            intrinsics,
                            [pixel_center(x), pixel_center(y)],
                        );
                        // OpenCV has pixel centers at integer coordinates.
                        map.extend(position.map(|p| (p * SOURCE_SIZE as f64 - 0.5) as f32));
                    }
                }
                map
            })
            .collect::<Vec<_>>();
        let focal = rectification.focal * DEPTH_SIZE as f64;
        let focal_baseline = focal * rectification.baseline();
        let max_disparity = focal_baseline / MIN_DEPTH;
        let num_disparities = ((max_disparity as i32 - MIN_DISPARITY) + 15) / 16 * 16;
        let matcher = StereoMatcher::new(
            CAMERA_SIZE as usize,
            SOURCE_SIZE,
            DEPTH_SIZE,
            &maps[0],
            &maps[1],
            MIN_DISPARITY,
            num_disparities,
        )?;
        Ok(Self {
            matcher,
            focal_baseline,
            disparity: vec![0; DEPTH_SIZE * DEPTH_SIZE],
        })
    }

    /// Compute the disparity map of a camera frame, the left and right grayscale
    /// images side by side.
    pub fn compute_gray(&mut self, frame: &[u8]) -> Result<&[i16]> {
        self.matcher.compute(frame, &mut self.disparity)?;
        Ok(&self.disparity)
    }

    /// Depth in meters of a point of the disparity map, along the axis of the
    /// rectified camera. Points without a match have no depth, points at or beyond
    /// infinity have an infinite depth.
    pub fn depth(&self, disparity: i16) -> Option<f32> {
        if (disparity as i32) < MIN_DISPARITY * DISPARITY_SCALE as i32 {
            return None;
        }
        let disparity = disparity as f32 / DISPARITY_SCALE;
        Some(if disparity > 0.0 {
            (self.focal_baseline / disparity as f64) as f32
        } else {
            f32::INFINITY
        })
    }
}
