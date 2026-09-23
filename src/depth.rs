//! Depth of the scene, from stereo matching of the rectified camera images.

use anyhow::Result;
use opencv_stereo::{StereoMatcher, DISPARITY_SCALE};

use crate::{rectification::Rectification, vrapi::StereoCamera, CAMERA_SIZE};

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

/// Size of the center of the view whose depth is measured, as a fraction of the size
/// of the rectified images.
const CENTER: f32 = 0.3;
/// Time constant of the smoothing of the depth of the center of the view, in seconds.
const SMOOTHING: f32 = 0.15;
/// The center of the view is never placed farther than this, in meters.
const MAX_CENTER_DEPTH: f32 = 10.0;
/// Threads used by the stereo matching. More barely make it faster at this size, and
/// would compete with the VR runtime.
const MATCHING_THREADS: usize = 2;

/// Estimates the depth of the left rectified camera image.
pub struct DepthEstimator {
    matcher: StereoMatcher,
    /// Focal length of the rectified images times the baseline, in pixels * meters.
    focal_baseline: f64,
    gray: Vec<u8>,
    disparity: Vec<i16>,
}

impl DepthEstimator {
    pub fn new(calib: &StereoCamera) -> Result<Self> {
        let rectification = Rectification::new(calib);
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
            gray: vec![0; CAMERA_SIZE as usize * CAMERA_SIZE as usize * 2],
            disparity: vec![0; DEPTH_SIZE * DEPTH_SIZE],
        })
    }

    /// Compute the disparity map of a camera frame in the YUYV format of the camera.
    pub fn compute_yuyv(&mut self, frame: &[u8]) -> Result<&[i16]> {
        // The luma of every pixel is every other byte.
        for (gray, yuyv) in self.gray.iter_mut().zip(frame.iter().step_by(2)) {
            *gray = *yuyv;
        }
        self.matcher.compute(&self.gray, &mut self.disparity)?;
        Ok(&self.disparity)
    }

    /// Depth in meters, along the axis of the rectified cameras, of what is in the
    /// center of the view in the last computed disparity map. Nearer points count more,
    /// so that an object in front of a background, like a hand, wins even if it covers
    /// less than half of the center. `None` if too few points have a match.
    pub fn center_depth(&self) -> Option<f32> {
        let size = (DEPTH_SIZE as f32 * CENTER) as usize;
        let start = (DEPTH_SIZE - size) / 2;
        let min_disparity = (MIN_DISPARITY * DISPARITY_SCALE as i32) as i16;
        let mut disparities: Vec<i16> = (start..start + size)
            .flat_map(|y| &self.disparity[y * DEPTH_SIZE + start..y * DEPTH_SIZE + start + size])
            .copied()
            .filter(|&d| d >= min_disparity)
            .collect();
        if disparities.len() * 10 < size * size {
            return None;
        }
        let index = disparities.len() * 7 / 10;
        let disparity = *disparities.select_nth_unstable(index).1;
        self.depth(disparity)
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

/// A measure of the depth of the center of the view, and when its frame was captured.
type Measure = (Option<f32>, std::time::Instant);

/// Measures the depth of the center of the view in a background thread, so that the
/// render loop does not wait for the stereo matching.
pub struct CenterDepthWorker {
    frames: std::sync::mpsc::SyncSender<(Vec<u8>, std::time::Instant)>,
    measures: std::sync::mpsc::Receiver<Measure>,
    /// Whether the worker is waiting for a frame, to skip copying frames it cannot take.
    idle: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CenterDepthWorker {
    pub fn new(mut estimator: DepthEstimator) -> Self {
        // A frame is only queued if the worker is free, so it always measures a recent
        // one.
        let (frames, frame_receiver) =
            std::sync::mpsc::sync_channel::<(Vec<u8>, std::time::Instant)>(0);
        let (measure_sender, measures) = std::sync::mpsc::channel();
        let idle = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_idle = idle.clone();
        std::thread::spawn(move || {
            opencv_stereo::set_num_threads(MATCHING_THREADS);
            loop {
                worker_idle.store(true, std::sync::atomic::Ordering::Relaxed);
                // Stops when the worker is dropped.
                let Ok((frame, time)) = frame_receiver.recv() else {
                    break;
                };
                worker_idle.store(false, std::sync::atomic::Ordering::Relaxed);
                let start = std::time::Instant::now();
                let depth = match estimator.compute_yuyv(&frame) {
                    Ok(_) => estimator.center_depth(),
                    Err(e) => {
                        log::warn!("Cannot measure the depth: {e}");
                        None
                    }
                };
                log::trace!("center depth {depth:?} in {:?}", start.elapsed());
                if measure_sender.send((depth, time)).is_err() {
                    break;
                }
            }
        });
        Self {
            frames,
            measures,
            idle,
        }
    }

    /// Measure a camera frame in the YUYV format of the camera, captured at `time`,
    /// unless the worker is still busy with the previous one.
    pub fn submit(&self, frame: &[u8], time: std::time::Instant) {
        if self.idle.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self.frames.try_send((frame.to_vec(), time));
        }
    }

    /// Measures completed since the last call.
    pub fn measures(&self) -> impl Iterator<Item = Measure> + '_ {
        self.measures.try_iter()
    }
}

/// Follows the depth of the center of the view over time, smoothing out the noise of
/// single frames and short changes, like something passing quickly in front.
#[derive(Debug, Default)]
pub struct DepthSmoothing {
    /// Smoothed inverse depth, in 1/m.
    inverse_depth: Option<f32>,
    last_update: Option<std::time::Instant>,
}

impl DepthSmoothing {
    /// Add the depth measured at `time`, and return the smoothed depth. Frames without
    /// a measure keep the previous depth.
    pub fn update(&mut self, depth: Option<f32>, time: std::time::Instant) -> Option<f32> {
        if let Some(depth) = depth {
            // Inverse depth, which is what the disparity measures linearly, and is finite
            // for points at infinity.
            let inverse_depth = 1.0 / depth.clamp(MIN_DEPTH as f32, MAX_CENTER_DEPTH);
            self.inverse_depth = Some(match (self.inverse_depth, self.last_update) {
                (Some(previous), Some(last_update)) => {
                    let elapsed = time.saturating_duration_since(last_update).as_secs_f32();
                    let weight = 1.0 - (-elapsed / SMOOTHING).exp();
                    previous + (inverse_depth - previous) * weight
                }
                _ => inverse_depth,
            });
            self.last_update = Some(time);
        }
        self.depth()
    }

    /// The smoothed depth.
    pub fn depth(&self) -> Option<f32> {
        self.inverse_depth.map(|inverse_depth| 1.0 / inverse_depth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn smoothing_follows_the_depth() {
        let mut smoothing = DepthSmoothing::default();
        let start = Instant::now();
        assert_eq!(smoothing.update(None, start), None);
        assert_eq!(smoothing.update(Some(2.0), start), Some(2.0));
        // A hand at 0.5 m: one frame later the depth has barely moved, half a second
        // later it has arrived.
        let frame = Duration::from_millis(18);
        let after_one_frame = smoothing.update(Some(0.5), start + frame).unwrap();
        assert!(after_one_frame > 1.4, "{after_one_frame}");
        let mut depth = after_one_frame;
        for i in 2..=28 {
            depth = smoothing.update(Some(0.5), start + frame * i).unwrap();
        }
        assert!((depth - 0.5).abs() < 0.02, "{depth}");
        // Frames without a measure keep the depth.
        assert_eq!(smoothing.update(None, start + frame * 29), Some(depth));
        // Infinite depth is clamped.
        let mut smoothing = DepthSmoothing::default();
        assert_eq!(
            smoothing.update(Some(f32::INFINITY), start),
            Some(MAX_CENTER_DEPTH)
        );
    }
}
