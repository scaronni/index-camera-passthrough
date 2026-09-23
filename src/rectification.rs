//! Stereo rectification of the Hmd cameras.
//!
//! Both camera images are resampled as seen by two virtual pinhole cameras with the
//! same orientation: the x axis is along the baseline and they look straight ahead.
//! In rectified images, a point of the scene appears on the same row in both images,
//! which is what stereo matching needs, and the images are no longer rotated by the
//! toe-out of the physical cameras.
//!
//! Axis conventions:
//! - the Steam lighthouse calibration and the Hmd frame of OpenVR use the OpenGL
//!   convention: x right, y up, looking along -z.
//! - image coordinates, and the camera rays used to sample them, use the OpenCV
//!   convention: x right, y down, looking along +z.

use nalgebra::{Isometry3, Matrix3, Rotation3, Translation3, UnitQuaternion, Vector3};

use crate::vrapi::{Extrinsics, StereoCamera};

/// Field of view of the rectified images, both horizontally and vertically.
pub const RECTIFIED_FOV: f64 = 120.0_f64.to_radians();

/// Conversion between the OpenGL and the OpenCV camera axes, it is its own inverse.
fn flip_yz() -> Matrix3<f64> {
    Matrix3::from_diagonal(&Vector3::new(1.0, -1.0, -1.0))
}

impl Extrinsics {
    /// Rotation from this frame to the tracking frame.
    fn rotation(&self) -> Matrix3<f64> {
        // The calibration is not exactly orthonormal.
        let z = Vector3::from(self.plus_z).normalize();
        let x = Vector3::from(self.plus_x);
        let x = (x - z * z.dot(&x)).normalize();
        Matrix3::from_columns(&[x, z.cross(&x), z])
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectification {
    /// Poses of the rectified left and right cameras in the Hmd frame, OpenGL axes.
    pub camera_to_head: [Isometry3<f64>; 2],
    /// Rotations from rays of the rectified cameras to rays of the physical cameras,
    /// OpenCV axes.
    pub rectified_to_camera: [Matrix3<f64>; 2],
    /// Focal length of the rectified images divided by their size. The optical
    /// center is the center of the images.
    pub focal: f64,
}

impl Rectification {
    pub fn new(calib: &StereoCamera, fov: f64) -> Self {
        let head_rotation = calib.head.rotation();
        let head_position = Vector3::from(calib.head.position);
        // Rotation and position of the physical cameras in the Hmd frame.
        let [left, right] = [&calib.left, &calib.right].map(|camera| {
            let extrinsics = &camera.extrinsics;
            (
                head_rotation.transpose() * extrinsics.rotation(),
                head_rotation.transpose() * (Vector3::from(extrinsics.position) - head_position),
            )
        });

        let x = (right.1 - left.1).normalize();
        // The cameras look along -z: average their z axes, and make the result
        // perpendicular to the baseline.
        let z = left.0.column(2) + right.0.column(2);
        let z = (z - x * x.dot(&z)).normalize();
        let rectified = Matrix3::from_columns(&[x, z.cross(&x), z]);
        let rotation =
            UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(rectified));

        Self {
            camera_to_head: [left, right]
                .map(|(_, position)| Isometry3::from_parts(Translation3::from(position), rotation)),
            rectified_to_camera: [left, right]
                .map(|(camera, _)| flip_yz() * camera.transpose() * rectified * flip_yz()),
            focal: 0.5 / (fov / 2.0).tan(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrapi::{Camera, Distort, Intrinsics, TrackedCamera};
    use nalgebra::Point3;

    fn camera(
        name: Camera,
        plus_x: [f64; 3],
        plus_z: [f64; 3],
        position: [f64; 3],
        center: [f64; 2],
        focal: [f64; 2],
        coeffs: [f64; 4],
    ) -> TrackedCamera {
        TrackedCamera {
            extrinsics: Extrinsics {
                plus_x,
                plus_z,
                position,
            },
            intrinsics: Intrinsics {
                center_x: center[0],
                center_y: center[1],
                focal_x: focal[0],
                focal_y: focal[1],
                height: 960.0,
                width: 960.0,
                distort: Distort { coeffs },
            },
            name,
        }
    }

    /// Calibration of a Valve Index.
    fn index_calibration() -> StereoCamera {
        StereoCamera {
            left: camera(
                Camera::Left,
                [
                    -0.9968744968432377,
                    0.0052457319015594833,
                    0.0788277713968661,
                ],
                [
                    -0.078679570653378758,
                    0.024150286109425472,
                    -0.99660743852963707,
                ],
                [
                    0.067489996552467346,
                    -0.032940000295639038,
                    0.068810001015663147,
                ],
                [499.38085414541564, 467.30836604143212],
                [412.72261066761439, 413.07730954889149],
                [
                    0.18758589369332876,
                    0.0355289954702738,
                    -0.20794098802633154,
                    0.083014859458607135,
                ],
            ),
            right: camera(
                Camera::Right,
                [
                    -0.99697262694164124,
                    0.021609662098656036,
                    -0.074691239243922264,
                ],
                [
                    0.075269536029811929,
                    0.027314176936646284,
                    -0.99678915034953208,
                ],
                [
                    -0.067489996552467346,
                    -0.032940000295639038,
                    0.068810001015663147,
                ],
                [490.86213045637214, 475.00403430283677],
                [415.37800912314083, 415.63712904229328],
                [
                    0.18781600385291031,
                    0.039087521376590433,
                    -0.21156591163939695,
                    0.083471942573671312,
                ],
            ),
            head: Extrinsics {
                plus_x: [-1.0, 0.0, 0.0],
                plus_z: [0.0, -0.0, -1.0],
                position: [0.0, 0.0, -0.01092000026255846],
            },
        }
    }

    /// Pixel of a point in the Hmd frame in a rectified image, in OpenCV axes.
    fn project_rectified(r: &Rectification, eye: usize, point: &Point3<f64>) -> [f64; 2] {
        let p = flip_yz() * r.camera_to_head[eye].inverse_transform_point(point).coords;
        [r.focal * p.x / p.z + 0.5, r.focal * p.y / p.z + 0.5]
    }

    #[test]
    fn cameras_in_hmd_frame() {
        let r = Rectification::new(&index_calibration(), RECTIFIED_FOV);
        let [left, right] = r.camera_to_head.map(|pose| pose.translation.vector);
        // The cameras are in front of the eyes, a bit below them, 13.5 cm apart.
        assert!(left.x < 0.0 && right.x > 0.0);
        assert!(left.y < 0.0 && left.z < 0.0);
        assert!(((right - left).norm() - 0.135).abs() < 1e-3);
        // The rectified cameras look straight ahead, along -z.
        let forward = r.camera_to_head[0].rotation * Vector3::new(0.0, 0.0, -1.0);
        assert!(forward.z < -0.99, "{forward}");
    }

    #[test]
    fn rows_match() {
        let r = Rectification::new(&index_calibration(), RECTIFIED_FOV);
        for point in [
            [0.0, 0.0, -2.0],
            [-1.0, 0.5, -1.5],
            [0.8, -0.6, -0.7],
            [0.1, 1.0, -3.0],
        ] {
            let point = Point3::from(point);
            let [l, r] = [0, 1].map(|eye| project_rectified(&r, eye, &point));
            assert!((l[1] - r[1]).abs() < 1e-9, "{point}: {l:?} {r:?}");
            // Positive disparity: points appear further right in the left image.
            assert!(l[0] > r[0], "{point}: {l:?} {r:?}");
        }
    }

    #[test]
    fn rays_reach_the_physical_cameras() {
        let calib = index_calibration();
        let r = Rectification::new(&calib, RECTIFIED_FOV);
        let point = Point3::new(0.3, -0.2, -1.0);
        for (eye, camera) in [calib.left, calib.right].iter().enumerate() {
            // Ray of the point in the rectified camera, turned into the physical camera.
            let [u, v] = project_rectified(&r, eye, &point);
            let ray = Vector3::new((u - 0.5) / r.focal, (v - 0.5) / r.focal, 1.0);
            let ray = (r.rectified_to_camera[eye] * ray).normalize();
            // Same ray computed directly from the physical camera pose.
            let head = calib.head.rotation();
            let rotation = head.transpose() * camera.extrinsics.rotation();
            let position = head.transpose()
                * (Vector3::from(camera.extrinsics.position) - Vector3::from(calib.head.position));
            let expected =
                (flip_yz() * rotation.transpose() * (point.coords - position)).normalize();
            assert!((ray - expected).norm() < 1e-9, "{ray} {expected}");
        }
    }
}
